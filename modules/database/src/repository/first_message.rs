//! Atomic first-message admission and durable outbox persistence.

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseTransaction,
    EntityTrait, QueryFilter, TransactionTrait,
};

use artisan_domain::{
    CommandReceipt, MessageBody, MessageId, QueuedMessage, ReceiptDisposition, RequestId, ThreadId,
    UnixMillis,
};

use crate::entities::{self, CommandKind, DispatchState};

use super::{Repository, RepositoryError, corrupt_data, database_error, millis};

const FIRST_MESSAGE_ORDINAL: i64 = 0;

/// Storage input after Forge mints the accepted message identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueFirstMessageInput {
    pub request_id: RequestId,
    pub message_id: MessageId,
    pub thread_id: ThreadId,
    pub body: MessageBody,
    pub accepted_at: UnixMillis,
}

/// Durable command receipt paired with the original queued message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueFirstMessageResult {
    pub receipt: CommandReceipt,
    pub message: QueuedMessage,
    pub queued_at: UnixMillis,
}

impl Repository {
    /// Checks a queue receipt before Forge mints another message id.
    ///
    /// # Errors
    ///
    /// Returns an idempotency conflict when the request id names another
    /// command payload, corrupt persisted data, or a database failure.
    pub async fn lookup_queue_first_message(
        &self,
        request_id: &RequestId,
        thread_id: &ThreadId,
        body: &MessageBody,
    ) -> Result<Option<QueueFirstMessageResult>, RepositoryError> {
        lookup_queue_receipt(
            &self.database,
            request_id,
            thread_id,
            body,
            ReceiptDisposition::Duplicate,
        )
        .await
    }

    /// Atomically stores the immutable first message, outbox row, and receipt.
    ///
    /// Call [`Self::lookup_queue_first_message`] before minting a message id;
    /// this method repeats the lookup transactionally to close concurrent
    /// races. The message insert is the transaction's first statement so
    /// SQLite obtains a writer position before any transactional read.
    ///
    /// # Errors
    ///
    /// Returns a missing-thread, chronology, message, first-message, or
    /// request conflict; corrupt persisted data; or a database failure.
    /// Every rejected provisional insert is rolled back.
    pub async fn queue_first_message(
        &self,
        input: QueueFirstMessageInput,
    ) -> Result<QueueFirstMessageResult, RepositoryError> {
        if let Some(duplicate) = self
            .lookup_queue_first_message(&input.request_id, &input.thread_id, &input.body)
            .await?
        {
            return Ok(duplicate);
        }

        let thread = thread_row_by_id(&self.database, &input.thread_id)
            .await?
            .ok_or_else(|| RepositoryError::ThreadNotFound {
                thread_id: input.thread_id.clone(),
            })?;
        if millis(input.accepted_at) < thread.created_at_ms {
            return Err(RepositoryError::InvalidChronology {
                earlier_field: "thread.created_at",
                later_field: "message.accepted_at",
            });
        }

        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin first-message transaction", source))?;
        let inserted_message = insert_first_message(&transaction, &input).await?;
        if inserted_message == 0 {
            return classify_message_conflict(transaction, input).await;
        }

        let inserted_dispatch = insert_queued_dispatch(&transaction, &input).await?;
        if inserted_dispatch == 0 {
            return rollback_with_error(
                transaction,
                RepositoryError::IdempotencyConflict {
                    request_id: input.request_id,
                },
            )
            .await;
        }

        let inserted_receipt = insert_queue_receipt(&transaction, &input).await?;
        if inserted_receipt == 0 {
            let result = lookup_queue_receipt(
                &transaction,
                &input.request_id,
                &input.thread_id,
                &input.body,
                ReceiptDisposition::Duplicate,
            )
            .await;
            return rollback_with_lookup(transaction, result).await;
        }

        let updated_at_ms = thread.updated_at_ms.max(millis(input.accepted_at));
        let mut updated_thread = entities::thread::ActiveModel::from(thread);
        updated_thread.updated_at_ms = Set(updated_at_ms);
        updated_thread
            .update(&transaction)
            .await
            .map_err(|source| database_error("update thread recency", source))?;

        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit first-message transaction", source))?;
        Ok(queue_result(&input, ReceiptDisposition::Accepted))
    }
}

async fn insert_first_message(
    database: &impl ConnectionTrait,
    input: &QueueFirstMessageInput,
) -> Result<u64, RepositoryError> {
    entities::message::Entity::insert(entities::message::ActiveModel {
        message_id: Set(input.message_id.as_str().to_owned()),
        thread_id: Set(input.thread_id.as_str().to_owned()),
        ordinal: Set(FIRST_MESSAGE_ORDINAL),
        body: Set(input.body.as_str().to_owned()),
        accepted_at_ms: Set(millis(input.accepted_at)),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(database)
    .await
    .map_err(|source| database_error("insert first message", source))
}

async fn insert_queued_dispatch(
    database: &impl ConnectionTrait,
    input: &QueueFirstMessageInput,
) -> Result<u64, RepositoryError> {
    entities::message_dispatch::Entity::insert(entities::message_dispatch::ActiveModel {
        message_id: Set(input.message_id.as_str().to_owned()),
        correlation_id: Set(input.request_id.as_str().to_owned()),
        state: Set(DispatchState::Queued),
        attempt_count: Set(0),
        queued_at_ms: Set(millis(input.accepted_at)),
        available_at_ms: Set(millis(input.accepted_at)),
        lease_owner: Set(None),
        lease_expires_at_ms: Set(None),
        last_error: Set(None),
        updated_at_ms: Set(millis(input.accepted_at)),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(database)
    .await
    .map_err(|source| database_error("queue message dispatch", source))
}

async fn insert_queue_receipt(
    database: &impl ConnectionTrait,
    input: &QueueFirstMessageInput,
) -> Result<u64, RepositoryError> {
    entities::command_receipt::Entity::insert(entities::command_receipt::ActiveModel {
        request_id: Set(input.request_id.as_str().to_owned()),
        command_kind: Set(CommandKind::QueueFirstMessage),
        directory_id: Set(None),
        project_id: Set(None),
        thread_id: Set(Some(input.thread_id.as_str().to_owned())),
        title: Set(None),
        message_id: Set(Some(input.message_id.as_str().to_owned())),
        body: Set(Some(input.body.as_str().to_owned())),
        accepted_at_ms: Set(millis(input.accepted_at)),
        engine_run_config_version: Set(None),
        engine_run_config: Set(None),
        engine_run_config_expected_revision: Set(None),
        engine_run_config_result_revision: Set(None),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(database)
    .await
    .map_err(|source| database_error("record first-message receipt", source))
}

async fn classify_message_conflict(
    transaction: DatabaseTransaction,
    input: QueueFirstMessageInput,
) -> Result<QueueFirstMessageResult, RepositoryError> {
    let receipt = lookup_queue_receipt(
        &transaction,
        &input.request_id,
        &input.thread_id,
        &input.body,
        ReceiptDisposition::Duplicate,
    )
    .await;
    match receipt {
        Ok(Some(duplicate)) => {
            transaction.rollback().await.map_err(|source| {
                database_error("finish duplicate first-message request", source)
            })?;
            return Ok(duplicate);
        }
        Err(error) => return rollback_with_error(transaction, error).await,
        Ok(None) => {}
    }

    if let Some(existing) = first_message_row(&transaction, &input.thread_id).await? {
        let existing_message_id = MessageId::parse(existing.message_id)
            .map_err(|error| corrupt_data("messages", "message_id", &error))?;
        return rollback_with_error(
            transaction,
            RepositoryError::FirstMessageAlreadyExists {
                thread_id: input.thread_id,
                existing_message_id,
            },
        )
        .await;
    }

    if message_row_by_id(&transaction, &input.message_id)
        .await?
        .is_some()
    {
        return rollback_with_error(
            transaction,
            RepositoryError::MessageConflict {
                message_id: input.message_id,
            },
        )
        .await;
    }

    rollback_with_error(
        transaction,
        RepositoryError::Invariant {
            reason: "first-message insert was ignored without an identifiable conflict",
        },
    )
    .await
}

async fn lookup_queue_receipt(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
    thread_id: &ThreadId,
    body: &MessageBody,
    disposition: ReceiptDisposition,
) -> Result<Option<QueueFirstMessageResult>, RepositoryError> {
    let Some(row) = receipt_row_by_id(database, request_id).await? else {
        return Ok(None);
    };
    if row.command_kind != CommandKind::QueueFirstMessage
        || row.thread_id.as_deref() != Some(thread_id.as_str())
        || row.body.as_deref() != Some(body.as_str())
    {
        return Err(RepositoryError::IdempotencyConflict {
            request_id: request_id.clone(),
        });
    }

    let message_id = MessageId::parse(required(row.message_id, "command_receipts", "message_id")?)
        .map_err(|error| corrupt_data("command_receipts", "message_id", &error))?;
    let message =
        message_row_by_id(database, &message_id)
            .await?
            .ok_or(RepositoryError::Invariant {
                reason: "queue receipt references a missing message",
            })?;
    if message.thread_id != thread_id.as_str() || message.body != body.as_str() {
        return Err(RepositoryError::Invariant {
            reason: "queue receipt and immutable message payload disagree",
        });
    }
    if message.accepted_at_ms != row.accepted_at_ms {
        return Err(RepositoryError::Invariant {
            reason: "queue receipt and immutable message acceptance times disagree",
        });
    }
    let dispatch = dispatch_row_by_message_id(database, &message_id)
        .await?
        .ok_or(RepositoryError::Invariant {
            reason: "queue receipt references a message without a durable dispatch",
        })?;
    if dispatch.correlation_id != request_id.as_str() {
        return Err(RepositoryError::Invariant {
            reason: "queue receipt and durable dispatch request identities disagree",
        });
    }
    if dispatch.queued_at_ms != row.accepted_at_ms {
        return Err(RepositoryError::Invariant {
            reason: "queue receipt and durable dispatch queue times disagree",
        });
    }
    let persisted_body = MessageBody::parse(message.body)
        .map_err(|error| corrupt_data("messages", "body", &error))?;

    Ok(Some(QueueFirstMessageResult {
        receipt: CommandReceipt {
            request_id: request_id.clone(),
            disposition,
        },
        message: QueuedMessage {
            message_id,
            thread_id: thread_id.clone(),
            request_id: request_id.clone(),
            body: persisted_body,
        },
        queued_at: UnixMillis::from_millis(row.accepted_at_ms),
    }))
}

async fn receipt_row_by_id(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
) -> Result<Option<entities::CommandReceipt>, RepositoryError> {
    entities::command_receipt::Entity::find_by_id(request_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find command receipt", source))
}

async fn thread_row_by_id(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
) -> Result<Option<entities::Thread>, RepositoryError> {
    entities::thread::Entity::find_by_id(thread_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find first-message thread", source))
}

async fn first_message_row(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
) -> Result<Option<entities::Message>, RepositoryError> {
    entities::message::Entity::find()
        .filter(entities::message::Column::ThreadId.eq(thread_id.as_str()))
        .filter(entities::message::Column::Ordinal.eq(FIRST_MESSAGE_ORDINAL))
        .one(database)
        .await
        .map_err(|source| database_error("find existing first message", source))
}

async fn message_row_by_id(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
) -> Result<Option<entities::Message>, RepositoryError> {
    entities::message::Entity::find_by_id(message_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find message by id", source))
}

async fn dispatch_row_by_message_id(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
) -> Result<Option<entities::MessageDispatch>, RepositoryError> {
    entities::message_dispatch::Entity::find_by_id(message_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find message dispatch by message id", source))
}

fn queue_result(
    input: &QueueFirstMessageInput,
    disposition: ReceiptDisposition,
) -> QueueFirstMessageResult {
    QueueFirstMessageResult {
        receipt: CommandReceipt {
            request_id: input.request_id.clone(),
            disposition,
        },
        message: QueuedMessage {
            message_id: input.message_id.clone(),
            thread_id: input.thread_id.clone(),
            request_id: input.request_id.clone(),
            body: input.body.clone(),
        },
        queued_at: input.accepted_at,
    }
}

fn required(
    value: Option<String>,
    table: &'static str,
    field: &'static str,
) -> Result<String, RepositoryError> {
    value.ok_or_else(|| corrupt_data(table, field, "required value is null"))
}

async fn rollback_with_error<T>(
    transaction: DatabaseTransaction,
    error: RepositoryError,
) -> Result<T, RepositoryError> {
    transaction
        .rollback()
        .await
        .map_err(|source| database_error("roll back rejected transaction", source))?;
    Err(error)
}

async fn rollback_with_lookup(
    transaction: DatabaseTransaction,
    result: Result<Option<QueueFirstMessageResult>, RepositoryError>,
) -> Result<QueueFirstMessageResult, RepositoryError> {
    transaction
        .rollback()
        .await
        .map_err(|source| database_error("roll back duplicate transaction", source))?;
    result?.ok_or(RepositoryError::Invariant {
        reason: "receipt insert was ignored without an identifiable request",
    })
}

fn do_nothing_on_conflict() -> OnConflict {
    let mut conflict = OnConflict::new();
    conflict.do_nothing();
    conflict.clone()
}
