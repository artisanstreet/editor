//! Bounded, backed-off retry for one turn's observation batch commit.
//!
//! The dispatcher's batch path retries a commit that failed on SQLite
//! writer-lock contention on the one shared write-retry schedule, and reports
//! a typed refusal when the command is permanently rejected or the bounded
//! schedule is exhausted. Exhaustion is never a silent idle: the caller marks
//! the turn's progress uncertain and the turn settles as interrupted so the
//! unpersisted output is surfaced instead of dropped.

#![forbid(unsafe_code)]

use std::time::Duration;

use artisan_database::{
    AssistantChange, CheckpointUpdate, CommitRunBatch, CommitRunBatchOutcome, Repository,
    RepositoryError, RunBatchScope, RunObservationError,
};
use artisan_domain::{PatchId, UnixMillis};
use sea_orm::DbErr;

use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::sqlite_write_retry_policy::{
    SqliteWriteErrorObservation, SqliteWriteRetryDecision, SqliteWriteRetryPolicy,
};

use super::dispatch_policy::notify_after_commit;

/// One batch commit request captured from the live turn state.
pub(crate) struct CommitBatchRequest<'a> {
    /// Repository that owns the run's durable scope.
    pub(crate) repository: &'a Repository,
    /// Notifier woken only after a commit or exact replay.
    pub(crate) notifier: &'a ConversationCommitNotifier,
    /// Full pair snapshot and credentials fencing the batch.
    pub(crate) scope: &'a RunBatchScope<'a>,
    /// Strictly next batch sequence.
    pub(crate) batch_sequence: i64,
    /// Caller-injected operation time.
    pub(crate) operated_at: UnixMillis,
    /// Turn activation patch, when this batch opens the active turn.
    pub(crate) activate_turn_patch_id: Option<&'a PatchId>,
    /// Ordered run-scoped assistant mutations.
    pub(crate) changes: &'a [AssistantChange<'a>],
    /// Keep or replace the persisted checkpoint.
    pub(crate) checkpoint: CheckpointUpdate<'a>,
    /// Bounded command-attempt budget from the dispatcher configuration.
    pub(crate) retries: std::num::NonZeroUsize,
}

/// Why one batch commit stopped without a durable receipt.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CommitBatchFailure {
    /// The database rejected the exact command; retrying cannot help.
    #[error("run batch commit was rejected after {attempts} attempt(s)")]
    Rejected {
        /// Total attempts made, at least one.
        attempts: usize,
        /// Repository refusal that produced the decision.
        #[source]
        source: RunObservationError,
    },
    /// Every bounded attempt failed on retryable writer contention.
    #[error("run batch commit exhausted {attempts} attempt(s) on writer contention")]
    Exhausted {
        /// Total attempts made, at least one.
        attempts: usize,
        /// Last retryable repository failure.
        #[source]
        source: RunObservationError,
    },
}

impl CommitBatchFailure {
    /// Returns the total number of attempts the commit made.
    #[must_use]
    pub(crate) const fn attempts(&self) -> usize {
        match self {
            Self::Rejected { attempts, .. } | Self::Exhausted { attempts, .. } => *attempts,
        }
    }

    /// Returns whether the final failure was retryable writer contention.
    #[must_use]
    pub(crate) const fn retryable(&self) -> bool {
        matches!(self, Self::Exhausted { .. })
    }
}

/// Commits one batch, retrying retryable writer contention on the shared
/// bounded schedule and surfacing a typed refusal otherwise.
///
/// # Errors
///
/// Returns [`CommitBatchFailure`] when the command was permanently rejected
/// or the bounded retry schedule was exhausted; the caller must treat both as
/// failed durable progress.
pub(crate) async fn commit_batch_with_retry(
    request: CommitBatchRequest<'_>,
) -> Result<(), CommitBatchFailure> {
    let CommitBatchRequest {
        repository,
        notifier,
        scope,
        batch_sequence,
        operated_at,
        activate_turn_patch_id,
        changes,
        checkpoint,
        retries,
    } = request;
    let max_attempts = retries.get();
    let mut completed_repetitions = 0_u32;
    loop {
        let result = repository
            .commit_run_batch(CommitRunBatch {
                scope: RunBatchScope {
                    claimed: scope.claimed,
                    launched: scope.launched,
                    bound: scope.bound,
                    run_start_key: scope.run_start_key,
                    credentials: scope.credentials,
                    expected_launch_at: scope.expected_launch_at,
                    expected_updated_at: scope.expected_updated_at,
                },
                batch_sequence,
                operated_at,
                activate_turn_patch_id,
                changes,
                checkpoint,
            })
            .await;
        // Wake subscribers only for a durably committed batch or its exact
        // receipt replay.
        if notify_after_commit(
            matches!(
                &result,
                Ok(CommitRunBatchOutcome::Committed(_)
                    | CommitRunBatchOutcome::AlreadyCommitted(_))
            ),
            || {
                let _ = notifier.publish(&scope.launched.thread_id);
            },
        ) {
            return Ok(());
        }
        let Err(error) = result else {
            unreachable!("committed outcomes return through notify_after_commit")
        };
        let attempts = usize::try_from(completed_repetitions).unwrap_or(usize::MAX) + 1;
        // A permanent refusal is never retried and never consumes schedule
        // repetitions. A retryable failure consumes one schedule repetition
        // per retry; `None` means the schedule or the configured attempt
        // budget is exhausted.
        let delay = commit_retry_delay(&error, completed_repetitions);
        if let Some(delay) = delay
            && attempts < max_attempts
        {
            tokio::time::sleep(delay).await;
            completed_repetitions += 1;
            continue;
        }
        let failure = if classify_commit_error(&error).is_retryable() {
            CommitBatchFailure::Exhausted {
                attempts,
                source: error,
            }
        } else {
            CommitBatchFailure::Rejected {
                attempts,
                source: error,
            }
        };
        eprintln!(
            "native run dispatch batch commit failed after {} attempt(s) (retryable: {}): {failure}",
            failure.attempts(),
            failure.retryable()
        );
        return Err(failure);
    }
}

/// Returns the bounded delay before the next attempt, or `None` when the
/// failure is permanent or the shared schedule is exhausted.
pub(crate) fn commit_retry_delay(
    error: &RunObservationError,
    completed_repetitions: u32,
) -> Option<Duration> {
    match SqliteWriteRetryPolicy::decide(&classify_commit_error(error), completed_repetitions) {
        SqliteWriteRetryDecision::Retry { delay, .. } => Some(delay),
        SqliteWriteRetryDecision::NotRetryable | SqliteWriteRetryDecision::Exhausted => None,
    }
}

/// Observes the concrete database failure carried by a repository error.
pub(crate) fn classify_commit_error(error: &RunObservationError) -> SqliteWriteErrorObservation {
    database_error_source(error).map_or_else(
        SqliteWriteErrorObservation::ordinary,
        SqliteWriteErrorObservation::from_db_error,
    )
}

/// Extracts the SQLite driver failure from a repository database error.
fn database_error_source(error: &RunObservationError) -> Option<&DbErr> {
    let RunObservationError::Repository(RepositoryError::Database { source, .. }) = error else {
        return None;
    };
    Some(source)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::error::Error as StdError;
    use std::sync::Arc;

    use sea_orm::sqlx::error::{DatabaseError, ErrorKind};
    use sea_orm::{DbErr, RuntimeErr};

    use super::*;

    #[derive(Debug)]
    struct FakeDatabaseError {
        code: i32,
    }

    impl std::fmt::Display for FakeDatabaseError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "(code: {}) database is locked", self.code)
        }
    }

    impl StdError for FakeDatabaseError {}

    impl DatabaseError for FakeDatabaseError {
        fn message(&self) -> &'static str {
            "database is locked"
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Owned(self.code.to_string()))
        }

        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    fn observation_error(code: i32) -> RunObservationError {
        RunObservationError::Repository(RepositoryError::Database {
            operation: "commit run batch",
            source: DbErr::Exec(RuntimeErr::SqlxError(Arc::new(
                sea_orm::sqlx::Error::Database(Box::new(FakeDatabaseError { code })),
            ))),
        })
    }

    #[test]
    fn retryable_contention_uses_the_shared_bounded_schedule() {
        let error = observation_error(517);

        assert_eq!(
            commit_retry_delay(&error, 0),
            Some(Duration::from_millis(5))
        );
        assert_eq!(
            commit_retry_delay(&error, 7),
            Some(Duration::from_millis(640))
        );
        assert_eq!(commit_retry_delay(&error, 8), None);
        assert!(classify_commit_error(&error).is_retryable());
    }

    #[test]
    fn permanent_and_driver_independent_failures_never_retry() {
        let constraint = observation_error(19);
        assert_eq!(commit_retry_delay(&constraint, 0), None);
        assert!(!classify_commit_error(&constraint).is_retryable());

        let internal = RunObservationError::Repository(RepositoryError::Database {
            operation: "commit run batch",
            source: DbErr::Query(RuntimeErr::Internal("syntax error".to_owned())),
        });
        assert_eq!(commit_retry_delay(&internal, 0), None);
        assert!(!classify_commit_error(&internal).is_retryable());
    }
}
