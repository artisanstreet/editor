//! Shared row mapping, identifier parsing, and image-attachment reads.

#![forbid(unsafe_code)]

use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use sha2::{Digest, Sha256};

use artisan_domain::{
    AuthoredText, ImageAttachment, ImageAttachmentRef, MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
    MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES, MessageId, RequestId, ThreadId,
};

use crate::entities::{self, CommandKind};
use crate::repository::{corrupt_data, database_error};

use super::QueuedMessageRepositoryError;
pub(super) async fn read_image_refs(
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

pub(super) async fn read_image_attachments(
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

pub(super) async fn ensure_thread(
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

pub(super) fn queue_receipt_owns_message(
    receipt: &entities::CommandReceipt,
    thread_id: &ThreadId,
    message_id: &MessageId,
) -> bool {
    receipt.command_kind == CommandKind::QueueMessage
        && receipt.thread_id.as_deref() == Some(thread_id.as_str())
        && receipt.message_id.as_deref() == Some(message_id.as_str())
}

pub(super) fn authored_text(
    body: Option<String>,
) -> Result<Option<AuthoredText>, QueuedMessageRepositoryError> {
    body.map(AuthoredText::parse)
        .transpose()
        .map_err(|error| corrupt_data("command_receipts", "body", error))
}

pub(super) fn parse_thread_id(value: String) -> Result<ThreadId, QueuedMessageRepositoryError> {
    ThreadId::parse(value).map_err(|error| corrupt_data("threads", "thread_id", error))
}

pub(super) fn parse_message_id(value: String) -> Result<MessageId, QueuedMessageRepositoryError> {
    MessageId::parse(value).map_err(|error| corrupt_data("messages", "message_id", error))
}

pub(super) fn parse_request_id(value: String) -> Result<RequestId, QueuedMessageRepositoryError> {
    RequestId::parse(value).map_err(|error| corrupt_data("command_receipts", "request_id", error))
}
