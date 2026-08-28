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

fn thinking(id: &'static str, group: &'static str) -> ThinkingOption<'static> {
    ThinkingOption::new(id, id, group, None, None)
}

fn speed(id: &'static str, default: bool, disabled: Option<bool>) -> SpeedOption<'static> {
    SpeedOption::new(id, id, id, None, default, disabled)
}

fn context(id: &'static str, native_suffix: &'static str) -> ContextWindowOption<'static> {
    ContextWindowOption::new(id, id, id, None, native_suffix)
}

fn permission(id: &'static str) -> PermissionOption<'static> {
    PermissionOption::new(id, id, id, None)
}

fn model<'a>(
    id: &'a str,
    label: &'a str,
    thinking: ThinkingCapability<'a>,
    speeds: &'a [SpeedOption<'a>],
    context: Option<ContextWindowCapability<'a>>,
) -> Model<'a> {
    Model::new(id, label, ModelCapabilities::new(thinking, speeds, context))
}

#[allow(clippy::too_many_arguments)]
fn input<'a>(
    model: Model<'a>,
    selected_model_id: &'a str,
    thinking_level: &'a str,
    speed_option_id: &'a str,
    policy: Option<ModelPolicy<'a>>,
    permission_mode: &'a str,
    permission_default: Option<&'a str>,
    variant_options: &'a [Model<'a>],
    permission_options: &'a [PermissionOption<'a>],
) -> ModelPolicyControlsInput<'a> {
    ModelPolicyControlsInput {
        model,
        selected_model_id,
        thinking_level,
        speed_option_id,
        policy,
        permission_mode,
        permission_default,
        variant_options,
        permission_options,
    }
}

fn empty_model() -> Model<'static> {
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
    let variants = [model];
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
    let thinking_options = [
        ThinkingOption::new(
            "base-first",
            "  Base label  ",
            "base",
            Some(""),
            Some("  base advisory  "),
        ),
        thinking("ignored", "unknown-group"),
        ThinkingOption::new(
            "special-first",
            "Special label",
            "special",
            Some("special description"),
            Some(""),
        ),
        thinking("base-second", "base"),
        thinking("special-second", "special"),
        thinking("also-ignored", "BASE"),
    ];
    let model = model(
        "model",
        "Model label",
        ThinkingCapability::supported("special-first", &thinking_options),
        &[],
        None,
    );
    let variants = [model];
    let permissions: [PermissionOption<'static>; 0] = [];
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
            .map(|option| option.id)
            .collect::<Vec<_>>(),
        ["base-first", "base-second"]
    );
    assert_eq!(
        presentation
            .special_thinking_options
            .iter()
            .map(|option| option.id)
            .collect::<Vec<_>>(),
        ["special-first", "special-second"]
    );
    assert_eq!(presentation.current_thinking, Some("caller level \nexact"));
    assert!(presentation.show_thinking);
    assert_eq!(
        presentation.base_thinking_options[0].label,
        "  Base label  "
    );
    assert_eq!(presentation.base_thinking_options[0].description, Some(""));
    assert_eq!(
        presentation.base_thinking_options[0].advisory,
        Some("  base advisory  ")
    );
    assert_eq!(
        presentation.special_thinking_options[0].description,
        Some("special description")
    );
    assert_eq!(presentation.special_thinking_options[0].advisory, Some(""));
}

#[test]
fn non_selected_model_uses_thinking_default_instead_of_callers_level() {
    let thinking_options = [thinking("default-level", "base")];
    let model = model(
        "model",
        "Model",
        ThinkingCapability::supported("default-level", &thinking_options),
        &[],
        None,
    );
    let variants = [model];
    let permissions: [PermissionOption<'static>; 0] = [];
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
        Some("default-level")
    );
}

#[test]
fn supported_empty_thinking_list_still_has_the_source_current_value() {
    assert!(ThinkingCapability::supported("default", &[]).is_supported());
    let model = model(
        "model",
        "Model",
        ThinkingCapability::supported("capability-default", &[]),
        &[],
        None,
    );
    let variants = [model];
    let permissions: [PermissionOption<'static>; 0] = [];
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
    assert_eq!(presentation.current_thinking, Some("caller-level"));
    assert!(presentation.show_thinking);
}

#[test]
fn speed_candidates_exclude_every_defined_disabled_property_and_keep_order() {
    let speeds = [
        SpeedOption::new(
            "defined-false",
            "Defined false",
            "",
            Some("advisory"),
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
    let variants = [model];
    let permissions: [PermissionOption<'static>; 0] = [];
    let selected_input = input(
        model,
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
            .map(|option| option.id)
            .collect::<Vec<_>>(),
        ["first-enabled", "default-enabled", "chosen", "chosen"]
    );
    assert_eq!(presentation.current_speed, Some(speeds[4]));
    assert_eq!(presentation.speed_options[2].description, "chosen");
    assert_eq!(presentation.speed_options[0].advisory, None);

    let non_selected = input(
        model,
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
        Some(speeds[3])
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
        Some(speeds[3])
    );
}

#[test]
fn speed_fallback_is_first_enabled_when_no_enabled_default_exists() {
    let speeds = [
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
    let variants = [model];
    let permissions: [PermissionOption<'static>; 0] = [];
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
        Some(speeds[0])
    );
}

#[test]
fn context_uses_selected_suffix_then_default_then_first() {
    let contexts = [
        ContextWindowOption::new(
            "first",
            "First label",
            "first description",
            Some("first advisory"),
            "",
        ),
        context("extended", "[1m]"),
        ContextWindowOption::new("default", "Default label", "", Some(""), "[default]"),
    ];
    let capability = ContextWindowCapability::new("default", &contexts);
    let model_value = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(capability),
    );
    let variants = [model_value];
    let permissions: [PermissionOption<'static>; 0] = [];

    let selected_suffix = input(
        model_value,
        "model",
        "thinking",
        "speed",
        Some(ModelPolicy::new(Some("[1m]"))),
        "permission",
        None,
        &variants,
        &permissions,
    );
    let projected = project_model_policy_controls(&selected_suffix);
    assert_eq!(projected.current_context, Some(contexts[1]));
    assert_eq!(projected.context_options, Some(contexts.to_vec()));
    assert!(projected.show_context);

    let present_empty_suffix = input(
        model_value,
        "model",
        "thinking",
        "speed",
        Some(ModelPolicy::new(Some(""))),
        "permission",
        None,
        &variants,
        &permissions,
    );
    assert_eq!(
        project_model_policy_controls(&present_empty_suffix).current_context,
        Some(contexts[0])
    );
}

#[test]
fn context_falls_back_from_absent_or_unknown_suffix_to_default_then_first() {
    let contexts = [
        context("first", ""),
        context("default", "[default]"),
        context("other", "[other]"),
    ];
    let model_value = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(ContextWindowCapability::new("default", &contexts)),
    );
    let variants = [model_value];
    let permissions: [PermissionOption<'static>; 0] = [];

    for policy in [
        None,
        Some(ModelPolicy::new(None)),
        Some(ModelPolicy::new(Some("missing"))),
    ] {
        let fallback = input(
            model_value,
            "model",
            "thinking",
            "speed",
            policy,
            "permission",
            None,
            &variants,
            &permissions,
        );
        assert_eq!(
            project_model_policy_controls(&fallback).current_context,
            Some(contexts[1])
        );
    }

    let non_selected_suffix = input(
        model_value,
        "other-model",
        "thinking",
        "speed",
        Some(ModelPolicy::new(Some("[other]"))),
        "permission",
        None,
        &variants,
        &permissions,
    );
    assert_eq!(
        project_model_policy_controls(&non_selected_suffix).current_context,
        Some(contexts[1])
    );

    let missing_default = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(ContextWindowCapability::new("missing", &contexts)),
    );
    let missing_default_variants = [missing_default];
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
        Some(contexts[0])
    );
}

#[test]
fn present_empty_context_is_distinct_from_absent_context_and_has_no_current_value() {
    let absent = empty_model();
    let absent_variants = [absent];
    let permissions: [PermissionOption<'static>; 0] = [];
    let absent_input = input(
        absent,
        "model",
        "thinking",
        "speed",
        Some(ModelPolicy::new(Some("suffix"))),
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
        Some(ContextWindowCapability::new("default", &[])),
    );
    let empty_context_variants = [empty_context_model];
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
    let permissions = [
        PermissionOption::new(
            "first",
            "First",
            "first description",
            Some("first advisory"),
        ),
        permission("default"),
        permission("live"),
    ];
    let model = empty_model();
    let variants = [model];

    let live = input(
        model,
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
        Some(permissions[2])
    );

    let defaulted = input(
        model,
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
        Some(permissions[1])
    );

    let first = input(
        model,
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
    assert_eq!(first_projection.current_permission, Some(permissions[0]));
    assert_eq!(first_projection.permission_options, permissions.to_vec());

    let duplicate_permissions = [
        permission("duplicate"),
        PermissionOption::new("duplicate", "second", "second", None),
    ];
    let duplicate_input = input(
        model,
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
        Some(duplicate_permissions[0])
    );

    let empty_permissions: [PermissionOption<'static>; 0] = [];
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
    let no_variants: [Model<'static>; 0] = [];
    let one_variant = [base_model];
    let two_variants = [base_model, base_model];
    let no_permissions: [PermissionOption<'static>; 0] = [];

    for (variants, expected) in [
        (&no_variants[..], false),
        (&one_variant[..], false),
        (&two_variants[..], true),
    ] {
        let input = input(
            base_model,
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
        let speed_variants = [speed_model];
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
    let one_variant = [base_model];
    let no_permissions: [PermissionOption<'static>; 0] = [];
    let supported_empty = model(
        "model",
        "Model",
        ThinkingCapability::supported("default", &[]),
        &[],
        None,
    );
    let supported_variants = [supported_empty];
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
            base_model,
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

    let context_option = [context("one", "")];
    let context_model = model(
        "model",
        "Model",
        ThinkingCapability::Unsupported,
        &[],
        Some(ContextWindowCapability::new("one", &context_option)),
    );
    let context_variants = [context_model];
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
        base_model,
        "model",
        "thinking",
        "speed",
        Some(ModelPolicy::new(Some("suffix"))),
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
        Some(ContextWindowCapability::new("one", &[])),
    );
    let empty_context_variants = [empty_context_model];
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
    let one_variant = [base_model];
    let no_permissions: [PermissionOption<'static>; 0] = [];
    let one_permission = [permission("one")];
    let two_permissions = [permission("one"), permission("two")];
    for (permissions, expected) in [
        (&no_permissions[..], false),
        (&one_permission[..], false),
        (&two_permissions[..], true),
    ] {
        let input = input(
            base_model,
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
    let thinking_options = [thinking("think", "base"), thinking("think", "special")];
    let speeds = [
        speed("speed", false, None),
        SpeedOption::new(
            "speed",
            "second",
            "second",
            Some("second advisory"),
            true,
            None,
        ),
        speed("blocked", false, Some(false)),
    ];
    let contexts = [context("context", "first"), context("context", "second")];
    let permissions = [
        permission("permission"),
        PermissionOption::new("permission", "second", "second", None),
    ];
    let model_value = model(
        "model",
        "Model",
        ThinkingCapability::supported("think", &thinking_options),
        &speeds,
        Some(ContextWindowCapability::new("context", &contexts)),
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
    let variants = [variant_one, variant_two];
    let input = input(
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

    assert_eq!(
        admit_model_policy_action(&input, ModelPolicySelection::Thinking { id: "think" }),
        Some(ModelPolicyAction::Thinking {
            model: model_value,
            option: thinking_options[0],
        })
    );
    assert_eq!(
        admit_model_policy_action(&input, ModelPolicySelection::Speed { id: "speed" }),
        Some(ModelPolicyAction::Speed {
            model: model_value,
            option: speeds[0],
        })
    );
    assert_eq!(
        admit_model_policy_action(&input, ModelPolicySelection::Context { id: "context" }),
        Some(ModelPolicyAction::Context {
            model: model_value,
            option: contexts[0],
        })
    );
    assert_eq!(
        admit_model_policy_action(
            &input,
            ModelPolicySelection::Permission { id: "permission" }
        ),
        Some(ModelPolicyAction::Permission {
            model: model_value,
            option: permissions[0],
        })
    );
    assert_eq!(
        admit_model_policy_action(&input, ModelPolicySelection::Variant { id: "variant" }),
        Some(ModelPolicyAction::Variant { model: variant_one })
    );
}

#[test]
fn invalid_actions_are_no_action_and_disabled_speed_is_not_a_candidate() {
    let thinking_options = [thinking("valid-thinking", "base")];
    let speeds = [speed("blocked", false, Some(false))];
    let model_value = model(
        "model",
        "Model",
        ThinkingCapability::supported("valid-thinking", &thinking_options),
        &speeds,
        None,
    );
    let variants = [model_value];
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
        ModelPolicySelection::Thinking { id: "missing" },
        ModelPolicySelection::Speed { id: "blocked" },
        ModelPolicySelection::Speed { id: "missing" },
        ModelPolicySelection::Context { id: "missing" },
        ModelPolicySelection::Permission { id: "missing" },
        ModelPolicySelection::Variant { id: "missing" },
    ] {
        assert_eq!(admit_model_policy_action(&selected_input, selection), None);
    }

    let unsupported = model("model", "Model", ThinkingCapability::Unsupported, &[], None);
    let unsupported_variants = [unsupported];
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
            ModelPolicySelection::Thinking { id: "anything" }
        ),
        None
    );
}

#[test]
fn valid_actions_do_not_depend_on_visibility() {
    let thinking_options = [thinking("thinking", "base")];
    let speeds = [speed("speed", false, None)];
    let contexts = [context("context", "")];
    let permissions = [permission("permission")];
    let model_value = model(
        "model",
        "Model",
        ThinkingCapability::supported("thinking", &thinking_options),
        &speeds,
        Some(ContextWindowCapability::new("context", &contexts)),
    );
    let variants = [model_value];
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
        admit_model_policy_action(
            &selected_input,
            ModelPolicySelection::Thinking { id: "thinking" }
        ),
        Some(ModelPolicyAction::Thinking { .. })
    ));
    assert!(matches!(
        admit_model_policy_action(&selected_input, ModelPolicySelection::Speed { id: "speed" }),
        Some(ModelPolicyAction::Speed { .. })
    ));
    assert!(matches!(
        admit_model_policy_action(
            &selected_input,
            ModelPolicySelection::Context { id: "context" }
        ),
        Some(ModelPolicyAction::Context { .. })
    ));
    assert!(matches!(
        admit_model_policy_action(
            &selected_input,
            ModelPolicySelection::Permission { id: "permission" }
        ),
        Some(ModelPolicyAction::Permission { .. })
    ));
    assert!(matches!(
        admit_model_policy_action(
            &selected_input,
            ModelPolicySelection::Variant { id: "model" }
        ),
        Some(ModelPolicyAction::Variant { .. })
    ));
}
