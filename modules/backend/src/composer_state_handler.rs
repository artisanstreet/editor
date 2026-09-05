//! Durable queue recovery and usage reads for the composer.
use super::*;
use artisan_database::{QueuedMessageRepositoryError, RunUsageRepositoryError};
use artisan_domain::{
    ListQueuedMessages, ReadRecalledMessage, ReadRunUsage, RecalledMessageResult, RunUsageResult,
    WithdrawQueuedMessage, WithdrawQueuedMessageCommand,
};

impl RequestHandler {
    pub(super) async fn read_composer_queue(
        &self,
        request_id: &RequestId,
        query: &ListQueuedMessages,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let listing = self
            .repository
            .read_queued_messages(query.clone())
            .await
            .map_err(|error| queue_failure(error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::QueuedMessages(listing),
        ))
    }

    pub(super) async fn read_recalled_composer_message(
        &self,
        request_id: &RequestId,
        query: &ReadRecalledMessage,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let payload = self
            .repository
            .read_withdrawn_message_payload(
                &query.thread_id,
                &query.message_id,
                &query.original_request_id,
            )
            .await
            .map_err(|error| queue_failure(error, request_id))?;
        let result = RecalledMessageResult::new(
            query.thread_id.clone(),
            query.message_id.clone(),
            query.original_request_id.clone(),
            payload,
        )
        .map_err(|_| {
            typed_failure(
                ErrorCode::Internal,
                "recalled-message payload violates its durable bounds",
                false,
                request_id,
            )
        })?;
        Ok(outcome(
            request_id,
            ResponsePayload::RecalledMessage(result),
        ))
    }

    pub(super) async fn read_composer_usage(
        &self,
        request_id: &RequestId,
        query: &ReadRunUsage,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let report = self
            .repository
            .read_latest_run_usage(&query.run_id, &query.thread_id)
            .await
            .map_err(|error| usage_failure(error, request_id))?;
        let result = RunUsageResult::new(query.thread_id.clone(), query.run_id.clone(), report)
            .map_err(|_| {
                typed_failure(
                    ErrorCode::Internal,
                    "run-usage report violates its durable scope",
                    false,
                    request_id,
                )
            })?;
        Ok(outcome(request_id, ResponsePayload::RunUsage(result)))
    }

    pub(super) async fn withdraw_composer_message(
        &self,
        request_id: &RequestId,
        command: &WithdrawQueuedMessageCommand,
    ) -> Result<ServerResponse, ProtocolFailure> {
        // This read-only exact check must remain ahead of Forge clock access:
        // an idempotent retry replays the stored acceptance instant and never
        // depends on fresh admission state.
        if let Some(receipt) = self
            .repository
            .lookup_queued_message_withdrawal(
                &command.thread_id,
                &command.message_id,
                &command.original_request_id,
                &command.request_id,
            )
            .await
            .map_err(|error| queue_failure(error, request_id))?
        {
            return Ok(outcome(
                request_id,
                ResponsePayload::MessageWithdrawn(receipt),
            ));
        }
        let accepted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let receipt = self
            .repository
            .withdraw_queued_message(WithdrawQueuedMessage {
                thread_id: command.thread_id.clone(),
                message_id: command.message_id.clone(),
                original_request_id: command.original_request_id.clone(),
                withdrawal_request_id: command.request_id.clone(),
                accepted_at,
            })
            .await
            .map_err(|error| queue_failure(error, request_id))?;
        // `withdraw_queued_message` commits its fence before returning. The
        // original payload is intentionally recovered only through the
        // separate recalled-message query after this result is returned.
        Ok(outcome(
            request_id,
            ResponsePayload::MessageWithdrawn(receipt),
        ))
    }
}

fn queue_failure(error: QueuedMessageRepositoryError, request_id: &RequestId) -> ProtocolFailure {
    let (code, detail, retryable) = match error {
        QueuedMessageRepositoryError::IdempotencyConflict { .. } => (
            ErrorCode::IdempotencyConflict,
            "withdrawal request conflicts with its stored payload",
            false,
        ),
        QueuedMessageRepositoryError::ThreadNotFound { .. } => (
            ErrorCode::ThreadUnknown,
            "queued-message thread is unavailable",
            false,
        ),
        QueuedMessageRepositoryError::CrossThread { .. }
        | QueuedMessageRepositoryError::OriginalRequestMismatch { .. }
        | QueuedMessageRepositoryError::InvalidListLimit(_) => (
            ErrorCode::InvalidInput,
            "queued-message request is invalid",
            false,
        ),
        QueuedMessageRepositoryError::InvalidChronology { .. }
        | QueuedMessageRepositoryError::InvalidListing(_)
        | QueuedMessageRepositoryError::CorruptData { .. }
        | QueuedMessageRepositoryError::Invariant { .. } => (
            ErrorCode::Internal,
            "queued-message storage state is invalid",
            false,
        ),
        QueuedMessageRepositoryError::Database { .. } => (
            ErrorCode::Internal,
            "queued-message storage is unavailable",
            true,
        ),
    };
    typed_failure(code, detail, retryable, request_id)
}

fn usage_failure(error: RunUsageRepositoryError, request_id: &RequestId) -> ProtocolFailure {
    let (code, detail, retryable) = match error {
        RunUsageRepositoryError::RunNotFound { .. }
        | RunUsageRepositoryError::RunThreadMismatch { .. }
        | RunUsageRepositoryError::ReportRunMismatch
        | RunUsageRepositoryError::ReportThreadMismatch => (
            ErrorCode::InvalidInput,
            "run-usage request is outside the exact run scope",
            false,
        ),
        RunUsageRepositoryError::InvalidRunSnapshot { .. }
        | RunUsageRepositoryError::ModelOriginMismatch { .. }
        | RunUsageRepositoryError::StoredThreadMismatch { .. }
        | RunUsageRepositoryError::GenerationMismatch { .. }
        | RunUsageRepositoryError::ProviderSessionConflict { .. }
        | RunUsageRepositoryError::StaleSequence { .. }
        | RunUsageRepositoryError::SequenceConflict { .. }
        | RunUsageRepositoryError::CorruptUsageRow { .. } => (
            ErrorCode::Internal,
            "run-usage storage state is invalid",
            false,
        ),
        RunUsageRepositoryError::Repository(RepositoryError::Database { .. }) => (
            ErrorCode::Internal,
            "run-usage storage is unavailable",
            true,
        ),
        RunUsageRepositoryError::Repository(RepositoryError::ThreadNotFound { .. }) => (
            ErrorCode::ThreadUnknown,
            "run-usage thread is unavailable",
            false,
        ),
        RunUsageRepositoryError::Repository(_) => (
            ErrorCode::Internal,
            "run-usage storage state is invalid",
            false,
        ),
    };
    typed_failure(code, detail, retryable, request_id)
}
