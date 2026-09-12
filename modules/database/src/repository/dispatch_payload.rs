//! Read-only loading of the persisted payload needed to execute one message.

use sea_orm::{ConnectionTrait, EntityTrait};

use artisan_domain::{MessageBody, MessageId, QueueMessagePayload, RequestId, RunId, ThreadId};

use crate::entities;

use super::{Repository, RepositoryError, corrupt_data, database_error};

/// The immutable execution payload persisted for one durably queued message.
///
/// Dispatch reads this snapshot to execute a message. It carries no claim or
/// lease state because reading it never observes or mutates the dispatch
/// lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageDispatchPayload {
    /// Forge-minted identity of the dispatched message.
    pub message_id: MessageId,
    /// Thread the dispatched message is queued on.
    pub thread_id: ThreadId,
    /// Client request identity whose acceptance produced the message.
    pub correlation_id: RequestId,
    /// Validated body exactly as accepted.
    pub body: MessageBody,
}

/// Immutable execution payload for a general text/image message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueMessageDispatchPayload {
    /// Forge-minted identity of the dispatched message.
    pub message_id: MessageId,
    /// Thread the dispatched message is queued on.
    pub thread_id: ThreadId,
    /// Client request identity whose acceptance produced the message.
    pub correlation_id: RequestId,
    /// Authored text and ordered owned image attachments.
    pub payload: QueueMessagePayload,
    /// Observed live run the message must steer into, if named at accept.
    /// Intent only: dispatch revalidates liveness and the same-engine
    /// rule; explicit-target failures fail typed, never fresh-run.
    pub steer_target: Option<artisan_domain::SteerTarget>,
}

impl Repository {
    /// Loads the immutable dispatch payload for one message without touching
    /// claim or lease state.
    ///
    /// Every persisted field is re-validated against the domain contract, and
    /// the payload stays readable in any dispatch state — queued, leased, or
    /// terminal — because executing a claimed message needs it after the
    /// claim has already mutated lifecycle columns.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError::CorruptData`] when a persisted field
    /// violates its domain contract, [`RepositoryError::Invariant`] when a
    /// dispatch row lost its paired immutable message row, or
    /// [`RepositoryError::Database`] when the read fails.
    pub async fn read_message_dispatch_payload(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<MessageDispatchPayload>, RepositoryError> {
        let Some(dispatch) = dispatch_row_by_message_id(&self.database, message_id).await? else {
            return Ok(None);
        };
        let Some(message) = message_row_by_id(&self.database, message_id).await? else {
            return Err(RepositoryError::Invariant {
                reason: "message dispatch references a missing message",
            });
        };

        let correlation_id = RequestId::parse(dispatch.correlation_id)
            .map_err(|error| corrupt_data("message_dispatches", "correlation_id", error))?;
        let thread_id = ThreadId::parse(message.thread_id)
            .map_err(|error| corrupt_data("messages", "thread_id", error))?;
        let body = MessageBody::parse(message.body)
            .map_err(|error| corrupt_data("messages", "body", error))?;

        Ok(Some(MessageDispatchPayload {
            message_id: message_id.clone(),
            thread_id,
            correlation_id,
            body,
        }))
    }

    /// Loads the immutable general text/image dispatch payload without
    /// touching claim or lease state.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] when the dispatch references a missing
    /// message or payload, a stored identifier or body fails validation, or a
    /// database query fails.
    pub async fn read_queue_message_dispatch_payload(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<QueueMessageDispatchPayload>, RepositoryError> {
        let Some(dispatch) = dispatch_row_by_message_id(&self.database, message_id).await? else {
            return Ok(None);
        };
        let Some(message) = message_row_by_id(&self.database, message_id).await? else {
            return Err(RepositoryError::Invariant {
                reason: "message dispatch references a missing message",
            });
        };

        let correlation_id = RequestId::parse(dispatch.correlation_id)
            .map_err(|error| corrupt_data("message_dispatches", "correlation_id", error))?;
        let thread_id = ThreadId::parse(message.thread_id)
            .map_err(|error| corrupt_data("messages", "thread_id", error))?;
        let payload = super::queue_message::read_queue_message_payload(
            &self.database,
            message_id,
        )
        .await?
        .ok_or(RepositoryError::Invariant {
            reason: "message dispatch references a missing message payload",
        })?;
        let steer_target = match dispatch.steer_run_id.as_deref() {
            None | Some("") => None,
            Some(steer_run_id) => Some(
                artisan_domain::SteerTarget::new(
                    RunId::parse(steer_run_id.to_owned()).map_err(|error| {
                        corrupt_data("message_dispatches", "steer_run_id", error)
                    })?,
                ),
            ),
        };

        Ok(Some(QueueMessageDispatchPayload {
            message_id: message_id.clone(),
            thread_id,
            correlation_id,
            payload,
            steer_target,
        }))
    }
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

async fn message_row_by_id(
    database: &impl ConnectionTrait,
    message_id: &MessageId,
) -> Result<Option<entities::Message>, RepositoryError> {
    entities::message::Entity::find_by_id(message_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find message by id", source))
}
