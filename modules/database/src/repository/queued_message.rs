//! Bounded queued-message reads and atomic edit/discard withdrawal.
//!
//! This module intentionally does not add a second message store. Immutable
//! message, image, and original queue-receipt rows remain the source of truth.
//! A withdrawal marks the exact never-claimed dispatch row `failed`, which is
//! already excluded by the existing claim SQL, and records the command result
//! in `queued_message_withdrawals`. The dispatch state update and receipt
//! insert happen in one SQLite `BEGIN IMMEDIATE` transaction.
//!
//! The listing reads live in [`read`], payload reads in [`payload`], the
//! withdrawal transaction in [`withdraw`], and shared row mapping in [`rows`].

#![forbid(unsafe_code)]
#![allow(
    clippy::module_name_repetitions,
    reason = "public repository values retain their queued-message context at crate boundaries"
)]

mod payload;
mod read;
mod rows;
mod withdraw;

use sea_orm::DbErr;
use thiserror::Error;

use artisan_domain::{
    FailedMessageListError, FailedMessageListingError, MessageId, QueuedMessageListError,
    QueuedMessageListingError, RequestId, ThreadId,
};

use super::RepositoryFailure;
/// Typed failures at the queued-message persistence boundary.
///
/// Error variants carry identities and bounded diagnostics only. Authored
/// text and image bytes are available only through the explicit payload read
/// seam after a successful withdrawal.
#[derive(Debug, Error)]
pub enum QueuedMessageRepositoryError {
    /// The withdrawal request id already names a different command payload.
    #[error("queued-message withdrawal request `{request_id}` conflicts with an existing payload")]
    IdempotencyConflict {
        /// Conflicting withdrawal request identity.
        request_id: RequestId,
    },
    /// The supplied thread is not present in native persistence.
    #[error("thread `{thread_id}` does not exist")]
    ThreadNotFound {
        /// Supplied thread identity.
        thread_id: ThreadId,
    },
    /// A message was supplied under a thread that does not own it.
    #[error("message `{message_id}` is not owned by thread `{thread_id}`")]
    CrossThread {
        /// Authenticated thread supplied by the caller.
        thread_id: ThreadId,
        /// Message found under another thread.
        message_id: MessageId,
    },
    /// The original request does not identify the supplied queue message.
    #[error(
        "original queue request `{original_request_id}` does not identify message `{message_id}`"
    )]
    OriginalRequestMismatch {
        /// Supplied original request identity.
        original_request_id: RequestId,
        /// Supplied message identity.
        message_id: MessageId,
    },
    /// The withdrawal timestamp would move the durable update stamp backward.
    #[error(
        "queued-message withdrawal time {accepted_at_ms} precedes message time {message_at_ms}"
    )]
    InvalidChronology {
        /// Supplied withdrawal acceptance time.
        accepted_at_ms: i64,
        /// Original message acceptance time.
        message_at_ms: i64,
    },
    /// The caller requested a page outside the domain bound.
    #[error("invalid queued-message list limit: {0}")]
    InvalidListLimit(QueuedMessageListError),
    /// A constructed page violated one of its domain invariants.
    #[error("queued-message listing is invalid: {0}")]
    InvalidListing(#[source] QueuedMessageListingError),
    /// The caller requested a failed-dispatch page outside the domain bound.
    #[error("invalid failed-message list limit: {0}")]
    InvalidFailedListLimit(FailedMessageListError),
    /// A constructed failed-dispatch page violated one of its domain invariants.
    #[error("failed-message listing is invalid: {0}")]
    InvalidFailedListing(#[source] FailedMessageListingError),
    /// Persisted rows violate a domain or schema invariant.
    #[error("persisted `{table}.{field}` violates the queued-message contract: {reason}")]
    CorruptData {
        /// Table containing the invalid value.
        table: &'static str,
        /// Field containing the invalid value.
        field: &'static str,
        /// Bounded diagnostic reason.
        reason: String,
    },
    /// A local repository invariant failed while applying a transaction.
    #[error("queued-message persistence invariant failed: {reason}")]
    Invariant {
        /// Stable invariant description.
        reason: &'static str,
    },
    /// A database operation failed.
    #[error("database operation `{operation}` failed")]
    Database {
        /// Operation being attempted.
        operation: &'static str,
        /// Original `SeaORM` error.
        #[source]
        source: DbErr,
    },
}

impl RepositoryFailure for QueuedMessageRepositoryError {
    fn corrupt_data(table: &'static str, field: &'static str, reason: String) -> Self {
        Self::CorruptData {
            table,
            field,
            reason,
        }
    }

    fn database_error(operation: &'static str, source: DbErr) -> Self {
        Self::Database { operation, source }
    }
}
