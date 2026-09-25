//! Bounded native composer state for messages that have not been claimed.
//!
//! A queued-message listing is deliberately a projection, not a second copy
//! of the accepted message payload. It carries the original queue request and
//! message identities, authored-text presence, ordered byte-free image
//! references, and the acceptance time. The separate withdrawal result is a
//! durable command acknowledgement; it never embeds authored text or image
//! bytes. An editor that successfully recalls a message asks the ownership-
//! checked database read seam for the original [`crate::QueueMessagePayload`].

#![allow(
    clippy::module_name_repetitions,
    reason = "public queued-message values retain their packet context at crate boundaries"
)]

use std::collections::HashSet;

use thiserror::Error;

use crate::{
    AuthoredText, CommandReceipt, EngineId, ImageAttachmentRef, MessageId, RequestId, ThreadId,
    UnixMillis,
};

/// Maximum number of queued-message summaries returned by one read.
///
/// The bound applies to either direction. `total_count` remains the exact
/// number of currently eligible rows, while `has_more` says whether the
/// finite page omitted any of them.
pub const QUEUED_MESSAGE_LIST_MAX: usize = 32;

/// Stable direction for a bounded queued-message page.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QueuedMessageListOrder {
    /// Oldest accepted messages first.
    OldestFirst,
    /// Newest accepted messages first.
    LatestFirst,
}

/// Read query for exactly one existing thread's still-unclaimed messages.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListQueuedMessages {
    /// Authenticated thread to read.
    pub thread_id: ThreadId,
    /// Stable page direction.
    pub order: QueuedMessageListOrder,
    /// Requested page size, in the inclusive range `1..=32`.
    pub limit: usize,
}

impl ListQueuedMessages {
    /// Creates a bounded queued-message query.
    ///
    /// # Errors
    ///
    /// Returns [`QueuedMessageListError`] when the requested limit is zero or
    /// exceeds [`QUEUED_MESSAGE_LIST_MAX`].
    pub fn new(
        thread_id: ThreadId,
        order: QueuedMessageListOrder,
        limit: usize,
    ) -> Result<Self, QueuedMessageListError> {
        validate_limit(limit)?;
        Ok(Self {
            thread_id,
            order,
            limit,
        })
    }
}

/// A queued-message page was requested outside its finite bound.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum QueuedMessageListError {
    /// A page must contain at least one row.
    #[error("queued-message list limit must be at least one")]
    Empty,
    /// A page cannot exceed the native queued-message bound.
    #[error("queued-message list limit is {limit}; the maximum is {maximum}")]
    TooLarge {
        /// Requested row count.
        limit: usize,
        /// Native maximum row count.
        maximum: usize,
    },
}

/// One byte-free projection of an eligible queued message.
///
/// `text: Some("")` is intentionally distinct from `text: None`, including
/// for image-only messages. Attachment order is the authored order and each
/// reference contains metadata and digest only.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QueuedMessageSummary {
    /// Forge-minted immutable message identity.
    pub message_id: MessageId,
    /// Authenticated thread owning the message.
    pub thread_id: ThreadId,
    /// Client request identity that originally queued the message.
    pub original_request_id: RequestId,
    /// Optional authored text with exact presence preserved.
    pub text: Option<AuthoredText>,
    /// Ordered byte-free image metadata.
    pub attachments: Vec<ImageAttachmentRef>,
    /// Original queue acceptance instant.
    pub accepted_at: UnixMillis,
    /// Latest dispatcher diagnostic persisted with the dispatch, if the
    /// dispatcher has claimed and requeued this message at least once. A
    /// never-attempted row carries no diagnostic. While the row is
    /// [`QueuedMessageState::Queued`] this is the reason the Forge is
    /// holding it (for example an engine that is not ready yet).
    pub last_error: Option<DispatchError>,
    /// Forge-owned delivery state of the row.
    pub state: QueuedMessageState,
    /// Engine the accepted configuration snapshot routes this message to,
    /// when the acceptance captured one.
    pub engine: Option<EngineId>,
}

/// Forge-owned delivery state of one accepted message that has not reached
/// the transcript yet.
///
/// A message leaves the queued listing in the same commit that projects it
/// into the transcript, or when it fails terminally (then it is listed as a
/// [`FailedMessageSummary`]) or is withdrawn.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QueuedMessageState {
    /// Accepted and waiting for the dispatcher, possibly held with the
    /// reason in [`QueuedMessageSummary::last_error`].
    Queued,
    /// Claimed by the dispatcher; its run is starting.
    Dispatching,
}

/// Bounded dispatcher diagnostic persisted with one queued dispatch.
///
/// The value carries only the operator-facing reason the dispatcher stored
/// verbatim on its last requeue (for example `engine unconfigured`). Its
/// contents stay out of error and log rendering; [`Debug`] reports only the
/// byte length and callers must opt into [`Self::as_str`].
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct DispatchError(String);

impl DispatchError {
    /// Maximum UTF-8 byte length accepted for one dispatch diagnostic.
    ///
    /// This matches the dispatcher failure-reason ceiling so a persisted
    /// `last_error` always round-trips through this type.
    pub const MAX_BYTES: usize = 4096;

    /// Creates the diagnostic after validating the persisted text without
    /// any truncation.
    ///
    /// # Errors
    ///
    /// Returns [`DispatchErrorParseError::Empty`] for empty text or
    /// [`DispatchErrorParseError::TooLong`] carrying only the offending
    /// length when the text exceeds `MAX_BYTES` UTF-8 bytes.
    pub fn parse(value: String) -> Result<Self, DispatchErrorParseError> {
        if value.is_empty() {
            return Err(DispatchErrorParseError::Empty);
        }
        let length = value.len();
        if length > Self::MAX_BYTES {
            return Err(DispatchErrorParseError::TooLong {
                length,
                maximum: Self::MAX_BYTES,
            });
        }
        Ok(Self(value))
    }

    /// Returns the validated diagnostic exactly as persisted.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for DispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DispatchError")
            .field("length_bytes", &self.0.len())
            .finish()
    }
}

/// Failure to accept one dispatch diagnostic.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum DispatchErrorParseError {
    /// The supplied diagnostic was empty.
    #[error("dispatch diagnostic must not be empty")]
    Empty,

    /// The supplied diagnostic exceeded its UTF-8 byte ceiling.
    #[error("dispatch diagnostic is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// The documented ceiling ([`DispatchError::MAX_BYTES`]).
        maximum: usize,
    },
}

/// A bounded, truthfully countable queued-message page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedMessageListing {
    thread_id: ThreadId,
    order: QueuedMessageListOrder,
    limit: usize,
    messages: Vec<QueuedMessageSummary>,
    total_count: u64,
    has_more: bool,
}

impl QueuedMessageListing {
    /// Builds a page after checking its finite and ownership invariants.
    ///
    /// The repository supplies `total_count` from the same eligibility
    /// predicate as the page query. `has_more` is derived here instead of
    /// trusting a separately supplied flag.
    ///
    /// # Errors
    ///
    /// Returns a listing error when the page exceeds its limit, its count is
    /// too small, or one row violates thread or identity uniqueness.
    pub fn new(
        thread_id: ThreadId,
        order: QueuedMessageListOrder,
        limit: usize,
        total_count: u64,
        messages: Vec<QueuedMessageSummary>,
    ) -> Result<Self, QueuedMessageListingError> {
        validate_limit(limit).map_err(QueuedMessageListingError::InvalidLimit)?;
        if messages.len() > limit {
            return Err(QueuedMessageListingError::TooManyMessages {
                count: messages.len(),
                maximum: limit,
            });
        }
        let message_count =
            u64::try_from(messages.len()).map_err(|_| QueuedMessageListingError::Invariant {
                reason: "queued-message page length does not fit its count type",
            })?;
        if total_count < message_count {
            return Err(QueuedMessageListingError::CountBelowPage {
                total_count,
                page_count: message_count,
            });
        }

        let mut seen = HashSet::with_capacity(messages.len());
        for message in &messages {
            if message.thread_id != thread_id {
                return Err(QueuedMessageListingError::WrongThread {
                    message_id: message.message_id.clone(),
                });
            }
            if !seen.insert(&message.message_id) {
                return Err(QueuedMessageListingError::DuplicateMessageId {
                    message_id: message.message_id.clone(),
                });
            }
        }

        Ok(Self {
            thread_id,
            order,
            limit,
            messages,
            total_count,
            has_more: total_count > message_count,
        })
    }

    /// Returns the exact thread named by the query.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the stable page direction.
    #[must_use]
    pub const fn order(&self) -> QueuedMessageListOrder {
        self.order
    }

    /// Returns the requested page size.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Returns the byte-free rows in stable requested order.
    #[must_use]
    pub fn messages(&self) -> &[QueuedMessageSummary] {
        &self.messages
    }

    /// Returns the exact eligible-row count at read time.
    #[must_use]
    pub const fn total_count(&self) -> u64 {
        self.total_count
    }

    /// Whether another bounded page is needed to see every eligible row.
    #[must_use]
    pub const fn has_more(&self) -> bool {
        self.has_more
    }
}

/// Failure while validating or constructing a queued-message page.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum QueuedMessageListingError {
    /// The query limit was outside its finite bound.
    #[error("invalid queued-message list limit: {0}")]
    InvalidLimit(QueuedMessageListError),
    /// The repository returned more rows than the requested page size.
    #[error("queued-message page contains {count} rows; the maximum is {maximum}")]
    TooManyMessages {
        /// Returned row count.
        count: usize,
        /// Requested page size.
        maximum: usize,
    },
    /// The count query and page query disagree.
    #[error("queued-message total count {total_count} is below page count {page_count}")]
    CountBelowPage {
        /// Count reported by SQLite.
        total_count: u64,
        /// Number of rows returned in the page.
        page_count: u64,
    },
    /// A page row belongs to another thread.
    #[error("queued-message `{message_id}` belongs to another thread")]
    WrongThread {
        /// Unexpected message identity.
        message_id: MessageId,
    },
    /// A page repeated one message identity.
    #[error("queued-message page repeats message `{message_id}`")]
    DuplicateMessageId {
        /// Repeated message identity.
        message_id: MessageId,
    },
    /// A local page invariant failed.
    #[error("queued-message page invariant failed: {reason}")]
    Invariant {
        /// Stable invariant description.
        reason: &'static str,
    },
}

/// Exact authenticated input for one discard/edit withdrawal command.
///
/// `original_request_id` and `message_id` are both required so a caller
/// cannot withdraw a message by a loosely related identity. The new
/// `withdrawal_request_id` is the idempotency key for this command itself.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WithdrawQueuedMessage {
    /// Existing authenticated thread containing the original message.
    pub thread_id: ThreadId,
    /// Original Forge-minted message identity.
    pub message_id: MessageId,
    /// Request identity that originally accepted the message.
    pub original_request_id: RequestId,
    /// Client request identity for this withdrawal command.
    pub withdrawal_request_id: RequestId,
    /// Authoritative withdrawal acceptance instant.
    pub accepted_at: UnixMillis,
}

impl WithdrawQueuedMessage {
    /// Creates an exact withdrawal command input.
    #[must_use]
    pub const fn new(
        thread_id: ThreadId,
        message_id: MessageId,
        original_request_id: RequestId,
        withdrawal_request_id: RequestId,
        accepted_at: UnixMillis,
    ) -> Self {
        Self {
            thread_id,
            message_id,
            original_request_id,
            withdrawal_request_id,
            accepted_at,
        }
    }
}

/// Result of the withdrawal fence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QueuedMessageWithdrawalOutcome {
    /// The still-queued, never-claimed dispatch was durably withdrawn.
    Withdrawn,
    /// Dispatch had already been claimed, started, or otherwise settled.
    TooLate,
    /// No eligible queued dispatch exists for the exact supplied target.
    NotQueued,
}

/// Payload-free receipt and outcome of one withdrawal request.
///
/// `receipt.disposition == Duplicate` means the exact command was replayed;
/// the outcome and acceptance time still describe the original command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawQueuedMessageResult {
    /// Idempotency receipt for the withdrawal request.
    pub receipt: CommandReceipt,
    /// Authenticated thread named by the original command.
    pub thread_id: ThreadId,
    /// Original message identity.
    pub message_id: MessageId,
    /// Original queue request identity.
    pub original_request_id: RequestId,
    /// Acceptance instant persisted in the withdrawal receipt.
    pub accepted_at: UnixMillis,
    /// Durable fence outcome.
    pub outcome: QueuedMessageWithdrawalOutcome,
}

impl WithdrawQueuedMessageResult {
    /// Returns the withdrawal request identity answered by this result.
    #[must_use]
    pub const fn withdrawal_request_id(&self) -> &RequestId {
        &self.receipt.request_id
    }

    /// Returns whether this call newly recorded or replayed its result.
    #[must_use]
    pub const fn disposition(&self) -> crate::ReceiptDisposition {
        self.receipt.disposition
    }
}

/// Validates a queued-message page limit at the domain boundary.
fn validate_limit(limit: usize) -> Result<(), QueuedMessageListError> {
    if limit == 0 {
        return Err(QueuedMessageListError::Empty);
    }
    if limit > QUEUED_MESSAGE_LIST_MAX {
        return Err(QueuedMessageListError::TooLarge {
            limit,
            maximum: QUEUED_MESSAGE_LIST_MAX,
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Terminally failed dispatches
// ---------------------------------------------------------------------------

/// Maximum number of failed-dispatch summaries returned by one read.
///
/// Failed rows are terminal: the dispatcher will never claim them again, so
/// this bound only limits how many a surface renders, never live retry work.
pub const FAILED_MESSAGE_LIST_MAX: usize = 32;

/// Read query for exactly one existing thread's terminally failed dispatches.
///
/// Unlike the queued listing, no direction is offered: newest failures first
/// is the only stable order a surface needs. Withdrawn rows are excluded: a
/// withdrawal is an explicit user dismissal recorded in
/// `queued_message_withdrawals`, not a failure to surface.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListFailedMessages {
    /// Authenticated thread to read.
    pub thread_id: ThreadId,
    /// Requested page size, in the inclusive range `1..=32`.
    pub limit: usize,
}

impl ListFailedMessages {
    /// Creates a bounded failed-dispatch query.
    ///
    /// # Errors
    ///
    /// Returns [`FailedMessageListError`] when the requested limit is zero or
    /// exceeds [`FAILED_MESSAGE_LIST_MAX`].
    pub fn new(thread_id: ThreadId, limit: usize) -> Result<Self, FailedMessageListError> {
        if limit == 0 {
            return Err(FailedMessageListError::Empty);
        }
        if limit > FAILED_MESSAGE_LIST_MAX {
            return Err(FailedMessageListError::TooLarge {
                limit,
                maximum: FAILED_MESSAGE_LIST_MAX,
            });
        }
        Ok(Self { thread_id, limit })
    }
}

/// A failed-dispatch page was requested outside its finite bound.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum FailedMessageListError {
    /// A page must contain at least one row.
    #[error("failed-message list limit must be at least one")]
    Empty,
    /// A page cannot exceed the native failed-message bound.
    #[error("failed-message list limit is {limit}; the maximum is {maximum}")]
    TooLarge {
        /// Requested row count.
        limit: usize,
        /// Native maximum row count.
        maximum: usize,
    },
}

/// One byte-free projection of a terminally failed dispatch.
///
/// `reason` is always present: a terminal dispatcher failure persists its
/// diagnostic verbatim. Text and attachment references mirror the queued
/// summary so the existing new-chat recovery action can restore the prompt
/// as an unsent draft without a second read.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FailedMessageSummary {
    /// Forge-minted immutable message identity.
    pub message_id: MessageId,
    /// Authenticated thread owning the message.
    pub thread_id: ThreadId,
    /// Client request identity that originally queued the message.
    pub original_request_id: RequestId,
    /// Optional authored text with exact presence preserved.
    pub text: Option<AuthoredText>,
    /// Ordered byte-free image metadata.
    pub attachments: Vec<ImageAttachmentRef>,
    /// Original queue acceptance instant.
    pub accepted_at: UnixMillis,
    /// Terminal failure instant.
    pub failed_at: UnixMillis,
    /// Dispatcher diagnostic persisted with the terminal failure.
    pub reason: DispatchError,
    /// Whether the Forge can re-dispatch the stored payload on request
    /// (`RetryFailedMessage`): the message never reached the transcript.
    pub retryable: bool,
}

/// A bounded, truthfully countable failed-dispatch page.
///
/// Rows arrive newest failures first. `total_count` is the exact number of
/// eligible failed rows and `has_more` says whether the finite page omitted
/// any of them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailedMessageListing {
    thread_id: ThreadId,
    limit: usize,
    messages: Vec<FailedMessageSummary>,
    total_count: u64,
    has_more: bool,
}

impl FailedMessageListing {
    /// Builds a page after checking its finite and ownership invariants.
    ///
    /// The repository supplies `total_count` from the same eligibility
    /// predicate as the page query. `has_more` is derived here instead of
    /// trusting a separately supplied flag.
    ///
    /// # Errors
    ///
    /// Returns a listing error when the page exceeds its limit, its count is
    /// too small, or one row violates thread or identity uniqueness.
    pub fn new(
        thread_id: ThreadId,
        limit: usize,
        total_count: u64,
        messages: Vec<FailedMessageSummary>,
    ) -> Result<Self, FailedMessageListingError> {
        if limit == 0 || limit > FAILED_MESSAGE_LIST_MAX {
            return Err(FailedMessageListingError::InvalidLimit {
                limit,
                maximum: FAILED_MESSAGE_LIST_MAX,
            });
        }
        if messages.len() > limit {
            return Err(FailedMessageListingError::TooManyMessages {
                count: messages.len(),
                maximum: limit,
            });
        }
        let message_count =
            u64::try_from(messages.len()).map_err(|_| FailedMessageListingError::Invariant {
                reason: "failed-message page length does not fit its count type",
            })?;
        if total_count < message_count {
            return Err(FailedMessageListingError::CountBelowPage {
                total_count,
                page_count: message_count,
            });
        }

        let mut seen = HashSet::with_capacity(messages.len());
        for message in &messages {
            if message.thread_id != thread_id {
                return Err(FailedMessageListingError::WrongThread {
                    message_id: message.message_id.clone(),
                });
            }
            if !seen.insert(&message.message_id) {
                return Err(FailedMessageListingError::DuplicateMessageId {
                    message_id: message.message_id.clone(),
                });
            }
        }

        Ok(Self {
            thread_id,
            limit,
            messages,
            total_count,
            has_more: total_count > message_count,
        })
    }

    /// Returns the exact thread named by the query.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the requested page size.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Returns the byte-free rows, newest failures first.
    #[must_use]
    pub fn messages(&self) -> &[FailedMessageSummary] {
        &self.messages
    }

    /// Returns the exact eligible-row count at read time.
    #[must_use]
    pub const fn total_count(&self) -> u64 {
        self.total_count
    }

    /// Whether another bounded page is needed to see every eligible row.
    #[must_use]
    pub const fn has_more(&self) -> bool {
        self.has_more
    }
}

/// Failure while validating or constructing a failed-dispatch page.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum FailedMessageListingError {
    /// The query limit was outside its finite bound.
    #[error("invalid failed-message list limit {limit}; the maximum is {maximum}")]
    InvalidLimit {
        /// Requested row count.
        limit: usize,
        /// Native maximum row count.
        maximum: usize,
    },
    /// The repository returned more rows than the requested page size.
    #[error("failed-message page contains {count} rows; the maximum is {maximum}")]
    TooManyMessages {
        /// Returned row count.
        count: usize,
        /// Requested page size.
        maximum: usize,
    },
    /// The count query and page query disagree.
    #[error("failed-message total count {total_count} is below page count {page_count}")]
    CountBelowPage {
        /// Count reported by SQLite.
        total_count: u64,
        /// Number of rows returned in the page.
        page_count: u64,
    },
    /// A page row belongs to another thread.
    #[error("failed-message `{message_id}` belongs to another thread")]
    WrongThread {
        /// Unexpected message identity.
        message_id: MessageId,
    },
    /// A page repeated one message identity.
    #[error("failed-message page repeats message `{message_id}`")]
    DuplicateMessageId {
        /// Repeated message identity.
        message_id: MessageId,
    },
    /// A local page invariant failed.
    #[error("failed-message page invariant failed: {reason}")]
    Invariant {
        /// Stable invariant description.
        reason: &'static str,
    },
}
