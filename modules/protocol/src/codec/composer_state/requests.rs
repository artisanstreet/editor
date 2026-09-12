//! Queued-message, withdrawal, recalled-read, and run-usage request codecs.

#![forbid(unsafe_code)]

use super::helpers::*;
use super::*;
/// Encodes a bounded list-queued-messages request into an imported builder.
pub fn encode_list_queued_messages_request(
    mut builder: composer_state_capnp::list_queued_messages_request::Builder<'_>,
    value: &ListQueuedMessages,
) -> Result<(), ComposerStateCodecError> {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_order(encode_list_order(value.order));
    builder.set_limit(u16::try_from(value.limit).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "request.listQueuedMessages.limit",
            length: value.limit,
        }
    })?);
    Ok(())
}

/// Decodes a bounded list-queued-messages request from an imported reader.
pub fn decode_list_queued_messages_request(
    value: composer_state_capnp::list_queued_messages_request::Reader<'_>,
) -> Result<ListQueuedMessages, ComposerStateCodecError> {
    let thread_id = parse_thread_id(
        read_text(value.get_thread_id(), "request.listQueuedMessages.threadId")?,
        "request.listQueuedMessages.threadId",
    )?;
    let order = decode_list_order(value.get_order(), "request.listQueuedMessages.order")?;
    ListQueuedMessages::new(thread_id, order, usize::from(value.get_limit())).map_err(|_| {
        ComposerStateCodecError::Listing {
            field: "request.listQueuedMessages.limit",
        }
    })
}

/// Encodes a withdrawal command. The client request id is the parent
/// envelope message id and is therefore intentionally not repeated here.
pub fn encode_withdraw_queued_message_request(
    mut builder: composer_state_capnp::withdraw_queued_message_request::Builder<'_>,
    value: &WithdrawQueuedMessageCommand,
) {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_message_id(value.message_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
}

/// Decodes a withdrawal command using the parent envelope request id as its
/// durable idempotency identity.
pub fn decode_withdraw_queued_message_request(
    value: composer_state_capnp::withdraw_queued_message_request::Reader<'_>,
    request_id: RequestId,
) -> Result<WithdrawQueuedMessageCommand, ComposerStateCodecError> {
    Ok(WithdrawQueuedMessageCommand::new(
        request_id,
        parse_thread_id(
            read_text(
                value.get_thread_id(),
                "request.withdrawQueuedMessage.threadId",
            )?,
            "request.withdrawQueuedMessage.threadId",
        )?,
        parse_message_id(
            read_text(
                value.get_message_id(),
                "request.withdrawQueuedMessage.messageId",
            )?,
            "request.withdrawQueuedMessage.messageId",
        )?,
        parse_request_id(
            read_text(
                value.get_original_request_id(),
                "request.withdrawQueuedMessage.originalRequestId",
            )?,
            "request.withdrawQueuedMessage.originalRequestId",
        )?,
    ))
}

/// Encodes an exact recalled-message query.
pub fn encode_read_recalled_message_request(
    mut builder: composer_state_capnp::read_recalled_message_request::Builder<'_>,
    value: &ReadRecalledMessage,
) {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_message_id(value.message_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
}

/// Decodes an exact recalled-message query.
pub fn decode_read_recalled_message_request(
    value: composer_state_capnp::read_recalled_message_request::Reader<'_>,
) -> Result<ReadRecalledMessage, ComposerStateCodecError> {
    Ok(ReadRecalledMessage::new(
        parse_thread_id(
            read_text(
                value.get_thread_id(),
                "request.readRecalledMessage.threadId",
            )?,
            "request.readRecalledMessage.threadId",
        )?,
        parse_message_id(
            read_text(
                value.get_message_id(),
                "request.readRecalledMessage.messageId",
            )?,
            "request.readRecalledMessage.messageId",
        )?,
        parse_request_id(
            read_text(
                value.get_original_request_id(),
                "request.readRecalledMessage.originalRequestId",
            )?,
            "request.readRecalledMessage.originalRequestId",
        )?,
    ))
}

/// Encodes an exact run-usage query.
pub fn encode_read_run_usage_request(
    mut builder: composer_state_capnp::read_run_usage_request::Builder<'_>,
    value: &ReadRunUsage,
) {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_run_id(value.run_id.as_str());
}

/// Decodes an exact run-usage query.
pub fn decode_read_run_usage_request(
    value: composer_state_capnp::read_run_usage_request::Reader<'_>,
) -> Result<ReadRunUsage, ComposerStateCodecError> {
    Ok(ReadRunUsage::new(
        parse_thread_id(
            read_text(value.get_thread_id(), "request.readRunUsage.threadId")?,
            "request.readRunUsage.threadId",
        )?,
        parse_run_id(
            read_text(value.get_run_id(), "request.readRunUsage.runId")?,
            "request.readRunUsage.runId",
        )?,
    ))
}
