//! Shared skill mention grammar. Rendering and provider input use the same spans.

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

#[cfg(test)]
mod tests {
    use super::*;
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
                .all(|mention| &text[mention.range.clone()] == format!("${}", mention.name))
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
        let text = format!("${} $short", "a.".repeat(100000));
        assert_eq!(skill_mentions(&text, &["short", "short"]).len(), 1);
    }
}
