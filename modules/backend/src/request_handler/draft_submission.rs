//! Sending a thread's composer draft as a message.
//!
//! The Forge first admits the send (see [`super::model_selection`]): it may
//! refuse it with a typed reason, save the configuration the send's
//! selection resolves to, and decide whether it steers the live run. The
//! repository then queues the draft at exactly the named revision, records
//! the submission, and empties the draft in one transaction; a repeated
//! submission of the revision answers the first one's message. Steers settle
//! exactly like a queued message's: a first submission routes into the live
//! run, and a repeat settles from the durable delivery state.

use artisan_database::{DraftSubmission, DraftSubmissionError, SubmitComposerDraftInput};
use artisan_domain::{
    ComposerDraftScope, ComposerDraftSubmitted, DraftSubmissionOutcome, ImageAttachment, MessageId,
    QueueMessage, ReceiptDisposition, RequestId, RunId, SubmissionRefusal, SubmissionRefusalKind,
    SubmitComposerDraft,
};
use artisan_protocol::{
    ErrorCode, ProtocolFailure, QueueMessageReceipt, ResponsePayload, ServerResponse,
};

use super::composer_drafts::draft_failure;
use super::failures::{outcome, repository_failure, typed_failure};
use super::model_selection::SubmissionAdmission;
use super::{
    RequestHandler, forged_identity_failure, origin_clock_failure, origin_entropy_failure,
};

impl RequestHandler {
    /// Queues the thread's draft at the named revision, or answers the
    /// message an earlier submission of that revision queued.
    pub(super) async fn submit_composer_draft_outcome(
        &self,
        request_id: &RequestId,
        submit: &SubmitComposerDraft,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let (steer_run_id, images) = match self.prepare_submission(request_id, submit).await? {
            Ok(prepared) => prepared,
            Err(refusal) => {
                let refused = DraftSubmissionOutcome::Refused(refusal);
                return Ok(submitted(request_id, submit, refused));
            }
        };
        let identity = self
            .origin
            .mint_identity()
            .map_err(|error| origin_entropy_failure(&error, request_id))?;
        let submitted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let message_id = MessageId::parse(identity)
            .map_err(|_| forged_identity_failure("message", request_id))?;
        let submission = self
            .repository
            .submit_composer_draft(SubmitComposerDraftInput {
                request_id: submit.request_id.clone(),
                thread_id: submit.thread_id.clone(),
                draft_revision: submit.draft_revision,
                message_id,
                steer_run_id,
                images,
                submitted_at,
            })
            .await
            .map_err(|error| match error {
                DraftSubmissionError::Queue(error) => repository_failure(&error, request_id),
                DraftSubmissionError::Draft(error) => draft_failure(&error, request_id),
            })?;
        let (result, cleared_revision) = match submission {
            DraftSubmission::Queued {
                result,
                cleared_revision,
            } => (result, cleared_revision),
            DraftSubmission::Stale { current_revision } => {
                return Ok(submitted(
                    request_id,
                    submit,
                    DraftSubmissionOutcome::Stale { current_revision },
                ));
            }
            DraftSubmission::Empty => {
                return Err(typed_failure(
                    ErrorCode::InvalidInput,
                    "composer draft has neither text nor images",
                    false,
                    request_id,
                ));
            }
        };
        // The message's own correlation id and stored steer target settle
        // the steer, exactly as a replayed queue-message request would.
        let mut queue = QueueMessage::new(
            result.receipt.request_id.clone(),
            result.thread_id.clone(),
            result.payload.clone(),
        );
        if let Some(run_id) = &result.steer_run_id {
            queue = queue.with_steer_target(artisan_domain::SteerTarget::new(run_id.clone()));
        }
        let disposition = result.receipt.disposition;
        let settled = if disposition == ReceiptDisposition::Accepted {
            let receipt = QueueMessageReceipt {
                request_id: result.receipt.request_id.clone(),
                message_id: result.message_id.clone(),
                thread_id: result.thread_id.clone(),
                disposition,
            };
            self.deliver_accepted_steer(request_id, &queue, &result.message_id, receipt)
                .await
        } else {
            self.settle_replayed_steer(request_id, &queue, &result)
                .await
        }?;
        if !matches!(settled.payload, ResponsePayload::MessageQueued(_)) {
            return Ok(settled);
        }
        let engine_config_revision = self
            .engine_config_revision(request_id, &submit.thread_id)
            .await?;
        Ok(submitted(
            request_id,
            submit,
            DraftSubmissionOutcome::Queued {
                message_id: result.message_id,
                disposition,
                cleared_revision,
                engine_config_revision,
            },
        ))
    }
}

impl RequestHandler {
    /// Admits the send and fits its images to the admitted engine: the live
    /// run it steers into and the fitted images, or the refusal.
    async fn prepare_submission(
        &self,
        request_id: &RequestId,
        submit: &SubmitComposerDraft,
    ) -> Result<Result<(Option<RunId>, Vec<ImageAttachment>), SubmissionRefusal>, ProtocolFailure>
    {
        let (steer_run_id, engine) = match self.admit_submission(request_id, submit).await? {
            SubmissionAdmission::Admitted {
                steer_run_id,
                engine,
            } => (steer_run_id, engine),
            SubmissionAdmission::Refused(refusal) => return Ok(Err(refusal)),
        };
        let Some(engine) = engine else {
            return Ok(Ok((steer_run_id, Vec::new())));
        };
        Ok(self
            .fitted_draft_images(request_id, submit, engine)
            .await?
            .map(|images| (steer_run_id, images)))
    }

    /// The submitted revision's picked images fitted to `engine`, in order,
    /// or the refusal naming the image the engine cannot take. A draft at
    /// another revision fits nothing: the repository answers it as stale.
    async fn fitted_draft_images(
        &self,
        request_id: &RequestId,
        submit: &SubmitComposerDraft,
        engine: artisan_domain::EngineId,
    ) -> Result<Result<Vec<ImageAttachment>, SubmissionRefusal>, ProtocolFailure> {
        let scope = ComposerDraftScope::Thread(submit.thread_id.clone());
        let draft = self
            .repository
            .read_composer_draft(&scope)
            .await
            .map_err(|error| draft_failure(&error, request_id))?;
        let Some(draft) = draft.filter(|draft| draft.revision() == submit.draft_revision) else {
            return Ok(Ok(Vec::new()));
        };
        if draft.attachments().is_empty() {
            return Ok(Ok(Vec::new()));
        }
        let picked = self.read_picked(request_id, draft.attachments()).await?;
        let fitted = self.fit_picked(request_id, engine, picked).await?;
        Ok(fitted.map_err(|reason| {
            SubmissionRefusal::new(
                SubmissionRefusalKind::AttachmentRejected,
                format!(
                    "{reason} Your draft is preserved; remove or replace the image and send again."
                ),
            )
            .unwrap_or_else(|_| {
                SubmissionRefusal::new(
                    SubmissionRefusalKind::AttachmentRejected,
                    "An attached image cannot be sent to this engine. Your draft is preserved.",
                )
                .expect("the fallback refusal message is valid")
            })
        }))
    }
}

fn submitted(
    request_id: &RequestId,
    submit: &SubmitComposerDraft,
    outcome_value: DraftSubmissionOutcome,
) -> ServerResponse {
    outcome(
        request_id,
        ResponsePayload::ComposerDraftSubmitted(ComposerDraftSubmitted {
            request_id: request_id.clone(),
            thread_id: submit.thread_id.clone(),
            draft_revision: submit.draft_revision,
            outcome: outcome_value,
        }),
    )
}
