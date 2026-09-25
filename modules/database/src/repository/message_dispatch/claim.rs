//! Atomic claim of the next eligible dispatch and its diagnosis helpers.

#![forbid(unsafe_code)]

use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, QueryOrder,
    QueryResult, Statement,
};

use artisan_domain::{MessageId, RequestId, UnixMillis};

use crate::entities::{self, DispatchState};
use crate::repository::{
    Repository, RepositoryError, corrupt_data, database_error, millis, row_value,
};

use super::{
    ClaimMessageDispatch, ClaimedMessageDispatch, DispatchLeaseOwner, MAX_DISPATCH_ATTEMPTS,
};

const CLAIM_NEXT_SQL: &str = r"
UPDATE message_dispatches
SET state = 'leased',
    attempt_count = attempt_count + 1,
    lease_owner = ?,
    lease_expires_at_ms = ?,
    last_error = NULL,
    updated_at_ms = ?
WHERE message_id = (
    SELECT message_id
    FROM message_dispatches
    WHERE ((state = 'queued' AND available_at_ms <= ?)
       OR (state = 'leased' AND lease_expires_at_ms <= ?))
      AND attempt_count < ?
      AND steer_run_id IS NULL
      AND NOT EXISTS (
          SELECT 1 FROM messages candidate
          JOIN messages sibling ON sibling.thread_id = candidate.thread_id
          JOIN message_dispatches busy ON busy.message_id = sibling.message_id
          WHERE candidate.message_id = message_dispatches.message_id
            AND busy.message_id != message_dispatches.message_id
            AND busy.state IN ('leased', 'running')
            AND busy.lease_expires_at_ms > ?
      )
    ORDER BY available_at_ms ASC, queued_at_ms ASC, message_id ASC
    LIMIT 1
)
  AND ((state = 'queued' AND available_at_ms <= ?)
    OR (state = 'leased' AND lease_expires_at_ms <= ?))
  AND attempt_count < ?
  AND steer_run_id IS NULL
RETURNING message_id,
          correlation_id,
          attempt_count,
          queued_at_ms,
          available_at_ms,
          lease_owner,
          lease_expires_at_ms,
          updated_at_ms
";

impl Repository {
    /// Atomically claims the oldest eligible queued or expired dispatch.
    ///
    /// The transaction's first statement both selects and updates one row.
    /// SQLite therefore serializes competing writers before either can
    /// observe a claimable result. A leased row becomes eligible again only
    /// when its persisted expiry is at or before `claimed_at`. Dispatches at
    /// the attempt ceiling are excluded from candidate selection so they do
    /// not block later work; when every eligible row is exhausted, the oldest
    /// exhausted dispatch produces [`RepositoryError::DispatchAttemptLimit`].
    ///
    /// # Errors
    ///
    /// Returns a typed lease-window error before opening a transaction. A
    /// database, corrupt-data, invariant, or exhausted-attempt error rolls
    /// back the claim before it is returned.
    pub async fn claim_next_message_dispatch(
        &self,
        claim: ClaimMessageDispatch,
    ) -> Result<Option<ClaimedMessageDispatch>, RepositoryError> {
        let claimed_at_ms = millis(claim.claimed_at);
        let lease_expires_at_ms = millis(claim.lease_expires_at);
        if lease_expires_at_ms <= claimed_at_ms {
            return Err(RepositoryError::InvalidDispatchLeaseWindow {
                claimed_at_ms,
                lease_expires_at_ms,
            });
        }

        let encoded_owner = claim.owner.to_storage();
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin message-dispatch claim", source))?;
        let statement = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            CLAIM_NEXT_SQL,
            [
                encoded_owner.into(),
                lease_expires_at_ms.into(),
                claimed_at_ms.into(),
                claimed_at_ms.into(),
                claimed_at_ms.into(),
                i64::from(MAX_DISPATCH_ATTEMPTS).into(),
                claimed_at_ms.into(),
                claimed_at_ms.into(),
                claimed_at_ms.into(),
                i64::from(MAX_DISPATCH_ATTEMPTS).into(),
            ],
        );
        let returned = transaction
            .query_one_raw(statement)
            .await
            .map_err(|source| database_error("claim next message dispatch", source));
        let returned = match returned {
            Ok(returned) => returned,
            Err(error) => return rollback_claim(transaction, error).await,
        };

        let Some(row) = returned else {
            let result = classify_unclaimed(&transaction, claim.claimed_at).await;
            return finish_unclaimed(transaction, result).await;
        };
        let claimed = match claimed_from_row(&row, &claim.owner, claimed_at_ms, lease_expires_at_ms)
        {
            Ok(claimed) => claimed,
            Err(error) => return rollback_claim(transaction, error).await,
        };

        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit message-dispatch claim", source))?;
        Ok(Some(claimed))
    }
}

fn claimed_from_row(
    row: &QueryResult,
    expected_owner: &DispatchLeaseOwner,
    claimed_at_ms: i64,
    lease_expires_at_ms: i64,
) -> Result<ClaimedMessageDispatch, RepositoryError> {
    let message_id = MessageId::parse(row_value::<String, _>(
        row,
        0,
        "message_id",
        "message_dispatches",
    )?)
    .map_err(|error| corrupt_data("message_dispatches", "message_id", error))?;
    let correlation_id = RequestId::parse(row_value::<String, _>(
        row,
        1,
        "correlation_id",
        "message_dispatches",
    )?)
    .map_err(|error| corrupt_data("message_dispatches", "correlation_id", error))?;
    let attempt_count = row_value::<i64, _>(row, 2, "attempt_count", "message_dispatches")?;
    let attempt_count = u32::try_from(attempt_count)
        .map_err(|error| corrupt_data("message_dispatches", "attempt_count", error))?;
    if attempt_count == 0 {
        return Err(RepositoryError::Invariant {
            reason: "claimed dispatch retained a zero attempt count",
        });
    }
    let queued_at =
        UnixMillis::from_millis(row_value(row, 3, "queued_at_ms", "message_dispatches")?);
    let available_at =
        UnixMillis::from_millis(row_value(row, 4, "available_at_ms", "message_dispatches")?);
    let owner = DispatchLeaseOwner::from_storage(&row_value::<String, _>(
        row,
        5,
        "lease_owner",
        "message_dispatches",
    )?)
    .map_err(|error| corrupt_data("message_dispatches", "lease_owner", error))?;
    if !owner.constant_time_eq(expected_owner) {
        return Err(RepositoryError::Invariant {
            reason: "claimed dispatch returned a different lease owner",
        });
    }
    let returned_expiry = row_value::<i64, _>(row, 6, "lease_expires_at_ms", "message_dispatches")?;
    let returned_update = row_value::<i64, _>(row, 7, "updated_at_ms", "message_dispatches")?;
    if returned_expiry != lease_expires_at_ms || returned_update != claimed_at_ms {
        return Err(RepositoryError::Invariant {
            reason: "claimed dispatch returned inconsistent lease timestamps",
        });
    }

    Ok(ClaimedMessageDispatch {
        message_id,
        correlation_id,
        attempt_count,
        queued_at,
        available_at,
        owner,
        lease_expires_at: UnixMillis::from_millis(returned_expiry),
        updated_at: UnixMillis::from_millis(returned_update),
    })
}

async fn classify_unclaimed(
    database: &impl ConnectionTrait,
    claimed_at: UnixMillis,
) -> Result<(), RepositoryError> {
    let claimed_at_ms = millis(claimed_at);
    let claimable = entities::message_dispatch::Entity::find()
        .filter(eligible_dispatch_condition(claimed_at_ms))
        .filter(entities::message_dispatch::Column::AttemptCount.lt(MAX_DISPATCH_ATTEMPTS))
        .one(database)
        .await
        .map_err(|source| database_error("classify claimable message dispatch", source))?;
    if claimable.is_some() {
        return Err(RepositoryError::Invariant {
            reason: "eligible dispatch was not changed by its atomic claim statement",
        });
    }

    let exhausted = entities::message_dispatch::Entity::find()
        .filter(eligible_dispatch_condition(claimed_at_ms))
        .filter(entities::message_dispatch::Column::AttemptCount.eq(MAX_DISPATCH_ATTEMPTS))
        .order_by_asc(entities::message_dispatch::Column::AvailableAtMs)
        .order_by_asc(entities::message_dispatch::Column::QueuedAtMs)
        .order_by_asc(entities::message_dispatch::Column::MessageId)
        .one(database)
        .await
        .map_err(|source| database_error("classify exhausted message dispatch", source))?;

    let Some(exhausted) = exhausted else {
        return Ok(());
    };
    let message_id = MessageId::parse(exhausted.message_id)
        .map_err(|error| corrupt_data("message_dispatches", "message_id", error))?;
    Err(RepositoryError::DispatchAttemptLimit { message_id })
}

/// Steered rows never enter the claim path: request-side routing owns
/// them from accept to completion or typed failure, so both the raw
/// claim statement and this eligibility diagnosis exclude them. Without
/// the exclusion, a claim that correctly returns nothing would trip the
/// invariant below on a steered row it must not touch.
fn eligible_dispatch_condition(claimed_at_ms: i64) -> Condition {
    Condition::all()
        .add(sea_orm::sea_query::Expr::cust_with_values(
            "NOT EXISTS (SELECT 1 FROM messages candidate JOIN messages sibling ON sibling.thread_id = candidate.thread_id JOIN message_dispatches busy ON busy.message_id = sibling.message_id WHERE candidate.message_id = message_dispatches.message_id AND busy.message_id != message_dispatches.message_id AND busy.state IN ('leased', 'running') AND busy.lease_expires_at_ms > ?)",
            [claimed_at_ms],
        ))
        .add(entities::message_dispatch::Column::SteerRunId.is_null())
        .add(
            Condition::any()
                .add(
                    Condition::all()
                        .add(entities::message_dispatch::Column::State.eq(DispatchState::Queued))
                        .add(entities::message_dispatch::Column::AvailableAtMs.lte(claimed_at_ms)),
                )
                .add(
                    Condition::all()
                        .add(entities::message_dispatch::Column::State.eq(DispatchState::Leased))
                        .add(
                            entities::message_dispatch::Column::LeaseExpiresAtMs.lte(claimed_at_ms),
                        ),
                ),
        )
}

async fn finish_unclaimed(
    transaction: sea_orm::DatabaseTransaction,
    result: Result<(), RepositoryError>,
) -> Result<Option<ClaimedMessageDispatch>, RepositoryError> {
    match result {
        Ok(()) => {
            transaction
                .commit()
                .await
                .map_err(|source| database_error("finish empty message-dispatch claim", source))?;
            Ok(None)
        }
        Err(error) => rollback_claim(transaction, error).await,
    }
}

async fn rollback_claim<T>(
    transaction: sea_orm::DatabaseTransaction,
    error: RepositoryError,
) -> Result<T, RepositoryError> {
    transaction
        .rollback()
        .await
        .map_err(|source| database_error("roll back message-dispatch claim", source))?;
    Err(error)
}
