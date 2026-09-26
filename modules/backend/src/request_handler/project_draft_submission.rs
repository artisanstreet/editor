//! Sending a project's new-task composer draft: the submission creates the
//! thread its message is queued in.
//!
//! The Forge admits the send exactly like a thread draft's, with no saved
//! configuration and no live run: the selection the send carries is
//! resolved against the catalog served for the project, must be runnable,
//! and becomes the new thread's configuration (and the default); without a
//! selection the thread starts from the default configuration, and without
//! one of those the send is refused. The repository then creates the
//! configured thread, queues the draft as its first message, records the
//! submission under the project and revision, and empties the draft in one
//! transaction. A repeated revision answers the first submission's thread
//! and message and creates nothing.

use artisan_database::{
    DraftSubmissionError, ProjectDraftSubmission, QueueMessageResult, SubmitProjectDraftInput,
};
use artisan_domain::{
    ComposerDraftRevision, DraftSubmissionOutcome, EngineRunConfig, MessageId, ProjectId,
    RequestId, SubmissionRefusal, SubmitComposerDraft, ThreadId, ThreadTitle,
};
use artisan_protocol::{ErrorCode, ProtocolFailure, ServerResponse};

use crate::composer_catalog_handler::CatalogSubject;

use super::composer_drafts::draft_failure;
use super::draft_submission::{empty_draft_failure, submitted};
use super::failures::{repository_failure, typed_failure};
use super::model_selection::{SubmissionPlan, plan_submission};
use super::{
    RequestHandler, forged_identity_failure, origin_clock_failure, origin_entropy_failure,
};

/// Title of the thread a new task's first message creates; the listing
/// shows the first message until the harness names the thread.
const NEW_TASK_TITLE: &str = "New task";

/// How a new task's first message is admitted.
enum NewTaskAdmission {
    /// Create the thread with `config`; `chosen` when the user's selection
    /// resolved to it, so it becomes the default.
    Admitted {
        config: EngineRunConfig,
        chosen: bool,
    },
    /// Refuse the send; nothing is created.
    Refused(SubmissionRefusal),
}

impl RequestHandler {
    /// Creates the project's new thread from its draft at the named
    /// revision, or answers the thread and message an earlier submission of
    /// that revision created.
    pub(super) async fn submit_project_draft(
        &self,
        request_id: &RequestId,
        project: &ProjectId,
        submit: &SubmitComposerDraft,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if let Some(replay) = self
            .repository
            .replay_project_draft_submission(project, submit.draft_revision)
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return self
                .answer_project_submission(request_id, submit, replay)
                .await;
        }
        let (config, chosen) = match self.admit_new_task(project, submit).await {
            NewTaskAdmission::Admitted { config, chosen } => (config, chosen),
            NewTaskAdmission::Refused(refusal) => {
                let refused = DraftSubmissionOutcome::Refused(refusal);
                return Ok(submitted(request_id, submit, refused));
            }
        };
        let engine = config.selection().engine_id();
        let images = match self
            .fitted_draft_images(request_id, &submit.scope, submit.draft_revision, engine)
            .await?
        {
            Ok(images) => images,
            Err(refusal) => {
                let refused = DraftSubmissionOutcome::Refused(refusal);
                return Ok(submitted(request_id, submit, refused));
            }
        };
        let (thread_id, message_id) = self.mint_thread_and_message(request_id)?;
        let submitted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let submission = self
            .repository
            .submit_project_draft(SubmitProjectDraftInput {
                request_id: submit.request_id.clone(),
                project_id: project.clone(),
                draft_revision: submit.draft_revision,
                thread_id,
                title: ThreadTitle::parse(NEW_TASK_TITLE).expect("the new-task title is valid"),
                config: config.clone(),
                message_id,
                images,
                submitted_at,
            })
            .await
            .map_err(|error| match error {
                DraftSubmissionError::Queue(error) => repository_failure(&error, request_id),
                DraftSubmissionError::Draft(error) => draft_failure(&error, request_id),
            })?;
        let first = matches!(
            &submission,
            ProjectDraftSubmission::Queued { result, .. }
                if result.receipt.disposition == artisan_domain::ReceiptDisposition::Accepted
        );
        if first && chosen {
            // The selection a send saves is the user's latest choice.
            self.remember_default_engine_config(&config).await;
        }
        self.answer_project_submission(request_id, submit, submission)
            .await
    }

    /// Admits a new task's first message: the configuration its thread
    /// starts with, or the refusal.
    async fn admit_new_task(
        &self,
        project: &ProjectId,
        submit: &SubmitComposerDraft,
    ) -> NewTaskAdmission {
        let resolved = match &submit.selection {
            Some(selection) => Some(
                self.resolve_selection(CatalogSubject::Project(project), selection, None)
                    .await,
            ),
            None => {
                let default = self
                    .repository
                    .read_user_preferences()
                    .await
                    .ok()
                    .and_then(|preferences| preferences.default_engine_config);
                if let Some(config) = default {
                    return NewTaskAdmission::Admitted {
                        config,
                        chosen: false,
                    };
                }
                None
            }
        };
        // No saved configuration and no live run: a selection must be
        // runnable, and none (without a default) is refused.
        let plan = plan_submission(None, None, resolved, |engine| {
            self.account_usage
                .as_ref()
                .and_then(|usage| usage.readiness(engine))
        });
        match plan {
            SubmissionPlan::Refuse(refusal) => NewTaskAdmission::Refused(refusal),
            SubmissionPlan::Admit {
                save: Some(config), ..
            } => NewTaskAdmission::Admitted {
                config: *config,
                chosen: true,
            },
            SubmissionPlan::Admit { save: None, .. } => {
                unreachable!("an admitted selection on an unconfigured thread is always saved")
            }
        }
    }

    fn mint_thread_and_message(
        &self,
        request_id: &RequestId,
    ) -> Result<(ThreadId, MessageId), ProtocolFailure> {
        let thread = self
            .origin
            .mint_identity()
            .map_err(|error| origin_entropy_failure(&error, request_id))?;
        let message = self
            .origin
            .mint_identity()
            .map_err(|error| origin_entropy_failure(&error, request_id))?;
        Ok((
            ThreadId::parse(thread).map_err(|_| forged_identity_failure("thread", request_id))?,
            MessageId::parse(message)
                .map_err(|_| forged_identity_failure("message", request_id))?,
        ))
    }

    async fn answer_project_submission(
        &self,
        request_id: &RequestId,
        submit: &SubmitComposerDraft,
        submission: ProjectDraftSubmission,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let (result, cleared_revision): (QueueMessageResult, ComposerDraftRevision) =
            match submission {
                ProjectDraftSubmission::Queued {
                    result,
                    cleared_revision,
                } => (result, cleared_revision),
                ProjectDraftSubmission::Stale { current_revision } => {
                    return Ok(submitted(
                        request_id,
                        submit,
                        DraftSubmissionOutcome::Stale { current_revision },
                    ));
                }
                ProjectDraftSubmission::Empty => return Err(empty_draft_failure(request_id)),
            };
        if result.steer_run_id.is_some() {
            return Err(typed_failure(
                ErrorCode::Internal,
                "a new task's first message cannot steer a run",
                false,
                request_id,
            ));
        }
        let engine_config_revision = self
            .engine_config_revision(request_id, &result.thread_id)
            .await?;
        Ok(submitted(
            request_id,
            submit,
            DraftSubmissionOutcome::Queued {
                thread_id: result.thread_id,
                message_id: result.message_id,
                disposition: result.receipt.disposition,
                cleared_revision,
                engine_config_revision,
            },
        ))
    }
}

#[cfg(test)]
#[path = "../../../../tests/backend/project_draft_submission.rs"]
mod tests;
