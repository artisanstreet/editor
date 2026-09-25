//! Sending a thread's composer draft as a message.
//!
//! One `IMMEDIATE` transaction reads the draft at exactly the submitted
//! revision, admits it through the ordinary queue-message admission (stored
//! attachments resolved to their bytes), records the submission under the
//! thread and revision, and empties the draft at the next revision. A second
//! submission of the same revision answers the first one's message and
//! writes nothing, whatever its request id.

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, EntityTrait, Statement, Value};
use thiserror::Error;

use artisan_domain::{
    CommandReceipt, ComposerDraftRevision, ComposerDraftScope, MessageId, QueueMessagePayload,
    ReceiptDisposition, RequestId, RunId, ThreadId, UnixMillis,
};

use super::composer_draft::{clear_draft, ensure_scope_exists, read_draft, resolve_attachments};
use super::queue_message::{
    Admission, admit_queue_message, queue_result, read_queue_message_payload,
};
use super::{
    ComposerDraftRepositoryError, QueueMessageInput, QueueMessageResult, Repository,
    RepositoryError, corrupt_data, database_error, entities,
};

/// Storage input after the Forge mints the would-be message identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmitComposerDraftInput {
    /// Request identity; becomes the new message's dispatch correlation.
    pub request_id: RequestId,
    /// Thread whose draft is sent.
    pub thread_id: ThreadId,
    /// The draft revision the sender last saved.
    pub draft_revision: ComposerDraftRevision,
    /// Identity for the message, used only when this is the first
    /// submission of the revision.
    pub message_id: MessageId,
    /// Observed live run the message must steer into, if named.
    pub steer_run_id: Option<RunId>,
    /// Authoritative acceptance time.
    pub submitted_at: UnixMillis,
}

/// What a draft submission did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DraftSubmission {
    /// The draft is the queued message in `result`: its receipt is
    /// `Accepted` for the first submission of the revision and `Duplicate`
    /// (carrying the original correlation id) for a repeat. The draft is
    /// empty at `cleared_revision`.
    Queued {
        /// The queued message.
        result: QueueMessageResult,
        /// Revision of the emptied draft.
        cleared_revision: ComposerDraftRevision,
    },
    /// The draft is at another revision (`None`: no draft); nothing changed.
    Stale {
        /// The thread's current draft revision.
        current_revision: Option<ComposerDraftRevision>,
    },
    /// The draft at that revision has neither text nor images.
    Empty,
}

/// Failure of a draft submission.
#[derive(Debug, Error)]
pub enum DraftSubmissionError {
    /// Message admission failed.
    #[error(transparent)]
    Queue(#[from] RepositoryError),
    /// Reading or clearing the draft failed.
    #[error(transparent)]
    Draft(#[from] ComposerDraftRepositoryError),
}

impl Repository {
    /// Sends a thread's composer draft at exactly `input.draft_revision`.
    ///
    /// # Errors
    ///
    /// Returns an unknown-thread, unconfigured-engine, attachment, or
    /// database error. A failed submission changes nothing.
    pub async fn submit_composer_draft(
        &self,
        input: SubmitComposerDraftInput,
    ) -> Result<DraftSubmission, DraftSubmissionError> {
        let transaction = self.begin_write().await.map_err(|source| {
            database_error::<RepositoryError>("begin composer-draft submission", source)
        })?;
        let outcome = submit(&transaction, &input).await;
        let first = matches!(
            &outcome,
            Ok(DraftSubmission::Queued { result, .. })
                if result.receipt.disposition == ReceiptDisposition::Accepted
        );
        if first {
            transaction.commit().await.map_err(|source| {
                database_error::<RepositoryError>("commit composer-draft submission", source)
            })?;
        } else {
            transaction.rollback().await.map_err(|source| {
                database_error::<RepositoryError>("roll back composer-draft submission", source)
            })?;
        }
        outcome
    }
}

async fn submit(
    transaction: &DatabaseTransaction,
    input: &SubmitComposerDraftInput,
) -> Result<DraftSubmission, DraftSubmissionError> {
    let scope = ComposerDraftScope::Thread(input.thread_id.clone());
    ensure_scope_exists(transaction, &scope).await?;
    if let Some((message_id, cleared_revision)) =
        submission(transaction, &input.thread_id, input.draft_revision).await?
    {
        let result = replay(transaction, &input.thread_id, message_id).await?;
        return Ok(DraftSubmission::Queued {
            result,
            cleared_revision,
        });
    }
    let draft = read_draft(transaction, &scope).await?;
    let current_revision = draft.as_ref().map(artisan_domain::ComposerDraft::revision);
    let Some(draft) = draft.filter(|draft| draft.revision() == input.draft_revision) else {
        return Ok(DraftSubmission::Stale { current_revision });
    };
    let images = resolve_attachments(transaction, draft.attachments()).await?;
    let text = (!draft.text().as_str().is_empty()).then(|| draft.text().clone());
    let Ok(payload) = QueueMessagePayload::new(text, images) else {
        return Ok(DraftSubmission::Empty);
    };
    let queue = QueueMessageInput {
        request_id: input.request_id.clone(),
        message_id: input.message_id.clone(),
        thread_id: input.thread_id.clone(),
        payload,
        steer_run_id: input.steer_run_id.clone(),
        accepted_at: input.submitted_at,
    };
    if !matches!(
        admit_queue_message(transaction, &queue).await?,
        Admission::Admitted
    ) {
        return Err(RepositoryError::IdempotencyConflict {
            request_id: input.request_id.clone(),
        }
        .into());
    }
    let cleared_revision = clear_draft(
        transaction,
        &scope,
        input.draft_revision,
        input.submitted_at,
    )
    .await?;
    record_submission(transaction, input, cleared_revision).await?;
    Ok(DraftSubmission::Queued {
        result: queue_result(&queue, ReceiptDisposition::Accepted),
        cleared_revision,
    })
}

async fn submission(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
    draft_revision: ComposerDraftRevision,
) -> Result<Option<(MessageId, ComposerDraftRevision)>, RepositoryError> {
    let table = "composer_draft_submissions";
    let Some(row) = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT message_id, cleared_revision FROM composer_draft_submissions WHERE thread_id = ? AND draft_revision = ?",
            [
                Value::String(Some(thread_id.as_str().to_owned())),
                Value::BigInt(Some(draft_revision.as_i64())),
            ],
        ))
        .await
        .map_err(|source| database_error("read composer-draft submission", source))?
    else {
        return Ok(None);
    };
    let message_id = row
        .try_get_by_index::<String>(0)
        .map_err(|source| corrupt_data(table, "message_id", source))?;
    let message_id =
        MessageId::parse(message_id).map_err(|source| corrupt_data(table, "message_id", source))?;
    let cleared = row
        .try_get_by_index::<i64>(1)
        .map_err(|source| corrupt_data(table, "cleared_revision", source))?;
    let cleared = u64::try_from(cleared)
        .ok()
        .and_then(|value| ComposerDraftRevision::new(value).ok())
        .ok_or_else(|| corrupt_data(table, "cleared_revision", "out of range"))?;
    Ok(Some((message_id, cleared)))
}

async fn record_submission(
    transaction: &DatabaseTransaction,
    input: &SubmitComposerDraftInput,
    cleared_revision: ComposerDraftRevision,
) -> Result<(), RepositoryError> {
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO composer_draft_submissions (thread_id, draft_revision, message_id, cleared_revision, submitted_at_ms) VALUES (?, ?, ?, ?, ?)",
            [
                Value::String(Some(input.thread_id.as_str().to_owned())),
                Value::BigInt(Some(input.draft_revision.as_i64())),
                Value::String(Some(input.message_id.as_str().to_owned())),
                Value::BigInt(Some(cleared_revision.as_i64())),
                Value::BigInt(Some(input.submitted_at.as_millis())),
            ],
        ))
        .await
        .map(|_| ())
        .map_err(|source| database_error("record composer-draft submission", source))
}

/// The message an earlier submission of the same revision queued, as a
/// duplicate carrying its original correlation id and steer target.
async fn replay(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
    message_id: MessageId,
) -> Result<QueueMessageResult, RepositoryError> {
    let missing = || RepositoryError::Invariant {
        reason: "composer-draft submission references a missing message",
    };
    let message = entities::message::Entity::find_by_id(message_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("read submitted message", source))?
        .ok_or_else(missing)?;
    let dispatch = entities::message_dispatch::Entity::find_by_id(message_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("read submitted message dispatch", source))?
        .ok_or_else(missing)?;
    let payload = read_queue_message_payload(database, &message_id)
        .await?
        .ok_or_else(missing)?;
    let request_id = RequestId::parse(dispatch.correlation_id)
        .map_err(|source| corrupt_data("message_dispatches", "correlation_id", source))?;
    let steer_run_id = match dispatch.steer_run_id.as_deref() {
        None | Some("") => None,
        Some(run_id) => Some(
            RunId::parse(run_id.to_owned())
                .map_err(|source| corrupt_data("message_dispatches", "steer_run_id", source))?,
        ),
    };
    Ok(QueueMessageResult {
        receipt: CommandReceipt {
            request_id,
            disposition: ReceiptDisposition::Duplicate,
        },
        message_id,
        thread_id: thread_id.clone(),
        payload,
        steer_run_id,
        queued_at: UnixMillis::from_millis(message.accepted_at_ms),
    })
}
