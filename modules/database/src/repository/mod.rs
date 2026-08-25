//! Domain-typed repositories for the native schema.

mod first_message;
mod message_dispatch;
mod project_catalog;
mod project_threads;

use sea_orm::{DatabaseConnection, DbErr};
use thiserror::Error;

use artisan_domain::{
    MessageId, ProjectId, ProjectListingError, RequestId, RootPath, ThreadId, ThreadListingError,
    UnixMillis,
};

pub use first_message::{QueueFirstMessageInput, QueueFirstMessageResult};
pub use message_dispatch::{
    ClaimMessageDispatch, ClaimedMessageDispatch, CompleteMessageDispatch, DispatchFailureReason,
    DispatchFailureReasonError, DispatchLeaseOwner, DispatchLeaseOwnerError, FailMessageDispatch,
    RequeueMessageDispatch, TransitionedMessageDispatch,
};
pub use project_threads::{
    AttachProjectInput, AttachProjectResult, CreateThreadInput, CreateThreadResult,
};

/// Typed failures at the native persistence boundary.
#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("project `{project_id}` is not attached")]
    ProjectNotFound { project_id: ProjectId },

    #[error(
        "project id `{project_id}` is already attached at `{existing_root_path}`, not `{requested_root_path}`"
    )]
    ProjectConflict {
        project_id: ProjectId,
        existing_root_path: RootPath,
        requested_root_path: RootPath,
    },

    #[error("thread `{thread_id}` is not attached to a known project")]
    ThreadNotFound { thread_id: ThreadId },

    #[error("thread `{thread_id}` already exists with different persisted values")]
    ThreadConflict { thread_id: ThreadId },

    #[error("message id `{message_id}` already identifies a different message")]
    MessageConflict { message_id: MessageId },

    #[error(
        "thread `{thread_id}` already has first message `{existing_message_id}` from another request"
    )]
    FirstMessageAlreadyExists {
        thread_id: ThreadId,
        existing_message_id: MessageId,
    },

    #[error("request id `{request_id}` was already used for a different command")]
    IdempotencyConflict { request_id: RequestId },

    #[error(
        "dispatch lease expiry {lease_expires_at_ms} must be later than claim time {claimed_at_ms}"
    )]
    InvalidDispatchLeaseWindow {
        claimed_at_ms: i64,
        lease_expires_at_ms: i64,
    },

    #[error("message dispatch for `{message_id}` exhausted its persisted attempt counter")]
    DispatchAttemptLimit { message_id: MessageId },

    #[error("message dispatch `{message_id}` does not exist")]
    DispatchNotFound { message_id: MessageId },

    #[error(
        "message dispatch `{message_id}` is {state}; only a live leased dispatch accepts lifecycle transitions"
    )]
    InvalidDispatchState {
        message_id: MessageId,
        state: &'static str,
    },

    #[error("message dispatch `{message_id}` belongs to a different lease owner")]
    DispatchOwnerMismatch { message_id: MessageId },

    #[error(
        "lease on message dispatch `{message_id}` expired at {lease_expires_at_ms}, no later than the operation time {operated_at_ms}"
    )]
    DispatchLeaseExpired {
        message_id: MessageId,
        lease_expires_at_ms: i64,
        operated_at_ms: i64,
    },

    #[error("{later_field} timestamp precedes {earlier_field} timestamp")]
    InvalidChronology {
        earlier_field: &'static str,
        later_field: &'static str,
    },

    #[error("persisted `{table}.{field}` violates the domain contract: {reason}")]
    CorruptData {
        table: &'static str,
        field: &'static str,
        reason: String,
    },

    #[error("persisted thread listing exceeds its domain bound")]
    ThreadListing {
        #[source]
        source: ThreadListingError,
    },

    #[error("persisted project listing exceeds its domain bound")]
    ProjectListing {
        #[source]
        source: ProjectListingError,
    },

    #[error("native database invariant failed: {reason}")]
    Invariant { reason: &'static str },

    #[error("database operation `{operation}` failed")]
    Database {
        operation: &'static str,
        #[source]
        source: DbErr,
    },
}

/// Cloneable access to native database repositories.
#[derive(Clone, Debug)]
pub struct Repository {
    database: DatabaseConnection,
}

impl Repository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }
}

fn database_error(operation: &'static str, source: DbErr) -> RepositoryError {
    RepositoryError::Database { operation, source }
}

fn corrupt_data(
    table: &'static str,
    field: &'static str,
    reason: &(impl ToString + ?Sized),
) -> RepositoryError {
    RepositoryError::CorruptData {
        table,
        field,
        reason: reason.to_string(),
    }
}

fn millis(value: UnixMillis) -> i64 {
    value.as_millis()
}
