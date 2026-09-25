//! Forge-owned composer drafts, stored attachments, and messages sent by
//! stored-attachment reference.
//!
//! Every draft save applies (the last to arrive wins) and the Forge assigns
//! the scope's next revision; a retried save stores the same body again. A
//! stored-attachment message resolves its references to owned bytes and then
//! takes the ordinary queue-message path, so receipt replay, idempotency, and
//! dispatch are exactly those of an inline-byte send.

use artisan_database::{ComposerDraftRepositoryError, SaveComposerDraftInput};
use artisan_domain::{
    ComposerAttachmentUploaded, ComposerDraftResult, ComposerDraftSaved, QueueMessage,
    QueueMessagePayload, QueueStoredMessage, ReadComposerAttachment, ReadComposerDraft, RequestId,
    SaveComposerDraft, UploadComposerAttachment,
};
use artisan_protocol::{ErrorCode, ProtocolFailure, ResponsePayload, ServerResponse};

use super::failures::{outcome, typed_failure};
use super::{RequestHandler, origin_clock_failure};

impl RequestHandler {
    /// Replaces one scope's draft and acknowledges the revision the Forge
    /// assigned to it.
    pub(super) async fn save_composer_draft(
        &self,
        request_id: &RequestId,
        save: &SaveComposerDraft,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let saved_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let saved = self
            .repository
            .save_composer_draft(SaveComposerDraftInput {
                scope: save.scope().clone(),
                text: save.text().clone(),
                attachments: save.attachments().to_vec(),
                saved_at,
            })
            .await
            .map_err(|error| draft_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::ComposerDraftSaved(ComposerDraftSaved {
                request_id: request_id.clone(),
                scope: save.scope().clone(),
                revision: saved.revision,
            }),
        ))
    }

    /// Stores one image in the content-addressed attachment store.
    pub(super) async fn upload_composer_attachment(
        &self,
        request_id: &RequestId,
        upload: &UploadComposerAttachment,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let stored_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let reference = self
            .repository
            .store_composer_attachment(&upload.image, stored_at)
            .await
            .map_err(|error| draft_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::ComposerAttachmentUploaded(ComposerAttachmentUploaded {
                request_id: request_id.clone(),
                reference,
            }),
        ))
    }

    /// Resolves stored references to owned images and admits the message
    /// through the ordinary queue-message path.
    pub(super) async fn queue_stored_message_outcome(
        &self,
        request_id: &RequestId,
        queue: &QueueStoredMessage,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let images = self
            .repository
            .resolve_composer_attachments(queue.attachments())
            .await
            .map_err(|error| draft_failure(&error, request_id))?;
        let payload = QueueMessagePayload::new(queue.text().cloned(), images).map_err(|_| {
            typed_failure(
                ErrorCode::InvalidInput,
                "stored-attachment message exceeds the message bounds",
                false,
                request_id,
            )
        })?;
        let mut resolved = QueueMessage::new(
            queue.request_id().clone(),
            queue.thread_id().clone(),
            payload,
        );
        if let Some(target) = queue.steer_target() {
            resolved = resolved.with_steer_target(target.clone());
        }
        self.queue_message_outcome(request_id, &resolved).await
    }

    /// Reads one scope's stored draft.
    pub(super) async fn read_composer_draft(
        &self,
        request_id: &RequestId,
        read: &ReadComposerDraft,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let draft = self
            .repository
            .read_composer_draft(&read.scope)
            .await
            .map_err(|error| draft_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::ComposerDraft(ComposerDraftResult {
                scope: read.scope.clone(),
                draft,
            }),
        ))
    }

    /// Reads the bytes of one stored attachment.
    pub(super) async fn read_composer_attachment(
        &self,
        request_id: &RequestId,
        read: &ReadComposerAttachment,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let stored = self
            .repository
            .read_composer_attachment(&read.digest)
            .await
            .map_err(|error| draft_failure(&error, request_id))?
            .ok_or_else(|| {
                typed_failure(
                    ErrorCode::InvalidInput,
                    "composer attachment is not stored",
                    false,
                    request_id,
                )
            })?;
        Ok(outcome(
            request_id,
            ResponsePayload::ComposerAttachment(stored),
        ))
    }
}

fn draft_failure(error: &ComposerDraftRepositoryError, request_id: &RequestId) -> ProtocolFailure {
    let (code, detail, retryable) = match error {
        ComposerDraftRepositoryError::ScopeNotFound { kind, .. } => (
            if *kind == "thread" {
                ErrorCode::ThreadUnknown
            } else {
                ErrorCode::ProjectUnknown
            },
            "composer draft scope is unavailable",
            false,
        ),
        ComposerDraftRepositoryError::AttachmentNotStored { .. }
        | ComposerDraftRepositoryError::AttachmentMismatch { .. } => (
            ErrorCode::InvalidInput,
            "composer attachment reference does not name stored bytes",
            false,
        ),
        ComposerDraftRepositoryError::CorruptData { .. }
        | ComposerDraftRepositoryError::RevisionExhausted => (
            ErrorCode::Internal,
            "composer draft storage state is invalid",
            false,
        ),
        ComposerDraftRepositoryError::Database { .. } => (
            ErrorCode::Internal,
            "composer draft storage is unavailable",
            true,
        ),
    };
    typed_failure(code, detail, retryable, request_id)
}
