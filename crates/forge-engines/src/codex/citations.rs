//! Codex's private citation tokens are provider bookkeeping, not Markdown links.
//! The app-server currently supplies no destination for these unresolved IDs.

use crate::{EngineEvent, LiveKind};
use artisan_api::ItemKind;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Citations {
    streams: HashMap<String, Stream>,
}
#[derive(Default)]
struct Stream {
    pending: String,
    delimiter: Option<usize>,
    backticks: usize,
    escaped: bool,
}
impl Citations {
    #[expect(
        clippy::expect_used,
        reason = "the adapter emits inline text; storage happens after normalising"
    )]
    pub(super) fn normalize(&mut self, events: Vec<EngineEvent>) -> Vec<EngineEvent> {
        events
            .into_iter()
            .filter_map(|mut event| {
                match &mut event {
                    EngineEvent::Live {
                        target: LiveKind::Text { key },
                        text,
                        append,
                    } => {
                        let stream = self.streams.entry(key.clone()).or_default();
                        if !*append {
                            *stream = Stream::default();
                        }
                        *text = stream.push(text);
                        if text.is_empty() {
                            return None;
                        }
                    }
                    EngineEvent::Item {
                        key,
                        kind: ItemKind::AssistantMessage { text, .. },
                        status,
                        ..
                    } => {
                        let canonical = text
                            .inline()
                            .expect("adapter text is inline before storage");
                        *text = Stream::default().push(canonical).into();
                        if status.is_terminal() {
                            self.streams.remove(key);
                        }
                    }
                    EngineEvent::Status { key, status } if status.is_terminal() => {
                        self.streams.remove(key);
                    }
                    EngineEvent::Session { .. }
                    | EngineEvent::Model { .. }
                    | EngineEvent::Title { .. }
                    | EngineEvent::Item { .. }
                    | EngineEvent::Image { .. }
                    | EngineEvent::Status { .. }
                    | EngineEvent::Live { .. }
                    | EngineEvent::Usage(_)
                    | EngineEvent::Commands(_)
                    | EngineEvent::InputAccepted { .. } => {}
                }
                Some(event)
            })
            .collect()
    }
}
impl Stream {
    fn push(&mut self, delta: &str) -> String {
        let mut output = String::with_capacity(delta.len());
        for character in delta.chars() {
            if self.pending.is_empty() {
                if character == '`' && !self.escaped {
                    self.backticks += 1;
                    output.push(character);
                    continue;
                }
                if self.backticks > 0 {
                    self.delimiter = match self.delimiter {
                        Some(delimiter) if delimiter == self.backticks => None,
                        Some(delimiter) => Some(delimiter),
                        None => Some(self.backticks),
                    };
                    self.backticks = 0;
                }
                if character == '\u{e200}' && self.delimiter.is_none() {
                    self.pending.push(character);
                } else {
                    output.push(character);
                }
                self.escaped = character == '\\' && !self.escaped;
            } else {
                self.pending.push(character);
                if character == '\u{e201}' {
                    if self.pending.starts_with("\u{e200}cite\u{e202}") {
                        output.push_str(" [source unavailable]");
                    } else {
                        output.push_str(&self.pending);
                    }
                    self.pending.clear();
                } else if self.pending.len() > 4096 {
                    output.push_str(" [invalid provider citation]");
                    self.pending.clear();
                }
            }
        }
        output
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_private_markers_never_escape_into_normal_prose() {
        let mut stream = Stream::default();
        assert_eq!(stream.push("Fact.\u{e200}cite\u{e202}turn0"), "Fact.");
        assert_eq!(
            stream.push("search0\u{e201} Next."),
            " [source unavailable] Next."
        );
        let literal = "`\u{e200}cite\u{e202}turn0\u{e201}`";
        assert_eq!(Stream::default().push(literal), literal);
        let literal = "``\u{e200}cite\u{e202}turn0\u{e201}``";
        assert_eq!(Stream::default().push(literal), literal);
    }
}
