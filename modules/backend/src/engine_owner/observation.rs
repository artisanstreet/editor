//! Bounded observation types for SSE-derived assistant content.
//!
//! Pure, minimal subset: distinct terminal states and lossless UTF-8-safe
//! chunking of assistant text with stable IDs and preserved durable sequences.
//! No raw frames, credentials, serde persistence, database calls, or public
//! exports are added here.

use artisan_domain::RunId;

/// Distinct terminal states for a provider run.
///
/// Never collapse non-user interruption into cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalState {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

/// Text-delta observation for one lossless chunk.
///
/// Contains the caller-provided run identity, provided durable sequence
/// (never zero/invented/incremented per chunk), stable chunk ID, and exact
/// delta text. Empty text may produce zero deltas.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextDelta {
    run_id: RunId,
    sequence: u64,
    chunk_id: String,
    delta: String,
}

impl TextDelta {
    #[must_use]
    pub(crate) fn run_id(&self) -> &RunId {
        &self.run_id
    }

    #[must_use]
    pub(crate) fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub(crate) fn chunk_id(&self) -> &str {
        &self.chunk_id
    }

    #[must_use]
    pub(crate) fn delta(&self) -> &str {
        &self.delta
    }
}

/// Lossless UTF-8-safe chunking of assistant text.
///
/// Emits at most 4096 bytes per delta, each chunk valid UTF-8, concatenated
/// deltas reproduce `text` exactly including multi-byte Unicode and interior
/// empty content (no ellipsis, replacement, truncation, or dropped content).
/// Stable chunk IDs are exactly `native_id:durable_sequence:chunk_index`.
/// Empty `text` produces zero deltas. Avoids panics on boundaries.
///
/// `durable_sequence` is preserved verbatim for every emitted delta (not
/// incremented, not invented, not zeroed).
pub(crate) fn chunk_text(
    run_id: &RunId,
    durable_sequence: u64,
    native_id: &str,
    text: &str,
) -> Vec<TextDelta> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut chunk_start = 0usize;
    let mut current_len = 0usize;
    let mut chunk_index = 0usize;

    for (byte_idx, ch) in text.char_indices() {
        let ch_len = ch.len_utf8();
        if current_len + ch_len > 4096 {
            let delta = text[chunk_start..byte_idx].to_owned();
            let chunk_id = format!("{native_id}:{durable_sequence}:{chunk_index}");
            out.push(TextDelta {
                run_id: run_id.clone(),
                sequence: durable_sequence,
                chunk_id,
                delta,
            });
            chunk_index += 1;
            chunk_start = byte_idx;
            current_len = 0;
        }
        current_len += ch_len;
    }
    if chunk_start < text.len() {
        let delta = text[chunk_start..].to_owned();
        debug_assert!(delta.len() <= 4096);
        let chunk_id = format!("{native_id}:{durable_sequence}:{chunk_index}");
        out.push(TextDelta {
            run_id: run_id.clone(),
            sequence: durable_sequence,
            chunk_id,
            delta,
        });
    }
    out
}
