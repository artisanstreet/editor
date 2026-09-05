//! Atomic general-message admission, ordered image persistence, and replay.

use std::fmt;

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseTransaction,
    EntityTrait, QueryFilter, QueryOrder, TransactionTrait,
};
use sha2::{Digest, Sha256};

use artisan_domain::{
    AuthoredText, CommandReceipt, ImageAttachment, ImageAttachmentRef, MessageId,
    QueueMessagePayload, ReceiptDisposition, RequestId, ThreadId, UnixMillis,
};

use crate::entities::{self, CommandKind, DispatchState};

use super::{Repository, RepositoryError, corrupt_data, database_error, millis};

/// Storage input after Forge mints the accepted message identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueMessageInput {
    /// Client request identity used for replay correlation.
    pub request_id: RequestId,
    /// Forge-minted immutable message identity.
    pub message_id: MessageId,
    /// Existing thread receiving the message.
    pub thread_id: ThreadId,
    /// Validated authored text and ordered image bytes.
    pub payload: QueueMessagePayload,
    /// Authoritative acceptance time.
    pub accepted_at: UnixMillis,
}

/// Durable general-message receipt paired with its original payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueMessageResult {
    /// Idempotent command receipt.
    pub receipt: CommandReceipt,
    /// Forge-minted message identity.
    pub message_id: MessageId,
    /// Owning thread identity.
    pub thread_id: ThreadId,
    /// Original text/image payload, including attachment order.
    pub payload: QueueMessagePayload,
    /// Durable acceptance instant.
    pub queued_at: UnixMillis,
}

/// One authenticated bounded image read, paired with byte-free metadata.
#[derive(Clone, PartialEq, Eq)]
pub struct MessageImageRead {
    /// Ownership-checked renderer reference.
    pub reference: ImageAttachmentRef,
    /// Original encoded image bytes.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for MessageImageRead {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MessageImageRead")
            .field("reference", &self.reference)
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}

impl Repository {
    /// Checks a general queue receipt before Forge mints another message id.
    pub async fn lookup_queue_message(
        &self,
        request_id: &RequestId,
        thread_id: &ThreadId,
        payload: &QueueMessagePayload,
    ) -> Result<Option<QueueMessageResult>, RepositoryError> {
        lookup_queue_receipt(
            &self.database,
            request_id,
            thread_id,
            payload,
            ReceiptDisposition::Duplicate,
        )
        .await
    }

    /// Atomically stores a general message, ordered image rows, outbox row,
    /// and request receipt.
    pub async fn queue_message(
        &self,
        input: QueueMessageInput,
    ) -> Result<QueueMessageResult, RepositoryError> {
        if let Some(duplicate) = self
            .lookup_queue_message(&input.request_id, &input.thread_id, &input.payload)
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
            .map_err(|source| database_error("begin queue-message transaction", source))?;
        let ordinal = next_message_ordinal(&transaction, &input.thread_id).await?;
        let inserted_message = insert_message(&transaction, &input, ordinal).await?;
        if inserted_message == 0 {
            return classify_message_conflict(transaction, input).await;
        }

        insert_image_attachments(&transaction, &input).await?;
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
                &input.payload,
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
            .map_err(|source| database_error("commit queue-message transaction", source))?;
        Ok(queue_result(&input, ReceiptDisposition::Accepted))
    }

    /// Reads exactly one persisted image only when both the message and its
    /// owning thread match. Wrong-thread and missing-index reads return
    /// `None`, so callers cannot use this seam to probe another thread's
    /// attachment rows.
    pub async fn read_message_image(
        &self,
        thread_id: &ThreadId,
        message_id: &MessageId,
        index: u32,
    ) -> Result<Option<MessageImageRead>, RepositoryError> {
        let Some(message) = message_row_by_id(&self.database, message_id).await? else {
            return Ok(None);
        };
        if message.thread_id != thread_id.as_str() {
            return Ok(None);
        }
        let position = i64::from(index);
        let Some(row) = entities::message_image_attachment::Entity::find_by_id((
            message_id.as_str().to_owned(),
            position,
        ))
        .one(&self.database)
        .await
        .map_err(|source| database_error("read message image attachment", source))?
        else {
            return Ok(None);
        };
        let reference = image_attachment_ref_from_row(&row, message_id, thread_id, index)?;
        Ok(Some(MessageImageRead {
            reference,
            bytes: row.bytes.into_vec(),
        }))
    }
}

/// Loads the immutable payload from the message and attachment tables for
/// dispatch and restart recovery. The queue receipt is authoritative for the
/// presence of authored text, so `Some("")` stays distinct from `None` even
/// though the legacy `messages.body` column stores both as an empty string.
pub(crate) async fn read_queue_message_payload(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
) -> Result<Option<QueueMessagePayload>, RepositoryError> {
    let Some(message) = message_row_by_id(database, message_id).await? else {
        return Ok(None);
    };
    let text = read_authored_text(database, message_id, &message).await?;
    let attachments = read_image_attachments(database, message_id).await?;
    QueueMessagePayload::new(text, attachments)
        .map(Some)
        .map_err(|error| corrupt_data("messages", "body", &error))
}

async fn next_message_ordinal(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
) -> Result<i64, RepositoryError> {
    let latest = entities::message::Entity::find()
        .filter(entities::message::Column::ThreadId.eq(thread_id.as_str()))
        .order_by_desc(entities::message::Column::Ordinal)
        .one(database)
        .await
        .map_err(|source| database_error("find latest message ordinal", source))?;
    match latest {
        Some(message) => message
            .ordinal
            .checked_add(1)
            .ok_or(RepositoryError::Invariant {
                reason: "message ordinal overflow",
            }),
        None => Ok(0),
    }
}

async fn insert_message(
    database: &impl ConnectionTrait,
    input: &QueueMessageInput,
    ordinal: i64,
) -> Result<u64, RepositoryError> {
    entities::message::Entity::insert(entities::message::ActiveModel {
        message_id: Set(input.message_id.as_str().to_owned()),
        thread_id: Set(input.thread_id.as_str().to_owned()),
        ordinal: Set(ordinal),
        body: Set(input
            .payload
            .text()
            .map_or_else(String::new, |text| text.as_str().to_owned())),
        accepted_at_ms: Set(millis(input.accepted_at)),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(database)
    .await
    .map_err(|source| database_error("insert queued message", source))
}

async fn insert_image_attachments(
    database: &impl ConnectionTrait,
    input: &QueueMessageInput,
) -> Result<(), RepositoryError> {
    for (position, attachment) in input.payload.attachments().iter().enumerate() {
        let position = i64::try_from(position).map_err(|_| RepositoryError::Invariant {
            reason: "image attachment position overflow",
        })?;
        let size_bytes = i64::try_from(attachment.byte_len()).map_err(|_| {
            RepositoryError::Invariant {
                reason: "image attachment byte length overflow",
            }
        })?;
        entities::message_image_attachment::Entity::insert(
            entities::message_image_attachment::ActiveModel {
                message_id: Set(input.message_id.as_str().to_owned()),
                position: Set(position),
                mime_type: Set(attachment.mime_type_str().to_owned()),
                name: Set(attachment.name().to_owned()),
                size_bytes: Set(size_bytes),
                bytes: Set(entities::OpaqueBytes::new(attachment.bytes().to_vec())),
            },
        )
        .exec_without_returning(database)
        .await
        .map_err(|source| database_error("insert message image attachment", source))?;
    }
    Ok(())
}

async fn insert_queued_dispatch(
    database: &impl ConnectionTrait,
    input: &QueueMessageInput,
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
    input: &QueueMessageInput,
) -> Result<u64, RepositoryError> {
    entities::command_receipt::Entity::insert(entities::command_receipt::ActiveModel {
        request_id: Set(input.request_id.as_str().to_owned()),
        command_kind: Set(CommandKind::QueueMessage),
        directory_id: Set(None),
        project_id: Set(None),
        thread_id: Set(Some(input.thread_id.as_str().to_owned())),
        title: Set(None),
        message_id: Set(Some(input.message_id.as_str().to_owned())),
        body: Set(input
            .payload
            .text()
            .map(|text| text.as_str().to_owned())),
        accepted_at_ms: Set(millis(input.accepted_at)),
        engine_run_config_version: Set(None),
        engine_run_config: Set(None),
        engine_run_config_expected_revision: Set(None),
        engine_run_config_result_revision: Set(None),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(database)
    .await
    .map_err(|source| database_error("record queue-message receipt", source))
}

async fn classify_message_conflict(
    transaction: DatabaseTransaction,
    input: QueueMessageInput,
) -> Result<QueueMessageResult, RepositoryError> {
    let receipt = lookup_queue_receipt(
        &transaction,
        &input.request_id,
        &input.thread_id,
        &input.payload,
        ReceiptDisposition::Duplicate,
    )
    .await;
    match receipt {
        Ok(Some(duplicate)) => {
            transaction
                .rollback()
                .await
                .map_err(|source| database_error("finish duplicate queue-message request", source))?;
            return Ok(duplicate);
        }
        Err(error) => return rollback_with_error(transaction, error).await,
        Ok(None) => {}
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
            reason: "message insert was ignored without an identifiable conflict",
        },
    )
    .await
}

async fn lookup_queue_receipt(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
    thread_id: &ThreadId,
    payload: &QueueMessagePayload,
    disposition: ReceiptDisposition,
) -> Result<Option<QueueMessageResult>, RepositoryError> {
    let Some(row) = receipt_row_by_id(database, request_id).await? else {
        return Ok(None);
    };
    if row.command_kind != CommandKind::QueueMessage
        || row.thread_id.as_deref() != Some(thread_id.as_str())
        || row.body.as_deref() != payload.text().map(AuthoredText::as_str)
    {
        return Err(RepositoryError::IdempotencyConflict {
            request_id: request_id.clone(),
        });
    }

    let message_id = MessageId::parse(required(row.message_id, "command_receipts", "message_id")?)
        .map_err(|error| corrupt_data("command_receipts", "message_id", &error))?;
    let message = message_row_by_id(database, &message_id)
        .await?
        .ok_or(RepositoryError::Invariant {
            reason: "queue receipt references a missing message",
        })?;
    if message.thread_id != thread_id.as_str()
        || message.body
            != payload
                .text()
                .map_or_else(String::new, |text| text.as_str().to_owned())
    {
        return Err(RepositoryError::Invariant {
            reason: "queue receipt and immutable message payload disagree",
        });
    }
    if message.accepted_at_ms != row.accepted_at_ms {
        return Err(RepositoryError::Invariant {
            reason: "queue receipt and immutable message acceptance times disagree",
        });
    }
    let attachments = read_image_attachments(database, &message_id).await?;
    if attachments.as_slice() != payload.attachments() {
        return Err(RepositoryError::IdempotencyConflict {
            request_id: request_id.clone(),
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

    Ok(Some(QueueMessageResult {
        receipt: CommandReceipt {
            request_id: request_id.clone(),
            disposition,
        },
        message_id,
        thread_id: thread_id.clone(),
        payload: payload.clone(),
        queued_at: UnixMillis::from_millis(row.accepted_at_ms),
    }))
}

pub(crate) async fn read_image_attachments(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
) -> Result<Vec<ImageAttachment>, RepositoryError> {
    let rows = entities::message_image_attachment::Entity::find()
        .filter(entities::message_image_attachment::Column::MessageId.eq(message_id.as_str()))
        .order_by_asc(entities::message_image_attachment::Column::Position)
        .all(database)
        .await
        .map_err(|source| database_error("read message image attachments", source))?;
    let mut attachments = Vec::with_capacity(rows.len());
    for (expected_position, row) in rows.into_iter().enumerate() {
        let expected_position = u32::try_from(expected_position).map_err(|_| {
            RepositoryError::Invariant {
                reason: "image attachment position overflow",
            }
        })?;
        if row.position != i64::from(expected_position) {
            return Err(corrupt_data(
                "message_image_attachments",
                "position",
                "attachment positions are not contiguous",
            ));
        }
        validate_image_attachment_row(&row)?;
        let attachment = ImageAttachment::new(row.mime_type, row.bytes.into_vec(), row.name)
            .map_err(|error| corrupt_data("message_image_attachments", "bytes", &error))?;
        attachments.push(attachment);
    }
    Ok(attachments)
}

/// Loads compact renderer references for one message without exposing image
/// bytes to the conversation snapshot path.
pub(crate) async fn read_queue_message_projection(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
    thread_id: &ThreadId,
) -> Result<(Option<AuthoredText>, Vec<ImageAttachmentRef>), RepositoryError> {
    let Some(message) = message_row_by_id(database, message_id).await? else {
        return Err(RepositoryError::Invariant {
            reason: "conversation item references a missing message",
        });
    };
    if message.thread_id != thread_id.as_str() {
        return Err(RepositoryError::Invariant {
            reason: "conversation item message belongs to a different thread",
        });
    }
    let text = read_authored_text(database, message_id, &message).await?;
    let rows = image_attachment_rows(database, message_id).await?;
    let mut references = Vec::with_capacity(rows.len());
    for (expected_position, row) in rows.into_iter().enumerate() {
        let index = u32::try_from(expected_position).map_err(|_| {
            RepositoryError::Invariant {
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
        references.push(image_attachment_ref_from_row(
            &row, message_id, thread_id, index,
        )?);
    }
    Ok((text, references))
}

async fn image_attachment_rows(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
) -> Result<Vec<entities::MessageImageAttachment>, RepositoryError> {
    entities::message_image_attachment::Entity::find()
        .filter(entities::message_image_attachment::Column::MessageId.eq(message_id.as_str()))
        .order_by_asc(entities::message_image_attachment::Column::Position)
        .all(database)
        .await
        .map_err(|source| database_error("read message image attachment rows", source))
}

fn validate_image_attachment_row(
    row: &entities::MessageImageAttachment,
) -> Result<(), RepositoryError> {
    if row.size_bytes != i64::try_from(row.bytes.as_slice().len()).unwrap_or(-1) {
        return Err(corrupt_data(
            "message_image_attachments",
            "size_bytes",
            "attachment byte length disagrees with its blob",
        ));
    }
    Ok(())
}

fn image_attachment_ref_from_row(
    row: &entities::MessageImageAttachment,
    message_id: &MessageId,
    thread_id: &ThreadId,
    index: u32,
) -> Result<ImageAttachmentRef, RepositoryError> {
    validate_image_attachment_row(row)?;
    let size_bytes = u32::try_from(row.size_bytes).map_err(|_| {
        corrupt_data(
            "message_image_attachments",
            "size_bytes",
            "attachment size is outside the reference range",
        )
    })?;
    let digest: [u8; 32] = Sha256::digest(row.bytes.as_slice()).into();
    ImageAttachmentRef::new(
        message_id.clone(),
        thread_id.clone(),
        index,
        &row.mime_type,
        row.name.clone(),
        size_bytes,
        digest,
    )
    .map_err(|error| corrupt_data("message_image_attachments", "metadata", &error))
}

/// Reconstructs authored text without collapsing the queue command's optional
/// text field. Legacy first-message rows have no general-message receipt and
/// continue to use their non-null message body as the compatibility fallback.
async fn read_authored_text(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
    message: &entities::Message,
) -> Result<Option<AuthoredText>, RepositoryError> {
    let receipt = entities::command_receipt::Entity::find()
        .filter(entities::command_receipt::Column::MessageId.eq(message_id.as_str()))
        .one(database)
        .await
        .map_err(|source| database_error("read message command receipt", source))?;

    let Some(receipt) = receipt else {
        return authored_text_from_body(message.body.clone());
    };
    if receipt.command_kind != CommandKind::QueueMessage {
        return authored_text_from_body(message.body.clone());
    }
    if receipt.thread_id.as_deref() != Some(message.thread_id.as_str()) {
        return Err(RepositoryError::Invariant {
            reason: "queue receipt and immutable message thread identities disagree",
        });
    }

    let text = receipt
        .body
        .map(|body| AuthoredText::parse(body))
        .transpose()
        .map_err(|error| corrupt_data("command_receipts", "body", &error))?;
    if message.body != text.as_ref().map_or("", AuthoredText::as_str) {
        return Err(RepositoryError::Invariant {
            reason: "queue receipt and immutable message text presence disagree",
        });
    }
    Ok(text)
}

fn authored_text_from_body(body: String) -> Result<Option<AuthoredText>, RepositoryError> {
    if body.is_empty() {
        return Ok(None);
    }
    AuthoredText::parse(body)
        .map(Some)
        .map_err(|error| corrupt_data("messages", "body", &error))
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
        .map_err(|source| database_error("find queue-message thread", source))
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

fn queue_result(input: &QueueMessageInput, disposition: ReceiptDisposition) -> QueueMessageResult {
    QueueMessageResult {
        receipt: CommandReceipt {
            request_id: input.request_id.clone(),
            disposition,
        },
        message_id: input.message_id.clone(),
        thread_id: input.thread_id.clone(),
        payload: input.payload.clone(),
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

fn do_nothing_on_conflict() -> OnConflict {
    let mut conflict = OnConflict::new();
    conflict.do_nothing();
    conflict.clone()
}

async fn rollback_with_error<T>(
    transaction: DatabaseTransaction,
    error: RepositoryError,
) -> Result<T, RepositoryError> {
    transaction
        .rollback()
        .await
        .map_err(|source| database_error("roll back rejected queue-message transaction", source))?;
    Err(error)
}

async fn rollback_with_lookup(
    transaction: DatabaseTransaction,
    result: Result<Option<QueueMessageResult>, RepositoryError>,
) -> Result<QueueMessageResult, RepositoryError> {
    transaction
        .rollback()
        .await
        .map_err(|source| database_error("roll back duplicate queue-message transaction", source))?;
    result?.ok_or(RepositoryError::Invariant {
        reason: "receipt insert was ignored without an identifiable request",
    })
}
