//! Owner-fenced dispatch lease renewal.

#![forbid(unsafe_code)]

use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, QueryResult, Statement};

use artisan_domain::{MessageId, UnixMillis};

use crate::entities::{self, DispatchState};
use crate::repository::{
    Repository, RepositoryError, corrupt_data, database_error, millis, row_value,
};

use super::transition::{rollback_transition, state_label};
use super::{DispatchLeaseOwner, TransitionedMessageDispatch};

/// Extends one owned dispatch lease without changing its state.
///
/// The owner token is the authority: a dispatcher heartbeats its own live
/// claim while a provider call runs, so a turn that legitimately outlives the
/// original lease window is neither reaped by the recovery sweep nor fenced
/// out of its own terminal settlement. Extending an already expired lease is
/// safe because a competing claim mints a different owner, so this statement
/// can never steal a row another dispatcher owns.
const RENEW_DISPATCH_LEASE_SQL: &str = r"
UPDATE message_dispatches
SET lease_expires_at_ms = ?,
    updated_at_ms = ?
WHERE message_id = ?
  AND state = 'leased'
  AND lease_owner = ?
RETURNING message_id,
          attempt_count,
          available_at_ms,
          lease_expires_at_ms,
          updated_at_ms
";

impl Repository {
    /// Extends one owned dispatch lease while its turn is still running.
    ///
    /// The single owner-fenced UPDATE moves only the lease expiry and update
    /// stamp; state, attempt count, availability, and failure text are
    /// untouched. The owner token is the only authority, so an expired lease
    /// that is still owned by this dispatcher is renewable: a competing claim
    /// mints a new owner and therefore cannot be overridden here.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::InvalidDispatchLeaseWindow`] when the
    /// requested expiry is not later than `operated_at`, or
    /// [`RepositoryError::DispatchNotFound`],
    /// [`RepositoryError::InvalidDispatchState`],
    /// [`RepositoryError::DispatchOwnerMismatch`],
    /// [`RepositoryError::InvalidChronology`], or corrupt-data, invariant, or
    /// database failures that roll back the transaction before they return.
    pub async fn renew_message_dispatch_lease(
        &self,
        message_id: &MessageId,
        owner: &DispatchLeaseOwner,
        operated_at: UnixMillis,
        lease_expires_at: UnixMillis,
    ) -> Result<TransitionedMessageDispatch, RepositoryError> {
        let operated_at_ms = millis(operated_at);
        let lease_expires_at_ms = millis(lease_expires_at);
        if lease_expires_at_ms <= operated_at_ms {
            return Err(RepositoryError::InvalidDispatchLeaseWindow {
                claimed_at_ms: operated_at_ms,
                lease_expires_at_ms,
            });
        }
        let encoded_owner = owner.to_storage();
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin message-dispatch lease renewal", source))?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            RENEW_DISPATCH_LEASE_SQL,
            [
                lease_expires_at_ms.into(),
                operated_at_ms.into(),
                message_id.as_str().into(),
                encoded_owner.into(),
            ],
        );
        let returned = transaction
            .query_one_raw(statement)
            .await
            .map_err(|source| database_error("renew message dispatch lease", source));
        let returned = match returned {
            Ok(returned) => returned,
            Err(error) => {
                return rollback_transition(
                    transaction,
                    "roll back message-dispatch lease renewal",
                    error,
                )
                .await;
            }
        };
        let Some(row) = returned else {
            let rejection =
                classify_unfenced_lease_renewal(&transaction, message_id, owner, operated_at_ms)
                    .await;
            return rollback_transition(
                transaction,
                "roll back message-dispatch lease renewal",
                rejection,
            )
            .await;
        };
        let transitioned =
            match renewed_from_row(&row, message_id, operated_at_ms, lease_expires_at_ms) {
                Ok(transitioned) => transitioned,
                Err(error) => {
                    return rollback_transition(
                        transaction,
                        "roll back message-dispatch lease renewal",
                        error,
                    )
                    .await;
                }
            };

        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit message-dispatch lease renewal", source))?;
        Ok(transitioned)
    }
}

fn renewed_from_row(
    row: &QueryResult,
    message_id: &MessageId,
    operated_at_ms: i64,
    lease_expires_at_ms: i64,
) -> Result<TransitionedMessageDispatch, RepositoryError> {
    let returned_id = row_value::<String, _>(row, 0, "message_id", "message_dispatches")?;
    if returned_id != message_id.as_str() {
        return Err(RepositoryError::Invariant {
            reason: "renewed lease returned a different message id",
        });
    }
    let attempt_count = row_value::<i64, _>(row, 1, "attempt_count", "message_dispatches")?;
    let attempt_count = u32::try_from(attempt_count)
        .map_err(|error| corrupt_data("message_dispatches", "attempt_count", error))?;
    if attempt_count == 0 {
        return Err(RepositoryError::Invariant {
            reason: "renewed lease retained a zero attempt count",
        });
    }
    let returned_expiry = row_value::<i64, _>(row, 3, "lease_expires_at_ms", "message_dispatches")?;
    if returned_expiry != lease_expires_at_ms {
        return Err(RepositoryError::Invariant {
            reason: "renewed lease returned inconsistent lease timestamps",
        });
    }
    let updated_at_ms = row_value::<i64, _>(row, 4, "updated_at_ms", "message_dispatches")?;
    if updated_at_ms != operated_at_ms {
        return Err(RepositoryError::Invariant {
            reason: "renewed lease returned inconsistent update timestamps",
        });
    }

    Ok(TransitionedMessageDispatch {
        message_id: message_id.clone(),
        updated_at: UnixMillis::from_millis(updated_at_ms),
    })
}

/// Classifies a rejected lease renewal inside its still-open transaction.
///
/// Mirrors [`classify_unfenced_transition`] except that an expired lease is
/// deliberately not a rejection: renewal exists to carry an owned lease
/// across its old expiry while the same dispatcher still runs the turn.
async fn classify_unfenced_lease_renewal(
    transaction: &sea_orm::DatabaseTransaction,
    message_id: &MessageId,
    owner: &DispatchLeaseOwner,
    operated_at_ms: i64,
) -> RepositoryError {
    let row = entities::message_dispatch::Entity::find_by_id(message_id.as_str())
        .one(transaction)
        .await;
    let row = match row {
        Ok(row) => row,
        Err(source) => {
            return database_error("classify unfenced message-dispatch lease renewal", source);
        }
    };
    let Some(row) = row else {
        return RepositoryError::DispatchNotFound {
            message_id: message_id.clone(),
        };
    };
    if row.state != DispatchState::Leased {
        return RepositoryError::InvalidDispatchState {
            message_id: message_id.clone(),
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
        return RepositoryError::DispatchOwnerMismatch {
            message_id: message_id.clone(),
        };
    }
    if row.updated_at_ms > operated_at_ms {
        return RepositoryError::InvalidChronology {
            earlier_field: "message_dispatches.updated_at_ms",
            later_field: "message_dispatches.operated_at",
        };
    }
    RepositoryError::Invariant {
        reason: "fenced lease renewal matched no rows without an identifiable cause",
    }
}
