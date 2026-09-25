//! One fresh message admission inside an open write transaction, shared by
//! the queue-message command and draft submission.

use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseTransaction};

use super::{
    QueueMessageInput, RepositoryError, database_error, entities, insert_image_attachments,
    insert_message, insert_queue_receipt, insert_queued_dispatch, millis, next_message_ordinal,
    settings_from_thread, thread_row_by_id,
};

/// Outcome of admitting one fresh message inside an open write transaction.
pub(crate) enum Admission {
    /// Every row was written; the caller commits.
    Admitted,
    /// The message id already exists.
    MessageConflict,
    /// The message already has a dispatch row.
    DispatchConflict,
    /// The request id already has a receipt.
    ReceiptConflict,
}

/// Admits one fresh message inside `transaction`: the authoritative thread
/// read, the accept-time settings snapshot (absence refuses the message
/// rather than queueing it into eternal requeue), and every insert, so a
/// concurrent config save cannot snapshot stale settings. Conflicts are
/// reported, not resolved: each caller keeps its own replay rule.
pub(crate) async fn admit_queue_message(
    transaction: &DatabaseTransaction,
    input: &QueueMessageInput,
) -> Result<Admission, RepositoryError> {
    let thread = thread_row_by_id(transaction, &input.thread_id)
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
    let settings = settings_from_thread(thread.clone())?.ok_or_else(|| {
        RepositoryError::ThreadEngineNotConfigured {
            thread_id: input.thread_id.clone(),
        }
    })?;
    let ordinal = next_message_ordinal(transaction, &input.thread_id).await?;
    if insert_message(transaction, input, ordinal).await? == 0 {
        return Ok(Admission::MessageConflict);
    }
    insert_image_attachments(transaction, input).await?;
    if insert_queued_dispatch(transaction, input).await? == 0 {
        return Ok(Admission::DispatchConflict);
    }
    if insert_queue_receipt(transaction, input, &settings).await? == 0 {
        return Ok(Admission::ReceiptConflict);
    }
    let updated_at_ms = thread.updated_at_ms.max(millis(input.accepted_at));
    let mut updated_thread = entities::thread::ActiveModel::from(thread);
    updated_thread.updated_at_ms = Set(updated_at_ms);
    updated_thread
        .update(transaction)
        .await
        .map_err(|source| database_error("update thread recency", source))?;
    Ok(Admission::Admitted)
}
