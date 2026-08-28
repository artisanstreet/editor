//! Atomic claim and recovery of durable message dispatches.

use artisan_domain::{MessageId, RequestId, UnixMillis};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, QueryOrder,
    QueryResult, Statement, TransactionTrait,
};
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::Zeroize;

use crate::entities::{self, DispatchState};

use super::{Repository, RepositoryError, corrupt_data, database_error, millis};

const OWNER_BYTES: usize = 32;
const OWNER_STORAGE_BYTES: usize = OWNER_BYTES * 2;

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
    WHERE (state = 'queued' AND available_at_ms <= ?)
       OR (state = 'leased' AND lease_expires_at_ms <= ?)
    ORDER BY available_at_ms ASC, queued_at_ms ASC, message_id ASC
    LIMIT 1
)
  AND ((state = 'queued' AND available_at_ms <= ?)
    OR (state = 'leased' AND lease_expires_at_ms <= ?))
  AND attempt_count < ?
RETURNING message_id,
          correlation_id,
          attempt_count,
          queued_at_ms,
          available_at_ms,
          lease_owner,
          lease_expires_at_ms,
          updated_at_ms
";

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

/// Maximum UTF-8 byte length accepted for one dispatch failure reason.
const FAILURE_REASON_MAX_BYTES: usize = 4096;

/// Failure to accept one dispatch failure reason.
///
/// Variants deliberately carry only lengths and bounds so raw failure text
/// cannot leak through repository error rendering.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum DispatchFailureReasonError {
    /// The supplied reason was empty.
    #[error("dispatch failure reason must not be empty")]
    Empty,

    /// The supplied reason exceeded its UTF-8 byte ceiling.
    #[error("dispatch failure reason is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// The documented ceiling ([`DispatchFailureReason::MAX_BYTES`]).
        maximum: usize,
    },
}

/// Bounded failure explanation persisted with one failed or requeued dispatch.
///
/// The value is operator-facing diagnostic text rather than a credential, but
/// its contents stay out of repository error and log rendering; [`Debug`]
/// reports only the byte length and callers must opt into [`Self::as_str`].
#[derive(Clone, Eq, PartialEq)]
pub struct DispatchFailureReason(String);

impl DispatchFailureReason {
    /// Maximum UTF-8 byte length accepted for this value.
    pub const MAX_BYTES: usize = FAILURE_REASON_MAX_BYTES;

    /// Creates the reason after validating the external text without any
    /// truncation.
    ///
    /// # Errors
    ///
    /// Returns [`DispatchFailureReasonError::Empty`] for empty text or
    /// [`DispatchFailureReasonError::TooLong`] carrying only the offending
    /// length when the text exceeds `MAX_BYTES` UTF-8 bytes.
    pub fn parse(value: impl Into<String>) -> Result<Self, DispatchFailureReasonError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DispatchFailureReasonError::Empty);
        }
        let length = value.len();
        if length > Self::MAX_BYTES {
            return Err(DispatchFailureReasonError::TooLong {
                length,
                maximum: Self::MAX_BYTES,
            });
        }
        Ok(Self(value))
    }

    /// Returns the validated failure reason exactly as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for DispatchFailureReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DispatchFailureReason")
            .field("length_bytes", &self.0.len())
            .finish()
    }
}

/// Failure to decode a persisted dispatch owner.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum DispatchLeaseOwnerError {
    /// The encoded token was not exactly 32 bytes represented as lowercase hex.
    #[error("dispatch owner encoding is {actual} bytes; expected {expected}")]
    InvalidLength { actual: usize, expected: usize },

    /// The encoded token contained a byte outside lowercase hexadecimal.
    #[error("dispatch owner encoding contains a non-lowercase-hex byte at index {index}")]
    InvalidCharacter { index: usize },
}

/// Exact 32-byte ownership token for one dispatch lease.
///
/// The type deliberately implements neither formatting nor cloning traits,
/// and its bytes are cleared when the value is dropped.
pub struct DispatchLeaseOwner([u8; OWNER_BYTES]);

impl DispatchLeaseOwner {
    /// Creates an owner token from exactly 32 caller-generated bytes.
    #[must_use]
    pub const fn new(bytes: [u8; OWNER_BYTES]) -> Self {
        Self(bytes)
    }

    /// Compares two ownership tokens without data-dependent early exit.
    #[must_use]
    pub fn constant_time_eq(&self, other: &Self) -> bool {
        bool::from(self.0.ct_eq(&other.0))
    }

    fn to_storage(&self) -> String {
        let mut encoded = String::with_capacity(OWNER_STORAGE_BYTES);
        for &byte in &self.0 {
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
        encoded
    }

    fn from_storage(encoded: &str) -> Result<Self, DispatchLeaseOwnerError> {
        if encoded.len() != OWNER_STORAGE_BYTES {
            return Err(DispatchLeaseOwnerError::InvalidLength {
                actual: encoded.len(),
                expected: OWNER_STORAGE_BYTES,
            });
        }

        let encoded = encoded.as_bytes();
        let mut bytes = [0_u8; OWNER_BYTES];
        let (pairs, remainder) = encoded.as_chunks::<2>();
        debug_assert!(remainder.is_empty());
        for (index, pair) in pairs.iter().enumerate() {
            let high = decode_nibble(pair[0], index * 2)?;
            let low = decode_nibble(pair[1], index * 2 + 1)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

impl Drop for DispatchLeaseOwner {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// One finite attempt to claim the next eligible dispatch.
pub struct ClaimMessageDispatch {
    pub owner: DispatchLeaseOwner,
    pub claimed_at: UnixMillis,
    pub lease_expires_at: UnixMillis,
}

/// A dispatch durably owned until its lease expiry.
pub struct ClaimedMessageDispatch {
    pub message_id: MessageId,
    pub correlation_id: RequestId,
    pub attempt_count: u32,
    pub queued_at: UnixMillis,
    pub available_at: UnixMillis,
    pub owner: DispatchLeaseOwner,
    pub lease_expires_at: UnixMillis,
    pub updated_at: UnixMillis,
}

/// Owner-fenced completion of one claimed dispatch.
pub struct CompleteMessageDispatch {
    /// The dispatch to complete.
    pub message_id: MessageId,
    /// The exact lease holder recorded by the live claim.
    pub owner: DispatchLeaseOwner,
    /// Operation time fencing the lease; the lease must outlive it.
    pub operated_at: UnixMillis,
}

/// Owner-fenced terminal failure of one claimed dispatch.
pub struct FailMessageDispatch {
    /// The dispatch to fail terminally.
    pub message_id: MessageId,
    /// The exact lease holder recorded by the live claim.
    pub owner: DispatchLeaseOwner,
    /// Operation time fencing the lease; the lease must outlive it.
    pub operated_at: UnixMillis,
    /// Bounded failure reason persisted verbatim with the row.
    pub reason: DispatchFailureReason,
}

/// Owner-fenced retryable requeue of one claimed dispatch.
pub struct RequeueMessageDispatch {
    /// The dispatch to requeue for retry.
    pub message_id: MessageId,
    /// The exact lease holder recorded by the live claim.
    pub owner: DispatchLeaseOwner,
    /// Operation time fencing the lease; the lease must outlive it.
    pub operated_at: UnixMillis,
    /// Absolute availability instant supplied by the caller. Every signed
    /// value is valid, including equality or earlier times for immediate
    /// retry.
    pub available_at: UnixMillis,
    /// Bounded failure reason persisted verbatim with the row.
    pub reason: DispatchFailureReason,
}

/// One durable lifecycle transition acknowledged by the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionedMessageDispatch {
    pub message_id: MessageId,
    pub updated_at: UnixMillis,
}

impl Repository {
    /// Atomically claims the oldest eligible queued or expired dispatch.
    ///
    /// The transaction's first statement both selects and updates one row.
    /// SQLite therefore serializes competing writers before either can
    /// observe a claimable result. A leased row becomes eligible again only
    /// when its persisted expiry is at or before `claimed_at`.
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
            .database
            .begin()
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
                claimed_at_ms.into(),
                claimed_at_ms.into(),
                i64::from(i32::MAX).into(),
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
            .database
            .begin()
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
        let transaction =
            self.database.begin().await.map_err(|source| {
                database_error("begin message-dispatch terminal failure", source)
            })?;
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
            .database
            .begin()
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

fn claimed_from_row(
    row: &QueryResult,
    expected_owner: &DispatchLeaseOwner,
    claimed_at_ms: i64,
    lease_expires_at_ms: i64,
) -> Result<ClaimedMessageDispatch, RepositoryError> {
    let message_id = MessageId::parse(row_value::<String>(row, 0, "message_id")?)
        .map_err(|error| corrupt_data("message_dispatches", "message_id", &error))?;
    let correlation_id = RequestId::parse(row_value::<String>(row, 1, "correlation_id")?)
        .map_err(|error| corrupt_data("message_dispatches", "correlation_id", &error))?;
    let attempt_count = row_value::<i64>(row, 2, "attempt_count")?;
    let attempt_count = u32::try_from(attempt_count)
        .map_err(|error| corrupt_data("message_dispatches", "attempt_count", &error))?;
    if attempt_count == 0 {
        return Err(RepositoryError::Invariant {
            reason: "claimed dispatch retained a zero attempt count",
        });
    }
    let queued_at = UnixMillis::from_millis(row_value(row, 3, "queued_at_ms")?);
    let available_at = UnixMillis::from_millis(row_value(row, 4, "available_at_ms")?);
    let owner = DispatchLeaseOwner::from_storage(&row_value::<String>(row, 5, "lease_owner")?)
        .map_err(|error| corrupt_data("message_dispatches", "lease_owner", &error))?;
    if !owner.constant_time_eq(expected_owner) {
        return Err(RepositoryError::Invariant {
            reason: "claimed dispatch returned a different lease owner",
        });
    }
    let returned_expiry = row_value::<i64>(row, 6, "lease_expires_at_ms")?;
    let returned_update = row_value::<i64>(row, 7, "updated_at_ms")?;
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
    let eligible = Condition::any()
        .add(
            Condition::all()
                .add(entities::message_dispatch::Column::State.eq(DispatchState::Queued))
                .add(entities::message_dispatch::Column::AvailableAtMs.lte(claimed_at_ms)),
        )
        .add(
            Condition::all()
                .add(entities::message_dispatch::Column::State.eq(DispatchState::Leased))
                .add(entities::message_dispatch::Column::LeaseExpiresAtMs.lte(claimed_at_ms)),
        );
    let candidate = entities::message_dispatch::Entity::find()
        .filter(eligible)
        .order_by_asc(entities::message_dispatch::Column::AvailableAtMs)
        .order_by_asc(entities::message_dispatch::Column::QueuedAtMs)
        .order_by_asc(entities::message_dispatch::Column::MessageId)
        .one(database)
        .await
        .map_err(|source| database_error("classify unclaimed message dispatch", source))?;

    let Some(candidate) = candidate else {
        return Ok(());
    };
    let message_id = MessageId::parse(candidate.message_id)
        .map_err(|error| corrupt_data("message_dispatches", "message_id", &error))?;
    if candidate.attempt_count == i32::MAX {
        return Err(RepositoryError::DispatchAttemptLimit { message_id });
    }
    Err(RepositoryError::Invariant {
        reason: "eligible dispatch was not changed by its atomic claim statement",
    })
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

fn row_value<T>(row: &QueryResult, index: usize, field: &'static str) -> Result<T, RepositoryError>
where
    T: sea_orm::TryGetable,
{
    row.try_get_by_index(index)
        .map_err(|error| corrupt_data("message_dispatches", field, &error))
}

const fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'a' + nibble - 10) as char,
        _ => unreachable!(),
    }
}

fn decode_nibble(byte: u8, index: usize) -> Result<u8, DispatchLeaseOwnerError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(DispatchLeaseOwnerError::InvalidCharacter { index }),
    }
}

/// Validates one RETURNING row of a fenced lifecycle transition.
fn transitioned_from_row(
    row: &QueryResult,
    message_id: &MessageId,
    operated_at_ms: i64,
    expected_available_at_ms: Option<i64>,
) -> Result<TransitionedMessageDispatch, RepositoryError> {
    let returned_id = row_value::<String>(row, 0, "message_id")?;
    if returned_id != message_id.as_str() {
        return Err(RepositoryError::Invariant {
            reason: "fenced transition returned a different message id",
        });
    }
    let attempt_count = row_value::<i64>(row, 1, "attempt_count")?;
    let attempt_count = u32::try_from(attempt_count)
        .map_err(|error| corrupt_data("message_dispatches", "attempt_count", &error))?;
    if attempt_count == 0 {
        return Err(RepositoryError::Invariant {
            reason: "transitioned dispatch retained a zero attempt count",
        });
    }
    let available_at_ms = row_value::<i64>(row, 2, "available_at_ms")?;
    if let Some(expected) = expected_available_at_ms
        && available_at_ms != expected
    {
        return Err(RepositoryError::Invariant {
            reason: "requeued dispatch returned inconsistent availability",
        });
    }
    let lease_expires_at_ms = row_value::<Option<i64>>(row, 3, "lease_expires_at_ms")?;
    if lease_expires_at_ms.is_some() {
        return Err(RepositoryError::Invariant {
            reason: "transitioned dispatch retained lease metadata",
        });
    }
    let updated_at_ms = row_value::<i64>(row, 4, "updated_at_ms")?;
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
            return corrupt_data("message_dispatches", "lease_owner", &error);
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

async fn rollback_transition<T>(
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

const fn state_label(state: &DispatchState) -> &'static str {
    match state {
        DispatchState::Queued => "queued",
        DispatchState::Leased => "leased",
        DispatchState::Running => "running",
        DispatchState::Completed => "completed",
        DispatchState::Failed => "failed",
    }
}
