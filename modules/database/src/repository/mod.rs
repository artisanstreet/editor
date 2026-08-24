//! Domain-typed repositories for the native schema.

mod project_threads;

use sea_orm::{DatabaseConnection, DbErr};
use thiserror::Error;

use artisan_domain::{ProjectId, RequestId, RootPath, ThreadId, ThreadListingError, UnixMillis};

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

    #[error("request id `{request_id}` was already used for a different command")]
    IdempotencyConflict { request_id: RequestId },

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
