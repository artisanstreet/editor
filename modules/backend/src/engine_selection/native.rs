//! Engine selections for the four native engines (Codex, Claude, Grok,
//! Cursor), built from a validated catalog policy.
//!
//! Moved from the Editor's `composer_model_config` with the rest of engine
//! configuration construction: the Forge is the only builder of the
//! configurations it runs.

#![forbid(unsafe_code)]

use artisan_catalog::{NativeModelCatalog, NativeModelPolicy, NativePermissionOption};
use artisan_domain::{
    ApprovalMode, ClaudeEffort, ClaudePermissionMode, ClaudeSelection, CodexModelContextWindow,
    CodexReasoningEffort, CodexSelection, CodexServiceTier, CursorPermissionMode,
    CursorReasoningEffort, CursorSelection, CursorSpeed, EngineAgentId, EngineModelId,
    EnginePermissionPolicy, EngineProfileId, EngineRunConfig, EngineSelection, FilesystemAccess,
    GrokPermissionMode, GrokReasoningEffort, GrokSelection, NetworkAccess, PermissionId,
    WebSearchAccess,
};

/// Builds the durable selection for the four fixture-proven native engines.
///
/// Every arm mirrors the backend TypeScript resolver
/// (`modules/backend/src/orchestration/session-policy.ts`): the canonical
/// permission traits travel into an [`EnginePermissionPolicy`], the native
/// permission spelling travels into the engine's own permission-mode option,
/// and effort/speed/window choices travel only where the durable selection
/// has a field for them. A validated choice the selection cannot represent
/// is an honest error, never a silent downgrade — except where the resolver
/// itself drops the axis (neutral standard speed), which is documented at
/// the call site.
#[expect(
    clippy::too_many_lines,
    reason = "one resolver per engine keeps each capability-to-selection mapping reviewable beside its engine's option vocabulary"
)]
pub(super) fn native_selection_config(
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
        _ => Err("This engine is not available in the native app yet"),
    }
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
        .map(EngineSelection::permission);
    (
        permission.map_or(
            NetworkAccess::Disabled,
            artisan_domain::EnginePermissionPolicy::network,
        ),
        permission.map_or(
            WebSearchAccess::Disabled,
            artisan_domain::EnginePermissionPolicy::web_search,
        ),
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
