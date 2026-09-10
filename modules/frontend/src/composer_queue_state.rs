//! Bounded application state for queued-message lip rows and run usage.
//!
//! This module is deliberately independent of GPUI widgets and the transport
//! service. It owns the identities and fences that make those two surfaces
//! safe to compose: a queue page is byte-free, a withdrawal remains visible
//! until Forge confirms it, and a recalled payload is never lost when the
//! composer changes underneath an in-flight read.
//!
//! The parent application supplies the current thread and composer
//! generation. This module never invents either value, never invents queue
//! text, and never derives context capacity from the selected model catalog.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{
    collections::{HashSet, VecDeque},
    fmt,
};

use artisan_domain::{
    AuthoredText, DispatchError, EngineModelId, EngineRouteId, EngineVariantId, FailedMessageListing,
    FailedMessageSummary, ImageAttachmentRef, MessageId,
    QueuedMessageListOrder, QueuedMessageListing, QueuedMessageWithdrawalOutcome,
    QueuedMessageWithdrawalResult, ReadRecalledMessage, RecalledMessageResult, RequestId, RunId,
    RunUsageReport, RunUsageResult, ThreadId, UnixMillis, WithdrawQueuedMessageCommand,
};

use crate::native_composer::ComposerRecallTarget;

/// The largest page the native queue surface will request or retain.
pub(crate) const COMPOSER_QUEUE_PAGE_LIMIT: usize = artisan_domain::QUEUED_MESSAGE_LIST_MAX;

/// The refresh cadence used by the parent while a queue or run is live.
pub(crate) const COMPOSER_QUEUE_REFRESH_INTERVAL_MS: u64 = 5000;

/// Completed withdrawal keys are retained only long enough to make a delayed
/// duplicate receipt harmless. They contain no payload bytes.
const COMPLETED_WITHDRAWAL_HISTORY_LIMIT: usize = 8;

/// Exact row identity used between the queue projection and controls.
///
/// `command_id` is the original queue request id, not the newer withdrawal
/// request id. The generation prevents a delayed control event from acting on
/// a visually reused command id after a thread/scope transition.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ComposerQueueIdentity {
    command_id: String,
    generation: u64,
}

impl ComposerQueueIdentity {
    /// Creates an exact queue-row identity without normalization.
    #[must_use]
    pub(crate) fn new(command_id: impl Into<String>, generation: u64) -> Self {
        Self {
            command_id: command_id.into(),
            generation,
        }
    }

    /// Returns the original queue command id.
    #[must_use]
    pub(crate) fn command_id(&self) -> &str {
        &self.command_id
    }

    /// Returns the application generation that projected this row.
    #[must_use]
    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }
}

/// Byte-free data needed to render one queued lip row and to issue an exact
/// withdrawal later.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComposerQueueEntry {
    identity: ComposerQueueIdentity,
    thread_id: ThreadId,
    message_id: MessageId,
    original_request_id: RequestId,
    text: Option<AuthoredText>,
    attachments: Vec<ImageAttachmentRef>,
    accepted_at: UnixMillis,
    last_error: Option<artisan_domain::DispatchError>,
}

impl ComposerQueueEntry {
    fn from_summary(
        summary: &artisan_domain::QueuedMessageSummary,
        generation: u64,
    ) -> Option<Self> {
        if summary.attachments.len() > artisan_domain::MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
            return None;
        }
        let total_bytes = summary
            .attachments
            .iter()
            .try_fold(0usize, |total, image| {
                total.checked_add(usize::try_from(image.size_bytes).ok()?)
            })?;
        if total_bytes > artisan_domain::MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES {
            return None;
        }
        if summary
            .attachments
            .iter()
            .enumerate()
            .any(|(index, image)| {
                image.message_id != summary.message_id
                    || image.thread_id != summary.thread_id
                    || usize::try_from(image.index).ok() != Some(index)
            })
        {
            return None;
        }
        Some(Self {
            identity: ComposerQueueIdentity::new(summary.original_request_id.as_str(), generation),
            thread_id: summary.thread_id.clone(),
            message_id: summary.message_id.clone(),
            original_request_id: summary.original_request_id.clone(),
            text: summary.text.clone(),
            attachments: summary.attachments.clone(),
            accepted_at: summary.accepted_at,
            last_error: summary.last_error.clone(),
        })
    }

    /// Returns the exact controls identity.
    #[must_use]
    pub(crate) fn identity(&self) -> &ComposerQueueIdentity {
        &self.identity
    }

    /// Returns the original owning thread.
    #[must_use]
    pub(crate) fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the Forge-minted message identity.
    #[must_use]
    pub(crate) fn message_id(&self) -> &MessageId {
        &self.message_id
    }

    /// Returns the request id that originally accepted the queued message.
    #[must_use]
    pub(crate) fn original_request_id(&self) -> &RequestId {
        &self.original_request_id
    }

    /// Returns authored text with its `None` versus `Some("")` presence.
    #[must_use]
    pub(crate) fn text(&self) -> Option<&AuthoredText> {
        self.text.as_ref()
    }

    /// Returns ordered, byte-free attachment references.
    #[must_use]
    pub(crate) fn attachments(&self) -> &[ImageAttachmentRef] {
        &self.attachments
    }

    /// Returns the authoritative queue acceptance instant.
    #[must_use]
    pub(crate) const fn accepted_at(&self) -> UnixMillis {
        self.accepted_at
    }

    /// Returns the latest dispatcher diagnostic for this row, if the
    /// dispatcher has claimed and requeued it at least once.
    #[must_use]
    pub(crate) fn dispatch_error(&self) -> Option<&str> {
        self.last_error.as_ref().map(artisan_domain::DispatchError::as_str)
    }

    /// Returns the exact text sent to the existing lip renderer.
    ///
    /// An absent or empty text remains empty here. The existing
    /// `QueuedSteerRow` policy supplies its documented image-only label at
    /// paint time; this state layer never inserts a message placeholder.
    #[must_use]
    pub(crate) fn lip_text(&self) -> &str {
        self.text.as_ref().map_or("", AuthoredText::as_str)
    }
}

/// Byte-free data needed to render one failed-dispatch card and to issue an
/// exact new-chat recovery later.
///
/// The identity binds the action to the exact failed message and the
/// generation that projected it: never to current composer text or a run id.
/// The reason is the verbatim dispatcher diagnostic, so the card states
/// exactly why the send cannot proceed on this thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FailedQueueEntry {
    identity: ComposerQueueIdentity,
    thread_id: ThreadId,
    message_id: MessageId,
    original_request_id: RequestId,
    text: Option<AuthoredText>,
    attachments: Vec<ImageAttachmentRef>,
    accepted_at: UnixMillis,
    failed_at: UnixMillis,
    reason: DispatchError,
}

impl FailedQueueEntry {
    fn from_summary(summary: &FailedMessageSummary, generation: u64) -> Option<Self> {
        if summary.attachments.len() > artisan_domain::MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
            return None;
        }
        let total_bytes = summary
            .attachments
            .iter()
            .try_fold(0usize, |total, image| {
                total.checked_add(usize::try_from(image.size_bytes).ok()?)
            })?;
        if total_bytes > artisan_domain::MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES {
            return None;
        }
        if summary
            .attachments
            .iter()
            .enumerate()
            .any(|(index, image)| {
                image.message_id != summary.message_id
                    || image.thread_id != summary.thread_id
                    || usize::try_from(image.index).ok() != Some(index)
            })
        {
            return None;
        }
        Some(Self {
            identity: ComposerQueueIdentity::new(summary.original_request_id.as_str(), generation),
            thread_id: summary.thread_id.clone(),
            message_id: summary.message_id.clone(),
            original_request_id: summary.original_request_id.clone(),
            text: summary.text.clone(),
            attachments: summary.attachments.clone(),
            accepted_at: summary.accepted_at,
            failed_at: summary.failed_at,
            reason: summary.reason.clone(),
        })
    }

    /// Returns the exact controls identity.
    #[must_use]
    pub(crate) fn identity(&self) -> &ComposerQueueIdentity {
        &self.identity
    }

    /// Returns the original owning thread.
    #[must_use]
    pub(crate) fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the Forge-minted failed message identity.
    #[must_use]
    pub(crate) fn message_id(&self) -> &MessageId {
        &self.message_id
    }

    /// Returns the request id that originally accepted the failed message.
    #[must_use]
    pub(crate) fn original_request_id(&self) -> &RequestId {
        &self.original_request_id
    }

    /// Returns authored text with its `None` versus `Some("")` presence.
    #[must_use]
    pub(crate) fn text(&self) -> Option<&AuthoredText> {
        self.text.as_ref()
    }

    /// Returns ordered, byte-free attachment references.
    #[must_use]
    pub(crate) fn attachments(&self) -> &[ImageAttachmentRef] {
        &self.attachments
    }

    /// Returns whether the failed prompt carries image attachments.
    #[must_use]
    pub(crate) fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
    }

    /// Returns the authoritative queue acceptance instant.
    #[must_use]
    pub(crate) const fn accepted_at(&self) -> UnixMillis {
        self.accepted_at
    }

    /// Returns the terminal failure instant.
    #[must_use]
    pub(crate) const fn failed_at(&self) -> UnixMillis {
        self.failed_at
    }

    /// Returns the verbatim dispatcher diagnostic for this failure.
    #[must_use]
    pub(crate) fn reason(&self) -> &str {
        self.reason.as_str()
    }

    /// Returns the exact text sent to the failed-card renderer.
    ///
    /// An absent or empty text remains empty here. The card never invents a
    /// placeholder: image-only failures render their attachment label from
    /// [`Self::has_attachments`] instead.
    #[must_use]
    pub(crate) fn card_text(&self) -> &str {
        self.text.as_ref().map_or("", AuthoredText::as_str)
    }
}

/// Why a failed-dispatch listing could not be installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FailedListingRejection {
    /// A newer scope or refresh owns the state.
    StaleRefresh,
    /// The response names another thread.
    WrongThread,
    /// The response exceeds the native page bound.
    TooManyRows,
    /// Two visible rows reuse one original queue command id, making a
    /// controls event ambiguous.
    DuplicateCommandId,
    /// One row contains out-of-scope or oversized image metadata.
    InvalidAttachments,
}

/// One row projection consumed by `NativeComposerControls`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueueLipRow {
    /// Original queue request id.
    pub(crate) command_id: String,
    /// Generation that owns the row.
    pub(crate) generation: u64,
    /// Exact authored text, empty when the wire field was absent.
    pub(crate) text: String,
    /// Whether an edit/discard route is currently admitted.
    pub(crate) editable: bool,
}

/// Truthful status for the queue banner and recovery action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueStatus {
    /// No queue operation is waiting for a result.
    Idle,
    /// A bounded listing request is in flight.
    Refreshing,
    /// Forge is fencing an edit or discard request.
    WithdrawalPending,
    /// Forge accepted an edit withdrawal and the payload read is pending.
    RecallReadPending,
    /// A withdrawal was accepted and the next listing is authoritative.
    WithdrawnAwaitingRefresh,
    /// Forge reported that dispatch already passed the withdrawal fence.
    TooLate,
    /// Forge reported that the exact message is no longer queued.
    NotQueued,
    /// The exact withdrawn row exists but has no readable payload.
    PayloadUnavailable,
    /// The exact payload is held for the empty composer.
    RestoreReady,
    /// A restore attempt lost a composer race; explicit retry remains valid.
    RestoreBlocked,
    /// The payload was accepted by the composer.
    Restored,
    /// A transport error left an exact command available for retry.
    TransportFailed,
}

impl QueueStatus {
    /// Returns the concise truthful copy shown by the optional queue banner.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Refreshing => "Refreshing queued messages…",
            Self::WithdrawalPending => "Updating queued message…",
            Self::RecallReadPending => "Retrieving queued message…",
            Self::WithdrawnAwaitingRefresh => "Queued message withdrawn.",
            Self::TooLate => "Queued message already started and cannot be edited.",
            Self::NotQueued => "Queued message is no longer queued.",
            Self::PayloadUnavailable => "Queued message payload is unavailable.",
            Self::RestoreReady => "Queued message ready to restore.",
            Self::RestoreBlocked => "Restore is waiting for an empty composer.",
            Self::Restored => "Queued message restored.",
            Self::TransportFailed => "Could not update queued messages.",
        }
    }

    /// Whether the status admits an explicit restore retry control.
    #[must_use]
    pub(crate) const fn restore_retry_available(self) -> bool {
        matches!(self, Self::RestoreReady | Self::RestoreBlocked)
    }
}

/// Failure to begin an edit or discard intent from a stale controls event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueIntentError {
    /// No real thread is currently mounted.
    NoThread,
    /// The controls identity was not in the current authoritative page.
    UnknownRow,
    /// The row belongs to another thread or generation.
    StaleRow,
    /// Another exact withdrawal/read/restore operation owns the surface.
    Busy,
}

/// The edit/discard intent attached to one pending stable withdrawal.
enum QueueWithdrawalIntent {
    Edit { target: ComposerRecallTarget },
    Discard,
}

/// One stable withdrawal request awaiting an authoritative receipt.
struct PendingWithdrawal {
    identity: ComposerQueueIdentity,
    command: WithdrawQueuedMessageCommand,
    intent: QueueWithdrawalIntent,
}

/// One exact recall read awaiting its byte-bearing result.
struct PendingRecallRead {
    identity: ComposerQueueIdentity,
    query: ReadRecalledMessage,
    target: ComposerRecallTarget,
    dispatched: bool,
}

/// Payload and opaque empty-composer target held for a restore attempt.
///
/// The payload is intentionally owned here, rather than borrowed from a
/// response or a GPUI task. Callers move it into
/// `NativeComposer::restore_recalled_payload`; if that method returns the
/// payload, callers must rebuild this value and call
/// [`ComposerQueueState::retain_restore_candidate`].
pub(crate) struct RecallRestoreCandidate {
    /// Exact queued row identity.
    pub(crate) identity: ComposerQueueIdentity,
    /// Thread that originally owned the queued row.
    pub(crate) thread_id: ThreadId,
    /// Forge-minted message identity.
    pub(crate) message_id: MessageId,
    /// Original queue request identity.
    pub(crate) original_request_id: RequestId,
    /// Opaque target captured before the withdrawal request.
    pub(crate) target: ComposerRecallTarget,
    /// Complete validated text/images, including exact `None` text presence.
    pub(crate) payload: artisan_domain::QueueMessagePayload,
}

impl RecallRestoreCandidate {
    fn new(
        identity: ComposerQueueIdentity,
        thread_id: ThreadId,
        message_id: MessageId,
        original_request_id: RequestId,
        target: ComposerRecallTarget,
        payload: artisan_domain::QueueMessagePayload,
    ) -> Self {
        Self {
            identity,
            thread_id,
            message_id,
            original_request_id,
            target,
            payload,
        }
    }

    /// Returns the target without exposing its private fields.
    #[must_use]
    pub(crate) fn target(&self) -> &ComposerRecallTarget {
        &self.target
    }

    /// Returns the owned payload without cloning its image bytes.
    #[must_use]
    pub(crate) fn payload(&self) -> &artisan_domain::QueueMessagePayload {
        &self.payload
    }

    /// Rebuilds the candidate after `restore_recalled_payload` returns its
    /// unchanged owned payload.
    #[must_use]
    pub(crate) fn with_payload(self, payload: artisan_domain::QueueMessagePayload) -> Self {
        Self { payload, ..self }
    }

    /// Rebinds the same exact payload to a newly captured empty-composer
    /// target after an explicit user retry.
    #[must_use]
    pub(crate) fn with_target(self, target: ComposerRecallTarget) -> Self {
        Self { target, ..self }
    }
}

impl fmt::Debug for RecallRestoreCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecallRestoreCandidate")
            .field("identity", &self.identity)
            .field("thread_id", &self.thread_id)
            .field("message_id", &self.message_id)
            .field("original_request_id", &self.original_request_id)
            .field("payload_text_present", &self.payload.text().is_some())
            .field("attachment_count", &self.payload.attachments().len())
            .field("attachment_bytes", &self.payload.total_attachment_bytes())
            .finish()
    }
}

/// Whether a withdrawal receipt was newly acted upon or already consumed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WithdrawalReceiptDisposition {
    /// Edit withdrawal committed; the parent must issue this exact read.
    EditNeedsRead(ReadRecalledMessage),
    /// Discard withdrawal committed; the parent must refresh the listing.
    Discarded,
    /// The dispatch passed the durable withdrawal fence.
    TooLate,
    /// The exact message was not eligible for withdrawal.
    NotQueued,
    /// A delayed duplicate receipt was already handled.
    DuplicateHandled,
}

/// Why a withdrawal receipt was rejected without consuming any pending state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WithdrawalReceiptRejection {
    /// No operation owns this receipt's request id and target.
    NoPendingOperation,
    /// The receipt does not match the exact pending request/target.
    TargetMismatch,
}

/// Owned receipt rejection. Returning the receipt keeps delayed data visible
/// to the caller instead of silently dropping it.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct WithdrawalReceiptError {
    /// Unconsumed authoritative receipt.
    pub(crate) result: QueuedMessageWithdrawalResult,
    /// Exact correlation failure.
    pub(crate) reason: WithdrawalReceiptRejection,
}

/// Why a recalled payload result was rejected without consuming it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecalledMessageRejection {
    /// No exact read is pending.
    NoPendingRead,
    /// The response named a different thread/message/original request.
    ScopeMismatch,
    /// A restore candidate is already held and cannot be overwritten.
    RestoreAlreadyHeld,
}

/// Owned recalled-message rejection. It preserves the complete payload for a
/// caller that wants to retain or explicitly retry it.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct RecalledMessageError {
    /// Unconsumed read result, including any image bytes.
    pub(crate) result: RecalledMessageResult,
    /// Exact correlation failure.
    pub(crate) reason: RecalledMessageRejection,
}

/// Result of accepting one exact recalled-message read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecalledMessageDisposition {
    /// A complete payload is now held as a restore candidate.
    PayloadReady,
    /// Forge had no readable payload for the exact withdrawn row.
    NoPayload,
}

/// A generation/thread/serial fence for one bounded queue refresh.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueueRefreshToken {
    thread_id: ThreadId,
    generation: u64,
    serial: u64,
}

impl QueueRefreshToken {
    /// Returns the thread fenced by this refresh.
    #[must_use]
    pub(crate) fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the composer generation fenced by this refresh.
    #[must_use]
    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the local serial used to prevent overlap.
    #[must_use]
    pub(crate) const fn serial(&self) -> u64 {
        self.serial
    }
}

/// Generation/run/sequence fence for one exact usage read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageReadToken {
    thread_id: ThreadId,
    run_id: RunId,
    generation: u64,
    sequence: u64,
}

impl UsageReadToken {
    /// Returns the exact thread read scope.
    #[must_use]
    pub(crate) fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the exact run read scope.
    #[must_use]
    pub(crate) fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Returns the app generation that owns this read.
    #[must_use]
    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the monotonic application read sequence.
    #[must_use]
    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// Why a queue listing could not be installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueListingRejection {
    /// A newer scope or refresh owns the state.
    StaleRefresh,
    /// The response names another thread.
    WrongThread,
    /// The response exceeds the native page bound.
    TooManyRows,
    /// Two visible rows reuse one original queue command id, making a
    /// controls event ambiguous.
    DuplicateCommandId,
    /// One row contains out-of-scope or oversized image metadata.
    InvalidAttachments,
}

/// Immutable policy identity for one run-usage read.
#[derive(Clone, Debug, Eq, PartialEq)]
struct UsageScope {
    thread_id: ThreadId,
    run_id: RunId,
    generation: u64,
    model_id: EngineModelId,
    route_id: EngineRouteId,
    variant_id: Option<EngineVariantId>,
}

/// Last accepted provider report and its catalog label.
#[derive(Clone, Debug, Eq, PartialEq)]
struct UsageSnapshot {
    report: RunUsageReport,
    model_name: String,
}

/// Pure report projection passed to the UI adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReportingUsage {
    /// Exact reporting run id.
    pub(crate) run_id: String,
    /// Exact provider route, used as the existing engine-id display field.
    pub(crate) engine_id: String,
    /// Exact reporting model id.
    pub(crate) model_id: String,
    /// Catalog label for the reporting model only.
    pub(crate) model_name: String,
    /// Optional provider context numerator.
    pub(crate) context_tokens: Option<u64>,
    /// Optional provider context denominator.
    pub(crate) context_window_tokens: Option<u64>,
    /// Optional provider input breakdown.
    pub(crate) input_tokens: Option<u64>,
    /// Optional provider cached-input breakdown.
    pub(crate) cached_input_tokens: Option<u64>,
    /// Optional provider output breakdown.
    pub(crate) output_tokens: Option<u64>,
}

/// Why an immutable usage result cannot be installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UsageRejection {
    /// There is no current run scope.
    NoScope,
    /// The result does not correspond to an outstanding read.
    NoPendingRead,
    /// The app generation no longer owns the read.
    StaleGeneration,
    /// The result names another thread.
    WrongThread,
    /// The result names another run.
    WrongRun,
    /// The provider report names another launch policy.
    PolicyMismatch,
    /// The provider sequence is old or a duplicate of an accepted report.
    StaleSequence,
}

/// Owned usage result rejected by the state fence.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct UsageResultError {
    /// Unconsumed result for diagnostics/retry policy.
    pub(crate) result: RunUsageResult,
    /// Exact stale/mismatch reason.
    pub(crate) reason: UsageRejection,
}

/// Outcome of accepting one exact usage read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UsageResultDisposition {
    /// A newer provider report replaced the previous report.
    Updated,
    /// The result was scoped correctly but contained no report; prior usage
    /// remains untouched when one already exists.
    NoReport,
}

/// Parent-controlled admission state for bounded queue polling.
#[derive(Default)]
struct QueueRefreshState {
    next_serial: u64,
    in_flight: Option<QueueRefreshToken>,
}

/// Complete queue/usage projection owned by the native application.
pub(crate) struct ComposerQueueState {
    current_thread: Option<ThreadId>,
    current_generation: u64,
    entries: Vec<ComposerQueueEntry>,
    total_count: u64,
    has_more: bool,
    order: QueuedMessageListOrder,
    refresh: QueueRefreshState,
    failed_entries: Vec<FailedQueueEntry>,
    failed_total_count: u64,
    failed_has_more: bool,
    failed_refresh: QueueRefreshState,
    pending_withdrawal: Option<PendingWithdrawal>,
    pending_recall_read: Option<PendingRecallRead>,
    restore_candidate: Option<RecallRestoreCandidate>,
    completed_withdrawals: VecDeque<CompletedWithdrawal>,
    terminal_identity: Option<ComposerQueueIdentity>,
    status: QueueStatus,
    usage_scope: Option<UsageScope>,
    usage: Option<UsageSnapshot>,
    usage_sequence: Option<u64>,
    usage_read_in_flight: Option<UsageReadToken>,
    next_usage_read_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompletedWithdrawal {
    request_id: RequestId,
    thread_id: ThreadId,
    message_id: MessageId,
    original_request_id: RequestId,
    outcome: QueuedMessageWithdrawalOutcome,
}

impl Default for ComposerQueueState {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ComposerQueueState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComposerQueueState")
            .field("current_thread", &self.current_thread)
            .field("current_generation", &self.current_generation)
            .field("entry_count", &self.entries.len())
            .field("total_count", &self.total_count)
            .field("has_more", &self.has_more)
            .field("refresh_in_flight", &self.refresh.in_flight.is_some())
            .field("failed_entry_count", &self.failed_entries.len())
            .field("failed_total_count", &self.failed_total_count)
            .field("failed_refresh_in_flight", &self.failed_refresh.in_flight.is_some())
            .field("withdrawal_pending", &self.pending_withdrawal.is_some())
            .field("recall_read_pending", &self.pending_recall_read.is_some())
            .field("restore_candidate", &self.restore_candidate)
            .field("status", &self.status)
            .field("usage_scope", &self.usage_scope)
            .field("usage_present", &self.usage.is_some())
            .finish()
    }
}

impl ComposerQueueState {
    /// Creates an empty, non-polling projection.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            current_thread: None,
            current_generation: 0,
            entries: Vec::new(),
            total_count: 0,
            has_more: false,
            order: QueuedMessageListOrder::OldestFirst,
            refresh: QueueRefreshState::default(),
            failed_entries: Vec::new(),
            failed_total_count: 0,
            failed_has_more: false,
            failed_refresh: QueueRefreshState::default(),
            pending_withdrawal: None,
            pending_recall_read: None,
            restore_candidate: None,
            completed_withdrawals: VecDeque::with_capacity(COMPLETED_WITHDRAWAL_HISTORY_LIMIT),
            terminal_identity: None,
            status: QueueStatus::Idle,
            usage_scope: None,
            usage: None,
            usage_sequence: None,
            usage_read_in_flight: None,
            next_usage_read_sequence: 0,
        }
    }

    /// Replaces the mounted thread/generation and fences old queue pages.
    ///
    /// Pending withdrawal/read/candidate ownership is intentionally retained:
    /// a response can arrive after a thread switch, and a byte-bearing recall
    /// result must still be offered back to the caller rather than dropped.
    pub(crate) fn set_scope(&mut self, thread_id: Option<ThreadId>, generation: u64) {
        if self.current_thread == thread_id && self.current_generation == generation {
            return;
        }
        let thread_changed = self.current_thread != thread_id;
        self.current_thread = thread_id;
        self.current_generation = generation;
        self.entries.clear();
        self.total_count = 0;
        self.has_more = false;
        self.refresh.in_flight = None;
        self.failed_entries.clear();
        self.failed_total_count = 0;
        self.failed_has_more = false;
        self.failed_refresh.in_flight = None;
        self.terminal_identity = None;
        if thread_changed {
            self.usage_scope = None;
            self.usage = None;
            self.usage_sequence = None;
            self.usage_read_in_flight = None;
            self.next_usage_read_sequence = 0;
        } else if self
            .usage_scope
            .as_ref()
            .is_some_and(|scope| scope.generation != generation)
        {
            self.usage_scope = None;
            self.usage = None;
            self.usage_sequence = None;
            self.usage_read_in_flight = None;
            self.next_usage_read_sequence = 0;
        }
        self.status = if self.pending_withdrawal.is_some() {
            QueueStatus::WithdrawalPending
        } else if self.pending_recall_read.is_some() {
            QueueStatus::RecallReadPending
        } else if self.restore_candidate.is_some() {
            QueueStatus::RestoreBlocked
        } else {
            QueueStatus::Idle
        };
    }

    /// Returns the current thread, when a real thread is mounted.
    #[must_use]
    pub(crate) fn current_thread(&self) -> Option<&ThreadId> {
        self.current_thread.as_ref()
    }

    /// Returns the current app/composer generation.
    #[must_use]
    pub(crate) const fn current_generation(&self) -> u64 {
        self.current_generation
    }

    /// Starts one bounded refresh if the parent-visible conditions permit it.
    ///
    /// `force` is for relevant authoritative events such as a newly accepted
    /// queue receipt or a withdrawal receipt. Slow polling passes `false` and
    /// runs only while a queue row or active run exists.
    pub(crate) fn begin_queue_refresh(
        &mut self,
        visible: bool,
        service_ready: bool,
        stopped: bool,
        run_active: bool,
        force: bool,
    ) -> Option<QueueRefreshToken> {
        let thread_id = self.current_thread.clone()?;
        if !visible || !service_ready || stopped || self.refresh.in_flight.is_some() {
            return None;
        }
        if !force && !run_active && self.total_count == 0 {
            return None;
        }
        let serial = self.refresh.next_serial.checked_add(1)?;
        self.refresh.next_serial = serial;
        let token = QueueRefreshToken {
            thread_id,
            generation: self.current_generation,
            serial,
        };
        self.refresh.in_flight = Some(token.clone());
        if matches!(self.status, QueueStatus::Idle | QueueStatus::Restored) {
            self.status = QueueStatus::Refreshing;
        }
        Some(token)
    }

    /// Returns whether one queue listing is currently outstanding.
    #[must_use]
    pub(crate) fn queue_refresh_in_flight(&self) -> bool {
        self.refresh.in_flight.is_some()
    }

    /// Returns whether slow polling is still useful for the current scope.
    #[must_use]
    pub(crate) fn slow_poll_is_eligible(&self, run_active: bool) -> bool {
        self.current_thread.is_some()
            && self.refresh.in_flight.is_none()
            && (run_active || self.total_count > 0)
    }

    /// Installs one exact bounded queue page and clears its matching refresh.
    pub(crate) fn apply_queue_listing(
        &mut self,
        token: &QueueRefreshToken,
        listing: QueuedMessageListing,
    ) -> Result<(), QueueListingRejection> {
        if self.refresh.in_flight.as_ref() != Some(token)
            || self.current_generation != token.generation
            || self.current_thread.as_ref() != Some(&token.thread_id)
        {
            return Err(QueueListingRejection::StaleRefresh);
        }
        self.refresh.in_flight = None;
        if listing.thread_id() != &token.thread_id {
            self.status = QueueStatus::TransportFailed;
            return Err(QueueListingRejection::WrongThread);
        }
        if listing.messages().len() > COMPOSER_QUEUE_PAGE_LIMIT {
            self.status = QueueStatus::TransportFailed;
            return Err(QueueListingRejection::TooManyRows);
        }
        self.order = listing.order();
        self.total_count = listing.total_count();
        self.has_more = listing.has_more();
        let mut seen_commands = HashSet::with_capacity(listing.messages().len());
        let mut entries = Vec::with_capacity(listing.messages().len());
        for summary in listing.messages() {
            let Some(entry) = ComposerQueueEntry::from_summary(summary, token.generation) else {
                self.status = QueueStatus::TransportFailed;
                return Err(QueueListingRejection::InvalidAttachments);
            };
            if !seen_commands.insert(entry.identity.command_id.clone()) {
                self.status = QueueStatus::TransportFailed;
                return Err(QueueListingRejection::DuplicateCommandId);
            }
            entries.push(entry);
        }
        self.entries = entries;
        if self.terminal_identity.as_ref().is_some_and(|identity| {
            !self
                .entries
                .iter()
                .any(|entry| entry.identity() == identity)
        }) {
            self.terminal_identity = None;
        }
        self.status = if self.pending_withdrawal.is_some() {
            QueueStatus::WithdrawalPending
        } else if self.pending_recall_read.is_some() {
            QueueStatus::RecallReadPending
        } else if self.restore_candidate.is_some() {
            QueueStatus::RestoreBlocked
        } else if matches!(
            self.status,
            QueueStatus::TooLate | QueueStatus::NotQueued | QueueStatus::TransportFailed
        ) && self.terminal_identity.is_some()
        {
            self.status
        } else {
            QueueStatus::Idle
        };
        Ok(())
    }

    /// Clears a refresh only when the completion belongs to its exact token.
    pub(crate) fn finish_queue_refresh(&mut self, token: &QueueRefreshToken) -> bool {
        if self.refresh.in_flight.as_ref() != Some(token) {
            return false;
        }
        self.refresh.in_flight = None;
        if self.pending_withdrawal.is_none()
            && self.pending_recall_read.is_none()
            && self.restore_candidate.is_none()
            && self.status == QueueStatus::Refreshing
        {
            self.status = QueueStatus::Idle;
        }
        true
    }

    /// Starts one bounded failed-dispatch refresh if the parent-visible
    /// conditions permit it.
    ///
    /// `force` is for relevant authoritative events such as a newly observed
    /// dispatch failure. Slow polling passes `false` and runs only while a
    /// failed row exists. The failed read never disturbs the queued refresh
    /// fence: both pages are independent projections of one thread.
    pub(crate) fn begin_failed_refresh(
        &mut self,
        visible: bool,
        service_ready: bool,
        stopped: bool,
        force: bool,
    ) -> Option<QueueRefreshToken> {
        let thread_id = self.current_thread.clone()?;
        if !visible || !service_ready || stopped || self.failed_refresh.in_flight.is_some() {
            return None;
        }
        if !force && self.failed_total_count == 0 {
            return None;
        }
        let serial = self.failed_refresh.next_serial.checked_add(1)?;
        self.failed_refresh.next_serial = serial;
        let token = QueueRefreshToken {
            thread_id,
            generation: self.current_generation,
            serial,
        };
        self.failed_refresh.in_flight = Some(token.clone());
        Some(token)
    }

    /// Returns whether one failed-dispatch listing is currently outstanding.
    #[must_use]
    pub(crate) fn failed_refresh_in_flight(&self) -> bool {
        self.failed_refresh.in_flight.is_some()
    }

    /// Installs one exact bounded failed-dispatch page and clears its
    /// matching refresh.
    pub(crate) fn apply_failed_listing(
        &mut self,
        token: &QueueRefreshToken,
        listing: FailedMessageListing,
    ) -> Result<(), FailedListingRejection> {
        if self.failed_refresh.in_flight.as_ref() != Some(token)
            || self.current_generation != token.generation
            || self.current_thread.as_ref() != Some(&token.thread_id)
        {
            return Err(FailedListingRejection::StaleRefresh);
        }
        self.failed_refresh.in_flight = None;
        if listing.thread_id() != &token.thread_id {
            return Err(FailedListingRejection::WrongThread);
        }
        if listing.messages().len() > COMPOSER_QUEUE_PAGE_LIMIT {
            return Err(FailedListingRejection::TooManyRows);
        }
        self.failed_total_count = listing.total_count();
        self.failed_has_more = listing.has_more();
        let mut seen_commands = HashSet::with_capacity(listing.messages().len());
        let mut entries = Vec::with_capacity(listing.messages().len());
        for summary in listing.messages() {
            let Some(entry) = FailedQueueEntry::from_summary(summary, token.generation) else {
                return Err(FailedListingRejection::InvalidAttachments);
            };
            if !seen_commands.insert(entry.identity.command_id.clone()) {
                return Err(FailedListingRejection::DuplicateCommandId);
            }
            entries.push(entry);
        }
        self.failed_entries = entries;
        Ok(())
    }

    /// Clears a failed refresh only when the completion belongs to its exact
    /// token.
    pub(crate) fn finish_failed_refresh(&mut self, token: &QueueRefreshToken) -> bool {
        if self.failed_refresh.in_flight.as_ref() != Some(token) {
            return false;
        }
        self.failed_refresh.in_flight = None;
        true
    }

    /// Returns the installed failed-dispatch rows, newest failures first.
    #[must_use]
    pub(crate) fn failed_entries(&self) -> &[FailedQueueEntry] {
        &self.failed_entries
    }

    /// Returns one exact failed row by its Forge-minted message identity,
    /// fenced to the current generation by the caller.
    #[must_use]
    pub(crate) fn failed_entry(&self, message_id: &MessageId) -> Option<&FailedQueueEntry> {
        self.failed_entries
            .iter()
            .find(|entry| entry.message_id() == message_id)
    }

    /// Returns the exact failed-row count at read time.
    #[must_use]
    pub(crate) const fn failed_total_count(&self) -> u64 {
        self.failed_total_count
    }

    /// Invalidates the current refresh without touching payload ownership.
    pub(crate) fn cancel_queue_refresh(&mut self) {
        self.refresh.in_flight = None;
        if self.status == QueueStatus::Refreshing {
            self.status = QueueStatus::Idle;
        }
    }

    /// Returns the byte-free rows in the authoritative page order.
    #[must_use]
    pub(crate) fn entries(&self) -> &[ComposerQueueEntry] {
        &self.entries
    }

    /// Returns the exact Forge count, including rows beyond this bounded page.
    #[must_use]
    pub(crate) const fn total_count(&self) -> u64 {
        self.total_count
    }

    /// Returns whether the page omitted eligible rows.
    #[must_use]
    pub(crate) const fn has_more(&self) -> bool {
        self.has_more
    }

    /// Returns the stable page direction used by the current projection.
    #[must_use]
    pub(crate) const fn order(&self) -> QueuedMessageListOrder {
        self.order
    }

    /// Projects rows into the existing controls contract.
    #[must_use]
    pub(crate) fn pending_lip_rows(&self) -> Vec<QueueLipRow> {
        self.entries
            .iter()
            .map(|entry| QueueLipRow {
                command_id: entry.identity.command_id.clone(),
                generation: entry.identity.generation,
                text: entry.lip_text().to_owned(),
                editable: self.row_is_editable(entry),
            })
            .collect()
    }

    fn row_is_editable(&self, entry: &ComposerQueueEntry) -> bool {
        self.pending_withdrawal.is_none()
            && self.pending_recall_read.is_none()
            && self.restore_candidate.is_none()
            && self.current_thread.as_ref() == Some(&entry.thread_id)
            && self.current_generation == entry.identity.generation
            && self.terminal_identity.as_ref() != Some(entry.identity())
    }

    /// Finds the exact byte-free row for a controls event.
    #[must_use]
    pub(crate) fn entry_for_identity(
        &self,
        identity: &ComposerQueueIdentity,
    ) -> Option<&ComposerQueueEntry> {
        self.entries
            .iter()
            .find(|entry| entry.identity() == identity)
    }

    /// Starts an edit withdrawal after the parent captured the exact empty
    /// composer target.
    pub(crate) fn begin_edit(
        &mut self,
        identity: &ComposerQueueIdentity,
        request_id: RequestId,
        target: ComposerRecallTarget,
    ) -> Result<WithdrawQueuedMessageCommand, QueueIntentError> {
        self.begin_withdrawal(identity, request_id, QueueWithdrawalIntent::Edit { target })
    }

    /// Starts a discard withdrawal without ever reading image bytes.
    pub(crate) fn begin_discard(
        &mut self,
        identity: &ComposerQueueIdentity,
        request_id: RequestId,
    ) -> Result<WithdrawQueuedMessageCommand, QueueIntentError> {
        self.begin_withdrawal(identity, request_id, QueueWithdrawalIntent::Discard)
    }

    fn begin_withdrawal(
        &mut self,
        identity: &ComposerQueueIdentity,
        request_id: RequestId,
        intent: QueueWithdrawalIntent,
    ) -> Result<WithdrawQueuedMessageCommand, QueueIntentError> {
        let Some(current_thread) = self.current_thread.as_ref() else {
            return Err(QueueIntentError::NoThread);
        };
        if self.pending_withdrawal.is_some()
            || self.pending_recall_read.is_some()
            || self.restore_candidate.is_some()
        {
            return Err(QueueIntentError::Busy);
        }
        let Some(entry) = self.entry_for_identity(identity) else {
            return Err(QueueIntentError::UnknownRow);
        };
        if current_thread != &entry.thread_id
            || self.current_generation != entry.identity.generation
            || !self.row_is_editable(entry)
        {
            return Err(QueueIntentError::StaleRow);
        }
        let command = WithdrawQueuedMessageCommand::new(
            request_id,
            entry.thread_id.clone(),
            entry.message_id.clone(),
            entry.original_request_id.clone(),
        );
        self.pending_withdrawal = Some(PendingWithdrawal {
            identity: identity.clone(),
            command: command.clone(),
            intent,
        });
        self.status = QueueStatus::WithdrawalPending;
        Ok(command)
    }

    /// Returns the exact pending withdrawal command for an explicit transport
    /// retry. Retrying this value preserves the original request/frame id.
    #[must_use]
    pub(crate) fn pending_withdrawal(&self) -> Option<&WithdrawQueuedMessageCommand> {
        self.pending_withdrawal
            .as_ref()
            .map(|pending| &pending.command)
    }

    /// Marks the pending withdrawal as sent again after a retry decision.
    pub(crate) fn retry_pending_withdrawal(&mut self) -> Option<WithdrawQueuedMessageCommand> {
        let command = self.pending_withdrawal.as_ref()?.command.clone();
        self.status = QueueStatus::WithdrawalPending;
        Some(command)
    }

    /// Leaves the exact withdrawal command pending after a service failure.
    pub(crate) fn mark_withdrawal_failed(&mut self) -> bool {
        if self.pending_withdrawal.is_none() {
            return false;
        }
        self.status = QueueStatus::TransportFailed;
        true
    }

    /// Consumes only a receipt that matches request id and all target ids.
    ///
    /// `TooLate` and `NotQueued` are surfaced as statuses and never trigger a
    /// read. A delayed duplicate of a completed exact receipt is harmless and
    /// does not issue a second command.
    pub(crate) fn accept_withdrawal_result(
        &mut self,
        result: QueuedMessageWithdrawalResult,
    ) -> Result<WithdrawalReceiptDisposition, WithdrawalReceiptError> {
        let matches_pending = self.pending_withdrawal.as_ref().is_some_and(|pending| {
            result.withdrawal_request_id() == pending.command.request_id()
                && result.thread_id == pending.command.thread_id
                && result.message_id == pending.command.message_id
                && result.original_request_id == pending.command.original_request_id
        });
        if !matches_pending {
            let duplicate = self.completed_withdrawals.iter().any(|completed| {
                result.withdrawal_request_id() == &completed.request_id
                    && result.thread_id == completed.thread_id
                    && result.message_id == completed.message_id
                    && result.original_request_id == completed.original_request_id
                    && result.outcome == completed.outcome
            });
            if duplicate {
                return Ok(WithdrawalReceiptDisposition::DuplicateHandled);
            }
            let reason = if self.pending_withdrawal.is_some() {
                WithdrawalReceiptRejection::TargetMismatch
            } else {
                WithdrawalReceiptRejection::NoPendingOperation
            };
            return Err(WithdrawalReceiptError { result, reason });
        }

        let pending = self
            .pending_withdrawal
            .take()
            .expect("pending withdrawal matched above");
        self.remember_completed_withdrawal(&pending.command, &result);
        match result.outcome {
            QueuedMessageWithdrawalOutcome::Withdrawn => match pending.intent {
                QueueWithdrawalIntent::Edit { target } => {
                    let query = ReadRecalledMessage::new(
                        pending.command.thread_id.clone(),
                        pending.command.message_id.clone(),
                        pending.command.original_request_id.clone(),
                    );
                    self.pending_recall_read = Some(PendingRecallRead {
                        identity: pending.identity,
                        query: query.clone(),
                        target,
                        dispatched: false,
                    });
                    self.status = QueueStatus::RecallReadPending;
                    Ok(WithdrawalReceiptDisposition::EditNeedsRead(query))
                }
                QueueWithdrawalIntent::Discard => {
                    self.terminal_identity = Some(pending.identity);
                    self.status = QueueStatus::WithdrawnAwaitingRefresh;
                    Ok(WithdrawalReceiptDisposition::Discarded)
                }
            },
            QueuedMessageWithdrawalOutcome::TooLate => {
                self.terminal_identity = Some(pending.identity);
                self.status = QueueStatus::TooLate;
                Ok(WithdrawalReceiptDisposition::TooLate)
            }
            QueuedMessageWithdrawalOutcome::NotQueued => {
                self.terminal_identity = Some(pending.identity);
                self.status = QueueStatus::NotQueued;
                Ok(WithdrawalReceiptDisposition::NotQueued)
            }
        }
    }

    fn remember_completed_withdrawal(
        &mut self,
        command: &WithdrawQueuedMessageCommand,
        result: &QueuedMessageWithdrawalResult,
    ) {
        if self.completed_withdrawals.len() >= COMPLETED_WITHDRAWAL_HISTORY_LIMIT {
            self.completed_withdrawals.pop_front();
        }
        self.completed_withdrawals.push_back(CompletedWithdrawal {
            request_id: command.request_id.clone(),
            thread_id: command.thread_id.clone(),
            message_id: command.message_id.clone(),
            original_request_id: command.original_request_id.clone(),
            outcome: result.outcome,
        });
    }

    /// Returns the next exact recalled-message query once.
    pub(crate) fn next_recalled_message_read(&mut self) -> Option<ReadRecalledMessage> {
        let pending = self.pending_recall_read.as_mut()?;
        if pending.dispatched {
            return None;
        }
        pending.dispatched = true;
        Some(pending.query.clone())
    }

    pub(crate) fn can_retry_recalled_read(&self) -> bool {
        self.pending_recall_read
            .as_ref()
            .is_some_and(|read| !read.dispatched)
    }

    /// Reopens one exact recalled read for a deliberate transport retry.
    pub(crate) fn mark_recalled_read_failed(&mut self, query: &ReadRecalledMessage) -> bool {
        let Some(pending) = self.pending_recall_read.as_mut() else {
            return false;
        };
        if &pending.query != query {
            return false;
        }
        pending.dispatched = false;
        self.status = QueueStatus::RecallReadPending;
        true
    }

    /// Accepts a byte-bearing response only for the exact pending read.
    ///
    /// The pending read is allowed to complete after a thread/generation
    /// change so the payload can be offered back to the caller. The target's
    /// own GPUI precondition then decides whether it can be restored now.
    pub(crate) fn accept_recalled_message(
        &mut self,
        result: RecalledMessageResult,
    ) -> Result<RecalledMessageDisposition, RecalledMessageError> {
        let Some(pending) = self.pending_recall_read.as_ref() else {
            return Err(RecalledMessageError {
                result,
                reason: RecalledMessageRejection::NoPendingRead,
            });
        };
        if result.thread_id != pending.query.thread_id
            || result.message_id != pending.query.message_id
            || result.original_request_id != pending.query.original_request_id
        {
            return Err(RecalledMessageError {
                result,
                reason: RecalledMessageRejection::ScopeMismatch,
            });
        }
        if self.restore_candidate.is_some() {
            return Err(RecalledMessageError {
                result,
                reason: RecalledMessageRejection::RestoreAlreadyHeld,
            });
        }
        let pending = self
            .pending_recall_read
            .take()
            .expect("pending recalled read matched above");
        let Some(payload) = result.payload else {
            self.terminal_identity = Some(pending.identity);
            self.status = QueueStatus::PayloadUnavailable;
            return Ok(RecalledMessageDisposition::NoPayload);
        };
        self.restore_candidate = Some(RecallRestoreCandidate::new(
            pending.identity,
            pending.query.thread_id.clone(),
            pending.query.message_id.clone(),
            pending.query.original_request_id.clone(),
            pending.target,
            payload,
        ));
        self.status = QueueStatus::RestoreReady;
        Ok(RecalledMessageDisposition::PayloadReady)
    }

    /// Returns the owned candidate for one immediate or explicit retry.
    pub(crate) fn take_restore_candidate(&mut self) -> Option<RecallRestoreCandidate> {
        self.restore_candidate.take()
    }

    /// Rebinds the held payload to a newly captured target without copying
    /// image bytes. The caller must have made this an explicit user retry.
    pub(crate) fn take_restore_candidate_with_target(
        &mut self,
        target: ComposerRecallTarget,
    ) -> Option<RecallRestoreCandidate> {
        self.restore_candidate
            .take()
            .map(|candidate| candidate.with_target(target))
    }

    /// Retains the unchanged payload after the composer rejected a restore.
    ///
    /// If another candidate appeared concurrently, the returned candidate is
    /// still owned by the caller; this method never overwrites or drops it.
    pub(crate) fn retain_restore_candidate(
        &mut self,
        candidate: RecallRestoreCandidate,
    ) -> Result<(), RecallRestoreCandidate> {
        if self.restore_candidate.is_some() {
            return Err(candidate);
        }
        self.restore_candidate = Some(candidate);
        self.status = QueueStatus::RestoreBlocked;
        Ok(())
    }

    /// Marks a candidate accepted by the composer and requests a fresh queue
    /// projection. No payload remains owned after the caller moved it into the
    /// composer.
    pub(crate) fn mark_restore_succeeded(&mut self, identity: &ComposerQueueIdentity) -> bool {
        if self.restore_candidate.is_some() {
            return false;
        }
        self.terminal_identity = Some(identity.clone());
        self.status = QueueStatus::Restored;
        true
    }

    /// Returns the current optional recovery candidate status.
    #[must_use]
    pub(crate) const fn restore_retry_available(&self) -> bool {
        self.restore_candidate.is_some()
    }

    /// Returns the current truthful queue status.
    #[must_use]
    pub(crate) const fn status(&self) -> QueueStatus {
        self.status
    }

    /// Returns whether an explicit retry can offer a candidate again.
    #[must_use]
    pub(crate) fn can_retry_restore(&self) -> bool {
        self.restore_candidate.is_some()
    }

    /// Records a transport failure while retaining the exact pending command.
    pub(crate) fn mark_transport_failure(&mut self) {
        if self.pending_withdrawal.is_some() || self.pending_recall_read.is_some() {
            self.status = QueueStatus::TransportFailed;
        }
    }

    /// Starts or retains an immutable reporting scope for one exact run.
    ///
    /// A matching scope keeps its last report after settlement. A new run,
    /// policy, thread, or generation clears the previous report before the
    /// next read is admitted.
    pub(crate) fn begin_usage_scope(
        &mut self,
        thread_id: ThreadId,
        generation: u64,
        run_id: RunId,
        model_id: EngineModelId,
        route_id: EngineRouteId,
        variant_id: Option<EngineVariantId>,
    ) -> bool {
        if self.current_thread.as_ref() != Some(&thread_id) || self.current_generation != generation
        {
            return false;
        }
        let next = UsageScope {
            thread_id,
            run_id,
            generation,
            model_id,
            route_id,
            variant_id,
        };
        if self.usage_scope.as_ref() != Some(&next) {
            self.usage_scope = Some(next);
            self.usage = None;
            self.usage_sequence = None;
            self.usage_read_in_flight = None;
            self.next_usage_read_sequence = 0;
        }
        true
    }

    /// Returns the immutable run currently used for usage reads.
    #[must_use]
    pub(crate) fn usage_scope(&self) -> Option<(&ThreadId, &RunId, u64)> {
        self.usage_scope
            .as_ref()
            .map(|scope| (&scope.thread_id, &scope.run_id, scope.generation))
    }

    /// Starts one exact usage read without allowing overlap.
    pub(crate) fn begin_usage_read(
        &mut self,
    ) -> Option<(UsageReadToken, artisan_domain::ReadRunUsage)> {
        let scope = self.usage_scope.as_ref()?;
        if self.usage_read_in_flight.is_some() {
            return None;
        }
        let sequence = self.next_usage_read_sequence.checked_add(1)?;
        self.next_usage_read_sequence = sequence;
        let token = UsageReadToken {
            thread_id: scope.thread_id.clone(),
            run_id: scope.run_id.clone(),
            generation: scope.generation,
            sequence,
        };
        self.usage_read_in_flight = Some(token.clone());
        Some((
            token,
            artisan_domain::ReadRunUsage::new(scope.thread_id.clone(), scope.run_id.clone()),
        ))
    }

    /// Returns whether an exact usage read is outstanding.
    #[must_use]
    pub(crate) fn usage_read_in_flight(&self) -> bool {
        self.usage_read_in_flight.is_some()
    }

    /// Reopens usage admission after a failed exact read.
    pub(crate) fn mark_usage_read_failed(&mut self, token: &UsageReadToken) -> bool {
        if self.usage_read_in_flight.as_ref() != Some(token) {
            return false;
        }
        self.usage_read_in_flight = None;
        true
    }

    /// Accepts a report only when every app and launch-policy fence agrees.
    ///
    /// `model_name` is a label lookup for the reporting model only. It never
    /// supplies a token denominator or rewrites the report's model identity.
    pub(crate) fn accept_usage_result(
        &mut self,
        result: RunUsageResult,
        token: &UsageReadToken,
        model_name: String,
    ) -> Result<UsageResultDisposition, UsageResultError> {
        let Some(scope) = self.usage_scope.as_ref() else {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::NoScope,
            });
        };
        if scope.generation != token.generation {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::StaleGeneration,
            });
        }
        if self.usage_read_in_flight.as_ref() != Some(token)
            || scope.thread_id != token.thread_id
            || scope.run_id != token.run_id
        {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::NoPendingRead,
            });
        }
        self.usage_read_in_flight = None;
        if result.thread_id != scope.thread_id {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::WrongThread,
            });
        }
        if result.run_id != scope.run_id {
            return Err(UsageResultError {
                result,
                reason: UsageRejection::WrongRun,
            });
        }
        let Some(report) = result.report else {
            return Ok(UsageResultDisposition::NoReport);
        };
        if report.thread_id() != &scope.thread_id
            || report.run_id() != &scope.run_id
            || report.model_id() != &scope.model_id
            || report.provider_route_id() != &scope.route_id
            || report.variant_id() != scope.variant_id.as_ref()
        {
            return Err(UsageResultError {
                result: RunUsageResult {
                    thread_id: scope.thread_id.clone(),
                    run_id: scope.run_id.clone(),
                    report: Some(report),
                },
                reason: UsageRejection::PolicyMismatch,
            });
        }
        if self
            .usage_sequence
            .is_some_and(|sequence| report.source_sequence() <= sequence)
        {
            return Err(UsageResultError {
                result: RunUsageResult {
                    thread_id: scope.thread_id.clone(),
                    run_id: scope.run_id.clone(),
                    report: Some(report),
                },
                reason: UsageRejection::StaleSequence,
            });
        }
        self.usage_sequence = Some(report.source_sequence());
        self.usage = Some(UsageSnapshot { report, model_name });
        Ok(UsageResultDisposition::Updated)
    }

    /// Returns the latest reporting-run usage for the requested run.
    ///
    /// The UI adapter maps `engine_id` to the existing
    /// `NativeContextUsage.reporting_engine_id` field. No selected-model
    /// catalog capacity is consulted.
    #[must_use]
    pub(crate) fn reporting_usage_for(
        &self,
        current_run_id: Option<&str>,
    ) -> Option<ReportingUsage> {
        let scope = self.usage_scope.as_ref()?;
        let usage = self.usage.as_ref()?;
        if current_run_id != Some(scope.run_id.as_str()) {
            return None;
        }
        let report = &usage.report;
        Some(ReportingUsage {
            run_id: report.run_id().as_str().to_owned(),
            engine_id: report.provider_route_id().as_str().to_owned(),
            model_id: report.model_id().as_str().to_owned(),
            model_name: usage.model_name.clone(),
            context_tokens: report.context_tokens(),
            context_window_tokens: report.context_window_tokens(),
            input_tokens: report.input_tokens(),
            cached_input_tokens: report.cached_input_tokens(),
            output_tokens: report.output_tokens(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{CommandReceipt, ReceiptDisposition, RunUsageReportInput};

    fn thread(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("thread id")
    }

    fn request(value: &str) -> RequestId {
        RequestId::parse(value).expect("request id")
    }

    fn message(value: &str) -> MessageId {
        MessageId::parse(value).expect("message id")
    }

    fn listing(
        thread_id: &ThreadId,
        rows: Vec<artisan_domain::QueuedMessageSummary>,
    ) -> QueuedMessageListing {
        QueuedMessageListing::new(
            thread_id.clone(),
            QueuedMessageListOrder::OldestFirst,
            COMPOSER_QUEUE_PAGE_LIMIT,
            rows.len() as u64,
            rows,
        )
        .expect("valid listing")
    }

    fn summary(
        thread_id: &ThreadId,
        id: &str,
        text: Option<&str>,
    ) -> artisan_domain::QueuedMessageSummary {
        artisan_domain::QueuedMessageSummary {
            message_id: message(id),
            thread_id: thread_id.clone(),
            original_request_id: request(&format!("request-{id}")),
            text: text.map(|value| AuthoredText::parse(value).expect("text")),
            attachments: Vec::new(),
            accepted_at: UnixMillis::EPOCH,
            last_error: None,
        }
    }

    fn withdrawal(
        request_id: &str,
        thread_id: &ThreadId,
        message_id: &MessageId,
        original_request_id: &RequestId,
        outcome: QueuedMessageWithdrawalOutcome,
    ) -> QueuedMessageWithdrawalResult {
        QueuedMessageWithdrawalResult {
            receipt: CommandReceipt {
                request_id: request(request_id),
                disposition: ReceiptDisposition::Accepted,
            },
            thread_id: thread_id.clone(),
            message_id: message_id.clone(),
            original_request_id: original_request_id.clone(),
            accepted_at: UnixMillis::EPOCH,
            outcome,
        }
    }

    fn report(
        thread_id: &ThreadId,
        run_id: &RunId,
        source_sequence: u64,
        model_id: &str,
        route_id: &str,
    ) -> RunUsageReport {
        RunUsageReport::new(RunUsageReportInput {
            run_id: run_id.clone(),
            thread_id: thread_id.clone(),
            provider_session_id: "session-1".to_owned(),
            source_sequence,
            model_id: EngineModelId::parse(model_id).expect("model id"),
            provider_route_id: EngineRouteId::parse(route_id).expect("route id"),
            variant_id: None,
            basis: artisan_domain::RunUsageBasis::Delta,
            provider_turn_id: None,
            input_tokens: Some(0),
            cached_input_tokens: None,
            output_tokens: Some(4),
            context_tokens: Some(0),
            context_window_tokens: Some(100),
            observed_at: UnixMillis::EPOCH,
        })
        .expect("usage report")
    }

    #[test]
    fn queue_refresh_is_bounded_and_does_not_overlap() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 4);
        let token = state
            .begin_queue_refresh(true, true, false, false, true)
            .expect("forced refresh");
        assert!(
            state
                .begin_queue_refresh(true, true, false, false, true)
                .is_none()
        );
        assert_eq!(token.generation(), 4);
        state
            .apply_queue_listing(&token, listing(&thread_id, Vec::new()))
            .expect("bounded empty listing");
        assert!(!state.queue_refresh_in_flight());
        assert!(
            state
                .begin_queue_refresh(true, true, false, false, false)
                .is_none()
        );
    }

    #[test]
    fn exact_withdrawal_receipt_ownership_rejects_wrong_target_and_accepts_duplicate() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 9);
        let token = state
            .begin_queue_refresh(true, true, false, false, true)
            .expect("refresh");
        state
            .apply_queue_listing(
                &token,
                listing(&thread_id, vec![summary(&thread_id, "one", Some("keep"))]),
            )
            .expect("listing");
        let identity = state.entries()[0].identity().clone();
        let command = state
            .begin_discard(&identity, request("withdraw-one"))
            .expect("discard intent");
        let wrong = withdrawal(
            "withdraw-other",
            &thread_id,
            command.message_id(),
            command.original_request_id(),
            QueuedMessageWithdrawalOutcome::Withdrawn,
        );
        let error = state
            .accept_withdrawal_result(wrong)
            .expect_err("wrong request must not settle");
        assert_eq!(error.reason, WithdrawalReceiptRejection::TargetMismatch);
        assert!(state.pending_withdrawal().is_some());

        let result = withdrawal(
            "withdraw-one",
            &thread_id,
            command.message_id(),
            command.original_request_id(),
            QueuedMessageWithdrawalOutcome::Withdrawn,
        );
        assert_eq!(
            state.accept_withdrawal_result(result.clone()),
            Ok(WithdrawalReceiptDisposition::Discarded)
        );
        assert_eq!(
            state.accept_withdrawal_result(result),
            Ok(WithdrawalReceiptDisposition::DuplicateHandled)
        );
    }

    #[test]
    fn stale_usage_is_rejected_without_reinterpreting_the_reporting_model() {
        let thread_id = thread("thread-a");
        let run_id = RunId::parse("run-a").expect("run id");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 3);
        assert!(state.begin_usage_scope(
            thread_id.clone(),
            3,
            run_id.clone(),
            EngineModelId::parse("model-a").expect("model"),
            EngineRouteId::parse("route-a").expect("route"),
            None,
        ));
        let (usage_read, _) = state.begin_usage_read().expect("usage read");
        let fresh = RunUsageResult {
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
            report: Some(report(&thread_id, &run_id, 2, "model-a", "route-a")),
        };
        assert_eq!(
            state.accept_usage_result(fresh, &usage_read, "Model A".to_owned()),
            Ok(UsageResultDisposition::Updated)
        );
        let (next_usage_read, _) = state.begin_usage_read().expect("next usage read");
        let stale = RunUsageResult {
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
            report: Some(report(&thread_id, &run_id, 1, "model-a", "route-a")),
        };
        let error = state
            .accept_usage_result(stale, &next_usage_read, "newly selected model".to_owned())
            .expect_err("older source sequence must be rejected");
        assert_eq!(error.reason, UsageRejection::StaleSequence);
        let usage = state
            .reporting_usage_for(Some(run_id.as_str()))
            .expect("last report remains");
        assert_eq!(usage.model_id, "model-a");
        assert_eq!(usage.model_name, "Model A");
        assert_eq!(usage.context_tokens, Some(0));
        assert_eq!(usage.context_window_tokens, Some(100));
    }

    #[test]
    fn absent_usage_fields_remain_absent_and_scope_mismatch_is_rejected() {
        let thread_id = thread("thread-a");
        let run_id = RunId::parse("run-a").expect("run id");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 1);
        assert!(state.begin_usage_scope(
            thread_id.clone(),
            1,
            run_id.clone(),
            EngineModelId::parse("model-a").expect("model"),
            EngineRouteId::parse("route-a").expect("route"),
            None,
        ));
        let absent = RunUsageResult {
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
            report: None,
        };
        let (usage_read, _) = state.begin_usage_read().expect("usage read");
        assert_eq!(
            state.accept_usage_result(absent, &usage_read, "Model A".to_owned()),
            Ok(UsageResultDisposition::NoReport)
        );
        assert!(state.reporting_usage_for(Some(run_id.as_str())).is_none());
        let wrong_thread = RunUsageResult {
            thread_id: thread("thread-b"),
            run_id,
            report: None,
        };
        let (next_usage_read, _) = state.begin_usage_read().expect("next usage read");
        let error = state
            .accept_usage_result(wrong_thread, &next_usage_read, "Model A".to_owned())
            .expect_err("wrong thread must be fenced");
        assert_eq!(error.reason, UsageRejection::WrongThread);
    }

    #[gpui::test]
    fn withdrawn_payload_survives_a_typing_race_for_explicit_retry(cx: &mut gpui::TestAppContext) {
        let (composer, cx) =
            cx.add_window_view(|_, cx| crate::native_composer::NativeComposer::new(cx));
        let thread_id = thread("thread-a");
        let target = cx.update(|_, app| {
            composer.update(app, |composer, composer_cx| {
                composer.switch_thread("thread-a".to_owned(), false, composer_cx);
                composer
                    .capture_recall_target()
                    .expect("empty composer target")
            })
        });

        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 2);
        let refresh = state
            .begin_queue_refresh(true, true, false, false, true)
            .expect("queue refresh");
        state
            .apply_queue_listing(
                &refresh,
                listing(&thread_id, vec![summary(&thread_id, "a", Some("queued"))]),
            )
            .expect("queue listing");
        let identity = state.entries()[0].identity().clone();
        let command = state
            .begin_edit(&identity, request("withdraw-a"), target)
            .expect("edit intent");
        assert_eq!(
            state
                .accept_withdrawal_result(withdrawal(
                    "withdraw-a",
                    &thread_id,
                    command.message_id(),
                    command.original_request_id(),
                    QueuedMessageWithdrawalOutcome::Withdrawn,
                ))
                .expect("withdrawal receipt"),
            WithdrawalReceiptDisposition::EditNeedsRead(ReadRecalledMessage::new(
                thread_id.clone(),
                command.message_id().clone(),
                command.original_request_id().clone(),
            ))
        );
        assert_eq!(
            state.next_recalled_message_read(),
            Some(ReadRecalledMessage::new(
                thread_id.clone(),
                command.message_id().clone(),
                command.original_request_id().clone(),
            ))
        );

        let retry_query = ReadRecalledMessage::new(
            thread_id.clone(),
            command.message_id().clone(),
            command.original_request_id().clone(),
        );
        assert!(!state.can_retry_recalled_read());
        assert!(state.mark_recalled_read_failed(&retry_query));
        assert!(state.can_retry_recalled_read());
        assert_eq!(state.next_recalled_message_read(), Some(retry_query));
        assert!(!state.can_retry_recalled_read());
        assert!(state.next_recalled_message_read().is_none());

        let payload =
            artisan_domain::QueueMessagePayload::text_only("queued text").expect("payload");
        let recalled =
            RecalledMessageResult::new(thread_id, command.message_id().clone(), command.original_request_id().clone(), Some(payload))
                .expect("recalled result");
        assert_eq!(
            state
                .accept_recalled_message(recalled)
                .expect("read result"),
            RecalledMessageDisposition::PayloadReady
        );
        let candidate = state
            .take_restore_candidate()
            .expect("candidate is owned by the state");
        let RecallRestoreCandidate {
            identity,
            thread_id,
            message_id,
            original_request_id,
            target,
            payload,
        } = candidate;

        cx.update(|_, app| {
            composer.update(app, |composer, composer_cx| {
                composer.set_draft("user started typing");
                composer_cx.notify();
            });
        });
        let result = cx.update(|_, app| {
            composer.update(app, |composer, composer_cx| {
                composer.restore_recalled_payload(&target, payload, composer_cx)
            })
        });
        let payload = result.expect_err("typing must not be overwritten");
        state
            .retain_restore_candidate(RecallRestoreCandidate::new(
                identity,
                thread_id,
                message_id,
                original_request_id,
                target,
                payload,
            ))
            .expect("the unchanged payload remains available");
        assert!(state.can_retry_restore());
    }

    fn failed_summary(thread_id: &ThreadId, id: &str) -> artisan_domain::FailedMessageSummary {
        artisan_domain::FailedMessageSummary {
            message_id: message(id),
            thread_id: thread_id.clone(),
            original_request_id: request(&format!("request-{id}")),
            text: Some(AuthoredText::parse("hello").expect("text")),
            attachments: Vec::new(),
            accepted_at: UnixMillis::from_millis(300),
            failed_at: UnixMillis::from_millis(500),
            reason: artisan_domain::DispatchError::parse(
                "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue".to_owned(),
            )
            .expect("diagnostic"),
        }
    }

    fn failed_listing(
        thread_id: &ThreadId,
        rows: Vec<artisan_domain::FailedMessageSummary>,
    ) -> artisan_domain::FailedMessageListing {
        artisan_domain::FailedMessageListing::new(
            thread_id.clone(),
            COMPOSER_QUEUE_PAGE_LIMIT,
            rows.len() as u64,
            rows,
        )
        .expect("valid failed listing")
    }

    #[test]
    fn failed_refresh_installs_exact_rows_without_touching_queue_fence() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 6);
        assert!(
            state
                .begin_failed_refresh(true, true, false, false)
                .is_none(),
            "slow poll discovers nothing while no failure exists"
        );
        let token = state
            .begin_failed_refresh(true, true, false, true)
            .expect("forced failed refresh");
        assert!(state.failed_refresh_in_flight());
        assert!(
            state
                .begin_queue_refresh(true, true, false, false, true)
                .is_some(),
            "the queued fence stays independent"
        );
        state
            .apply_failed_listing(&token, failed_listing(&thread_id, vec![failed_summary(&thread_id, "one")]))
            .expect("failed page");
        assert!(!state.failed_refresh_in_flight());
        assert_eq!(state.failed_total_count(), 1);
        let [entry] = state.failed_entries() else {
            panic!("exactly one failed row should be installed");
        };
        assert_eq!(entry.message_id(), &message("one"));
        assert_eq!(entry.thread_id(), &thread_id);
        assert_eq!(entry.card_text(), "hello");
        assert!(!entry.has_attachments());
        assert_eq!(
            entry.reason(),
            "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue"
        );
        assert_eq!(entry.identity().generation(), 6);
        let token = state
            .begin_failed_refresh(true, true, false, false)
            .expect("slow poll continues while failures persist");
        assert!(state.finish_failed_refresh(&token));
    }

    #[test]
    fn failed_listing_rejects_stale_wrong_thread_and_duplicate_rows() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 6);
        let token = state
            .begin_failed_refresh(true, true, false, true)
            .expect("failed refresh");
        state.set_scope(Some(thread_id.clone()), 7);
        assert_eq!(
            state.apply_failed_listing(&token, failed_listing(&thread_id, Vec::new())),
            Err(FailedListingRejection::StaleRefresh)
        );

        let other = thread("thread-b");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(other.clone()), 6);
        let token = state
            .begin_failed_refresh(true, true, false, true)
            .expect("failed refresh");
        assert_eq!(
            state.apply_failed_listing(&token, failed_listing(&thread_id, Vec::new())),
            Err(FailedListingRejection::WrongThread)
        );

        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 6);
        let token = state
            .begin_failed_refresh(true, true, false, true)
            .expect("failed refresh");
        assert_eq!(
            state.apply_failed_listing(
                &token,
                failed_listing(&thread_id, vec![
                    failed_summary(&thread_id, "one"),
                    failed_summary(&thread_id, "one"),
                ])
            ),
            Err(FailedListingRejection::DuplicateCommandId)
        );
    }

    #[test]
    fn failed_scope_change_clears_failed_rows_and_fence() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 6);
        let token = state
            .begin_failed_refresh(true, true, false, true)
            .expect("failed refresh");
        state
            .apply_failed_listing(&token, failed_listing(&thread_id, vec![failed_summary(&thread_id, "one")]))
            .expect("failed page");
        assert_eq!(state.failed_entries().len(), 1);
        state.set_scope(Some(thread("thread-b")), 7);
        assert!(state.failed_entries().is_empty());
        assert_eq!(state.failed_total_count(), 0);
        assert!(!state.failed_refresh_in_flight());
    }
}
