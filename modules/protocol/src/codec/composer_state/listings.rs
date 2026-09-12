//! Queued and failed message listings, summaries, and withdrawal results.

#![forbid(unsafe_code)]

use super::attachments::*;
use super::helpers::*;
use super::*;
/// Encodes an exact queued-message listing.
pub fn encode_queued_message_listing(
    mut builder: composer_state_capnp::queued_message_listing::Builder<'_>,
    value: &QueuedMessageListing,
) -> Result<(), ComposerStateCodecError> {
    builder.set_thread_id(value.thread_id().as_str());
    builder.set_order(encode_list_order(value.order()));
    builder.set_limit(u16::try_from(value.limit()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "response.queuedMessages.limit",
            length: value.limit(),
        }
    })?);
    let mut messages = builder.reborrow().init_messages(list_length(
        "response.queuedMessages.messages",
        value.messages().len(),
    )?);
    for (index, message) in value.messages().iter().enumerate() {
        encode_queued_message_summary(
            messages
                .reborrow()
                .get(list_index("response.queuedMessages.messages", index)?),
            message,
        )?;
    }
    builder.set_total_count(value.total_count());
    builder.set_has_more(value.has_more());
    Ok(())
}

/// Decodes and validates an exact queued-message listing. Count and
/// `hasMore` consistency are checked before constructing the owned listing.
pub fn decode_queued_message_listing(
    value: composer_state_capnp::queued_message_listing::Reader<'_>,
) -> Result<QueuedMessageListing, ComposerStateCodecError> {
    let thread_id = parse_thread_id(
        read_text(value.get_thread_id(), "response.queuedMessages.threadId")?,
        "response.queuedMessages.threadId",
    )?;
    let order = decode_list_order(value.get_order(), "response.queuedMessages.order")?;
    let limit = usize::from(value.get_limit());
    let encoded_messages = value.get_messages()?;
    let count = usize::try_from(encoded_messages.len()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "response.queuedMessages.messages",
            length: usize::MAX,
        }
    })?;
    if count > QUEUED_MESSAGE_LIST_MAX {
        return Err(ComposerStateCodecError::Listing {
            field: "response.queuedMessages.messages",
        });
    }

    let mut messages = Vec::with_capacity(count);
    for encoded in encoded_messages {
        messages.push(decode_queued_message_summary(encoded)?);
    }
    let total_count = value.get_total_count();
    let has_more = value.get_has_more();
    let listing = QueuedMessageListing::new(thread_id, order, limit, total_count, messages)
        .map_err(|_| ComposerStateCodecError::Listing {
            field: "response.queuedMessages",
        })?;
    if listing.has_more() != has_more {
        return Err(ComposerStateCodecError::Listing {
            field: "response.queuedMessages.hasMore",
        });
    }
    Ok(listing)
}

/// Encodes a bounded list-failed-messages request into an imported builder.
pub fn encode_list_failed_messages_request(
    mut builder: composer_state_capnp::list_failed_messages_request::Builder<'_>,
    value: &ListFailedMessages,
) -> Result<(), ComposerStateCodecError> {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_limit(u16::try_from(value.limit).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "request.listFailedMessages.limit",
            length: value.limit,
        }
    })?);
    Ok(())
}

/// Decodes a bounded list-failed-messages request from an imported reader.
pub fn decode_list_failed_messages_request(
    value: composer_state_capnp::list_failed_messages_request::Reader<'_>,
) -> Result<ListFailedMessages, ComposerStateCodecError> {
    let thread_id = parse_thread_id(
        read_text(value.get_thread_id(), "request.listFailedMessages.threadId")?,
        "request.listFailedMessages.threadId",
    )?;
    ListFailedMessages::new(thread_id, usize::from(value.get_limit())).map_err(|_| {
        ComposerStateCodecError::Listing {
            field: "request.listFailedMessages.limit",
        }
    })
}

/// Encodes an exact failed-dispatch listing.
pub fn encode_failed_message_listing(
    mut builder: composer_state_capnp::failed_message_listing::Builder<'_>,
    value: &FailedMessageListing,
) -> Result<(), ComposerStateCodecError> {
    builder.set_thread_id(value.thread_id().as_str());
    builder.set_limit(u16::try_from(value.limit()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "response.failedMessages.limit",
            length: value.limit(),
        }
    })?);
    let mut messages = builder.reborrow().init_messages(list_length(
        "response.failedMessages.messages",
        value.messages().len(),
    )?);
    for (index, message) in value.messages().iter().enumerate() {
        encode_failed_message_summary(
            messages
                .reborrow()
                .get(list_index("response.failedMessages.messages", index)?),
            message,
        )?;
    }
    builder.set_total_count(value.total_count());
    builder.set_has_more(value.has_more());
    Ok(())
}

/// Decodes and validates an exact failed-dispatch listing. Count and
/// `hasMore` consistency are checked before constructing the owned listing.
pub fn decode_failed_message_listing(
    value: composer_state_capnp::failed_message_listing::Reader<'_>,
) -> Result<FailedMessageListing, ComposerStateCodecError> {
    let thread_id = parse_thread_id(
        read_text(value.get_thread_id(), "response.failedMessages.threadId")?,
        "response.failedMessages.threadId",
    )?;
    let limit = usize::from(value.get_limit());
    let encoded_messages = value.get_messages()?;
    let count = usize::try_from(encoded_messages.len()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "response.failedMessages.messages",
            length: usize::MAX,
        }
    })?;
    if count > FAILED_MESSAGE_LIST_MAX {
        return Err(ComposerStateCodecError::Listing {
            field: "response.failedMessages.messages",
        });
    }

    let mut messages = Vec::with_capacity(count);
    for encoded in encoded_messages {
        messages.push(decode_failed_message_summary(encoded)?);
    }
    let total_count = value.get_total_count();
    let has_more = value.get_has_more();
    let listing =
        FailedMessageListing::new(thread_id, limit, total_count, messages).map_err(|_| {
            ComposerStateCodecError::Listing {
                field: "response.failedMessages",
            }
        })?;
    if listing.has_more() != has_more {
        return Err(ComposerStateCodecError::Listing {
            field: "response.failedMessages.hasMore",
        });
    }
    Ok(listing)
}

/// Encodes a withdrawal receipt and checks its nested receipt id against the
/// enclosing response request id before writing any fields.
pub fn encode_queued_message_withdrawal_result(
    mut builder: composer_state_capnp::queued_message_withdrawal_result::Builder<'_>,
    outer_request_id: &RequestId,
    value: &QueuedMessageWithdrawalResult,
) -> Result<(), ComposerStateCodecError> {
    validate_withdrawal_response_correlation(outer_request_id, value)?;
    builder.set_request_id(value.receipt.request_id.as_str());
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_message_id(value.message_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
    builder.set_accepted_at_millis(value.accepted_at.as_millis());
    builder.set_disposition(encode_disposition(value.receipt.disposition));
    builder.set_outcome(encode_withdrawal_outcome(value.outcome));
    Ok(())
}

/// Decodes a withdrawal receipt and enforces exact outer response
/// correlation.
pub fn decode_queued_message_withdrawal_result(
    value: composer_state_capnp::queued_message_withdrawal_result::Reader<'_>,
    outer_request_id: &RequestId,
) -> Result<QueuedMessageWithdrawalResult, ComposerStateCodecError> {
    let nested_request_id = parse_request_id(
        read_text(
            value.get_request_id(),
            "response.messageWithdrawn.requestId",
        )?,
        "response.messageWithdrawn.requestId",
    )?;
    if &nested_request_id != outer_request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.messageWithdrawn.requestId",
        });
    }
    Ok(QueuedMessageWithdrawalResult {
        receipt: CommandReceipt {
            request_id: nested_request_id,
            disposition: decode_disposition(
                value.get_disposition(),
                "response.messageWithdrawn.disposition",
            )?,
        },
        thread_id: parse_thread_id(
            read_text(value.get_thread_id(), "response.messageWithdrawn.threadId")?,
            "response.messageWithdrawn.threadId",
        )?,
        message_id: parse_message_id(
            read_text(
                value.get_message_id(),
                "response.messageWithdrawn.messageId",
            )?,
            "response.messageWithdrawn.messageId",
        )?,
        original_request_id: parse_request_id(
            read_text(
                value.get_original_request_id(),
                "response.messageWithdrawn.originalRequestId",
            )?,
            "response.messageWithdrawn.originalRequestId",
        )?,
        accepted_at: UnixMillis::from_millis(value.get_accepted_at_millis()),
        outcome: decode_withdrawal_outcome(
            value.get_outcome(),
            "response.messageWithdrawn.outcome",
        )?,
    })
}

fn encode_queued_message_summary(
    mut builder: composer_state_capnp::queued_message_summary::Builder<'_>,
    value: &QueuedMessageSummary,
) -> Result<(), ComposerStateCodecError> {
    validate_summary_attachment_scope(value)?;
    builder.set_message_id(value.message_id.as_str());
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
    if let Some(text) = &value.text {
        builder.set_text(text.as_str());
    }
    let mut attachments = builder.reborrow().init_attachments(list_length(
        "response.queuedMessages.messages.attachments",
        value.attachments.len(),
    )?);
    for (index, attachment) in value.attachments.iter().enumerate() {
        encode_image_attachment_ref(
            attachments.reborrow().get(list_index(
                "response.queuedMessages.messages.attachments",
                index,
            )?),
            attachment,
        );
    }
    builder.set_accepted_at_millis(value.accepted_at.as_millis());
    if let Some(error) = value.last_error.as_ref() {
        builder.set_last_error(error.as_str());
    }
    Ok(())
}

fn decode_queued_message_summary(
    value: composer_state_capnp::queued_message_summary::Reader<'_>,
) -> Result<QueuedMessageSummary, ComposerStateCodecError> {
    let message_id = parse_message_id(
        read_text(
            value.get_message_id(),
            "response.queuedMessages.messages.messageId",
        )?,
        "response.queuedMessages.messages.messageId",
    )?;
    let thread_id = parse_thread_id(
        read_text(
            value.get_thread_id(),
            "response.queuedMessages.messages.threadId",
        )?,
        "response.queuedMessages.messages.threadId",
    )?;
    let original_request_id = parse_request_id(
        read_text(
            value.get_original_request_id(),
            "response.queuedMessages.messages.originalRequestId",
        )?,
        "response.queuedMessages.messages.originalRequestId",
    )?;
    let text = if value.has_text() {
        Some(
            AuthoredText::parse(read_text(
                value.get_text(),
                "response.queuedMessages.messages.text",
            )?)
            .map_err(|source| ComposerStateCodecError::AuthoredText {
                field: "response.queuedMessages.messages.text",
                source,
            })?,
        )
    } else {
        None
    };
    let encoded_attachments = value.get_attachments()?;
    let count = usize::try_from(encoded_attachments.len()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "response.queuedMessages.messages.attachments",
            length: usize::MAX,
        }
    })?;
    if count > COMPOSER_STATE_IMAGE_MAX_COUNT {
        return Err(ComposerStateCodecError::Listing {
            field: "response.queuedMessages.messages.attachments",
        });
    }
    let mut attachments = Vec::with_capacity(count);
    for (index, encoded) in encoded_attachments.iter().enumerate() {
        attachments.push(decode_image_attachment_ref(
            encoded,
            &message_id,
            &thread_id,
            u32::try_from(index).map_err(|_| ComposerStateCodecError::CollectionTooLarge {
                field: "response.queuedMessages.messages.attachments",
                length: index,
            })?,
        )?);
    }
    let last_error = if value.has_last_error() {
        Some(
            DispatchError::parse(read_text(
                value.get_last_error(),
                "response.queuedMessages.messages.lastError",
            )?)
            .map_err(|_| ComposerStateCodecError::Listing {
                field: "response.queuedMessages.messages.lastError",
            })?,
        )
    } else {
        None
    };
    Ok(QueuedMessageSummary {
        message_id,
        thread_id,
        original_request_id,
        text,
        attachments,
        accepted_at: UnixMillis::from_millis(value.get_accepted_at_millis()),
        last_error,
    })
}

fn encode_failed_message_summary(
    mut builder: composer_state_capnp::failed_message_summary::Builder<'_>,
    value: &FailedMessageSummary,
) -> Result<(), ComposerStateCodecError> {
    validate_failed_summary_attachment_scope(value)?;
    builder.set_message_id(value.message_id.as_str());
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
    if let Some(text) = &value.text {
        builder.set_text(text.as_str());
    }
    let mut attachments = builder.reborrow().init_attachments(list_length(
        "response.failedMessages.messages.attachments",
        value.attachments.len(),
    )?);
    for (index, attachment) in value.attachments.iter().enumerate() {
        encode_image_attachment_ref(
            attachments.reborrow().get(list_index(
                "response.failedMessages.messages.attachments",
                index,
            )?),
            attachment,
        );
    }
    builder.set_accepted_at_millis(value.accepted_at.as_millis());
    builder.set_failed_at_millis(value.failed_at.as_millis());
    builder.set_reason(value.reason.as_str());
    Ok(())
}

fn decode_failed_message_summary(
    value: composer_state_capnp::failed_message_summary::Reader<'_>,
) -> Result<FailedMessageSummary, ComposerStateCodecError> {
    let message_id = parse_message_id(
        read_text(
            value.get_message_id(),
            "response.failedMessages.messages.messageId",
        )?,
        "response.failedMessages.messages.messageId",
    )?;
    let thread_id = parse_thread_id(
        read_text(
            value.get_thread_id(),
            "response.failedMessages.messages.threadId",
        )?,
        "response.failedMessages.messages.threadId",
    )?;
    let original_request_id = parse_request_id(
        read_text(
            value.get_original_request_id(),
            "response.failedMessages.messages.originalRequestId",
        )?,
        "response.failedMessages.messages.originalRequestId",
    )?;
    let text = if value.has_text() {
        Some(
            AuthoredText::parse(read_text(
                value.get_text(),
                "response.failedMessages.messages.text",
            )?)
            .map_err(|source| ComposerStateCodecError::AuthoredText {
                field: "response.failedMessages.messages.text",
                source,
            })?,
        )
    } else {
        None
    };
    let encoded_attachments = value.get_attachments()?;
    let count = usize::try_from(encoded_attachments.len()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "response.failedMessages.messages.attachments",
            length: usize::MAX,
        }
    })?;
    if count > COMPOSER_STATE_IMAGE_MAX_COUNT {
        return Err(ComposerStateCodecError::Listing {
            field: "response.failedMessages.messages.attachments",
        });
    }
    let mut attachments = Vec::with_capacity(count);
    for (index, encoded) in encoded_attachments.iter().enumerate() {
        attachments.push(decode_image_attachment_ref(
            encoded,
            &message_id,
            &thread_id,
            u32::try_from(index).map_err(|_| ComposerStateCodecError::CollectionTooLarge {
                field: "response.failedMessages.messages.attachments",
                length: index,
            })?,
        )?);
    }
    if !value.has_reason() {
        return Err(ComposerStateCodecError::Listing {
            field: "response.failedMessages.messages.reason",
        });
    }
    let reason = DispatchError::parse(read_text(
        value.get_reason(),
        "response.failedMessages.messages.reason",
    )?)
    .map_err(|_| ComposerStateCodecError::Listing {
        field: "response.failedMessages.messages.reason",
    })?;
    Ok(FailedMessageSummary {
        message_id,
        thread_id,
        original_request_id,
        text,
        attachments,
        accepted_at: UnixMillis::from_millis(value.get_accepted_at_millis()),
        failed_at: UnixMillis::from_millis(value.get_failed_at_millis()),
        reason,
    })
}
