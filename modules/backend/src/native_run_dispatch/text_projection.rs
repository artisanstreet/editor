//! Assistant text projection.
//!
//! Owns the bounded, byte-exact assembly of distinct provider text parts into
//! one durable assistant body: same-part deltas stay byte-exact, distinct
//! nonempty parts earn exactly one paragraph separator, and every admitted
//! byte counts against [`AssistantBody::MAX_BYTES`].

use artisan_domain::AssistantBody;

use crate::engine_owner::observation::{TextDelta, TextSnapshot};

const MAX_ASSISTANT_TEXT_PARTS: usize = 128;
const FIXTURE_TEXT_PART_ID: &str = "fixture-text-part";

/// Separator placed between distinct nonempty provider text parts.
///
/// One blank line is the faithful single-body rendering of two provider
/// messages: it keeps them as separate Markdown paragraphs/blocks while every
/// part's own bytes stay untouched. A single newline would soft-wrap the
/// boundary into one paragraph and rejoin sentences (`naturally.I'm`), and
/// mutating part bytes to insert spaces would corrupt streamed token chunks.
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
    fn nonempty_part_count(&self) -> usize {
        self.parts
            .iter()
            .filter(|part| !part.text.is_empty())
            .count()
    }

    pub(super) fn append_delta(&mut self, delta: &TextDelta) -> Option<String> {
        self.append(
            delta.part_id().unwrap_or(FIXTURE_TEXT_PART_ID),
            delta.delta(),
        )
    }

    pub(super) fn replace_snapshot(&mut self, snapshot: &TextSnapshot) -> Option<String> {
        self.replace(snapshot.part_id(), snapshot.text())
    }

    /// Appends `text` onto one logical provider part.
    ///
    /// Bytes stay byte-exact within the part: deltas for the same `part_id`
    /// never gain a separator, which is what keeps the streaming fast path in
    /// `handle_text_delta` (`previous + delta == next_body`) valid. A first
    /// contribution that makes a new part nonempty earns exactly one separator
    /// when another nonempty part already exists; empty contributions earn
    /// none, so empty parts can never produce leading, trailing, or doubled
    /// separators.
    fn append(&mut self, part_id: &str, text: &str) -> Option<String> {
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
            return Some(self.body());
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
        Some(self.body())
    }

    /// Replaces one logical provider part wholesale.
    ///
    /// The aggregate bound is recharged with the part's previous bytes and its
    /// previous separator share before the replacement (plus its new share) is
    /// admitted, so corrections can shrink, grow, or empty a part without
    /// leaking separator bytes. Callers other than tests and snapshot
    /// handling must not use this for streaming deltas: same-part streaming
    /// stays byte-exact through [`Self::append`].
    fn replace(&mut self, part_id: &str, text: &str) -> Option<String> {
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
            return Some(self.body());
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
        Some(self.body())
    }

    fn body(&self) -> String {
        let mut body = String::with_capacity(self.total_bytes);
        let mut first = true;
        for part in &self.parts {
            if part.text.is_empty() {
                continue;
            }
            if !first {
                body.push_str(PART_SEPARATOR);
            }
            first = false;
            body.push_str(&part.text);
        }
        debug_assert_eq!(body.len(), self.total_bytes);
        body
    }
}

#[cfg(test)]
mod text_projection_tests {
    use artisan_domain::RunId;

    use super::OrderedAssistantText;
    use crate::engine_owner::observation::{TextSnapshot, chunk_text};

    #[test]
    fn multipart_byte_bound_tracks_all_parts_separators_and_replacements() {
        let mut parts = OrderedAssistantText::default();
        // Two separator bytes join the two nonempty parts, so the aggregate
        // fills exactly: (MAX - 3) + 1 + 2.
        let full = "x".repeat(artisan_domain::AssistantBody::MAX_BYTES - 3);
        assert!(parts.append("a", &full).is_some());
        assert!(parts.append("b", "b").is_some());
        assert_eq!(parts.total_bytes, artisan_domain::AssistantBody::MAX_BYTES);
        assert!(parts.append("c", "c").is_none());
        assert!(parts.replace("b", "bb").is_none());
        assert!(parts.replace("a", "a").is_some());
        assert_eq!(parts.append("c", "c").as_deref(), Some("a\n\nb\n\nc"));
        assert_eq!(parts.total_bytes, 7);
    }

    #[test]
    fn correcting_second_text_part_retains_first_without_duplicate_bytes() {
        let run_id = RunId::parse("text-projection-run").expect("bounded run id");
        let mut parts = OrderedAssistantText::default();
        let part_a_delta = chunk_text(&run_id, 1, "event-a", "A-1")
            .pop()
            .expect("part A delta")
            .with_part_id("part-a".to_owned());
        assert_eq!(parts.append_delta(&part_a_delta), Some("A-1".to_owned()));
        let part_a_delta = chunk_text(&run_id, 2, "event-a-2", "A-2")
            .pop()
            .expect("part A second delta")
            .with_part_id("part-a".to_owned());
        // Same logical part stays byte-exact: no separator may enter streamed
        // token chunks.
        assert_eq!(parts.append_delta(&part_a_delta), Some("A-1A-2".to_owned()));
        assert_eq!(
            parts.replace_snapshot(&TextSnapshot::new(
                run_id.clone(),
                3,
                "part-a".to_owned(),
                "A-1A-2".to_owned(),
            )),
            Some("A-1A-2".to_owned())
        );
        let second_part_delta = chunk_text(&run_id, 4, "event-b", "B-1")
            .pop()
            .expect("part B delta")
            .with_part_id("part-b".to_owned());
        // A distinct nonempty provider part earns paragraph separation instead
        // of rejoining the previous part's sentence.
        assert_eq!(
            parts.append_delta(&second_part_delta),
            Some("A-1A-2\n\nB-1".to_owned())
        );
        assert_eq!(
            parts.replace_snapshot(&TextSnapshot::new(
                run_id,
                5,
                "part-b".to_owned(),
                "B-corrected".to_owned(),
            )),
            Some("A-1A-2\n\nB-corrected".to_owned())
        );
        assert_eq!(parts.body(), "A-1A-2\n\nB-corrected");
    }

    #[test]
    fn empty_parts_contribute_no_text_and_no_separator() {
        let mut parts = OrderedAssistantText::default();
        assert_eq!(parts.append("a", "A").as_deref(), Some("A"));
        // An empty part is retained without text and without separators.
        assert_eq!(parts.append("b", "").as_deref(), Some("A"));
        assert_eq!(parts.append("c", "C").as_deref(), Some("A\n\nC"));
        // Emptying a part withdraws its separator share as well.
        assert_eq!(parts.replace("c", "").as_deref(), Some("A"));
        assert_eq!(parts.replace("a", "").as_deref(), Some(""));
        assert_eq!(parts.body(), "");
        assert_eq!(parts.total_bytes, 0);
        // Refilling from empty re-earns separators in part order.
        assert_eq!(parts.append("c", "C").as_deref(), Some("C"));
        assert_eq!(parts.append("a", "A").as_deref(), Some("A\n\nC"));
    }
}
