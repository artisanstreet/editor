//! Building the configuration a manual (`OpenCode` 2-shaped) settings
//! document describes.
//!
//! The Editor's manual settings form sends its fields as text; the Forge
//! validates every value and the registered profile and builds the typed
//! [`EngineRunConfig`], or refuses with the first invalid field. This is the
//! former Editor-side `EngineSettingsDraft::build_config`.

use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, EngineAgentId, EngineConfigError, EngineConfigReason,
    EngineModelId, EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig,
    EngineRuntimeControls, EngineRuntimeControlsInput, EngineSelection, EngineVariantId,
    FilesystemAccess, FiniteMillis, ManualEngineConfiguration, NetworkAccess, OpenCode2Selection,
    PermissionId, WebSearchAccess,
};

fn invalid(field: &'static str) -> EngineConfigError {
    EngineConfigError::new(field, EngineConfigReason::InvalidIdentifier)
}

fn unsupported(field: &'static str) -> EngineConfigError {
    EngineConfigError::new(field, EngineConfigReason::Unsupported)
}

/// Builds the configuration `manual` describes, with its profile among the
/// `registered` ones.
///
/// # Errors
///
/// Returns the first bounded field failure without the rejected value.
pub(crate) fn build_manual_config(
    manual: &ManualEngineConfiguration,
    registered: &[EngineProfileId],
) -> Result<EngineRunConfig, EngineConfigError> {
    let profile_id =
        EngineProfileId::parse(manual.profile_id.clone()).map_err(|_| invalid("profile_id"))?;
    if !registered.contains(&profile_id) {
        return Err(invalid("profile_id"));
    }
    let variant_id = if manual.variant_id.is_empty() {
        None
    } else {
        Some(EngineVariantId::parse(manual.variant_id.clone()).map_err(|_| invalid("variant_id"))?)
    };
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse(manual.permission_id.clone()).map_err(|_| invalid("permission_id"))?,
        EngineAgentId::parse(manual.agent_id.clone()).map_err(|_| invalid("agent_id"))?,
        match manual.approval.as_str() {
            "never" => ApprovalMode::Never,
            "on_request" => ApprovalMode::OnRequest,
            "always" => ApprovalMode::Always,
            _ => return Err(unsupported("approval")),
        },
        match manual.filesystem.as_str() {
            "none" => FilesystemAccess::None,
            "workspace" => FilesystemAccess::Workspace,
            "host" => FilesystemAccess::Host,
            _ => return Err(unsupported("filesystem")),
        },
        match manual.network.as_str() {
            "disabled" => NetworkAccess::Disabled,
            "enabled" => NetworkAccess::Enabled,
            _ => return Err(unsupported("network")),
        },
        match manual.web_search.as_str() {
            "disabled" => WebSearchAccess::Disabled,
            "enabled" => WebSearchAccess::Enabled,
            _ => return Err(unsupported("web_search")),
        },
    );
    let selection = EngineSelection::OpenCode2(OpenCode2Selection::new(
        profile_id,
        EngineModelId::parse(manual.model_id.clone()).map_err(|_| invalid("model_id"))?,
        EngineRouteId::parse(manual.route_id.clone()).map_err(|_| invalid("route_id"))?,
        variant_id,
        permission,
    ));
    Ok(EngineRunConfig::new(selection, runtime_controls(manual)?))
}

fn number(field: &'static str, value: &str) -> Result<u64, EngineConfigError> {
    value.parse::<u64>().map_err(|_| invalid(field))
}

fn millis(field: &'static str, value: &str) -> Result<FiniteMillis, EngineConfigError> {
    FiniteMillis::new(number(field, value)?)
        .map_err(|error| EngineConfigError::new(field, error.reason()))
}

fn bytes(field: &'static str, value: &str) -> Result<ByteLimit, EngineConfigError> {
    ByteLimit::new(number(field, value)?)
        .map_err(|error| EngineConfigError::new(field, error.reason()))
}

fn count(field: &'static str, value: &str) -> Result<CountLimit, EngineConfigError> {
    CountLimit::new(number(field, value)?)
        .map_err(|error| EngineConfigError::new(field, error.reason()))
}

fn runtime_controls(
    manual: &ManualEngineConfiguration,
) -> Result<EngineRuntimeControls, EngineConfigError> {
    EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: millis("attempt_budget", &manual.attempt_budget)?,
        readiness_budget: millis("readiness_budget", &manual.readiness_budget)?,
        health_budget: millis("health_budget", &manual.health_budget)?,
        prompt_budget: millis("prompt_budget", &manual.prompt_budget)?,
        stream_budget: millis("stream_budget", &manual.stream_budget)?,
        close_budget: millis("close_budget", &manual.close_budget)?,
        max_json_body_bytes: bytes("max_json_body_bytes", &manual.max_json_body_bytes)?,
        max_sse_line_bytes: bytes("max_sse_line_bytes", &manual.max_sse_line_bytes)?,
        max_sse_event_bytes: bytes("max_sse_event_bytes", &manual.max_sse_event_bytes)?,
        max_readiness_line_bytes: bytes(
            "max_readiness_line_bytes",
            &manual.max_readiness_line_bytes,
        )?,
        max_header_count: count("max_header_count", &manual.max_header_count)?,
        max_http_buffer_bytes: bytes("max_http_buffer_bytes", &manual.max_http_buffer_bytes)?,
        max_stderr_bytes: bytes("max_stderr_bytes", &manual.max_stderr_bytes)?,
        observation_capacity: count("observation_capacity", &manual.observation_capacity)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(profile: &str) -> ManualEngineConfiguration {
        ManualEngineConfiguration {
            profile_id: profile.into(),
            model_id: "model".into(),
            route_id: "route".into(),
            variant_id: String::new(),
            permission_id: "permission".into(),
            agent_id: "agent".into(),
            approval: "on_request".into(),
            filesystem: "workspace".into(),
            network: "enabled".into(),
            web_search: "disabled".into(),
            attempt_budget: "100".into(),
            readiness_budget: "1".into(),
            health_budget: "1".into(),
            prompt_budget: "1".into(),
            stream_budget: "1".into(),
            close_budget: "1".into(),
            max_json_body_bytes: "8192".into(),
            max_sse_line_bytes: "4096".into(),
            max_sse_event_bytes: "8192".into(),
            max_readiness_line_bytes: "4096".into(),
            max_header_count: "8".into(),
            max_http_buffer_bytes: "8192".into(),
            max_stderr_bytes: "4096".into(),
            observation_capacity: "16".into(),
        }
    }

    #[test]
    fn a_complete_document_on_a_registered_profile_builds_its_configuration() {
        let registered = [EngineProfileId::parse("profile").expect("profile")];
        let config = build_manual_config(&filled("profile"), &registered).expect("builds");
        assert_eq!(
            ManualEngineConfiguration::from_config(&config),
            filled("profile")
        );
        let unregistered = build_manual_config(&filled("other"), &registered)
            .expect_err("an unregistered profile is refused");
        assert_eq!(unregistered.field(), "profile_id");
        let mut bad = filled("profile");
        bad.approval = "sometimes".into();
        assert_eq!(
            build_manual_config(&bad, &registered)
                .expect_err("an unknown approval is refused")
                .field(),
            "approval"
        );
        assert!(build_manual_config(&ManualEngineConfiguration::default(), &registered).is_err());
    }
}
