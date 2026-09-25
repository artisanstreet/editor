//! Transport-thread child for Forge-owned composer drafts and stored
//! attachments.
//!
//! Every draft command is answered by exactly one [`ComposerDraftEvent`] that
//! names its scope, so the application can settle its per-scope save chain.
//! Digests this connection uploaded or read back are remembered: a later
//! message whose images are all stored is sent by reference instead of
//! re-uploading its bytes.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    ComposerAttachmentDigest, ComposerAttachmentRef, ComposerAttachmentResult, ComposerDraft,
    ComposerDraftRevision, ComposerDraftScope, QueueStoredMessage, ReadComposerAttachment,
    ReadComposerDraft, SaveComposerDraft, UploadComposerAttachment,
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
    Attachment(ComposerAttachmentDigest),
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
            (Self::Attachment(digest), ResponsePayload::ComposerAttachment(result)) => {
                &result.digest == digest
                    && Sha256::digest(&result.bytes).as_slice() == digest.as_bytes()
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
            let request_id = command.request_id.clone();
            let expected = ComposerDraftExpectation::Uploaded(request_id.clone());
            let result = mutate(
                runtime,
                frames,
                &request_id,
                Command::UploadComposerAttachment(*command),
                expected,
            )
            .await
            .and_then(|payload| match payload {
                ResponsePayload::ComposerAttachmentUploaded(uploaded) => Ok(uploaded.reference),
                _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
            });
            if let Ok(reference) = &result {
                runtime.stored_attachments.insert(*reference.digest());
            }
            ComposerDraftEvent::Uploaded {
                scope,
                attachment_id,
                result,
            }
        }
        ComposerDraftCommand::ReadAttachment { scope, digest } => {
            let result = runtime
                .request(
                    frames,
                    query_request(Query::ReadComposerAttachment(ReadComposerAttachment {
                        digest,
                    })),
                    ExpectedResponse::ComposerDraft(ComposerDraftExpectation::Attachment(digest)),
                )
                .await
                .map_err(ServiceFailure::from)
                .and_then(|payload| match payload {
                    ResponsePayload::ComposerAttachment(result) => Ok(Box::new(result)),
                    _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
                });
            if result.is_ok() {
                runtime.stored_attachments.insert(digest);
            }
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

/// The command a general message is sent as: by stored-attachment reference
/// when every image is already in the Forge store, otherwise with its bytes.
pub(super) fn message_command(
    command: QueueMessage,
    stored: &HashSet<ComposerAttachmentDigest>,
) -> Command {
    let attachments = command.payload.attachments();
    if attachments.is_empty() {
        return Command::QueueMessage(command);
    }
    let references = attachments
        .iter()
        .map(|image| {
            let digest = ComposerAttachmentDigest::new(Sha256::digest(image.bytes()).into());
            stored.contains(&digest).then_some(())?;
            let size = u32::try_from(image.byte_len()).ok()?;
            ComposerAttachmentRef::new(digest, image.mime_type(), image.name(), size).ok()
        })
        .collect::<Option<Vec<_>>>();
    let stored_message = references.and_then(|references| {
        QueueStoredMessage::new(
            command.request_id.clone(),
            command.thread_id.clone(),
            command.payload.text().cloned(),
            references,
            command.steer_target().cloned(),
        )
        .ok()
    });
    stored_message.map_or(Command::QueueMessage(command), Command::QueueStoredMessage)
}

#[cfg(test)]
mod tests {
    use artisan_domain::{
        AuthoredText, ComposerAttachmentDigest, ImageAttachment, QueueMessagePayload, RequestId,
        ThreadId,
    };
    use sha2::{Digest as _, Sha256};

    use super::{Command, HashSet, QueueMessage, message_command};

    fn message(images: Vec<ImageAttachment>) -> QueueMessage {
        QueueMessage::new(
            RequestId::parse("send-stored").unwrap(),
            ThreadId::parse("send-thread").unwrap(),
            QueueMessagePayload::new(Some(AuthoredText::parse("look").unwrap()), images).unwrap(),
        )
    }

    #[test]
    fn sends_reference_stored_attachments_instead_of_their_bytes() {
        let image = ImageAttachment::new("image/png", vec![1, 2, 3], "shot.png").unwrap();
        let digest = ComposerAttachmentDigest::new(Sha256::digest(image.bytes()).into());
        let stored = HashSet::from([digest]);
        let Command::QueueStoredMessage(sent) =
            message_command(message(vec![image.clone()]), &stored)
        else {
            panic!("an uploaded image is sent by reference");
        };
        assert_eq!(sent.attachments()[0].digest(), &digest);
        assert_eq!(sent.attachments()[0].name(), "shot.png");
        assert_eq!(sent.text().map(AuthoredText::as_str), Some("look"));

        // Bytes the Forge store does not hold travel inline, as does text only.
        let other = ImageAttachment::new("image/png", vec![9], "new.png").unwrap();
        assert!(matches!(
            message_command(message(vec![image, other]), &stored),
            Command::QueueMessage(_)
        ));
        assert!(matches!(
            message_command(message(Vec::new()), &stored),
            Command::QueueMessage(_)
        ));
    }
}
