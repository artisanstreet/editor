//! Providers send prose and heading deltas; the Client receives one concise line.

use crate::{EngineEvent, LiveKind};
use pulldown_cmark::{Event, Parser};
const MAX_RAW_BYTES: usize = 16 * 1024;
const MAX_LABEL_CHARS: usize = 200;

#[derive(Default)]
pub struct Summary {
    raw: String,
    shown: String,
}
impl Summary {
    pub(crate) fn normalize(
        &mut self,
        events: Vec<EngineEvent>,
        first_clause: bool,
    ) -> Vec<EngineEvent> {
        events
            .into_iter()
            .filter_map(|event| {
                let EngineEvent::Live {
                    target: LiveKind::Reasoning,
                    text,
                    append,
                } = event
                else {
                    return Some(event);
                };
                if !append {
                    self.raw.clear();
                }
                if first_clause {
                    let mut take = (MAX_RAW_BYTES - self.raw.len()).min(text.len());
                    while !text.is_char_boundary(take) {
                        take -= 1;
                    }
                    self.raw.push_str(&text[..take]);
                } else if text.len() > MAX_RAW_BYTES {
                    let mut start = text.len() - MAX_RAW_BYTES;
                    while !text.is_char_boundary(start) {
                        start += 1;
                    }
                    self.raw.clear();
                    self.raw.push_str(&text[start..]);
                } else {
                    self.raw.push_str(&text);
                }
                if self.raw.len() > MAX_RAW_BYTES {
                    if first_clause {
                        let mut end = MAX_RAW_BYTES;
                        while !self.raw.is_char_boundary(end) {
                            end -= 1;
                        }
                        self.raw.truncate(end);
                    } else {
                        let mut start = self.raw.len() - MAX_RAW_BYTES;
                        while !self.raw.is_char_boundary(start) {
                            start += 1;
                        }
                        self.raw.drain(..start);
                    }
                }
                let cleared = text.is_empty() && !append;
                let line = label(&self.raw, first_clause);
                if line == self.shown && !cleared {
                    return None;
                }
                self.shown.clone_from(&line);
                Some(EngineEvent::Live {
                    target: LiveKind::Reasoning,
                    text: line,
                    append: false,
                })
            })
            .collect()
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only text and breaks make plain text; every other markdown event is dropped"
)]
fn plain(markdown: &str) -> String {
    let mut text = String::new();
    let mut end = markdown.len().min(2048);
    while !markdown.is_char_boundary(end) {
        end -= 1;
    }
    for event in Parser::new(&markdown[..end]) {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(&value),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn clause_end(line: &str, first_clause: bool) -> Option<usize> {
    let mut words = 0;
    let mut in_word = false;
    line.char_indices().find_map(|(index, character)| {
        if character.is_whitespace() {
            in_word = false;
            return None;
        }
        if !in_word {
            words += 1;
            in_word = true;
        }
        let remainder = &line[index + character.len_utf8()..];
        if !(remainder.is_empty() || remainder.starts_with(char::is_whitespace)) {
            return None;
        }
        let separator = if first_clause {
            matches!(character, ':' | ';' | '—' | '!' | '?')
                || character == ',' && words >= 3
                || character == '.'
        } else {
            matches!(character, '.' | '!' | '?')
        };
        (separator && index > 0).then_some(index)
    })
}

fn label(raw: &str, first_clause: bool) -> String {
    let line = if first_clause {
        raw.lines()
            .filter(|line| !line.trim().starts_with("```") && !line.trim().starts_with("~~~"))
            .map(|line| plain(clause_end(line, true).map_or(line, |end| &line[..end])))
            .find(|line| !line.is_empty())
            .unwrap_or_default()
    } else {
        let heading = raw.rsplit("\n\n").find_map(|section| {
            let line = section.lines().next()?.trim();
            (line.starts_with("**") || line.starts_with('#'))
                .then(|| plain(line.trim_matches('*').trim_start_matches('#').trim()))
        });
        heading.unwrap_or_else(|| {
            raw.rsplit("\n\n")
                .find(|section| !section.trim().is_empty())
                .map(plain)
                .unwrap_or_default()
        })
    };
    let cut = clause_end(&line, first_clause);
    let line = cut.map_or(line.as_str(), |index| &line[..index]).trim();
    let mut label = line.chars().take(MAX_LABEL_CHARS).collect::<String>();
    if line.chars().nth(MAX_LABEL_CHARS).is_some() {
        label.pop();
        label.push('…');
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_clause_keeps_arithmetic_delimiters_from_later_prose() {
        assert_eq!(
            label("17*23: breaking it into 17*20 gives 340", true),
            "17*23"
        );
    }

    #[test]
    fn provider_reasoning_is_bounded_canonical_replacement_text() {
        let mut summary = Summary::default();
        let events = summary.normalize(
            vec![EngineEvent::Live {
                target: LiveKind::Reasoning,
                text: "**Check ownership**\n\nI will check the run.".into(),
                append: true,
            }],
            false,
        );
        assert_eq!(
            events,
            vec![EngineEvent::Live {
                target: LiveKind::Reasoning,
                text: "Check ownership".into(),
                append: false
            }]
        );
        summary.normalize(
            vec![EngineEvent::Live {
                target: LiveKind::Reasoning,
                text: "x".repeat(100_000),
                append: true,
            }],
            false,
        );
        assert!(summary.raw.len() <= MAX_RAW_BYTES);
        assert!(summary.shown.chars().count() <= MAX_LABEL_CHARS);
        let clear = summary.normalize(
            vec![EngineEvent::Live {
                target: LiveKind::Reasoning,
                text: String::new(),
                append: false,
            }],
            false,
        );
        assert_eq!(
            clear,
            vec![EngineEvent::Live {
                target: LiveKind::Reasoning,
                text: String::new(),
                append: false
            }]
        );
    }
}
