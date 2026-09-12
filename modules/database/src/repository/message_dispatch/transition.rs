//! Owner-fenced terminal transitions and retryable requeue.

#![forbid(unsafe_code)]

use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, QueryResult, Statement};

use artisan_domain::{MessageId, UnixMillis};

use crate::entities::{self, DispatchState};
use crate::repository::{
    Repository, RepositoryError, corrupt_data, database_error, millis, row_value,
};

use super::{
    CompleteMessageDispatch, DispatchLeaseOwner, FailMessageDispatch, RequeueMessageDispatch,
    TransitionedMessageDispatch,
};

const COMPLETE_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET state = 'completed',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = NULL,
    updated_at_ms = ?
WHERE message_id = ?
  AND state = 'leased'
  AND lease_owner = ?
  AND lease_expires_at_ms > ?
  AND updated_at_ms <= ?
RETURNING message_id,
          attempt_count,
          available_at_ms,
          lease_expires_at_ms,
          updated_at_ms
";

const FAIL_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET state = 'failed',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = ?,
    updated_at_ms = ?
WHERE message_id = ?
  AND state = 'leased'
  AND lease_owner = ?
  AND lease_expires_at_ms > ?
  AND updated_at_ms <= ?
RETURNING message_id,
          attempt_count,
          available_at_ms,
          lease_expires_at_ms,
          updated_at_ms
";

const REQUEUE_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET state = 'queued',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = ?,
    available_at_ms = ?,
    updated_at_ms = ?
WHERE message_id = ?
  AND state = 'leased'
  AND lease_owner = ?
  AND lease_expires_at_ms > ?
  AND updated_at_ms <= ?
RETURNING message_id,
          attempt_count,
          available_at_ms,
          lease_expires_at_ms,
          updated_at_ms
";

impl Repository {
    /// Completes one claimed dispatch under its live lease.
    ///
    /// The single fenced UPDATE moves a `leased` row to `completed`, clears
    /// all lease metadata and the persisted failure reason, and stamps the
    /// operation time without ever moving the row's update stamp backwards.
    /// The attempt count is preserved; only the claim path increments it. At
    /// expiry equality the lease is already dead and the transition is
    /// rejected.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::DispatchNotFound`],
    /// [`RepositoryError::InvalidDispatchState`],
    /// [`RepositoryError::DispatchOwnerMismatch`],
    /// [`RepositoryError::InvalidChronology`], or
    /// [`RepositoryError::DispatchLeaseExpired`] with the row left untouched,
    /// or corrupt-data, invariant, or database failures that roll back the
    /// transaction before they are returned.
    pub async fn complete_message_dispatch(
        &self,
        command: CompleteMessageDispatch,
    ) -> Result<TransitionedMessageDispatch, RepositoryError> {
        let operated_at_ms = millis(command.operated_at);
        let encoded_owner = command.owner.to_storage();
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin message-dispatch completion", source))?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            COMPLETE_DISPATCH_SQL,
            [
                operated_at_ms.into(),
                command.message_id.as_str().into(),
                encoded_owner.into(),
                operated_at_ms.into(),
                operated_at_ms.into(),
            ],
        );
        let returned = transaction
            .query_one_raw(statement)
            .await
            .map_err(|source| database_error("complete message dispatch", source));
        let returned = match returned {
            Ok(returned) => returned,
            Err(error) => {
                return rollback_transition(
                    transaction,
                    "roll back message-dispatch completion",
                    error,
                )
                .await;
            }
        };
        let Some(row) = returned else {
            let rejection = classify_unfenced_transition(
                &transaction,
                command.message_id,
                &command.owner,
                operated_at_ms,
            )
            .await;
            return rollback_transition(
                transaction,
                "roll back message-dispatch completion",
                rejection,
            )
            .await;
        };
        let transitioned =
            match transitioned_from_row(&row, &command.message_id, operated_at_ms, None) {
                Ok(transitioned) => transitioned,
                Err(error) => {
                    return rollback_transition(
                        transaction,
                        "roll back message-dispatch completion",
                        error,
                    )
                    .await;
                }
            };

        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit message-dispatch completion", source))?;
        Ok(transitioned)
    }

    /// Fails one claimed dispatch terminally under its live lease.
    ///
    /// The single fenced UPDATE moves a `leased` row to `failed`, persists the
    /// bounded failure reason verbatim, clears all lease metadata, and stamps
    /// the operation time without ever moving the row's update stamp
    /// backwards. The attempt count is preserved. At expiry equality the
    /// lease is already dead and the transition is rejected.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::DispatchNotFound`],
    /// [`RepositoryError::InvalidDispatchState`],
    /// [`RepositoryError::DispatchOwnerMismatch`],
    /// [`RepositoryError::InvalidChronology`], or
    /// [`RepositoryError::DispatchLeaseExpired`] with the row left untouched,
    /// or corrupt-data, invariant, or database failures that roll back the
    /// transaction before they are returned.
    pub async fn fail_message_dispatch(
        &self,
        command: FailMessageDispatch,
    ) -> Result<TransitionedMessageDispatch, RepositoryError> {
        let operated_at_ms = millis(command.operated_at);
        let encoded_owner = command.owner.to_storage();
        let reason = command.reason.as_str().to_owned();
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin message-dispatch terminal failure", source))?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            FAIL_DISPATCH_SQL,
            [
                reason.into(),
                operated_at_ms.into(),
                command.message_id.as_str().into(),
                encoded_owner.into(),
                operated_at_ms.into(),
                operated_at_ms.into(),
            ],
        );
        let returned = transaction
            .query_one_raw(statement)
            .await
            .map_err(|source| database_error("fail message dispatch", source));
        let returned = match returned {
            Ok(returned) => returned,
            Err(error) => {
                return rollback_transition(
                    transaction,
                    "roll back message-dispatch terminal failure",
                    error,
                )
                .await;
            }
        };
        let Some(row) = returned else {
            let rejection = classify_unfenced_transition(
                &transaction,
                command.message_id,
                &command.owner,
                operated_at_ms,
            )
            .await;
            return rollback_transition(
                transaction,
                "roll back message-dispatch terminal failure",
                rejection,
            )
            .await;
        };
        let transitioned =
            match transitioned_from_row(&row, &command.message_id, operated_at_ms, None) {
                Ok(transitioned) => transitioned,
                Err(error) => {
                    return rollback_transition(
                        transaction,
                        "roll back message-dispatch terminal failure",
                        error,
                    )
                    .await;
                }
            };

        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit message-dispatch terminal failure", source))?;
        Ok(transitioned)
    }

    /// Requeues one claimed dispatch for retry under its live lease.
    ///
    /// The single fenced UPDATE moves a `leased` row to `queued`, persists the
    /// bounded failure reason verbatim, writes the caller-supplied absolute
    /// availability instant, clears all lease metadata, and stamps the
    /// operation time without ever moving the row's update stamp backwards.
    /// The attempt count is preserved; only the claim path increments it. Any
    /// signed availability is valid, including equality with or times earlier
    /// than the operation time for immediate retry. At expiry equality the
    /// lease is already dead and the transition is rejected.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::DispatchNotFound`],
    /// [`RepositoryError::InvalidDispatchState`],
    /// [`RepositoryError::DispatchOwnerMismatch`],
    /// [`RepositoryError::InvalidChronology`], or
    /// [`RepositoryError::DispatchLeaseExpired`] with the row left untouched,
    /// or corrupt-data, invariant, or database failures that roll back the
    /// transaction before they are returned.
    pub async fn requeue_message_dispatch(
        &self,
        command: RequeueMessageDispatch,
    ) -> Result<TransitionedMessageDispatch, RepositoryError> {
        let operated_at_ms = millis(command.operated_at);
        let available_at_ms = millis(command.available_at);
        let encoded_owner = command.owner.to_storage();
        let reason = command.reason.as_str().to_owned();
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin message-dispatch requeue", source))?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            REQUEUE_DISPATCH_SQL,
            [
                reason.into(),
                available_at_ms.into(),
                operated_at_ms.into(),
                command.message_id.as_str().into(),
                encoded_owner.into(),
                operated_at_ms.into(),
                operated_at_ms.into(),
            ],
        );
        let returned = transaction
            .query_one_raw(statement)
            .await
            .map_err(|source| database_error("requeue message dispatch", source));
        let returned = match returned {
            Ok(returned) => returned,
            Err(error) => {
                return rollback_transition(
                    transaction,
                    "roll back message-dispatch requeue",
                    error,
                )
                .await;
            }
        };
        let Some(row) = returned else {
            let rejection = classify_unfenced_transition(
                &transaction,
                command.message_id,
                &command.owner,
                operated_at_ms,
            )
            .await;
            return rollback_transition(
                transaction,
                "roll back message-dispatch requeue",
                rejection,
            )
            .await;
        };
        let transitioned = match transitioned_from_row(
            &row,
            &command.message_id,
            operated_at_ms,
            Some(available_at_ms),
        ) {
            Ok(transitioned) => transitioned,
            Err(error) => {
                return rollback_transition(
                    transaction,
                    "roll back message-dispatch requeue",
                    error,
                )
                .await;
            }
        };

        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit message-dispatch requeue", source))?;
        Ok(transitioned)
    }
}

fn transitioned_from_row(
    row: &QueryResult,
    message_id: &MessageId,
    operated_at_ms: i64,
    expected_available_at_ms: Option<i64>,
) -> Result<TransitionedMessageDispatch, RepositoryError> {
    let returned_id = row_value::<String, _>(row, 0, "message_id", "message_dispatches")?;
    if returned_id != message_id.as_str() {
        return Err(RepositoryError::Invariant {
            reason: "fenced transition returned a different message id",
        });
    }
    let attempt_count = row_value::<i64, _>(row, 1, "attempt_count", "message_dispatches")?;
    let attempt_count = u32::try_from(attempt_count)
        .map_err(|error| corrupt_data("message_dispatches", "attempt_count", error))?;
    if attempt_count == 0 {
        return Err(RepositoryError::Invariant {
            reason: "transitioned dispatch retained a zero attempt count",
        });
    }
    let available_at_ms = row_value::<i64, _>(row, 2, "available_at_ms", "message_dispatches")?;
    if let Some(expected) = expected_available_at_ms
        && available_at_ms != expected
    {
        return Err(RepositoryError::Invariant {
            reason: "requeued dispatch returned inconsistent availability",
        });
    }
    let lease_expires_at_ms =
        row_value::<Option<i64>, _>(row, 3, "lease_expires_at_ms", "message_dispatches")?;
    if lease_expires_at_ms.is_some() {
        return Err(RepositoryError::Invariant {
            reason: "transitioned dispatch retained lease metadata",
        });
    }
    let updated_at_ms = row_value::<i64, _>(row, 4, "updated_at_ms", "message_dispatches")?;
    if updated_at_ms != operated_at_ms {
        return Err(RepositoryError::Invariant {
            reason: "transitioned dispatch returned inconsistent update timestamps",
        });
    }

    Ok(TransitionedMessageDispatch {
        message_id: message_id.clone(),
        updated_at: UnixMillis::from_millis(updated_at_ms),
    })
}

/// Classifies a rejected fence inside its still-open transaction.
///
/// The UPDATE matched no rows, so the persisted row decides the typed
/// rejection: missing id, invalid state, corrupt or mismatched owner using
/// private decode plus [`DispatchLeaseOwner::constant_time_eq`], expired
/// lease (at equality the lease is expired), or an otherwise unreachable
/// invariant. The caller rolls back with the returned error either way.
async fn classify_unfenced_transition(
    transaction: &sea_orm::DatabaseTransaction,
    message_id: MessageId,
    owner: &DispatchLeaseOwner,
    operated_at_ms: i64,
) -> RepositoryError {
    let row = entities::message_dispatch::Entity::find_by_id(message_id.as_str())
        .one(transaction)
        .await;
    let row = match row {
        Ok(row) => row,
        Err(source) => {
            return database_error("classify unfenced message-dispatch transition", source);
        }
    };
    let Some(row) = row else {
        return RepositoryError::DispatchNotFound { message_id };
    };
    if row.state != DispatchState::Leased {
        return RepositoryError::InvalidDispatchState {
            message_id,
            state: state_label(&row.state),
        };
    }
    let Some(persisted_owner) = row.lease_owner.as_deref() else {
        return corrupt_data(
            "message_dispatches",
            "lease_owner",
            "required value is null",
        );
    };
    let persisted_owner = match DispatchLeaseOwner::from_storage(persisted_owner) {
        Ok(persisted_owner) => persisted_owner,
        Err(error) => {
            return corrupt_data("message_dispatches", "lease_owner", error);
        }
    };
    if !persisted_owner.constant_time_eq(owner) {
        return RepositoryError::DispatchOwnerMismatch { message_id };
    }
    if row.updated_at_ms > operated_at_ms {
        return RepositoryError::InvalidChronology {
            earlier_field: "message_dispatches.updated_at_ms",
            later_field: "message_dispatches.operated_at",
        };
    }
    let Some(lease_expires_at_ms) = row.lease_expires_at_ms else {
        return corrupt_data(
            "message_dispatches",
            "lease_expires_at_ms",
            "required value is null",
        );
    };
    if lease_expires_at_ms <= operated_at_ms {
        return RepositoryError::DispatchLeaseExpired {
            message_id,
            lease_expires_at_ms,
            operated_at_ms,
        };
    }
    RepositoryError::Invariant {
        reason: "fenced transition matched no rows without an identifiable cause",
    }
}

pub(super) async fn rollback_transition<T>(
    transaction: sea_orm::DatabaseTransaction,
    operation: &'static str,
    error: RepositoryError,
) -> Result<T, RepositoryError> {
    transaction
        .rollback()
        .await
        .map_err(|source| database_error(operation, source))?;
    Err(error)
}

pub(super) const fn state_label(state: &DispatchState) -> &'static str {
    match state {
        DispatchState::Queued => "queued",
        DispatchState::Leased => "leased",
        DispatchState::Running => "running",
        DispatchState::Completed => "completed",
        DispatchState::Failed => "failed",
    }
}
