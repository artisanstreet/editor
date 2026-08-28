//! Exhaustive dependency-free tests for the model-selector presentation leaf.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/model_selection_presentation.rs"]
mod model_selection_presentation;

use model_selection_presentation::{
    apply_policy_patch, context_for_defaults, models_for_engine, permissions_for_selection,
    thinking_for_defaults, ContextWindowCapability, ContextWindowChoice, ModelChoice,
    ModelDefinition, PermissionLevel, PermissionMode, PermissionOption, ReasoningEffort,
    SandboxMode, SavedModelDefaults, SessionDefaults, SessionPolicy, SessionPolicyPatch,
    ThinkingCapability, ThinkingLevel,
};

fn model(
    engine: &str,
    id: &str,
    thinking: ThinkingCapability,
    context_window: Option<ContextWindowCapability>,
) -> ModelChoice {
    ModelChoice::new(engine, id, ModelDefinition::new(thinking, context_window))
}

fn disabled_model(engine: &str, id: &str) -> ModelChoice {
    ModelChoice::new(
        engine,
        id,
        ModelDefinition::disabled(
            "temporarily unavailable",
            ThinkingCapability::Supported {
                default: ThinkingLevel::Medium,
            },
            None,
        ),
    )
}

fn context(default: &str, options: &[(&str, &str)]) -> ContextWindowCapability {
    ContextWindowCapability::new(
        default,
        options
            .iter()
            .map(|(id, suffix)| ContextWindowChoice::new(*id, *suffix))
            .collect(),
    )
}

fn saved(
    model_id: &str,
    reasoning_effort: Option<ReasoningEffort>,
    context_window: Option<&str>,
) -> SavedModelDefaults {
    SavedModelDefaults::new(
        model_id,
        reasoning_effort,
        context_window.map(str::to_owned),
    )
}

fn policy() -> SessionPolicy {
    SessionPolicy {
        engine_id: "codex".to_owned(),
        model_id: Some("gpt-5.6".to_owned()),
        context_window: Some("[1m]".to_owned()),
        reasoning_effort: ReasoningEffort::High,
        permission: PermissionLevel::Autonomous,
        permission_mode: PermissionMode::OnRequest,
        sandbox_mode: SandboxMode::WorkspaceWrite,
        web_search_enabled: false,
        strict_clarification: true,
    }
}

#[test]
fn models_for_engine_preserves_input_order_and_duplicates() {
    let models = [
        model(
            "claude",
            "claude-first",
            ThinkingCapability::Unavailable,
            None,
        ),
        model("codex", "codex-first", ThinkingCapability::Native, None),
        model("codex", "codex-second", ThinkingCapability::Native, None),
        model("codex", "codex-second", ThinkingCapability::Native, None),
        model("claude", "claude-last", ThinkingCapability::Native, None),
    ];

    let filtered = models_for_engine(&models, "codex");

    assert_eq!(
        filtered
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["codex-first", "codex-second", "codex-second"]
    );
    assert!(models_for_engine(&models, "missing").is_empty());
    assert!(models_for_engine(&[], "codex").is_empty());
}

#[test]
fn permissions_are_all_and_ordered_for_enabled_models_but_empty_for_disabled() {
    let permissions = [
        PermissionOption::new(PermissionLevel::Restricted),
        PermissionOption::new(PermissionLevel::Unrestricted),
        PermissionOption::new(PermissionLevel::Restricted),
    ];
    let enabled = model("codex", "enabled", ThinkingCapability::Native, None);
    let disabled = disabled_model("codex", "disabled");

    assert_eq!(
        permissions_for_selection(&enabled, &permissions),
        permissions.to_vec()
    );
    assert_eq!(
        permissions_for_selection(&disabled, &permissions),
        Vec::<PermissionOption>::new()
    );
    assert!(permissions_for_selection(&enabled, &[]).is_empty());
}

#[test]
fn thinking_defaults_cover_supported_missing_saved_and_unknown_model_rows() {
    let supported = model(
        "codex",
        "supported",
        ThinkingCapability::Supported {
            default: ThinkingLevel::High,
        },
        None,
    );
    let defaults = SessionDefaults::new(vec![saved(
        "different-model",
        Some(ReasoningEffort::Ultra),
        None,
    )]);

    assert_eq!(
        thinking_for_defaults(&SessionDefaults::default(), &supported),
        Some(ThinkingLevel::High)
    );
    assert_eq!(
        thinking_for_defaults(&defaults, &supported),
        Some(ThinkingLevel::High)
    );
}

#[test]
fn thinking_defaults_remap_low_and_return_other_saved_efforts() {
    let model = model(
        "codex",
        "selected",
        ThinkingCapability::Supported {
            default: ThinkingLevel::Medium,
        },
        None,
    );

    for (saved_effort, expected) in [
        (ReasoningEffort::Low, ThinkingLevel::Light),
        (ReasoningEffort::Medium, ThinkingLevel::Medium),
        (ReasoningEffort::High, ThinkingLevel::High),
        (ReasoningEffort::XHigh, ThinkingLevel::XHigh),
        (ReasoningEffort::Max, ThinkingLevel::Max),
        (ReasoningEffort::Ultra, ThinkingLevel::Ultra),
    ] {
        let defaults = SessionDefaults::new(vec![saved("selected", Some(saved_effort), None)]);
        assert_eq!(
            thinking_for_defaults(&defaults, &model),
            Some(expected),
            "saved effort {saved_effort:?}"
        );
    }
}

#[test]
fn unsupported_and_native_thinking_return_none_even_with_saved_defaults() {
    let defaults = SessionDefaults::new(vec![saved("native", Some(ReasoningEffort::High), None)]);

    for capability in [ThinkingCapability::Native, ThinkingCapability::Unavailable] {
        let model = model("codex", "native", capability, None);
        assert_eq!(thinking_for_defaults(&defaults, &model), None);
    }
}

#[test]
fn context_saved_suffix_has_precedence_over_declared_default() {
    let model = model(
        "claude",
        "selected",
        ThinkingCapability::Unavailable,
        Some(context(
            "base",
            &[("base", ""), ("extended", "[1m]"), ("other", "[2m]")],
        )),
    );
    let defaults = SessionDefaults::new(vec![saved("selected", None, Some("[2m]"))]);

    assert_eq!(
        context_for_defaults(&defaults, &model),
        Some(ContextWindowChoice::new("other", "[2m]"))
    );
}

#[test]
fn context_unknown_or_missing_saved_suffix_uses_declared_default_then_first_option() {
    let with_known_default = model(
        "claude",
        "known-default",
        ThinkingCapability::Unavailable,
        Some(context(
            "extended",
            &[("base", ""), ("extended", "[1m]"), ("other", "[2m]")],
        )),
    );
    let unknown_saved = SessionDefaults::new(vec![saved("known-default", None, Some("[unknown]"))]);
    let missing_saved = SessionDefaults::default();

    assert_eq!(
        context_for_defaults(&unknown_saved, &with_known_default),
        Some(ContextWindowChoice::new("extended", "[1m]"))
    );
    assert_eq!(
        context_for_defaults(&missing_saved, &with_known_default),
        Some(ContextWindowChoice::new("extended", "[1m]"))
    );

    let unknown_default = model(
        "claude",
        "unknown-default",
        ThinkingCapability::Unavailable,
        Some(context(
            "not-an-option",
            &[("first", ""), ("second", "[1m]")],
        )),
    );
    assert_eq!(
        context_for_defaults(&SessionDefaults::default(), &unknown_default),
        Some(ContextWindowChoice::new("first", ""))
    );
}

#[test]
fn context_without_capability_or_with_empty_options_returns_none() {
    let without_capability = model("codex", "none", ThinkingCapability::Native, None);
    let empty_options = model(
        "codex",
        "empty",
        ThinkingCapability::Native,
        Some(context("missing", &[])),
    );

    assert_eq!(
        context_for_defaults(&SessionDefaults::default(), &without_capability),
        None
    );
    assert_eq!(
        context_for_defaults(&SessionDefaults::default(), &empty_options),
        None
    );
}

#[test]
fn context_saved_defaults_are_scoped_to_the_selected_model_and_first_row_wins() {
    let model = model(
        "claude",
        "selected",
        ThinkingCapability::Unavailable,
        Some(context(
            "base",
            &[("base", ""), ("extended", "[1m]"), ("other", "[2m]")],
        )),
    );
    let defaults = SessionDefaults::new(vec![
        saved("other-model", None, Some("[2m]")),
        saved("selected", None, Some("[1m]")),
        saved("selected", None, Some("[2m]")),
    ]);

    assert_eq!(
        context_for_defaults(&defaults, &model),
        Some(ContextWindowChoice::new("extended", "[1m]"))
    );
}

#[test]
fn policy_patch_replaces_supplied_axes_without_dropping_independent_axes() {
    let original = policy();
    let patch = SessionPolicyPatch {
        engine_id: Some("claude".to_owned()),
        model_id: Some("claude-sonnet".to_owned()),
        context_window: Some("[2m]".to_owned()),
        reasoning_effort: Some(ReasoningEffort::Low),
        permission: Some(PermissionLevel::Restricted),
        permission_mode: Some(PermissionMode::Never),
        sandbox_mode: Some(SandboxMode::ReadOnly),
        web_search_enabled: Some(true),
        strict_clarification: Some(false),
    };
    let patched = apply_policy_patch(&original, &patch);

    assert_eq!(
        patched,
        SessionPolicy {
            engine_id: "claude".to_owned(),
            model_id: Some("claude-sonnet".to_owned()),
            context_window: Some("[2m]".to_owned()),
            reasoning_effort: ReasoningEffort::Low,
            permission: PermissionLevel::Restricted,
            permission_mode: PermissionMode::Never,
            sandbox_mode: SandboxMode::ReadOnly,
            web_search_enabled: true,
            strict_clarification: false,
        }
    );
}

#[test]
fn empty_policy_patch_preserves_every_axis_including_optional_values() {
    let original = policy();

    assert_eq!(
        apply_policy_patch(&original, &SessionPolicyPatch::default()),
        original
    );
}
