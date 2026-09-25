//! Domain seams for bounded queued-message recall and run-usage reads.
//!
//! The existing queued-message and usage modules own the durable values. This
//! module owns only the request/result vocabulary that connects those values
//! to the protocol. In particular, the wire withdrawal command carries no
//! acceptance timestamp: the database command remains the only owner of that
//! Forge-minted fact.

use thiserror::Error;

use crate::{MessageId, QueueMessagePayload, RequestId, RunId, RunUsageReport, ThreadId};

/// Maximum image count accepted by the composer-state wire payload.
///
/// This intentionally aliases the general composer/domain bound. The wire
/// leaf must not narrow valid nine- or ten-image messages.
pub const COMPOSER_STATE_IMAGE_MAX_COUNT: usize = crate::MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT;

/// Maximum encoded size accepted for one composer-state image.
pub const COMPOSER_STATE_IMAGE_MAX_BYTES: usize = crate::MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES;

/// Maximum aggregate encoded image size accepted by one composer-state
/// payload.
pub const COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES: usize =
    crate::MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES;

/// Client-facing withdrawal command for one exact queued message.
///
/// `accepted_at` is deliberately absent. The database command adds its
/// authoritative clock value when it fences the queued row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WithdrawQueuedMessageCommand {
    /// Client request identity for this withdrawal operation.
    pub request_id: RequestId,
    /// Existing thread containing the queued message.
    pub thread_id: ThreadId,
    /// Exact Forge-minted queued message identity.
    pub message_id: MessageId,
    /// Request identity that originally accepted the queued message.
    pub original_request_id: RequestId,
    /// Whether a successful withdrawal moves the payload into the thread's
    /// Forge composer draft (the edit flow) instead of discarding it.
    pub recall_to_draft: bool,
}

impl WithdrawQueuedMessageCommand {
    /// Constructs an exact discard withdrawal without a client-supplied
    /// clock.
    #[must_use]
    pub const fn new(
        request_id: RequestId,
        thread_id: ThreadId,
        message_id: MessageId,
        original_request_id: RequestId,
    ) -> Self {
        Self {
            request_id,
            thread_id,
            message_id,
            original_request_id,
            recall_to_draft: false,
        }
    }

    /// Makes this withdrawal move the payload into the thread's composer
    /// draft once the Forge withdraws it.
    #[must_use]
    pub const fn recalling_to_draft(mut self) -> Self {
        self.recall_to_draft = true;
        self
    }

    /// Returns the withdrawal request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns the exact owning thread.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the exact queued message identity.
    #[must_use]
    pub const fn message_id(&self) -> &MessageId {
        &self.message_id
    }

    /// Returns the original queue request identity.
    #[must_use]
    pub const fn original_request_id(&self) -> &RequestId {
        &self.original_request_id
    }
}

/// Reads the durable payload of one exact withdrawn or terminally failed message.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadRecalledMessage {
    /// Authenticated owning thread.
    pub thread_id: ThreadId,
    /// Exact withdrawn message identity.
    pub message_id: MessageId,
    /// Request identity that originally queued the message.
    pub original_request_id: RequestId,
}

impl ReadRecalledMessage {
    /// Constructs an exact recalled-message read.
    #[must_use]
    pub const fn new(
        thread_id: ThreadId,
        message_id: MessageId,
        original_request_id: RequestId,
    ) -> Self {
        Self {
            thread_id,
            message_id,
            original_request_id,
        }
    }

    /// Returns the authenticated owning thread.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the exact recalled message identity.
    #[must_use]
    pub const fn message_id(&self) -> &MessageId {
        &self.message_id
    }

    /// Returns the original queue request identity.
    #[must_use]
    pub const fn original_request_id(&self) -> &RequestId {
        &self.original_request_id
    }
}

/// Reads usage persisted for one exact durable run in one exact thread.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadRunUsage {
    /// Authenticated owning thread.
    pub thread_id: ThreadId,
    /// Exact durable run identity.
    pub run_id: RunId,
}

impl ReadRunUsage {
    /// Constructs an exact run-usage read.
    #[must_use]
    pub const fn new(thread_id: ThreadId, run_id: RunId) -> Self {
        Self { thread_id, run_id }
    }

    /// Returns the authenticated owning thread.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the exact durable run identity.
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }
}

/// Result of an exact recalled-message read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecalledMessageResult {
    /// Authenticated owning thread.
    pub thread_id: ThreadId,
    /// Exact recalled message identity.
    pub message_id: MessageId,
    /// Request identity that originally queued the message.
    pub original_request_id: RequestId,
    /// Durable payload, absent when no exact withdrawn or failed row is readable.
    pub payload: Option<QueueMessagePayload>,
}

impl RecalledMessageResult {
    /// Constructs a recalled-message result while retaining payload presence
    /// and authored text presence exactly.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerStateValueError`] if the payload exceeds the shared
    /// image count or aggregate byte bound.
    pub fn new(
        thread_id: ThreadId,
        message_id: MessageId,
        original_request_id: RequestId,
        payload: Option<QueueMessagePayload>,
    ) -> Result<Self, ComposerStateValueError> {
        if let Some(payload) = &payload {
            validate_payload_bounds(payload)?;
        }
        Ok(Self {
            thread_id,
            message_id,
            original_request_id,
            payload,
        })
    }
}

/// Result of an exact run-usage read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunUsageResult {
    /// Authenticated owning thread.
    pub thread_id: ThreadId,
    /// Exact durable run identity.
    pub run_id: RunId,
    /// Durable usage report, absent when no authoritative usage exists.
    pub report: Option<RunUsageReport>,
    /// Context size, in tokens, at which the reporting run's engine
    /// compacts, as the Forge decides it; `None` when no documented policy
    /// applies and the window is the only limit.
    pub compaction_at_tokens: Option<u64>,
}

/// Existing durable withdrawal result under the state-wire vocabulary.
///
/// The storage-facing type is retained as the single representation; this
/// alias gives the parent response union the name used by the wire contract
/// without introducing a second receipt type.
pub type QueuedMessageWithdrawalResult = crate::WithdrawQueuedMessageResult;

impl RunUsageResult {
    /// Constructs a result and rejects a report attributed to another scope.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerStateValueError`] when the report names another
    /// thread or run.
    pub fn new(
        thread_id: ThreadId,
        run_id: RunId,
        report: Option<RunUsageReport>,
    ) -> Result<Self, ComposerStateValueError> {
        if let Some(report) = &report {
            if report.thread_id() != &thread_id {
                return Err(ComposerStateValueError::ReportThreadMismatch);
            }
            if report.run_id() != &run_id {
                return Err(ComposerStateValueError::ReportRunMismatch);
            }
        }
        Ok(Self {
            thread_id,
            run_id,
            report,
            compaction_at_tokens: None,
        })
    }

    /// Returns this result carrying the Forge's compaction threshold.
    #[must_use]
    pub const fn with_compaction_at(mut self, compaction_at_tokens: Option<u64>) -> Self {
        self.compaction_at_tokens = compaction_at_tokens;
        self
    }
}

/// Validation failure for a composer-state value assembled at a boundary.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ComposerStateValueError {
    /// The payload contains more images than the shared general composer
    /// bound.
    #[error("composer-state payload contains too many images")]
    TooManyImages,
    /// The payload's aggregate encoded image bytes exceed the shared bound.
    #[error("composer-state payload image bytes exceed the shared bound")]
    ImageBytesTooLarge,
    /// A usage report names another thread than its result envelope.
    #[error("run-usage report thread does not match its result scope")]
    ReportThreadMismatch,
    /// A usage report names another run than its result envelope.
    #[error("run-usage report run does not match its result scope")]
    ReportRunMismatch,
}

/// Checks the shared payload count and aggregate image bounds.
///
/// `QueueMessagePayload` already enforces these bounds at construction. This
/// helper is intentionally repeatable for values crossing another owned
/// boundary and makes the wire contract explicit without copying bytes.
///
/// # Errors
///
/// Returns [`ComposerStateValueError`] when the image count or aggregate
/// encoded bytes exceed the shared composer bound.
pub fn validate_payload_bounds(
    payload: &QueueMessagePayload,
) -> Result<(), ComposerStateValueError> {
    if payload.attachments().len() > COMPOSER_STATE_IMAGE_MAX_COUNT {
        return Err(ComposerStateValueError::TooManyImages);
    }
    if payload.total_attachment_bytes() > COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES {
        return Err(ComposerStateValueError::ImageBytesTooLarge);
    }
    Ok(())
}
