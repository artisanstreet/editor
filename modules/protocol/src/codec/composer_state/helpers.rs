//! Shared optional-field, identifier, enum, and correlation helpers.

#![forbid(unsafe_code)]

use super::*;
/// Validates the nested durable receipt identity against the parent response
/// correlation before either encoding or admitting the result.
pub fn validate_withdrawal_response_correlation(
    outer_request_id: &RequestId,
    value: &QueuedMessageWithdrawalResult,
) -> Result<(), ComposerStateCodecError> {
    if outer_request_id != &value.receipt.request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.messageWithdrawn.requestId",
        });
    }
    Ok(())
}

pub(super) fn encode_optional_text(
    mut builder: composer_state_capnp::optional_text::Builder<'_>,
    value: Option<&str>,
) {
    if let Some(value) = value {
        builder.set_present(true);
        builder.set_value(value);
    } else {
        builder.set_present(false);
    }
}

pub(super) fn decode_optional_text(
    value: composer_state_capnp::optional_text::Reader<'_>,
    field: &'static str,
) -> Result<Option<String>, ComposerStateCodecError> {
    let present = value.get_present();
    let text = read_text(value.get_value(), field)?;
    if !present && !text.is_empty() {
        return Err(ComposerStateCodecError::NonCanonicalOptional { field });
    }
    Ok(present.then_some(text))
}

pub(super) fn encode_optional_u64(
    mut builder: composer_state_capnp::optional_u_int64::Builder<'_>,
    value: Option<u64>,
) {
    if let Some(value) = value {
        builder.set_present(true);
        builder.set_value(value);
    } else {
        builder.set_present(false);
    }
}

pub(super) fn decode_optional_u64(
    value: composer_state_capnp::optional_u_int64::Reader<'_>,
    field: &'static str,
) -> Result<Option<u64>, ComposerStateCodecError> {
    let present = value.get_present();
    let number = value.get_value();
    if !present && number != 0 {
        return Err(ComposerStateCodecError::NonCanonicalOptional { field });
    }
    Ok(present.then_some(number))
}

pub(super) fn read_text(
    value: capnp::Result<capnp::text::Reader<'_>>,
    field: &'static str,
) -> Result<String, ComposerStateCodecError> {
    value?
        .to_str()
        .map(str::to_owned)
        .map_err(|source| ComposerStateCodecError::InvalidUtf8 { field, source })
}

pub(super) fn parse_request_id(
    value: String,
    field: &'static str,
) -> Result<RequestId, ComposerStateCodecError> {
    RequestId::parse(value).map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

pub(super) fn parse_thread_id(
    value: String,
    field: &'static str,
) -> Result<ThreadId, ComposerStateCodecError> {
    ThreadId::parse(value).map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

pub(super) fn parse_message_id(
    value: String,
    field: &'static str,
) -> Result<MessageId, ComposerStateCodecError> {
    MessageId::parse(value).map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

pub(super) fn parse_run_id(
    value: String,
    field: &'static str,
) -> Result<RunId, ComposerStateCodecError> {
    RunId::parse(value).map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

pub(super) fn parse_model_id(
    value: String,
    field: &'static str,
) -> Result<EngineModelId, ComposerStateCodecError> {
    EngineModelId::parse(value)
        .map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

pub(super) fn parse_route_id(
    value: String,
    field: &'static str,
) -> Result<EngineRouteId, ComposerStateCodecError> {
    EngineRouteId::parse(value)
        .map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

pub(super) fn parse_variant_id(
    value: String,
    field: &'static str,
) -> Result<EngineVariantId, ComposerStateCodecError> {
    EngineVariantId::parse(value)
        .map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

pub(super) fn list_length(
    field: &'static str,
    length: usize,
) -> Result<u32, ComposerStateCodecError> {
    u32::try_from(length).map_err(|_| ComposerStateCodecError::CollectionTooLarge { field, length })
}

pub(super) fn list_index(
    field: &'static str,
    index: usize,
) -> Result<u32, ComposerStateCodecError> {
    u32::try_from(index).map_err(|_| ComposerStateCodecError::CollectionTooLarge {
        field,
        length: index,
    })
}

pub(super) fn encode_list_order(
    value: QueuedMessageListOrder,
) -> composer_state_capnp::QueuedMessageListOrder {
    match value {
        QueuedMessageListOrder::OldestFirst => {
            composer_state_capnp::QueuedMessageListOrder::OldestFirst
        }
        QueuedMessageListOrder::LatestFirst => {
            composer_state_capnp::QueuedMessageListOrder::LatestFirst
        }
    }
}

pub(super) fn decode_list_order(
    value: Result<composer_state_capnp::QueuedMessageListOrder, capnp::NotInSchema>,
    field: &'static str,
) -> Result<QueuedMessageListOrder, ComposerStateCodecError> {
    match value.map_err(|source| ComposerStateCodecError::UnknownEnum {
        field,
        value: source.0,
    })? {
        composer_state_capnp::QueuedMessageListOrder::OldestFirst => {
            Ok(QueuedMessageListOrder::OldestFirst)
        }
        composer_state_capnp::QueuedMessageListOrder::LatestFirst => {
            Ok(QueuedMessageListOrder::LatestFirst)
        }
    }
}

pub(super) fn encode_disposition(
    value: ReceiptDisposition,
) -> composer_state_capnp::ReceiptDisposition {
    match value {
        ReceiptDisposition::Accepted => composer_state_capnp::ReceiptDisposition::Accepted,
        ReceiptDisposition::Duplicate => composer_state_capnp::ReceiptDisposition::Duplicate,
    }
}

pub(super) fn decode_disposition(
    value: Result<composer_state_capnp::ReceiptDisposition, capnp::NotInSchema>,
    field: &'static str,
) -> Result<ReceiptDisposition, ComposerStateCodecError> {
    match value.map_err(|source| ComposerStateCodecError::UnknownEnum {
        field,
        value: source.0,
    })? {
        composer_state_capnp::ReceiptDisposition::Accepted => Ok(ReceiptDisposition::Accepted),
        composer_state_capnp::ReceiptDisposition::Duplicate => Ok(ReceiptDisposition::Duplicate),
    }
}

pub(super) fn encode_withdrawal_outcome(
    value: artisan_domain::QueuedMessageWithdrawalOutcome,
) -> composer_state_capnp::QueuedMessageWithdrawalOutcome {
    match value {
        artisan_domain::QueuedMessageWithdrawalOutcome::Withdrawn => {
            composer_state_capnp::QueuedMessageWithdrawalOutcome::Withdrawn
        }
        artisan_domain::QueuedMessageWithdrawalOutcome::TooLate => {
            composer_state_capnp::QueuedMessageWithdrawalOutcome::TooLate
        }
        artisan_domain::QueuedMessageWithdrawalOutcome::NotQueued => {
            composer_state_capnp::QueuedMessageWithdrawalOutcome::NotQueued
        }
    }
}

pub(super) fn decode_withdrawal_outcome(
    value: Result<composer_state_capnp::QueuedMessageWithdrawalOutcome, capnp::NotInSchema>,
    field: &'static str,
) -> Result<artisan_domain::QueuedMessageWithdrawalOutcome, ComposerStateCodecError> {
    match value.map_err(|source| ComposerStateCodecError::UnknownEnum {
        field,
        value: source.0,
    })? {
        composer_state_capnp::QueuedMessageWithdrawalOutcome::Withdrawn => {
            Ok(artisan_domain::QueuedMessageWithdrawalOutcome::Withdrawn)
        }
        composer_state_capnp::QueuedMessageWithdrawalOutcome::TooLate => {
            Ok(artisan_domain::QueuedMessageWithdrawalOutcome::TooLate)
        }
        composer_state_capnp::QueuedMessageWithdrawalOutcome::NotQueued => {
            Ok(artisan_domain::QueuedMessageWithdrawalOutcome::NotQueued)
        }
    }
}
