//! Attachment-scope validation and image reference codecs for summaries.

#![forbid(unsafe_code)]

use super::helpers::*;
use super::*;
pub(super) fn validate_failed_summary_attachment_scope(
    value: &FailedMessageSummary,
) -> Result<(), ComposerStateCodecError> {
    if value.attachments.len() > COMPOSER_STATE_IMAGE_MAX_COUNT {
        return Err(ComposerStateCodecError::Listing {
            field: "response.failedMessages.messages.attachments",
        });
    }
    for (index, attachment) in value.attachments.iter().enumerate() {
        if attachment.message_id != value.message_id {
            return Err(ComposerStateCodecError::Listing {
                field: "response.failedMessages.messages.attachments.messageId",
            });
        }
        if attachment.thread_id != value.thread_id {
            return Err(ComposerStateCodecError::Listing {
                field: "response.failedMessages.messages.attachments.threadId",
            });
        }
        if attachment.index != u32::try_from(index).unwrap_or(u32::MAX) {
            return Err(ComposerStateCodecError::Listing {
                field: "response.failedMessages.messages.attachments.index",
            });
        }
    }
    Ok(())
}

pub(super) fn validate_summary_attachment_scope(
    value: &QueuedMessageSummary,
) -> Result<(), ComposerStateCodecError> {
    if value.attachments.len() > COMPOSER_STATE_IMAGE_MAX_COUNT {
        return Err(ComposerStateCodecError::Listing {
            field: "response.queuedMessages.messages.attachments",
        });
    }
    for (index, attachment) in value.attachments.iter().enumerate() {
        if attachment.message_id != value.message_id {
            return Err(ComposerStateCodecError::Listing {
                field: "response.queuedMessages.messages.attachments.messageId",
            });
        }
        if attachment.thread_id != value.thread_id {
            return Err(ComposerStateCodecError::Listing {
                field: "response.queuedMessages.messages.attachments.threadId",
            });
        }
        if attachment.index != u32::try_from(index).unwrap_or(u32::MAX) {
            return Err(ComposerStateCodecError::Listing {
                field: "response.queuedMessages.messages.attachments.index",
            });
        }
    }
    Ok(())
}

pub(super) fn encode_image_attachment_ref(
    mut builder: composer_state_capnp::image_attachment_ref::Builder<'_>,
    value: &ImageAttachmentRef,
) {
    builder.set_message_id(value.message_id.as_str());
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_index(value.index);
    builder.set_mime_type(value.mime_type_str());
    builder.set_name(value.name.as_str());
    builder.set_size_bytes(value.size_bytes);
    builder.set_digest(&value.digest[..]);
}

pub(super) fn decode_image_attachment_ref(
    value: composer_state_capnp::image_attachment_ref::Reader<'_>,
    expected_message_id: &MessageId,
    expected_thread_id: &ThreadId,
    expected_index: u32,
) -> Result<ImageAttachmentRef, ComposerStateCodecError> {
    if value.get_index() != expected_index {
        return Err(ComposerStateCodecError::ImageReference {
            field: "response.queuedMessages.messages.attachments.index",
        });
    }
    let message_id = parse_message_id(
        read_text(
            value.get_message_id(),
            "response.queuedMessages.messages.attachments.messageId",
        )?,
        "response.queuedMessages.messages.attachments.messageId",
    )?;
    let thread_id = parse_thread_id(
        read_text(
            value.get_thread_id(),
            "response.queuedMessages.messages.attachments.threadId",
        )?,
        "response.queuedMessages.messages.attachments.threadId",
    )?;
    if &message_id != expected_message_id {
        return Err(ComposerStateCodecError::ImageReference {
            field: "response.queuedMessages.messages.attachments.messageId",
        });
    }
    if &thread_id != expected_thread_id {
        return Err(ComposerStateCodecError::ImageReference {
            field: "response.queuedMessages.messages.attachments.threadId",
        });
    }
    let digest: [u8; 32] = value.get_digest()?.to_vec().try_into().map_err(|_| {
        ComposerStateCodecError::ImageReference {
            field: "response.queuedMessages.messages.attachments.digest",
        }
    })?;
    ImageAttachmentRef::new(
        message_id,
        thread_id,
        expected_index,
        read_text(
            value.get_mime_type(),
            "response.queuedMessages.messages.attachments.mimeType",
        )?,
        read_text(
            value.get_name(),
            "response.queuedMessages.messages.attachments.name",
        )?,
        value.get_size_bytes(),
        digest,
    )
    .map_err(|_| ComposerStateCodecError::ImageReference {
        field: "response.queuedMessages.messages.attachments",
    })
}
