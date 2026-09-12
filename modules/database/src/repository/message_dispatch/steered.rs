//! Steered dispatches that never enter the lease claim path.

#![forbid(unsafe_code)]

use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Statement};

use artisan_domain::{MessageId, UnixMillis};

use crate::entities::{self, DispatchState};
use crate::repository::{Repository, RepositoryError, database_error, millis};

use super::{DispatchFailureReason, TransitionedMessageDispatch};

/// Completes a steered dispatch that was never leased.
///
/// Steered rows never enter the claim path (`CLAIM_NEXT_SQL` excludes
/// them), so no lease columns participate: the fence is open-state plus
/// the steer-target guard, which keeps a miswired caller from
/// completing a fresh row through this seam. Shared with the steered
/// projection transaction in `run_launch`, which must complete under
/// the same fence.
pub(crate) const COMPLETE_STEERED_SQL: &str = r"
UPDATE message_dispatches
SET state = 'completed',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = NULL,
    updated_at_ms = ?
WHERE message_id = ?
  AND state = 'queued'
  AND steer_run_id IS NOT NULL
";

/// Fails a steered dispatch that was never leased, mirroring the
/// completion fence above.
const FAIL_STEERED_SQL: &str = r"
UPDATE message_dispatches
SET state = 'failed',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = ?,
    updated_at_ms = ?
WHERE message_id = ?
  AND state = 'queued'
  AND steer_run_id IS NOT NULL
";

/// Fails steered dispatches whose target run is no longer live.
///
/// Recovery-only: the dispatch loop calls this once at startup, when no
/// turn loop can be pumping (a live target with a live loop can only
/// exist while this process runs it). Rows whose target run is still in
/// a live lifecycle are left untouched.
const FAIL_ORPHANED_STEERED_SQL: &str = r"
UPDATE message_dispatches
SET state = 'failed',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = ?,
    updated_at_ms = ?
WHERE steer_run_id IS NOT NULL
  AND state = 'queued'
  AND NOT EXISTS (
    SELECT 1 FROM assistant_runs
    WHERE run_id = message_dispatches.steer_run_id
      AND lifecycle IN ('queued','launching','running','waiting','cancel_requested')
  )
";

impl Repository {
    /// Completes one steered dispatch that was never leased.
    ///
    /// Steered rows never enter the claim path, so no lease participates:
    /// the fence is open-state plus the steer-target guard. A zero-row
    /// result diagnoses the current row instead of failing blind.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::DispatchNotFound`] for a missing row or
    /// [`RepositoryError::IdentityConflict`] when the row already left its
    /// queued state, with the row left untouched.
    /// Completes a steered dispatch that was never leased.
    ///
    /// Terminal outcomes stay distinct: an already-completed row reports
    /// success idempotently, but an already-failed row is NEVER reported
    /// as completed — completing after failure would mislabel a dead
    /// delivery as success.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::DispatchNotFound`] for a missing row or
    /// [`RepositoryError::InvalidDispatchState`] when the row already
    /// left its queued state for anything but completion, with the row
    /// left untouched.
    pub async fn complete_steered_dispatch(
        &self,
        message_id: &MessageId,
        operated_at: UnixMillis,
    ) -> Result<TransitionedMessageDispatch, RepositoryError> {
        let operated_at_ms = millis(operated_at);
        let updated = self
            .database
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                COMPLETE_STEERED_SQL,
                [operated_at_ms.into(), message_id.as_str().into()],
            ))
            .await
            .map_err(|source| database_error("complete steered dispatch", source))?
            .rows_affected();
        if updated == 1 {
            return Ok(TransitionedMessageDispatch {
                message_id: message_id.clone(),
                updated_at: operated_at,
            });
        }
        match classify_steered_transition_miss(&self.database, message_id).await? {
            SteeredRowTerminal::Completed(transitioned) => Ok(transitioned),
            SteeredRowTerminal::Failed(_) => Err(RepositoryError::InvalidDispatchState {
                message_id: message_id.clone(),
                state: "failed",
            }),
        }
    }

    /// Fails one steered dispatch terminally without a lease.
    ///
    /// The reason arrives as a bounded static diagnostic from the
    /// dispatch-side steer arm; an overlong reason is a programming
    /// error and surfaces as an invariant rather than truncating.
    /// Terminal outcomes stay distinct, mirroring completion: an
    /// already-failed row reports success idempotently, but an
    /// already-completed row is NEVER failed.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::DispatchNotFound`] for a missing row or
    /// [`RepositoryError::InvalidDispatchState`] when the row already
    /// left its queued state for anything but failure, with the row left
    /// untouched.
    pub async fn fail_steered_dispatch(
        &self,
        message_id: &MessageId,
        reason: &'static str,
        operated_at: UnixMillis,
    ) -> Result<TransitionedMessageDispatch, RepositoryError> {
        let reason =
            DispatchFailureReason::parse(reason).map_err(|_| RepositoryError::Invariant {
                reason: "steer refusal reason exceeds its bound",
            })?;
        let operated_at_ms = millis(operated_at);
        let updated = self
            .database
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                FAIL_STEERED_SQL,
                [
                    reason.as_str().into(),
                    operated_at_ms.into(),
                    message_id.as_str().into(),
                ],
            ))
            .await
            .map_err(|source| database_error("fail steered dispatch", source))?
            .rows_affected();
        if updated == 1 {
            return Ok(TransitionedMessageDispatch {
                message_id: message_id.clone(),
                updated_at: operated_at,
            });
        }
        match classify_steered_transition_miss(&self.database, message_id).await? {
            SteeredRowTerminal::Failed(transitioned) => Ok(transitioned),
            SteeredRowTerminal::Completed(_) => Err(RepositoryError::InvalidDispatchState {
                message_id: message_id.clone(),
                state: "completed",
            }),
        }
    }

    /// Reads one dispatch row's steer delivery state for replay consult.
    ///
    /// Returns the row state, its persisted failure reason (if failed),
    /// and its persisted steer target. Request-side replay uses this to
    /// decide receipt / refusal-reproduction / safe reroute without ever
    /// delivering twice.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::DispatchNotFound`] for a missing row.
    pub async fn read_steered_dispatch_state(
        &self,
        message_id: &MessageId,
    ) -> Result<(DispatchState, Option<String>, Option<String>), RepositoryError> {
        let Some(row) = entities::message_dispatch::Entity::find_by_id(message_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| database_error("read steered dispatch state", source))?
        else {
            return Err(RepositoryError::DispatchNotFound {
                message_id: message_id.clone(),
            });
        };
        Ok((row.state, row.last_error, row.steer_run_id))
    }

    /// Fails steered dispatches whose target run is no longer live.
    ///
    /// Recovery-only: the dispatch loop calls this once at startup, when
    /// no turn loop can be pumping. Rows whose target run is still in a
    /// live lifecycle are left untouched for their owning loop.
    ///
    /// Returns the number of rows failed.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] when the constant failure reason would
    /// exceed its persisted bound or the recovery update fails.
    pub async fn fail_orphaned_steered_dispatches(
        &self,
        operated_at: UnixMillis,
    ) -> Result<u64, RepositoryError> {
        let reason =
            DispatchFailureReason::parse("steer target run is no longer live").map_err(|_| {
                RepositoryError::Invariant {
                    reason: "steer orphan failure reason exceeds its bound",
                }
            })?;
        let operated_at_ms = millis(operated_at);
        let updated = self
            .database
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                FAIL_ORPHANED_STEERED_SQL,
                [reason.as_str().into(), operated_at_ms.into()],
            ))
            .await
            .map_err(|source| database_error("fail orphaned steered dispatches", source))?
            .rows_affected();
        Ok(updated)
    }
}

/// Validates one RETURNING row of a fenced lifecycle transition.
/// Diagnoses a steered transition that changed no row.
///
/// A zero-row result means the dispatch left its queued state
/// concurrently (sweep failure, a racing completion) or never existed.
/// Terminal states report distinctly so callers never mistake them for
/// a fresh failure.
/// Terminal state of a steered dispatch that left its queued state.
///
/// Terminal outcomes stay distinct: a completed row must never report
/// completion success for a failed row, and a failed row must never be
/// completed. Callers map each side explicitly. Shared with the steered
/// projection transaction.
pub(crate) enum SteeredRowTerminal {
    Completed(TransitionedMessageDispatch),
    Failed(TransitionedMessageDispatch),
}

pub(crate) async fn classify_steered_transition_miss(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
) -> Result<SteeredRowTerminal, RepositoryError> {
    let Some(row) = entities::message_dispatch::Entity::find_by_id(message_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find steered dispatch", source))?
    else {
        return Err(RepositoryError::DispatchNotFound {
            message_id: message_id.clone(),
        });
    };
    let transitioned = TransitionedMessageDispatch {
        message_id: message_id.clone(),
        updated_at: UnixMillis::from_millis(row.updated_at_ms),
    };
    match row.state {
        DispatchState::Completed => Ok(SteeredRowTerminal::Completed(transitioned)),
        DispatchState::Failed => Ok(SteeredRowTerminal::Failed(transitioned)),
        DispatchState::Queued => Err(RepositoryError::InvalidDispatchState {
            message_id: message_id.clone(),
            state: "queued",
        }),
        DispatchState::Leased => Err(RepositoryError::InvalidDispatchState {
            message_id: message_id.clone(),
            state: "leased",
        }),
        DispatchState::Running => Err(RepositoryError::InvalidDispatchState {
            message_id: message_id.clone(),
            state: "running",
        }),
    }
}
