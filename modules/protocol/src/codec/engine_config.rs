//! Engine configuration, model-selection, permission, and runtime codec.
//!
//! Owns wire conversions for thread engine configuration, registered engine
//! profiles, engine run configs, per-engine selections, permissions, and runtime
//! controls. Lifetimes and validation stay in the domain; this module maps them
//! to and from `artisan_capnp`.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn encode_set_thread_engine_config(
    mut builder: artisan_capnp::request::Builder<'_>,
    command: &SetThreadEngineConfig,
) {
    let mut encoded = builder.reborrow().init_set_thread_engine_config();
    encoded.set_thread_id(command.thread_id().as_str());
    encode_engine_config_precondition(
        encoded.reborrow().init_precondition(),
        command.precondition(),
    );
    encode_engine_run_config(encoded.init_config(), command.config());
}

pub(crate) fn encode_thread_engine_config_result(
    mut builder: artisan_capnp::response::Builder<'_>,
    result: &SetThreadEngineConfigResult,
) {
    let mut encoded = builder.reborrow().init_thread_engine_config_set();
    encoded.set_request_id(result.request_id.as_str());
    encoded.set_thread_id(result.thread_id.as_str());
    encoded.set_revision(result.revision.get());
    encoded.set_disposition(encode_disposition(result.disposition));
}

pub(crate) fn encode_thread_engine_settings_result(
    mut builder: artisan_capnp::response::Builder<'_>,
    value: &crate::types::ThreadEngineSettingsResult,
) {
    let mut encoded = builder.reborrow().init_thread_engine_settings();
    encoded.set_thread_id(value.thread_id().as_str());
    match value {
        crate::types::ThreadEngineSettingsResult::Unconfigured { .. } => {
            encoded.init_state().set_unconfigured(());
        }
        crate::types::ThreadEngineSettingsResult::Configured {
            revision, config, ..
        } => {
            let mut configured = encoded.init_state().init_configured();
            configured.set_revision(revision.get());
            encode_engine_run_config(configured.init_config(), config);
        }
    }
}

pub(crate) fn encode_registered_engine_profiles_result(
    builder: artisan_capnp::registered_engine_profiles_result::Builder<'_>,
    value: &RegisteredEngineProfilesResult,
) -> Result<(), ProtocolEncodeError> {
    match value {
        RegisteredEngineProfilesResult::RegistryMissing => {
            builder.init_state().set_registry_missing(());
        }
        RegisteredEngineProfilesResult::RegistryPresent { profile_ids } => {
            if profile_ids.len() > 64 {
                return Err(ProtocolEncodeError::CollectionTooLarge {
                    field: "response.registeredEngineProfiles.profileIds",
                    length: profile_ids.len(),
                });
            }
            let mut seen = std::collections::HashSet::with_capacity(profile_ids.len());
            for id in profile_ids {
                if !seen.insert(id.as_str()) {
                    return Err(ProtocolEncodeError::Duplicate {
                        field: "response.registeredEngineProfiles.profileIds",
                        value: id.as_str().to_owned(),
                    });
                }
            }
            let mut list = builder
                .init_state()
                .init_registry_present()
                .init_profile_ids(list_length(
                    "response.registeredEngineProfiles.profileIds",
                    profile_ids.len(),
                )?);
            for (index, id) in profile_ids.iter().enumerate() {
                list.set(
                    list_index("response.registeredEngineProfiles.profileIds", index)?,
                    id.as_str(),
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn encode_engine_config_precondition(
    mut builder: engine_config_precondition::Builder<'_>,
    value: EngineConfigUpdatePrecondition,
) {
    match value {
        EngineConfigUpdatePrecondition::Unconfigured => {
            builder.set_kind("unconfigured");
            builder.set_revision(0);
        }
        EngineConfigUpdatePrecondition::Exact(revision) => {
            builder.set_kind("exact_revision");
            builder.set_revision(revision.get());
        }
    }
}

pub(crate) fn encode_engine_variant(
    mut builder: artisan_capnp::engine_variant::Builder<'_>,
    variant: Option<&EngineVariantId>,
) {
    if let Some(id) = variant {
        builder.set_kind("selected");
        builder.set_id(id.as_str());
    } else {
        builder.set_kind("none");
        builder.set_id("");
    }
}

pub(crate) fn encode_engine_permission(
    mut builder: artisan_capnp::engine_permission_policy::Builder<'_>,
    permission: &EnginePermissionPolicy,
) {
    builder.set_permission_id(permission.permission_id().as_str());
    builder.set_agent_id(permission.agent_id().as_str());
    builder.set_approval(permission.approval().as_str());
    builder.set_filesystem(permission.filesystem().as_str());
    builder.set_network(permission.network().as_str());
    builder.set_web_search(permission.web_search().as_str());
}

pub(crate) fn encode_engine_runtime(
    mut builder: artisan_capnp::engine_runtime_controls::Builder<'_>,
    runtime: EngineRuntimeControls,
) {
    builder.set_attempt_budget_ms(runtime.attempt_budget().get());
    builder.set_readiness_budget_ms(runtime.readiness_budget().get());
    builder.set_health_budget_ms(runtime.health_budget().get());
    builder.set_prompt_budget_ms(runtime.prompt_budget().get());
    builder.set_stream_budget_ms(runtime.stream_budget().get());
    builder.set_close_budget_ms(runtime.close_budget().get());
    builder.set_max_json_body_bytes(runtime.max_json_body_bytes().get());
    builder.set_max_sse_line_bytes(runtime.max_sse_line_bytes().get());
    builder.set_max_sse_event_bytes(runtime.max_sse_event_bytes().get());
    builder.set_max_readiness_line_bytes(runtime.max_readiness_line_bytes().get());
    builder.set_max_header_count(runtime.max_header_count().get());
    builder.set_max_http_buffer_bytes(runtime.max_http_buffer_bytes().get());
    builder.set_max_stderr_bytes(runtime.max_stderr_bytes().get());
    builder.set_observation_capacity(runtime.observation_capacity().get());
}

#[expect(
    clippy::too_many_lines,
    reason = "single engine-selection dispatcher; each arm writes one engine's selection wire"
)]
pub(crate) fn encode_engine_run_config(
    mut builder: engine_run_config::Builder<'_>,
    value: &EngineRunConfig,
) {
    match value.selection() {
        EngineSelection::OpenCode2(selection) => {
            builder.set_schema_version(1);
            builder.set_engine(EngineId::OpenCode2.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().as_str());
            builder.set_route_id(selection.route_id().as_str());
            encode_engine_variant(builder.reborrow().init_variant(), selection.variant_id());
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            builder.init_selection_v2().set_unset(());
        }
        EngineSelection::Codex(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Codex.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            builder.set_route_id("");
            encode_engine_variant(builder.reborrow().init_variant(), None);
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_codex();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            arm.set_reasoning_effort(
                selection
                    .reasoning_effort()
                    .map_or("", CodexReasoningEffort::as_str),
            );
            arm.set_service_tier(
                selection
                    .service_tier()
                    .map_or("", CodexServiceTier::as_str),
            );
            arm.set_model_context_window(
                selection
                    .model_context_window()
                    .map_or(0, CodexModelContextWindow::get),
            );
            encode_engine_permission(arm.reborrow().init_permission(), selection.permission());
        }
        EngineSelection::Claude(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Claude.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            builder.set_route_id("");
            encode_engine_variant(builder.reborrow().init_variant(), None);
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_claude();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            arm.set_effort(selection.effort().map_or("", ClaudeEffort::as_str));
            arm.set_permission_mode(
                selection
                    .permission_mode()
                    .map_or("", ClaudePermissionMode::as_str),
            );
            arm.set_disable_tools(selection.disable_tools());
            arm.set_safe_mode(selection.safe_mode());
            encode_engine_permission(arm.reborrow().init_permission(), selection.permission());
        }
        EngineSelection::Grok(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Grok.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            builder.set_route_id("");
            encode_engine_variant(builder.reborrow().init_variant(), None);
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_grok();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            arm.set_reasoning_effort(
                selection
                    .reasoning_effort()
                    .map_or("", GrokReasoningEffort::as_str),
            );
            arm.set_permission_mode(
                selection
                    .permission_mode()
                    .map_or("", GrokPermissionMode::as_str),
            );
            encode_engine_permission(arm.reborrow().init_permission(), selection.permission());
        }
        EngineSelection::Cursor(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Cursor.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            builder.set_route_id("");
            encode_engine_variant(builder.reborrow().init_variant(), None);
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_cursor();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            arm.set_reasoning_effort(
                selection
                    .reasoning_effort()
                    .map_or("", CursorReasoningEffort::as_str),
            );
            arm.set_speed(selection.speed().map_or("", CursorSpeed::as_str));
            arm.set_permission_mode(
                selection
                    .permission_mode()
                    .map_or("", CursorPermissionMode::as_str),
            );
            encode_engine_permission(arm.reborrow().init_permission(), selection.permission());
        }
        EngineSelection::Hermes(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Hermes.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().as_str());
            builder.set_route_id(selection.route_id().as_str());
            encode_engine_variant(builder.reborrow().init_variant(), None);
            // Hermes authorization is profile-owned; the legacy mirror
            // carries a restrictive sentinel that old readers reject along
            // with the unknown engine instead of misreading it.
            let mut permission = builder.reborrow().init_permission();
            permission.set_permission_id("hermes-managed");
            permission.set_agent_id("hermes-managed-agent");
            permission.set_approval(ApprovalMode::Never.as_str());
            permission.set_filesystem(FilesystemAccess::None.as_str());
            permission.set_network(NetworkAccess::Disabled.as_str());
            permission.set_web_search(WebSearchAccess::Disabled.as_str());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_hermes();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().as_str());
            arm.set_route_id(selection.route_id().as_str());
            arm.set_reasoning_effort(
                selection
                    .reasoning_effort()
                    .map_or("", HermesReasoningEffort::as_str),
            );
            arm.set_permission_mode(selection.permission_mode().as_str());
            arm.set_fast(selection.fast());
        }
    }
}

pub(crate) fn decode_set_thread_engine_config(
    command: set_thread_engine_config_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(
            command.get_thread_id(),
            "request.setThreadEngineConfig.threadId",
        )?,
        "request.setThreadEngineConfig.threadId",
    )?;
    let precondition = decode_engine_config_precondition(command.get_precondition()?)?;
    let config = decode_engine_run_config(command.get_config()?)?;
    Ok(ClientRequest::Command(Command::SetThreadEngineConfig(
        Box::new(SetThreadEngineConfig::new(
            request_id,
            thread_id,
            precondition,
            config,
        )),
    )))
}

pub(crate) fn decode_read_thread_engine_settings(
    query: artisan_capnp::read_thread_engine_settings_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(
            query.get_thread_id(),
            "request.readThreadEngineSettings.threadId",
        )?,
        "request.readThreadEngineSettings.threadId",
    )?;
    Ok(ClientRequest::Query(
        artisan_domain::Query::ReadThreadEngineSettings(
            artisan_domain::commands::ReadThreadEngineSettings::new(thread_id),
        ),
    ))
}

pub(crate) fn engine_config_error(
    field: &'static str,
    reason: EngineConfigReason,
) -> ProtocolDecodeError {
    ProtocolDecodeError::EngineConfig {
        source: EngineConfigError::new(field, reason),
    }
}

pub(crate) fn decode_engine_config_precondition(
    value: artisan_capnp::engine_config_precondition::Reader<'_>,
) -> Result<EngineConfigUpdatePrecondition, ProtocolDecodeError> {
    let kind = read_text(
        value.get_kind(),
        "request.setThreadEngineConfig.precondition.kind",
    )?;
    match kind.as_str() {
        "unconfigured" if value.get_revision() == 0 => {
            Ok(EngineConfigUpdatePrecondition::Unconfigured)
        }
        "exact_revision" => Ok(EngineConfigUpdatePrecondition::Exact(
            EngineConfigRevision::new(value.get_revision())
                .map_err(|error| ProtocolDecodeError::EngineConfig { source: error })?,
        )),
        "unconfigured" => Err(engine_config_error(
            "request.setThreadEngineConfig.precondition.revision",
            EngineConfigReason::Inconsistent,
        )),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.precondition.kind",
            EngineConfigReason::Unsupported,
        )),
    }
}

pub(crate) fn decode_engine_run_config(
    value: artisan_capnp::engine_run_config::Reader<'_>,
) -> Result<EngineRunConfig, ProtocolDecodeError> {
    match value.get_schema_version() {
        1 => decode_engine_run_config_v1(value),
        2 => decode_engine_run_config_v2(value),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.schemaVersion",
            EngineConfigReason::Unsupported,
        )),
    }
}

pub(crate) fn decode_engine_run_config_v1(
    value: artisan_capnp::engine_run_config::Reader<'_>,
) -> Result<EngineRunConfig, ProtocolDecodeError> {
    if !matches!(
        value.get_selection_v2()?.which()?,
        engine_selection_v2::Which::Unset(())
    ) {
        return Err(engine_config_error(
            "request.setThreadEngineConfig.config.selectionV2",
            EngineConfigReason::Inconsistent,
        ));
    }
    let engine = read_text(
        value.get_engine(),
        "request.setThreadEngineConfig.config.engine",
    )?;
    if engine != EngineId::OpenCode2.as_str() {
        return Err(engine_config_error(
            "request.setThreadEngineConfig.config.engine",
            EngineConfigReason::Unsupported,
        ));
    }
    let profile_id = EngineProfileId::parse(read_text(
        value.get_profile_id(),
        "request.setThreadEngineConfig.config.profileId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.profileId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let model_id = EngineModelId::parse(read_text(
        value.get_model_id(),
        "request.setThreadEngineConfig.config.modelId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.modelId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let route_id = EngineRouteId::parse(read_text(
        value.get_route_id(),
        "request.setThreadEngineConfig.config.routeId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.routeId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let variant = decode_engine_variant(value.get_variant()?)?;
    let permission = decode_engine_permission(value.get_permission()?)?;
    let runtime = decode_engine_runtime(value.get_runtime()?)?;
    Ok(EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            profile_id, model_id, route_id, variant, permission,
        )),
        runtime,
    ))
}

pub(crate) fn decode_engine_run_config_v2(
    value: artisan_capnp::engine_run_config::Reader<'_>,
) -> Result<EngineRunConfig, ProtocolDecodeError> {
    let engine = read_text(
        value.get_engine(),
        "request.setThreadEngineConfig.config.engine",
    )?;
    let engine = EngineId::parse(&engine).map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.engine",
            EngineConfigReason::Unsupported,
        )
    })?;
    // The legacy profile field stays populated as a diagnostic mirror; it
    // must agree with the authority arm below.
    let legacy_profile_id = read_text(
        value.get_profile_id(),
        "request.setThreadEngineConfig.config.profileId",
    )?;
    let runtime = decode_engine_runtime(value.get_runtime()?)?;
    let selection = match value.get_selection_v2()?.which()? {
        engine_selection_v2::Which::Codex(arm) => {
            if engine != EngineId::Codex {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Codex(decode_codex_selection(arm?)?)
        }
        engine_selection_v2::Which::Claude(arm) => {
            if engine != EngineId::Claude {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Claude(decode_claude_selection(arm?)?)
        }
        engine_selection_v2::Which::Grok(arm) => {
            if engine != EngineId::Grok {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Grok(decode_grok_selection(arm?)?)
        }
        engine_selection_v2::Which::Cursor(arm) => {
            if engine != EngineId::Cursor {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Cursor(decode_cursor_selection(arm?)?)
        }
        engine_selection_v2::Which::Hermes(arm) => {
            if engine != EngineId::Hermes {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Hermes(decode_hermes_selection(arm?)?)
        }
        engine_selection_v2::Which::Unset(()) => {
            return Err(engine_config_error(
                "request.setThreadEngineConfig.config.selectionV2",
                EngineConfigReason::Inconsistent,
            ));
        }
    };
    if selection.profile_id().as_str() != legacy_profile_id {
        return Err(engine_config_error(
            "request.setThreadEngineConfig.config.profileId",
            EngineConfigReason::Inconsistent,
        ));
    }
    Ok(EngineRunConfig::new(selection, runtime))
}

pub(crate) fn parse_optional_model_id(
    value: String,
    field: &'static str,
) -> Result<Option<EngineModelId>, ProtocolDecodeError> {
    if value.is_empty() {
        Ok(None)
    } else {
        EngineModelId::parse(value)
            .map(Some)
            .map_err(|_| engine_config_error(field, EngineConfigReason::InvalidIdentifier))
    }
}

pub(crate) fn parse_optional_setting<T>(
    value: &str,
    field: &'static str,
    parse: impl FnOnce(&str) -> Result<T, EngineConfigError>,
) -> Result<Option<T>, ProtocolDecodeError> {
    if value.is_empty() {
        Ok(None)
    } else {
        parse(value)
            .map(Some)
            .map_err(|error| engine_config_error(field, error.reason()))
    }
}

pub(crate) fn parse_required_model_id(
    value: String,
    field: &'static str,
) -> Result<EngineModelId, ProtocolDecodeError> {
    if value.is_empty() {
        return Err(engine_config_error(
            field,
            EngineConfigReason::InvalidIdentifier,
        ));
    }
    EngineModelId::parse(value)
        .map_err(|_| engine_config_error(field, EngineConfigReason::InvalidIdentifier))
}

pub(crate) fn parse_required_route_id(
    value: String,
    field: &'static str,
) -> Result<EngineRouteId, ProtocolDecodeError> {
    if value.is_empty() {
        return Err(engine_config_error(
            field,
            EngineConfigReason::InvalidIdentifier,
        ));
    }
    EngineRouteId::parse(value)
        .map_err(|_| engine_config_error(field, EngineConfigReason::InvalidIdentifier))
}

pub(crate) fn decode_codex_selection(
    arm: artisan_capnp::codex_engine_selection::Reader<'_>,
) -> Result<CodexSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_optional_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let reasoning_effort = parse_optional_setting(
        &read_text(
            arm.get_reasoning_effort(),
            "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        CodexReasoningEffort::parse,
    )?;
    let service_tier = parse_optional_setting(
        &read_text(
            arm.get_service_tier(),
            "request.setThreadEngineConfig.config.selectionV2.serviceTier",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.serviceTier",
        CodexServiceTier::parse,
    )?;
    let model_context_window = {
        let window = arm.get_model_context_window();
        if window == 0 {
            None
        } else {
            Some(CodexModelContextWindow::new(window).map_err(|error| {
                engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2.modelContextWindow",
                    error.reason(),
                )
            })?)
        }
    };
    let permission = decode_engine_permission(arm.get_permission()?)?;
    Ok(CodexSelection::new(
        profile_id,
        model_id,
        permission,
        reasoning_effort,
        service_tier,
        model_context_window,
    )?)
}

pub(crate) fn decode_claude_selection(
    arm: artisan_capnp::claude_engine_selection::Reader<'_>,
) -> Result<ClaudeSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_optional_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let effort = parse_optional_setting(
        &read_text(
            arm.get_effort(),
            "request.setThreadEngineConfig.config.selectionV2.effort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.effort",
        ClaudeEffort::parse,
    )?;
    let permission_mode = parse_optional_setting(
        &read_text(
            arm.get_permission_mode(),
            "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        ClaudePermissionMode::parse,
    )?;
    let permission = decode_engine_permission(arm.get_permission()?)?;
    Ok(ClaudeSelection::new(
        profile_id,
        model_id,
        permission,
        effort,
        permission_mode,
        arm.get_disable_tools(),
        arm.get_safe_mode(),
    )?)
}

pub(crate) fn decode_grok_selection(
    arm: artisan_capnp::grok_engine_selection::Reader<'_>,
) -> Result<GrokSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_optional_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let reasoning_effort = parse_optional_setting(
        &read_text(
            arm.get_reasoning_effort(),
            "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        |text| GrokReasoningEffort::parse(text),
    )?;
    let permission_mode = parse_optional_setting(
        &read_text(
            arm.get_permission_mode(),
            "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        GrokPermissionMode::parse,
    )?;
    let permission = decode_engine_permission(arm.get_permission()?)?;
    Ok(GrokSelection::new(
        profile_id,
        model_id,
        permission,
        reasoning_effort,
        permission_mode,
    ))
}

pub(crate) fn decode_cursor_selection(
    arm: artisan_capnp::cursor_engine_selection::Reader<'_>,
) -> Result<CursorSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_optional_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let reasoning_effort = parse_optional_setting(
        &read_text(
            arm.get_reasoning_effort(),
            "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        |text| CursorReasoningEffort::parse(text),
    )?;
    let speed = parse_optional_setting(
        &read_text(
            arm.get_speed(),
            "request.setThreadEngineConfig.config.selectionV2.speed",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.speed",
        CursorSpeed::parse,
    )?;
    let permission_mode = parse_optional_setting(
        &read_text(
            arm.get_permission_mode(),
            "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        CursorPermissionMode::parse,
    )?;
    let permission = decode_engine_permission(arm.get_permission()?)?;
    Ok(CursorSelection::new(
        profile_id,
        model_id,
        permission,
        reasoning_effort,
        speed,
        permission_mode,
    ))
}

pub(crate) fn decode_hermes_selection(
    arm: artisan_capnp::hermes_engine_selection::Reader<'_>,
) -> Result<HermesSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_required_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let route_id = parse_required_route_id(
        read_text(
            arm.get_route_id(),
            "request.setThreadEngineConfig.config.selectionV2.routeId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.routeId",
    )?;
    let reasoning_effort = parse_optional_setting(
        &read_text(
            arm.get_reasoning_effort(),
            "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        |text| HermesReasoningEffort::parse(text),
    )?;
    let permission_mode = HermesPermissionMode::parse(&read_text(
        arm.get_permission_mode(),
        "request.setThreadEngineConfig.config.selectionV2.permissionMode",
    )?)
    .map_err(|error| {
        engine_config_error(
            "request.setThreadEngineConfig.config.selectionV2.permissionMode",
            error.reason(),
        )
    })?;
    Ok(HermesSelection::new(
        profile_id,
        model_id,
        route_id,
        permission_mode,
        reasoning_effort,
        arm.get_fast(),
    ))
}

pub(crate) fn decode_engine_variant(
    value: artisan_capnp::engine_variant::Reader<'_>,
) -> Result<Option<EngineVariantId>, ProtocolDecodeError> {
    let kind = read_text(
        value.get_kind(),
        "request.setThreadEngineConfig.config.variant.kind",
    )?;
    let id = read_text(
        value.get_id(),
        "request.setThreadEngineConfig.config.variant.id",
    )?;
    match kind.as_str() {
        "none" if id.is_empty() => Ok(None),
        "none" => Err(engine_config_error(
            "request.setThreadEngineConfig.config.variant.id",
            EngineConfigReason::Inconsistent,
        )),
        "selected" => EngineVariantId::parse(id).map(Some).map_err(|_| {
            engine_config_error(
                "request.setThreadEngineConfig.config.variant.id",
                EngineConfigReason::InvalidIdentifier,
            )
        }),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.variant.kind",
            EngineConfigReason::Unsupported,
        )),
    }
}

pub(crate) fn decode_engine_permission(
    value: artisan_capnp::engine_permission_policy::Reader<'_>,
) -> Result<EnginePermissionPolicy, ProtocolDecodeError> {
    let permission_id = PermissionId::parse(read_text(
        value.get_permission_id(),
        "request.setThreadEngineConfig.config.permission.permissionId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.permission.permissionId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let agent_id = EngineAgentId::parse(read_text(
        value.get_agent_id(),
        "request.setThreadEngineConfig.config.permission.agentId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.permission.agentId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let approval = parse_approval(&read_text(
        value.get_approval(),
        "request.setThreadEngineConfig.config.permission.approval",
    )?)?;
    let filesystem = parse_filesystem(&read_text(
        value.get_filesystem(),
        "request.setThreadEngineConfig.config.permission.filesystem",
    )?)?;
    let network = parse_network(&read_text(
        value.get_network(),
        "request.setThreadEngineConfig.config.permission.network",
    )?)?;
    let web_search = parse_web_search(&read_text(
        value.get_web_search(),
        "request.setThreadEngineConfig.config.permission.webSearch",
    )?)?;
    Ok(EnginePermissionPolicy::new(
        permission_id,
        agent_id,
        approval,
        filesystem,
        network,
        web_search,
    ))
}

pub(crate) fn parse_approval(value: &str) -> Result<ApprovalMode, ProtocolDecodeError> {
    match value {
        "never" => Ok(ApprovalMode::Never),
        "on_request" => Ok(ApprovalMode::OnRequest),
        "always" => Ok(ApprovalMode::Always),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.permission.approval",
            EngineConfigReason::Unsupported,
        )),
    }
}

pub(crate) fn parse_filesystem(value: &str) -> Result<FilesystemAccess, ProtocolDecodeError> {
    match value {
        "none" => Ok(FilesystemAccess::None),
        "workspace" => Ok(FilesystemAccess::Workspace),
        "host" => Ok(FilesystemAccess::Host),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.permission.filesystem",
            EngineConfigReason::Unsupported,
        )),
    }
}

pub(crate) fn parse_network(value: &str) -> Result<NetworkAccess, ProtocolDecodeError> {
    match value {
        "disabled" => Ok(NetworkAccess::Disabled),
        "enabled" => Ok(NetworkAccess::Enabled),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.permission.network",
            EngineConfigReason::Unsupported,
        )),
    }
}

pub(crate) fn parse_web_search(value: &str) -> Result<WebSearchAccess, ProtocolDecodeError> {
    match value {
        "disabled" => Ok(WebSearchAccess::Disabled),
        "enabled" => Ok(WebSearchAccess::Enabled),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.permission.webSearch",
            EngineConfigReason::Unsupported,
        )),
    }
}

pub(crate) fn decode_engine_runtime(
    value: artisan_capnp::engine_runtime_controls::Reader<'_>,
) -> Result<EngineRuntimeControls, ProtocolDecodeError> {
    let millis = |value: u64, field: &'static str| {
        FiniteMillis::new(value)
            .map_err(|_| engine_config_error(field, EngineConfigReason::OutOfRange))
    };
    let bytes = |value: u64, field: &'static str| {
        ByteLimit::new(value)
            .map_err(|_| engine_config_error(field, EngineConfigReason::OutOfRange))
    };
    let count = |value: u64, field: &'static str| {
        CountLimit::new(value)
            .map_err(|_| engine_config_error(field, EngineConfigReason::OutOfRange))
    };
    Ok(EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: millis(
            value.get_attempt_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.attemptBudgetMs",
        )?,
        readiness_budget: millis(
            value.get_readiness_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.readinessBudgetMs",
        )?,
        health_budget: millis(
            value.get_health_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.healthBudgetMs",
        )?,
        prompt_budget: millis(
            value.get_prompt_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.promptBudgetMs",
        )?,
        stream_budget: millis(
            value.get_stream_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.streamBudgetMs",
        )?,
        close_budget: millis(
            value.get_close_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.closeBudgetMs",
        )?,
        max_json_body_bytes: bytes(
            value.get_max_json_body_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxJsonBodyBytes",
        )?,
        max_sse_line_bytes: bytes(
            value.get_max_sse_line_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxSseLineBytes",
        )?,
        max_sse_event_bytes: bytes(
            value.get_max_sse_event_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxSseEventBytes",
        )?,
        max_readiness_line_bytes: bytes(
            value.get_max_readiness_line_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxReadinessLineBytes",
        )?,
        max_header_count: count(
            value.get_max_header_count(),
            "request.setThreadEngineConfig.config.runtime.maxHeaderCount",
        )?,
        max_http_buffer_bytes: bytes(
            value.get_max_http_buffer_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxHttpBufferBytes",
        )?,
        max_stderr_bytes: bytes(
            value.get_max_stderr_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxStderrBytes",
        )?,
        observation_capacity: count(
            value.get_observation_capacity(),
            "request.setThreadEngineConfig.config.runtime.observationCapacity",
        )?,
    })?)
}

pub(crate) fn decode_thread_engine_config_set(
    value: artisan_capnp::set_thread_engine_config_result::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            value.get_request_id(),
            "response.threadEngineConfigSet.requestId",
        )?,
        "response.threadEngineConfigSet.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.threadEngineConfigSet.requestId",
        });
    }
    let revision = EngineConfigRevision::new(value.get_revision())?;
    Ok(ResponsePayload::ThreadEngineConfigSet(
        SetThreadEngineConfigResult {
            request_id: nested_request_id,
            thread_id: parse_thread_id(
                read_text(
                    value.get_thread_id(),
                    "response.threadEngineConfigSet.threadId",
                )?,
                "response.threadEngineConfigSet.threadId",
            )?,
            revision,
            disposition: decode_disposition(value.get_disposition()?),
        },
    ))
}

pub(crate) fn decode_thread_engine_settings_result(
    value: artisan_capnp::thread_engine_settings_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(
            value.get_thread_id(),
            "response.threadEngineSettings.threadId",
        )?,
        "response.threadEngineSettings.threadId",
    )?;
    let result = match value.get_state().which()? {
        artisan_capnp::thread_engine_settings_result::state::Which::Unconfigured(()) => {
            crate::types::ThreadEngineSettingsResult::Unconfigured { thread_id }
        }
        artisan_capnp::thread_engine_settings_result::state::Which::Configured(configured) => {
            let configured = configured?;
            let revision = EngineConfigRevision::new(configured.get_revision())
                .map_err(|source| ProtocolDecodeError::EngineConfig { source })?;
            let config = decode_engine_run_config(configured.get_config()?)?;
            crate::types::ThreadEngineSettingsResult::Configured {
                thread_id,
                revision,
                config: Box::new(config),
            }
        }
    };
    Ok(ResponsePayload::ThreadEngineSettings(result))
}

pub(crate) fn decode_registered_engine_profiles_result(
    value: artisan_capnp::registered_engine_profiles_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let result = match value.get_state().which()? {
        artisan_capnp::registered_engine_profiles_result::state::Which::RegistryMissing(()) => {
            RegisteredEngineProfilesResult::RegistryMissing
        }
        artisan_capnp::registered_engine_profiles_result::state::Which::RegistryPresent(
            present,
        ) => {
            let present = present?;
            let ids = present.get_profile_ids()?;
            let count = ids.len() as usize;
            if count > 64 {
                return Err(engine_config_error(
                    "response.registeredEngineProfiles.profileIds",
                    EngineConfigReason::OutOfRange,
                ));
            }
            let mut profile_ids = Vec::with_capacity(count);
            let mut seen = std::collections::HashSet::with_capacity(count);
            for raw in ids {
                let text = read_text(raw, "response.registeredEngineProfiles.profileIds")?;
                let id = EngineProfileId::parse(text).map_err(|_| {
                    engine_config_error(
                        "response.registeredEngineProfiles.profileIds",
                        EngineConfigReason::InvalidIdentifier,
                    )
                })?;
                if !seen.insert(id.as_str().to_owned()) {
                    return Err(engine_config_error(
                        "response.registeredEngineProfiles.profileIds",
                        EngineConfigReason::Inconsistent,
                    ));
                }
                profile_ids.push(id);
            }
            RegisteredEngineProfilesResult::RegistryPresent { profile_ids }
        }
    };
    Ok(ResponsePayload::RegisteredEngineProfiles(result))
}
