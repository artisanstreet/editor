//! The mention grammar the composer paints and Forge hands to engines:
//! `$skill` and `@connector` names, matched against the installed ones,
//! `@thread title`s, matched against the threads, and the `/command` a
//! message can start with. Rendering and provider input use
//! the same spans.

use std::{collections::HashSet, ops::Range};

/// Canonical skill identifiers are bounded so an unmatched `$` token cannot
/// make either consumer hash arbitrarily long prefixes.
pub const MAX_SKILL_NAME_BYTES: usize = 255;
const TERMINATORS: &[char] = &[',', '.', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    pub name: String,
    /// Includes the sigil, in UTF-8 byte coordinates.
    pub range: Range<usize>,
}

/// All known mentions in source order, preserving repeated painted spans.
/// A dot may belong to a qualified name or terminate a shorter name: the
/// longest registered name followed by a terminator wins.
#[must_use]
pub fn skill_mentions(text: &str, names: &[&str]) -> Vec<Mention> {
    named_mentions(text, names, '$')
}

#[must_use]
pub fn valid_mention_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SKILL_NAME_BYTES
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_whitespace)
}

/// Shared registered-name grammar for skill and connector highlighting.
#[must_use]
pub fn named_mentions(text: &str, names: &[&str], sigil: char) -> Vec<Mention> {
    MentionNames::new(names).mentions(text, sigil)
}

/// Compiled names shared across edits until the catalog changes.
#[derive(Clone, Debug, Default)]
pub struct MentionNames {
    known: HashSet<String>,
    longest: usize,
}
impl MentionNames {
    #[must_use]
    pub fn new(names: &[&str]) -> Self {
        let known: HashSet<String> = names
            .iter()
            .copied()
            .filter(|name| valid_mention_name(name))
            .map(str::to_owned)
            .collect();
        let longest = known.iter().map(String::len).max().unwrap_or(0);
        Self { known, longest }
    }

    #[must_use]
    pub fn mentions(&self, text: &str, sigil: char) -> Vec<Mention> {
        let known = &self.known;
        let longest = self.longest;
        if longest == 0 {
            return Vec::new();
        }
        let mut mentions = Vec::new();
        for (start, character) in text.char_indices() {
            if character != sigil
                || !text[..start]
                    .chars()
                    .next_back()
                    .is_none_or(char::is_whitespace)
            {
                continue;
            }
            let rest = &text[start + sigil.len_utf8()..];
            let mut matched = None;
            for (index, next) in rest.char_indices() {
                if index > longest {
                    break;
                }
                if (next.is_whitespace() || TERMINATORS.contains(&next))
                    && known.contains(&rest[..index])
                {
                    matched = Some(index);
                }
                if next.is_whitespace() {
                    break;
                }
            }
            if rest.len() <= longest && known.contains(rest) {
                matched = Some(rest.len());
            }
            if let Some(length) = matched {
                mentions.push(Mention {
                    name: rest[..length].to_owned(),
                    range: start..start + sigil.len_utf8() + length,
                });
            }
        }
        mentions
    }
}

/// Longer titles (a first message's long first line) can't be mentioned.
pub const MAX_THREAD_TITLE_BYTES: usize = 512;

/// A thread title a mention can name: one line, its own trimmed form and
/// bounded.
#[must_use]
pub fn valid_thread_title(title: &str) -> bool {
    !title.is_empty()
        && title.len() <= MAX_THREAD_TITLE_BYTES
        && title.trim() == title
        && !title.contains(['\n', '\r'])
}

/// `@title` mentions of threads in source order. Titles hold spaces, so the
/// longest title that follows an `@` at the start of a word and ends at a
/// word end wins.
#[must_use]
pub fn thread_mentions(text: &str, titles: &[&str]) -> Vec<Mention> {
    let mut mentions = Vec::new();
    for (start, character) in text.char_indices() {
        if character != '@'
            || !text[..start]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace)
        {
            continue;
        }
        let rest = &text[start + 1..];
        let matched = titles
            .iter()
            .filter(|title| valid_thread_title(title) && rest.starts_with(**title))
            .filter(|title| {
                rest[title.len()..]
                    .chars()
                    .next()
                    .is_none_or(|next| next.is_whitespace() || TERMINATORS.contains(&next))
            })
            .max_by_key(|title| title.len());
        if let Some(title) = matched {
            mentions.push(Mention {
                name: (*title).to_owned(),
                range: start..start + 1 + title.len(),
            });
        }
    }
    mentions
}

/// The `/command` a message starts with, sigil included.
///
/// `/goal`, `/ultracode`, `/plugin:skill`: any engine's command reads the same, so
/// nothing here knows which commands exist; a path (`/home/me`) is no
/// command.
#[must_use]
pub fn leading_command(text: &str) -> Option<Range<usize>> {
    let start = text.len() - text.trim_start().len();
    let rest = text[start..].strip_prefix('/')?;
    let name = rest
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, ':' | '_' | '-')))
        .unwrap_or(rest.len());
    let ends = rest[name..].chars().next().is_none_or(char::is_whitespace);
    (name > 0 && ends).then(|| start..start + 1 + name)
}

/// Commands Artisan gives a meaning of its own, so they stand out where
/// they're typed and sent; every other command reads as plain text.
pub const SPECIAL_COMMANDS: &[&str] = &["goal", "ultracode", "ultrathink"];

/// The [`leading_command`] when it is one of [`SPECIAL_COMMANDS`].
#[must_use]
pub fn special_command(text: &str) -> Option<Range<usize>> {
    leading_command(text)
        .filter(|range| SPECIAL_COMMANDS.contains(&&text[range.start + 1..range.end]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_message_starting_with_a_command_names_it_and_a_path_does_not() {
        for (text, command) in [
            ("/goal ship it", Some("/goal")),
            ("  /ultracode build", Some("/ultracode")),
            ("/compact", Some("/compact")),
            ("/cloudflare:wrangler deploy", Some("/cloudflare:wrangler")),
            ("/home/sander is full", None),
            ("/ alone", None),
            ("see /goal later", None),
            ("/goal, now", None),
        ] {
            assert_eq!(
                leading_command(text).map(|range| &text[range]),
                command,
                "{text}"
            );
        }
    }
    #[test]
    fn thread_titles_with_spaces_take_the_longest_match_at_a_word_end() {
        let text = "see @Fix sidebar lag, then @Fix and email@Fix or @Fixed";
        let mentions = thread_mentions(text, &["Fix", "Fix sidebar lag", "Fix sidebar"]);
        assert_eq!(
            mentions
                .iter()
                .map(|mention| &text[mention.range.clone()])
                .collect::<Vec<_>>(),
            ["@Fix sidebar lag", "@Fix"]
        );
    }
    #[test]
    fn qualified_names_and_punctuation_share_one_longest_match() {
        let text = "$docs.web, then $docs. Again $docs.web!";
        let mentions = skill_mentions(text, &["docs", "docs.web"]);
        assert_eq!(
            mentions
                .iter()
                .map(|mention| mention.name.as_str())
                .collect::<Vec<_>>(),
            ["docs.web", "docs", "docs.web"]
        );
        assert!(
            mentions
                .iter()
                .all(|mention| text[mention.range.clone()] == format!("${}", mention.name))
        );
    }
    #[test]
    fn unicode_byte_ranges_and_word_boundaries_are_stable() {
        let text = "é $review; email$review $reviewer $🦀";
        assert_eq!(
            skill_mentions(text, &["review", "🦀"])
                .iter()
                .map(|mention| mention.name.as_str())
                .collect::<Vec<_>>(),
            ["review", "🦀"]
        );
    }
    #[test]
    fn long_unmatched_tokens_and_duplicate_names_remain_bounded() {
        let text = format!("${} $short", "a.".repeat(100_000));
        assert_eq!(skill_mentions(&text, &["short", "short"]).len(), 1);
    }
}
