//! Outbox and usage state vocabulary: identities, rows, receipts, results,
//! and statuses.

#[allow(clippy::wildcard_imports)]
use super::*;

/// The largest page of queued or failed rows the native surface retains.
pub(crate) const COMPOSER_QUEUE_PAGE_LIMIT: usize = artisan_domain::QUEUED_MESSAGE_LIST_MAX;

/// Exact row identity used between the outbox projection and controls.
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

/// Checks that a row's byte-free image references stay within the message
/// bounds and name exactly their own message in authored order.
fn attachments_in_bounds(
    attachments: &[ImageAttachmentRef],
    message_id: &MessageId,
    thread_id: &ThreadId,
) -> bool {
    if attachments.len() > artisan_domain::MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
        return false;
    }
    let total_bytes = attachments.iter().try_fold(0usize, |total, image| {
        total.checked_add(usize::try_from(image.size_bytes).ok()?)
    });
    if total_bytes
        .is_none_or(|total| total > artisan_domain::MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES)
    {
        return false;
    }
    attachments.iter().enumerate().all(|(index, image)| {
        &image.message_id == message_id
            && &image.thread_id == thread_id
            && usize::try_from(image.index).ok() == Some(index)
    })
}

/// One Forge row of an accepted message that has not reached the
/// transcript: what the transcript tail renders and what a withdrawal names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComposerQueueEntry {
    pub(super) identity: ComposerQueueIdentity,
    pub(super) thread_id: ThreadId,
    pub(super) message_id: MessageId,
    pub(super) original_request_id: RequestId,
    text: Option<AuthoredText>,
    attachments: Vec<ImageAttachmentRef>,
    last_error: Option<artisan_domain::DispatchError>,
    state: QueuedMessageState,
    engine: Option<EngineId>,
}

impl ComposerQueueEntry {
    pub(crate) fn attachments(&self) -> &[ImageAttachmentRef] {
        &self.attachments
    }

    pub(super) fn from_summary(
        summary: &artisan_domain::QueuedMessageSummary,
        generation: u64,
    ) -> Option<Self> {
        attachments_in_bounds(
            &summary.attachments,
            &summary.message_id,
            &summary.thread_id,
        )
        .then(|| Self {
            identity: ComposerQueueIdentity::new(summary.original_request_id.as_str(), generation),
            thread_id: summary.thread_id.clone(),
            message_id: summary.message_id.clone(),
            original_request_id: summary.original_request_id.clone(),
            text: summary.text.clone(),
            attachments: summary.attachments.clone(),
            last_error: summary.last_error.clone(),
            state: summary.state,
            engine: summary.engine,
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

    /// Returns the Forge-owned delivery state.
    #[must_use]
    pub(crate) const fn state(&self) -> QueuedMessageState {
        self.state
    }

    /// Returns the engine the accepted configuration routes this message to.
    #[must_use]
    pub(crate) const fn engine(&self) -> Option<EngineId> {
        self.engine
    }

    /// Returns why the Forge is holding this row, when its dispatcher
    /// requeued it at least once.
    #[must_use]
    pub(crate) fn dispatch_error(&self) -> Option<&str> {
        self.last_error
            .as_ref()
            .map(artisan_domain::DispatchError::as_str)
    }

    /// Returns the exact authored text; absent or empty text stays empty and
    /// the renderer supplies its image-only label.
    #[must_use]
    pub(crate) fn lip_text(&self) -> &str {
        self.text.as_ref().map_or("", AuthoredText::as_str)
    }
}

/// One Forge failure row: what the failure card renders and what a retry or
/// new-chat recovery names.
///
/// The identity binds the action to the exact failed message and the
/// generation that projected it. The reason is the verbatim dispatcher
/// diagnostic; `retryable` is the Forge's verdict on whether its stored
/// payload can be dispatched again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FailedQueueEntry {
    pub(super) identity: ComposerQueueIdentity,
    pub(super) thread_id: ThreadId,
    pub(super) message_id: MessageId,
    pub(super) original_request_id: RequestId,
    text: Option<AuthoredText>,
    attachments: Vec<ImageAttachmentRef>,
    reason: DispatchError,
    retryable: bool,
}

impl FailedQueueEntry {
    pub(super) fn from_summary(summary: &FailedMessageSummary, generation: u64) -> Option<Self> {
        attachments_in_bounds(
            &summary.attachments,
            &summary.message_id,
            &summary.thread_id,
        )
        .then(|| Self {
            identity: ComposerQueueIdentity::new(summary.original_request_id.as_str(), generation),
            thread_id: summary.thread_id.clone(),
            message_id: summary.message_id.clone(),
            original_request_id: summary.original_request_id.clone(),
            text: summary.text.clone(),
            attachments: summary.attachments.clone(),
            reason: summary.reason.clone(),
            retryable: summary.retryable,
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

    /// Names this failure for a Forge retry or recovery command.
    #[must_use]
    pub(crate) fn target(&self) -> FailedMessageTarget {
        FailedMessageTarget {
            thread_id: self.thread_id.clone(),
            message_id: self.message_id.clone(),
            original_request_id: self.original_request_id.clone(),
        }
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

    /// Whether the Forge can re-dispatch the stored payload.
    #[must_use]
    pub(crate) const fn retryable(&self) -> bool {
        self.retryable
    }

    /// Returns the exact text sent to the failed-card renderer. Image-only
    /// failures render their attachment label from
    /// [`Self::has_attachments`] instead of a placeholder.
    #[must_use]
    pub(crate) fn card_text(&self) -> &str {
        self.text.as_ref().map_or("", AuthoredText::as_str)
    }
}

/// Why a pushed outbox could not be installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutboxRejection {
    /// The outbox names another thread than the mounted scope.
    WrongThread,
    /// A listing exceeds the native page bound.
    TooManyRows,
    /// Two rows of one listing reuse an original queue command id, making a
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

/// Truthful status of the latest queue operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueStatus {
    /// No queue operation is waiting for a result.
    Idle,
    /// Forge is fencing an edit or discard request.
    WithdrawalPending,
    /// A discard was withdrawn; the next outbox omits the row.
    Withdrawn,
    /// An edit was withdrawn into the thread's Forge draft.
    RecalledToDraft,
    /// Forge reported that dispatch already passed the withdrawal fence.
    TooLate,
    /// Forge reported that the exact message is no longer queued.
    NotQueued,
    /// A transport error left an exact command available for retry.
    TransportFailed,
}

/// Failure to begin an edit or discard intent from a stale controls event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueueIntentError {
    /// No real thread is currently mounted.
    NoThread,
    /// The controls identity was not in the current outbox.
    UnknownRow,
    /// The row belongs to another thread or generation, or is dispatching.
    StaleRow,
    /// Another exact withdrawal owns the surface.
    Busy,
}

/// Whether a withdrawal receipt was newly acted upon or already consumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WithdrawalReceiptDisposition {
    /// Edit withdrawal committed; the Forge draft now holds the prompt.
    Recalled,
    /// Discard withdrawal committed.
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
    /// Forge-decided context size at which the reporting engine compacts.
    pub(crate) compaction_at_tokens: Option<u64>,
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
