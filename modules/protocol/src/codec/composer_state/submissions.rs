//! Forge-owned submissions: the pushed message outbox and the retry and
//! recovery commands that name a failed message by identity.

#![forbid(unsafe_code)]

use artisan_domain::{
    FailedMessageRecovered, FailedMessageRetried, FailedMessageRetryOutcome, FailedMessageTarget,
    MessageOutbox,
};

use super::helpers::*;
use super::*;

/// Encodes one thread's message outbox.
pub fn encode_message_outbox(
    mut builder: composer_state_capnp::message_outbox::Builder<'_>,
    value: &MessageOutbox,
) -> Result<(), ComposerStateCodecError> {
    encode_queued_message_listing(builder.reborrow().init_queued(), value.queued())?;
    encode_failed_message_listing(builder.reborrow().init_failed(), value.failed())
}

/// Decodes one thread's message outbox; both listings must name one thread.
pub fn decode_message_outbox(
    value: composer_state_capnp::message_outbox::Reader<'_>,
) -> Result<MessageOutbox, ComposerStateCodecError> {
    MessageOutbox::new(
        decode_queued_message_listing(value.get_queued()?)?,
        decode_failed_message_listing(value.get_failed()?)?,
    )
    .map_err(|_| ComposerStateCodecError::ScopeMismatch {
        field: "event.messageOutbox",
    })
}

/// Encodes the failed message a retry or recovery names.
pub fn encode_failed_message_target(
    mut builder: composer_state_capnp::failed_message_target::Builder<'_>,
    value: &FailedMessageTarget,
) {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_message_id(value.message_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
}

/// Decodes the failed message a retry or recovery names.
pub fn decode_failed_message_target(
    value: composer_state_capnp::failed_message_target::Reader<'_>,
) -> Result<FailedMessageTarget, ComposerStateCodecError> {
    Ok(FailedMessageTarget {
        thread_id: parse_thread_id(
            read_text(value.get_thread_id(), "failedMessage.threadId")?,
            "failedMessage.threadId",
        )?,
        message_id: parse_message_id(
            read_text(value.get_message_id(), "failedMessage.messageId")?,
            "failedMessage.messageId",
        )?,
        original_request_id: parse_request_id(
            read_text(
                value.get_original_request_id(),
                "failedMessage.originalRequestId",
            )?,
            "failedMessage.originalRequestId",
        )?,
    })
}

/// Encodes a retry answer after checking its correlation.
pub fn encode_failed_message_retried(
    mut builder: composer_state_capnp::failed_message_retried::Builder<'_>,
    outer_request_id: &RequestId,
    value: &FailedMessageRetried,
) -> Result<(), ComposerStateCodecError> {
    if outer_request_id != &value.request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.failedMessageRetried.requestId",
        });
    }
    builder.set_request_id(value.request_id.as_str());
    encode_failed_message_target(builder.reborrow().init_target(), &value.target);
    builder.set_outcome(match value.outcome {
        FailedMessageRetryOutcome::Requeued => {
            composer_state_capnp::FailedMessageRetryOutcome::Requeued
        }
        FailedMessageRetryOutcome::NotRetryable => {
            composer_state_capnp::FailedMessageRetryOutcome::NotRetryable
        }
    });
    Ok(())
}

/// Decodes a retry answer and enforces its correlation.
pub fn decode_failed_message_retried(
    value: composer_state_capnp::failed_message_retried::Reader<'_>,
    outer_request_id: &RequestId,
) -> Result<FailedMessageRetried, ComposerStateCodecError> {
    let request_id = decode_correlated_request_id(
        value.get_request_id(),
        outer_request_id,
        "response.failedMessageRetried.requestId",
    )?;
    let outcome =
        match value
            .get_outcome()
            .map_err(|source| ComposerStateCodecError::UnknownEnum {
                field: "response.failedMessageRetried.outcome",
                value: source.0,
            })? {
            composer_state_capnp::FailedMessageRetryOutcome::Requeued => {
                FailedMessageRetryOutcome::Requeued
            }
            composer_state_capnp::FailedMessageRetryOutcome::NotRetryable => {
                FailedMessageRetryOutcome::NotRetryable
            }
        };
    Ok(FailedMessageRetried {
        request_id,
        target: decode_failed_message_target(value.get_target()?)?,
        outcome,
    })
}

/// Encodes a recovery answer after checking its correlation.
pub fn encode_failed_message_recovered(
    mut builder: composer_state_capnp::failed_message_recovered::Builder<'_>,
    outer_request_id: &RequestId,
    value: &FailedMessageRecovered,
) -> Result<(), ComposerStateCodecError> {
    if outer_request_id != &value.request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.failedMessageRecovered.requestId",
        });
    }
    builder.set_request_id(value.request_id.as_str());
    encode_failed_message_target(builder.reborrow().init_target(), &value.target);
    builder.set_new_thread_id(value.new_thread_id.as_ref().map_or("", ThreadId::as_str));
    builder.set_disposition(encode_disposition(value.disposition));
    Ok(())
}

/// Decodes a recovery answer and enforces its correlation.
pub fn decode_failed_message_recovered(
    value: composer_state_capnp::failed_message_recovered::Reader<'_>,
    outer_request_id: &RequestId,
) -> Result<FailedMessageRecovered, ComposerStateCodecError> {
    let request_id = decode_correlated_request_id(
        value.get_request_id(),
        outer_request_id,
        "response.failedMessageRecovered.requestId",
    )?;
    let new_thread_id = read_text(
        value.get_new_thread_id(),
        "response.failedMessageRecovered.newThreadId",
    )?;
    let new_thread_id = if new_thread_id.is_empty() {
        None
    } else {
        Some(parse_thread_id(
            new_thread_id,
            "response.failedMessageRecovered.newThreadId",
        )?)
    };
    Ok(FailedMessageRecovered {
        request_id,
        target: decode_failed_message_target(value.get_target()?)?,
        new_thread_id,
        disposition: decode_disposition(
            value.get_disposition(),
            "response.failedMessageRecovered.disposition",
        )?,
    })
}

fn decode_correlated_request_id(
    value: capnp::Result<capnp::text::Reader<'_>>,
    outer_request_id: &RequestId,
    field: &'static str,
) -> Result<RequestId, ComposerStateCodecError> {
    let request_id = parse_request_id(read_text(value, field)?, field)?;
    if &request_id != outer_request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch { field });
    }
    Ok(request_id)
}
