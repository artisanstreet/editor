//! Composer draft, stored-attachment, and stored-message codecs.

#![forbid(unsafe_code)]

use artisan_domain::{
    ComposerAttachmentDigest, ComposerAttachmentRef, ComposerAttachmentResult,
    ComposerAttachmentUploaded, ComposerDraft, ComposerDraftResult, ComposerDraftRevision,
    ComposerDraftSaved, ComposerDraftScope, ImageMimeType, ProjectId, QueueStoredMessage,
    ReadComposerAttachment, ReadComposerDraft, SaveComposerDraft, SteerTarget,
    UploadComposerAttachment,
};

use super::helpers::*;
use super::*;

fn state_value(field: &'static str) -> ComposerStateCodecError {
    ComposerStateCodecError::StateValue { field }
}

fn encode_scope(
    mut builder: composer_state_capnp::composer_draft_scope::Builder<'_>,
    scope: &ComposerDraftScope,
) {
    match scope {
        ComposerDraftScope::Thread(thread) => builder.set_thread(thread.as_str()),
        ComposerDraftScope::Project(project) => builder.set_project(project.as_str()),
    }
}

fn decode_scope(
    value: composer_state_capnp::composer_draft_scope::Reader<'_>,
    field: &'static str,
) -> Result<ComposerDraftScope, ComposerStateCodecError> {
    let scope = value
        .which()
        .map_err(|source| ComposerStateCodecError::UnknownEnum {
            field,
            value: source.0,
        })?;
    match scope {
        composer_state_capnp::composer_draft_scope::Which::Thread(thread) => Ok(
            ComposerDraftScope::Thread(parse_thread_id(read_text(thread, field)?, field)?),
        ),
        composer_state_capnp::composer_draft_scope::Which::Project(project) => {
            ProjectId::parse(read_text(project, field)?)
                .map(ComposerDraftScope::Project)
                .map_err(|source| ComposerStateCodecError::Identifier { field, source })
        }
    }
}

fn encode_reference(
    mut builder: composer_state_capnp::composer_attachment_ref::Builder<'_>,
    reference: &ComposerAttachmentRef,
) {
    builder.set_digest(reference.digest().as_bytes());
    builder.set_mime_type(reference.mime_type().as_str());
    builder.set_name(reference.name());
    builder.set_size_bytes(reference.size_bytes());
}

fn decode_digest(
    value: capnp::Result<capnp::data::Reader<'_>>,
    field: &'static str,
) -> Result<ComposerAttachmentDigest, ComposerStateCodecError> {
    ComposerAttachmentDigest::from_slice(value?).map_err(|_| state_value(field))
}

fn decode_mime(
    value: capnp::Result<capnp::text::Reader<'_>>,
    field: &'static str,
) -> Result<ImageMimeType, ComposerStateCodecError> {
    ImageMimeType::parse(&read_text(value, field)?)
        .map_err(|_| ComposerStateCodecError::ImageReference { field })
}

fn decode_reference(
    value: composer_state_capnp::composer_attachment_ref::Reader<'_>,
    field: &'static str,
) -> Result<ComposerAttachmentRef, ComposerStateCodecError> {
    ComposerAttachmentRef::new(
        decode_digest(value.get_digest(), field)?,
        decode_mime(value.get_mime_type(), field)?,
        read_text(value.get_name(), field)?,
        value.get_size_bytes(),
    )
    .map_err(|_| ComposerStateCodecError::ImageReference { field })
}

fn encode_references(
    mut builder: capnp::struct_list::Builder<
        '_,
        composer_state_capnp::composer_attachment_ref::Owned,
    >,
    references: &[ComposerAttachmentRef],
    field: &'static str,
) -> Result<(), ComposerStateCodecError> {
    for (index, reference) in references.iter().enumerate() {
        encode_reference(builder.reborrow().get(list_index(field, index)?), reference);
    }
    Ok(())
}

fn decode_references(
    value: capnp::struct_list::Reader<'_, composer_state_capnp::composer_attachment_ref::Owned>,
    field: &'static str,
) -> Result<Vec<ComposerAttachmentRef>, ComposerStateCodecError> {
    if usize::try_from(value.len()).map_or(true, |count| count > COMPOSER_STATE_IMAGE_MAX_COUNT) {
        return Err(state_value(field));
    }
    value
        .iter()
        .map(|reference| decode_reference(reference, field))
        .collect()
}

fn decode_revision(
    value: u64,
    field: &'static str,
) -> Result<ComposerDraftRevision, ComposerStateCodecError> {
    ComposerDraftRevision::new(value).map_err(|_| state_value(field))
}

fn decode_authored_text(
    value: capnp::Result<capnp::text::Reader<'_>>,
    field: &'static str,
) -> Result<AuthoredText, ComposerStateCodecError> {
    AuthoredText::parse(read_text(value, field)?)
        .map_err(|source| ComposerStateCodecError::AuthoredText { field, source })
}

/// Encodes one draft save. The request id is the parent envelope id.
pub fn encode_save_composer_draft_request(
    mut builder: composer_state_capnp::save_composer_draft_request::Builder<'_>,
    value: &SaveComposerDraft,
) -> Result<(), ComposerStateCodecError> {
    encode_scope(builder.reborrow().init_scope(), value.scope());
    builder.set_text(value.text().as_str());
    let field = "request.saveComposerDraft.attachments";
    encode_references(
        builder
            .reborrow()
            .init_attachments(list_length(field, value.attachments().len())?),
        value.attachments(),
        field,
    )
}

/// Decodes one draft save using the parent envelope request id.
pub fn decode_save_composer_draft_request(
    value: composer_state_capnp::save_composer_draft_request::Reader<'_>,
    request_id: RequestId,
) -> Result<SaveComposerDraft, ComposerStateCodecError> {
    let field = "request.saveComposerDraft";
    SaveComposerDraft::new(
        request_id,
        decode_scope(value.get_scope()?, "request.saveComposerDraft.scope")?,
        decode_authored_text(value.get_text(), "request.saveComposerDraft.text")?,
        decode_references(
            value.get_attachments()?,
            "request.saveComposerDraft.attachments",
        )?,
    )
    .map_err(|_| state_value(field))
}

/// Encodes one draft-save acknowledgement after checking its correlation.
pub fn encode_composer_draft_saved(
    mut builder: composer_state_capnp::composer_draft_saved::Builder<'_>,
    outer_request_id: &RequestId,
    value: &ComposerDraftSaved,
) -> Result<(), ComposerStateCodecError> {
    if outer_request_id != &value.request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.composerDraftSaved.requestId",
        });
    }
    builder.set_request_id(value.request_id.as_str());
    encode_scope(builder.reborrow().init_scope(), &value.scope);
    builder.set_revision(value.revision.get());
    Ok(())
}

/// Decodes one draft-save acknowledgement and checks its correlation.
pub fn decode_composer_draft_saved(
    value: composer_state_capnp::composer_draft_saved::Reader<'_>,
    outer_request_id: &RequestId,
) -> Result<ComposerDraftSaved, ComposerStateCodecError> {
    let request_id = parse_request_id(
        read_text(
            value.get_request_id(),
            "response.composerDraftSaved.requestId",
        )?,
        "response.composerDraftSaved.requestId",
    )?;
    if &request_id != outer_request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.composerDraftSaved.requestId",
        });
    }
    Ok(ComposerDraftSaved {
        request_id,
        scope: decode_scope(value.get_scope()?, "response.composerDraftSaved.scope")?,
        revision: decode_revision(value.get_revision(), "response.composerDraftSaved.revision")?,
    })
}

/// Encodes one draft read.
pub fn encode_read_composer_draft_request(
    mut builder: composer_state_capnp::read_composer_draft_request::Builder<'_>,
    value: &ReadComposerDraft,
) {
    encode_scope(builder.reborrow().init_scope(), &value.scope);
}

/// Decodes one draft read.
pub fn decode_read_composer_draft_request(
    value: composer_state_capnp::read_composer_draft_request::Reader<'_>,
) -> Result<ReadComposerDraft, ComposerStateCodecError> {
    Ok(ReadComposerDraft {
        scope: decode_scope(value.get_scope()?, "request.readComposerDraft.scope")?,
    })
}

/// Encodes one draft read result.
pub fn encode_composer_draft_result(
    mut builder: composer_state_capnp::composer_draft_result::Builder<'_>,
    value: &ComposerDraftResult,
) -> Result<(), ComposerStateCodecError> {
    encode_scope(builder.reborrow().init_scope(), &value.scope);
    let Some(draft) = &value.draft else {
        return Ok(());
    };
    let mut encoded = builder.reborrow().init_draft();
    encoded.set_revision(draft.revision().get());
    encoded.set_text(draft.text().as_str());
    encoded.set_updated_at_millis(draft.updated_at().as_millis());
    let field = "response.composerDraft.draft.attachments";
    encode_references(
        encoded
            .reborrow()
            .init_attachments(list_length(field, draft.attachments().len())?),
        draft.attachments(),
        field,
    )
}

/// Decodes one draft read result.
pub fn decode_composer_draft_result(
    value: composer_state_capnp::composer_draft_result::Reader<'_>,
) -> Result<ComposerDraftResult, ComposerStateCodecError> {
    let scope = decode_scope(value.get_scope()?, "response.composerDraft.scope")?;
    let draft = if value.has_draft() {
        let draft = value.get_draft()?;
        Some(
            ComposerDraft::new(
                decode_revision(
                    draft.get_revision(),
                    "response.composerDraft.draft.revision",
                )?,
                decode_authored_text(draft.get_text(), "response.composerDraft.draft.text")?,
                decode_references(
                    draft.get_attachments()?,
                    "response.composerDraft.draft.attachments",
                )?,
                UnixMillis::from_millis(draft.get_updated_at_millis()),
            )
            .map_err(|_| state_value("response.composerDraft.draft"))?,
        )
    } else {
        None
    };
    Ok(ComposerDraftResult { scope, draft })
}

/// Encodes one attachment upload.
pub fn encode_upload_composer_attachment_request(
    builder: composer_state_capnp::upload_composer_attachment_request::Builder<'_>,
    value: &UploadComposerAttachment,
) {
    let mut image = builder.init_image();
    image.set_mime_type(value.image.mime_type_str());
    image.set_name(value.image.name());
    image.set_bytes(value.image.bytes());
}

/// Decodes one attachment upload using the parent envelope request id.
pub fn decode_upload_composer_attachment_request(
    value: composer_state_capnp::upload_composer_attachment_request::Reader<'_>,
    request_id: RequestId,
) -> Result<UploadComposerAttachment, ComposerStateCodecError> {
    let field = "request.uploadComposerAttachment.image";
    let image = value.get_image()?;
    let bytes = image.get_bytes()?;
    if bytes.is_empty() || bytes.len() > artisan_domain::COMPOSER_ATTACHMENT_MAX_BYTES {
        return Err(ComposerStateCodecError::Image { field });
    }
    let image = artisan_domain::ComposerImage::new(
        read_text(image.get_mime_type(), field)?,
        bytes.to_vec(),
        read_text(image.get_name(), field)?,
    )
    .map_err(|_| ComposerStateCodecError::Image { field })?;
    Ok(UploadComposerAttachment { request_id, image })
}

/// Encodes one upload acknowledgement after checking its correlation.
pub fn encode_composer_attachment_uploaded(
    mut builder: composer_state_capnp::composer_attachment_uploaded::Builder<'_>,
    outer_request_id: &RequestId,
    value: &ComposerAttachmentUploaded,
) -> Result<(), ComposerStateCodecError> {
    if outer_request_id != &value.request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.composerAttachmentUploaded.requestId",
        });
    }
    builder.set_request_id(value.request_id.as_str());
    encode_reference(builder.reborrow().init_reference(), &value.reference);
    Ok(())
}

/// Decodes one upload acknowledgement and checks its correlation.
pub fn decode_composer_attachment_uploaded(
    value: composer_state_capnp::composer_attachment_uploaded::Reader<'_>,
    outer_request_id: &RequestId,
) -> Result<ComposerAttachmentUploaded, ComposerStateCodecError> {
    let field = "response.composerAttachmentUploaded.requestId";
    let request_id = parse_request_id(read_text(value.get_request_id(), field)?, field)?;
    if &request_id != outer_request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch { field });
    }
    Ok(ComposerAttachmentUploaded {
        request_id,
        reference: decode_reference(
            value.get_reference()?,
            "response.composerAttachmentUploaded.reference",
        )?,
    })
}

/// Encodes one stored-attachment read.
pub fn encode_read_composer_attachment_request(
    mut builder: composer_state_capnp::read_composer_attachment_request::Builder<'_>,
    value: &ReadComposerAttachment,
) {
    builder.set_digest(value.digest.as_bytes());
}

/// Decodes one stored-attachment read.
pub fn decode_read_composer_attachment_request(
    value: composer_state_capnp::read_composer_attachment_request::Reader<'_>,
) -> Result<ReadComposerAttachment, ComposerStateCodecError> {
    Ok(ReadComposerAttachment {
        digest: decode_digest(value.get_digest(), "request.readComposerAttachment.digest")?,
    })
}

/// Encodes one stored-attachment read result.
pub fn encode_composer_attachment_result(
    mut builder: composer_state_capnp::composer_attachment_result::Builder<'_>,
    value: &ComposerAttachmentResult,
) {
    builder.set_digest(value.digest.as_bytes());
    builder.set_mime_type(value.mime_type.as_str());
    builder.set_bytes(&value.bytes);
}

/// Decodes one stored-attachment read result.
pub fn decode_composer_attachment_result(
    value: composer_state_capnp::composer_attachment_result::Reader<'_>,
) -> Result<ComposerAttachmentResult, ComposerStateCodecError> {
    let field = "response.composerAttachment";
    let bytes = value.get_bytes()?;
    if bytes.is_empty() || bytes.len() > artisan_domain::COMPOSER_ATTACHMENT_MAX_BYTES {
        return Err(ComposerStateCodecError::Image { field });
    }
    Ok(ComposerAttachmentResult {
        digest: decode_digest(value.get_digest(), field)?,
        mime_type: decode_mime(value.get_mime_type(), field)?,
        bytes: bytes.to_vec(),
    })
}

/// Encodes one stored-attachment message.
pub fn encode_queue_stored_message_request(
    mut builder: composer_state_capnp::queue_stored_message_request::Builder<'_>,
    value: &QueueStoredMessage,
) -> Result<(), ComposerStateCodecError> {
    builder.set_thread_id(value.thread_id().as_str());
    if let Some(text) = value.text() {
        builder.set_text(text.as_str());
    }
    builder.set_steer_run_id(
        value
            .steer_target()
            .map_or("", |target| target.run_id().as_str()),
    );
    let field = "request.queueStoredMessage.attachments";
    encode_references(
        builder
            .reborrow()
            .init_attachments(list_length(field, value.attachments().len())?),
        value.attachments(),
        field,
    )
}

/// Decodes one stored-attachment message using the parent envelope id.
pub fn decode_queue_stored_message_request(
    value: composer_state_capnp::queue_stored_message_request::Reader<'_>,
    request_id: RequestId,
) -> Result<QueueStoredMessage, ComposerStateCodecError> {
    let text = if value.has_text() {
        Some(decode_authored_text(
            value.get_text(),
            "request.queueStoredMessage.text",
        )?)
    } else {
        None
    };
    let steer = read_text(
        value.get_steer_run_id(),
        "request.queueStoredMessage.steerRunId",
    )?;
    let steer_target = if steer.is_empty() {
        None
    } else {
        Some(SteerTarget::new(parse_run_id(
            steer,
            "request.queueStoredMessage.steerRunId",
        )?))
    };
    QueueStoredMessage::new(
        request_id,
        parse_thread_id(
            read_text(value.get_thread_id(), "request.queueStoredMessage.threadId")?,
            "request.queueStoredMessage.threadId",
        )?,
        text,
        decode_references(
            value.get_attachments()?,
            "request.queueStoredMessage.attachments",
        )?,
        steer_target,
    )
    .map_err(|_| state_value("request.queueStoredMessage.attachments"))
}
