//! Forge-owned message submissions after acceptance.
//!
//! Once the Forge accepts a message it owns the only copy. A thread's
//! [`MessageOutbox`] is what the Editor renders for messages that have not
//! reached the transcript: every queued or dispatching row with its delivery
//! state, plus the terminal failures still offered to the user. The Forge
//! pushes it over the thread's conversation subscription whenever it
//! changes. A failed message is retried or moved to a new thread by its
//! identity alone ([`RetryFailedMessage`], [`RecoverFailedMessage`]); the
//! Forge re-dispatches or moves the payload it stored, so no client keeps a
//! copy.

use thiserror::Error;

use crate::{
    FailedMessageListing, MessageId, QueuedMessageListing, ReceiptDisposition, RequestId, ThreadId,
};

/// Every undelivered message of one thread, as the Forge sees it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageOutbox {
    queued: QueuedMessageListing,
    failed: FailedMessageListing,
}

impl MessageOutbox {
    /// Pairs a thread's queued and failed listings.
    ///
    /// # Errors
    ///
    /// Returns [`MessageOutboxError`] when the listings name different
    /// threads.
    pub fn new(
        queued: QueuedMessageListing,
        failed: FailedMessageListing,
    ) -> Result<Self, MessageOutboxError> {
        if queued.thread_id() != failed.thread_id() {
            return Err(MessageOutboxError::ThreadMismatch);
        }
        Ok(Self { queued, failed })
    }

    /// The thread both listings belong to.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        self.queued.thread_id()
    }

    /// Accepted messages that are queued or dispatching, oldest first.
    #[must_use]
    pub const fn queued(&self) -> &QueuedMessageListing {
        &self.queued
    }

    /// Terminal failures still offered to the user, newest first.
    #[must_use]
    pub const fn failed(&self) -> &FailedMessageListing {
        &self.failed
    }
}

/// A message outbox could not be assembled.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum MessageOutboxError {
    /// The queued and failed listings belong to different threads.
    #[error("message outbox listings belong to different threads")]
    ThreadMismatch,
}

/// Names one terminally failed message by the identities the Forge minted
/// and the client's original queue request.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FailedMessageTarget {
    /// Thread that owns the failed message.
    pub thread_id: ThreadId,
    /// Forge-minted message identity.
    pub message_id: MessageId,
    /// Request identity that originally queued the message.
    pub original_request_id: RequestId,
}

/// Asks the Forge to dispatch one failed message again from its stored
/// payload.
///
/// `request_id` identifies this command. Retrying is idempotent by state: a
/// message that is no longer a retryable failure answers
/// [`FailedMessageRetryOutcome::NotRetryable`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RetryFailedMessage {
    /// Client request identity for this command.
    pub request_id: RequestId,
    /// The failed message.
    pub target: FailedMessageTarget,
}

/// What a retry did.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FailedMessageRetryOutcome {
    /// The stored payload is queued for dispatch again.
    Requeued,
    /// The message is not a retryable failure (already retried, withdrawn,
    /// recovered, or it reached the transcript before failing).
    NotRetryable,
}

/// Answer to [`RetryFailedMessage`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailedMessageRetried {
    /// The retry command's request identity.
    pub request_id: RequestId,
    /// The message the command named.
    pub target: FailedMessageTarget,
    /// What the Forge did.
    pub outcome: FailedMessageRetryOutcome,
}

/// Asks the Forge to move one failed message into a new thread of the same
/// project: it creates the thread with the failed message's engine
/// configuration, stores the payload as that thread's composer draft (never
/// sent automatically), and stops offering the failure on the old thread.
///
/// `request_id` identifies this command; a replay answers the same thread.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RecoverFailedMessage {
    /// Client request identity for this command.
    pub request_id: RequestId,
    /// The failed message.
    pub target: FailedMessageTarget,
}

/// Answer to [`RecoverFailedMessage`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailedMessageRecovered {
    /// The recovery command's request identity.
    pub request_id: RequestId,
    /// The message the command named.
    pub target: FailedMessageTarget,
    /// The new thread holding the prompt as its draft, or `None` when the
    /// message is not a recoverable failure.
    pub new_thread_id: Option<ThreadId>,
    /// Whether this call performed the recovery or replayed it.
    pub disposition: ReceiptDisposition,
}
