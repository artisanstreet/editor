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
    NativeCatalogRuntime, NativeCatalogScope, NativeDisabledModel, NativeModelCapabilities,
    NativeModelCatalog, NativeModelCost, NativeModelDefinition, NativeModelManifest,
    NativeModelProvider, NativeModelRoute, NativeModelRouteGroup, NativeModelRouteStatus,
    NativeModelRouting, NativeModelSelection, NativeThinkingCapability,
};
use artisan_domain::{
    EngineModelId, EngineProfileId, EngineRouteId, EngineVariantId, ModelFavoritesSnapshot,
};
use thiserror::Error;

use crate::engine_owner::catalog::{
    CatalogAvailability, CatalogModel, CatalogResult, CatalogRoute,
};

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
        result.routes.iter_mut().find(|route| route.id == "opencode").expect("fixture route").availability = CatalogAvailability::Unavailable {
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
}
