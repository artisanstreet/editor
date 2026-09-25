//! Parent-union dispatch for Forge-owned submissions: retrying or
//! recovering a failed message by identity. The payload codecs live in the
//! composer-state leaf; this module routes the parent arms.

#[allow(clippy::wildcard_imports)]
use super::*;

use artisan_domain::{RecoverFailedMessage, RetryFailedMessage};

use crate::composer_state_codec as leaf;

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
        _ => {
            return Err(leaf::ComposerStateCodecError::StateValue {
                field: "response.messageSubmission",
            }
            .into());
        }
    })
}
