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
fn named_mentions(text: &str, names: &[&str], sigil: char) -> Vec<Mention> {
    MentionNames::new(names).mentions(text, sigil)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
        let mut mentions = Vec::new();
        self.each_start(text, sigil, |start, names| {
            if let Some(name) = names.last() {
                mentions.push(Mention {
                    name: (*name).to_owned(),
                    range: start..start + sigil.len_utf8() + name.len(),
                });
            }
        });
        mentions
    }

    #[must_use]
    fn candidates(&self, text: &str, sigil: char) -> Vec<Mention> {
        let mut mentions = Vec::new();
        self.each_start(text, sigil, |start, names| {
            mentions.extend(names.iter().rev().map(|name| Mention {
                name: (*name).to_owned(),
                range: start..start + sigil.len_utf8() + name.len(),
            }));
        });
        mentions
    }

    fn each_start<'a>(&self, text: &'a str, sigil: char, mut found: impl FnMut(usize, &[&'a str])) {
        let known = &self.known;
        let longest = self.longest;
        if longest == 0 {
            return;
        }
        let mut names = Vec::new();
        let mut previous = None;
        for (start, character) in text.char_indices() {
            let after_space = previous.is_none_or(char::is_whitespace);
            previous = Some(character);
            if character != sigil || !after_space {
                continue;
            }
            let (_, rest) = text.split_at(start + sigil.len_utf8());
            names.clear();
            for (index, next) in rest.char_indices() {
                if index > longest {
                    break;
                }
                let (name, _) = rest.split_at(index);
                if (next.is_whitespace() || TERMINATORS.contains(&next)) && known.contains(name) {
                    names.push(name);
                }
                if next.is_whitespace() {
                    break;
                }
            }
            if rest.len() <= longest && known.contains(rest) {
                names.push(rest);
            }
            found(start, &names);
        }
    }
}

const MAX_THREAD_TITLE_BYTES: usize = 512;

#[must_use]
pub fn valid_thread_title(title: &str) -> bool {
    !title.is_empty()
        && title.len() <= MAX_THREAD_TITLE_BYTES
        && title.trim() == title
        && !title.contains(['\n', '\r'])
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadTitles {
    sorted: Vec<String>,
    longest: usize,
}

impl ThreadTitles {
    #[must_use]
    pub fn new<T: AsRef<str>>(titles: &[T]) -> Self {
        let mut sorted: Vec<String> = titles
            .iter()
            .map(AsRef::as_ref)
            .filter(|title| valid_thread_title(title))
            .map(str::to_owned)
            .collect();
        sorted.sort_unstable();
        sorted.dedup();
        let longest = sorted.iter().map(String::len).max().unwrap_or(0);
        Self { sorted, longest }
    }

    fn matches_at<'s>(&'s self, rest: &str, mut matched: impl FnMut(&'s str)) {
        let bytes = rest.as_bytes();
        let (mut low, mut high) = (0, self.sorted.len());
        for depth in 0..=bytes.len().min(self.longest) {
            if low >= high {
                break;
            }
            let shortest = &self.sorted[low];
            if shortest.len() == depth {
                if rest
                    .get(depth..)
                    .and_then(|after| after.chars().next())
                    .is_none_or(|next| next.is_whitespace() || TERMINATORS.contains(&next))
                {
                    matched(shortest.as_str());
                }
                low += 1;
            }
            let Some(&byte) = bytes.get(depth) else {
                break;
            };
            let range = &self.sorted[low..high];
            let first = range.partition_point(|title| title.as_bytes()[depth] < byte);
            let last = range.partition_point(|title| title.as_bytes()[depth] <= byte);
            (low, high) = (low + first, low + last);
        }
    }

    #[must_use]
    pub fn mentions(&self, text: &str) -> Vec<Mention> {
        if self.sorted.is_empty() {
            return Vec::new();
        }
        let mut mentions = Vec::new();
        let mut previous = None;
        for (start, character) in text.char_indices() {
            let after_space = previous.is_none_or(char::is_whitespace);
            previous = Some(character);
            if character != '@' || !after_space {
                continue;
            }
            let (_, rest) = text.split_at(start + 1);
            let mut longest = None;
            self.matches_at(rest, |title| longest = Some(title));
            if let Some(title) = longest {
                mentions.push(Mention {
                    name: title.to_owned(),
                    range: start..start + 1 + title.len(),
                });
            }
        }
        mentions
    }

    #[must_use]
    fn candidates(&self, text: &str) -> Vec<Mention> {
        if self.sorted.is_empty() {
            return Vec::new();
        }
        let mut mentions = Vec::new();
        let mut titles = Vec::new();
        let mut previous = None;
        for (start, character) in text.char_indices() {
            let after_space = previous.is_none_or(char::is_whitespace);
            previous = Some(character);
            if character != '@' || !after_space {
                continue;
            }
            let (_, rest) = text.split_at(start + 1);
            titles.clear();
            self.matches_at(rest, |title| titles.push(title));
            mentions.extend(titles.iter().rev().map(|title| Mention {
                name: (*title).to_owned(),
                range: start..start + 1 + title.len(),
            }));
        }
        mentions
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MentionKind {
    Skill,
    Thread,
}

impl MentionKind {
    #[must_use]
    pub const fn sigil(self) -> char {
        match self {
            Self::Skill => '$',
            Self::Thread => '@',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MessageMention {
    pub kind: MentionKind,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageVocabulary {
    skills: MentionNames,
    threads: ThreadTitles,
}

impl MessageVocabulary {
    #[must_use]
    pub fn new<S: AsRef<str>, T: AsRef<str>>(skills: &[S], threads: &[T]) -> Self {
        let skills: Vec<&str> = skills.iter().map(AsRef::as_ref).collect();
        Self {
            skills: MentionNames::new(&skills),
            threads: ThreadTitles::new(threads),
        }
    }

    #[must_use]
    pub fn mentions(&self, text: &str) -> Vec<MessageMention> {
        exclusive(self.candidates(text))
    }

    #[must_use]
    pub fn candidates(&self, text: &str) -> Vec<MessageMention> {
        let skills = self
            .skills
            .candidates(text, MentionKind::Skill.sigil())
            .into_iter()
            .map(|mention| (MentionKind::Skill, mention));
        let threads = self
            .threads
            .candidates(text)
            .into_iter()
            .map(|mention| (MentionKind::Thread, mention));
        skills
            .chain(threads)
            .map(|(kind, mention)| MessageMention {
                kind,
                range: mention.range,
            })
            .collect()
    }
}

#[must_use]
pub fn exclusive(mut found: Vec<MessageMention>) -> Vec<MessageMention> {
    found.sort_by_key(|mention| (mention.range.start, std::cmp::Reverse(mention.range.end)));
    let mut end = 0;
    found.retain(|mention| {
        let keep = mention.range.start >= end;
        if keep {
            end = mention.range.end;
        }
        keep
    });
    found
}

#[must_use]
fn leading_command(text: &str) -> Option<Range<usize>> {
    let trimmed = text.trim_start();
    let start = text.len() - trimmed.len();
    let rest = trimmed.strip_prefix('/')?;
    let name = rest
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, ':' | '_' | '-')))
        .unwrap_or(rest.len());
    let (command, after) = rest.split_at(name);
    let ends = after.chars().next().is_none_or(char::is_whitespace);
    (!command.is_empty() && ends).then(|| start..start + 1 + name)
}

const SPECIAL_COMMANDS: &[&str] = &["goal", "ultracode", "ultrathink"];

#[must_use]
pub fn special_command(text: &str) -> Option<Range<usize>> {
    leading_command(text).filter(|range| {
        text.get(range.start + 1..range.end)
            .is_some_and(|name| SPECIAL_COMMANDS.contains(&name))
    })
}

#[cfg(test)]
#[path = "../tests/lib/unit.rs"]
mod tests;
