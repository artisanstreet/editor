//! Bounded observation types for SSE-derived assistant content.
//!
//! Pure, minimal subset: distinct terminal states and lossless UTF-8-safe
//! chunking of assistant text with stable IDs and preserved durable sequences.
//! No raw frames, credentials, serde persistence, database calls, or public
//! exports are added here.

use artisan_domain::RunId;
use artisan_transport::CancelHandle;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::Instant;

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

/// Typed terminal observation preserving caller-supplied identity and state.
///
/// Keeps the provided [`RunId`], durable sequence, one of the four distinct
/// [`TerminalState`] values, and optional reason/error reference strings.
/// No sequence is invented, no `Interrupted` is collapsed into `Cancelled`,
/// and no raw frames, auth, secrets, or serialization are added.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalObservation {
    run_id: RunId,
    sequence: u64,
    state: TerminalState,
    reason: Option<String>,
    error_ref: Option<String>,
}

impl TerminalObservation {
    #[must_use]
    pub(crate) fn new(
        run_id: RunId,
        sequence: u64,
        state: TerminalState,
        reason: Option<String>,
        error_ref: Option<String>,
    ) -> Self {
        Self {
            run_id,
            sequence,
            state,
            reason,
            error_ref,
        }
    }

    #[must_use]
    pub(crate) fn run_id(&self) -> &RunId {
        &self.run_id
    }

    #[must_use]
    pub(crate) fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub(crate) fn state(&self) -> TerminalState {
        self.state
    }

    #[must_use]
    pub(crate) fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    #[must_use]
    pub(crate) fn error_ref(&self) -> Option<&str> {
        self.error_ref.as_deref()
    }
}

/// Minimal wakeable observation carrying either a text delta or a terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EngineObservation {
    TextDelta(TextDelta),
    Terminal(TerminalObservation),
}

/// Payload-free error for one bounded observation delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum DeliveryError {
    #[error("observation delivery shut down")]
    Shutdown,
    #[error("observation delivery cancelled")]
    Cancelled,
    #[error("observation delivery deadline exceeded")]
    Deadline,
    #[error("observation sink closed")]
    SinkClosed,
}

/// Delivers exactly one [`EngineObservation`] through a bounded channel with
/// wakeable backpressure.
///
/// Retains the pending observation and exactly one
/// `sender.clone().reserve_owned()` future across `Pending`. The future is
/// pinned once and the delivery races with `shutdown`, `cancel`, and
/// `deadline` under `tokio::select! { biased; }` with precedence
/// `shutdown > cancel > deadline > permit`. The same precedence is checked
/// before creating the permit so already-signalled conditions are
/// deterministic.
///
/// On `OwnedPermit` the observation is sent exactly once via
/// `permit.send(observation)`. On `SendError(())` the error is
/// `SinkClosed`. No retry, no `try_send` parking, no unbounded channel,
/// and no silent drop of a payload. Cancellation, deadline, or shutdown
/// ends only this delivery; they do not mutate the observation.
pub(crate) async fn deliver_observation(
    observation: EngineObservation,
    sender: mpsc::Sender<EngineObservation>,
    shutdown: &CancelHandle,
    cancel: &CancelHandle,
    deadline: Instant,
) -> Result<(), DeliveryError> {
    if shutdown.is_cancelled() {
        return Err(DeliveryError::Shutdown);
    }
    if cancel.is_cancelled() {
        return Err(DeliveryError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(DeliveryError::Deadline);
    }

    let mut pending = Some(observation);
    let mut reserve = Box::pin(sender.clone().reserve_owned());

    tokio::select! {
        biased;

        () = shutdown.wait() => Err(DeliveryError::Shutdown),
        () = cancel.wait() => Err(DeliveryError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(DeliveryError::Deadline),
        res = &mut reserve => match res {
            Ok(permit) => {
                let obs = pending.take().expect("pending observation present");
                permit.send(obs);
                Ok(())
            }
            Err(_) => Err(DeliveryError::SinkClosed),
        }
    }
}
