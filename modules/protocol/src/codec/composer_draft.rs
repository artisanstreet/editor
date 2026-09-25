//! Parent-union dispatch for composer requests: drafts, stored attachments,
//! stored-attachment messages, and model favorites. Draft payload codecs
//! live in the composer-state leaf; this module routes the parent arms.

#[allow(clippy::wildcard_imports)]
use super::*;

use crate::composer_state_codec as leaf;

/// Encodes one composer-draft request arm.
pub(crate) fn encode_composer_draft_request(
    mut builder: request::Builder<'_>,
    value: &ClientRequest,
) -> Result<(), ProtocolEncodeError> {
    let encoded = match value {
        ClientRequest::Command(Command::SaveComposerDraft(command)) => {
            leaf::encode_save_composer_draft_request(
                builder.reborrow().init_save_composer_draft(),
                command,
            )
        }
        ClientRequest::Command(Command::UploadComposerAttachment(command)) => {
            leaf::encode_upload_composer_attachment_request(
                builder.reborrow().init_upload_composer_attachment(),
                command,
            );
            Ok(())
        }
        ClientRequest::Command(Command::QueueStoredMessage(command)) => {
            leaf::encode_queue_stored_message_request(
                builder.reborrow().init_queue_stored_message(),
                command,
            )
        }
        ClientRequest::Query(Query::ReadComposerDraft(query)) => {
            leaf::encode_read_composer_draft_request(
                builder.reborrow().init_read_composer_draft(),
                query,
            );
            Ok(())
        }
        ClientRequest::Query(Query::ReadComposerAttachment(query)) => {
            leaf::encode_read_composer_attachment_request(
                builder.reborrow().init_read_composer_attachment(),
                query,
            );
            Ok(())
        }
        _ => Err(leaf::ComposerStateCodecError::StateValue {
            field: "request.composerDraft",
        }),
    };
    encoded.map_err(|_| ProtocolEncodeError::ComposerState)
}

/// Decodes one composer-draft request arm.
pub(crate) fn decode_composer_draft_request(
    value: request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(match value.which()? {
        request::Which::SaveComposerDraft(value) => {
            ClientRequest::Command(Command::SaveComposerDraft(
                leaf::decode_save_composer_draft_request(value?, request_id)?,
            ))
        }
        request::Which::UploadComposerAttachment(value) => {
            ClientRequest::Command(Command::UploadComposerAttachment(
                leaf::decode_upload_composer_attachment_request(value?, request_id)?,
            ))
        }
        request::Which::QueueStoredMessage(value) => {
            ClientRequest::Command(Command::QueueStoredMessage(
                leaf::decode_queue_stored_message_request(value?, request_id)?,
            ))
        }
        request::Which::ReadComposerDraft(value) => ClientRequest::Query(Query::ReadComposerDraft(
            leaf::decode_read_composer_draft_request(value?)?,
        )),
        request::Which::ReadComposerAttachment(value) => ClientRequest::Query(
            Query::ReadComposerAttachment(leaf::decode_read_composer_attachment_request(value?)?),
        ),
        _ => {
            return Err(leaf::ComposerStateCodecError::StateValue {
                field: "request.composerDraft",
            }
            .into());
        }
    })
}

/// Encodes one composer-draft response arm.
pub(crate) fn encode_composer_draft_response(
    mut builder: response::Builder<'_>,
    payload: &ResponsePayload,
    outer_request_id: &RequestId,
) -> Result<(), ProtocolEncodeError> {
    let encoded = match payload {
        ResponsePayload::ComposerDraftSaved(value) => leaf::encode_composer_draft_saved(
            builder.reborrow().init_composer_draft_saved(),
            outer_request_id,
            value,
        ),
        ResponsePayload::ComposerDraft(value) => {
            leaf::encode_composer_draft_result(builder.reborrow().init_composer_draft(), value)
        }
        ResponsePayload::ComposerAttachmentUploaded(value) => {
            leaf::encode_composer_attachment_uploaded(
                builder.reborrow().init_composer_attachment_uploaded(),
                outer_request_id,
                value,
            )
        }
        ResponsePayload::ComposerAttachment(value) => {
            leaf::encode_composer_attachment_result(
                builder.reborrow().init_composer_attachment(),
                value,
            );
            Ok(())
        }
        _ => Err(leaf::ComposerStateCodecError::StateValue {
            field: "response.composerDraft",
        }),
    };
    encoded.map_err(|_| ProtocolEncodeError::ComposerState)
}

/// Decodes one composer-draft response arm.
pub(crate) fn decode_composer_draft_response(
    value: response::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    Ok(match value.which()? {
        response::Which::ComposerDraftSaved(value) => ResponsePayload::ComposerDraftSaved(
            leaf::decode_composer_draft_saved(value?, request_id)?,
        ),
        response::Which::ComposerDraft(value) => {
            ResponsePayload::ComposerDraft(leaf::decode_composer_draft_result(value?)?)
        }
        response::Which::ComposerAttachmentUploaded(value) => {
            ResponsePayload::ComposerAttachmentUploaded(leaf::decode_composer_attachment_uploaded(
                value?, request_id,
            )?)
        }
        response::Which::ComposerAttachment(value) => {
            ResponsePayload::ComposerAttachment(leaf::decode_composer_attachment_result(value?)?)
        }
        _ => {
            return Err(leaf::ComposerStateCodecError::StateValue {
                field: "response.composerDraft",
            }
            .into());
        }
    })
}

/// Decodes one model-favorite mutation using the parent envelope request id.
pub(crate) fn decode_set_model_favorite(
    command: artisan_capnp::set_model_favorite_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let catalog_revision = CatalogRevision::parse(read_text(
        command.get_catalog_revision(),
        "request.setModelFavorite.catalogRevision",
    )?)?;
    let model_id = ModelFavoriteId::parse(read_text(
        command.get_model_id(),
        "request.setModelFavorite.modelId",
    )?)?;
    Ok(ClientRequest::Command(Command::SetModelFavorite(
        SetModelFavorite::new(
            request_id,
            parse_thread_id(
                read_text(command.get_thread_id(), "request.setModelFavorite.threadId")?,
                "request.setModelFavorite.threadId",
            )?,
            parse_profile_id(
                read_text(
                    command.get_profile_id(),
                    "request.setModelFavorite.profileId",
                )?,
                "request.setModelFavorite.profileId",
            )?,
            catalog_revision,
            model_id,
            command.get_favorite(),
        ),
    )))
}
