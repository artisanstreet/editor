//! Agent extensions and the global prompt: the reads and pushes, enabling
//! and adopting extensions, and saving or settling the global prompt.

#[allow(clippy::wildcard_imports)]
use super::*;

use artisan_domain::{
    AdoptExtension, EXTENSION_CATALOG_MAX, EXTENSION_ENGINES_MAX, Extension, ExtensionCatalog,
    ExtensionEngineState, ExtensionEngineStatus, ExtensionError, ExtensionKind,
    ExtensionLinkSupport, ExtensionOrigin, GLOBAL_PROMPT_CANDIDATES_MAX, GLOBAL_PROMPT_ENGINES_MAX,
    GlobalPrompt, GlobalPromptCandidate, GlobalPromptEngineState, GlobalPromptEngineStatus,
    GlobalPromptError, GlobalPromptResolution, GlobalPromptRevision, GlobalPromptSetup,
    ReadExtensions, ReadGlobalPrompt, ResolveGlobalPrompt, SaveGlobalPrompt, SetExtensionEnabled,
};

use crate::artisan_capnp::{
    extension_catalog, global_prompt, global_prompt_resolution, save_global_prompt_request,
    set_extension_enabled_request,
};

/// Encodes the extension catalog read.
pub(crate) fn encode_read_extensions(mut builder: request::Builder<'_>) {
    builder.set_read_extensions(());
}

/// Encodes the global prompt read.
pub(crate) fn encode_read_global_prompt(mut builder: request::Builder<'_>) {
    builder.set_read_global_prompt(());
}

/// Encodes an enable or disable request.
pub(crate) fn encode_set_extension_enabled(
    builder: request::Builder<'_>,
    command: &SetExtensionEnabled,
) {
    let mut encoded = builder.init_set_extension_enabled();
    encoded.set_extension_id(&command.extension_id);
    encoded.set_enabled(command.enabled);
}

/// Encodes an adopt request.
pub(crate) fn encode_adopt_extension(mut builder: request::Builder<'_>, command: &AdoptExtension) {
    builder.set_adopt_extension(&command.extension_id);
}

/// Encodes a global prompt save.
pub(crate) fn encode_save_global_prompt(builder: request::Builder<'_>, command: &SaveGlobalPrompt) {
    let mut encoded = builder.init_save_global_prompt();
    encoded.set_base_revision(command.base_revision.get());
    encoded.set_text(&command.text);
}

/// Encodes a global prompt resolution.
pub(crate) fn encode_resolve_global_prompt(
    builder: request::Builder<'_>,
    command: &ResolveGlobalPrompt,
) {
    let mut encoded = builder.init_resolve_global_prompt();
    match &command.resolution {
        GlobalPromptResolution::ChooseCandidate { candidate_id } => {
            encoded.set_choose_candidate(candidate_id);
        }
        GlobalPromptResolution::ImportEngine { engine_id } => encoded.set_import_engine(engine_id),
        GlobalPromptResolution::OverwriteEngine { engine_id } => {
            encoded.set_overwrite_engine(engine_id);
        }
    }
}

/// Decodes the extension catalog read.
pub(crate) const fn decode_read_extensions() -> ClientRequest {
    ClientRequest::Query(Query::ReadExtensions(ReadExtensions))
}

/// Decodes the global prompt read.
pub(crate) const fn decode_read_global_prompt() -> ClientRequest {
    ClientRequest::Query(Query::ReadGlobalPrompt(ReadGlobalPrompt))
}

/// Decodes an enable or disable request.
pub(crate) fn decode_set_extension_enabled(
    request_id: RequestId,
    value: set_extension_enabled_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let extension_id = read_text(
        value.get_extension_id(),
        "request.setExtensionEnabled.extensionId",
    )?;
    artisan_domain::validate_extension_id(&extension_id).map_err(extension_error)?;
    Ok(ClientRequest::Command(Command::SetExtensionEnabled(
        SetExtensionEnabled {
            request_id,
            extension_id,
            enabled: value.get_enabled(),
        },
    )))
}

/// Decodes an adopt request.
pub(crate) fn decode_adopt_extension(
    request_id: RequestId,
    value: capnp::Result<capnp::text::Reader<'_>>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let extension_id = read_text(value, "request.adoptExtension")?;
    artisan_domain::validate_extension_id(&extension_id).map_err(extension_error)?;
    Ok(ClientRequest::Command(Command::AdoptExtension(
        AdoptExtension {
            request_id,
            extension_id,
        },
    )))
}

/// Decodes a global prompt save.
pub(crate) fn decode_save_global_prompt(
    request_id: RequestId,
    value: save_global_prompt_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let text = read_text(value.get_text(), "request.saveGlobalPrompt.text")?;
    artisan_domain::validate_prompt_text(&text).map_err(prompt_error)?;
    Ok(ClientRequest::Command(Command::SaveGlobalPrompt(
        SaveGlobalPrompt {
            request_id,
            base_revision: GlobalPromptRevision::new(value.get_base_revision()),
            text,
        },
    )))
}

/// Decodes a global prompt resolution.
pub(crate) fn decode_resolve_global_prompt(
    request_id: RequestId,
    value: global_prompt_resolution::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let resolution = match value.which()? {
        global_prompt_resolution::Which::ChooseCandidate(candidate) => {
            GlobalPromptResolution::ChooseCandidate {
                candidate_id: read_text(candidate, "request.resolveGlobalPrompt.candidate")?,
            }
        }
        global_prompt_resolution::Which::ImportEngine(engine) => {
            GlobalPromptResolution::ImportEngine {
                engine_id: read_engine_id(engine, "request.resolveGlobalPrompt.importEngine")?,
            }
        }
        global_prompt_resolution::Which::OverwriteEngine(engine) => {
            GlobalPromptResolution::OverwriteEngine {
                engine_id: read_engine_id(engine, "request.resolveGlobalPrompt.overwriteEngine")?,
            }
        }
    };
    Ok(ClientRequest::Command(Command::ResolveGlobalPrompt(
        ResolveGlobalPrompt {
            request_id,
            resolution,
        },
    )))
}

fn read_engine_id(
    value: capnp::Result<capnp::text::Reader<'_>>,
    field: &'static str,
) -> Result<String, ProtocolDecodeError> {
    let engine_id = read_text(value, field)?;
    artisan_domain::engine_install::validate_engine_id(&engine_id)
        .map_err(|_| prompt_error(GlobalPromptError::EngineId))?;
    Ok(engine_id)
}

/// Encodes the extension catalog, shared by the answer and the push.
pub(crate) fn encode_extension_catalog(
    mut builder: extension_catalog::Builder<'_>,
    catalog: &ExtensionCatalog,
) -> Result<(), ProtocolEncodeError> {
    builder.set_links(match catalog.links() {
        ExtensionLinkSupport::Available => artisan_capnp::ExtensionLinkSupport::Available,
        ExtensionLinkSupport::DeveloperModeRequired => {
            artisan_capnp::ExtensionLinkSupport::DeveloperModeRequired
        }
    });
    let field = "extensions.extensions";
    let mut extensions = builder.init_extensions(list_length(field, catalog.extensions().len())?);
    for (index, extension) in catalog.extensions().iter().enumerate() {
        let mut encoded = extensions.reborrow().get(list_index(field, index)?);
        encoded.set_id(&extension.id);
        encoded.set_kind(match extension.kind {
            ExtensionKind::Skill => artisan_capnp::ExtensionKind::Skill,
        });
        encoded.set_origin(match extension.origin {
            ExtensionOrigin::FirstParty => artisan_capnp::ExtensionOrigin::FirstParty,
            ExtensionOrigin::Adopted => artisan_capnp::ExtensionOrigin::Adopted,
            ExtensionOrigin::Discovered => artisan_capnp::ExtensionOrigin::Discovered,
        });
        encoded.set_name(&extension.name);
        encoded.set_description(&extension.description);
        encoded.set_digest(&extension.digest);
        encoded.set_file_count(extension.file_count);
        encoded.set_byte_count(extension.byte_count);
        encoded.set_enabled(extension.enabled);
        let engines_field = "extensions.engines";
        let mut engines =
            encoded.init_engines(list_length(engines_field, extension.engines.len())?);
        for (position, state) in extension.engines.iter().enumerate() {
            let mut engine = engines.reborrow().get(list_index(engines_field, position)?);
            engine.set_engine_id(&state.engine_id);
            engine.set_status(encode_extension_status(state.status));
            engine.set_reason(state.reason.as_deref().unwrap_or_default());
        }
    }
    Ok(())
}

const fn encode_extension_status(
    status: ExtensionEngineStatus,
) -> artisan_capnp::ExtensionEngineStatus {
    use artisan_capnp::ExtensionEngineStatus as Wire;
    match status {
        ExtensionEngineStatus::Synced => Wire::Synced,
        ExtensionEngineStatus::Off => Wire::Off,
        ExtensionEngineStatus::EngineAbsent => Wire::EngineAbsent,
        ExtensionEngineStatus::Unavailable => Wire::Unavailable,
        ExtensionEngineStatus::Conflict => Wire::Conflict,
        ExtensionEngineStatus::Native => Wire::Native,
        ExtensionEngineStatus::NotPresent => Wire::NotPresent,
        ExtensionEngineStatus::LinksUnavailable => Wire::LinksUnavailable,
        ExtensionEngineStatus::Failed => Wire::Failed,
    }
}

const fn decode_extension_status(
    status: artisan_capnp::ExtensionEngineStatus,
) -> ExtensionEngineStatus {
    use artisan_capnp::ExtensionEngineStatus as Wire;
    match status {
        Wire::Synced => ExtensionEngineStatus::Synced,
        Wire::Off => ExtensionEngineStatus::Off,
        Wire::EngineAbsent => ExtensionEngineStatus::EngineAbsent,
        Wire::Unavailable => ExtensionEngineStatus::Unavailable,
        Wire::Conflict => ExtensionEngineStatus::Conflict,
        Wire::Native => ExtensionEngineStatus::Native,
        Wire::NotPresent => ExtensionEngineStatus::NotPresent,
        Wire::LinksUnavailable => ExtensionEngineStatus::LinksUnavailable,
        Wire::Failed => ExtensionEngineStatus::Failed,
    }
}

/// Decodes the extension catalog after checking its bounds.
pub(crate) fn decode_extension_catalog(
    value: extension_catalog::Reader<'_>,
) -> Result<ExtensionCatalog, ProtocolDecodeError> {
    let extensions = value.get_extensions()?;
    if extensions.len() as usize > EXTENSION_CATALOG_MAX {
        return Err(extension_error(ExtensionError::TooMany));
    }
    let optional = |value: capnp::Result<capnp::text::Reader<'_>>, field| {
        read_text(value, field).map(|text| (!text.is_empty()).then_some(text))
    };
    let decoded = extensions
        .iter()
        .map(|extension| {
            let engines = extension.get_engines()?;
            if engines.len() as usize > EXTENSION_ENGINES_MAX {
                return Err(extension_error(ExtensionError::TooMany));
            }
            Ok(Extension {
                id: read_text(extension.get_id(), "extensions.id")?,
                kind: match extension.get_kind()? {
                    artisan_capnp::ExtensionKind::Skill => ExtensionKind::Skill,
                },
                origin: match extension.get_origin()? {
                    artisan_capnp::ExtensionOrigin::FirstParty => ExtensionOrigin::FirstParty,
                    artisan_capnp::ExtensionOrigin::Adopted => ExtensionOrigin::Adopted,
                    artisan_capnp::ExtensionOrigin::Discovered => ExtensionOrigin::Discovered,
                },
                name: read_text(extension.get_name(), "extensions.name")?,
                description: read_text(extension.get_description(), "extensions.description")?,
                digest: read_text(extension.get_digest(), "extensions.digest")?,
                file_count: extension.get_file_count(),
                byte_count: extension.get_byte_count(),
                enabled: extension.get_enabled(),
                engines: engines
                    .iter()
                    .map(|engine| {
                        Ok(ExtensionEngineState {
                            engine_id: read_text(engine.get_engine_id(), "extensions.engineId")?,
                            status: decode_extension_status(engine.get_status()?),
                            reason: optional(engine.get_reason(), "extensions.reason")?,
                        })
                    })
                    .collect::<Result<Vec<_>, ProtocolDecodeError>>()?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;
    let links = match value.get_links()? {
        artisan_capnp::ExtensionLinkSupport::Available => ExtensionLinkSupport::Available,
        artisan_capnp::ExtensionLinkSupport::DeveloperModeRequired => {
            ExtensionLinkSupport::DeveloperModeRequired
        }
    };
    ExtensionCatalog::new(decoded, links).map_err(extension_error)
}

/// Encodes the global prompt, shared by the answer and the push.
pub(crate) fn encode_global_prompt(
    mut builder: global_prompt::Builder<'_>,
    prompt: &GlobalPrompt,
) -> Result<(), ProtocolEncodeError> {
    builder.set_revision(prompt.revision().get());
    builder.set_text(prompt.text());
    let candidates: &[GlobalPromptCandidate] = match prompt.setup() {
        GlobalPromptSetup::Ready => &[],
        GlobalPromptSetup::ChoiceRequired(candidates) => candidates,
    };
    let field = "globalPrompt.candidates";
    let mut encoded_candidates = builder
        .reborrow()
        .init_candidates(list_length(field, candidates.len())?);
    for (index, candidate) in candidates.iter().enumerate() {
        let mut encoded = encoded_candidates.reborrow().get(list_index(field, index)?);
        encoded.set_candidate_id(&candidate.candidate_id);
        encoded.set_text(&candidate.text);
        let engines_field = "globalPrompt.candidates.engineIds";
        let mut engine_ids =
            encoded.init_engine_ids(list_length(engines_field, candidate.engine_ids.len())?);
        for (position, engine_id) in candidate.engine_ids.iter().enumerate() {
            engine_ids.set(list_index(engines_field, position)?, engine_id);
        }
    }
    let field = "globalPrompt.engines";
    let mut engines = builder.init_engines(list_length(field, prompt.engines().len())?);
    for (index, state) in prompt.engines().iter().enumerate() {
        let mut encoded = engines.reborrow().get(list_index(field, index)?);
        encoded.set_engine_id(&state.engine_id);
        encoded.set_status(match state.status {
            GlobalPromptEngineStatus::Synced => artisan_capnp::GlobalPromptEngineStatus::Synced,
            GlobalPromptEngineStatus::Drifted => artisan_capnp::GlobalPromptEngineStatus::Drifted,
            GlobalPromptEngineStatus::AwaitingChoice => {
                artisan_capnp::GlobalPromptEngineStatus::AwaitingChoice
            }
            GlobalPromptEngineStatus::EngineAbsent => {
                artisan_capnp::GlobalPromptEngineStatus::EngineAbsent
            }
            GlobalPromptEngineStatus::Unavailable => {
                artisan_capnp::GlobalPromptEngineStatus::Unavailable
            }
            GlobalPromptEngineStatus::Failed => artisan_capnp::GlobalPromptEngineStatus::Failed,
        });
        encoded.set_file(state.file.as_deref().unwrap_or_default());
        encoded.set_has_external_text(state.external_text.is_some());
        encoded.set_external_text(state.external_text.as_deref().unwrap_or_default());
        encoded.set_reason(state.reason.as_deref().unwrap_or_default());
    }
    Ok(())
}

/// Decodes the global prompt after checking its bounds.
pub(crate) fn decode_global_prompt(
    value: global_prompt::Reader<'_>,
) -> Result<GlobalPrompt, ProtocolDecodeError> {
    let optional = |value: capnp::Result<capnp::text::Reader<'_>>, field| {
        read_text(value, field).map(|text| (!text.is_empty()).then_some(text))
    };
    let candidates = value.get_candidates()?;
    if candidates.len() as usize > GLOBAL_PROMPT_CANDIDATES_MAX {
        return Err(prompt_error(GlobalPromptError::Candidates));
    }
    let candidates = candidates
        .iter()
        .map(|candidate| {
            let engine_ids = candidate.get_engine_ids()?;
            if engine_ids.len() as usize > GLOBAL_PROMPT_ENGINES_MAX {
                return Err(prompt_error(GlobalPromptError::Candidates));
            }
            Ok(GlobalPromptCandidate {
                candidate_id: read_text(candidate.get_candidate_id(), "globalPrompt.candidateId")?,
                engine_ids: engine_ids
                    .iter()
                    .map(|engine_id| read_text(engine_id, "globalPrompt.candidateEngine"))
                    .collect::<Result<Vec<_>, ProtocolDecodeError>>()?,
                text: read_text(candidate.get_text(), "globalPrompt.candidateText")?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;
    let engines = value.get_engines()?;
    if engines.len() as usize > GLOBAL_PROMPT_ENGINES_MAX {
        return Err(prompt_error(GlobalPromptError::TooMany));
    }
    let engines = engines
        .iter()
        .map(|state| {
            Ok(GlobalPromptEngineState {
                engine_id: read_text(state.get_engine_id(), "globalPrompt.engineId")?,
                status: match state.get_status()? {
                    artisan_capnp::GlobalPromptEngineStatus::Synced => {
                        GlobalPromptEngineStatus::Synced
                    }
                    artisan_capnp::GlobalPromptEngineStatus::Drifted => {
                        GlobalPromptEngineStatus::Drifted
                    }
                    artisan_capnp::GlobalPromptEngineStatus::AwaitingChoice => {
                        GlobalPromptEngineStatus::AwaitingChoice
                    }
                    artisan_capnp::GlobalPromptEngineStatus::EngineAbsent => {
                        GlobalPromptEngineStatus::EngineAbsent
                    }
                    artisan_capnp::GlobalPromptEngineStatus::Unavailable => {
                        GlobalPromptEngineStatus::Unavailable
                    }
                    artisan_capnp::GlobalPromptEngineStatus::Failed => {
                        GlobalPromptEngineStatus::Failed
                    }
                },
                file: optional(state.get_file(), "globalPrompt.file")?,
                external_text: if state.get_has_external_text() {
                    Some(read_text(
                        state.get_external_text(),
                        "globalPrompt.externalText",
                    )?)
                } else {
                    None
                },
                reason: optional(state.get_reason(), "globalPrompt.reason")?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;
    let setup = if candidates.is_empty() {
        GlobalPromptSetup::Ready
    } else {
        GlobalPromptSetup::ChoiceRequired(candidates)
    };
    GlobalPrompt::new(
        GlobalPromptRevision::new(value.get_revision()),
        read_text(value.get_text(), "globalPrompt.text")?,
        setup,
        engines,
    )
    .map_err(prompt_error)
}

const fn extension_error(source: ExtensionError) -> ProtocolDecodeError {
    ProtocolDecodeError::Extension { source }
}

const fn prompt_error(source: GlobalPromptError) -> ProtocolDecodeError {
    ProtocolDecodeError::GlobalPrompt { source }
}
