//! Queue and usage state vocabulary: identities, rows, receipts, results, and
//! statuses.
//!
//! Extracted verbatim from `composer_queue_state.rs` during the module split.

#[allow(clippy::wildcard_imports)]
use super::*;

/// The largest page the native queue surface will request or retain.
pub(crate) const COMPOSER_QUEUE_PAGE_LIMIT: usize = artisan_domain::QUEUED_MESSAGE_LIST_MAX;

/// The refresh cadence used by the parent while a queue or run is live.
pub(crate) const COMPOSER_QUEUE_REFRESH_INTERVAL_MS: u64 = 5000;

/// Maximum staged echo watches. Receipts end the message flight, so several
/// sends can await their echo at once; the bound keeps a pathological burst
/// from growing the set without a scope change. Eviction drops the oldest
/// watch, whose send falls back to the generic narration and the
/// listing-driven lip.
pub(crate) const ECHO_WATCH_LIMIT: usize = 8;

/// Exact row identity used between the queue projection and controls.
///
/// `command_id` is the original queue request id, not the newer withdrawal
/// request id. The generation prevents a delayed control event from acting on
/// a visually reused command id after a thread/scope transition.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ComposerQueueIdentity {
    pub(super) command_id: String,
    pub(super) generation: u64,
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
    pub(super) identity: ComposerQueueIdentity,
    pub(super) thread_id: ThreadId,
    pub(super) message_id: MessageId,
    pub(super) original_request_id: RequestId,
    text: Option<AuthoredText>,
    attachments: Vec<ImageAttachmentRef>,
    accepted_at: UnixMillis,
    last_error: Option<artisan_domain::DispatchError>,
}

impl ComposerQueueEntry {
    pub(crate) fn attachments(&self) -> &[ImageAttachmentRef] {
        &self.attachments
    }

    pub(super) fn from_summary(
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

    /// Returns the Forge-minted message identity.
    #[must_use]
    pub(crate) fn message_id(&self) -> &MessageId {
        &self.message_id
    }

    /// Returns the latest dispatcher diagnostic for this row, if the
    /// dispatcher has claimed and requeued it at least once.
    #[must_use]
    pub(crate) fn dispatch_error(&self) -> Option<&str> {
        self.last_error
            .as_ref()
            .map(artisan_domain::DispatchError::as_str)
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
    pub(super) identity: ComposerQueueIdentity,
    pub(super) thread_id: ThreadId,
    pub(super) message_id: MessageId,
    pub(super) original_request_id: RequestId,
    text: Option<AuthoredText>,
    attachments: Vec<ImageAttachmentRef>,
    accepted_at: UnixMillis,
    failed_at: UnixMillis,
    reason: DispatchError,
}

impl FailedQueueEntry {
    pub(crate) fn accepted_at(&self) -> UnixMillis {
        self.accepted_at
    }

    pub(super) fn from_summary(summary: &FailedMessageSummary, generation: u64) -> Option<Self> {
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

    /// Returns whether the failed prompt carries image attachments.
    #[must_use]
    pub(crate) fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
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
    pub(super) fn new(
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
            .finish_non_exhaustive()
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
    pub(super) thread_id: ThreadId,
    pub(super) generation: u64,
    pub(super) serial: u64,
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
}

/// Generation/run/sequence fence for one exact usage read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageReadToken {
    pub(super) thread_id: ThreadId,
    pub(super) run_id: RunId,
    pub(super) generation: u64,
    pub(super) sequence: u64,
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

/// One accepted send awaiting its transcript echo.
///
/// Staged at receipt (the Forge message id is known only then) and retired
/// exactly once when the canonical user item projects, or dropped on
/// failure/scope change. Carries the send-time routed engine label for the
/// projection lane's Waiting narration; never re-resolved from the picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EchoWatch {
    /// Forge-minted accepted message identity to match against echo items.
    pub(super) message_id: MessageId,
    /// Named steer target run, if the send named one. Exact-correlation
    /// proof: only an observed run equal to this id may override the
    /// captured label.
    pub(super) steer_run_id: Option<RunId>,
    /// Typed engine and display label captured at send, if any. `None`
    /// renders the generic fallback, never "Waiting for Other".
    pub(super) engine_label: Option<TurnEngineLabel>,
}

impl EchoWatch {
    /// Returns the watched Forge message identity.
    #[must_use]
    pub(crate) const fn message_id(&self) -> &MessageId {
        &self.message_id
    }

    /// Returns the named steer target run, if the send named one.
    #[must_use]
    pub(crate) const fn steer_run_id(&self) -> Option<&RunId> {
        self.steer_run_id.as_ref()
    }

    /// Returns the send-time engine label, if one was captured.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn engine_label(&self) -> Option<&str> {
        self.engine_label.as_deref()
    }

    /// Returns the send-time typed engine metadata, if captured.
    #[must_use]
    pub(crate) const fn turn_engine_label(&self) -> Option<&TurnEngineLabel> {
        self.engine_label.as_ref()
    }
}
