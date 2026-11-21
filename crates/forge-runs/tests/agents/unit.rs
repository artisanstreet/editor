use super::*;
use artisan_api::{EngineStatus, Model};

fn model(id: &str, efforts: &[&str]) -> Model {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": id, "efforts": efforts, "default_effort": efforts.first(),
        "context_window": null, "supports_images": true,
    }))
    .unwrap()
}

fn status(engine: EngineId, models: Vec<Model>, default: &str) -> EngineStatus {
    let mut status: EngineStatus = serde_json::from_value(serde_json::json!({
        "engine": engine, "install": {"state": "absent"}, "sign_in": {"state": "unknown"},
        "models": [], "default_model": default, "usage": null,
    }))
    .unwrap_or_else(|_| panic!("status shape"));
    status.install = InstallState::Installed {
        version: "1".into(),
        update: None,
    };
    status.models = models.into();
    status
}

fn parent() -> EngineConfig {
    EngineConfig {
        model: Some("opus".into()),
        effort: Some("high".into()),
        ..EngineConfig::new(EngineId::Claude)
    }
}

#[test]
fn nothing_given_inherits_the_parent_without_a_warning() {
    let claude = status(
        EngineId::Claude,
        vec![model("opus", &["low", "high"])],
        "opus",
    );
    let (config, warnings) = resolve(&parent(), &SpawnAgent::default(), &claude).unwrap();
    assert_eq!(config.model.as_deref(), Some("opus"));
    assert_eq!(config.effort.as_deref(), Some("high"));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn another_harness_without_a_model_gets_its_default_and_says_so() {
    let codex = status(
        EngineId::Codex,
        vec![
            model("gpt-a", &["low", "medium", "high"]),
            model("gpt-b", &["low"]),
        ],
        "gpt-a",
    );
    let request = SpawnAgent {
        harness: Some(EngineId::Codex),
        ..SpawnAgent::default()
    };
    let (config, warnings) = resolve(&parent(), &request, &codex).unwrap();
    assert_eq!(config.engine, EngineId::Codex);
    assert_eq!(config.model.as_deref(), Some("gpt-a"));
    assert_eq!(
        config.effort.as_deref(),
        Some("high"),
        "the parent's effort fits"
    );
    assert_eq!(
        warnings,
        ["No model given for Codex; using gpt-a (its default)."]
    );
}

#[test]
fn what_the_model_lacks_falls_back_with_a_warning_and_permissions_cap() {
    let codex = status(EngineId::Codex, vec![model("gpt-b", &["low"])], "gpt-b");
    let request = SpawnAgent {
        harness: Some(EngineId::Codex),
        model: Some("claude-opus".into()),
        effort: Some("max".into()),
        permissions: Some(Permissions::Full),
        ..SpawnAgent::default()
    };
    let (config, warnings) = resolve(&parent(), &request, &codex).unwrap();
    assert_eq!(config.model.as_deref(), Some("gpt-b"));
    assert_eq!(config.effort.as_deref(), Some("low"));
    assert_eq!(config.permissions, Permissions::Auto);
    assert_eq!(
        warnings,
        [
            "Codex has no model claude-opus; using gpt-b.",
            "gpt-b has no effort max; using low.",
            "Permissions capped at auto (the parent's).",
        ]
    );
}

fn configurable_model() -> Model {
    Model {
        variants: vec![
            artisan_api::Variant {
                id: "medium".into(),
                label: "Medium".into(),
            },
            artisan_api::Variant {
                id: "xhigh".into(),
                label: "Extra high".into(),
            },
        ],
        default_variant: Some("medium".into()),
        context_windows: vec![artisan_api::ContextWindow {
            tokens: 1_000_000,
            override_tokens: Some(1_000_000),
        }],
        ..model("muse", &[])
    }
}

#[test]
fn opencode_variant_context_and_options_can_be_requested_across_harnesses() {
    let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
    let request = SpawnAgent {
        model: Some("muse".into()),
        variant: Some("xhigh".into()),
        context_window: Some(1_000_000),
        options: Some(
            serde_json::from_value(serde_json::json!({
                "context_window": 200_000, "custom": true,
            }))
            .unwrap(),
        ),
        ..SpawnAgent::default()
    };
    let (config, warnings) = resolve(&parent(), &request, &status).unwrap();
    assert_eq!(config.variant.as_deref(), Some("xhigh"));
    assert_eq!(config.effort, None);
    assert_eq!(config.context_window_override(), Some(1_000_000));
    assert_eq!(config.options["custom"], true);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn opencode_inherits_variant_without_stale_effort() {
    let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
    let mut parent = EngineConfig::new(EngineId::OpenCode);
    parent.model = Some("muse".into());
    parent.effort = Some("medium".into());
    parent.variant = Some("xhigh".into());
    let (config, warnings) = resolve(&parent, &SpawnAgent::default(), &status).unwrap();
    assert_eq!(config.variant.as_deref(), Some("xhigh"));
    assert_eq!(config.effort, None);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn opencode_warns_when_effort_is_requested_instead_of_variant() {
    let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
    let request = SpawnAgent {
        effort: Some("xhigh".into()),
        ..SpawnAgent::default()
    };
    let (config, warnings) = resolve(&parent(), &request, &status).unwrap();
    assert_eq!(config.effort, None);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("Set variant instead"))
    );
}

#[test]
fn unsupported_variant_and_context_fall_back_with_warnings() {
    let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
    let request = SpawnAgent {
        model: Some("muse".into()),
        variant: Some("unknown".into()),
        context_window: Some(123),
        ..SpawnAgent::default()
    };
    let (config, warnings) = resolve(&parent(), &request, &status).unwrap();
    assert_eq!(config.variant.as_deref(), Some("medium"));
    assert_eq!(config.context_window_override(), None);
    assert_eq!(
        warnings,
        [
            "muse has no variant unknown; using medium.",
            "muse has no context window override 123; using its standard window.",
        ]
    );
}

#[test]
fn model_options_inherit_merge_and_reset_context() {
    let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
    let mut parent = EngineConfig::new(EngineId::OpenCode);
    parent.model = Some("muse".into());
    parent.variant = Some("xhigh".into());
    parent.options = serde_json::from_value(serde_json::json!({
        "context_window": 1_000_000, "keep": true, "replace": "old",
    }))
    .unwrap();
    let (inherited, warnings) = resolve(&parent, &SpawnAgent::default(), &status).unwrap();
    assert_eq!(inherited, parent);
    assert!(warnings.is_empty());
    let request = SpawnAgent {
        options: Some(
            serde_json::from_value(serde_json::json!({
                "context_window": null, "replace": "new",
            }))
            .unwrap(),
        ),
        ..SpawnAgent::default()
    };
    let (config, warnings) = resolve(&parent, &request, &status).unwrap();
    assert_eq!(config.variant, parent.variant);
    assert_eq!(config.context_window_override(), None);
    assert!(!config.options.contains_key("context_window"));
    assert_eq!(config.options["keep"], true);
    assert_eq!(config.options["replace"], "new");
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn changing_models_drops_inherited_options_and_uses_variant_default() {
    let status = status(EngineId::Claude, vec![configurable_model()], "muse");
    let mut parent = parent();
    parent.variant = Some("xhigh".into());
    parent.options.insert("custom".into(), true.into());
    let request = SpawnAgent {
        model: Some("muse".into()),
        ..SpawnAgent::default()
    };
    let (config, warnings) = resolve(&parent, &request, &status).unwrap();
    assert_eq!(config.variant.as_deref(), Some("medium"));
    assert!(config.options.is_empty());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_signed_out_harness_refuses_with_sign_in_first() {
    let mut codex = status(EngineId::Codex, vec![model("gpt-b", &["low"])], "gpt-b");
    codex.sign_in = SignInState::SignedOut;
    let request = SpawnAgent {
        harness: Some(EngineId::Codex),
        ..SpawnAgent::default()
    };
    let error = resolve(&parent(), &request, &codex).unwrap_err();
    assert!(error.to_string().contains("isn't signed in"), "{error}");
}
