use artisan_catalog::{NativeContextConfig, NativeContextSelection};
use artisan_domain::{ClaudePermissionMode, CursorSpeed, GrokPermissionMode, ModelFavoriteId};

use super::*;

fn fixture() -> NativeModelCatalog {
    NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .unwrap()
}

fn profiled_policy(catalog: &NativeModelCatalog, model_id: &str) -> NativeModelPolicy {
    let mut policy = catalog.selection_policy_for_model(model_id).unwrap();
    policy.profile_id = Some("default".to_owned());
    policy
}

/// The selection the Editor sends for a policy it displays.
fn selection_for(policy: &NativeModelPolicy) -> CatalogSelection {
    let option = |value: Option<&str>| value.map(|id| CatalogOptionId::parse(id).unwrap());
    CatalogSelection {
        model_id: ModelFavoriteId::parse(policy.model_id.clone()).unwrap(),
        profile_id: policy
            .profile_id
            .clone()
            .map(|profile| EngineProfileId::parse(profile).unwrap()),
        reasoning_effort: option(
            policy
                .reasoning_effort
                .as_ref()
                .map(|value| value.id.as_str()),
        ),
        speed: option(policy.speed.as_ref().map(|value| value.id.as_str())),
        context_window: option(
            policy
                .context_window
                .as_ref()
                .map(|value| value.id.as_str()),
        ),
        permission: option(policy.permission.as_ref().map(|value| value.id.as_str())),
    }
}

#[test]
fn default_transport_can_carry_maximum_image_payload() {
    let runtime = default_runtime().unwrap();
    assert!(runtime.max_json_body_bytes().get() > 12 * 1024 * 1024 / 3 * 4);
    assert_eq!(runtime.stream_budget().get(), 3_600_000);
}

#[test]
fn a_selection_resolves_to_the_policy_the_picker_showed() {
    let catalog = fixture();
    for model_id in [
        "codex-sol",
        "claude-fable",
        "grok-4-6",
        "cursor-composer-2-5",
    ] {
        let shown = profiled_policy(&catalog, model_id);
        let resolved = policy_from_selection(&catalog, &selection_for(&shown)).unwrap();
        assert_eq!(resolved, shown, "{model_id} resolves to the shown policy");
    }
}

#[test]
fn a_selection_without_a_profile_gets_the_native_default_profile() {
    let catalog = fixture();
    let shown = catalog.selection_policy_for_model("codex-sol").unwrap();
    assert_eq!(shown.profile_id, None);
    let resolved = policy_from_selection(&catalog, &selection_for(&shown)).unwrap();
    assert_eq!(
        resolved.profile_id.as_deref(),
        Some(NATIVE_DEFAULT_PROFILE_ID)
    );
}

#[test]
fn a_selection_naming_what_the_catalog_lacks_is_refused() {
    let catalog = fixture();
    let mut unknown = selection_for(&profiled_policy(&catalog, "codex-sol"));
    unknown.model_id = ModelFavoriteId::parse("not-in-catalog").unwrap();
    assert_eq!(
        policy_from_selection(&catalog, &unknown).unwrap_err(),
        "This model is not in the host model catalog"
    );
    let mut stale_option = selection_for(&profiled_policy(&catalog, "codex-sol"));
    stale_option.reasoning_effort = Some(CatalogOptionId::parse("no-such-effort").unwrap());
    assert_eq!(
        policy_from_selection(&catalog, &stale_option).unwrap_err(),
        "This model option is no longer offered by the host model catalog"
    );
    // An absent option stays absent rather than taking a catalog default.
    let mut bare = selection_for(&profiled_policy(&catalog, "codex-sol"));
    bare.context_window = None;
    assert_eq!(
        policy_from_selection(&catalog, &bare)
            .unwrap()
            .context_window,
        None
    );
}

#[test]
fn codex_choice_builds_a_runnable_codex_selection() {
    let catalog = fixture();
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
}

#[test]
fn codex_extended_window_selects_a_window_override() {
    let catalog = fixture();
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
        selection
            .model_context_window()
            .map(artisan_domain::CodexModelContextWindow::get),
        Some(1_050_000)
    );
    // The window is configuration, never identity: the model id stays bare.
    assert_eq!(selection.model_id().unwrap().as_str(), "gpt-5.6-sol");
}

#[test]
fn claude_choice_builds_a_floored_claude_selection() {
    let catalog = fixture();
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
}

#[test]
fn claude_restricted_selects_plan_mode_at_the_write_floor() {
    let catalog = fixture();
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
    let catalog = fixture();
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
    let catalog = fixture();
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
}

#[test]
fn cursor_speed_selects_fast_delivery_or_none() {
    let catalog = fixture();
    let mut policy = profiled_policy(&catalog, "cursor-composer-2-5");
    policy.speed = None;
    let config = config_for_policy(&catalog, &policy, None).unwrap();
    let EngineSelection::Cursor(selection) = config.selection() else {
        panic!("expected a Cursor selection");
    };
    assert_eq!(selection.model_id().unwrap().as_str(), "composer-2.5");
    assert_eq!(selection.speed(), None);
    assert_eq!(selection.permission_mode(), None);
    assert_eq!(selection.permission().filesystem(), FilesystemAccess::Host);
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
    let mut catalog = fixture();
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
    let catalog = fixture();
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
    let catalog = fixture();
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

#[test]
fn a_same_engine_reselection_inherits_network_and_web_grants() {
    let catalog = fixture();
    let policy = profiled_policy(&catalog, "codex-sol");
    let first = config_for_policy(&catalog, &policy, None).unwrap();
    // Resolving the same selection against the configuration it produced
    // yields the same configuration: resolution is idempotent.
    assert_eq!(
        config_for_policy(&catalog, &policy, Some(&first)).unwrap(),
        first
    );
}
