//! Exact byte-bearing payload reads for failed and withdrawn messages.

#![forbid(unsafe_code)]

use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Statement, Value};

use artisan_domain::{AuthoredText, MessageId, QueueMessagePayload, RequestId, ThreadId};

use crate::entities::{self, DispatchState};
use crate::repository::{Repository, corrupt_data, database_error};

use super::QueuedMessageRepositoryError;
use super::rows::{
    authored_text, ensure_thread, parse_message_id, parse_thread_id, queue_receipt_owns_message,
    read_image_attachments,
};

/// Selects the exact withdrawal receipt for one withdrawn message target.
const WITHDRAWN_TARGET_SQL: &str = r"
SELECT 1
FROM queued_message_withdrawals
WHERE thread_id = ?
  AND message_id = ?
  AND original_request_id = ?
  AND outcome = 'withdrawn'
LIMIT 1
";
impl Repository {
    /// Reads the exact immutable payload of one terminally failed dispatch.
    ///
    /// Ownership, receipt agreement, failed state, and withdrawal absence are
    /// all checked: a row that is not a non-withdrawn terminal failure
    /// returns `Ok(None)` without distinguishing which check failed, while
    /// genuinely corrupt durable state is an error. This is the only
    /// byte-bearing seam behind the new-chat recovery action, so attachments
    /// are never silently dropped when the prompt moves to a new thread.
    ///
    /// # Errors
    ///
    /// Returns a typed corruption or database error.
    pub async fn read_failed_message_payload(
        &self,
        thread_id: &ThreadId,
        message_id: &MessageId,
        original_request_id: &RequestId,
    ) -> Result<Option<QueueMessagePayload>, QueuedMessageRepositoryError> {
        ensure_thread(&self.database, thread_id).await?;
        let Some(message) = entities::message::Entity::find_by_id(message_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| database_error("read failed message", source))?
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
                .map_err(|source| database_error("read failed queue receipt", source))?
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
        let Some(dispatch) = entities::message_dispatch::Entity::find_by_id(message_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| database_error("read failed dispatch", source))?
        else {
            return Ok(None);
        };
        if dispatch.state != DispatchState::Failed || dispatch.last_error.is_none() {
            return Ok(None);
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
            .map_err(|source| database_error("read failed-message withdrawal", source))?
            .is_some();
        if withdrawn {
            return Ok(None);
        }

        reconstruct_payload(&self.database, &message, &receipt)
            .await
            .map(Some)
    }

    /// Reads the full typed payload of a successfully withdrawn message.
    ///
    /// The read is ownership checked against the supplied thread, message id,
    /// and original queue request id, and requires a durable `withdrawn`
    /// receipt. It is intentionally separate from [`Self::read_queued_messages`]
    /// so normal composer listing never carries image bytes. Wrong-thread,
    /// missing, mismatched, or not-withdrawn targets return `None`.
    ///
    /// # Errors
    ///
    /// Returns [`QueuedMessageRepositoryError`] when the thread is unknown,
    /// stored rows disagree, or a database read fails.
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
    ///
    /// # Errors
    ///
    /// Inherits [`Self::read_withdrawn_message_payload`]'s failures.
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
