//! Focused parity tests for the dependency-free harness-config registry policy.

#![allow(dead_code)]

#[path = "../../modules/backend/src/harness_config_registry_policy.rs"]
mod harness_config_registry_policy;

use harness_config_registry_policy::{
    ActivationTiming, ConfigDocumentFormat, HarnessConfigKeyIdentity, HarnessConfigRegistry,
    HarnessConfigRegistryError, HarnessConfigTarget, codex_auto_compaction_trigger_tokens_key,
    codex_request_user_input_key, default_harness_config_keys, empty_harness_config_registry,
    harness_config_key_id,
};

fn key(harness_id: &str, path: &[&str], description: &str) -> HarnessConfigKeyIdentity {
    HarnessConfigKeyIdentity::new(ActivationTiming::NextTurn, description, harness_id, path)
}

fn target(harness_id: &str, format: ConfigDocumentFormat, path: &str) -> HarnessConfigTarget {
    HarnessConfigTarget::new("backups", format, harness_id, path)
}

#[test]
fn defaults_select_only_the_declared_codex_request_input_key() {
    let keys = default_harness_config_keys();

    assert_eq!(keys, vec![codex_request_user_input_key()]);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].activation, ActivationTiming::NewThreads);
    assert_eq!(keys[0].harness_id, "codex");
    assert_eq!(
        keys[0].path,
        vec![
            "features".to_owned(),
            "default_mode_request_user_input".to_owned()
        ]
    );
    assert_eq!(
        keys[0].description,
        "Let the agent pause and ask you a question outside plan mode instead of assuming an answer."
    );
}

#[test]
fn empty_registry_is_fail_closed_for_keys_and_targets() {
    let registry = empty_harness_config_registry();
    let request_input = codex_request_user_input_key();

    assert!(registry.keys().is_empty());
    assert!(registry.targets().is_empty());
    assert!(!registry.declares(&request_input));
    assert!(registry.find_target("codex").is_none());
    assert_eq!(HarnessConfigRegistry::default(), registry);
}

#[test]
fn key_identity_uses_harness_then_dot_joined_path_exactly() {
    let identity = key("codex", &["features", "nested", "enabled"], "owned");

    assert_eq!(
        harness_config_key_id(&identity),
        "codex:features.nested.enabled"
    );
    assert_eq!(identity.key_id(), "codex:features.nested.enabled");
}

#[test]
fn same_path_under_different_harnesses_has_distinct_identity() {
    let codex = key("codex", &["shared", "setting"], "codex description");
    let claude = key("claude", &["shared", "setting"], "claude description");

    assert_eq!(harness_config_key_id(&codex), "codex:shared.setting");
    assert_eq!(harness_config_key_id(&claude), "claude:shared.setting");
    assert_ne!(
        harness_config_key_id(&codex),
        harness_config_key_id(&claude)
    );
}

#[test]
fn duplicate_key_identities_are_rejected_before_target_validation() {
    let first = key("codex", &["features", "enabled"], "first");
    let same_identity = key("codex", &["features", "enabled"], "different custody");
    let duplicate_target = target("codex", ConfigDocumentFormat::Toml, "config.toml");

    let error = HarnessConfigRegistry::new(
        Some(vec![first, same_identity]),
        vec![duplicate_target.clone(), duplicate_target],
    )
    .expect_err("duplicate key identities must be rejected");

    assert_eq!(error, HarnessConfigRegistryError::DuplicateKeyIdentity);
    assert_eq!(error.to_string(), "Harness config keys must be unique");
}

#[test]
fn duplicate_harness_targets_are_rejected() {
    let error = HarnessConfigRegistry::new(
        Some(Vec::new()),
        vec![
            target("codex", ConfigDocumentFormat::Toml, "first.toml"),
            target("codex", ConfigDocumentFormat::Json, "second.json"),
        ],
    )
    .expect_err("one harness cannot own two target documents");

    assert_eq!(error, HarnessConfigRegistryError::DuplicateHarnessTarget);
    assert_eq!(
        error.to_string(),
        "Each harness may declare only one config target"
    );
}

#[test]
fn declaration_and_target_lookups_preserve_hits_and_misses() {
    let declared = key("codex", &["features", "enabled"], "owned");
    let same_identity_different_custody = key("codex", &["features", "enabled"], "updated");
    let undeclared = key("codex", &["features", "other"], "not owned");
    let codex_target = target("codex", ConfigDocumentFormat::Toml, "config.toml");
    let claude_target = target("claude", ConfigDocumentFormat::Json, "settings.json");
    let registry = HarnessConfigRegistry::new(
        Some(vec![declared.clone()]),
        vec![codex_target.clone(), claude_target],
    )
    .expect("unique registry inputs must build");

    assert!(registry.declares(&declared));
    assert!(registry.declares(&same_identity_different_custody));
    assert!(!registry.declares(&undeclared));
    assert_eq!(registry.find_target("codex"), Some(&codex_target));
    assert_eq!(registry.find_target("missing"), None);
}

#[test]
fn keys_and_targets_keep_input_order() {
    let keys = vec![
        key("claude", &["first"], "first key"),
        key("codex", &["second"], "second key"),
    ];
    let targets = vec![
        target("claude", ConfigDocumentFormat::Json, "settings.json"),
        target("codex", ConfigDocumentFormat::Toml, "config.toml"),
    ];
    let registry = HarnessConfigRegistry::new(Some(keys.clone()), targets.clone())
        .expect("ordered unique inputs must build");

    assert_eq!(registry.keys(), keys.as_slice());
    assert_eq!(registry.targets(), targets.as_slice());
}

#[test]
fn omitted_keys_select_defaults_while_explicit_empty_keys_stay_empty() {
    let targets = vec![target("codex", ConfigDocumentFormat::Toml, "config.toml")];
    let with_defaults = HarnessConfigRegistry::new(None, targets.clone())
        .expect("default keys with one target must build");
    let explicitly_empty = HarnessConfigRegistry::new(Some(Vec::new()), targets)
        .expect("explicit empty keys must build");

    assert_eq!(
        with_defaults.keys(),
        default_harness_config_keys().as_slice()
    );
    assert!(explicitly_empty.keys().is_empty());
}

#[test]
fn auto_compaction_key_is_described_but_excluded_from_writable_declarations() {
    let migration_key = codex_auto_compaction_trigger_tokens_key();
    let registry = HarnessConfigRegistry::new(None, Vec::new())
        .expect("default declarations do not require a target");

    assert_eq!(migration_key.activation, ActivationTiming::NewThreads);
    assert_eq!(migration_key.harness_id, "codex");
    assert_eq!(migration_key.path, vec!["model_auto_compact_token_limit"]);
    assert_eq!(
        migration_key.description,
        "Token threshold that triggers automatic history compaction; this does not change model context capacity."
    );
    assert!(!registry.declares(&migration_key));
    assert_ne!(
        harness_config_key_id(&migration_key),
        harness_config_key_id(&codex_request_user_input_key())
    );
}
