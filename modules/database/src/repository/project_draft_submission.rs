//! Sending a project's new-task composer draft: the submission creates the
//! thread its message is queued in.
//!
//! One `IMMEDIATE` transaction reads the project draft at exactly the
//! submitted revision, creates the thread already configured with the
//! engine configuration the Forge admitted, admits the draft as the thread's
//! first message through the ordinary queue-message admission, records the
//! submission under the project and revision, and empties the draft at the
//! next revision. A second submission of the same revision answers the first
//! one's thread and message and writes nothing, whatever its request id.

use sea_orm::{
    ActiveValue::Set, ConnectionTrait, DatabaseTransaction, DbBackend, EntityTrait, Statement,
    Value,
};

use artisan_domain::{
    ComposerDraftRevision, ComposerDraftScope, EngineRunConfig, ImageAttachment, MessageId,
    ProjectId, QueueMessagePayload, ReceiptDisposition, RequestId, ThreadId, ThreadTitle,
    UnixMillis,
};

use crate::entities::{self, OpaqueBytes};

use super::composer_draft::{clear_draft, ensure_scope_exists, read_draft};
use super::draft_submission::replay;
use super::queue_message::{Admission, admit_queue_message, queue_result};
use super::thread_engine_config::encode_config;
use super::{
    DraftSubmissionError, QueueMessageInput, QueueMessageResult, Repository, RepositoryError,
    corrupt_data, database_error, millis,
};

/// Storage input after the Forge admitted the send and minted the would-be
/// thread and message identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmitProjectDraftInput {
    /// Request identity; becomes the new message's dispatch correlation.
    pub request_id: RequestId,
    /// Project whose new-task draft is sent.
    pub project_id: ProjectId,
    /// The draft revision the sender last saved.
    pub draft_revision: ComposerDraftRevision,
    /// Identity for the created thread, used only when this is the first
    /// submission of the revision.
    pub thread_id: ThreadId,
    /// The created thread's placeholder title.
    pub title: ThreadTitle,
    /// The engine configuration the created thread starts with.
    pub config: EngineRunConfig,
    /// Identity for the message, used only for a first submission.
    pub message_id: MessageId,
    /// The draft's images as the Forge fitted them to the admitted engine,
    /// in authored order; unused when the revision was already submitted.
    pub images: Vec<ImageAttachment>,
    /// Authoritative acceptance time of the thread and its message.
    pub submitted_at: UnixMillis,
}

/// What a project draft submission did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectDraftSubmission {
    /// The draft is the queued first message in `result` of the created
    /// thread `result.thread_id`: `Accepted` for the first submission of the
    /// revision, `Duplicate` for a repeat. The draft is empty at
    /// `cleared_revision`.
    Queued {
        /// The queued message and the thread it is queued in.
        result: QueueMessageResult,
        /// Revision of the emptied draft.
        cleared_revision: ComposerDraftRevision,
    },
    /// The draft is at another revision (`None`: no draft); nothing changed.
    Stale {
        /// The project's current draft revision.
        current_revision: Option<ComposerDraftRevision>,
    },
    /// The draft at that revision has neither text nor images.
    Empty,
}

impl Repository {
    /// The first submission of this revision of the project's draft, as a
    /// duplicate, when there was one: a repeat answers it instead of being
    /// admitted anew.
    ///
    /// # Errors
    ///
    /// Returns a database or corrupt-data error.
    pub async fn replay_project_draft_submission(
        &self,
        project_id: &ProjectId,
        draft_revision: ComposerDraftRevision,
    ) -> Result<Option<ProjectDraftSubmission>, RepositoryError> {
        let Some(recorded) = submission(&self.database, project_id, draft_revision).await? else {
            return Ok(None);
        };
        let result = replay(&self.database, &recorded.thread_id, recorded.message_id).await?;
        Ok(Some(ProjectDraftSubmission::Queued {
            result,
            cleared_revision: recorded.cleared_revision,
        }))
    }

    /// Sends a project's new-task draft at exactly `input.draft_revision`,
    /// creating the thread it is queued in.
    ///
    /// # Errors
    ///
    /// Returns an unknown-project, attachment, conflict, or database error.
    /// A failed submission changes nothing.
    pub async fn submit_project_draft(
        &self,
        input: SubmitProjectDraftInput,
    ) -> Result<ProjectDraftSubmission, DraftSubmissionError> {
        let transaction = self.begin_write().await.map_err(|source| {
            database_error::<RepositoryError>("begin project-draft submission", source)
        })?;
        let outcome = submit(&transaction, &input).await;
        let first = matches!(
            &outcome,
            Ok(ProjectDraftSubmission::Queued { result, .. })
                if result.receipt.disposition == ReceiptDisposition::Accepted
        );
        if first {
            transaction.commit().await.map_err(|source| {
                database_error::<RepositoryError>("commit project-draft submission", source)
            })?;
        } else {
            transaction.rollback().await.map_err(|source| {
                database_error::<RepositoryError>("roll back project-draft submission", source)
            })?;
        }
        outcome
    }
}

async fn submit(
    transaction: &DatabaseTransaction,
    input: &SubmitProjectDraftInput,
) -> Result<ProjectDraftSubmission, DraftSubmissionError> {
    let scope = ComposerDraftScope::Project(input.project_id.clone());
    ensure_scope_exists(transaction, &scope).await?;
    if let Some(recorded) = submission(transaction, &input.project_id, input.draft_revision).await?
    {
        let result = replay(transaction, &recorded.thread_id, recorded.message_id).await?;
        return Ok(ProjectDraftSubmission::Queued {
            result,
            cleared_revision: recorded.cleared_revision,
        });
    }
    let draft = read_draft(transaction, &scope).await?;
    let current_revision = draft.as_ref().map(artisan_domain::ComposerDraft::revision);
    let Some(draft) = draft.filter(|draft| draft.revision() == input.draft_revision) else {
        return Ok(ProjectDraftSubmission::Stale { current_revision });
    };
    // The revision names the draft's content, so the images the Forge fitted
    // from this revision are its attachments, in order.
    if input.images.len() != draft.attachments().len() {
        return Err(RepositoryError::Invariant {
            reason: "fitted images do not match the submitted draft's attachments",
        }
        .into());
    }
    let text = (!draft.text().as_str().is_empty()).then(|| draft.text().clone());
    let Ok(payload) = QueueMessagePayload::new(text, input.images.clone()) else {
        return Ok(ProjectDraftSubmission::Empty);
    };
    if insert_configured_thread(transaction, input).await? == 0 {
        return Err(RepositoryError::ThreadConflict {
            thread_id: input.thread_id.clone(),
        }
        .into());
    }
    let queue = QueueMessageInput {
        request_id: input.request_id.clone(),
        message_id: input.message_id.clone(),
        thread_id: input.thread_id.clone(),
        payload,
        steer_run_id: None,
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
    Ok(ProjectDraftSubmission::Queued {
        result: queue_result(&queue, ReceiptDisposition::Accepted),
        cleared_revision,
    })
}

/// Creates the thread already configured at its first configuration
/// revision, so the message admission snapshots that configuration.
async fn insert_configured_thread(
    transaction: &DatabaseTransaction,
    input: &SubmitProjectDraftInput,
) -> Result<u64, RepositoryError> {
    let encoded = encode_config(&input.config)?;
    entities::thread::Entity::insert(entities::thread::ActiveModel {
        thread_id: Set(input.thread_id.as_str().to_owned()),
        project_id: Set(input.project_id.as_str().to_owned()),
        title: Set(input.title.as_str().to_owned()),
        created_at_ms: Set(millis(input.submitted_at)),
        updated_at_ms: Set(millis(input.submitted_at)),
        engine_run_config_version: Set(Some(i64::from(input.config.storage_codec_version()))),
        engine_run_config_revision: Set(1),
        engine_run_config: Set(Some(OpaqueBytes::new(encoded))),
    })
    .on_conflict(
        sea_orm::sea_query::OnConflict::new()
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(transaction)
    .await
    .map_err(|source| database_error("create submitted project draft thread", source))
}

/// A recorded project draft submission.
struct Recorded {
    thread_id: ThreadId,
    message_id: MessageId,
    cleared_revision: ComposerDraftRevision,
}

async fn submission(
    database: &impl ConnectionTrait,
    project_id: &ProjectId,
    draft_revision: ComposerDraftRevision,
) -> Result<Option<Recorded>, RepositoryError> {
    let table = "project_draft_submissions";
    let Some(row) = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT thread_id, message_id, cleared_revision FROM project_draft_submissions WHERE project_id = ? AND draft_revision = ?",
            [
                Value::String(Some(project_id.as_str().to_owned())),
                Value::BigInt(Some(draft_revision.as_i64())),
            ],
        ))
        .await
        .map_err(|source| database_error("read project-draft submission", source))?
    else {
        return Ok(None);
    };
    let thread_id = row
        .try_get_by_index::<String>(0)
        .map_err(|source| corrupt_data(table, "thread_id", source))?;
    let thread_id =
        ThreadId::parse(thread_id).map_err(|source| corrupt_data(table, "thread_id", source))?;
    let message_id = row
        .try_get_by_index::<String>(1)
        .map_err(|source| corrupt_data(table, "message_id", source))?;
    let message_id =
        MessageId::parse(message_id).map_err(|source| corrupt_data(table, "message_id", source))?;
    let cleared = row
        .try_get_by_index::<i64>(2)
        .map_err(|source| corrupt_data(table, "cleared_revision", source))?;
    let cleared_revision = u64::try_from(cleared)
        .ok()
        .and_then(|value| ComposerDraftRevision::new(value).ok())
        .ok_or_else(|| corrupt_data(table, "cleared_revision", "out of range"))?;
    Ok(Some(Recorded {
        thread_id,
        message_id,
        cleared_revision,
    }))
}

async fn record_submission(
    transaction: &DatabaseTransaction,
    input: &SubmitProjectDraftInput,
    cleared_revision: ComposerDraftRevision,
) -> Result<(), RepositoryError> {
    transaction
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO project_draft_submissions (project_id, draft_revision, thread_id, message_id, cleared_revision, submitted_at_ms) VALUES (?, ?, ?, ?, ?, ?)",
            [
                Value::String(Some(input.project_id.as_str().to_owned())),
                Value::BigInt(Some(input.draft_revision.as_i64())),
                Value::String(Some(input.thread_id.as_str().to_owned())),
                Value::String(Some(input.message_id.as_str().to_owned())),
                Value::BigInt(Some(cleared_revision.as_i64())),
                Value::BigInt(Some(input.submitted_at.as_millis())),
            ],
        ))
        .await
        .map(|_| ())
        .map_err(|source| database_error("record project-draft submission", source))
}
