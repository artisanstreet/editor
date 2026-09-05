//! Converts a validated picker choice into the existing durable engine configuration.
use artisan_catalog::{NativeModelCatalog, NativeModelPolicy};
use artisan_domain::*;

/// A displayed choice must never silently run the previous saved model.
pub(crate) fn validate_run_choice(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
    saved: Option<&EngineRunConfig>,
) -> Result<(), &'static str> {
    catalog.admit_policy(policy).map_err(
        |_| "Connect and configure this model's engine before running. Your draft is preserved.",
    )?;
    let expected = config_for_policy(catalog, policy, saved)?;
    if saved != Some(&expected) {
        return Err(
            "This model's settings have not been saved yet. Your draft is preserved; try again once saving finishes.",
        );
    }
    Ok(())
}

pub(crate) fn config_for_policy(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
    previous: Option<&EngineRunConfig>,
) -> Result<EngineRunConfig, &'static str> {
    catalog
        .validate_policy(policy)
        .map_err(|_| "Model selection is no longer available")?;
    if policy.engine_id != "opencode2" {
        return Err("This engine is not available in the native app yet");
    }
    if policy.reasoning_effort.is_some()
        || policy.speed.is_some()
        || policy.context_window.is_some()
    {
        return Err("This provider does not support these configuration overrides");
    }
    let profile = EngineProfileId::parse(
        policy
            .profile_id
            .clone()
            .ok_or("Select an engine profile")?,
    )
    .map_err(|_| "Invalid engine profile")?;
    let native = policy
        .native_selection
        .as_ref()
        .ok_or("Model routing is unavailable")?;
    let model =
        EngineModelId::parse(native.model_id.clone()).map_err(|_| "Invalid model identity")?;
    let route = EngineRouteId::parse(native.provider_route_id.clone())
        .map_err(|_| "Invalid model route")?;
    let variant = native
        .variant_id
        .clone()
        .map(EngineVariantId::parse)
        .transpose()
        .map_err(|_| "Invalid model variant")?;
    let option = policy
        .permission
        .as_ref()
        .ok_or("Select a permission mode")?;
    let old = previous.map(|config| config.selection().as_opencode2().permission());
    // These are the Electron managed-agent modes in engines/opencode2/config.ts.
    let (approval, filesystem, network) = match option.id.as_str() {
        "restricted" => (
            ApprovalMode::Never,
            FilesystemAccess::None,
            old.map_or(NetworkAccess::Disabled, |p| p.network()),
        ),
        "autonomous" => (
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            old.map_or(NetworkAccess::Disabled, |p| p.network()),
        ),
        "unrestricted" => (
            ApprovalMode::Never,
            FilesystemAccess::Host,
            NetworkAccess::Enabled,
        ),
        _ => return Err("Unsupported permission mode"),
    };
    let web = old.map_or(WebSearchAccess::Disabled, |p| p.web_search());
    let agent = format!(
        "artisan-v1-{}-{}-{}",
        option.id,
        if network == NetworkAccess::Enabled {
            "network"
        } else {
            "offline"
        },
        if web == WebSearchAccess::Enabled {
            "web"
        } else {
            "no-web"
        }
    );
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse(option.id.clone()).map_err(|_| "Invalid permission mode")?,
        EngineAgentId::parse(agent).map_err(|_| "Invalid managed agent")?,
        approval,
        filesystem,
        network,
        web,
    );
    let runtime = match previous {
        Some(config) => config.runtime(),
        None => default_runtime()?,
    };
    Ok(EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            profile, model, route, variant, permission,
        )),
        runtime,
    ))
}

/// Product transport budgets; these never claim a model context size or provider limit.
fn default_runtime() -> Result<EngineRuntimeControls, &'static str> {
    let duration = |value| FiniteMillis::new(value).map_err(|_| "Invalid runtime budget");
    let bytes = |value| ByteLimit::new(value).map_err(|_| "Invalid runtime capacity");
    let count = |value| CountLimit::new(value).map_err(|_| "Invalid runtime capacity");
    EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: duration(3_660_000)?,
        readiness_budget: duration(15_000)?,
        health_budget: duration(5_000)?,
        prompt_budget: duration(30_000)?,
        stream_budget: duration(3_600_000)?,
        close_budget: duration(10_000)?,
        max_json_body_bytes: bytes(24 * 1024 * 1024)?,
        max_sse_line_bytes: bytes(64 * 1024)?,
        max_sse_event_bytes: bytes(1024 * 1024)?,
        max_readiness_line_bytes: bytes(8192)?,
        max_header_count: count(64)?,
        max_http_buffer_bytes: bytes(64 * 1024)?,
        max_stderr_bytes: bytes(64 * 1024)?,
        observation_capacity: count(256)?,
    })
    .map_err(|_| "Invalid runtime configuration")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_transport_can_carry_maximum_image_payload() {
        let runtime = default_runtime().unwrap();
        assert!(runtime.max_json_body_bytes().get() > 12 * 1024 * 1024 / 3 * 4);
        assert_eq!(runtime.stream_budget().get(), 3_600_000);
    }

    #[test]
    fn offline_choice_cannot_fall_back_to_another_run_configuration() {
        let catalog = NativeModelCatalog::offline().unwrap();
        let policy = catalog.selection_policy_for_model("codex-sol").unwrap();
        assert!(validate_run_choice(&catalog, &policy, None).is_err());
        assert!(catalog.validate_selection_policy(&policy).is_ok());
    }
}
