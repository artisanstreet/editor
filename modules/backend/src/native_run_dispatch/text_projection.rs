//! Assistant text projection.
//!
//! Retains each provider message separately for durable projection. The total
//! run budget includes every part, including corrected and empty parts, without
//! rebuilding the entire transcript for each streamed token.

use artisan_domain::AssistantBody;

use crate::engine_owner::observation::{TextDelta, TextSnapshot};

const MAX_ASSISTANT_TEXT_PARTS: usize = 128;
const FIXTURE_TEXT_PART_ID: &str = "fixture-text-part";

/// Retain the existing aggregate budget, including the paragraph separators
/// a text export would insert between messages. Storage keeps separate rows.
const PART_SEPARATOR: &str = "\n\n";

struct AssistantTextPart {
    part_id: String,
    text: String,
}

#[derive(Default)]
pub(super) struct OrderedAssistantText {
    parts: Vec<AssistantTextPart>,
    /// Exact length of [`Self::body`] output: part bytes plus separators.
    /// Separator bytes count against [`AssistantBody::MAX_BYTES`] so the bound
    /// stays honest once distinct parts no longer concatenate directly.
    total_bytes: usize,
}

/// Separator bytes contributed by `nonempty_parts` nonempty parts.
fn part_separator_bytes(nonempty_parts: usize) -> usize {
    nonempty_parts
        .saturating_sub(1)
        .saturating_mul(PART_SEPARATOR.len())
}

impl OrderedAssistantText {
    pub(super) fn part_body(&self, part_id: &str) -> &str {
        self.parts
            .iter()
            .find(|part| part.part_id == part_id)
            .map_or("", |part| part.text.as_str())
    }

    fn nonempty_part_count(&self) -> usize {
        self.parts
            .iter()
            .filter(|part| !part.text.is_empty())
            .count()
    }

    pub(super) fn append_delta(&mut self, delta: &TextDelta) -> Option<()> {
        self.append(
            delta.part_id().unwrap_or(FIXTURE_TEXT_PART_ID),
            delta.delta(),
        )
    }

    pub(super) fn replace_snapshot(&mut self, snapshot: &TextSnapshot) -> Option<()> {
        self.replace(snapshot.part_id(), snapshot.text())
    }

    /// Appends exact provider bytes within one message and charges the shared
    /// run budget before mutation. No other message body is copied or changed.
    fn append(&mut self, part_id: &str, text: &str) -> Option<()> {
        if part_id.is_empty() {
            return None;
        }
        let Some(index) = self.parts.iter().position(|part| part.part_id == part_id) else {
            if self.parts.len() >= MAX_ASSISTANT_TEXT_PARTS {
                return None;
            }
            let mut added = text.len();
            if !text.is_empty() && self.nonempty_part_count() > 0 {
                added = added.checked_add(PART_SEPARATOR.len())?;
            }
            let next_total = self.total_bytes.checked_add(added)?;
            if next_total > AssistantBody::MAX_BYTES {
                return None;
            }
            self.parts.push(AssistantTextPart {
                part_id: part_id.to_owned(),
                text: text.to_owned(),
            });
            self.total_bytes = next_total;
            return Some(());
        };
        let mut added = text.len();
        if self.parts[index].text.is_empty() && !text.is_empty() && self.nonempty_part_count() > 0 {
            added = added.checked_add(PART_SEPARATOR.len())?;
        }
        let next_total = self.total_bytes.checked_add(added)?;
        if next_total > AssistantBody::MAX_BYTES {
            return None;
        }
        self.parts[index].text.push_str(text);
        self.total_bytes = next_total;
        Some(())
    }

    /// Replaces one logical provider part wholesale.
    ///
    /// The aggregate bound is recharged with the part's previous bytes and its
    /// previous separator share before the replacement (plus its new share) is
    /// admitted, so corrections can shrink, grow, or empty a part without
    /// leaking separator bytes. Callers other than tests and snapshot
    /// handling must not use this for streaming deltas: same-part streaming
    /// stays byte-exact through [`Self::append`].
    fn replace(&mut self, part_id: &str, text: &str) -> Option<()> {
        if part_id.is_empty() {
            return None;
        }
        let Some(index) = self.parts.iter().position(|part| part.part_id == part_id) else {
            if self.parts.len() >= MAX_ASSISTANT_TEXT_PARTS {
                return None;
            }
            let mut added = text.len();
            if !text.is_empty() && self.nonempty_part_count() > 0 {
                added = added.checked_add(PART_SEPARATOR.len())?;
            }
            let next_total = self.total_bytes.checked_add(added)?;
            if next_total > AssistantBody::MAX_BYTES {
                return None;
            }
            self.parts.push(AssistantTextPart {
                part_id: part_id.to_owned(),
                text: text.to_owned(),
            });
            self.total_bytes = next_total;
            return Some(());
        };
        let was_nonempty = !self.parts[index].text.is_empty();
        let is_nonempty = !text.is_empty();
        let before_count = self.nonempty_part_count();
        let after_count = if is_nonempty && !was_nonempty {
            before_count + 1
        } else if !is_nonempty && was_nonempty {
            // The touched part itself was counted, so this cannot underflow.
            before_count - 1
        } else {
            before_count
        };
        let previous_length = self.parts[index].text.len();
        let next_total = self
            .total_bytes
            .checked_sub(previous_length)?
            .checked_sub(part_separator_bytes(before_count))?
            .checked_add(text.len())?
            .checked_add(part_separator_bytes(after_count))?;
        if next_total > AssistantBody::MAX_BYTES {
            return None;
        }
        self.parts[index].text.clear();
        self.parts[index].text.push_str(text);
        self.total_bytes = next_total;
        Some(())
    }
}

#[cfg(test)]
mod text_projection_tests {
    use super::*;

    #[test]
    fn corrections_and_interleaved_deltas_preserve_message_boundaries() {
        let mut parts = OrderedAssistantText::default();
        parts.append("a", "A").expect("first");
        parts.append("b", "B").expect("second");
        parts.append("a", "!").expect("late delta");
        assert_eq!(parts.part_body("a"), "A!");
        assert_eq!(parts.part_body("b"), "B");
        parts.replace("b", "corrected").expect("correction");
        assert_eq!(parts.part_body("a"), "A!");
        assert_eq!(parts.part_body("b"), "corrected");
    }

    #[test]
    fn multipart_budget_remains_bounded_across_replacements() {
        let mut parts = OrderedAssistantText::default();
        parts
            .append("a", &"x".repeat(AssistantBody::MAX_BYTES - 3))
            .expect("large");
        parts.append("b", "b").expect("full");
        assert_eq!(parts.total_bytes, AssistantBody::MAX_BYTES);
        assert!(parts.append("c", "c").is_none());
        assert!(parts.replace("b", "bb").is_none());
        parts.replace("a", "a").expect("shrink");
        parts.append("c", "c").expect("room again");
        assert_eq!(parts.total_bytes, 7);
    }

    #[test]
    fn empty_parts_keep_identity_without_spending_text_budget() {
        let mut parts = OrderedAssistantText::default();
        for index in 0..MAX_ASSISTANT_TEXT_PARTS {
            parts
                .append(&index.to_string(), "")
                .expect("bounded empty part");
        }
        assert!(parts.append("overflow", "").is_none());
        parts.append("0", "hello").expect("existing identity");
        assert_eq!(parts.total_bytes, 5);
        parts.replace("0", "").expect("clear");
        assert_eq!(parts.total_bytes, 0);
    }
}
