use artisan_catalog::{NativeModelRouteStatus, NativeModelSelectability, NativeThinkingCapability};
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
                "limit": {"context": 128_000, "output": 4096},
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
        CatalogScope::new("profile-main", "C:/workspace", "safe").expect("fixture scope is valid"),
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

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive route/variant identity assertion over the fixture; splitting would fragment the coverage matrix"
)]
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
    let catalog =
        from_catalog_result(result.clone(), &favorites(&result)).expect("typed result converts");

    assert_eq!(catalog.catalog_revision, result.catalog_revision);
    assert_eq!(
        catalog.runnable_harness_ids,
        vec!["opencode2", "codex", "claude", "grok", "cursor"]
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
    assert_eq!(variant.capabilities.context_window_tokens, Some(128_000));
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
    assert!(
        catalog
            .manifest
            .models
            .iter()
            .all(|model| model.harness == "opencode2")
    );
    assert!(!catalog.selectability("codex-sol").is_available());
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
        vec!["opencode2", "codex", "claude", "grok", "cursor"]
    );
    assert!(
        catalog
            .manifest
            .models
            .iter()
            .all(|model| model.harness == "opencode2")
    );
    assert!(!catalog.selectability("codex-sol").is_available());
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
        variant_id: None,
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
        missing_engines: Vec::new(),
    };
    let catalog = from_catalog_result_with_discovery(
        fixture_result(),
        &discovery,
        &ModelFavoritesSnapshot::empty(),
    )
    .expect("catalog builds");

    let sol = catalog
        .manifest
        .model("codex-gpt-5-6-sol")
        .expect("static sol row");
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
            .is_some_and(|config| config.model_context_window == 872_000)
    }));

    let extended = context
        .options
        .iter()
        .find(|option| option.id == "extended")
        .unwrap();
    assert_eq!(extended.label, "872K");
    assert_eq!(extended.tokens, 872_000);

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
    assert!(
        catalog
            .manifest
            .models
            .iter()
            .all(|model| model.native_model_id != "gpt-5.5")
    );
}

#[test]
fn cli_discovered_opencode2_rows_carry_runnable_route_identity() {
    let mut variant = discovered(
        "opencode2",
        "opencode-go",
        "kimi-k3",
        "Kimi K3",
        Some(262_144),
        Some(262_144),
    );
    variant.variant_id = Some("high".to_owned());
    let discovery = crate::model_discovery::DiscoveryBundle {
        models: vec![
            discovered(
                "opencode2",
                "opencode-go",
                "kimi-k3",
                "Kimi K3",
                Some(262_144),
                Some(262_144),
            ),
            variant,
        ],
        probed_engines: vec!["opencode2"],
        missing_engines: Vec::new(),
    };
    let catalog = from_discovery(&discovery).expect("catalog builds");
    let id = artisan_catalog::wire::opencode2_catalog_id("kimi-k3", "opencode-go", None)
        .expect("route identity");
    let model = catalog
        .manifest
        .model(&id)
        .expect("discovered opencode2 row");
    assert_eq!(model.harness, "opencode2");
    assert_eq!(model.name, "Kimi K3");
    assert_eq!(model.status, "dynamic");
    let selection = model.native_selection.as_ref().expect("route selection");
    assert_eq!(selection.model_id, "kimi-k3");
    assert_eq!(selection.provider_route_id, "opencode-go");
    assert_eq!(selection.variant_id, None);
    let variant_id =
        artisan_catalog::wire::opencode2_catalog_id("kimi-k3", "opencode-go", Some("high"))
            .expect("variant identity");
    let variant_row = catalog
        .manifest
        .model(&variant_id)
        .expect("variant row is its own identity");
    assert_eq!(
        variant_row
            .native_selection
            .as_ref()
            .and_then(|selection| selection.variant_id.as_deref()),
        Some("high")
    );
    assert!(catalog.selectability(&variant_id).is_available());
    let route = catalog
        .routes
        .iter()
        .find(|route| route.id == "opencode-go")
        .expect("Go route");
    assert_eq!(route.label, "Go");
    assert_eq!(route.engine_id, "opencode2");
    assert!(
        catalog.selectability(&id).is_available(),
        "CLI-discovered OpenCode2 rows must be selectable through their route"
    );
}

#[test]
fn missing_engine_harnesses_are_hidden() {
    let discovery = crate::model_discovery::DiscoveryBundle {
        models: Vec::new(),
        probed_engines: Vec::new(),
        missing_engines: vec!["cursor", "grok"],
    };
    let catalog = from_discovery(&discovery).expect("catalog builds");
    let hidden = |id: &str| catalog.manifest.harness(id).expect("harness exists").hidden;
    assert!(hidden("cursor"));
    assert!(hidden("grok"));
    assert!(
        catalog.manifest.harness("hermes").is_none(),
        "hermes is not part of the bundled manifest"
    );
    assert!(!hidden("codex"));
    assert!(!hidden("claude"));
    assert!(!hidden("opencode2"));
}

#[test]
fn discovered_display_names_are_dehyphenated() {
    let discovery = crate::model_discovery::DiscoveryBundle {
        models: vec![
            discovered(
                "codex",
                "openai",
                "gpt-5.6-sol",
                "GPT-5.6-Sol",
                Some(272_000),
                Some(872_000),
            ),
            discovered(
                "codex",
                "openai",
                "gpt-7-stealth",
                "GPT-7-Stealth",
                Some(272_000),
                Some(872_000),
            ),
        ],
        probed_engines: vec!["codex"],
        missing_engines: Vec::new(),
    };
    let catalog = from_catalog_result_with_discovery(
        fixture_result(),
        &discovery,
        &ModelFavoritesSnapshot::empty(),
    )
    .expect("catalog builds");

    let sol = catalog
        .manifest
        .model("codex-gpt-5-6-sol")
        .expect("static sol row");
    assert_eq!(sol.name, "GPT 5.6 Sol");
    let stealth = catalog
        .manifest
        .models
        .iter()
        .find(|model| model.native_model_id == "gpt-7-stealth")
        .expect("new discovered row");
    assert_eq!(stealth.name, "GPT 7 Stealth");
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
        missing_engines: Vec::new(),
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
    assert_eq!(context.options[0].tokens, 1_000_000);
    assert!(context.options[0].native_suffix.is_empty());
    let haiku = catalog
        .manifest
        .models
        .iter()
        .find(|model| model.native_model_id == "claude-haiku-legacy")
        .expect("new haiku row");
    assert_eq!(
        haiku.capabilities.context_window.as_ref().unwrap().options[0].tokens,
        200_000
    );
}

#[test]
fn hidden_discovered_rows_are_never_surfaced() {
    let mut hidden = discovered(
        "codex",
        "openai",
        "gpt-reserve",
        "GPT-Reserve",
        Some(272_000),
        Some(872_000),
    );
    hidden.hidden = true;
    let discovery = crate::model_discovery::DiscoveryBundle {
        models: vec![
            discovered(
                "codex",
                "openai",
                "gpt-5.6-sol",
                "Sol",
                Some(272_000),
                Some(872_000),
            ),
            hidden,
        ],
        probed_engines: vec!["codex"],
        missing_engines: Vec::new(),
    };
    let catalog = from_catalog_result_with_discovery(
        fixture_result(),
        &discovery,
        &ModelFavoritesSnapshot::empty(),
    )
    .expect("catalog builds");
    assert!(
        catalog
            .manifest
            .models
            .iter()
            .all(|model| model.native_model_id != "gpt-reserve"),
        "engine-internal hidden rows must not reach the picker"
    );
}

#[test]
fn discovery_only_catalog_overlays_static_baseline() {
    let catalog = from_discovery(&crate::model_discovery::DiscoveryBundle {
        models: vec![discovered(
            "codex",
            "openai",
            "gpt-5.6-sol",
            "Sol Live",
            Some(272_000),
            Some(872_000),
        )],
        probed_engines: vec!["codex"],
        missing_engines: Vec::new(),
    })
    .expect("scope-free catalog builds");
    assert_eq!(
        catalog
            .manifest
            .model("codex-gpt-5-6-sol")
            .expect("static row")
            .name,
        "Sol Live"
    );
    assert!(
        catalog
            .manifest
            .model("codex-gpt-5-6-sol")
            .expect("static row")
            .capabilities
            .context_window
            .is_some()
    );
    assert_eq!(catalog.manifest.models.len(), 1);
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
        missing_engines: Vec::new(),
    };
    let catalog = from_catalog_result(fixture_result(), &ModelFavoritesSnapshot::empty())
        .expect("base catalog");
    let runtime = catalog.runtime();
    let mut manifest = catalog.manifest;
    let _ = apply_discovery(&mut manifest, &discovery, false);
    let next = NativeModelCatalog::from_manifest(manifest, runtime);
    if let Err(error) = artisan_catalog::wire::encode_catalog(&next) {
        panic!("wire error: {error:?}");
    }
}
