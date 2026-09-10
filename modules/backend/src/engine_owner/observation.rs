//! Bounded observation types for SSE-derived assistant content.
//!
//! Pure, minimal subset: distinct terminal states and lossless UTF-8-safe
//! chunking of assistant text with stable IDs and preserved durable sequences,
//! plus validated provider-native subagent lifecycle and transcript rows that
//! travel the same owner channel without ever adopting the root turn.
//! No raw frames, credentials, serde persistence, database calls, or public
//! exports are added here.

use artisan_domain::{RunId, RunUsageReport, SubagentObservation, SubagentTranscriptObservation};
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
/// (never zero/invented/incremented per chunk), stable chunk ID, optional
/// explicit provider part ID, and exact delta text. Empty text may produce
/// zero deltas.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextDelta {
    run_id: RunId,
    sequence: u64,
    chunk_id: String,
    part_id: Option<String>,
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
    pub(crate) fn part_id(&self) -> Option<&str> {
        self.part_id.as_deref()
    }

    /// Attaches the provider's already-validated explicit text-part identity.
    /// Fixture deltas intentionally leave this absent and use one bounded
    /// compatibility part in the dispatcher.
    #[must_use]
    pub(crate) fn with_part_id(mut self, part_id: String) -> Self {
        self.part_id = Some(part_id);
        self
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
                part_id: None,
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
            part_id: None,
            delta,
        });
    }
    out
}

/// Typed terminal observation preserving caller-supplied identity and state.
///
/// Keeps the provided [`RunId`], durable sequence, one of the four distinct
/// [`TerminalState`] values, optional reason/error reference strings, and an
/// optional generated session title captured at the terminal fence. No
/// sequence is invented, no `Interrupted` is collapsed into `Cancelled`, and
/// no raw frames, auth, secrets, or serialization are added.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalObservation {
    run_id: RunId,
    sequence: u64,
    state: TerminalState,
    reason: Option<String>,
    error_ref: Option<String>,
    summary_title: Option<String>,
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
            summary_title: None,
        }
    }

    /// Attaches the harness-generated session title captured at the terminal
    /// fence. Engines that capture no title leave the observation unchanged.
    #[must_use]
    pub(crate) fn with_summary_title(mut self, summary_title: Option<String>) -> Self {
        self.summary_title = summary_title;
        self
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

    /// Returns the harness-generated session title, when the engine produced
    /// one by the time the run settled.
    #[must_use]
    pub(crate) fn summary_title(&self) -> Option<&str> {
        self.summary_title.as_deref()
    }
}

/// A bounded provider text projection used when OpenCode sends a text-end
/// reconciliation envelope. The dispatcher applies it to the explicit part
/// projection and rebuilds the one durable assistant item without appending
/// the same full part twice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextSnapshot {
    run_id: RunId,
    sequence: u64,
    part_id: String,
    text: String,
}

impl TextSnapshot {
    #[must_use]
    pub(crate) fn new(run_id: RunId, sequence: u64, part_id: String, text: String) -> Self {
        Self {
            run_id,
            sequence,
            part_id,
            text,
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
    pub(crate) fn part_id(&self) -> &str {
        &self.part_id
    }

    #[must_use]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// A provider usage report attributed to the immutable current run snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageObservation {
    report: RunUsageReport,
}

impl UsageObservation {
    #[must_use]
    pub(crate) fn new(report: RunUsageReport) -> Self {
        Self { report }
    }

    #[must_use]
    pub(crate) fn report(&self) -> &RunUsageReport {
        &self.report
    }
}

/// A validated provider-native subagent lifecycle row.
///
/// Carries the domain lifecycle row through the owner channel; its public
/// content travels separately as transcript rows, never as root text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SubagentLifecycleRow {
    observation: SubagentObservation,
}

impl SubagentLifecycleRow {
    /// Wraps a validated domain lifecycle row for channel delivery.
    #[must_use]
    pub(crate) fn new(observation: SubagentObservation) -> Self {
        Self { observation }
    }

    /// Releases the carried domain row for the dispatcher commit.
    #[must_use]
    pub(crate) fn into_observation(self) -> SubagentObservation {
        self.observation
    }
}

/// A validated provider-native subagent transcript row.
///
/// Carries renderer-safe projected content with its own durable identity
/// plus both native thread identities through the owner channel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SubagentTranscriptRow {
    observation: SubagentTranscriptObservation,
}

impl SubagentTranscriptRow {
    /// Wraps a validated domain transcript row for channel delivery.
    #[must_use]
    pub(crate) fn new(observation: SubagentTranscriptObservation) -> Self {
        Self { observation }
    }

    /// Releases the carried domain row for the dispatcher commit.
    #[must_use]
    pub(crate) fn into_observation(self) -> SubagentTranscriptObservation {
        self.observation
    }
}

/// Minimal wakeable observation carrying normalized provider state.
///
/// Subagent rows ride beside text deltas: lifecycle reports and transcript
/// rows carry validated domain content with their own identities and never
/// adopt the root turn. `Activity` carries one validated provider-neutral
/// domain observation (reasoning summary, tool, terminal activity, file,
/// search, or plan) decoded from rich engine frames; owner rows carry
/// SOURCE-LOCAL FRAME order only, and fragment rows from one frame may
/// share its sequence. The dispatcher remints the monotonic RUN-local
/// observation sequence and identity; the database allocates the separate
/// THREAD-scoped attribution `delivery_sequence`. `Eq` is deliberately
/// absent: transcript content has no total-equality bound, and channel
/// delivery plus matching need only [`PartialEq`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EngineObservation {
    TextDelta(TextDelta),
    TextSnapshot(TextSnapshot),
    Usage(UsageObservation),
    Terminal(TerminalObservation),
    Subagent(SubagentLifecycleRow),
    SubagentTranscript(SubagentTranscriptRow),
    Activity(artisan_domain::Observation),
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
