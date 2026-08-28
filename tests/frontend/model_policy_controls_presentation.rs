//! Direct dependency-free parity tests for model policy-controls presentation.
//!
//! The production module is included by path so this harness exercises the
//! public API without Cargo, Bazel, catalog registration, or UI dependencies.

#[path = "../../modules/frontend/src/model_policy_controls_presentation.rs"]
mod model_policy_controls_presentation;

use model_policy_controls_presentation::{
    ContextWindowCapability, ContextWindowOption, Model, ModelCapabilities, ModelPolicy,
    ModelPolicyAction, ModelPolicyControlsInput, ModelPolicySelection, PermissionOption,
    SpeedOption, ThinkingCapability, ThinkingOption, admit_model_policy_action,
    project_model_policy_controls,
};

fn thinking(id: &str, group: &str) -> ThinkingOption {
    ThinkingOption::new(id, id, group, None, None)
}

fn supported(default: &str, options: &[ThinkingOption]) -> ThinkingCapability {
    ThinkingCapability::supported(default, options.to_vec())
}

fn speed(id: &str, default: bool, disabled: Option<bool>) -> SpeedOption {
    SpeedOption::new(id, id, id, None, default, disabled)
}

fn context(id: &str, native_suffix: &str) -> ContextWindowOption {
    ContextWindowOption::new(id, id, id, None, native_suffix)
}

fn context_capability(default: &str, options: &[ContextWindowOption]) -> ContextWindowCapability {
    ContextWindowCapability::new(default, options.to_vec())
}

fn permission(id: &str) -> PermissionOption {
    PermissionOption::new(id, id, id, None)
}

fn policy(context_window: Option<&str>) -> ModelPolicy {
    ModelPolicy::new(context_window.map(str::to_owned))
}

fn model(
    id: &str,
    label: &str,
    thinking: ThinkingCapability,
    speeds: &[SpeedOption],
    context: Option<ContextWindowCapability>,
) -> Model {
    Model::new(
        id,
        label,
        ModelCapabilities::new(thinking, speeds.to_vec(), context),
    )
}

#[allow(clippy::too_many_arguments)]
fn input(
    model: Model,
    selected_model_id: &str,
    thinking_level: &str,
    speed_option_id: &str,
    policy: Option<ModelPolicy>,
    permission_mode: &str,
    permission_default: Option<&str>,
    variant_options: &[Model],
    permission_options: &[PermissionOption],
) -> ModelPolicyControlsInput {
    ModelPolicyControlsInput::new(
        model,
        selected_model_id,
        thinking_level,
        speed_option_id,
        policy,
        permission_mode,
        permission_default.map(str::to_owned),
        variant_options.to_vec(),
        permission_options.to_vec(),
    )
}

fn empty_model() -> Model {
    model(
        "model",
        "Model",
        ThinkingCapability::unsupported(),
        &[],
        None,
    )
}

#[test]
fn unsupported_thinking_exposes_no_options_or_current_value() {
    assert!(!ThinkingCapability::unsupported().is_supported());
    let model = empty_model();
    let variants = [model.clone()];
    let permissions = [permission("restricted")];
    let selected_input = input(
        model,
        "model",
        "caller-level",
        "caller-speed",
        None,
        "restricted",
        None,
        &variants,
        &permissions,
    );

    let presentation = project_model_policy_controls(&selected_input);

    assert!(presentation.base_thinking_options.is_empty());
    assert!(presentation.special_thinking_options.is_empty());
    assert_eq!(presentation.current_thinking, None);
    assert!(!presentation.show_thinking);
}

#[test]
fn supported_thinking_partitions_only_exact_groups_in_original_order() {
    let thinking_options = vec![
        ThinkingOption::new(
            "base-first",
            "  Base label  ",
            "base",
            Some(String::new()),
            Some("  base advisory  ".to_owned()),
        ),
        thinking("ignored", "unknown-group"),
        ThinkingOption::new(
            "special-first",
            "Special label",
            "special",
            Some("special description".to_owned()),
            Some(String::new()),
        ),
        thinking("base-second", "base"),
        thinking("special-second", "special"),
        thinking("also-ignored", "BASE"),
    ];
    let model = model(
        "model",
        "Model label",
        supported("special-first", &thinking_options),
        &[],
        None,
    );
    let variants = [model.clone()];
    let permissions: [PermissionOption; 0] = [];
    let selected_input = input(
        model,
        "model",
        "caller level \nexact",
        "",
        None,
        "",
        Some(""),
        &variants,
        &permissions,
    );

    let presentation = project_model_policy_controls(&selected_input);

    assert_eq!(
        presentation
            .base_thinking_options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>(),
        ["base-first", "base-second"]
    );
    assert_eq!(
        presentation
            .special_thinking_options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>(),
        ["special-first", "special-second"]
    );
    assert_eq!(
        presentation.current_thinking,
        Some("caller level \nexact".to_owned())
    );
    assert!(presentation.show_thinking);
    assert_eq!(
        presentation.base_thinking_options[0].label,
        "  Base label  "
    );
    assert_eq!(
        presentation.base_thinking_options[0].description,
        Some(String::new())
    );
    assert_eq!(
        presentation.base_thinking_options[0].advisory,
        Some("  base advisory  ".to_owned())
    );
    assert_eq!(
        presentation.special_thinking_options[0].description,
        Some("special description".to_owned())
    );
    assert_eq!(
        presentation.special_thinking_options[0].advisory,
        Some(String::new())
    );
}

#[test]
fn non_selected_model_uses_thinking_default_instead_of_callers_level() {
    let thinking_options = vec![thinking("default-level", "base")];
    let model = model(
        "model",
        "Model",
        supported("default-level", &thinking_options),
        &[],
        None,
    );
    let variants = [model.clone()];
    let permissions: [PermissionOption; 0] = [];
    let input = input(
        model,
        "other-model",
        "caller-level",
        "speed",
        None,
        "permission",
        None,
        &variants,
        &permissions,
    );

    assert_eq!(
        project_model_policy_controls(&input).current_thinking,
        Some("default-level".to_owned())
    );
}

#[test]
fn supported_empty_thinking_list_still_has_the_source_current_value() {
    assert!(supported("default", &[]).is_supported());
    let model = model(
        "model",
        "Model",
        supported("capability-default", &[]),
        &[],
        None,
    );
    let variants = [model.clone()];
    let permissions: [PermissionOption; 0] = [];
    let input = input(
        model,
        "model",
        "caller-level",
        "speed",
        None,
        "permission",
        None,
        &variants,
        &permissions,
    );

    let presentation = project_model_policy_controls(&input);

    assert!(presentation.base_thinking_options.is_empty());
    assert!(presentation.special_thinking_options.is_empty());
    assert_eq!(
        presentation.current_thinking,
        Some("caller-level".to_owned())
    );
    assert!(presentation.show_thinking);
}

#[test]
fn speed_candidates_exclude_every_defined_disabled_property_and_keep_order() {
    let speeds = vec![
        SpeedOption::new(
            "defined-false",
            "Defined false",
            "",
            Some("advisory".to_owned()),
            true,
            Some(false),
        ),
        speed("defined-true", true, Some(true)),
        speed("first-enabled", false, None),
        speed("default-enabled", true, None),
        speed("chosen", false, None),
        speed("chosen", true, None),
    ];
    let model = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &speeds,
        None,
    );
    let variants = [model.clone()];
    let permissions: [PermissionOption; 0] = [];
    let selected_input = input(
        model.clone(),
        "model",
        "thinking",
        "chosen",
        None,
        "permission",
        None,
        &variants,
        &permissions,
    );
    let presentation = project_model_policy_controls(&selected_input);

    assert_eq!(
        presentation
            .speed_options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>(),
        ["first-enabled", "default-enabled", "chosen", "chosen"]
    );
    assert_eq!(presentation.current_speed, Some(speeds[4].clone()));
    assert_eq!(presentation.speed_options[2].description, "chosen");
    assert_eq!(presentation.speed_options[0].advisory, None);

    let non_selected = input(
        model.clone(),
        "other-model",
        "thinking",
        "chosen",
        None,
        "permission",
        None,
        &variants,
        &permissions,
    );
    assert_eq!(
        project_model_policy_controls(&non_selected).current_speed,
        Some(speeds[3].clone())
    );

    let unknown_selected = input(
        model,
        "model",
        "thinking",
        "missing",
        None,
        "permission",
        None,
        &variants,
        &permissions,
    );
    assert_eq!(
        project_model_policy_controls(&unknown_selected).current_speed,
        Some(speeds[3].clone())
    );
}

#[test]
fn speed_fallback_is_first_enabled_when_no_enabled_default_exists() {
    let speeds = vec![
        speed("first", false, None),
        speed("disabled-default", true, Some(false)),
    ];
    let model = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &speeds,
        None,
    );
    let variants = [model.clone()];
    let permissions: [PermissionOption; 0] = [];
    let input = input(
        model,
        "other",
        "thinking",
        "missing",
        None,
        "permission",
        None,
        &variants,
        &permissions,
    );

    assert_eq!(
        project_model_policy_controls(&input).current_speed,
        Some(speeds[0].clone())
    );
}

#[test]
fn context_uses_selected_suffix_then_default_then_first() {
    let contexts = vec![
        ContextWindowOption::new(
            "first",
            "First label",
            "first description",
            Some("first advisory".to_owned()),
            "",
        ),
        context("extended", "[1m]"),
        ContextWindowOption::new(
            "default",
            "Default label",
            "",
            Some(String::new()),
            "[default]",
        ),
    ];
    let model_value = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(context_capability("default", &contexts)),
    );
    let variants = [model_value.clone()];
    let permissions: [PermissionOption; 0] = [];

    let selected_suffix = input(
        model_value.clone(),
        "model",
        "thinking",
        "speed",
        Some(policy(Some("[1m]"))),
        "permission",
        None,
        &variants,
        &permissions,
    );
    let projected = project_model_policy_controls(&selected_suffix);
    assert_eq!(projected.current_context, Some(contexts[1].clone()));
    assert_eq!(projected.context_options, Some(contexts.clone()));
    assert!(projected.show_context);

    let present_empty_suffix = input(
        model_value,
        "model",
        "thinking",
        "speed",
        Some(policy(Some(""))),
        "permission",
        None,
        &variants,
        &permissions,
    );
    assert_eq!(
        project_model_policy_controls(&present_empty_suffix).current_context,
        Some(contexts[0].clone())
    );
}

#[test]
fn context_falls_back_from_absent_or_unknown_suffix_to_default_then_first() {
    let contexts = vec![
        context("first", ""),
        context("default", "[default]"),
        context("other", "[other]"),
    ];
    let model_value = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(context_capability("default", &contexts)),
    );
    let variants = [model_value.clone()];
    let permissions: [PermissionOption; 0] = [];

    for policy_value in [None, Some(policy(None)), Some(policy(Some("missing")))] {
        let fallback = input(
            model_value.clone(),
            "model",
            "thinking",
            "speed",
            policy_value,
            "permission",
            None,
            &variants,
            &permissions,
        );
        assert_eq!(
            project_model_policy_controls(&fallback).current_context,
            Some(contexts[1].clone())
        );
    }

    let non_selected_suffix = input(
        model_value.clone(),
        "other-model",
        "thinking",
        "speed",
        Some(policy(Some("[other]"))),
        "permission",
        None,
        &variants,
        &permissions,
    );
    assert_eq!(
        project_model_policy_controls(&non_selected_suffix).current_context,
        Some(contexts[1].clone())
    );

    let missing_default = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(context_capability("missing", &contexts)),
    );
    let missing_default_variants = [missing_default.clone()];
    let missing_default_input = input(
        missing_default,
        "model",
        "thinking",
        "speed",
        None,
        "permission",
        None,
        &missing_default_variants,
        &permissions,
    );
    assert_eq!(
        project_model_policy_controls(&missing_default_input).current_context,
        Some(contexts[0].clone())
    );
}

#[test]
fn present_empty_context_is_distinct_from_absent_context_and_has_no_current_value() {
    let absent = empty_model();
    let absent_variants = [absent.clone()];
    let permissions: [PermissionOption; 0] = [];
    let absent_input = input(
        absent,
        "model",
        "thinking",
        "speed",
        Some(policy(Some("suffix"))),
        "permission",
        None,
        &absent_variants,
        &permissions,
    );
    let absent_projection = project_model_policy_controls(&absent_input);
    assert_eq!(absent_projection.context_options, None);
    assert_eq!(absent_projection.current_context, None);
    assert!(!absent_projection.show_context);

    let empty_context_model = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(context_capability("default", &[])),
    );
    let empty_context_variants = [empty_context_model.clone()];
    let empty_context_input = input(
        empty_context_model,
        "model",
        "thinking",
        "speed",
        None,
        "permission",
        None,
        &empty_context_variants,
        &permissions,
    );
    let empty_context_projection = project_model_policy_controls(&empty_context_input);
    assert_eq!(empty_context_projection.context_options, Some(Vec::new()));
    assert_eq!(empty_context_projection.current_context, None);
    assert!(!empty_context_projection.show_context);
}

#[test]
fn permission_uses_live_mode_then_optional_default_then_first() {
    let permissions = vec![
        PermissionOption::new(
            "first",
            "First",
            "first description",
            Some("first advisory".to_owned()),
        ),
        permission("default"),
        permission("live"),
    ];
    let model = empty_model();
    let variants = [model.clone()];

    let live = input(
        model.clone(),
        "model",
        "thinking",
        "speed",
        None,
        "live",
        Some("default"),
        &variants,
        &permissions,
    );
    assert_eq!(
        project_model_policy_controls(&live).current_permission,
        Some(permissions[2].clone())
    );

    let defaulted = input(
        model.clone(),
        "model",
        "thinking",
        "speed",
        None,
        "missing",
        Some("default"),
        &variants,
        &permissions,
    );
    assert_eq!(
        project_model_policy_controls(&defaulted).current_permission,
        Some(permissions[1].clone())
    );

    let first = input(
        model.clone(),
        "model",
        "thinking",
        "speed",
        None,
        "missing",
        Some("also-missing"),
        &variants,
        &permissions,
    );
    let first_projection = project_model_policy_controls(&first);
    assert_eq!(
        first_projection.current_permission,
        Some(permissions[0].clone())
    );
    assert_eq!(first_projection.permission_options, permissions);

    let duplicate_permissions = vec![
        permission("duplicate"),
        PermissionOption::new("duplicate", "second", "second", None),
    ];
    let duplicate_input = input(
        model.clone(),
        "model",
        "thinking",
        "speed",
        None,
        "duplicate",
        None,
        &variants,
        &duplicate_permissions,
    );
    assert_eq!(
        project_model_policy_controls(&duplicate_input).current_permission,
        Some(duplicate_permissions[0].clone())
    );

    let empty_permissions: Vec<PermissionOption> = Vec::new();
    let empty_input = input(
        model,
        "model",
        "thinking",
        "speed",
        None,
        "missing",
        Some("missing"),
        &variants,
        &empty_permissions,
    );
    let empty_projection = project_model_policy_controls(&empty_input);
    assert_eq!(empty_projection.current_permission, None);
    assert!(!empty_projection.show_permission);
}

#[test]
fn variant_and_speed_visibility_boundaries_match_the_source_conditions() {
    let base_model = empty_model();
    let no_variants: Vec<Model> = Vec::new();
    let one_variant = [base_model.clone()];
    let two_variants = [base_model.clone(), base_model.clone()];
    let no_permissions: Vec<PermissionOption> = Vec::new();

    for (variants, expected) in [
        (&no_variants[..], false),
        (&one_variant[..], false),
        (&two_variants[..], true),
    ] {
        let input = input(
            base_model.clone(),
            "model",
            "thinking",
            "speed",
            None,
            "one",
            None,
            variants,
            &no_permissions,
        );
        assert_eq!(project_model_policy_controls(&input).show_variant, expected);
    }

    let one_speed = [speed("one", false, None)];
    let two_speeds = [speed("one", false, None), speed("two", false, None)];
    for (speeds, expected) in [
        (&[][..], false),
        (&one_speed[..], false),
        (&two_speeds[..], true),
    ] {
        let speed_model = model(
            "model",
            "Model",
            ThinkingCapability::Unsupported,
            speeds,
            None,
        );
        let speed_variants = [speed_model.clone()];
        let input = input(
            speed_model,
            "model",
            "thinking",
            "speed",
            None,
            "one",
            None,
            &speed_variants,
            &no_permissions,
        );
        assert_eq!(project_model_policy_controls(&input).show_speed, expected);
    }
}

#[test]
fn thinking_and_context_visibility_boundaries_match_the_source_conditions() {
    let base_model = empty_model();
    let one_variant = [base_model.clone()];
    let no_permissions: Vec<PermissionOption> = Vec::new();
    let supported_empty = model("model", "Model", supported("default", &[]), &[], None);
    let supported_variants = [supported_empty.clone()];
    let supported_input = input(
        supported_empty,
        "model",
        "caller",
        "speed",
        None,
        "one",
        None,
        &supported_variants,
        &no_permissions,
    );
    assert!(project_model_policy_controls(&supported_input).show_thinking);
    assert!(
        !project_model_policy_controls(&input(
            base_model.clone(),
            "model",
            "caller",
            "speed",
            None,
            "one",
            None,
            &one_variant,
            &no_permissions,
        ))
        .show_thinking
    );

    let context_option = vec![context("one", "")];
    let context_model = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(context_capability("one", &context_option)),
    );
    let context_variants = [context_model.clone()];
    let context_input = input(
        context_model,
        "model",
        "thinking",
        "speed",
        None,
        "one",
        None,
        &context_variants,
        &no_permissions,
    );
    assert!(project_model_policy_controls(&context_input).show_context);

    let absent_input = input(
        base_model.clone(),
        "model",
        "thinking",
        "speed",
        Some(policy(Some("suffix"))),
        "one",
        None,
        &one_variant,
        &no_permissions,
    );
    assert!(!project_model_policy_controls(&absent_input).show_context);

    let empty_context_model = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(context_capability("one", &[])),
    );
    let empty_context_variants = [empty_context_model.clone()];
    let empty_context_input = input(
        empty_context_model,
        "model",
        "thinking",
        "speed",
        None,
        "one",
        None,
        &empty_context_variants,
        &no_permissions,
    );
    assert!(!project_model_policy_controls(&empty_context_input).show_context);
}

#[test]
fn permission_visibility_boundaries_match_the_source_conditions() {
    let base_model = empty_model();
    let one_variant = [base_model.clone()];
    let no_permissions: Vec<PermissionOption> = Vec::new();
    let one_permission = [permission("one")];
    let two_permissions = [permission("one"), permission("two")];
    for (permissions, expected) in [
        (&no_permissions[..], false),
        (&one_permission[..], false),
        (&two_permissions[..], true),
    ] {
        let input = input(
            base_model.clone(),
            "model",
            "thinking",
            "speed",
            None,
            "one",
            None,
            &one_variant,
            permissions,
        );
        assert_eq!(
            project_model_policy_controls(&input).show_permission,
            expected
        );
    }
}

#[test]
fn valid_actions_use_exact_candidates_and_duplicate_ids_choose_first_match() {
    let thinking_options = vec![thinking("think", "base"), thinking("think", "special")];
    let speeds = vec![
        speed("speed", false, None),
        SpeedOption::new(
            "speed",
            "second",
            "second",
            Some("second advisory".to_owned()),
            true,
            None,
        ),
        speed("blocked", false, Some(false)),
    ];
    let contexts = vec![context("context", "first"), context("context", "second")];
    let permissions = vec![
        permission("permission"),
        PermissionOption::new("permission", "second", "second", None),
    ];
    let model_value = model(
        "model",
        "Model",
        supported("think", &thinking_options),
        &speeds,
        Some(context_capability("context", &contexts)),
    );
    let variant_one = model(
        "variant",
        "Variant one",
        ThinkingCapability::Unsupported,
        &[],
        None,
    );
    let variant_two = model(
        "variant",
        "Variant two",
        ThinkingCapability::Unsupported,
        &[],
        None,
    );
    let variants = vec![variant_one.clone(), variant_two];
    let input = input(
        model_value.clone(),
        "model",
        "thinking",
        "speed",
        None,
        "permission",
        None,
        &variants,
        &permissions,
    );

    assert_eq!(
        admit_model_policy_action(&input, ModelPolicySelection::thinking("think")),
        Some(ModelPolicyAction::Thinking {
            model: model_value.clone(),
            option: thinking_options[0].clone(),
        })
    );
    assert_eq!(
        admit_model_policy_action(&input, ModelPolicySelection::speed("speed")),
        Some(ModelPolicyAction::Speed {
            model: model_value.clone(),
            option: speeds[0].clone(),
        })
    );
    assert_eq!(
        admit_model_policy_action(&input, ModelPolicySelection::context("context")),
        Some(ModelPolicyAction::Context {
            model: model_value.clone(),
            option: contexts[0].clone(),
        })
    );
    assert_eq!(
        admit_model_policy_action(&input, ModelPolicySelection::permission("permission")),
        Some(ModelPolicyAction::Permission {
            model: model_value,
            option: permissions[0].clone(),
        })
    );
    assert_eq!(
        admit_model_policy_action(&input, ModelPolicySelection::variant("variant")),
        Some(ModelPolicyAction::Variant { model: variant_one })
    );
}

#[test]
fn invalid_actions_are_no_action_and_disabled_speed_is_not_a_candidate() {
    let thinking_options = vec![thinking("valid-thinking", "base")];
    let speeds = vec![speed("blocked", false, Some(false))];
    let model_value = model(
        "model",
        "Model",
        supported("valid-thinking", &thinking_options),
        &speeds,
        None,
    );
    let variants = [model_value.clone()];
    let permissions = [permission("valid-permission")];
    let selected_input = input(
        model_value,
        "model",
        "thinking",
        "speed",
        None,
        "valid-permission",
        None,
        &variants,
        &permissions,
    );

    for selection in [
        ModelPolicySelection::thinking("missing"),
        ModelPolicySelection::speed("blocked"),
        ModelPolicySelection::speed("missing"),
        ModelPolicySelection::context("missing"),
        ModelPolicySelection::permission("missing"),
        ModelPolicySelection::variant("missing"),
    ] {
        assert_eq!(admit_model_policy_action(&selected_input, selection), None);
    }

    let unsupported = model("model", "Model", ThinkingCapability::Unsupported, &[], None);
    let unsupported_variants = [unsupported.clone()];
    let unsupported_input = input(
        unsupported,
        "model",
        "thinking",
        "speed",
        None,
        "permission",
        None,
        &unsupported_variants,
        &[],
    );
    assert_eq!(
        admit_model_policy_action(
            &unsupported_input,
            ModelPolicySelection::thinking("anything")
        ),
        None
    );
}

#[test]
fn valid_actions_do_not_depend_on_visibility() {
    let thinking_options = vec![thinking("thinking", "base")];
    let speeds = vec![speed("speed", false, None)];
    let contexts = vec![context("context", "")];
    let permissions = vec![permission("permission")];
    let model_value = model(
        "model",
        "Model",
        supported("thinking", &thinking_options),
        &speeds,
        Some(context_capability("context", &contexts)),
    );
    let variants = [model_value.clone()];
    let selected_input = input(
        model_value,
        "model",
        "thinking",
        "speed",
        None,
        "permission",
        None,
        &variants,
        &permissions,
    );
    let presentation = project_model_policy_controls(&selected_input);
    assert!(!presentation.show_variant);
    assert!(!presentation.show_speed);
    assert!(!presentation.show_permission);

    assert!(matches!(
        admit_model_policy_action(&selected_input, ModelPolicySelection::thinking("thinking")),
        Some(ModelPolicyAction::Thinking { .. })
    ));
    assert!(matches!(
        admit_model_policy_action(&selected_input, ModelPolicySelection::speed("speed")),
        Some(ModelPolicyAction::Speed { .. })
    ));
    assert!(matches!(
        admit_model_policy_action(&selected_input, ModelPolicySelection::context("context")),
        Some(ModelPolicyAction::Context { .. })
    ));
    assert!(matches!(
        admit_model_policy_action(
            &selected_input,
            ModelPolicySelection::permission("permission")
        ),
        Some(ModelPolicyAction::Permission { .. })
    ));
    assert!(matches!(
        admit_model_policy_action(&selected_input, ModelPolicySelection::variant("model")),
        Some(ModelPolicyAction::Variant { .. })
    ));
}

#[test]
fn projection_and_action_remain_usable_after_owned_input_is_dropped() {
    let (projection, action) = {
        let thinking_options = vec![ThinkingOption::new(
            "thinking\nid",
            "  retained label  ",
            "base",
            Some("retained description".to_owned()),
            Some("retained advisory".to_owned()),
        )];
        let speeds = vec![speed("speed", false, None)];
        let contexts = vec![context("context", "")];
        let permissions = vec![permission("permission")];
        let model_value = model(
            "model\nid",
            "retained model label",
            supported("thinking\nid", &thinking_options),
            &speeds,
            Some(context_capability("context", &contexts)),
        );
        let variants = vec![model_value.clone()];
        let input = ModelPolicyControlsInput::new(
            model_value,
            "model\nid",
            "thinking\nid",
            "speed",
            Some(policy(Some(""))),
            "permission",
            None,
            variants,
            permissions,
        );
        let projection = project_model_policy_controls(&input);
        let action =
            admit_model_policy_action(&input, ModelPolicySelection::thinking("thinking\nid"));
        (projection, action)
    };

    assert_eq!(projection.model.id, "model\nid");
    assert_eq!(projection.model.label, "retained model label");
    assert_eq!(projection.current_thinking, Some("thinking\nid".to_owned()));
    assert_eq!(
        projection.base_thinking_options[0].description,
        Some("retained description".to_owned())
    );
    assert_eq!(
        projection.base_thinking_options[0].advisory,
        Some("retained advisory".to_owned())
    );
    match action {
        Some(ModelPolicyAction::Thinking { model, option }) => {
            assert_eq!(model.id, "model\nid");
            assert_eq!(option.id, "thinking\nid");
            assert_eq!(option.label, "  retained label  ");
        }
        other => panic!("unexpected action: {other:?}"),
    }
}
