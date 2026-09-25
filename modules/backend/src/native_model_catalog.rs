//! Converts owner-scoped and host-discovered models into the shared catalog.
//! Model availability and capabilities come from discovery; local descriptors
//! define supported harness permissions without inventing models.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::collections::HashSet;

use artisan_catalog::{
    NativeCatalogRuntime, NativeCatalogScope, NativeContextWindowCapability, NativeDisabledModel,
    NativeModelCapabilities, NativeModelCatalog, NativeModelCost, NativeModelDefinition,
    NativeModelManifest, NativeModelProvider, NativeModelRoute, NativeModelRouteGroup,
    NativeModelRouteStatus, NativeModelRouting, NativeModelSelection, NativeThinkingCapability,
    NativeThinkingOption,
};
use artisan_domain::{
    EngineModelId, EngineProfileId, EngineRouteId, EngineVariantId, ModelFavoritesSnapshot,
};
use thiserror::Error;

use crate::engine_owner::catalog::{
    CatalogAvailability, CatalogModel, CatalogResult, CatalogRoute,
};
use crate::engine_owner::consts::{
    CLAUDE_ENGINE_ID, CODEX_ENGINE_ID, CURSOR_ENGINE_ID, GROK_ENGINE_ID, OPENCODE2_ENGINE_ID,
};
use crate::model_discovery::DiscoveredModel;

/// Harness identifiers Forge can execute once their fixture-proven runtimes
/// are registered in this process.
///
/// `runnable` means a supported harness with an in-tree runtime, not an
/// installed binary. Executable resolution and the readiness handshake stay
/// the live gate in each per-engine executor and probe path: a missing CLI
/// still yields unavailable-with-reason at runtime, never a false ready.
const RUNNABLE_ENGINE_IDS: [&str; 5] = [
    OPENCODE2_ENGINE_ID,
    CODEX_ENGINE_ID,
    CLAUDE_ENGINE_ID,
    GROK_ENGINE_ID,
    CURSOR_ENGINE_ID,
];

/// Payload-free failure while combining the typed runtime result with the
/// shared catalog manifest.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub(crate) enum NativeModelCatalogBridgeError {
    /// The result came from an engine other than the supported native engine.
    #[error("native catalog result is from an unsupported engine")]
    UnsupportedEngine,
    /// The shipped harness descriptors could not be decoded.
    #[error("shipped harness descriptors are invalid")]
    HarnessManifest,
    /// A runtime route did not satisfy the `OpenCode2` route contract.
    #[error("native catalog route is invalid")]
    InvalidRoute,
    /// A normalized row did not retain its exact native identity.
    #[error("native catalog model identity is invalid")]
    InvalidModelIdentity,
    /// A runtime row or route repeated an identity.
    #[error("native catalog contains a duplicate identity")]
    DuplicateIdentity,
    /// The resulting shared snapshot failed its bounded validation.
    #[error("native catalog snapshot is invalid")]
    InvalidCatalog,
}

/// Converts the exact models and routes reported by an owner-scoped engine.
pub(crate) fn from_catalog_result(
    result: CatalogResult,
    favorites: &ModelFavoritesSnapshot,
) -> Result<NativeModelCatalog, NativeModelCatalogBridgeError> {
    if result.engine_id != OPENCODE2_ENGINE_ID {
        return Err(NativeModelCatalogBridgeError::UnsupportedEngine);
    }
    if EngineProfileId::parse(result.scope.profile_id()).is_err() {
        return Err(NativeModelCatalogBridgeError::InvalidCatalog);
    }

    let mut manifest = NativeModelCatalog::harnesses_only()
        .map_err(|_| NativeModelCatalogBridgeError::HarnessManifest)?
        .manifest;

    let mut route_ids = HashSet::with_capacity(result.routes.len());
    for route in &result.routes {
        if route.engine_id != OPENCODE2_ENGINE_ID
            || EngineRouteId::parse(route.id.as_str()).is_err()
        {
            return Err(NativeModelCatalogBridgeError::InvalidRoute);
        }
        if !route_ids.insert(route.id.clone()) {
            return Err(NativeModelCatalogBridgeError::DuplicateIdentity);
        }
        ensure_provider(&mut manifest, route);
    }

    let mut model_ids = manifest
        .models
        .iter()
        .map(|model| model.id.clone())
        .collect::<HashSet<_>>();
    let mut dynamic_models = Vec::with_capacity(result.models.len());
    for model in &result.models {
        validate_model_identity(model, &route_ids)?;
        if !model_ids.insert(model.catalog_id.clone()) {
            return Err(NativeModelCatalogBridgeError::DuplicateIdentity);
        }
        dynamic_models.push(convert_model(model));
    }
    manifest.models.extend(dynamic_models);

    let runtime = NativeCatalogRuntime {
        catalog_revision: Some(result.catalog_revision),
        runnable_harness_ids: RUNNABLE_ENGINE_IDS
            .iter()
            .map(|harness| (*harness).to_owned())
            .collect(),
        routes: result.routes.into_iter().map(convert_route).collect(),
        default_model_id: None,
        favorite_ids: favorites
            .model_ids()
            .iter()
            .map(|model_id| model_id.as_str().to_owned())
            .collect(),
        model_defaults: Vec::new(),
        scope: Some(NativeCatalogScope {
            profile_id: result.scope.profile_id().to_owned(),
            working_directory: result.scope.working_directory().to_owned(),
            workspace_trust: result.scope.workspace_trust().to_owned(),
        }),
    };

    let catalog = NativeModelCatalog::from_manifest(manifest, runtime);
    artisan_catalog::wire::encode_catalog(&catalog)
        .map_err(|_| NativeModelCatalogBridgeError::InvalidCatalog)?;
    Ok(catalog)
}

fn ensure_provider(manifest: &mut NativeModelManifest, route: &CatalogRoute) {
    if manifest.provider(&route.id).is_none() {
        manifest.providers.push(NativeModelProvider {
            id: route.id.clone(),
            label: route.label.clone(),
        });
    }
}

fn validate_model_identity(
    model: &CatalogModel,
    route_ids: &HashSet<String>,
) -> Result<(), NativeModelCatalogBridgeError> {
    if model.native_model_id != model.model_id
        || model.provider_route_id != model.native_selection.provider_route_id
        || model.model_id != model.native_selection.model_id
        || model.variant_id != model.native_selection.variant_id
        || !route_ids.contains(&model.provider_route_id)
    {
        return Err(NativeModelCatalogBridgeError::InvalidModelIdentity);
    }
    if EngineModelId::parse(model.native_selection.model_id.as_str()).is_err()
        || EngineRouteId::parse(model.native_selection.provider_route_id.as_str()).is_err()
        || model
            .native_selection
            .variant_id
            .as_deref()
            .is_some_and(|variant_id| EngineVariantId::parse(variant_id).is_err())
    {
        return Err(NativeModelCatalogBridgeError::InvalidModelIdentity);
    }

    let expected_id = artisan_catalog::wire::opencode2_catalog_id(
        &model.native_selection.model_id,
        &model.native_selection.provider_route_id,
        model.native_selection.variant_id.as_deref(),
    )
    .map_err(|_| NativeModelCatalogBridgeError::InvalidModelIdentity)?;
    if model.catalog_id != expected_id {
        return Err(NativeModelCatalogBridgeError::InvalidModelIdentity);
    }
    Ok(())
}

fn convert_model(model: &CatalogModel) -> NativeModelDefinition {
    NativeModelDefinition {
        id: model.catalog_id.clone(),
        name: model.name.clone(),
        native_model_id: model.native_model_id.clone(),
        description: None,
        harness: OPENCODE2_ENGINE_ID.to_owned(),
        provider: model.provider_route_id.clone(),
        routing: NativeModelRouting::ProviderRoute {
            provider_route_id: model.provider_route_id.clone(),
        },
        native_selection: Some(NativeModelSelection {
            model_id: model.native_selection.model_id.clone(),
            provider_route_id: model.native_selection.provider_route_id.clone(),
            variant_id: model.native_selection.variant_id.clone(),
        }),
        status: "dynamic".to_owned(),
        upstream_model_id: Some(model.upstream_model_id.clone()),
        metadata_confidence: Some(model.metadata_confidence.to_owned()),
        cost: model.cost.map(|cost| NativeModelCost {
            input_usd_per_million: Some(cost.input_usd_per_million),
            output_usd_per_million: Some(cost.output_usd_per_million),
        }),
        disabled: model
            .availability
            .unavailable_reason()
            .map(|reason| NativeDisabledModel {
                reason: reason.to_owned(),
            }),
        capabilities: NativeModelCapabilities {
            context_window_tokens: Some(model.capabilities.context_window_tokens),
            context_window: None,
            image_input: model.capabilities.image_input,
            local_tools: model.capabilities.tools,
            mcp: false,
            output_tokens: Some(model.capabilities.output_tokens),
            reasoning_display: None,
            speed_options: Vec::new(),
            thinking: NativeThinkingCapability::Unavailable,
            web_search: false,
        },
    }
}

fn convert_route(route: CatalogRoute) -> NativeModelRoute {
    let (status, unavailable_reason) = match route.availability {
        CatalogAvailability::Available => (NativeModelRouteStatus::Available, None),
        CatalogAvailability::Unavailable { reason } => {
            (NativeModelRouteStatus::Unavailable, Some(reason.to_owned()))
        }
    };
    NativeModelRoute {
        engine_id: route.engine_id.to_owned(),
        group: NativeModelRouteGroup {
            id: route.group.id,
            label: route.group.label,
            order: route.group.order,
            show_route_labels: route.group.show_route_labels,
        },
        id: route.id,
        label: route.label,
        status,
        unavailable_reason,
    }
}

/// Combines the typed `OpenCode2` result with best-effort live discovery from
/// every other engine.
///
/// Adds host discovery to scoped runtime routes; absent models stay absent.
pub(crate) fn from_catalog_result_with_discovery(
    result: CatalogResult,
    discovery: &crate::model_discovery::DiscoveryBundle,
    favorites: &ModelFavoritesSnapshot,
) -> Result<NativeModelCatalog, NativeModelCatalogBridgeError> {
    let catalog = from_catalog_result(result, favorites)?;
    if discovery.models.is_empty() && discovery.missing_engines.is_empty() {
        return Ok(catalog);
    }
    let mut runtime = catalog.runtime();
    let mut manifest = catalog.manifest;
    let base_revision = runtime.catalog_revision.clone().unwrap_or_default();
    // Runtime rows win; CLI discovery fills only identities the scoped engine
    // result did not return (including every route when that result was empty).
    for route in apply_discovery(&mut manifest, discovery, true) {
        if !runtime
            .routes
            .iter()
            .any(|existing| existing.id == route.id && existing.engine_id == route.engine_id)
        {
            runtime.routes.push(route);
        }
    }
    runtime.catalog_revision = Some(format!(
        "{base_revision}+discovery-{:016x}",
        discovery_revision_hash(discovery)
    ));
    let next = NativeModelCatalog::from_manifest(manifest, runtime);
    artisan_catalog::wire::encode_catalog(&next)
        .map_err(|_| NativeModelCatalogBridgeError::InvalidCatalog)?;
    Ok(next)
}

/// Builds a scope-free catalog using only host-discovered rows.
pub(crate) fn from_discovery(
    discovery: &crate::model_discovery::DiscoveryBundle,
) -> Result<NativeModelCatalog, NativeModelCatalogBridgeError> {
    let mut manifest = NativeModelCatalog::harnesses_only()
        .map_err(|_| NativeModelCatalogBridgeError::HarnessManifest)?
        .manifest;
    let routes = apply_discovery(&mut manifest, discovery, true);
    let runtime = NativeCatalogRuntime {
        catalog_revision: Some(format!(
            "discovery-{:016x}",
            discovery_revision_hash(discovery)
        )),
        runnable_harness_ids: RUNNABLE_ENGINE_IDS
            .iter()
            .map(|harness| (*harness).to_owned())
            .collect(),
        routes,
        ..NativeCatalogRuntime::default()
    };
    let catalog = NativeModelCatalog::from_manifest(manifest, runtime);
    artisan_catalog::wire::encode_catalog(&catalog)
        .map_err(|_| NativeModelCatalogBridgeError::InvalidCatalog)?;
    Ok(catalog)
}

/// Stable revision contribution for one discovery bundle.
fn discovery_revision_hash(discovery: &crate::model_discovery::DiscoveryBundle) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    // The revision covers capabilities as well as identities: changing a
    // context limit or reasoning option must invalidate old admission policies.
    for byte in format!("{discovery:?}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    hash
}

/// Adds only models actually reported by the host's engine discovery.
fn apply_discovery(
    manifest: &mut NativeModelManifest,
    discovery: &crate::model_discovery::DiscoveryBundle,
    include_opencode2: bool,
) -> Vec<NativeModelRoute> {
    let mut routes = Vec::new();
    for row in discovery.models.iter().filter(|row| !row.hidden) {
        if row.engine_id == OPENCODE2_ENGINE_ID {
            if include_opencode2 {
                apply_opencode2_rows(manifest, &[row], &mut routes);
            }
            continue;
        }
        if manifest.harness(row.engine_id).is_none()
            || manifest.models.iter().any(|model| {
                model.harness == row.engine_id && model.native_model_id == row.native_model_id
            })
        {
            continue;
        }
        let existing = manifest
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        let id = unique_discovered_id(row.engine_id, &row.native_model_id, &existing);
        ensure_discovered_provider(manifest, &row.provider);
        manifest.models.push(discovered_definition(row, id));
    }
    hide_missing_harnesses(manifest, discovery);
    routes
}

/// Hides harnesses whose engine CLI is not installed on this machine.
///
/// Harness descriptors stay fully decodable, so stored selections still
/// resolve; only the picker tab and its rows disappear.
fn hide_missing_harnesses(
    manifest: &mut NativeModelManifest,
    discovery: &crate::model_discovery::DiscoveryBundle,
) {
    if discovery.missing_engines.is_empty() {
        return;
    }
    for harness in &mut manifest.harnesses {
        if discovery
            .missing_engines
            .iter()
            .any(|engine_id| *engine_id == harness.id)
        {
            harness.hidden = true;
        }
    }
}

/// Adds runtime-only `OpenCode2` rows from local CLI discovery.
///
/// Each row is built with the exact `opencode2:` route identity the runtime
/// path uses, so a selection stays runnable end to end. Route metadata is
/// emitted so the picker can group Go/Zen/custom rows.
fn apply_opencode2_rows(
    manifest: &mut NativeModelManifest,
    rows: &[&DiscoveredModel],
    routes: &mut Vec<NativeModelRoute>,
) {
    let mut existing = manifest
        .models
        .iter()
        .map(|model| model.id.clone())
        .collect::<HashSet<_>>();
    for row in rows {
        if row.hidden {
            continue;
        }
        let Ok(id) = artisan_catalog::wire::opencode2_catalog_id(
            &row.native_model_id,
            &row.provider,
            row.variant_id.as_deref(),
        ) else {
            continue;
        };
        if !existing.insert(id.clone()) {
            continue;
        }
        if !routes.iter().any(|route| route.id == row.provider) {
            let route = crate::engine_owner::catalog::route_for(&row.provider);
            if manifest.provider(&route.id).is_none() {
                manifest.providers.push(NativeModelProvider {
                    id: route.id.clone(),
                    label: route.label.clone(),
                });
            }
            routes.push(convert_route(route));
        }
        manifest.models.push(opencode2_definition(row, id));
    }
}

/// Builds one CLI-discovered `OpenCode2` row with exact route identity.
fn opencode2_definition(row: &DiscoveredModel, id: String) -> NativeModelDefinition {
    NativeModelDefinition {
        id,
        name: readable_name(&row.name),
        native_model_id: row.native_model_id.clone(),
        description: row.description.clone(),
        harness: OPENCODE2_ENGINE_ID.to_owned(),
        provider: row.provider.clone(),
        routing: NativeModelRouting::ProviderRoute {
            provider_route_id: row.provider.clone(),
        },
        native_selection: Some(NativeModelSelection {
            model_id: row.native_model_id.clone(),
            provider_route_id: row.provider.clone(),
            variant_id: row.variant_id.clone(),
        }),
        status: "dynamic".to_owned(),
        upstream_model_id: row
            .upstream_model_id
            .clone()
            .or_else(|| Some(row.native_model_id.clone())),
        metadata_confidence: Some(row.metadata_confidence.to_owned()),
        cost: row.cost.map(|(input, output)| NativeModelCost {
            input_usd_per_million: Some(input),
            output_usd_per_million: Some(output),
        }),
        disabled: None,
        capabilities: NativeModelCapabilities {
            context_window_tokens: row.context_window_tokens,
            context_window: None,
            image_input: row.image_input,
            local_tools: row.tools,
            mcp: false,
            output_tokens: row.output_tokens,
            reasoning_display: None,
            speed_options: Vec::new(),
            thinking: NativeThinkingCapability::Unavailable,
            web_search: false,
        },
    }
}

/// Converts an engine-reported display name into picker copy.
///
/// Engines commonly report hyphenated identifiers (`GPT-6-Astra`) where the
/// product uses space-separated names (`GPT 6 Astra`); separator runs and
/// repeated whitespace collapse to one space and the result is trimmed.
fn readable_name(raw: &str) -> String {
    let mut name = String::with_capacity(raw.len());
    let mut pending_space = false;
    for character in raw.chars() {
        if character == '-' || character.is_whitespace() {
            pending_space = !name.is_empty();
            continue;
        }
        if pending_space {
            name.push(' ');
            pending_space = false;
        }
        name.push(character);
    }
    name
}

/// Builds one runtime-only row from discovery plus cloned harness policy.
fn discovered_definition(row: &DiscoveredModel, id: String) -> NativeModelDefinition {
    NativeModelDefinition {
        id,
        name: readable_name(&row.name),
        native_model_id: row.native_model_id.clone(),
        description: row.description.clone(),
        harness: row.engine_id.to_owned(),
        provider: row.provider.clone(),
        routing: NativeModelRouting::Default,
        native_selection: None,
        status: "dynamic".to_owned(),
        upstream_model_id: row
            .upstream_model_id
            .clone()
            .or_else(|| Some(row.native_model_id.clone())),
        metadata_confidence: Some(row.metadata_confidence.to_owned()),
        cost: row.cost.map(|(input, output)| NativeModelCost {
            input_usd_per_million: Some(input),
            output_usd_per_million: Some(output),
        }),
        disabled: None,
        capabilities: NativeModelCapabilities {
            context_window_tokens: row.context_window_tokens,
            context_window: discovered_context_policy(row),
            image_input: row.image_input,
            local_tools: row.tools,
            mcp: row.tools,
            output_tokens: row.output_tokens,
            reasoning_display: None,
            speed_options: discovered_speed_options(row),
            thinking: thinking_from_discovery(&row.thinking),
            web_search: row.web_search,
        },
    }
}

/// Builds context choices from reported limits using the harness override protocol.
fn discovered_context_policy(row: &DiscoveredModel) -> Option<NativeContextWindowCapability> {
    use artisan_catalog::{NativeContextConfig, NativeContextWindowOption};
    let base = row.context_window_tokens?;
    let max = row.max_context_window_tokens.unwrap_or(base);
    let option = |id: &str, tokens: u64, native_config| NativeContextWindowOption {
        advisory: None,
        description: None,
        id: id.to_owned(),
        label: if tokens % 1000 == 0 {
            format!("{}K", tokens / 1000)
        } else {
            tokens.to_string()
        },
        native_config,
        native_suffix: String::new(),
        tokens,
    };
    let mut options = vec![option("standard", base, None)];
    if row.engine_id == CODEX_ENGINE_ID && max > base {
        options.push(option(
            "extended",
            max,
            Some(NativeContextConfig {
                model_context_window: max,
            }),
        ));
    }
    Some(NativeContextWindowCapability {
        availability: "configurable".to_owned(),
        default: "standard".to_owned(),
        options,
    })
}

fn discovered_speed_options(row: &DiscoveredModel) -> Vec<artisan_catalog::NativeSpeedOption> {
    if !row.fast {
        return Vec::new();
    }
    [("standard", "Standard", true), ("fast", "Fast", false)]
        .into_iter()
        .map(|(id, label, default)| artisan_catalog::NativeSpeedOption {
            availability: "available".to_owned(),
            consumption_basis: "unknown".to_owned(),
            consumption_multiplier: None,
            input_consumption_multiplier: None,
            output_consumption_multiplier: None,
            default,
            description: label.to_owned(),
            disabled: None,
            id: id.to_owned(),
            label: label.to_owned(),
            native_value: id.to_owned(),
            source_url: None,
            speed_multiplier: None,
            verified_at: None,
        })
        .collect()
}

fn thinking_from_discovery(
    thinking: &crate::model_discovery::DiscoveredThinking,
) -> NativeThinkingCapability {
    use crate::model_discovery::DiscoveredThinking;
    match thinking {
        DiscoveredThinking::Unavailable => NativeThinkingCapability::Unavailable,
        DiscoveredThinking::Native { description } => NativeThinkingCapability::Native {
            description: description.clone(),
        },
        DiscoveredThinking::Supported { default, options } => {
            if options.is_empty() {
                return NativeThinkingCapability::Unavailable;
            }
            let default_id = options
                .iter()
                .find(|option| option.id == *default)
                .map_or_else(|| options[0].id.clone(), |option| option.id.clone());
            NativeThinkingCapability::Supported {
                default: default_id,
                options: options
                    .iter()
                    .map(|option| NativeThinkingOption {
                        advisory: None,
                        description: option.description.clone(),
                        economics: option.economics.to_owned(),
                        id: option.id.clone(),
                        native_value: option.native_value.clone(),
                        presentation_group: option.presentation_group.to_owned(),
                    })
                    .collect(),
            }
        }
    }
}

fn unique_discovered_id(
    engine_id: &str,
    native_model_id: &str,
    existing: &HashSet<String>,
) -> String {
    let slug = native_model_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if native_model_id.starts_with(&format!("{engine_id}-")) {
        native_model_id.to_owned()
    } else if slug.is_empty() {
        format!("{engine_id}-model")
    } else {
        format!("{engine_id}-{slug}")
    };
    if !existing.contains(&base) {
        return base;
    }
    for index in 2..1000 {
        let candidate = format!("{base}-{index}");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    base
}

fn ensure_discovered_provider(manifest: &mut NativeModelManifest, provider: &str) {
    if manifest.provider(provider).is_none() {
        manifest.providers.push(NativeModelProvider {
            id: provider.to_owned(),
            label: provider_label(provider).to_owned(),
        });
    }
}

fn provider_label(provider: &str) -> &'static str {
    match provider {
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "xai" => "xAI",
        "google" => "Google",
        "cursor" => "Cursor",
        "moonshot" => "Moonshot AI",
        "zai" => "Z.ai",
        "deepseek" => "DeepSeek",
        "minimax" => "MiniMax",
        "meta" => "Meta",
        _ => "Unknown",
    }
}

#[cfg(test)]
#[path = "native_model_catalog/tests.rs"]
mod tests;
