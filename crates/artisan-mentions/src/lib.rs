#![expect(
    clippy::string_slice,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]

use std::{collections::HashSet, ops::Range};

pub const MAX_SKILL_NAME_BYTES: usize = 255;
const TERMINATORS: &[char] = &[',', '.', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    pub name: String,
    pub range: Range<usize>,
}

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

#[must_use]
pub fn named_mentions(text: &str, names: &[&str], sigil: char) -> Vec<Mention> {
    MentionNames::new(names).mentions(text, sigil)
}

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

pub const MAX_THREAD_TITLE_BYTES: usize = 512;

#[must_use]
pub fn valid_thread_title(title: &str) -> bool {
    !title.is_empty()
        && title.len() <= MAX_THREAD_TITLE_BYTES
        && title.trim() == title
        && !title.contains(['\n', '\r'])
}

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

pub const SPECIAL_COMMANDS: &[&str] = &["goal", "ultracode", "ultrathink"];

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
