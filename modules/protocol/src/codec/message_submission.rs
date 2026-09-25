//! Parent-union dispatch for Forge-owned submissions: sending a composer
//! draft, and retrying or recovering a failed message by identity. The
//! payload codecs live in the composer-state leaf; this module routes the
//! parent arms.

#[allow(clippy::wildcard_imports)]
use super::*;

use artisan_domain::{RecoverFailedMessage, RetryFailedMessage};

use crate::composer_state_codec as leaf;

/// Encodes one queue withdrawal, recalled-message read, or run-usage read
/// request arm; any other request is left unset.
pub(crate) fn encode_queue_state_request(mut builder: request::Builder<'_>, value: &ClientRequest) {
    match value {
        ClientRequest::Command(Command::WithdrawQueuedMessage(command)) => {
            leaf::encode_withdraw_queued_message_request(
                builder.reborrow().init_withdraw_queued_message(),
                command,
            );
        }
        ClientRequest::Query(Query::ReadRecalledMessage(query)) => {
            leaf::encode_read_recalled_message_request(
                builder.reborrow().init_read_recalled_message(),
                query,
            );
        }
        ClientRequest::Query(Query::ReadRunUsage(query)) => {
            leaf::encode_read_run_usage_request(builder.reborrow().init_read_run_usage(), query);
        }
        _ => {}
    }
}

/// Encodes one failed-message retry or recovery request arm.
pub(crate) fn encode_message_submission_request(
    mut builder: request::Builder<'_>,
    value: &ClientRequest,
) -> Result<(), ProtocolEncodeError> {
    match value {
        ClientRequest::Command(Command::RetryFailedMessage(command)) => {
            leaf::encode_failed_message_target(
                builder.reborrow().init_retry_failed_message(),
                &command.target,
            );
            Ok(())
        }
        ClientRequest::Command(Command::RecoverFailedMessage(command)) => {
            leaf::encode_failed_message_target(
                builder.reborrow().init_recover_failed_message(),
                &command.target,
            );
            Ok(())
        }
        ClientRequest::Command(Command::SubmitComposerDraft(command)) => {
            leaf::encode_submit_composer_draft_request(
                builder.reborrow().init_submit_composer_draft(),
                command,
            );
            Ok(())
        }
        _ => Err(ProtocolEncodeError::ComposerState),
    }
}

/// Decodes one failed-message retry or recovery request arm; the parent
/// envelope request id is the command identity.
pub(crate) fn decode_message_submission_request(
    value: request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(match value.which()? {
        request::Which::RetryFailedMessage(target) => {
            ClientRequest::Command(Command::RetryFailedMessage(RetryFailedMessage {
                request_id,
                target: leaf::decode_failed_message_target(target?)?,
            }))
        }
        request::Which::RecoverFailedMessage(target) => {
            ClientRequest::Command(Command::RecoverFailedMessage(RecoverFailedMessage {
                request_id,
                target: leaf::decode_failed_message_target(target?)?,
            }))
        }
        request::Which::SubmitComposerDraft(submit) => {
            ClientRequest::Command(Command::SubmitComposerDraft(
                leaf::decode_submit_composer_draft_request(submit?, request_id)?,
            ))
        }
        _ => {
            return Err(leaf::ComposerStateCodecError::StateValue {
                field: "request.messageSubmission",
            }
            .into());
        }
    })
}

/// Encodes one failed-message retry or recovery response arm.
pub(crate) fn encode_message_submission_response(
    mut builder: response::Builder<'_>,
    payload: &ResponsePayload,
    outer_request_id: &RequestId,
) -> Result<(), ProtocolEncodeError> {
    let encoded = match payload {
        ResponsePayload::FailedMessageRetried(value) => leaf::encode_failed_message_retried(
            builder.reborrow().init_failed_message_retried(),
            outer_request_id,
            value,
        ),
        ResponsePayload::FailedMessageRecovered(value) => leaf::encode_failed_message_recovered(
            builder.reborrow().init_failed_message_recovered(),
            outer_request_id,
            value,
        ),
        ResponsePayload::ComposerDraftSubmitted(value) => leaf::encode_composer_draft_submitted(
            builder.reborrow().init_composer_draft_submitted(),
            outer_request_id,
            value,
        ),
        _ => Err(leaf::ComposerStateCodecError::StateValue {
            field: "response.messageSubmission",
        }),
    };
    encoded.map_err(|_| ProtocolEncodeError::ComposerState)
}

/// Decodes one failed-message retry or recovery response arm.
pub(crate) fn decode_message_submission_response(
    value: response::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    Ok(match value.which()? {
        response::Which::FailedMessageRetried(value) => ResponsePayload::FailedMessageRetried(
            leaf::decode_failed_message_retried(value?, request_id)?,
        ),
        response::Which::FailedMessageRecovered(value) => ResponsePayload::FailedMessageRecovered(
            leaf::decode_failed_message_recovered(value?, request_id)?,
        ),
        response::Which::ComposerDraftSubmitted(value) => ResponsePayload::ComposerDraftSubmitted(
            leaf::decode_composer_draft_submitted(value?, request_id)?,
        ),
        _ => {
            return Err(leaf::ComposerStateCodecError::StateValue {
                field: "response.messageSubmission",
            }
            .into());
        }
    })
}
