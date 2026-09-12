//! Atomic withdraw transaction and its idempotency receipt replay.

#![forbid(unsafe_code)]

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, EntityTrait, Statement, Value};

use artisan_domain::{
    CommandReceipt, MessageId, QueuedMessageWithdrawalOutcome, ReceiptDisposition, RequestId,
    ThreadId, UnixMillis, WithdrawQueuedMessage, WithdrawQueuedMessageResult,
};

use crate::entities::{self, CommandKind, DispatchState};
use crate::repository::{Repository, corrupt_data, database_error, row_value};

use super::QueuedMessageRepositoryError;
use super::rows::{parse_message_id, parse_request_id, parse_thread_id};

const WITHDRAWAL_RECEIPT_SQL: &str = r"
SELECT thread_id, message_id, original_request_id, outcome, accepted_at_ms
FROM queued_message_withdrawals
WHERE withdrawal_request_id = ?
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
impl Repository {
    /// Replays an exact withdrawal before the caller consults its acceptance clock.
    /// No dispatch state or receipt is changed by this lookup.
    ///
    /// # Errors
    /// Returns an identity conflict, corrupt receipt, or database error.
    pub async fn lookup_queued_message_withdrawal(
        &self,
        thread_id: &ThreadId,
        message_id: &MessageId,
        original_request_id: &RequestId,
        withdrawal_request_id: &RequestId,
    ) -> Result<Option<WithdrawQueuedMessageResult>, QueuedMessageRepositoryError> {
        lookup_withdrawal(
            &self.database,
            &WithdrawQueuedMessage {
                thread_id: thread_id.clone(),
                message_id: message_id.clone(),
                original_request_id: original_request_id.clone(),
                withdrawal_request_id: withdrawal_request_id.clone(),
                // Lookup replaces this unused stamp with the durable receipt time.
                accepted_at: UnixMillis::EPOCH,
            },
        )
        .await
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
            .begin_write()
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
    let stored_outcome_value =
        row_value::<String, _>(&row, 3, "outcome", "queued_message_withdrawals")?;
    let stored_outcome = parse_outcome(&stored_outcome_value)?;
    let stored_accepted_at =
        row_value::<i64, _>(&row, 4, "accepted_at_ms", "queued_message_withdrawals")?;

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

fn parse_outcome(
    value: &str,
) -> Result<QueuedMessageWithdrawalOutcome, QueuedMessageRepositoryError> {
    match value {
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
