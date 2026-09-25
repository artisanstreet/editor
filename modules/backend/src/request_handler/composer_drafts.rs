//! Forge-owned composer drafts, stored attachments, and messages sent by
//! stored-attachment reference.
//!
//! Every draft save applies (the last to arrive wins) and the Forge assigns
//! the scope's next revision; a retried save stores the same body again. A
//! stored-attachment message resolves its references to owned bytes, fitted
//! to the thread's engine like a draft send's, and then takes the ordinary queue-message path, so receipt replay, idempotency, and
//! dispatch are exactly those of an inline-byte send.

use artisan_database::{ComposerDraftRepositoryError, SaveComposerDraftInput};
use artisan_domain::{
    AuthoredText, COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES, ComposerAttachmentRef,
    ComposerAttachmentResult, ComposerAttachmentUploaded, ComposerDraftResult, ComposerDraftSaved,
    ComposerDraftScope, ComposerUpload, EngineId, ImageAttachment, QueueMessage,
    QueueMessagePayload, QueueStoredMessage, ReadComposerAttachment, ReadComposerDraft, RequestId,
    SaveComposerDraft, ThreadId, UploadComposerAttachment,
};
use artisan_protocol::{ErrorCode, ProtocolFailure, ResponsePayload, ServerResponse};

use super::failures::{outcome, repository_failure, typed_failure};
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
                request_id: save.request_id().clone(),
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

    /// Stores one image, or one chunk of one, in the content-addressed
    /// attachment store.
    pub(super) async fn upload_composer_attachment(
        &self,
        request_id: &RequestId,
        upload: &UploadComposerAttachment,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let stored_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let (reference, pending_bytes) = match &upload.upload {
            ComposerUpload::Image(image) => (
                self.repository
                    .store_composer_attachment(image, stored_at)
                    .await
                    .map_err(|error| draft_failure(&error, request_id))?,
                0,
            ),
            ComposerUpload::Chunk(chunk) => {
                let stored = self
                    .repository
                    .store_composer_attachment_chunk(chunk, stored_at)
                    .await
                    .map_err(|error| draft_failure(&error, request_id))?;
                (stored.reference, stored.pending_bytes)
            }
        };
        Ok(outcome(
            request_id,
            ResponsePayload::ComposerAttachmentUploaded(ComposerAttachmentUploaded {
                request_id: request_id.clone(),
                reference,
                pending_bytes,
            }),
        ))
    }

    /// A stored-attachment message's images: fitted to the thread's engine
    /// like a draft send's, or (for a thread with no engine yet) sent as
    /// they are, which only message-sized images survive.
    async fn stored_message_images(
        &self,
        request_id: &RequestId,
        queue: &QueueStoredMessage,
    ) -> Result<Vec<ImageAttachment>, ProtocolFailure> {
        if queue.attachments().is_empty() {
            return Ok(Vec::new());
        }
        let engine = self
            .repository
            .read_thread_engine_settings(queue.thread_id())
            .await
            .map_err(|error| repository_failure(&error, request_id))?
            .map(|settings| settings.config().selection().engine_id());
        let Some(engine) = engine else {
            return self
                .repository
                .resolve_composer_attachments(queue.attachments())
                .await
                .map_err(|error| draft_failure(&error, request_id));
        };
        let picked = self.read_picked(request_id, queue.attachments()).await?;
        self.fit_picked(request_id, engine, picked)
            .await?
            .map_err(|reason| typed_failure(ErrorCode::InvalidInput, reason, false, request_id))
    }

    /// The stored bytes of each reference, in order, checked against it.
    pub(super) async fn read_picked(
        &self,
        request_id: &RequestId,
        references: &[ComposerAttachmentRef],
    ) -> Result<Vec<(ComposerAttachmentResult, String)>, ProtocolFailure> {
        let mut picked = Vec::with_capacity(references.len());
        for reference in references {
            let stored = self
                .repository
                .read_composer_attachment(reference.digest())
                .await
                .map_err(|error| draft_failure(&error, request_id))?
                .filter(|stored| {
                    stored.mime_type == reference.mime_type()
                        && stored.total_bytes == reference.size_bytes()
                })
                .ok_or_else(|| {
                    typed_failure(
                        ErrorCode::InvalidInput,
                        "composer attachment reference does not name stored bytes",
                        false,
                        request_id,
                    )
                })?;
            picked.push((stored, reference.name().to_owned()));
        }
        Ok(picked)
    }

    /// Fits picked images to `engine` off the async runtime: the fitted
    /// images, or the presentation-ready reason one cannot be sent.
    pub(super) async fn fit_picked(
        &self,
        request_id: &RequestId,
        engine: EngineId,
        picked: Vec<(ComposerAttachmentResult, String)>,
    ) -> Result<Result<Vec<ImageAttachment>, String>, ProtocolFailure> {
        tokio::task::spawn_blocking(move || {
            crate::attachment_policy::fit_draft_images(engine.as_str(), &picked)
        })
        .await
        .map_err(|_| {
            typed_failure(
                ErrorCode::Internal,
                "fitting the message's images failed",
                true,
                request_id,
            )
        })
    }

    /// Resolves stored references to owned images and admits the message
    /// through the ordinary queue-message path.
    pub(super) async fn queue_stored_message_outcome(
        &self,
        request_id: &RequestId,
        queue: &QueueStoredMessage,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let images = self.stored_message_images(request_id, queue).await?;
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

    /// Stores a message payload as a thread's composer draft: the images go
    /// to the attachment store and the draft references them. Used when the
    /// Forge hands a withdrawn or failed prompt back to the user.
    pub(super) async fn store_payload_as_draft(
        &self,
        request_id: &RequestId,
        thread_id: &ThreadId,
        payload: &QueueMessagePayload,
    ) -> Result<(), ProtocolFailure> {
        let saved_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let mut attachments = Vec::with_capacity(payload.attachments().len());
        for image in payload.attachments() {
            // A message image always fits the store's larger picked-image
            // bound.
            let image = artisan_domain::ComposerImage::new(
                image.mime_type_str(),
                image.bytes().to_vec(),
                image.name(),
            )
            .map_err(|_| {
                typed_failure(
                    ErrorCode::Internal,
                    "a message image does not fit the composer store",
                    false,
                    request_id,
                )
            })?;
            attachments.push(
                self.repository
                    .store_composer_attachment(&image, saved_at)
                    .await
                    .map_err(|error| draft_failure(&error, request_id))?,
            );
        }
        self.repository
            .save_composer_draft(SaveComposerDraftInput {
                request_id: request_id.clone(),
                scope: ComposerDraftScope::Thread(thread_id.clone()),
                text: payload.text().cloned().unwrap_or_else(AuthoredText::empty),
                attachments,
                saved_at,
            })
            .await
            .map_err(|error| draft_failure(&error, request_id))?;
        Ok(())
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

    /// Reads a window of the bytes of one stored attachment.
    pub(super) async fn read_composer_attachment(
        &self,
        request_id: &RequestId,
        read: &ReadComposerAttachment,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let stored = self
            .repository
            .read_composer_attachment_window(&ReadComposerAttachment {
                max_bytes: bounded_window(read.max_bytes),
                ..read.clone()
            })
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

pub(super) fn draft_failure(
    error: &ComposerDraftRepositoryError,
    request_id: &RequestId,
) -> ProtocolFailure {
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
        ComposerDraftRepositoryError::ChunkRejected { .. } => (
            ErrorCode::InvalidInput,
            "the uploaded image did not match its digest; upload it again",
            false,
        ),
        ComposerDraftRepositoryError::AttachmentNotSendable { .. } => (
            ErrorCode::InvalidInput,
            "a stored composer image is too large to send as it is; send the draft instead",
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

/// A read window of at most one chunk, so every answer fits one frame.
fn bounded_window(max_bytes: u32) -> u32 {
    let chunk = u32::try_from(COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES).unwrap_or(u32::MAX);
    if max_bytes == 0 {
        chunk
    } else {
        max_bytes.min(chunk)
    }
}
