//! Model selections named by catalog identities, and the Forge's typed
//! refusals.

#![forbid(unsafe_code)]

use artisan_domain::{
    CatalogOptionId, CatalogSelection, EngineProfileId, ModelFavoriteId, SubmissionRefusal,
    SubmissionRefusalKind,
};

use super::helpers::*;
use super::*;

/// Encodes one selection; absent options are empty text.
pub fn encode_catalog_selection(
    mut builder: composer_state_capnp::catalog_selection::Builder<'_>,
    value: &CatalogSelection,
) {
    builder.set_model_id(value.model_id.as_str());
    builder.set_profile_id(
        value
            .profile_id
            .as_ref()
            .map_or("", EngineProfileId::as_str),
    );
    let option = |option: &Option<CatalogOptionId>| {
        option
            .as_ref()
            .map_or("", CatalogOptionId::as_str)
            .to_owned()
    };
    builder.set_reasoning_effort(option(&value.reasoning_effort).as_str());
    builder.set_speed(option(&value.speed).as_str());
    builder.set_context_window(option(&value.context_window).as_str());
    builder.set_permission(option(&value.permission).as_str());
}

/// Decodes one selection through the owned identity constructors.
pub fn decode_catalog_selection(
    value: composer_state_capnp::catalog_selection::Reader<'_>,
    field: &'static str,
) -> Result<CatalogSelection, ComposerStateCodecError> {
    let invalid = || ComposerStateCodecError::StateValue { field };
    let text = |value: capnp::Result<capnp::text::Reader<'_>>| read_text(value, field);
    let option = |value: String| {
        if value.is_empty() {
            Ok(None)
        } else {
            CatalogOptionId::parse(value)
                .map(Some)
                .map_err(|_| invalid())
        }
    };
    let profile = text(value.get_profile_id())?;
    Ok(CatalogSelection {
        model_id: ModelFavoriteId::parse(text(value.get_model_id())?).map_err(|_| invalid())?,
        profile_id: if profile.is_empty() {
            None
        } else {
            Some(EngineProfileId::parse(profile).map_err(|_| invalid())?)
        },
        reasoning_effort: option(text(value.get_reasoning_effort())?)?,
        speed: option(text(value.get_speed())?)?,
        context_window: option(text(value.get_context_window())?)?,
        permission: option(text(value.get_permission())?)?,
    })
}

/// Encodes one refusal.
pub fn encode_submission_refusal(
    mut builder: composer_state_capnp::submission_refusal::Builder<'_>,
    value: &SubmissionRefusal,
) {
    builder.set_kind(match value.kind() {
        SubmissionRefusalKind::InvalidSelection => {
            composer_state_capnp::SubmissionRefusalKind::InvalidSelection
        }
        SubmissionRefusalKind::NoSelection => {
            composer_state_capnp::SubmissionRefusalKind::NoSelection
        }
        SubmissionRefusalKind::EngineNotReady => {
            composer_state_capnp::SubmissionRefusalKind::EngineNotReady
        }
        SubmissionRefusalKind::RunStarting => {
            composer_state_capnp::SubmissionRefusalKind::RunStarting
        }
        SubmissionRefusalKind::AttachmentRejected => {
            composer_state_capnp::SubmissionRefusalKind::AttachmentRejected
        }
    });
    builder.set_message(value.message());
}

/// Decodes one refusal through the owned constructor.
pub fn decode_submission_refusal(
    value: composer_state_capnp::submission_refusal::Reader<'_>,
    field: &'static str,
) -> Result<SubmissionRefusal, ComposerStateCodecError> {
    let kind = match value
        .get_kind()
        .map_err(|_| ComposerStateCodecError::StateValue { field })?
    {
        composer_state_capnp::SubmissionRefusalKind::InvalidSelection => {
            SubmissionRefusalKind::InvalidSelection
        }
        composer_state_capnp::SubmissionRefusalKind::NoSelection => {
            SubmissionRefusalKind::NoSelection
        }
        composer_state_capnp::SubmissionRefusalKind::EngineNotReady => {
            SubmissionRefusalKind::EngineNotReady
        }
        composer_state_capnp::SubmissionRefusalKind::RunStarting => {
            SubmissionRefusalKind::RunStarting
        }
        composer_state_capnp::SubmissionRefusalKind::AttachmentRejected => {
            SubmissionRefusalKind::AttachmentRejected
        }
    };
    SubmissionRefusal::new(kind, read_text(value.get_message(), field)?)
        .map_err(|_| ComposerStateCodecError::StateValue { field })
}
