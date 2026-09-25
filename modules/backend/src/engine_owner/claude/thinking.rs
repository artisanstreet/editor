//! Claude thinking-stretch tracking onto the shared reasoning observations.
//!
//! Mirrors the Codex reasoning-summary projection (`codex/adapter.rs`): each
//! public fragment becomes a [`ReasoningSummaryDeltaObservation`] and each
//! settled stretch one [`ReasoningSummaryCompletedObservation`], emitted
//! through the normal activity sink. Only launches that requested
//! `--thinking-display summarized` project anything; other display
//! semantics are unknown and stay unavailable.
//!
//! Transport contract (captured from Claude Code 2.1.282, see
//! `tests/fixtures/claude/manifest.json`): a stretch opens with a
//! `content_block_start` of type `thinking`, streams `thinking_delta`
//! fragments at that block index, then the CLI emits one buffered
//! `assistant` frame carrying exactly that block's authoritative text
//! immediately before the block's `content_block_stop`. The buffered frame
//! therefore maps to the open stretch of the same message, and completes it
//! with the authoritative text (the consumer replaces the streamed text, so
//! nothing is appended twice). A stop without a buffered frame settles the
//! stretch without replacement text; a buffered thinking frame with no open
//! stretch has no verified block index and projects nothing.
//!
//! Identity is one reasoning item per provider message and content-block
//! index; the run supplies the turn scope (one finite CLI process per turn
//! and Claude names no provider turn), so resumed sessions start from fresh
//! run-local tracking. Frame sequence and fragment index only deduplicate
//! observation rows. Signatures never reach this module.

use artisan_domain::{
    OBSERVATION_DELTA_MAX_BYTES, OBSERVATION_MESSAGE_MAX_BYTES, Observation, ObservationId,
    ObservationSequence, ReasoningSummaryCompletedObservation, ReasoningSummaryDeltaObservation,
    RunId,
};

use super::launch::ClaudeThinkingDisplay;

/// One open thinking stretch.
#[derive(Debug)]
struct OpenStretch {
    message_id: String,
    index: u64,
    /// Public bytes already projected as deltas; bounded by the message
    /// ceiling so the consumer's accumulated row stays representable.
    streamed_bytes: usize,
    /// Set once the buffered frame settled this stretch.
    completed: bool,
}

/// Run-local thinking tracker for one Claude turn.
#[derive(Debug, Default)]
pub(crate) struct ClaudeThinkingTracker {
    display: ClaudeThinkingDisplay,
    open: Option<OpenStretch>,
}

impl ClaudeThinkingTracker {
    /// Creates a tracker for one run under the launch's display policy.
    pub(crate) fn new(display: ClaudeThinkingDisplay) -> Self {
        Self {
            display,
            open: None,
        }
    }

    fn projects(&self) -> bool {
        self.display == ClaudeThinkingDisplay::Summarized
    }

    /// Opens the stretch at `index` of the current stream message.
    ///
    /// A stretch left open by a malformed stream is dropped without a
    /// completion: its identity is never reused.
    pub(crate) fn start(&mut self, message_id: Option<&str>, index: u64) {
        self.open = message_id.map(|message_id| OpenStretch {
            message_id: message_id.to_owned(),
            index,
            streamed_bytes: 0,
            completed: false,
        });
    }

    /// Projects one streamed public fragment of the open stretch.
    ///
    /// Fragments split on UTF-8 boundaries within the delta ceiling; once the
    /// stretch's projected text would exceed the message ceiling, further
    /// fragments are dropped rather than truncated.
    pub(crate) fn delta(
        &mut self,
        run_id: &RunId,
        frame_sequence: u64,
        index: u64,
        text: &str,
    ) -> Vec<Observation> {
        if !self.projects() {
            return Vec::new();
        }
        let Some(open) = self.open.as_mut().filter(|open| open.index == index) else {
            return Vec::new();
        };
        if open.completed {
            return Vec::new();
        }
        // Saturating: once over the ceiling the stretch stays over it, so no
        // later fragment can leave a gap in the projected text.
        open.streamed_bytes = open.streamed_bytes.saturating_add(text.len());
        if open.streamed_bytes > OBSERVATION_MESSAGE_MAX_BYTES {
            return Vec::new();
        }
        let Some((sequence, item, turn)) = row_scope(run_id, frame_sequence, open) else {
            return Vec::new();
        };
        fragment_utf8(text, OBSERVATION_DELTA_MAX_BYTES)
            .into_iter()
            .enumerate()
            .filter_map(|(fragment, part)| {
                let id = row_id(run_id, frame_sequence, &format!("rsum:{index}:{fragment}"))?;
                ReasoningSummaryDeltaObservation::new(
                    id,
                    sequence,
                    item.clone(),
                    0,
                    part.to_owned(),
                    None,
                    turn.clone(),
                )
                .ok()
                .map(Observation::ReasoningSummaryDelta)
            })
            .collect()
    }

    /// Settles the open stretch of `message_id` with the buffered frame's
    /// authoritative text.
    ///
    /// Duplicate buffered frames and frames without an open stretch project
    /// nothing. Empty text (omitted display) and text over the message
    /// ceiling settle without replacement: truncated canonical prose is
    /// never invented.
    pub(crate) fn buffered(
        &mut self,
        run_id: &RunId,
        frame_sequence: u64,
        message_id: Option<&str>,
        text: &str,
    ) -> Vec<Observation> {
        if !self.projects() {
            return Vec::new();
        }
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| !open.completed && Some(open.message_id.as_str()) == message_id)
        else {
            return Vec::new();
        };
        open.completed = true;
        let text = Some(text)
            .filter(|text| !text.is_empty() && text.len() <= OBSERVATION_MESSAGE_MAX_BYTES);
        completed_rows(run_id, frame_sequence, open, text)
    }

    /// Closes the stretch at `index`; settles it without replacement text
    /// when no buffered frame did.
    pub(crate) fn stop(
        &mut self,
        run_id: &RunId,
        frame_sequence: u64,
        index: u64,
    ) -> Vec<Observation> {
        if self.open.as_ref().is_none_or(|open| open.index != index) {
            return Vec::new();
        }
        let Some(open) = self.open.take() else {
            return Vec::new();
        };
        if !self.projects() || open.completed {
            return Vec::new();
        }
        completed_rows(run_id, frame_sequence, &open, None)
    }

    /// Drops transient stretch state when a new provider message starts.
    pub(crate) fn message_started(&mut self) {
        self.open = None;
    }
}

fn row_id(run_id: &RunId, frame_sequence: u64, slug: &str) -> Option<ObservationId> {
    ObservationId::parse(format!(
        "{}:claude:{frame_sequence}:{slug}",
        run_id.as_str()
    ))
    .ok()
}

/// Resolves the sequence, item identity, and turn scope for one stretch.
fn row_scope(
    run_id: &RunId,
    frame_sequence: u64,
    open: &OpenStretch,
) -> Option<(ObservationSequence, ObservationId, ObservationId)> {
    Some((
        ObservationSequence::new(frame_sequence).ok()?,
        ObservationId::parse(format!("thinking:{}:{}", open.message_id, open.index)).ok()?,
        ObservationId::parse(run_id.as_str()).ok()?,
    ))
}

fn completed_rows(
    run_id: &RunId,
    frame_sequence: u64,
    open: &OpenStretch,
    text: Option<&str>,
) -> Vec<Observation> {
    let Some((sequence, item, turn)) = row_scope(run_id, frame_sequence, open) else {
        return Vec::new();
    };
    let Some(id) = row_id(run_id, frame_sequence, &format!("rsc:{}", open.index)) else {
        return Vec::new();
    };
    ReasoningSummaryCompletedObservation::new(id, sequence, item, text.map(str::to_owned), turn)
        .ok()
        .map(Observation::ReasoningSummaryCompleted)
        .into_iter()
        .collect()
}

/// Splits text into lossless UTF-8-boundary fragments of at most
/// `max_bytes` bytes (the Codex `fragment_text` rule).
fn fragment_utf8(text: &str, max_bytes: usize) -> Vec<&str> {
    let bound = max_bytes.max(1);
    let mut fragments = Vec::new();
    let mut start = 0;
    for (byte, character) in text.char_indices() {
        if byte - start + character.len_utf8() > bound {
            fragments.push(&text[start..byte]);
            start = byte;
        }
    }
    if start < text.len() {
        fragments.push(&text[start..]);
    }
    fragments
}

#[cfg(test)]
mod fragment_tests {
    use super::fragment_utf8;

    #[test]
    fn fragments_split_on_character_boundaries_losslessly() {
        let text = "a\u{00e9}\u{1f600}b";
        let fragments = fragment_utf8(text, 4);
        assert_eq!(fragments, vec!["a\u{00e9}", "\u{1f600}", "b"]);
        assert_eq!(fragments.concat(), text);
        assert!(fragment_utf8("", 4).is_empty());
    }
}
