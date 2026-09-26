//! Forge-owned submissions after acceptance: retrying a failed message,
//! moving it into a new thread, and waking outbox delivery after every
//! command that changes a thread's undelivered messages.
//!
//! The Forge holds the only copy of an accepted payload. A retry requeues
//! the stored dispatch; a recovery creates a thread in the same project with
//! the failed message's engine configuration and stores the payload as that
//! thread's composer draft. Neither is ever sent on the user's behalf.

use artisan_domain::{
    Command, CreateThread, EngineConfigUpdatePrecondition, FailedMessageRecovered,
    FailedMessageRetried, ReceiptDisposition, RecoverFailedMessage, RequestId, RetryFailedMessage,
    SetThreadEngineConfig, ThreadId, ThreadTitle,
};
use artisan_protocol::{ErrorCode, ProtocolFailure, ResponsePayload, ServerResponse};

use super::composer_state_handler::queue_failure;
use super::failures::{outcome, repository_failure, typed_failure};
use super::{RequestHandler, origin_clock_failure};

/// Title of the thread a recovered prompt moves into, matching a new task.
const RECOVERED_THREAD_TITLE: &str = "New task";

impl RequestHandler {
    /// Wakes subscription delivery for the thread a successful command
    /// changed, so its subscribers receive the new message outbox.
    pub(super) fn wake_message_outbox(&self, command: &Command) {
        let thread_id = match command {
            Command::QueueFirstMessage(queue) => &queue.thread_id,
            Command::QueueMessage(queue) => &queue.thread_id,
            Command::QueueStoredMessage(queue) => queue.thread_id(),
            Command::WithdrawQueuedMessage(withdraw) => &withdraw.thread_id,
            Command::RetryFailedMessage(retry) => &retry.target.thread_id,
            Command::RecoverFailedMessage(recover) => &recover.target.thread_id,
            // A new task's thread has no subscriber yet.
            Command::SubmitComposerDraft(submit) => match &submit.scope {
                artisan_domain::ComposerDraftScope::Thread(thread_id) => thread_id,
                artisan_domain::ComposerDraftScope::Project(_) => return,
            },
            _ => return,
        };
        if let Some(notifier) = &self.conversation_commit_notifier {
            let _ = notifier.publish(thread_id);
        }
    }

    /// Requeues one failed message from its stored payload.
    pub(super) async fn retry_failed_message_outcome(
        &self,
        request_id: &RequestId,
        retry: &RetryFailedMessage,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let retried_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let retry_outcome = self
            .repository
            .retry_failed_message(&retry.target, retried_at)
            .await
            .map_err(|error| queue_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::FailedMessageRetried(FailedMessageRetried {
                request_id: request_id.clone(),
                target: retry.target.clone(),
                outcome: retry_outcome,
            }),
        ))
    }

    /// Moves one failed message into a new thread of its project.
    ///
    /// Every step is idempotent under the recovery's request id: the thread
    /// is created under that id (a replay finds it), the engine
    /// configuration is copied under a derived id, and the recovery record
    /// is written last, so a replay after an interrupted recovery finishes
    /// it and a replay after a completed one answers the same thread.
    pub(super) async fn recover_failed_message_outcome(
        &self,
        request_id: &RequestId,
        recover: &RecoverFailedMessage,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let target = &recover.target;
        let answer = |new_thread_id: Option<ThreadId>, disposition| {
            outcome(
                request_id,
                ResponsePayload::FailedMessageRecovered(FailedMessageRecovered {
                    request_id: request_id.clone(),
                    target: target.clone(),
                    new_thread_id,
                    disposition,
                }),
            )
        };
        if let Some(recovery) = self
            .repository
            .failed_message_recovery(&target.message_id)
            .await
            .map_err(|error| queue_failure(&error, request_id))?
        {
            return Ok(answer(
                Some(recovery.new_thread_id),
                ReceiptDisposition::Duplicate,
            ));
        }
        let Some(payload) = self
            .repository
            .read_failed_message_payload(
                &target.thread_id,
                &target.message_id,
                &target.original_request_id,
            )
            .await
            .map_err(|error| queue_failure(&error, request_id))?
        else {
            return Ok(answer(None, ReceiptDisposition::Accepted));
        };
        let project_id = self
            .repository
            .read_thread_project(&target.thread_id)
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        let title = ThreadTitle::parse(RECOVERED_THREAD_TITLE)
            .map_err(|_| internal("recovered thread title is invalid", request_id))?;
        let created = self
            .create_thread_record(
                request_id,
                &CreateThread {
                    request_id: request_id.clone(),
                    project_id,
                    title,
                },
            )
            .await?;
        let new_thread_id = created.thread.thread_id;
        self.copy_failed_message_config(request_id, recover, &new_thread_id)
            .await?;
        self.store_payload_as_draft(request_id, &new_thread_id, &payload)
            .await?;
        let recovered_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        self.repository
            .record_failed_message_recovery(request_id, target, &new_thread_id, recovered_at)
            .await
            .map_err(|error| queue_failure(&error, request_id))?;
        Ok(answer(Some(new_thread_id), created.receipt.disposition))
    }

    /// Gives the recovery thread the engine configuration the failed message
    /// was accepted with (or, for a legacy receipt without a snapshot, the
    /// old thread's current configuration). Nothing is copied when neither
    /// exists.
    async fn copy_failed_message_config(
        &self,
        request_id: &RequestId,
        recover: &RecoverFailedMessage,
        new_thread_id: &ThreadId,
    ) -> Result<(), ProtocolFailure> {
        let target = &recover.target;
        let snapshot = self
            .repository
            .read_receipt_engine_settings(&target.original_request_id)
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        let settings = match snapshot {
            Some(settings) => Some(settings),
            None => self
                .repository
                .read_thread_engine_settings(&target.thread_id)
                .await
                .map_err(|error| repository_failure(&error, request_id))?,
        };
        let Some(settings) = settings else {
            return Ok(());
        };
        let config_request = RequestId::parse(format!("{request_id}.engine-config"))
            .map_err(|_| internal("recovery request id cannot name its config", request_id))?;
        self.set_thread_engine_config_outcome(
            request_id,
            &SetThreadEngineConfig::new(
                config_request,
                new_thread_id.clone(),
                EngineConfigUpdatePrecondition::Unconfigured,
                settings.config().clone(),
            ),
        )
        .await
        .map(|_| ())
    }
}

fn internal(detail: &'static str, request_id: &RequestId) -> ProtocolFailure {
    typed_failure(ErrorCode::Internal, detail, false, request_id)
}
