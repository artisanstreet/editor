//! Transport-thread child for Forge-owned composer drafts and stored
//! attachments.
//!
//! Every draft command is answered by exactly one [`ComposerDraftEvent`] that
//! names its scope, so the application can settle its per-scope save chain.
//! A message is never sent from here: the Editor sends its draft by revision
//! (`SubmitComposerDraft`).

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES, ComposerAttachmentChunk, ComposerAttachmentDigest,
    ComposerAttachmentRef, ComposerAttachmentResult, ComposerDraft, ComposerDraftRevision,
    ComposerDraftScope, ComposerUpload, ReadComposerAttachment, ReadComposerDraft,
    SaveComposerDraft, UploadComposerAttachment,
};

use super::*;

/// One draft command sent from the application thread to the service child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerDraftCommand {
    /// Save one scope's draft; the Forge assigns its revision.
    Save {
        /// Local per-scope send sequence echoed by the result event.
        sequence: u64,
        /// Save with its request identity.
        command: Box<SaveComposerDraft>,
    },
    /// Read one scope's stored draft.
    Read(ComposerDraftScope),
    /// Store one attachment of a scope's composer.
    Upload {
        /// Scope whose composer owns the attachment.
        scope: ComposerDraftScope,
        /// Composer-local attachment identity.
        attachment_id: String,
        /// Upload with its stable request identity.
        command: Box<UploadComposerAttachment>,
    },
    /// Read the bytes of one stored attachment referenced by a scope's draft.
    ReadAttachment {
        /// Scope whose draft references the attachment.
        scope: ComposerDraftScope,
        /// Store key.
        digest: ComposerAttachmentDigest,
    },
}

/// One draft result returned by the service child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerDraftEvent {
    /// A save was stored under the revision the Forge assigned.
    Saved {
        /// Draft scope.
        scope: ComposerDraftScope,
        /// Local send sequence of the save.
        sequence: u64,
        /// Revision the Forge assigned.
        revision: ComposerDraftRevision,
    },
    /// A save failed definitively.
    SaveFailed {
        /// Draft scope.
        scope: ComposerDraftScope,
        /// Local send sequence of the save.
        sequence: u64,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// The Forge reported a scope's revision outside a save (the emptied
    /// draft after a send).
    Revision {
        /// Draft scope.
        scope: ComposerDraftScope,
        /// Reported revision.
        revision: ComposerDraftRevision,
    },
    /// A scope's stored draft, or its read failure.
    Read {
        /// Draft scope.
        scope: ComposerDraftScope,
        /// Stored draft (`None` when never saved), or the failure.
        result: Result<Option<ComposerDraft>, ServiceFailure>,
    },
    /// An attachment was stored, or its upload failed.
    Uploaded {
        /// Scope whose composer owns the attachment.
        scope: ComposerDraftScope,
        /// Composer-local attachment identity.
        attachment_id: String,
        /// Stored reference, or the failure.
        result: Result<ComposerAttachmentRef, ServiceFailure>,
    },
    /// Stored attachment bytes, or their read failure.
    AttachmentRead {
        /// Scope whose draft references the attachment.
        scope: ComposerDraftScope,
        /// Store key.
        digest: ComposerAttachmentDigest,
        /// Verified bytes, or the failure.
        result: Result<Box<ComposerAttachmentResult>, ServiceFailure>,
    },
}

/// The response shape one draft request accepts.
#[derive(Clone)]
pub(super) enum ComposerDraftExpectation {
    Saved {
        request_id: RequestId,
        scope: ComposerDraftScope,
    },
    Draft(ComposerDraftScope),
    Uploaded(RequestId),
    /// One window of a stored attachment, starting at the offset asked.
    Attachment {
        digest: ComposerAttachmentDigest,
        offset: u32,
    },
}

impl ComposerDraftExpectation {
    /// Whether `payload` is the exact answer to this request.
    pub(super) fn accepts(&self, payload: &ResponsePayload) -> bool {
        match (self, payload) {
            (Self::Saved { request_id, scope }, ResponsePayload::ComposerDraftSaved(saved)) => {
                &saved.request_id == request_id && &saved.scope == scope
            }
            (Self::Draft(scope), ResponsePayload::ComposerDraft(result)) => &result.scope == scope,
            (Self::Uploaded(request_id), ResponsePayload::ComposerAttachmentUploaded(uploaded)) => {
                &uploaded.request_id == request_id
            }
            (Self::Attachment { digest, offset }, ResponsePayload::ComposerAttachment(result)) => {
                &result.digest == digest && result.offset == *offset
            }
            _ => false,
        }
    }
}

/// Handles one draft command on the authenticated service runtime.
pub(super) async fn handle_composer_draft_command(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: ComposerDraftCommand,
) -> Result<(), ServiceFailure> {
    let event = match command {
        ComposerDraftCommand::Save { sequence, command } => {
            save_draft(runtime, frames, sequence, *command).await
        }
        ComposerDraftCommand::Read(scope) => {
            let result = runtime
                .request(
                    frames,
                    query_request(Query::ReadComposerDraft(ReadComposerDraft {
                        scope: scope.clone(),
                    })),
                    ExpectedResponse::ComposerDraft(ComposerDraftExpectation::Draft(scope.clone())),
                )
                .await
                .map_err(ServiceFailure::from)
                .and_then(|payload| match payload {
                    ResponsePayload::ComposerDraft(result) => Ok(result.draft),
                    _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
                });
            ComposerDraftEvent::Read { scope, result }
        }
        ComposerDraftCommand::Upload {
            scope,
            attachment_id,
            command,
        } => {
            let result = upload(runtime, frames, *command).await;
            ComposerDraftEvent::Uploaded {
                scope,
                attachment_id,
                result,
            }
        }
        ComposerDraftCommand::ReadAttachment { scope, digest } => {
            let result = read_attachment(runtime, frames, digest).await.map(Box::new);
            ComposerDraftEvent::AttachmentRead {
                scope,
                digest,
                result,
            }
        }
    };
    publish(events, NativeTransportEvent::ComposerDraft(event))
}

/// Sends one draft save and reports its acknowledgement or failure.
async fn save_draft(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    sequence: u64,
    save: SaveComposerDraft,
) -> ComposerDraftEvent {
    let scope = save.scope().clone();
    let expected = ComposerDraftExpectation::Saved {
        request_id: save.request_id().clone(),
        scope: scope.clone(),
    };
    let request_id = save.request_id().clone();
    match mutate(
        runtime,
        frames,
        &request_id,
        Command::SaveComposerDraft(save),
        expected,
    )
    .await
    {
        Ok(ResponsePayload::ComposerDraftSaved(saved)) => ComposerDraftEvent::Saved {
            scope,
            sequence,
            revision: saved.revision,
        },
        Ok(_) => ComposerDraftEvent::SaveFailed {
            scope,
            sequence,
            failure: ServiceFailure::invalid(ServiceFailureStage::Request),
        },
        Err(failure) => ComposerDraftEvent::SaveFailed {
            scope,
            sequence,
            failure,
        },
    }
}

/// Sends one idempotent draft mutation with its stable frame identity.
async fn mutate(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    request_id: &RequestId,
    command: Command,
    expected: ComposerDraftExpectation,
) -> Result<ResponsePayload, ServiceFailure> {
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let mutation = StableMutation {
        frame_id,
        sent_at,
        command,
    };
    durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::ComposerDraft(expected),
    )
    .await
    .map_err(ServiceFailure::from)
}

/// Stores one picked image and answers its reference once every byte is
/// stored.
async fn upload(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    command: UploadComposerAttachment,
) -> Result<ComposerAttachmentRef, ServiceFailure> {
    let mut stored = None;
    for request in upload_requests(command)? {
        stored = Some(upload_one(runtime, frames, request).await?);
    }
    match stored {
        Some((reference, 0)) => Ok(reference),
        _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
    }
}

/// The requests that store one picked image: the image itself when it fits
/// one chunk, otherwise its chunks in order under request identities
/// derived from the stable one, so a retransmitted upload repeats exactly
/// the same chunks.
pub(super) fn upload_requests(
    command: UploadComposerAttachment,
) -> Result<Vec<UploadComposerAttachment>, ServiceFailure> {
    let image = match &command.upload {
        ComposerUpload::Image(image) if image.byte_len() > COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES => {
            image
        }
        _ => return Ok(vec![command]),
    };
    let digest = ComposerAttachmentDigest::new(Sha256::digest(image.bytes()).into());
    ComposerAttachmentChunk::split(image, digest)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let request_id = RequestId::parse(format!("{}-chunk-{index}", command.request_id))
                .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
            Ok(UploadComposerAttachment {
                request_id,
                upload: ComposerUpload::Chunk(chunk),
            })
        })
        .collect()
}

/// Sends one upload request: the reference and the bytes still missing.
async fn upload_one(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    command: UploadComposerAttachment,
) -> Result<(ComposerAttachmentRef, u32), ServiceFailure> {
    let request_id = command.request_id.clone();
    let expected = ComposerDraftExpectation::Uploaded(request_id.clone());
    match mutate(
        runtime,
        frames,
        &request_id,
        Command::UploadComposerAttachment(command),
        expected,
    )
    .await?
    {
        ResponsePayload::ComposerAttachmentUploaded(uploaded) => {
            Ok((uploaded.reference, uploaded.pending_bytes))
        }
        _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
    }
}

/// Reads one stored attachment window by window.
async fn read_attachment(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    digest: ComposerAttachmentDigest,
) -> Result<ComposerAttachmentResult, ServiceFailure> {
    let mut readback = AttachmentReadback::new(digest);
    loop {
        let read = readback.next_read()?;
        let expected = ComposerDraftExpectation::Attachment {
            digest,
            offset: read.offset,
        };
        let window = match runtime
            .request(
                frames,
                query_request(Query::ReadComposerAttachment(read)),
                ExpectedResponse::ComposerDraft(expected),
            )
            .await
            .map_err(ServiceFailure::from)?
        {
            ResponsePayload::ComposerAttachment(window) => window,
            _ => return Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
        };
        if let Some(image) = readback.accept(window)? {
            return Ok(image);
        }
    }
}

/// Joins the windows of one stored attachment and verifies the whole image
/// against its digest.
pub(super) struct AttachmentReadback {
    digest: ComposerAttachmentDigest,
    bytes: Vec<u8>,
    mime_type: Option<artisan_domain::ImageMimeType>,
}

impl AttachmentReadback {
    pub(super) fn new(digest: ComposerAttachmentDigest) -> Self {
        Self {
            digest,
            bytes: Vec::new(),
            mime_type: None,
        }
    }

    /// The next window to ask for.
    pub(super) fn next_read(&self) -> Result<ReadComposerAttachment, ServiceFailure> {
        let invalid = || ServiceFailure::invalid(ServiceFailureStage::Request);
        Ok(ReadComposerAttachment {
            digest: self.digest,
            offset: u32::try_from(self.bytes.len()).map_err(|_| invalid())?,
            max_bytes: u32::try_from(COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES).map_err(|_| invalid())?,
        })
    }

    /// Takes the answered window: the verified image once complete.
    pub(super) fn accept(
        &mut self,
        window: ComposerAttachmentResult,
    ) -> Result<Option<ComposerAttachmentResult>, ServiceFailure> {
        let invalid = || ServiceFailure::invalid(ServiceFailureStage::Request);
        let offset = usize::try_from(window.offset).map_err(|_| invalid())?;
        if window.digest != self.digest
            || offset != self.bytes.len()
            || self
                .mime_type
                .is_some_and(|known| known != window.mime_type)
        {
            return Err(invalid());
        }
        self.mime_type = Some(window.mime_type);
        self.bytes.extend_from_slice(&window.bytes);
        let total = usize::try_from(window.total_bytes).map_err(|_| invalid())?;
        if self.bytes.len() < total {
            return Ok(None);
        }
        if self.bytes.len() != total
            || Sha256::digest(&self.bytes).as_slice() != self.digest.as_bytes()
        {
            return Err(invalid());
        }
        Ok(Some(ComposerAttachmentResult::whole(
            self.digest,
            window.mime_type,
            std::mem::take(&mut self.bytes),
        )))
    }
}

#[cfg(test)]
#[path = "native_composer_draft_transport_tests.rs"]
mod tests;
