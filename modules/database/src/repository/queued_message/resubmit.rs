//! Forge-owned resubmission of failed messages and the thread outbox
//! fingerprint that decides when subscribers receive a fresh outbox.
//!
//! A retry requeues the stored dispatch; a recovery is recorded after the
//! Forge moved the payload into a new thread. Neither reads or copies the
//! payload here: the immutable message, image, and receipt rows stay the
//! only copy.

#![forbid(unsafe_code)]

use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};

use artisan_domain::{
    FailedMessageRetryOutcome, FailedMessageTarget, MessageId, RequestId, ThreadId, UnixMillis,
};

use crate::repository::{Repository, corrupt_data, database_error, row_value};

use super::QueuedMessageRepositoryError;
use super::rows::{ensure_thread, parse_request_id, parse_thread_id};

/// Moves one retryable failure back to `queued`. A retried steer becomes an
/// ordinary queued message: the run it named has ended.
const RETRY_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET state = 'queued',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = NULL,
    steer_run_id = NULL,
    available_at_ms = ?,
    updated_at_ms = MAX(updated_at_ms, ?)
WHERE message_id = ?
  AND correlation_id = ?
  AND state = 'failed'
  AND last_error IS NOT NULL
  AND EXISTS (
      SELECT 1 FROM messages AS m
      WHERE m.message_id = message_dispatches.message_id AND m.thread_id = ?
  )
  AND NOT EXISTS (
      SELECT 1 FROM queued_message_withdrawals AS w
      WHERE w.message_id = message_dispatches.message_id AND w.outcome = 'withdrawn'
  )
  AND NOT EXISTS (
      SELECT 1 FROM failed_message_recoveries AS fr
      WHERE fr.message_id = message_dispatches.message_id
  )
  AND NOT EXISTS (
      SELECT 1 FROM conversation_items AS i
      WHERE i.source_message_id = message_dispatches.message_id
  )
";

const RECOVERY_BY_MESSAGE_SQL: &str = r"
SELECT request_id, new_thread_id FROM failed_message_recoveries WHERE message_id = ?
";

const INSERT_RECOVERY_SQL: &str = r"
INSERT OR IGNORE INTO failed_message_recoveries (
    message_id, request_id, thread_id, new_thread_id, recovered_at_ms
) VALUES (?, ?, ?, ?, ?)
";

/// Cheap aggregate over every dispatch of a thread plus its recoveries and
/// withdrawals. Any transition that changes the outbox changes at least one
/// component (state, attempts, update time, or a row count).
const OUTBOX_FINGERPRINT_SQL: &str = r"
SELECT COUNT(*),
       COALESCE(MAX(d.updated_at_ms), 0),
       COALESCE(SUM(d.updated_at_ms % 1000003), 0),
       COALESCE(SUM(d.attempt_count), 0),
       COALESCE(SUM(CASE d.state
           WHEN 'queued' THEN 1 WHEN 'leased' THEN 3 WHEN 'running' THEN 9
           WHEN 'completed' THEN 27 ELSE 81 END), 0),
       (SELECT COUNT(*) FROM failed_message_recoveries AS fr WHERE fr.thread_id = ?),
       (SELECT COUNT(*) FROM queued_message_withdrawals AS w WHERE w.thread_id = ?)
FROM message_dispatches AS d
JOIN messages AS m ON m.message_id = d.message_id
WHERE m.thread_id = ?
";

/// Opaque summary of a thread's outbox-relevant rows; equal fingerprints
/// mean the outbox has not changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MessageOutboxFingerprint([i64; 7]);

/// A failed message the Forge already moved into a new thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailedMessageRecovery {
    /// Request identity of the recovery that moved it.
    pub request_id: RequestId,
    /// The thread that received the prompt.
    pub new_thread_id: ThreadId,
}

impl Repository {
    /// Requeues one failed message that never reached the transcript, so the
    /// dispatcher delivers its stored payload again.
    ///
    /// Idempotent by state: anything but a retryable failure of exactly this
    /// thread, message, and original request answers
    /// [`FailedMessageRetryOutcome::NotRetryable`] without a change.
    ///
    /// # Errors
    ///
    /// Returns a missing-thread or database error.
    pub async fn retry_failed_message(
        &self,
        target: &FailedMessageTarget,
        retried_at: UnixMillis,
    ) -> Result<FailedMessageRetryOutcome, QueuedMessageRepositoryError> {
        ensure_thread(&self.database, &target.thread_id).await?;
        let transaction = self
            .begin_write()
            .await
            .map_err(|source| database_error("begin failed-message retry", source))?;
        let now = retried_at.as_millis();
        let updated = transaction
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                RETRY_DISPATCH_SQL,
                [
                    Value::BigInt(Some(now)),
                    Value::BigInt(Some(now)),
                    Value::String(Some(target.message_id.as_str().to_owned())),
                    Value::String(Some(target.original_request_id.as_str().to_owned())),
                    Value::String(Some(target.thread_id.as_str().to_owned())),
                ],
            ))
            .await
            .map_err(|source| database_error("requeue failed message", source))?
            .rows_affected();
        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit failed-message retry", source))?;
        Ok(if updated == 1 {
            FailedMessageRetryOutcome::Requeued
        } else {
            FailedMessageRetryOutcome::NotRetryable
        })
    }

    /// The recovery that already moved one failed message, if any.
    ///
    /// # Errors
    ///
    /// Returns a corruption or database error.
    pub async fn failed_message_recovery(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<FailedMessageRecovery>, QueuedMessageRepositoryError> {
        let row = self
            .database
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                RECOVERY_BY_MESSAGE_SQL,
                [Value::String(Some(message_id.as_str().to_owned()))],
            ))
            .await
            .map_err(|source| database_error("read failed-message recovery", source))?;
        row.map(|row| {
            Ok(FailedMessageRecovery {
                request_id: parse_request_id(row_value(
                    &row,
                    0,
                    "request_id",
                    "failed_message_recoveries",
                )?)?,
                new_thread_id: parse_thread_id(row_value(
                    &row,
                    1,
                    "new_thread_id",
                    "failed_message_recoveries",
                )?)?,
            })
        })
        .transpose()
    }

    /// Records that a failed message moved into `new_thread_id`, which stops
    /// the failed listing from offering it. Recording the same message again
    /// keeps the first recovery.
    ///
    /// # Errors
    ///
    /// Returns a database error.
    pub async fn record_failed_message_recovery(
        &self,
        request_id: &RequestId,
        target: &FailedMessageTarget,
        new_thread_id: &ThreadId,
        recovered_at: UnixMillis,
    ) -> Result<(), QueuedMessageRepositoryError> {
        self.database
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                INSERT_RECOVERY_SQL,
                [
                    Value::String(Some(target.message_id.as_str().to_owned())),
                    Value::String(Some(request_id.as_str().to_owned())),
                    Value::String(Some(target.thread_id.as_str().to_owned())),
                    Value::String(Some(new_thread_id.as_str().to_owned())),
                    Value::BigInt(Some(recovered_at.as_millis())),
                ],
            ))
            .await
            .map(|_| ())
            .map_err(|source| database_error("record failed-message recovery", source))
    }

    /// Summarizes every outbox-relevant row of one thread, so delivery reads
    /// and pushes the outbox only when it changed.
    ///
    /// # Errors
    ///
    /// Returns a corruption or database error.
    pub async fn message_outbox_fingerprint(
        &self,
        thread_id: &ThreadId,
    ) -> Result<MessageOutboxFingerprint, QueuedMessageRepositoryError> {
        let thread = || Value::String(Some(thread_id.as_str().to_owned()));
        let row = self
            .database
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                OUTBOX_FINGERPRINT_SQL,
                [thread(), thread(), thread()],
            ))
            .await
            .map_err(|source| database_error("read message outbox fingerprint", source))?
            .ok_or_else(|| {
                corrupt_data(
                    "message_dispatches",
                    "count",
                    "outbox fingerprint returned no row",
                )
            })?;
        let mut parts = [0_i64; 7];
        for (index, part) in parts.iter_mut().enumerate() {
            *part = row_value(&row, index, "fingerprint", "message_dispatches")?;
        }
        Ok(MessageOutboxFingerprint(parts))
    }
}
