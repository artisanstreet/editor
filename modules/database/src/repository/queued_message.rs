//! Bounded queued-message reads and atomic edit/discard withdrawal.
//!
//! This module intentionally does not add a second message store. Immutable
//! message, image, and original queue-receipt rows remain the source of truth.
//! A withdrawal marks the exact never-claimed dispatch row `failed`, which is
//! already excluded by the existing claim SQL, and records the command result
//! in `queued_message_withdrawals`. The dispatch state update and receipt
//! insert happen in one SQLite `BEGIN IMMEDIATE` transaction.

#![allow(
    clippy::module_name_repetitions,
    reason = "public repository values retain their queued-message context at crate boundaries"
)]

use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QueryResult, QuerySelect, SqliteTransactionMode, Statement, TransactionOptions,
    TransactionTrait, TryGetable, Value,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use artisan_domain::{
    AuthoredText, CommandReceipt, ImageAttachment, ImageAttachmentRef, ListQueuedMessages,
    MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT, MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES, MessageId,
    QUEUED_MESSAGE_LIST_MAX, QueueMessagePayload, QueuedMessageListError, QueuedMessageListOrder,
    QueuedMessageListing, QueuedMessageListingError, QueuedMessageSummary,
    QueuedMessageWithdrawalOutcome, ReceiptDisposition, RequestId, ThreadId, UnixMillis,
    WithdrawQueuedMessage, WithdrawQueuedMessageResult,
};

use crate::entities::{self, CommandKind, DispatchState};

use super::Repository;

const LIST_COUNT_SQL: &str = r"
SELECT COUNT(*)
FROM messages AS m
JOIN message_dispatches AS d ON d.message_id = m.message_id
JOIN command_receipts AS r ON r.request_id = d.correlation_id
WHERE m.thread_id = ?
  AND r.command_kind = 'queue_message'
  AND r.thread_id = m.thread_id
  AND r.message_id = m.message_id
  AND d.state = 'queued'
  AND d.attempt_count = 0
  AND d.lease_owner IS NULL
  AND d.lease_expires_at_ms IS NULL
  AND NOT EXISTS (
      SELECT 1
      FROM queued_message_withdrawals AS w
      WHERE w.thread_id = m.thread_id
        AND w.message_id = d.message_id
        AND w.original_request_id = d.correlation_id
        AND w.outcome = 'withdrawn'
  )
";

const LIST_OLDEST_SQL: &str = r"
SELECT m.message_id,
       m.thread_id,
       m.body,
       d.correlation_id,
       r.body,
       m.accepted_at_ms,
       d.queued_at_ms,
       r.accepted_at_ms
FROM messages AS m
JOIN message_dispatches AS d ON d.message_id = m.message_id
JOIN command_receipts AS r ON r.request_id = d.correlation_id
WHERE m.thread_id = ?
  AND r.command_kind = 'queue_message'
  AND r.thread_id = m.thread_id
  AND r.message_id = m.message_id
  AND d.state = 'queued'
  AND d.attempt_count = 0
  AND d.lease_owner IS NULL
  AND d.lease_expires_at_ms IS NULL
  AND NOT EXISTS (
       SELECT 1
       FROM queued_message_withdrawals AS w
       WHERE w.thread_id = m.thread_id
         AND w.message_id = d.message_id
         AND w.original_request_id = d.correlation_id
         AND w.outcome = 'withdrawn'
  )
ORDER BY d.queued_at_ms ASC, d.message_id ASC
LIMIT ?
";

const LIST_LATEST_SQL: &str = r"
SELECT m.message_id,
       m.thread_id,
       m.body,
       d.correlation_id,
       r.body,
       m.accepted_at_ms,
       d.queued_at_ms,
       r.accepted_at_ms
FROM messages AS m
JOIN message_dispatches AS d ON d.message_id = m.message_id
JOIN command_receipts AS r ON r.request_id = d.correlation_id
WHERE m.thread_id = ?
  AND r.command_kind = 'queue_message'
  AND r.thread_id = m.thread_id
  AND r.message_id = m.message_id
  AND d.state = 'queued'
  AND d.attempt_count = 0
  AND d.lease_owner IS NULL
  AND d.lease_expires_at_ms IS NULL
  AND NOT EXISTS (
       SELECT 1
       FROM queued_message_withdrawals AS w
       WHERE w.thread_id = m.thread_id
         AND w.message_id = d.message_id
         AND w.original_request_id = d.correlation_id
         AND w.outcome = 'withdrawn'
  )
ORDER BY d.queued_at_ms DESC, d.message_id DESC
LIMIT ?
";

const WITHDRAWAL_RECEIPT_SQL: &str = r"
SELECT thread_id, message_id, original_request_id, outcome, accepted_at_ms
FROM queued_message_withdrawals
WHERE withdrawal_request_id = ?
";

const WITHDRAWN_TARGET_SQL: &str = r"
SELECT 1
FROM queued_message_withdrawals
WHERE thread_id = ?
  AND message_id = ?
  AND original_request_id = ?
  AND outcome = 'withdrawn'
LIMIT 1
";

const WITHDRAW_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET state = 'failed',
    lease_owner = NULL,
    lease_expires_at_ms = NULL,
    last_error = NULL,
    updated_at_ms = ?
WHERE message_id = ?
  AND correlation_id = ?
  AND state = 'queued'
  AND attempt_count = 0
  AND lease_owner IS NULL
  AND lease_expires_at_ms IS NULL
  AND updated_at_ms <= ?
";

const INSERT_WITHDRAWAL_SQL: &str = r"
INSERT OR IGNORE INTO queued_message_withdrawals (
    withdrawal_request_id,
    thread_id,
    message_id,
    original_request_id,
    outcome,
    accepted_at_ms
) VALUES (?, ?, ?, ?, ?, ?)
";

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
        /// Original SeaORM error.
        #[source]
        source: DbErr,
    },
}

impl Repository {
    /// Reads one bounded page of still-queued, never-claimed general
    /// messages for exactly one existing thread.
    ///
    /// Rows are ordered by `(queued_at_ms, message_id)` in the requested
    /// direction. Only `queue_message` receipts paired with a `queued`
    /// dispatch whose attempt count is zero are eligible; leased, running,
    /// completed, failed, requeued, and withdrawn rows are excluded. The
    /// result contains authored text and byte-free image references, never
    /// image bytes. `total_count` and `has_more` are derived from the same
    /// eligibility predicate as the page query in one read transaction.
    ///
    /// # Errors
    ///
    /// Returns a typed missing-thread, limit, corruption, or database error.
    pub async fn read_queued_messages(
        &self,
        query: ListQueuedMessages,
    ) -> Result<QueuedMessageListing, QueuedMessageRepositoryError> {
        validate_limit(query.limit)?;
        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin queued-message listing", source))?;
        if let Err(error) = ensure_thread(&transaction, &query.thread_id).await {
            transaction
                .rollback()
                .await
                .map_err(|source| database_error("rollback queued-message listing", source))?;
            return Err(error);
        }

        let result = Self::read_queued_message_page(&transaction, &query).await;
        match result {
            Ok(listing) => {
                transaction
                    .commit()
                    .await
                    .map_err(|source| database_error("commit queued-message listing", source))?;
                Ok(listing)
            }
            Err(error) => {
                transaction
                    .rollback()
                    .await
                    .map_err(|source| database_error("rollback queued-message listing", source))?;
                Err(error)
            }
        }
    }

    /// Reads the count and page from one consistent SQLite read transaction.
    async fn read_queued_message_page(
        database: &impl ConnectionTrait,
        query: &ListQueuedMessages,
    ) -> Result<QueuedMessageListing, QueuedMessageRepositoryError> {
        let total_count = read_eligible_count(database, &query.thread_id).await?;
        let limit = sqlite_limit(query.limit)?;
        let sql = match query.order {
            QueuedMessageListOrder::OldestFirst => LIST_OLDEST_SQL,
            QueuedMessageListOrder::LatestFirst => LIST_LATEST_SQL,
        };
        let rows = database
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                sql,
                [
                    Value::String(Some(query.thread_id.as_str().to_owned())),
                    Value::BigInt(Some(limit)),
                ],
            ))
            .await
            .map_err(|source| database_error("read queued-message page", source))?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            messages.push(summary_from_row(database, &query.thread_id, &row).await?);
        }

        QueuedMessageListing::new(
            query.thread_id.clone(),
            query.order,
            query.limit,
            total_count,
            messages,
        )
        .map_err(QueuedMessageRepositoryError::InvalidListing)
    }

    /// Alias with the verb used by callers that treat the result as a list.
    ///
    /// # Errors
    ///
    /// Returns the same bounded-list, ownership, corruption, and database
    /// errors as [`Self::read_queued_messages`].
    pub async fn list_queued_messages(
        &self,
        query: ListQueuedMessages,
    ) -> Result<QueuedMessageListing, QueuedMessageRepositoryError> {
        self.read_queued_messages(query).await
    }

    /// Atomically withdraws one exact, never-claimed queued message.
    ///
    /// The transaction starts with SQLite `BEGIN IMMEDIATE`, so it takes the
    /// writer fence before examining the withdrawal receipt or dispatch row.
    /// The only successful dispatch mutation matches the original message,
    /// original queue request, `queued` state, zero attempts, null lease
    /// metadata, and a non-backwards update time. The existing claim SQL
    /// already excludes `failed`, so the unchanged worker claim path cannot
    /// resurrect a withdrawn message. No message, image, original receipt,
    /// conversation history, run, or leased/launched dispatch is deleted or
    /// cancelled.
    ///
    /// Every valid command outcome is durably recorded. An exact retry returns
    /// the original outcome with `ReceiptDisposition::Duplicate`; reusing the
    /// withdrawal request id with any changed field returns an idempotency
    /// conflict.
    ///
    /// # Errors
    ///
    /// Returns a typed identity, chronology, corruption, or database error.
    /// `TooLate` and `NotQueued` are ordinary durable outcomes, not errors.
    pub async fn withdraw_queued_message(
        &self,
        input: WithdrawQueuedMessage,
    ) -> Result<WithdrawQueuedMessageResult, QueuedMessageRepositoryError> {
        let transaction = self
            .database
            .begin_with_options(TransactionOptions {
                sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
                ..Default::default()
            })
            .await
            .map_err(|source| database_error("begin queued-message withdrawal", source))?;

        match apply_withdrawal(&transaction, &input).await {
            Ok(WithdrawalTransactionOutcome::Accepted(result)) => {
                transaction
                    .commit()
                    .await
                    .map_err(|source| database_error("commit queued-message withdrawal", source))?;
                Ok(result)
            }
            Ok(WithdrawalTransactionOutcome::Duplicate(result)) => {
                transaction.rollback().await.map_err(|source| {
                    database_error("rollback duplicate queued-message withdrawal", source)
                })?;
                Ok(result)
            }
            Err(error) => {
                transaction.rollback().await.map_err(|source| {
                    database_error("rollback queued-message withdrawal", source)
                })?;
                Err(error)
            }
        }
    }

    /// Reads the full typed payload of a successfully withdrawn message.
    ///
    /// The read is ownership checked against the supplied thread, message id,
    /// and original queue request id, and requires a durable `withdrawn`
    /// receipt. It is intentionally separate from [`Self::read_queued_messages`]
    /// so normal composer listing never carries image bytes. Wrong-thread,
    /// missing, mismatched, or not-withdrawn targets return `None`.
    pub async fn read_withdrawn_message_payload(
        &self,
        thread_id: &ThreadId,
        message_id: &MessageId,
        original_request_id: &RequestId,
    ) -> Result<Option<QueueMessagePayload>, QueuedMessageRepositoryError> {
        ensure_thread(&self.database, thread_id).await?;
        let Some(message) = entities::message::Entity::find_by_id(message_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| database_error("read withdrawn message", source))?
        else {
            return Ok(None);
        };
        if message.thread_id != thread_id.as_str() {
            return Ok(None);
        }

        let Some(receipt) =
            entities::command_receipt::Entity::find_by_id(original_request_id.as_str())
                .one(&self.database)
                .await
                .map_err(|source| database_error("read original queue receipt", source))?
        else {
            return Ok(None);
        };
        if !queue_receipt_owns_message(&receipt, thread_id, message_id) {
            return Ok(None);
        }
        if message.accepted_at_ms != receipt.accepted_at_ms {
            return Err(corrupt_data(
                "messages",
                "accepted_at_ms",
                "message and original queue receipt acceptance times disagree",
            ));
        }
        let withdrawn = self
            .database
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                WITHDRAWN_TARGET_SQL,
                [
                    Value::String(Some(thread_id.as_str().to_owned())),
                    Value::String(Some(message_id.as_str().to_owned())),
                    Value::String(Some(original_request_id.as_str().to_owned())),
                ],
            ))
            .await
            .map_err(|source| database_error("read withdrawn-message receipt", source))?
            .is_some();
        if !withdrawn {
            return Ok(None);
        }

        reconstruct_payload(&self.database, &message, &receipt)
            .await
            .map(Some)
    }

    /// Explicitly named alias for edit flows that restore an original queue
    /// payload after the discard fence has committed.
    pub async fn read_original_queued_message_payload(
        &self,
        thread_id: &ThreadId,
        message_id: &MessageId,
        original_request_id: &RequestId,
    ) -> Result<Option<QueueMessagePayload>, QueuedMessageRepositoryError> {
        self.read_withdrawn_message_payload(thread_id, message_id, original_request_id)
            .await
    }
}

#[derive(Debug)]
enum WithdrawalTransactionOutcome {
    Accepted(WithdrawQueuedMessageResult),
    Duplicate(WithdrawQueuedMessageResult),
}

#[allow(
    clippy::too_many_lines,
    reason = "the transaction fence and its exact classification stay auditable in one function"
)]
async fn apply_withdrawal(
    transaction: &DatabaseTransaction,
    input: &WithdrawQueuedMessage,
) -> Result<WithdrawalTransactionOutcome, QueuedMessageRepositoryError> {
    if let Some(duplicate) = lookup_withdrawal(transaction, input).await? {
        return Ok(WithdrawalTransactionOutcome::Duplicate(duplicate));
    }

    if input.original_request_id == input.withdrawal_request_id {
        return Err(QueuedMessageRepositoryError::IdempotencyConflict {
            request_id: input.withdrawal_request_id.clone(),
        });
    }

    let existing_command_receipt =
        entities::command_receipt::Entity::find_by_id(input.withdrawal_request_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| database_error("check withdrawal request identity", source))?;
    if existing_command_receipt.is_some() {
        return Err(QueuedMessageRepositoryError::IdempotencyConflict {
            request_id: input.withdrawal_request_id.clone(),
        });
    }

    let Some(thread) = entities::thread::Entity::find_by_id(input.thread_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| database_error("find withdrawal thread", source))?
    else {
        return Err(QueuedMessageRepositoryError::ThreadNotFound {
            thread_id: input.thread_id.clone(),
        });
    };
    if input.accepted_at.as_millis() < thread.created_at_ms {
        return Err(QueuedMessageRepositoryError::InvalidChronology {
            accepted_at_ms: input.accepted_at.as_millis(),
            message_at_ms: thread.created_at_ms,
        });
    }

    let Some(message) = entities::message::Entity::find_by_id(input.message_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| database_error("find withdrawal message", source))?
    else {
        return accept_outcome(
            transaction,
            input,
            QueuedMessageWithdrawalOutcome::NotQueued,
        )
        .await;
    };
    if message.thread_id != input.thread_id.as_str() {
        return Err(QueuedMessageRepositoryError::CrossThread {
            thread_id: input.thread_id.clone(),
            message_id: input.message_id.clone(),
        });
    }
    if input.accepted_at.as_millis() < message.accepted_at_ms {
        return Err(QueuedMessageRepositoryError::InvalidChronology {
            accepted_at_ms: input.accepted_at.as_millis(),
            message_at_ms: message.accepted_at_ms,
        });
    }

    let Some(original_receipt) =
        entities::command_receipt::Entity::find_by_id(input.original_request_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| database_error("find original queue receipt", source))?
    else {
        return accept_outcome(
            transaction,
            input,
            QueuedMessageWithdrawalOutcome::NotQueued,
        )
        .await;
    };
    if original_receipt.thread_id.as_deref() != Some(input.thread_id.as_str()) {
        return Err(QueuedMessageRepositoryError::CrossThread {
            thread_id: input.thread_id.clone(),
            message_id: input.message_id.clone(),
        });
    }
    if original_receipt.command_kind != CommandKind::QueueMessage
        || original_receipt.message_id.as_deref() != Some(input.message_id.as_str())
    {
        return Err(QueuedMessageRepositoryError::OriginalRequestMismatch {
            original_request_id: input.original_request_id.clone(),
            message_id: input.message_id.clone(),
        });
    }
    if original_receipt.accepted_at_ms != message.accepted_at_ms {
        return Err(corrupt_data(
            "command_receipts",
            "accepted_at_ms",
            "original queue receipt and message acceptance times disagree",
        ));
    }
    if message.body != original_receipt.body.as_deref().unwrap_or_default() {
        return Err(corrupt_data(
            "command_receipts",
            "body",
            "original queue receipt and message text disagree",
        ));
    }

    let Some(dispatch) = entities::message_dispatch::Entity::find_by_id(input.message_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| database_error("find withdrawal dispatch", source))?
    else {
        return accept_outcome(
            transaction,
            input,
            QueuedMessageWithdrawalOutcome::NotQueued,
        )
        .await;
    };
    if dispatch.correlation_id != input.original_request_id.as_str() {
        return Err(QueuedMessageRepositoryError::OriginalRequestMismatch {
            original_request_id: input.original_request_id.clone(),
            message_id: input.message_id.clone(),
        });
    }
    if dispatch.queued_at_ms != message.accepted_at_ms {
        return Err(corrupt_data(
            "message_dispatches",
            "queued_at_ms",
            "dispatch and message acceptance times disagree",
        ));
    }

    let outcome = if dispatch.state == DispatchState::Queued {
        if dispatch.attempt_count < 0 {
            return Err(corrupt_data(
                "message_dispatches",
                "attempt_count",
                "attempt count is negative",
            ));
        }
        if dispatch.attempt_count == 0 {
            if dispatch.lease_owner.is_some() || dispatch.lease_expires_at_ms.is_some() {
                return Err(corrupt_data(
                    "message_dispatches",
                    "lease_owner",
                    "queued zero-attempt dispatch retains lease metadata",
                ));
            }
            if dispatch.updated_at_ms > input.accepted_at.as_millis() {
                return Err(QueuedMessageRepositoryError::InvalidChronology {
                    accepted_at_ms: input.accepted_at.as_millis(),
                    message_at_ms: dispatch.updated_at_ms,
                });
            }
            let updated = transaction
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    WITHDRAW_DISPATCH_SQL,
                    [
                        Value::BigInt(Some(input.accepted_at.as_millis())),
                        Value::String(Some(input.message_id.as_str().to_owned())),
                        Value::String(Some(input.original_request_id.as_str().to_owned())),
                        Value::BigInt(Some(input.accepted_at.as_millis())),
                    ],
                ))
                .await
                .map_err(|source| database_error("withdraw queued dispatch", source))?
                .rows_affected();
            if updated != 1 {
                return Err(QueuedMessageRepositoryError::Invariant {
                    reason: "queued dispatch fence changed without an identified claim",
                });
            }
            QueuedMessageWithdrawalOutcome::Withdrawn
        } else {
            QueuedMessageWithdrawalOutcome::TooLate
        }
    } else {
        QueuedMessageWithdrawalOutcome::TooLate
    };

    accept_outcome(transaction, input, outcome).await
}

async fn accept_outcome(
    transaction: &DatabaseTransaction,
    input: &WithdrawQueuedMessage,
    outcome: QueuedMessageWithdrawalOutcome,
) -> Result<WithdrawalTransactionOutcome, QueuedMessageRepositoryError> {
    let inserted = transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            INSERT_WITHDRAWAL_SQL,
            [
                Value::String(Some(input.withdrawal_request_id.as_str().to_owned())),
                Value::String(Some(input.thread_id.as_str().to_owned())),
                Value::String(Some(input.message_id.as_str().to_owned())),
                Value::String(Some(input.original_request_id.as_str().to_owned())),
                Value::String(Some(outcome_label(outcome).to_owned())),
                Value::BigInt(Some(input.accepted_at.as_millis())),
            ],
        ))
        .await
        .map_err(|source| database_error("record queued-message withdrawal", source))?
        .rows_affected();
    if inserted == 1 {
        return Ok(WithdrawalTransactionOutcome::Accepted(withdrawal_result(
            input,
            ReceiptDisposition::Accepted,
            outcome,
        )));
    }

    lookup_withdrawal(transaction, input)
        .await?
        .map(WithdrawalTransactionOutcome::Duplicate)
        .ok_or(QueuedMessageRepositoryError::Invariant {
            reason: "withdrawal receipt insert was ignored without a receipt",
        })
}

async fn lookup_withdrawal(
    database: &impl ConnectionTrait,
    input: &WithdrawQueuedMessage,
) -> Result<Option<WithdrawQueuedMessageResult>, QueuedMessageRepositoryError> {
    let Some(row) = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            WITHDRAWAL_RECEIPT_SQL,
            [Value::String(Some(
                input.withdrawal_request_id.as_str().to_owned(),
            ))],
        ))
        .await
        .map_err(|source| database_error("find queued-message withdrawal receipt", source))?
    else {
        return Ok(None);
    };

    let stored_thread_id = parse_thread_id(row_value(
        &row,
        0,
        "thread_id",
        "queued_message_withdrawals",
    )?)?;
    let stored_message_id = parse_message_id(row_value(
        &row,
        1,
        "message_id",
        "queued_message_withdrawals",
    )?)?;
    let stored_original_request_id = parse_request_id(row_value(
        &row,
        2,
        "original_request_id",
        "queued_message_withdrawals",
    )?)?;
    let stored_outcome =
        parse_outcome(row_value(&row, 3, "outcome", "queued_message_withdrawals")?)?;
    let stored_accepted_at =
        row_value::<i64>(&row, 4, "accepted_at_ms", "queued_message_withdrawals")?;

    if stored_thread_id != input.thread_id
        || stored_message_id != input.message_id
        || stored_original_request_id != input.original_request_id
    {
        return Err(QueuedMessageRepositoryError::IdempotencyConflict {
            request_id: input.withdrawal_request_id.clone(),
        });
    }

    // Acceptance time belongs to Forge, not the immutable client command.
    // A later server clock on retry replays the original receipt timestamp.
    let mut original = input.clone();
    original.accepted_at = UnixMillis::from_millis(stored_accepted_at);
    Ok(Some(withdrawal_result(
        &original,
        ReceiptDisposition::Duplicate,
        stored_outcome,
    )))
}

fn withdrawal_result(
    input: &WithdrawQueuedMessage,
    disposition: ReceiptDisposition,
    outcome: QueuedMessageWithdrawalOutcome,
) -> WithdrawQueuedMessageResult {
    WithdrawQueuedMessageResult {
        receipt: CommandReceipt {
            request_id: input.withdrawal_request_id.clone(),
            disposition,
        },
        thread_id: input.thread_id.clone(),
        message_id: input.message_id.clone(),
        original_request_id: input.original_request_id.clone(),
        accepted_at: input.accepted_at,
        outcome,
    }
}

async fn read_eligible_count(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
) -> Result<u64, QueuedMessageRepositoryError> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            LIST_COUNT_SQL,
            [Value::String(Some(thread_id.as_str().to_owned()))],
        ))
        .await
        .map_err(|source| database_error("count queued messages", source))?
        .ok_or(QueuedMessageRepositoryError::Invariant {
            reason: "queued-message count query returned no row",
        })?;
    let count = row_value::<i64>(&row, 0, "count", "message_dispatches")?;
    u64::try_from(count).map_err(|_| {
        corrupt_data(
            "message_dispatches",
            "count",
            "queued-message count is negative",
        )
    })
}

async fn summary_from_row(
    database: &impl ConnectionTrait,
    requested_thread_id: &ThreadId,
    row: &QueryResult,
) -> Result<QueuedMessageSummary, QueuedMessageRepositoryError> {
    let message_id = parse_message_id(row_value(row, 0, "message_id", "messages")?)?;
    let thread_id = parse_thread_id(row_value(row, 1, "thread_id", "messages")?)?;
    if thread_id != *requested_thread_id {
        return Err(corrupt_data(
            "messages",
            "thread_id",
            "queued-message page returned another thread",
        ));
    }
    let message_body = row_value::<String>(row, 2, "body", "messages")?;
    let original_request_id =
        parse_request_id(row_value(row, 3, "correlation_id", "message_dispatches")?)?;
    let text = authored_text(row_value::<Option<String>>(
        row,
        4,
        "body",
        "command_receipts",
    )?)?;
    if message_body != text.as_ref().map_or("", AuthoredText::as_str) {
        return Err(corrupt_data(
            "command_receipts",
            "body",
            "queue receipt and immutable message text presence disagree",
        ));
    }
    let accepted_at_ms = row_value::<i64>(row, 5, "accepted_at_ms", "messages")?;
    let queued_at_ms = row_value::<i64>(row, 6, "queued_at_ms", "message_dispatches")?;
    let receipt_accepted_at_ms = row_value::<i64>(row, 7, "accepted_at_ms", "command_receipts")?;
    if queued_at_ms != accepted_at_ms || receipt_accepted_at_ms != accepted_at_ms {
        return Err(corrupt_data(
            "message_dispatches",
            "queued_at_ms",
            "message, dispatch, and receipt acceptance times disagree",
        ));
    }

    let attachments = read_image_refs(database, &message_id, &thread_id).await?;
    if text.as_ref().is_none_or(AuthoredText::is_blank) && attachments.is_empty() {
        return Err(corrupt_data(
            "messages",
            "body",
            "queued general message has neither authored text nor image attachments",
        ));
    }

    Ok(QueuedMessageSummary {
        message_id,
        thread_id,
        original_request_id,
        text,
        attachments,
        accepted_at: UnixMillis::from_millis(accepted_at_ms),
    })
}

async fn reconstruct_payload(
    database: &impl ConnectionTrait,
    message: &entities::Message,
    receipt: &entities::CommandReceipt,
) -> Result<QueueMessagePayload, QueuedMessageRepositoryError> {
    let message_id = parse_message_id(message.message_id.clone())?;
    let thread_id = parse_thread_id(message.thread_id.clone())?;
    if !queue_receipt_owns_message(receipt, &thread_id, &message_id) {
        return Err(QueuedMessageRepositoryError::OriginalRequestMismatch {
            original_request_id: RequestId::parse(receipt.request_id.clone())
                .map_err(|error| corrupt_data("command_receipts", "request_id", error))?,
            message_id,
        });
    }
    let text = authored_text(receipt.body.clone())?;
    if message.body != text.as_ref().map_or("", AuthoredText::as_str) {
        return Err(corrupt_data(
            "command_receipts",
            "body",
            "queue receipt and immutable message text presence disagree",
        ));
    }
    let attachments = read_image_attachments(database, &message_id).await?;
    QueueMessagePayload::new(text, attachments).map_err(|error| {
        corrupt_data(
            "messages",
            "body",
            format!("invalid queued payload: {error}"),
        )
    })
}

async fn read_image_refs(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
    thread_id: &ThreadId,
) -> Result<Vec<ImageAttachmentRef>, QueuedMessageRepositoryError> {
    let rows = read_image_rows(database, message_id).await?;
    validate_image_row_count(rows.len())?;
    let mut references = Vec::with_capacity(rows.len());
    for (expected_position, row) in rows.into_iter().enumerate() {
        let index = u32::try_from(expected_position).map_err(|_| {
            QueuedMessageRepositoryError::Invariant {
                reason: "image attachment position overflow",
            }
        })?;
        if row.position != i64::from(index) {
            return Err(corrupt_data(
                "message_image_attachments",
                "position",
                "attachment positions are not contiguous",
            ));
        }
        let size_bytes = u32::try_from(row.size_bytes).map_err(|_| {
            corrupt_data(
                "message_image_attachments",
                "size_bytes",
                "attachment size is outside the reference range",
            )
        })?;
        if row.size_bytes != i64::try_from(row.bytes.as_slice().len()).unwrap_or(-1) {
            return Err(corrupt_data(
                "message_image_attachments",
                "size_bytes",
                "attachment byte length disagrees with its blob",
            ));
        }
        let digest: [u8; 32] = Sha256::digest(row.bytes.as_slice()).into();
        let reference = ImageAttachmentRef::new(
            message_id.clone(),
            thread_id.clone(),
            index,
            row.mime_type,
            row.name,
            size_bytes,
            digest,
        )
        .map_err(|error| corrupt_data("message_image_attachments", "metadata", error))?;
        references.push(reference);
    }
    let total_bytes = references.iter().try_fold(0usize, |total, reference| {
        total.checked_add(usize::try_from(reference.size_bytes).unwrap_or(usize::MAX))
    });
    if total_bytes.is_none_or(|total| total > MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES) {
        return Err(corrupt_data(
            "message_image_attachments",
            "size_bytes",
            "attachment aggregate size exceeds the queued payload bound",
        ));
    }
    Ok(references)
}

async fn read_image_attachments(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
) -> Result<Vec<ImageAttachment>, QueuedMessageRepositoryError> {
    let rows = read_image_rows(database, message_id).await?;
    validate_image_row_count(rows.len())?;
    let mut attachments = Vec::with_capacity(rows.len());
    for (expected_position, row) in rows.into_iter().enumerate() {
        let index = u32::try_from(expected_position).map_err(|_| {
            QueuedMessageRepositoryError::Invariant {
                reason: "image attachment position overflow",
            }
        })?;
        if row.position != i64::from(index) {
            return Err(corrupt_data(
                "message_image_attachments",
                "position",
                "attachment positions are not contiguous",
            ));
        }
        if row.size_bytes != i64::try_from(row.bytes.as_slice().len()).unwrap_or(-1) {
            return Err(corrupt_data(
                "message_image_attachments",
                "size_bytes",
                "attachment byte length disagrees with its blob",
            ));
        }
        let attachment = ImageAttachment::new(row.mime_type, row.bytes.into_vec(), row.name)
            .map_err(|error| corrupt_data("message_image_attachments", "bytes", error))?;
        attachments.push(attachment);
    }
    Ok(attachments)
}

async fn read_image_rows(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
) -> Result<Vec<entities::MessageImageAttachment>, QueuedMessageRepositoryError> {
    entities::message_image_attachment::Entity::find()
        .filter(entities::message_image_attachment::Column::MessageId.eq(message_id.as_str()))
        .order_by_asc(entities::message_image_attachment::Column::Position)
        .limit(
            u64::try_from(MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT + 1).map_err(|_| {
                QueuedMessageRepositoryError::Invariant {
                    reason: "image attachment row bound does not fit SQLite's limit type",
                }
            })?,
        )
        .all(database)
        .await
        .map_err(|source| database_error("read queued-message image attachments", source))
}

fn validate_image_row_count(count: usize) -> Result<(), QueuedMessageRepositoryError> {
    if count > MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
        return Err(corrupt_data(
            "message_image_attachments",
            "position",
            "attachment count exceeds the queued payload bound",
        ));
    }
    Ok(())
}

async fn ensure_thread(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
) -> Result<(), QueuedMessageRepositoryError> {
    let exists = entities::thread::Entity::find_by_id(thread_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find queued-message thread", source))?
        .is_some();
    if exists {
        Ok(())
    } else {
        Err(QueuedMessageRepositoryError::ThreadNotFound {
            thread_id: thread_id.clone(),
        })
    }
}

fn queue_receipt_owns_message(
    receipt: &entities::CommandReceipt,
    thread_id: &ThreadId,
    message_id: &MessageId,
) -> bool {
    receipt.command_kind == CommandKind::QueueMessage
        && receipt.thread_id.as_deref() == Some(thread_id.as_str())
        && receipt.message_id.as_deref() == Some(message_id.as_str())
}

fn authored_text(
    body: Option<String>,
) -> Result<Option<AuthoredText>, QueuedMessageRepositoryError> {
    body.map(|body| AuthoredText::parse(body))
        .transpose()
        .map_err(|error| corrupt_data("command_receipts", "body", error))
}

fn validate_limit(limit: usize) -> Result<(), QueuedMessageRepositoryError> {
    if limit == 0 {
        return Err(QueuedMessageRepositoryError::InvalidListLimit(
            QueuedMessageListError::Empty,
        ));
    }
    if limit > QUEUED_MESSAGE_LIST_MAX {
        return Err(QueuedMessageRepositoryError::InvalidListLimit(
            QueuedMessageListError::TooLarge {
                limit,
                maximum: QUEUED_MESSAGE_LIST_MAX,
            },
        ));
    }
    Ok(())
}

fn sqlite_limit(limit: usize) -> Result<i64, QueuedMessageRepositoryError> {
    i64::try_from(limit).map_err(|_| QueuedMessageRepositoryError::Invariant {
        reason: "queued-message list limit does not fit SQLite",
    })
}

fn parse_thread_id(value: String) -> Result<ThreadId, QueuedMessageRepositoryError> {
    ThreadId::parse(value).map_err(|error| corrupt_data("threads", "thread_id", error))
}

fn parse_message_id(value: String) -> Result<MessageId, QueuedMessageRepositoryError> {
    MessageId::parse(value).map_err(|error| corrupt_data("messages", "message_id", error))
}

fn parse_request_id(value: String) -> Result<RequestId, QueuedMessageRepositoryError> {
    RequestId::parse(value).map_err(|error| corrupt_data("command_receipts", "request_id", error))
}

fn parse_outcome(
    value: String,
) -> Result<QueuedMessageWithdrawalOutcome, QueuedMessageRepositoryError> {
    match value.as_str() {
        "withdrawn" => Ok(QueuedMessageWithdrawalOutcome::Withdrawn),
        "too_late" => Ok(QueuedMessageWithdrawalOutcome::TooLate),
        "not_queued" => Ok(QueuedMessageWithdrawalOutcome::NotQueued),
        _ => Err(corrupt_data(
            "queued_message_withdrawals",
            "outcome",
            "unknown withdrawal outcome",
        )),
    }
}

const fn outcome_label(outcome: QueuedMessageWithdrawalOutcome) -> &'static str {
    match outcome {
        QueuedMessageWithdrawalOutcome::Withdrawn => "withdrawn",
        QueuedMessageWithdrawalOutcome::TooLate => "too_late",
        QueuedMessageWithdrawalOutcome::NotQueued => "not_queued",
    }
}

fn row_value<T>(
    row: &QueryResult,
    index: usize,
    field: &'static str,
    table: &'static str,
) -> Result<T, QueuedMessageRepositoryError>
where
    T: TryGetable,
{
    row.try_get_by_index(index)
        .map_err(|error| corrupt_data(table, field, error))
}

fn corrupt_data(
    table: &'static str,
    field: &'static str,
    reason: impl ToString,
) -> QueuedMessageRepositoryError {
    QueuedMessageRepositoryError::CorruptData {
        table,
        field,
        reason: reason.to_string(),
    }
}

fn database_error(operation: &'static str, source: DbErr) -> QueuedMessageRepositoryError {
    QueuedMessageRepositoryError::Database { operation, source }
}
