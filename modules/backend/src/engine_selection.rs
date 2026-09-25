//! Resolves a user's model selection into the engine configuration the Forge
//! runs.
//!
//! The Editor names a model and its options by catalog identity
//! ([`CatalogSelection`]); the Forge looks them up in the catalog it serves
//! (with its account readiness applied), builds the durable
//! [`EngineRunConfig`], and refuses what it cannot build with a reason the
//! Editor shows as it is. This is the former Editor `composer_model_config`
//! (`config_for_policy`, `with_default_native_profile`, and the run-choice
//! validation), which now runs only here.

#![forbid(unsafe_code)]

use artisan_catalog::{
    NativeContextSelection, NativeModelCatalog, NativeModelPolicy, NativeOptionValue,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CatalogOptionId, CatalogSelection, CountLimit, EngineAgentId,
    EngineModelId, EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig,
    EngineRuntimeControls, EngineRuntimeControlsInput, EngineSelection, EngineVariantId,
    FilesystemAccess, FiniteMillis, NetworkAccess, OpenCode2Selection, PermissionId,
    WebSearchAccess,
};

mod manual;
mod native;

pub(crate) use manual::build_manual_config;

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

/// Builds the catalog policy a selection names, looking every identity up in
/// `catalog`.
///
/// The model's preview supplies what the selection does not name only
/// through the catalog itself; each named option must exist on the model (or
/// its harness, for permissions), and an absent option stays absent, exactly
/// as the picker showed it. Native choices without a profile get the default
/// profile.
///
/// # Errors
///
/// Returns a presentation-ready reason when the model or an option is not in
/// the catalog.
pub(crate) fn policy_from_selection(
    catalog: &NativeModelCatalog,
    selection: &CatalogSelection,
) -> Result<NativeModelPolicy, &'static str> {
    let unavailable = "This model is not in the host model catalog";
    let model = catalog
        .manifest
        .model(selection.model_id.as_str())
        .ok_or(unavailable)?;
    let harness = catalog
        .manifest
        .harness(&model.harness)
        .ok_or(unavailable)?;
    let mut policy = catalog
        .preview_policy_for_model(&model.id)
        .map_err(|_| unavailable)?;
    policy.profile_id = selection
        .profile_id
        .as_ref()
        .map(|profile| profile.as_str().to_owned());
    policy.reasoning_effort = option(selection.reasoning_effort.as_ref(), |id| {
        match &model.capabilities.thinking {
            artisan_catalog::NativeThinkingCapability::Supported { options, .. } => options
                .iter()
                .find(|option| option.id == id)
                .map(|option| NativeOptionValue {
                    id: option.id.clone(),
                    native_value: option.native_value.clone(),
                }),
            _ => None,
        }
    })?;
    policy.speed = option(selection.speed.as_ref(), |id| {
        model
            .capabilities
            .speed_options
            .iter()
            .find(|option| option.id == id && option.disabled.is_none())
            .map(|option| NativeOptionValue {
                id: option.id.clone(),
                native_value: option.native_value.clone(),
            })
    })?;
    policy.context_window = option(selection.context_window.as_ref(), |id| {
        model
            .capabilities
            .context_window
            .as_ref()?
            .options
            .iter()
            .find(|option| option.id == id)
            .map(|option| NativeContextSelection {
                id: option.id.clone(),
                native_suffix: option.native_suffix.clone(),
                native_config: option.native_config.clone(),
            })
    })?;
    policy.permission = option(selection.permission.as_ref(), |id| {
        harness
            .permissions
            .options
            .iter()
            .find(|option| option.id == id)
            .map(|option| NativeOptionValue {
                id: option.id.clone(),
                native_value: option.native_value.clone(),
            })
    })?;
    Ok(with_default_native_profile(&policy))
}

/// Looks one named option up; an unnamed option stays absent.
fn option<T>(
    id: Option<&CatalogOptionId>,
    find: impl FnOnce(&str) -> Option<T>,
) -> Result<Option<T>, &'static str> {
    id.map(|id| {
        find(id.as_str()).ok_or("This model option is no longer offered by the host model catalog")
    })
    .transpose()
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
            native::native_selection_config(catalog, policy, previous)?
        }
        _ => opencode2_selection_config(policy, previous)?,
    };
    let runtime = match previous {
        Some(config) => config.runtime(),
        None => default_runtime()?,
    };
    Ok(EngineRunConfig::new(selection, runtime))
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
#[path = "engine_selection/tests.rs"]
mod tests;
