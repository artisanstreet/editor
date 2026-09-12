//! Converts a validated picker choice into the existing durable engine configuration.
use artisan_catalog::{
    NativeModelCatalog, NativeModelDefinition, NativeModelPolicy, NativeOptionValue,
    NativePermissionOption,
};
use artisan_domain::{EngineRunConfig, EngineSelection, EngineProfileId, CodexReasoningEffort, CodexServiceTier, EngineModelId, CodexSelection, ClaudePermissionMode, ClaudeEffort, ClaudeSelection, GrokReasoningEffort, GrokSelection, CursorReasoningEffort, CursorSpeed, CursorSelection, EngineRouteId, EngineVariantId, ApprovalMode, FilesystemAccess, NetworkAccess, WebSearchAccess, EnginePermissionPolicy, PermissionId, EngineAgentId, OpenCode2Selection, GrokPermissionMode, CursorPermissionMode, CodexModelContextWindow, EngineRuntimeControls, FiniteMillis, ByteLimit, CountLimit, EngineRuntimeControlsInput};

/// Default profile identity persisted for native engine selections that
/// carry no explicit profile.
///
/// The Codex/Claude/Grok/Cursor launch authorities resolve their
/// installed executables without consulting the managed `OpenCode` profile
/// registry, so an unconfigured thread persists a native selection under
/// this supported default instead of blocking on registry state. `OpenCode`
/// 2 selections keep requiring an explicit managed profile.
pub(crate) const NATIVE_DEFAULT_PROFILE_ID: &str = "default";

/// Engines whose selections may use [`NATIVE_DEFAULT_PROFILE_ID`].
const NATIVE_DEFAULT_PROFILE_ENGINES: [&str; 4] = ["codex", "claude", "grok", "cursor"];

/// Attaches the default profile to a native choice without one.
///
/// Policies that already name a profile and every `OpenCode` 2 policy are
/// returned unchanged.
pub(crate) fn with_default_native_profile(policy: &NativeModelPolicy) -> NativeModelPolicy {
    if policy.profile_id.is_some() {
        return policy.clone();
    }
    if !NATIVE_DEFAULT_PROFILE_ENGINES.contains(&policy.engine_id.as_str()) {
        return policy.clone();
    }
    let mut policy = policy.clone();
    policy.profile_id = Some(NATIVE_DEFAULT_PROFILE_ID.to_owned());
    policy
}

/// A displayed choice must never silently run the previous saved model.
pub(crate) fn validate_run_choice(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
    saved: Option<&EngineRunConfig>,
) -> Result<(), &'static str> {
    catalog.admit_policy(policy).map_err(
        |_| "This model is unavailable in the runtime catalog right now. Your draft is preserved; open Settings → Engines to review it, or retry.",
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
        "codex" | "claude" | "grok" | "cursor" => {
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
        _ => Err("This engine is not available in the native app yet"),
    }
}

/// Projects a saved native engine selection back onto a displayable catalog
/// policy.
///
/// This is the reverse of [`config_for_policy`] for the four native engines:
/// given the durable selection inside a saved [`EngineRunConfig`], it
/// recovers the exact catalog model plus the reasoning/speed/context/
/// permission option values that rebuild it, so a configured thread restored
/// from storage (reload, switch-back) displays and validates its saved
/// Codex/Claude/Grok/Cursor model instead of drifting to no
/// selection. Every axis is recovered from live catalog options by native
/// value; the candidate is then verified by rebuilding through
/// [`config_for_policy`] against the saved configuration, so only an exact
/// round-trip is returned — never a silent downgrade. `OpenCode` 2 keeps its
/// existing registry-shaped projection outside this function.
///
/// # Errors
///
/// Returns a static reason when the selection names no model, no catalog
/// model still carries it, a saved axis value has no catalog option, or the
/// rebuilt configuration differs from the saved one (for example after a
/// manifest change removed the original option).
pub(crate) fn policy_for_selection(
    catalog: &NativeModelCatalog,
    saved: &EngineRunConfig,
) -> Result<NativeModelPolicy, &'static str> {
    match saved.selection() {
        EngineSelection::OpenCode2(_) => Err("OpenCode selections keep their registry projection"),
        EngineSelection::Codex(selection) => policy_for_codex(catalog, saved, selection),
        EngineSelection::Claude(selection) => policy_for_claude(catalog, saved, selection),
        EngineSelection::Grok(selection) => policy_for_grok(catalog, saved, selection),
        EngineSelection::Cursor(selection) => policy_for_cursor(catalog, saved, selection),
    }
}

/// Verifies one projected policy rebuilds exactly the saved configuration.
///
/// The saved configuration travels as the previous run config so runtime
/// budgets and inherited network/web grants match the original save; only a
/// byte-identical rebuild is accepted.
fn verified_policy(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
    saved: &EngineRunConfig,
) -> Result<NativeModelPolicy, &'static str> {
    config_for_policy(catalog, policy, Some(saved))
        .ok()
        .filter(|rebuilt| rebuilt == saved)
        .map(|_| policy.clone())
        .ok_or("Saved model configuration is no longer available")
}

/// Finds one thinking option by its native value.
fn thinking_option(model: &NativeModelDefinition, native_value: &str) -> Option<NativeOptionValue> {
    match &model.capabilities.thinking {
        artisan_catalog::NativeThinkingCapability::Supported { options, .. } => options
            .iter()
            .find(|option| option.native_value == native_value)
            .map(|option| NativeOptionValue {
                id: option.id.clone(),
                native_value: option.native_value.clone(),
            }),
        _ => None,
    }
}

/// Finds one enabled speed option by its native value.
fn speed_option(model: &NativeModelDefinition, native_value: &str) -> Option<NativeOptionValue> {
    model
        .capabilities
        .speed_options
        .iter()
        .find(|option| option.native_value == native_value && option.disabled.is_none())
        .map(|option| NativeOptionValue {
            id: option.id.clone(),
            native_value: option.native_value.clone(),
        })
}

/// Finds one context option carrying an exact native window value.
fn window_option(
    model: &NativeModelDefinition,
    model_context_window: u64,
) -> Option<artisan_catalog::NativeContextSelection> {
    model
        .capabilities
        .context_window
        .as_ref()
        .and_then(|capability| {
            capability.options.iter().find(|option| {
                option
                    .native_config
                    .as_ref()
                    .is_some_and(|config| config.model_context_window == model_context_window)
            })
        })
        .map(|option| artisan_catalog::NativeContextSelection {
            id: option.id.clone(),
            native_suffix: option.native_suffix.clone(),
            native_config: option.native_config.clone(),
        })
}

/// Recovers the permission option by exact rebuild.
///
/// The durable selection carries only canonical traits plus an inherited
/// grant, never the catalog option identity — so every harness option is
/// tried and the first one whose rebuilt configuration equals the saved one
/// wins. A manifest change that removed the original option is an honest
/// error, never a neighboring permission.
fn permission_by_rebuild(
    catalog: &NativeModelCatalog,
    engine_id: &str,
    policy: &mut NativeModelPolicy,
    saved: &EngineRunConfig,
) -> Result<NativeModelPolicy, &'static str> {
    let harness = catalog
        .manifest
        .harness(engine_id)
        .ok_or("Saved model configuration is no longer available")?;
    let options = harness.permissions.options.clone();
    for option in &options {
        policy.permission = Some(NativeOptionValue {
            id: option.id.clone(),
            native_value: option.native_value.clone(),
        });
        if let Ok(verified) = verified_policy(catalog, policy, saved) {
            return Ok(verified);
        }
    }
    Err("Saved permission is no longer available")
}

/// Projects a saved Codex selection onto its catalog policy.
fn policy_for_codex(
    catalog: &NativeModelCatalog,
    saved: &EngineRunConfig,
    selection: &CodexSelection,
) -> Result<NativeModelPolicy, &'static str> {
    let wanted = selection
        .model_id()
        .map(|model| model.as_str().to_owned())
        .ok_or("Saved Codex selection names no model")?;
    let effort = selection
        .reasoning_effort()
        .map(|effort| effort.as_str().to_owned());
    let speed = selection
        .service_tier()
        .map(|tier| tier.as_str().to_owned());
    let window = selection.model_context_window().map(artisan_domain::CodexModelContextWindow::get);
    let profile = selection.profile_id().as_str().to_owned();
    for model in catalog
        .manifest
        .models
        .iter()
        .filter(|model| model.harness == "codex" && model.native_model_id == wanted)
    {
        let Ok(mut policy) = catalog.preview_policy_for_model(&model.id) else {
            continue;
        };
        policy.profile_id = Some(profile.clone());
        policy.reasoning_effort = match effort.as_deref() {
            None => None,
            Some(wanted) => match thinking_option(model, wanted) {
                Some(option) => Some(option),
                None => continue,
            },
        };
        policy.speed = match speed.as_deref() {
            None => None,
            Some(wanted) => match speed_option(model, wanted) {
                Some(option) => Some(option),
                None => continue,
            },
        };
        // A saved base window carries no override, so the projection keeps
        // the preview default (the canonical `standard` identity) instead
        // of clearing the axis: both rebuild the same saved configuration.
        if let Some(wanted) = window {
            policy.context_window = match window_option(model, wanted) {
                Some(option) => Some(option),
                None => continue,
            };
        }
        if let Ok(verified) = permission_by_rebuild(catalog, "codex", &mut policy, saved) {
            return Ok(verified);
        }
    }
    Err("Saved Codex model is no longer available")
}

/// Projects a saved Claude selection onto its catalog policy.
///
/// The durable Claude model composes the base id with its context suffix
/// (`ComposeNativeModelId`), so the manifest row is found by splitting the
/// saved id back into its base plus a config-less option suffix.
fn policy_for_claude(
    catalog: &NativeModelCatalog,
    saved: &EngineRunConfig,
    selection: &ClaudeSelection,
) -> Result<NativeModelPolicy, &'static str> {
    let wanted = selection
        .model_id()
        .map(|model| model.as_str().to_owned())
        .ok_or("Saved Claude selection names no model")?;
    let effort = selection.effort().map(|effort| effort.as_str().to_owned());
    let profile = selection.profile_id().as_str().to_owned();
    for model in catalog
        .manifest
        .models
        .iter()
        .filter(|model| model.harness == "claude")
    {
        let Some(context_window) = claude_context_for_model(model, &wanted) else {
            continue;
        };
        let Ok(mut policy) = catalog.preview_policy_for_model(&model.id) else {
            continue;
        };
        policy.profile_id = Some(profile.clone());
        policy.reasoning_effort = match effort.as_deref() {
            None => None,
            Some(wanted) => match thinking_option(model, wanted) {
                Some(option) => Some(option),
                None => continue,
            },
        };
        // The resolver carries no Claude speed axis: an absent or neutral
        // choice rebuilds identically, so the projection stays canonical.
        policy.speed = None;
        policy.context_window = context_window;
        if let Ok(verified) = permission_by_rebuild(catalog, "claude", &mut policy, saved) {
            return Ok(verified);
        }
    }
    Err("Saved Claude model is no longer available")
}

/// Splits a saved Claude model id into its manifest row plus context option.
///
/// An exact base match prefers the empty-suffix config-less option and falls
/// back to no selection (both rebuild to the bare id); otherwise the id must
/// end in a real config-less option suffix. A missing row or suffix is
/// `None` so the caller keeps scanning later rows.
#[expect(
    clippy::option_option,
    reason = "the outer None means the saved id did not match this manifest row; the inner None is the config-less option choice, and both are load-bearing"
)]
fn claude_context_for_model(
    model: &NativeModelDefinition,
    wanted: &str,
) -> Option<Option<artisan_catalog::NativeContextSelection>> {
    let capability = model.capabilities.context_window.as_ref()?;
    if wanted == model.native_model_id {
        return Some(
            capability
                .options
                .iter()
                .find(|option| option.native_suffix.is_empty() && option.native_config.is_none())
                .map(|option| artisan_catalog::NativeContextSelection {
                    id: option.id.clone(),
                    native_suffix: option.native_suffix.clone(),
                    native_config: option.native_config.clone(),
                }),
        );
    }
    let suffix = wanted.strip_prefix(model.native_model_id.as_str())?;
    if suffix.is_empty() {
        return None;
    }
    capability
        .options
        .iter()
        .find(|option| option.native_suffix == suffix && option.native_config.is_none())
        .map(|option| {
            Some(artisan_catalog::NativeContextSelection {
                id: option.id.clone(),
                native_suffix: option.native_suffix.clone(),
                native_config: option.native_config.clone(),
            })
        })
}

/// Projects a saved Grok selection onto its catalog policy.
fn policy_for_grok(
    catalog: &NativeModelCatalog,
    saved: &EngineRunConfig,
    selection: &GrokSelection,
) -> Result<NativeModelPolicy, &'static str> {
    let wanted = selection
        .model_id()
        .map(|model| model.as_str().to_owned())
        .ok_or("Saved Grok selection names no model")?;
    let effort = selection
        .reasoning_effort()
        .map(|effort| effort.as_str().to_owned());
    let profile = selection.profile_id().as_str().to_owned();
    for model in catalog
        .manifest
        .models
        .iter()
        .filter(|model| model.harness == "grok" && model.native_model_id == wanted)
    {
        let Ok(mut policy) = catalog.preview_policy_for_model(&model.id) else {
            continue;
        };
        policy.profile_id = Some(profile.clone());
        policy.reasoning_effort = match effort.as_deref() {
            None => None,
            Some(wanted) => match thinking_option(model, wanted) {
                Some(option) => Some(option),
                None => continue,
            },
        };
        // The resolver sends no Grok speed or context axis: the preview
        // values display while the rebuild ignores them.
        if let Ok(verified) = permission_by_rebuild(catalog, "grok", &mut policy, saved) {
            return Ok(verified);
        }
    }
    Err("Saved Grok model is no longer available")
}

/// Projects a saved Cursor selection onto its catalog policy.
fn policy_for_cursor(
    catalog: &NativeModelCatalog,
    saved: &EngineRunConfig,
    selection: &CursorSelection,
) -> Result<NativeModelPolicy, &'static str> {
    let wanted = selection
        .model_id()
        .map(|model| model.as_str().to_owned())
        .ok_or("Saved Cursor selection names no model")?;
    let effort = selection
        .reasoning_effort()
        .map(|effort| effort.as_str().to_owned());
    let speed = selection.speed().map(|speed| speed.as_str().to_owned());
    let profile = selection.profile_id().as_str().to_owned();
    for model in catalog
        .manifest
        .models
        .iter()
        .filter(|model| model.harness == "cursor" && model.native_model_id == wanted)
    {
        let Ok(mut policy) = catalog.preview_policy_for_model(&model.id) else {
            continue;
        };
        policy.profile_id = Some(profile.clone());
        policy.reasoning_effort = match effort.as_deref() {
            None => None,
            Some(wanted) => match thinking_option(model, wanted) {
                Some(option) => Some(option),
                None => continue,
            },
        };
        policy.speed = match speed.as_deref() {
            None => None,
            Some(wanted) => match speed_option(model, wanted) {
                Some(option) => Some(option),
                None => continue,
            },
        };
        // The resolver carries no Cursor context axis: the preview value
        // displays while the rebuild ignores it.
        if let Ok(verified) = permission_by_rebuild(catalog, "cursor", &mut policy, saved) {
            return Ok(verified);
        }
    }
    Err("Saved Cursor model is no longer available")
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
        .map(EngineSelection::permission);
    (
        permission.map_or(NetworkAccess::Disabled, artisan_domain::EnginePermissionPolicy::network),
        permission.map_or(WebSearchAccess::Disabled, artisan_domain::EnginePermissionPolicy::web_search),
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
    use artisan_catalog::{NativeContextConfig, NativeContextSelection, NativeOptionValue};

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
        for engine_id in ["codex", "claude", "grok", "cursor"] {
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
        assert_eq!(selection.reasoning_effort().unwrap().as_str(), "low");
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
            selection.model_context_window().map(artisan_domain::CodexModelContextWindow::get),
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

    #[test]
    fn native_choices_without_a_profile_fall_back_to_default() {
        let catalog = runnable_catalog();
        for (engine_id, model_id) in [
            ("codex", "codex-sol"),
            ("claude", "claude-fable"),
            ("grok", "grok-4-6"),
            ("cursor", "cursor-composer-2-5"),
        ] {
            let policy = catalog.selection_policy_for_model(model_id).unwrap();
            assert_eq!(policy.engine_id, engine_id);
            assert_eq!(policy.profile_id, None);
            let defaulted = with_default_native_profile(&policy);
            assert_eq!(defaulted.profile_id.as_deref(), Some("default"));
            // The defaulted choice builds the same runnable selection the
            // explicit profile spelling produces.
            let mut explicit = policy.clone();
            explicit.profile_id = Some("default".to_owned());
            assert_eq!(
                config_for_policy(&catalog, &defaulted, None).unwrap(),
                config_for_policy(&catalog, &explicit, None).unwrap()
            );
        }
    }

    #[test]
    fn default_profile_leaves_managed_and_explicit_profiles_alone() {
        let catalog = runnable_catalog();
        let policy = catalog.selection_policy_for_model("codex-sol").unwrap();
        let mut managed = policy.clone();
        managed.engine_id = "opencode2".to_owned();
        assert_eq!(with_default_native_profile(&managed).profile_id, None);
        let mut explicit = with_default_native_profile(&policy);
        explicit.profile_id = Some("work".to_owned());
        assert_eq!(
            with_default_native_profile(&explicit).profile_id.as_deref(),
            Some("work")
        );
    }

    /// Projects one default choice back and requires the exact durable
    /// round-trip: the projected policy must rebuild byte-identically and
    /// keep the displayed model/profile identity.
    fn assert_selection_round_trip(catalog: &NativeModelCatalog, model_id: &str) {
        let policy = profiled_policy(catalog, model_id);
        let config = config_for_policy(catalog, &policy, None).unwrap();
        let projected = policy_for_selection(catalog, &config).unwrap();
        assert_eq!(
            config_for_policy(catalog, &projected, Some(&config)).unwrap(),
            config,
            "{model_id} projection must rebuild its saved configuration"
        );
        assert_eq!(projected.engine_id, policy.engine_id);
        assert_eq!(projected.model_id, policy.model_id);
        assert_eq!(projected.native_model_id, policy.native_model_id);
        assert_eq!(projected.native_selection, policy.native_selection);
        assert_eq!(projected.profile_id, policy.profile_id);
        assert!(validate_run_choice(catalog, &projected, Some(&config)).is_ok());
    }

    #[test]
    fn saved_codex_selection_projects_back_exactly() {
        let catalog = runnable_catalog();
        assert_selection_round_trip(&catalog, "codex-sol");
        let policy = profiled_policy(&catalog, "codex-sol");
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let projected = policy_for_selection(&catalog, &config).unwrap();
        // Default Codex axes survive with their exact option identities.
        assert_eq!(projected.reasoning_effort, policy.reasoning_effort);
        assert_eq!(projected.speed, policy.speed);
        assert_eq!(projected.context_window, policy.context_window);
        assert_eq!(projected.permission, policy.permission);
    }

    #[test]
    fn saved_claude_selection_recovers_its_window_suffix() {
        let catalog = runnable_catalog();
        assert_selection_round_trip(&catalog, "claude-fable");
        let policy = profiled_policy(&catalog, "claude-fable");
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        let EngineSelection::Claude(selection) = config.selection() else {
            panic!("expected a Claude selection");
        };
        // The durable id composes the base with the extended suffix.
        assert_eq!(selection.model_id().unwrap().as_str(), "claude-fable-5[1m]");
        let projected = policy_for_selection(&catalog, &config).unwrap();
        assert_eq!(projected.context_window, policy.context_window);
        assert_eq!(projected.permission, policy.permission);
    }

    #[test]
    fn saved_grok_and_cursor_selections_project_back_exactly() {
        let catalog = runnable_catalog();
        assert_selection_round_trip(&catalog, "grok-4-6");
        assert_selection_round_trip(&catalog, "cursor-composer-2-5");
    }

    #[test]
    fn projection_rejects_unknown_models_and_keeps_opencode2_outside() {
        let catalog = runnable_catalog();
        let policy = profiled_policy(&catalog, "codex-sol");
        let config = config_for_policy(&catalog, &policy, None).unwrap();
        // A manifest that lost the saved row cannot project it.
        let mut without_row = catalog.clone();
        without_row
            .manifest
            .models
            .retain(|model| model.id != "codex-sol");
        assert_eq!(
            policy_for_selection(&without_row, &config).unwrap_err(),
            "Saved Codex model is no longer available"
        );
        // `OpenCode` selections keep their registry projection outside this
        // boundary.
        let managed = EngineRunConfig::new(
            EngineSelection::OpenCode2(OpenCode2Selection::new(
                EngineProfileId::parse("default").expect("profile"),
                EngineModelId::parse("model-test").expect("model"),
                EngineRouteId::parse("route-test").expect("route"),
                None,
                EnginePermissionPolicy::new(
                    PermissionId::parse("autonomous").expect("permission"),
                    EngineAgentId::parse("artisan-v1-autonomous-offline-no-web").expect("agent"),
                    ApprovalMode::OnRequest,
                    FilesystemAccess::Workspace,
                    NetworkAccess::Disabled,
                    WebSearchAccess::Disabled,
                ),
            )),
            default_runtime().expect("runtime"),
        );
        assert_eq!(
            policy_for_selection(&catalog, &managed).unwrap_err(),
            "OpenCode selections keep their registry projection"
        );
    }
}
