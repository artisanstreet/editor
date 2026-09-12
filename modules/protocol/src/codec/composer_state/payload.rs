//! Recalled-message payload and bounded queue-message payload codecs.

#![forbid(unsafe_code)]

use super::helpers::*;
use super::*;
/// Encodes an exact recalled-message result, including a typed optional
/// payload.
pub fn encode_recalled_message_result(
    mut builder: composer_state_capnp::recalled_message_result::Builder<'_>,
    value: &RecalledMessageResult,
) -> Result<(), ComposerStateCodecError> {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_message_id(value.message_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
    if let Some(payload) = &value.payload {
        encode_queue_message_payload(builder.reborrow().init_payload(), payload)?;
    }
    Ok(())
}

/// Decodes an exact recalled-message result while retaining absent payload
/// versus a present image/text payload.
pub fn decode_recalled_message_result(
    value: composer_state_capnp::recalled_message_result::Reader<'_>,
) -> Result<RecalledMessageResult, ComposerStateCodecError> {
    let payload = if value.has_payload() {
        Some(decode_queue_message_payload(value.get_payload()?)?)
    } else {
        None
    };
    RecalledMessageResult::new(
        parse_thread_id(
            read_text(value.get_thread_id(), "response.recalledMessage.threadId")?,
            "response.recalledMessage.threadId",
        )?,
        parse_message_id(
            read_text(value.get_message_id(), "response.recalledMessage.messageId")?,
            "response.recalledMessage.messageId",
        )?,
        parse_request_id(
            read_text(
                value.get_original_request_id(),
                "response.recalledMessage.originalRequestId",
            )?,
            "response.recalledMessage.originalRequestId",
        )?,
        payload,
    )
    .map_err(|_| ComposerStateCodecError::StateValue {
        field: "response.recalledMessage.payload",
    })
}

/// Validates a recalled result against the exact authenticated read scope.
pub fn validate_recalled_message_scope(
    query: &ReadRecalledMessage,
    result: &RecalledMessageResult,
) -> Result<(), ComposerStateCodecError> {
    if query.thread_id != result.thread_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "recalledMessage.threadId",
        });
    }
    if query.message_id != result.message_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "recalledMessage.messageId",
        });
    }
    if query.original_request_id != result.original_request_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "recalledMessage.originalRequestId",
        });
    }
    Ok(())
}

fn encode_queue_message_payload(
    mut builder: composer_state_capnp::queue_message_payload::Builder<'_>,
    value: &QueueMessagePayload,
) -> Result<(), ComposerStateCodecError> {
    validate_payload_bounds(value).map_err(|_| ComposerStateCodecError::StateValue {
        field: "composerState.payload",
    })?;
    if let Some(text) = value.text() {
        builder.set_text(text.as_str());
    }
    let mut attachments = builder.reborrow().init_attachments(list_length(
        "composerState.payload.attachments",
        value.attachments().len(),
    )?);
    for (index, attachment) in value.attachments().iter().enumerate() {
        let mut encoded = attachments
            .reborrow()
            .get(list_index("composerState.payload.attachments", index)?);
        encoded.set_mime_type(attachment.mime_type_str());
        encoded.set_name(attachment.name());
        encoded.set_bytes(attachment.bytes());
    }
    Ok(())
}

fn decode_queue_message_payload(
    value: composer_state_capnp::queue_message_payload::Reader<'_>,
) -> Result<QueueMessagePayload, ComposerStateCodecError> {
    let encoded_attachments = value.get_attachments()?;
    let count = usize::try_from(encoded_attachments.len()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "composerState.payload.attachments",
            length: usize::MAX,
        }
    })?;
    if count > COMPOSER_STATE_IMAGE_MAX_COUNT {
        return Err(ComposerStateCodecError::Payload {
            field: "composerState.payload.attachments",
            source: QueueMessagePayloadError::TooManyAttachments {
                count,
                maximum: COMPOSER_STATE_IMAGE_MAX_COUNT,
            },
        });
    }

    // Validate every encoded byte slice and the aggregate before creating a
    // Vec or copying one byte. This keeps hostile list lengths and image data
    // from causing an allocation before the finite budget is known.
    let mut total_bytes = 0usize;
    for attachment in encoded_attachments {
        let bytes = attachment.get_bytes()?;
        if bytes.is_empty() {
            return Err(ComposerStateCodecError::Image {
                field: "composerState.payload.attachments.bytes",
            });
        }
        if bytes.len() > COMPOSER_STATE_IMAGE_MAX_BYTES {
            return Err(ComposerStateCodecError::Image {
                field: "composerState.payload.attachments.bytes",
            });
        }
        total_bytes =
            total_bytes
                .checked_add(bytes.len())
                .ok_or(ComposerStateCodecError::Image {
                    field: "composerState.payload.attachments.bytes",
                })?;
        if total_bytes > COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES {
            return Err(ComposerStateCodecError::Payload {
                field: "composerState.payload.attachments",
                source: QueueMessagePayloadError::AttachmentsTooLarge {
                    length: total_bytes,
                    maximum: COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES,
                },
            });
        }
    }

    let mut attachments = Vec::with_capacity(count);
    for attachment in encoded_attachments {
        let mime_type = read_text(
            attachment.get_mime_type(),
            "composerState.payload.attachments.mimeType",
        )?;
        let name = read_text(
            attachment.get_name(),
            "composerState.payload.attachments.name",
        )?;
        let bytes = attachment.get_bytes()?.to_vec();
        attachments.push(ImageAttachment::new(mime_type, bytes, name).map_err(|_| {
            ComposerStateCodecError::Image {
                field: "composerState.payload.attachments",
            }
        })?);
    }
    let text = if value.has_text() {
        Some(
            AuthoredText::parse(read_text(value.get_text(), "composerState.payload.text")?)
                .map_err(|source| ComposerStateCodecError::AuthoredText {
                    field: "composerState.payload.text",
                    source,
                })?,
        )
    } else {
        None
    };
    QueueMessagePayload::new(text, attachments).map_err(|source| ComposerStateCodecError::Payload {
        field: "composerState.payload",
        source,
    })
}
