//! Conversion from the owner-scoped OpenCode2 runtime result to the shared
//! static-plus-runtime native model catalog.
//!
//! The engine-owner catalog worker owns discovery and returns typed data. This
//! leaf performs no process, HTTP, or provider work. It only combines that
//! result with the checked-in manifest and the authoritative durable favorites
//! snapshot, preserving native route/variant identity all the way to policy
//! admission.

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
use crate::model_discovery::DiscoveredModel;

const OPENCODE2_ENGINE_ID: &str = "opencode2";
const CODEX_ENGINE_ID: &str = "codex";
const CLAUDE_ENGINE_ID: &str = "claude";
const GROK_ENGINE_ID: &str = "grok";
const CURSOR_ENGINE_ID: &str = "cursor";
const HERMES_ENGINE_ID: &str = "hermes";

/// Harness identifiers Forge can execute once their fixture-proven runtimes
/// are registered in this process.
///
/// `runnable` means a supported harness with an in-tree runtime, not an
/// installed binary. Executable resolution and the readiness handshake stay
/// the live gate in each per-engine executor and probe path: a missing CLI
/// still yields unavailable-with-reason at runtime, never a false ready.
const RUNNABLE_ENGINE_IDS: [&str; 6] = [
    OPENCODE2_ENGINE_ID,
    CODEX_ENGINE_ID,
    CLAUDE_ENGINE_ID,
    GROK_ENGINE_ID,
    CURSOR_ENGINE_ID,
    HERMES_ENGINE_ID,
];

/// Payload-free failure while combining the typed runtime result with the
/// shared catalog manifest.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub(crate) enum NativeModelCatalogBridgeError {
    /// The result came from an engine other than the supported native engine.
    #[error("native catalog result is from an unsupported engine")]
    UnsupportedEngine,
    /// The checked-in manifest could not be decoded.
    #[error("bundled native catalog manifest is invalid")]
    BundledManifest,
    /// A runtime route did not satisfy the OpenCode2 route contract.
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

/// Combines one typed OpenCode2 discovery result with the exact bundled
/// manifest and owner-authoritative favorites.
///
/// Only the discovered `opencode2` rows are added to the manifest. Static
/// rows for the other fixture-proven harnesses are readable and runnable
/// through [`RUNNABLE_ENGINE_IDS`]; Hermes and OpenCode2 rows arrive only
/// through live discovery. No thinking, speed, MCP, web-search, permission,
/// or cost value is inferred when OpenCode2 did not report it.
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

    let mut manifest = NativeModelCatalog::offline()
        .map_err(|_| NativeModelCatalogBridgeError::BundledManifest)?
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

/// Combines the typed OpenCode2 result with best-effort live discovery from
/// every other engine.
///
/// Discovery rows replace static reported fields (name, description,
/// reasoning options, limits, modalities, pricing) while static harness
/// policy survives: context-window choices (including Codex's 272K/1M
/// override and Claude's `[1m]` selection), speed economics, MCP/web-search
/// policy, and routing. Rows a fully authoritative probe (Codex, Claude) no
/// longer returns are disabled with a truthful reason rather than deleted,
/// so saved selections still render.
pub(crate) fn from_catalog_result_with_discovery(
    result: CatalogResult,
    discovery: &crate::model_discovery::DiscoveryBundle,
    favorites: &ModelFavoritesSnapshot,
) -> Result<NativeModelCatalog, NativeModelCatalogBridgeError> {
    let catalog = from_catalog_result(result, favorites)?;
    if discovery.probed_engines.is_empty() {
        return Ok(catalog);
    }
    let runtime = catalog.runtime();
    let base_revision = runtime.catalog_revision.clone().unwrap_or_default();
    let runtime = NativeCatalogRuntime {
        catalog_revision: Some(format!(
            "{base_revision}+discovery-{:016x}",
            discovery_revision_hash(discovery)
        )),
        ..runtime
    };
    let mut manifest = catalog.manifest;
    apply_discovery(&mut manifest, discovery);
    let next = NativeModelCatalog::from_manifest(manifest, runtime);
    artisan_catalog::wire::encode_catalog(&next)
        .map_err(|_| NativeModelCatalogBridgeError::InvalidCatalog)?;
    Ok(next)
}

/// Stable revision contribution for one discovery bundle.
fn discovery_revision_hash(discovery: &crate::model_discovery::DiscoveryBundle) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for model in &discovery.models {
        for byte in model.engine_id.bytes().chain(model.native_model_id.bytes()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= u64::from(model.hidden);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Engines whose probe is a complete account catalog, so an absent static row
/// is truthfully unavailable rather than merely unreported.
fn discovery_is_authoritative(engine_id: &str) -> bool {
    matches!(engine_id, "codex" | "claude")
}

fn apply_discovery(
    manifest: &mut NativeModelManifest,
    discovery: &crate::model_discovery::DiscoveryBundle,
) {
    for engine_id in &discovery.probed_engines {
        let rows = discovery.for_engine(engine_id);
        if rows.is_empty() {
            continue;
        }
        let Some(template) = manifest
            .models
            .iter()
            .find(|model| model.harness == *engine_id)
            .cloned()
        else {
            continue;
        };
        let discovered = rows
            .iter()
            .map(|row| row.native_model_id.as_str())
            .collect::<HashSet<_>>();

        for model in manifest
            .models
            .iter_mut()
            .filter(|model| model.harness == *engine_id)
        {
            if let Some(row) = rows
                .iter()
                .find(|row| row.native_model_id == model.native_model_id)
            {
                overlay_reported(model, row);
            }
        }

        let mut existing = manifest
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect::<HashSet<_>>();
        for row in &rows {
            if manifest.models.iter().any(|model| {
                model.harness == *engine_id && model.native_model_id == row.native_model_id
            }) {
                continue;
            }
            let id = unique_discovered_id(engine_id, &row.native_model_id, &existing);
            existing.insert(id.clone());
            ensure_discovered_provider(manifest, &row.provider);
            manifest
                .models
                .push(discovered_definition(row, &template, id));
        }

        if discovery_is_authoritative(engine_id) {
            let reason = format!(
                "The {engine_id} engine did not return this model for the current account."
            );
            for model in manifest
                .models
                .iter_mut()
                .filter(|model| model.harness == *engine_id)
            {
                if discovered.contains(model.native_model_id.as_str()) || model.status == "dynamic"
                {
                    continue;
                }
                if model.disabled.is_none() {
                    model.disabled = Some(NativeDisabledModel {
                        reason: reason.clone(),
                    });
                }
            }
        }
    }
}

/// Replaces reported fields on a matched static row, retaining that row's
/// harness policy (context options, speed economics, routing, MCP/search).
fn overlay_reported(model: &mut NativeModelDefinition, row: &DiscoveredModel) {
    if !row.name.is_empty() {
        model.name.clone_from(&row.name);
    }
    if row.description.is_some() {
        model.description.clone_from(&row.description);
    }
    model.capabilities.thinking = thinking_from_discovery(&row.thinking);
    model.capabilities.context_window_tokens = row
        .context_window_tokens
        .or(model.capabilities.context_window_tokens);
    model.capabilities.output_tokens = row.output_tokens.or(model.capabilities.output_tokens);
    model.capabilities.image_input = row.image_input;
    model.capabilities.local_tools = row.tools;
    model.upstream_model_id = row
        .upstream_model_id
        .clone()
        .or_else(|| model.upstream_model_id.clone());
    model.metadata_confidence = Some(row.metadata_confidence.to_owned());
    model.cost = row.cost.map(|(input, output)| NativeModelCost {
        input_usd_per_million: Some(input),
        output_usd_per_million: Some(output),
    });
    model.disabled = None;
}

/// Builds one runtime-only row from discovery plus cloned harness policy.
fn discovered_definition(
    row: &DiscoveredModel,
    template: &NativeModelDefinition,
    id: String,
) -> NativeModelDefinition {
    NativeModelDefinition {
        id,
        name: row.name.clone(),
        native_model_id: row.native_model_id.clone(),
        description: row.description.clone(),
        harness: row.engine_id.to_owned(),
        provider: row.provider.clone(),
        routing: template.routing.clone(),
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
            context_window: discovered_context_policy(row, template),
            image_input: row.image_input,
            local_tools: row.tools,
            mcp: template.capabilities.mcp,
            output_tokens: row.output_tokens,
            reasoning_display: template.capabilities.reasoning_display.clone(),
            speed_options: template.capabilities.speed_options.clone(),
            thinking: thinking_from_discovery(&row.thinking),
            web_search: template.capabilities.web_search,
        },
    }
}

/// Applies the static harness context policy to a new discovered row.
///
/// Codex keeps its 272K/1M override only for models the cache reports as
/// extending beyond their default window; Claude keeps the 200K/1M `[1m]`
/// choice only for models whose runtime advertises a 1M input window.
fn discovered_context_policy(
    row: &DiscoveredModel,
    template: &NativeModelDefinition,
) -> Option<NativeContextWindowCapability> {
    let applies = match row.engine_id {
        "codex" => matches!(
            (row.context_window_tokens, row.max_context_window_tokens),
            (Some(context), Some(max)) if max > context
        ),
        "claude" => row
            .context_window_tokens
            .is_some_and(|tokens| tokens >= 1_000_000),
        _ => false,
    };
    applies.then(|| template.capabilities.context_window.clone())?
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
                .map(|option| option.id.clone())
                .unwrap_or_else(|| options[0].id.clone());
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
mod tests {
    use artisan_catalog::{
        NativeModelRouteStatus, NativeModelSelectability, NativeThinkingCapability,
    };
    use artisan_domain::{ModelFavoriteId, ModelFavoritesRevision, ModelFavoritesSnapshot};
    use serde_json::json;

    use super::*;
    use crate::engine_owner::catalog::{CatalogScope, decode_models_response, normalize_catalog};

    fn fixture_result() -> CatalogResult {
        let value = json!({
            "data": [
                {
                    "id": "x-preview-f-free",
                    "modelID": "x-preview-f-free",
                    "name": "Preview Free",
                    "providerID": "opencode",
                    "status": "active",
                    "enabled": true,
                    "limit": {"context": 128000, "output": 4096},
                    "capabilities": {
                        "input": ["text", "image"],
                        "output": ["text"],
                        "tools": true
                    },
                    "cost": [{"input": 0.2, "output": 0.8}],
                    "variants": [{"id": "high"}, {"id": "low"}]
                },
                {
                    "id": "x-preview-go",
                    "modelID": "x-preview-go",
                    "name": "Preview Go",
                    "providerID": "opencode-go",
                    "status": "alpha",
                    "enabled": false,
                    "limit": {"context": 64000, "output": 2048},
                    "capabilities": {
                        "input": ["text"],
                        "output": ["text"],
                        "tools": false
                    },
                    "cost": [],
                    "variants": [{"id": "balanced"}]
                },
                {
                    "id": "legacy-custom",
                    "modelID": "legacy-upstream",
                    "name": "Legacy Custom",
                    "providerID": "provider-custom",
                    "status": "deprecated",
                    "enabled": true,
                    "limit": {"context": 32000, "output": 1024},
                    "capabilities": {
                        "input": ["text"],
                        "output": ["text"],
                        "tools": false
                    },
                    "cost": [{"input": 1.0, "output": 2.0}],
                    "variants": []
                },
                {
                    "id": "beta-model",
                    "modelID": "beta-upstream",
                    "name": "Beta model",
                    "providerID": "opencode",
                    "status": "beta",
                    "enabled": true,
                    "limit": {"context": 16000, "output": 512},
                    "capabilities": {
                        "input": ["text"],
                        "output": ["text"],
                        "tools": false
                    },
                    "cost": [],
                    "variants": []
                }
            ]
        });
        let bytes = serde_json::to_vec(&value).expect("fixture encodes");
        let raw = decode_models_response(&bytes).expect("fixture has the worker shape");
        normalize_catalog(
            raw,
            CatalogScope::new("profile-main", "C:/workspace", "safe")
                .expect("fixture scope is valid"),
        )
        .expect("fixture normalizes")
    }

    fn favorites(result: &CatalogResult) -> ModelFavoritesSnapshot {
        let model_id = result
            .models
            .iter()
            .find(|model| {
                model.provider_route_id == "opencode-go"
                    && model.variant_id.as_deref() == Some("balanced")
            })
            .expect("disabled variant fixture row")
            .catalog_id
            .clone();
        ModelFavoritesSnapshot::new(
            ModelFavoritesRevision::new(7).expect("revision is valid"),
            vec![ModelFavoriteId::parse(model_id).expect("fixture id is valid")],
        )
        .expect("fixture favorites are valid")
    }

    #[test]
    fn converts_all_runtime_rows_with_exact_route_variant_identity() {
        let result = fixture_result();
        assert!(
            result
                .models
                .iter()
                .any(|model| model.status.as_str() == "beta")
        );
        assert_eq!(result.models.len(), 7);
        let favorite_id = result
            .models
            .iter()
            .find(|model| {
                model.provider_route_id == "opencode-go"
                    && model.variant_id.as_deref() == Some("balanced")
            })
            .expect("disabled variant fixture row")
            .catalog_id
            .clone();
        let catalog = from_catalog_result(result.clone(), &favorites(&result))
            .expect("typed result converts");

        assert_eq!(catalog.catalog_revision, result.catalog_revision);
        assert_eq!(
            catalog.runnable_harness_ids,
            vec!["opencode2", "codex", "claude", "grok", "cursor", "hermes"]
        );
        assert_eq!(catalog.favorite_ids, vec![favorite_id.clone()]);
        assert_eq!(
            catalog
                .scope
                .as_ref()
                .expect("scope is retained")
                .profile_id,
            "profile-main"
        );
        assert!(catalog.manifest.provider("opencode").is_some());
        assert!(catalog.manifest.provider("opencode-go").is_some());
        assert!(catalog.manifest.provider("provider-custom").is_some());
        assert_eq!(catalog.routes.len(), 3);
        let go_route = catalog
            .routes
            .iter()
            .find(|route| route.id == "opencode-go")
            .expect("go route");
        assert_eq!(go_route.group.id, "go");
        assert_eq!(go_route.group.order, 0);
        assert!(!go_route.group.show_route_labels);
        let custom_route = catalog
            .routes
            .iter()
            .find(|route| route.id == "provider-custom")
            .expect("custom route");
        assert_eq!(custom_route.group.id, "custom");
        assert!(custom_route.group.show_route_labels);

        let variant = &catalog
            .manifest
            .models
            .iter()
            .find(|model| {
                model
                    .native_selection
                    .as_ref()
                    .and_then(|selection| selection.variant_id.as_deref())
                    == Some("high")
            })
            .expect("variant row");
        assert_eq!(variant.harness, "opencode2");
        assert_eq!(variant.provider, "opencode");
        assert_eq!(variant.native_model_id, "x-preview-f-free");
        assert_eq!(
            variant
                .native_selection
                .as_ref()
                .expect("native selection")
                .variant_id
                .as_deref(),
            Some("high")
        );
        assert_eq!(variant.capabilities.context_window_tokens, Some(128000));
        assert_eq!(variant.capabilities.output_tokens, Some(4096));
        assert!(variant.capabilities.image_input);
        assert!(variant.capabilities.local_tools);
        assert!(!variant.capabilities.mcp);
        assert!(!variant.capabilities.web_search);
        assert!(variant.capabilities.speed_options.is_empty());
        assert!(matches!(
            &variant.capabilities.thinking,
            NativeThinkingCapability::Unavailable
        ));
        assert_eq!(
            variant
                .cost
                .as_ref()
                .expect("reported cost")
                .output_usd_per_million,
            Some(0.8)
        );

        assert!(matches!(
            catalog.selectability(&favorite_id),
            NativeModelSelectability::Unavailable { .. }
        ));
        let disabled = catalog
            .manifest
            .model(&favorite_id)
            .expect("disabled model");
        assert!(disabled.disabled.is_some());
        assert_eq!(
            disabled
                .native_selection
                .as_ref()
                .expect("disabled identity")
                .variant_id
                .as_deref(),
            Some("balanced")
        );
        assert_eq!(
            catalog
                .manifest
                .model("codex-sol")
                .expect("static row")
                .harness,
            "codex"
        );
        assert!(
            catalog.selectability("codex-sol").is_available(),
            "fixture-proven codex harness is runnable"
        );
        assert!(
            catalog
                .routes
                .iter()
                .all(|route| matches!(&route.status, NativeModelRouteStatus::Available))
        );
    }

    #[test]
    fn rejects_wrong_engine_and_tampered_native_identity() {
        let mut wrong_engine = fixture_result();
        wrong_engine.engine_id = "other".to_owned();
        assert_eq!(
            from_catalog_result(wrong_engine, &ModelFavoritesSnapshot::empty()),
            Err(NativeModelCatalogBridgeError::UnsupportedEngine)
        );

        let mut tampered = fixture_result();
        tampered.models[0].catalog_id.push('x');
        assert_eq!(
            from_catalog_result(tampered, &ModelFavoritesSnapshot::empty()),
            Err(NativeModelCatalogBridgeError::InvalidModelIdentity)
        );

        let mut oversized_native_id = fixture_result();
        let oversized = "m".repeat(129);
        {
            let selection = &mut oversized_native_id.models[0].native_selection;
            selection.model_id = oversized.clone();
        }
        oversized_native_id.models[0].model_id = oversized.clone();
        oversized_native_id.models[0].native_model_id = oversized.clone();
        oversized_native_id.models[0].catalog_id =
            artisan_catalog::wire::opencode2_catalog_id(&oversized, "opencode", None)
                .expect("oversized catalog identity remains within the catalog bound");
        assert_eq!(
            from_catalog_result(oversized_native_id, &ModelFavoritesSnapshot::empty()),
            Err(NativeModelCatalogBridgeError::InvalidModelIdentity)
        );
    }

    #[test]
    fn runnable_set_admits_all_fixture_proven_engines() {
        let result = fixture_result();
        let catalog = from_catalog_result(result, &ModelFavoritesSnapshot::empty())
            .expect("typed result converts");
        assert_eq!(
            catalog.runnable_harness_ids,
            vec!["opencode2", "codex", "claude", "grok", "cursor", "hermes"]
        );
        for model_id in [
            "codex-sol",
            "claude-fable",
            "grok-4-6",
            "cursor-composer-2-5",
        ] {
            assert!(
                catalog.selectability(model_id).is_available(),
                "{model_id} is selectable on its fixture-proven harness"
            );
            assert!(
                catalog.policy_for_model(model_id).is_ok(),
                "{model_id} admits a runnable policy"
            );
        }
    }

    #[test]
    fn runnable_is_harness_support_not_probe_readiness() {
        let result = fixture_result();
        let catalog = from_catalog_result(result, &ModelFavoritesSnapshot::empty())
            .expect("typed result converts");
        assert!(
            catalog.runnable_harness_ids.contains(&"codex".to_owned()),
            "codex is a supported harness"
        );
        assert!(
            catalog
                .routes
                .iter()
                .all(|route| route.engine_id == "opencode2"),
            "the bridge manufactures no readiness evidence for other engines"
        );
        assert!(
            catalog
                .routes
                .iter()
                .all(|route| route.engine_id != "codex"),
            "no codex route is invented without live discovery"
        );
        let unknown = catalog.selectability("codex:does-not-exist");
        assert!(
            !unknown.is_available(),
            "an undiscovered engine model stays unavailable"
        );
        assert!(
            unknown
                .unavailable_reason()
                .is_some_and(|reason| reason.contains("not in this catalog")),
            "the unavailable reason stays honest, never a false ready"
        );
    }

    #[test]
    fn preserves_unavailable_route_status_and_reason() {
        let mut result = fixture_result();
        result
            .routes
            .iter_mut()
            .find(|route| route.id == "opencode")
            .expect("fixture route")
            .availability = CatalogAvailability::Unavailable {
            reason: "OpenCode route is unavailable in this runtime.",
        };
        let catalog = from_catalog_result(result, &ModelFavoritesSnapshot::empty())
            .expect("route status converts");
        let route = catalog
            .routes
            .iter()
            .find(|route| route.id == "opencode")
            .expect("fixture route");
        assert!(matches!(&route.status, NativeModelRouteStatus::Unavailable));
        assert_eq!(
            route.unavailable_reason.as_deref(),
            Some("OpenCode route is unavailable in this runtime.")
        );
        let active_model = catalog
            .manifest
            .models
            .iter()
            .find(|model| model.native_model_id == "x-preview-f-free")
            .expect("active fixture model");
        assert_eq!(
            catalog.selectability(&active_model.id).unavailable_reason(),
            Some("OpenCode route is unavailable in this runtime.")
        );
    }

    fn discovered(
        engine_id: &'static str,
        provider: &str,
        native_model_id: &str,
        name: &str,
        context: Option<u64>,
        max_context: Option<u64>,
    ) -> DiscoveredModel {
        DiscoveredModel {
            engine_id,
            provider: provider.to_owned(),
            native_model_id: native_model_id.to_owned(),
            upstream_model_id: None,
            name: name.to_owned(),
            description: Some(format!("{name} description")),
            hidden: false,
            default: false,
            thinking: crate::model_discovery::DiscoveredThinking::Supported {
                default: "high".to_owned(),
                options: vec![
                    crate::model_discovery::DiscoveredThinkingOption {
                        id: "light".to_owned(),
                        native_value: "low".to_owned(),
                        description: Some("Quick".to_owned()),
                        economics: "standard",
                        presentation_group: "base",
                    },
                    crate::model_discovery::DiscoveredThinkingOption {
                        id: "high".to_owned(),
                        native_value: "high".to_owned(),
                        description: None,
                        economics: "standard",
                        presentation_group: "base",
                    },
                ],
            },
            fast: true,
            context_window_tokens: context,
            max_context_window_tokens: max_context,
            output_tokens: Some(128_000),
            image_input: true,
            tools: true,
            web_search: true,
            cost: None,
            status: "active",
            metadata_confidence: "reported",
        }
    }

    #[test]
    fn discovery_overlays_reported_fields_and_preserves_codex_context_policy() {
        let discovery = crate::model_discovery::DiscoveryBundle {
            models: vec![
                discovered(
                    "codex",
                    "openai",
                    "gpt-5.6-sol",
                    "Sol Discovered",
                    Some(272_000),
                    Some(872_000),
                ),
                discovered(
                    "codex",
                    "openai",
                    "gpt-7-stealth",
                    "Stealth",
                    Some(272_000),
                    Some(872_000),
                ),
            ],
            probed_engines: vec!["codex"],
        };
        let catalog = from_catalog_result_with_discovery(
            fixture_result(),
            &discovery,
            &ModelFavoritesSnapshot::empty(),
        )
        .expect("catalog builds");

        let sol = catalog.manifest.model("codex-sol").expect("static sol row");
        assert_eq!(sol.name, "Sol Discovered");
        assert_eq!(sol.metadata_confidence.as_deref(), Some("reported"));
        let context = sol
            .capabilities
            .context_window
            .as_ref()
            .expect("codex 1M policy survives overlay");
        assert_eq!(context.default, "standard");
        assert!(context.options.iter().any(|option| {
            option
                .native_config
                .as_ref()
                .is_some_and(|config| config.model_context_window == 1_050_000)
        }));

        let stealth = catalog
            .manifest
            .models
            .iter()
            .find(|model| model.native_model_id == "gpt-7-stealth")
            .expect("new discovered row");
        assert_eq!(stealth.id, "codex-gpt-7-stealth");
        assert_eq!(stealth.status, "dynamic");
        assert!(
            stealth.capabilities.context_window.is_some(),
            "new extended-window codex rows inherit the 1M policy"
        );
        let retired = catalog
            .manifest
            .model("codex-gpt-5-5")
            .expect("static retired row");
        assert!(retired.disabled.is_some(), "absent codex rows are disabled");
    }

    #[test]
    fn discovery_adds_claude_rows_with_1m_policy_only_when_eligible() {
        let discovery = crate::model_discovery::DiscoveryBundle {
            models: vec![
                discovered(
                    "claude",
                    "anthropic",
                    "claude-opus-4-8",
                    "Opus 4.8",
                    Some(1_000_000),
                    Some(1_000_000),
                ),
                discovered(
                    "claude",
                    "anthropic",
                    "claude-haiku-legacy",
                    "Haiku Legacy",
                    Some(200_000),
                    Some(200_000),
                ),
            ],
            probed_engines: vec!["claude"],
        };
        let catalog = from_catalog_result_with_discovery(
            fixture_result(),
            &discovery,
            &ModelFavoritesSnapshot::empty(),
        )
        .expect("catalog builds");
        let opus = catalog
            .manifest
            .models
            .iter()
            .find(|model| model.native_model_id == "claude-opus-4-8")
            .expect("new claude row");
        assert_eq!(opus.id, "claude-opus-4-8");
        let context = opus
            .capabilities
            .context_window
            .as_ref()
            .expect("1M claude rows inherit the [1m] choice");
        assert!(
            context
                .options
                .iter()
                .any(|option| option.native_suffix == "[1m]")
        );
        let haiku = catalog
            .manifest
            .models
            .iter()
            .find(|model| model.native_model_id == "claude-haiku-legacy")
            .expect("new haiku row");
        assert!(haiku.capabilities.context_window.is_none());
    }

    #[test]
    fn discovery_rows_survive_wire_validation() {
        let discovery = crate::model_discovery::DiscoveryBundle {
            models: vec![discovered(
                "codex",
                "openai",
                "gpt-7-stealth",
                "Stealth",
                Some(272_000),
                Some(872_000),
            )],
            probed_engines: vec!["codex"],
        };
        let catalog = from_catalog_result(fixture_result(), &ModelFavoritesSnapshot::empty())
            .expect("base catalog");
        let runtime = catalog.runtime();
        let mut manifest = catalog.manifest;
        apply_discovery(&mut manifest, &discovery);
        let next = NativeModelCatalog::from_manifest(manifest, runtime);
        if let Err(error) = artisan_catalog::wire::encode_catalog(&next) {
            panic!("wire error: {error:?}");
        }
    }
}
