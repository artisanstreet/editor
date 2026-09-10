//! Converts a validated picker choice into the existing durable engine configuration.
use artisan_catalog::{NativeModelCatalog, NativeModelPolicy, NativePermissionOption};
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
    let selection = match policy.engine_id.as_str() {
        "codex" | "claude" | "grok" | "cursor" | "hermes" => {
            native_selection_config(catalog, policy, previous)?
        }
        _ => opencode2_selection_config(policy, previous)?,
    };
    let runtime = match previous {
        Some(config) => config.runtime(),
        None => default_runtime()?,
    };
    Ok(EngineRunConfig::new(selection, runtime))
}

/// Builds the durable selection for the five fixture-proven native engines.
///
/// Every arm mirrors the backend TypeScript resolver
/// (`modules/backend/src/orchestration/session-policy.ts`): the canonical
/// permission traits travel into an [`EnginePermissionPolicy`], the native
/// permission spelling travels into the engine's own permission-mode option,
/// and effort/speed/window choices travel only where the durable selection
/// has a field for them. A validated choice the selection cannot represent
/// is an honest error, never a silent downgrade — except where the resolver
/// itself drops the axis (Hermes context, neutral standard speed), which is
/// documented at the call site.
fn native_selection_config(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
    previous: Option<&EngineRunConfig>,
) -> Result<EngineSelection, &'static str> {
    let profile = EngineProfileId::parse(
        policy
            .profile_id
            .clone()
            .ok_or("Select an engine profile")?,
    )
    .map_err(|_| "Invalid engine profile")?;
    match policy.engine_id.as_str() {
        "codex" => {
            let permission = codex_permission(catalog, policy, previous)?;
            let reasoning_effort = policy
                .reasoning_effort
                .as_ref()
                .map(|value| {
                    CodexReasoningEffort::parse(&value.native_value)
                        .map_err(|_| "This provider does not support these configuration overrides")
                })
                .transpose()?;
            let service_tier = policy
                .speed
                .as_ref()
                .map(|value| {
                    CodexServiceTier::parse(&value.native_value)
                        .map_err(|_| "This provider does not support these configuration overrides")
                })
                .transpose()?;
            let model_context_window = codex_context_window(policy)?;
            let model = direct_model(policy)?;
            let model = EngineModelId::parse(model).map_err(|_| "Invalid model identity")?;
            CodexSelection::new(
                profile,
                Some(model),
                permission,
                reasoning_effort,
                service_tier,
                model_context_window,
            )
            .map(EngineSelection::Codex)
            .map_err(|_| "This permission selection cannot run on this engine")
        }
        "claude" => {
            let permission = claude_permission(catalog, policy, previous)?;
            let option = harness_option(catalog, policy)?;
            let permission_mode = ClaudePermissionMode::parse(&option.native_value)
                .map(Some)
                .map_err(|_| "Unsupported permission mode")?;
            let effort = policy
                .reasoning_effort
                .as_ref()
                .map(|value| {
                    ClaudeEffort::parse(&value.native_value)
                        .map_err(|_| "This provider does not support these configuration overrides")
                })
                .transpose()?;
            neutral_speed(policy)?;
            let model = claude_model(policy)?;
            let model = EngineModelId::parse(model).map_err(|_| "Invalid model identity")?;
            ClaudeSelection::new(
                profile,
                Some(model),
                permission,
                effort,
                permission_mode,
                false,
                false,
            )
            .map(EngineSelection::Claude)
            .map_err(|_| "This permission selection cannot run on this engine")
        }
        "grok" => {
            let permission = shared_permission(catalog, policy, previous)?;
            let permission_mode = grok_permission_mode(catalog, policy)?;
            let reasoning_effort = policy
                .reasoning_effort
                .as_ref()
                .map(|value| {
                    GrokReasoningEffort::parse(value.native_value.clone())
                        .map_err(|_| "This provider does not support these configuration overrides")
                })
                .transpose()?;
            neutral_speed(policy)?;
            let model = direct_model(policy)?;
            let model = EngineModelId::parse(model).map_err(|_| "Invalid model identity")?;
            Ok(EngineSelection::Grok(GrokSelection::new(
                profile,
                Some(model),
                permission,
                reasoning_effort,
                permission_mode,
            )))
        }
        "cursor" => {
            let permission = shared_permission(catalog, policy, previous)?;
            let permission_mode = cursor_permission_mode(catalog, policy)?;
            let reasoning_effort = policy
                .reasoning_effort
                .as_ref()
                .map(|value| {
                    CursorReasoningEffort::parse(value.native_value.clone())
                        .map_err(|_| "This provider does not support these configuration overrides")
                })
                .transpose()?;
            let speed = match policy.speed.as_ref() {
                None => None,
                Some(value) if value.native_value == "standard" => None,
                Some(value) if value.native_value == "fast" => Some(CursorSpeed::Fast),
                Some(_) => {
                    return Err("This provider does not support these configuration overrides");
                }
            };
            let model = direct_model(policy)?;
            let model = EngineModelId::parse(model).map_err(|_| "Invalid model identity")?;
            Ok(EngineSelection::Cursor(CursorSelection::new(
                profile,
                Some(model),
                permission,
                reasoning_effort,
                speed,
                permission_mode,
            )))
        }
        "hermes" => {
            let native = policy
                .native_selection
                .as_ref()
                .ok_or("Model routing is unavailable")?;
            if native.variant_id.is_some() {
                return Err("This provider does not support these configuration overrides");
            }
            let model = EngineModelId::parse(native.model_id.clone())
                .map_err(|_| "Invalid model identity")?;
            let route = EngineRouteId::parse(native.provider_route_id.clone())
                .map_err(|_| "Invalid model route")?;
            let option = harness_option(catalog, policy)?;
            let permission_mode = HermesPermissionMode::parse(&option.native_value)
                .map_err(|_| "Unsupported permission mode")?;
            let reasoning_effort = policy
                .reasoning_effort
                .as_ref()
                .map(|value| {
                    HermesReasoningEffort::parse(value.native_value.clone())
                        .map_err(|_| "This provider does not support these configuration overrides")
                })
                .transpose()?;
            // The resolver carries no Hermes context axis: Hermes authorization
            // and routing own the session, so a validated window choice stays
            // unrepresented rather than failing a runnable Hermes policy.
            let fast = match policy.speed.as_ref() {
                None => false,
                Some(value) if value.native_value == "standard" => false,
                Some(value) if value.native_value == "fast" => true,
                Some(_) => {
                    return Err("This provider does not support these configuration overrides");
                }
            };
            Ok(EngineSelection::Hermes(HermesSelection::new(
                profile,
                model,
                route,
                permission_mode,
                reasoning_effort,
                fast,
            )))
        }
        _ => Err("This engine is not available in the native app yet"),
    }
}

/// Builds the durable selection for the incumbent `OpenCode` 2 engine.
///
/// This is the original `config_for_policy` body, unchanged apart from
/// returning the selection: the caller owns runtime assembly for every
/// engine now. The managed-agent modes below are the Electron modes in
/// `engines/opencode2/config.ts`, and the agent identity keeps its exact
/// historical spelling so saved configurations keep round-tripping.
fn opencode2_selection_config(
    policy: &NativeModelPolicy,
    previous: Option<&EngineRunConfig>,
) -> Result<EngineSelection, &'static str> {
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
    // A previous configuration for another engine carries no OpenCode2
    // policy to inherit; the restrictive defaults below apply instead.
    let old = previous.and_then(|config| config.selection().as_opencode2().ok());
    // These are the Electron managed-agent modes in engines/opencode2/config.ts.
    let (approval, filesystem, network) = match option.id.as_str() {
        "restricted" => (
            ApprovalMode::Never,
            FilesystemAccess::None,
            old.map_or(NetworkAccess::Disabled, |selection| {
                selection.permission().network()
            }),
        ),
        "autonomous" => (
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            old.map_or(NetworkAccess::Disabled, |selection| {
                selection.permission().network()
            }),
        ),
        "unrestricted" => (
            ApprovalMode::Never,
            FilesystemAccess::Host,
            NetworkAccess::Enabled,
        ),
        _ => return Err("Unsupported permission mode"),
    };
    let web = old.map_or(WebSearchAccess::Disabled, |selection| {
        selection.permission().web_search()
    });
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
    Ok(EngineSelection::OpenCode2(OpenCode2Selection::new(
        profile, model, route, variant, permission,
    )))
}

/// Returns the validated manifest permission option for a policy.
///
/// Membership was already proven by [`NativeModelCatalog::validate_policy`];
/// this projects the option's canonical traits so every engine maps the same
/// catalog truth instead of re-spelling it per engine.
fn harness_option<'a>(
    catalog: &'a NativeModelCatalog,
    policy: &NativeModelPolicy,
) -> Result<&'a NativePermissionOption, &'static str> {
    let selected = policy
        .permission
        .as_ref()
        .ok_or("Select a permission mode")?;
    let harness = catalog
        .manifest
        .harness(&policy.engine_id)
        .ok_or("Model selection is no longer available")?;
    harness
        .permissions
        .options
        .iter()
        .find(|option| option.id == selected.id && option.native_value == selected.native_value)
        .ok_or("Unsupported permission mode")
}

/// Maps one catalog permission option onto the canonical access axes.
///
/// This mirrors the TypeScript policy projection (`policy_fields_for_permission`
/// in `lib/engine/model-selection.ts` plus the narrowing in
/// `orchestration/session-policy.ts`): `none` approval behavior means never
/// ask, every other behavior asks on request, and the edit scope names the
/// filesystem boundary verbatim.
fn canonical_access(
    option: &NativePermissionOption,
) -> Result<(ApprovalMode, FilesystemAccess), &'static str> {
    let approval = match option.approval_behavior.as_str() {
        "none" => ApprovalMode::Never,
        "prompts" | "classifier" => ApprovalMode::OnRequest,
        _ => return Err("Unsupported permission mode"),
    };
    let filesystem = match option.edit_scope.as_str() {
        "none" => FilesystemAccess::None,
        "workspace" => FilesystemAccess::Workspace,
        "host" => FilesystemAccess::Host,
        _ => return Err("Unsupported permission mode"),
    };
    Ok((approval, filesystem))
}

/// Inherits network and web-search access from a previous same-engine
/// configuration, defaulting to restrictive when there is none.
///
/// A previous configuration for another engine carries no policy to inherit,
/// matching the `OpenCode` 2 path's treatment of foreign selections.
fn inherited_network_web(
    previous: Option<&EngineRunConfig>,
    engine_id: &str,
) -> (NetworkAccess, WebSearchAccess) {
    let permission = previous
        .map(EngineRunConfig::selection)
        .filter(|selection| selection.engine_id().as_str() == engine_id)
        .and_then(EngineSelection::permission);
    (
        permission.map_or(NetworkAccess::Disabled, |policy| policy.network()),
        permission.map_or(WebSearchAccess::Disabled, |policy| policy.web_search()),
    )
}

/// Names the managed agent for a durable native-engine permission.
///
/// Engine-scoped so a Codex autonomous selection never wears the `OpenCode` 2
/// agent spelling: the `OpenCode` 2 path keeps its own historical format.
fn managed_agent(
    engine_id: &str,
    option_id: &str,
    network: NetworkAccess,
    web: WebSearchAccess,
) -> Result<EngineAgentId, &'static str> {
    let agent = format!(
        "artisan-v1-{engine_id}-{option_id}-{}-{}",
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
    EngineAgentId::parse(agent).map_err(|_| "Invalid managed agent")
}

/// Builds a canonical permission policy for one engine and option.
///
/// Host scope forces network access, mirroring the resolver (a host-wide
/// sandbox removes network isolation); workspace and read-only scopes inherit
/// the previous same-engine network grant so a saved web-search grant
/// survives reselection.
fn engine_permission(
    policy: &NativeModelPolicy,
    previous: Option<&EngineRunConfig>,
    option: &NativePermissionOption,
    approval: ApprovalMode,
    filesystem: FilesystemAccess,
) -> Result<EnginePermissionPolicy, &'static str> {
    let (network, web) = inherited_network_web(previous, &policy.engine_id);
    let network = if filesystem == FilesystemAccess::Host {
        NetworkAccess::Enabled
    } else {
        network
    };
    let permission_id =
        PermissionId::parse(option.id.clone()).map_err(|_| "Invalid permission mode")?;
    let agent = managed_agent(&policy.engine_id, &option.id, network, web)?;
    Ok(EnginePermissionPolicy::new(
        permission_id,
        agent,
        approval,
        filesystem,
        network,
        web,
    ))
}

/// Builds the Codex permission policy straight from the catalog traits.
///
/// The adapter maps the durable axes verbatim (`read-only`, `workspace-write`,
/// `danger-full-access` in `engine_owner/codex.rs`), so no floor applies.
fn codex_permission(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
    previous: Option<&EngineRunConfig>,
) -> Result<EnginePermissionPolicy, &'static str> {
    let option = harness_option(catalog, policy)?;
    let (approval, filesystem) = canonical_access(option)?;
    engine_permission(policy, previous, option, approval, filesystem)
}

/// Builds the Claude permission policy at the adapter's floor.
///
/// The CLI always needs write and network access (`cli-engine.ts` rejects a
/// policy without them, and the native adapter fails closed the same way), so
/// read-only plan mode is granted the workspace floor while plan-ness itself
/// travels in the native `plan` permission mode, not in a denied filesystem.
fn claude_permission(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
    previous: Option<&EngineRunConfig>,
) -> Result<EnginePermissionPolicy, &'static str> {
    let option = harness_option(catalog, policy)?;
    let (approval, filesystem) = canonical_access(option)?;
    let filesystem = match filesystem {
        FilesystemAccess::None => FilesystemAccess::Workspace,
        scoped => scoped,
    };
    let (_, web) = inherited_network_web(previous, &policy.engine_id);
    let selected = policy
        .permission
        .as_ref()
        .ok_or("Select a permission mode")?;
    let permission_id =
        PermissionId::parse(selected.id.clone()).map_err(|_| "Invalid permission mode")?;
    let agent = managed_agent(&policy.engine_id, &selected.id, NetworkAccess::Enabled, web)?;
    Ok(EnginePermissionPolicy::new(
        permission_id,
        agent,
        approval,
        filesystem,
        NetworkAccess::Enabled,
        web,
    ))
}

/// Builds the shared-trait permission policy for Grok and Cursor.
///
/// Both adapters derive write access from a non-`None` filesystem at
/// argument-building time (`engine_owner/grok.rs`, `engine_owner/cursor.rs`),
/// so the catalog traits pass through with only the host network rule.
fn shared_permission(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
    previous: Option<&EngineRunConfig>,
) -> Result<EnginePermissionPolicy, &'static str> {
    let option = harness_option(catalog, policy)?;
    let (approval, filesystem) = canonical_access(option)?;
    engine_permission(policy, previous, option, approval, filesystem)
}

/// Resolves the Grok native permission mode.
///
/// Read-only `plan` has no adapter spelling: like the native settings
/// derivation, a denied filesystem selects plan mode at argument-building
/// time, so the selection carries no mode for it.
fn grok_permission_mode(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
) -> Result<Option<GrokPermissionMode>, &'static str> {
    let option = harness_option(catalog, policy)?;
    if option.native_value == "plan" {
        return Ok(None);
    }
    GrokPermissionMode::parse(&option.native_value)
        .map(Some)
        .map_err(|_| "Unsupported permission mode")
}

/// Resolves the Cursor native permission mode.
///
/// Only `force` maps to a CLI flag; read-only Ask mode and the interactive
/// default are selected by the canonical filesystem axis at
/// argument-building time (`engine_owner/cursor.rs`).
fn cursor_permission_mode(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
) -> Result<Option<CursorPermissionMode>, &'static str> {
    let option = harness_option(catalog, policy)?;
    if option.native_value == "ask" || option.native_value == "default" {
        return Ok(None);
    }
    CursorPermissionMode::parse(&option.native_value)
        .map(Some)
        .map_err(|_| "Unsupported permission mode")
}

/// Returns the CLI-facing model for a direct (non-routed) engine model.
///
/// Route-aware selections contribute their route-local model id, matching the
/// `OpenCode` 2 path and the TypeScript selector; provider-owned variants
/// have no durable field on these selections and are an honest error.
fn direct_model(policy: &NativeModelPolicy) -> Result<String, &'static str> {
    match policy.native_selection.as_ref() {
        Some(native) => {
            if native.variant_id.is_some() {
                return Err("This provider does not support these configuration overrides");
            }
            Ok(native.model_id.clone())
        }
        None => Ok(policy.native_model_id.clone()),
    }
}

/// Composes the Claude CLI model id with its context-window suffix.
///
/// This mirrors `ComposeNativeModelId` in `catalog/src/context-window.ts`: a
/// window expressed as configuration never reaches the model id (Claude has
/// no such option today, so one is an honest error), while an identity suffix
/// such as `[1m]` is appended verbatim.
fn claude_model(policy: &NativeModelPolicy) -> Result<String, &'static str> {
    let base = direct_model(policy)?;
    match policy.context_window.as_ref() {
        None => Ok(base),
        Some(choice) => {
            if choice.native_config.is_some() {
                return Err("This provider does not support these configuration overrides");
            }
            Ok(format!("{base}{}", choice.native_suffix))
        }
    }
}

/// Resolves the Codex model-context-window override.
///
/// Only a configuration option (the extended window's `native_config`)
/// produces an override; the base window's empty suffix selects nothing, and
/// an identity-style suffix would name a model the Codex catalog cannot
/// resolve, so it is an honest error.
fn codex_context_window(
    policy: &NativeModelPolicy,
) -> Result<Option<CodexModelContextWindow>, &'static str> {
    match policy.context_window.as_ref() {
        None => Ok(None),
        Some(choice) => match choice.native_config.as_ref() {
            Some(config) => CodexModelContextWindow::new(config.model_context_window)
                .map(Some)
                .map_err(|_| "Invalid runtime configuration"),
            None if choice.native_suffix.is_empty() => Ok(None),
            None => Err("This provider does not support these configuration overrides"),
        },
    }
}

/// Accepts an absent or neutral-standard speed where the durable selection
/// has no speed field.
///
/// The resolver sends no speed axis for Claude and Grok, and `standard` is
/// the neutral tier everywhere; anything else names an override the
/// selection cannot represent.
fn neutral_speed(policy: &NativeModelPolicy) -> Result<(), &'static str> {
    match policy.speed.as_ref() {
        None => Ok(()),
        Some(value) if value.native_value == "standard" => Ok(()),
        Some(_) => Err("This provider does not support these configuration overrides"),
    }
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
    use artisan_catalog::{
        NativeContextConfig, NativeContextSelection, NativeModelDefinition, NativeModelRoute,
        NativeModelRouteGroup, NativeModelRouteStatus, NativeModelSelection, NativeOptionValue,
    };

    fn profiled_policy(catalog: &NativeModelCatalog, model_id: &str) -> NativeModelPolicy {
        let mut policy = catalog.selection_policy_for_model(model_id).unwrap();
        policy.profile_id = Some("default".to_owned());
        policy
    }

    /// Builds the offline snapshot with the five fixture-proven harness ids
    /// marked runnable, mirroring the catalog test helpers: static
    /// direct models need no route, so runnable harnesses admit them.
    fn runnable_catalog() -> NativeModelCatalog {
        let mut catalog = NativeModelCatalog::offline().unwrap();
        for engine_id in ["codex", "claude", "grok", "cursor", "hermes"] {
            catalog.runnable_harness_ids.push(engine_id.to_owned());
        }
        catalog
    }

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

    #[test]
    fn offline_catalog_admits_nothing_runnable() {
        let catalog = NativeModelCatalog::offline().unwrap();
        assert!(catalog.runnable_harness_ids.is_empty());
        let policy = profiled_policy(&catalog, "codex-sol");
        assert!(catalog.admit_policy(&policy).is_err());
        assert!(validate_run_choice(&catalog, &policy, None).is_err());
    }

    #[test]
    fn codex_choice_builds_a_runnable_codex_selection() {
        let catalog = runnable_catalog();
        let policy = profiled_policy(&catalog, "codex-sol");
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let EngineSelection::Codex(selection) = config.selection() else {
            panic!("expected a Codex selection");
        };
        assert_eq!(selection.model_id().unwrap().as_str(), "gpt-5.6-sol");
        assert_eq!(selection.reasoning_effort().unwrap().as_str(), "high");
        assert_eq!(selection.service_tier().unwrap().as_str(), "standard");
        assert_eq!(selection.model_context_window(), None);
        assert_eq!(selection.permission().approval(), ApprovalMode::OnRequest);
        assert_eq!(
            selection.permission().filesystem(),
            FilesystemAccess::Workspace
        );
        assert_eq!(selection.permission().network(), NetworkAccess::Disabled);
        assert!(validate_run_choice(&catalog, &policy, Some(&config)).is_ok());
    }

    #[test]
    fn codex_extended_window_selects_a_window_override() {
        let catalog = NativeModelCatalog::offline().unwrap();
        let mut policy = profiled_policy(&catalog, "codex-sol");
        policy.context_window = Some(NativeContextSelection {
            id: "extended".to_owned(),
            native_suffix: "1m".to_owned(),
            native_config: Some(NativeContextConfig {
                model_context_window: 1_050_000,
            }),
        });
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let EngineSelection::Codex(selection) = config.selection() else {
            panic!("expected a Codex selection");
        };
        assert_eq!(
            selection.model_context_window().map(|window| window.get()),
            Some(1_050_000)
        );
        // The window is configuration, never identity: the model id stays bare.
        assert_eq!(selection.model_id().unwrap().as_str(), "gpt-5.6-sol");
    }

    #[test]
    fn claude_choice_builds_a_floored_claude_selection() {
        let catalog = runnable_catalog();
        let policy = profiled_policy(&catalog, "claude-fable");
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let EngineSelection::Claude(selection) = config.selection() else {
            panic!("expected a Claude selection");
        };
        // The default extended window is identity: it composes into the model.
        assert_eq!(selection.model_id().unwrap().as_str(), "claude-fable-5[1m]");
        assert_eq!(
            selection.permission_mode(),
            Some(ClaudePermissionMode::Auto)
        );
        assert_eq!(selection.effort().unwrap().as_str(), "high");
        // The CLI always needs write and network access, even for plan mode.
        assert_eq!(selection.permission().filesystem(), FilesystemAccess::Host);
        assert_eq!(selection.permission().network(), NetworkAccess::Enabled);
        assert!(validate_run_choice(&catalog, &policy, Some(&config)).is_ok());
    }

    #[test]
    fn claude_restricted_selects_plan_mode_at_the_write_floor() {
        let catalog = NativeModelCatalog::offline().unwrap();
        let mut policy = profiled_policy(&catalog, "claude-fable");
        policy.permission = Some(NativeOptionValue {
            id: "restricted".to_owned(),
            native_value: "plan".to_owned(),
        });
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let EngineSelection::Claude(selection) = config.selection() else {
            panic!("expected a Claude selection");
        };
        assert_eq!(
            selection.permission_mode(),
            Some(ClaudePermissionMode::Plan)
        );
        assert_eq!(
            selection.permission().filesystem(),
            FilesystemAccess::Workspace
        );
        assert_eq!(selection.permission().network(), NetworkAccess::Enabled);
    }

    #[test]
    fn claude_unrepresentable_speed_is_an_honest_error() {
        let catalog = NativeModelCatalog::offline().unwrap();
        let mut policy = profiled_policy(&catalog, "claude-opus");
        policy.speed = Some(NativeOptionValue {
            id: "fast".to_owned(),
            native_value: "fast".to_owned(),
        });
        assert_eq!(
            config_for_policy(&catalog, &policy, None).unwrap_err(),
            "This provider does not support these configuration overrides"
        );
    }

    #[test]
    fn grok_choice_builds_a_grok_selection() {
        let catalog = runnable_catalog();
        let policy = profiled_policy(&catalog, "grok-4-6");
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let EngineSelection::Grok(selection) = config.selection() else {
            panic!("expected a Grok selection");
        };
        assert_eq!(selection.model_id().unwrap().as_str(), "grok-4.6");
        assert_eq!(selection.reasoning_effort().unwrap().as_str(), "high");
        assert_eq!(selection.permission_mode(), Some(GrokPermissionMode::Auto));
        assert_eq!(selection.permission().approval(), ApprovalMode::OnRequest);
        assert_eq!(selection.permission().filesystem(), FilesystemAccess::Host);
        assert_eq!(selection.permission().network(), NetworkAccess::Enabled);
        assert!(validate_run_choice(&catalog, &policy, Some(&config)).is_ok());
    }

    #[test]
    fn cursor_choice_builds_a_cursor_selection_without_speed() {
        let catalog = runnable_catalog();
        let mut policy = profiled_policy(&catalog, "cursor-composer-2-5");
        // The fixture defaults this model to fast speed; clear it so this
        // test exercises the no-speed path its name claims.
        policy.speed = None;
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let EngineSelection::Cursor(selection) = config.selection() else {
            panic!("expected a Cursor selection");
        };
        assert_eq!(selection.model_id().unwrap().as_str(), "composer-2.5");
        assert_eq!(selection.reasoning_effort(), None);
        assert_eq!(selection.speed(), None);
        assert_eq!(selection.permission_mode(), None);
        assert_eq!(selection.permission().filesystem(), FilesystemAccess::Host);
        assert!(validate_run_choice(&catalog, &policy, Some(&config)).is_ok());
    }

    #[test]
    fn cursor_fast_speed_selects_fast_delivery() {
        let catalog = NativeModelCatalog::offline().unwrap();
        let mut policy = profiled_policy(&catalog, "cursor-composer-2-5");
        policy.speed = Some(NativeOptionValue {
            id: "fast".to_owned(),
            native_value: "fast".to_owned(),
        });
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let EngineSelection::Cursor(selection) = config.selection() else {
            panic!("expected a Cursor selection");
        };
        assert_eq!(selection.speed(), Some(CursorSpeed::Fast));
    }

    /// Extends the runnable snapshot with a routed Hermes model plus a direct
    /// one, mirroring the live dynamic rows the backend merges at runtime.
    /// The routed row requires its provider route, so the helper seats that
    /// route Available the way the catalog test helpers do.
    fn catalog_with_hermes_models() -> NativeModelCatalog {
        let mut catalog = runnable_catalog();
        let template = catalog.manifest.model("codex-sol").cloned().unwrap();
        let routed = NativeModelDefinition {
            id: "hermes-test-route".to_owned(),
            native_model_id: "openai/gpt-test".to_owned(),
            harness: "hermes".to_owned(),
            provider: "openai".to_owned(),
            native_selection: Some(NativeModelSelection {
                model_id: "openai/gpt-test".to_owned(),
                provider_route_id: "openai-codex".to_owned(),
                variant_id: None,
            }),
            ..template.clone()
        };
        let direct = NativeModelDefinition {
            id: "hermes-test-direct".to_owned(),
            native_model_id: "openai/gpt-direct".to_owned(),
            harness: "hermes".to_owned(),
            provider: "openai".to_owned(),
            native_selection: None,
            ..template
        };
        catalog.manifest.models.push(routed);
        catalog.manifest.models.push(direct);
        catalog.routes.push(NativeModelRoute {
            engine_id: "hermes".to_owned(),
            group: NativeModelRouteGroup {
                id: "openai-codex".to_owned(),
                label: "OpenAI Codex".to_owned(),
                order: 0,
                show_route_labels: false,
            },
            id: "openai-codex".to_owned(),
            label: "OpenAI Codex".to_owned(),
            status: NativeModelRouteStatus::Available,
            unavailable_reason: None,
        });
        catalog
    }

    fn hermes_policy(catalog: &NativeModelCatalog, model_id: &str) -> NativeModelPolicy {
        let model = catalog.manifest.model(model_id).unwrap();
        NativeModelPolicy {
            catalog_revision: catalog.catalog_revision.clone(),
            profile_id: Some("default".to_owned()),
            engine_id: model.harness.clone(),
            model_id: model.id.clone(),
            native_model_id: model.native_model_id.clone(),
            native_selection: model.native_selection.clone(),
            reasoning_effort: None,
            speed: None,
            context_window: None,
            permission: Some(NativeOptionValue {
                id: "autonomous".to_owned(),
                native_value: "profile".to_owned(),
            }),
        }
    }

    #[test]
    fn hermes_routed_choice_builds_a_hermes_selection() {
        let catalog = catalog_with_hermes_models();
        let policy = hermes_policy(&catalog, "hermes-test-route");
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let EngineSelection::Hermes(selection) = config.selection() else {
            panic!("expected a Hermes selection");
        };
        assert_eq!(selection.model_id().as_str(), "openai/gpt-test");
        assert_eq!(selection.route_id().as_str(), "openai-codex");
        assert_eq!(selection.permission_mode(), HermesPermissionMode::Profile);
        assert!(!selection.fast());
        assert!(validate_run_choice(&catalog, &policy, Some(&config)).is_ok());
    }

    #[test]
    fn hermes_choice_without_routing_is_an_honest_error() {
        let catalog = catalog_with_hermes_models();
        let policy = hermes_policy(&catalog, "hermes-test-direct");
        assert_eq!(
            config_for_policy(&catalog, &policy, None).unwrap_err(),
            "Model routing is unavailable"
        );
    }

    #[test]
    fn unknown_registry_engine_stays_unavailable() {
        let mut catalog = NativeModelCatalog::offline().unwrap();
        let mut harness = catalog.manifest.harness("codex").cloned().unwrap();
        harness.id = "custom".to_owned();
        catalog.manifest.harnesses.push(harness);
        let mut model = catalog.manifest.model("codex-sol").cloned().unwrap();
        model.id = "custom-model".to_owned();
        model.harness = "custom".to_owned();
        catalog.manifest.models.push(model);
        let mut policy = catalog.selection_policy_for_model("custom-model").unwrap();
        policy.profile_id = Some("default".to_owned());
        assert_eq!(
            config_for_policy(&catalog, &policy, None).unwrap_err(),
            "This engine is not available in the native app yet"
        );
    }
}
