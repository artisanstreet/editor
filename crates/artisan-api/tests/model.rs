use artisan_api::*;

fn model(id: &str, efforts: &[&str]) -> Model {
    Model {
        id: id.into(),
        efforts: efforts.iter().map(|id| (*id).into()).collect(),
        default_effort: Some("medium".into()),
        context_window: Some(1_000_000),
        context_windows: vec![ContextWindow {
            tokens: 1_000_000,
            override_tokens: None,
        }],
        ..Model::default()
    }
}

fn config(engine: EngineId, id: &str) -> EngineConfig {
    EngineConfig {
        model: Some(id.into()),
        ..EngineConfig::new(engine)
    }
}

#[test]
fn levels_and_special_modes_keep_the_native_execution_ids() {
    let model = model(
        "gpt-test",
        &["low", "medium", "high", "xhigh", "max", "ultra"],
    );
    let config = config(EngineId::Codex, &model.id);
    let spec = model.specification(config.engine, &[]);
    let levels = spec.visible_choices(ModelAxis::Thinking, &config, false);
    assert_eq!(
        levels
            .iter()
            .skip(1)
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        ["Light", "Medium", "High", "Extra High", "Max"]
    );
    assert_eq!(
        levels[4].binding.apply(&config).effort.as_deref(),
        Some("xhigh")
    );
    let special = spec.visible_choices(ModelAxis::Special, &config, false);
    assert_eq!(
        special[1].binding.apply(&config).effort.as_deref(),
        Some("ultra")
    );
    assert!(
        spec.visible_choices(ModelAxis::Variant, &config, false)
            .is_empty()
    );
}

#[test]
fn boolean_thinking_never_manufactures_intensity_levels() {
    let model = Model {
        id: "gateway/toggle".into(),
        variants: vec![
            Variant {
                id: "none".into(),
                label: "None".into(),
            },
            Variant {
                id: "thinking".into(),
                label: "Thinking".into(),
            },
        ],
        ..Model::default()
    };
    let config = config(EngineId::OpenCode, &model.id);
    let spec = model.specification(config.engine, &[]);
    assert!(
        spec.visible_choices(ModelAxis::Thinking, &config, false)
            .is_empty()
    );
    let activation = spec.visible_choices(ModelAxis::Activation, &config, false);
    assert_eq!(
        activation
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        ["Default", "Off", "On"]
    );
    assert_eq!(
        activation[2].binding.apply(&config).variant.as_deref(),
        Some("thinking")
    );
}

#[test]
fn unknown_native_variants_remain_variants_and_unknown_images_stay_unknown() {
    let mut model = model("gateway/experimental", &[]);
    model.variants = vec![Variant {
        id: "creative".into(),
        label: "Creative".into(),
    }];
    let spec = model.specification(EngineId::OpenCode, &[]);
    assert_eq!(spec.images, Capability::Unknown);
    assert_eq!(
        spec.controls
            .iter()
            .find(|control| control.axis == ModelAxis::Variant)
            .unwrap()
            .choices[1]
            .id,
        "creative"
    );
    model.evidence.sources.push(FieldSource {
        field: "images".into(),
        source: "native".into(),
        observed_at: 123,
    });
    assert_eq!(
        model.specification(EngineId::OpenCode, &[]).images,
        Capability::Unsupported
    );
}

#[test]
fn visibility_never_changes_execution_and_keeps_a_hidden_selection() {
    let model = model(
        "claude-opus-5-5",
        &["low", "medium", "high", "xhigh", "max", "ultra"],
    );
    let mut config = config(EngineId::Claude, &model.id);
    let spec = model.specification(config.engine, &[]);
    let visible = spec.visible_choices(ModelAxis::Thinking, &config, true);
    assert_eq!(
        visible
            .iter()
            .map(|choice| choice.id.as_str())
            .collect::<Vec<_>>(),
        ["default", "medium", "high"]
    );
    config.effort = Some("max".into());
    let visible = spec.visible_choices(ModelAxis::Thinking, &config, true);
    assert!(
        visible
            .iter()
            .any(|choice| choice.id == "max" && choice.hidden && choice.binding.selected(&config))
    );
    assert_eq!(spec.resolve(&config).effective, config);
    assert_eq!(
        spec.visible_choices(ModelAxis::Thinking, &config, false)
            .len(),
        6
    );
}

#[test]
fn opus_rule_does_not_match_other_models_or_opencode() {
    let mut model = model("claude-opus-5-50", &["low", "medium", "xhigh", "max"]);
    let config = config(EngineId::Claude, &model.id);
    assert_eq!(
        model
            .specification(config.engine, &[])
            .visible_choices(ModelAxis::Thinking, &config, true)
            .len(),
        4
    );
    model.id = "anthropic/claude-opus-5-5".into();
    let config = EngineConfig::new(EngineId::OpenCode);
    assert_eq!(
        model
            .specification(config.engine, &[])
            .visible_choices(ModelAxis::Thinking, &config, true)
            .len(),
        5
    );
}

#[test]
fn default_intent_and_invalid_selection_resolution_are_distinct() {
    let model = model("gpt-test", &["medium", "high"]);
    let default = config(EngineId::Codex, &model.id);
    assert_eq!(model.resolve_config(&default, &[]).effective.effort, None);
    let requested = EngineConfig {
        effort: Some("retired".into()),
        ..default
    };
    let resolution = model.resolve_config(&requested, &[]);
    assert_eq!(resolution.requested, requested);
    assert_eq!(resolution.effective.effort, None);
    assert_eq!(resolution.changes.len(), 1);
}

#[test]
fn claude_500k_compaction_becomes_the_500k_context_choice() {
    let model = Model {
        context_windows: vec![
            ContextWindow {
                tokens: 1_000_000,
                override_tokens: None,
            },
            ContextWindow {
                tokens: 500_000,
                override_tokens: Some(500_000),
            },
        ],
        ..model("claude-opus-5-5", &["medium"])
    };
    let mut requested = config(EngineId::Claude, &model.id);
    requested
        .options
        .insert("compaction_window".into(), 500_000.into());
    let resolved = model.resolve_config(&requested, &[]);
    assert_eq!(resolved.effective.compaction_window_override(), None);
    assert_eq!(resolved.effective.context_window_override(), Some(500_000));
    assert!(
        model
            .specification(requested.engine, &[])
            .visible_choices(ModelAxis::Context, &resolved.effective, false)
            .iter()
            .any(|choice| choice.binding == ConfigBinding::Context(Some(500_000)) && choice.label == "500K")
    );
    assert_eq!(resolved.requested, requested);
}

fn rule(scope: CustomizationScope, native_id: &str, hidden: bool) -> ModelCustomization {
    ModelCustomization {
        name: "test".into(),
        scope,
        axis: ModelAxis::Thinking,
        native_id: native_id.into(),
        label: None,
        semantic: None,
        hidden: Some(hidden),
    }
}

#[test]
fn model_customizations_override_harness_rules_without_creating_native_choices() {
    let model = model("gpt-test", &["high", "max"]);
    let config = config(EngineId::Codex, &model.id);
    let rules = vec![
        rule(
            CustomizationScope {
                harness: Some(EngineId::Codex),
                model: Some(model.id.clone()),
            },
            "max",
            false,
        ),
        rule(
            CustomizationScope {
                harness: Some(EngineId::Codex),
                model: None,
            },
            "max",
            true,
        ),
        rule(CustomizationScope::default(), "imaginary", false),
    ];
    let spec = model.specification(config.engine, &rules);
    let choices = spec.visible_choices(ModelAxis::Thinking, &config, true);
    assert!(choices.iter().any(|choice| choice.id == "max"));
    assert!(!choices.iter().any(|choice| choice.id == "imaginary"));
}

#[test]
fn custom_compaction_windows_respect_reported_capacity_and_version() {
    let model = model("claude-test", &[]);
    let mut custom = rule(CustomizationScope::default(), "250000", false);
    custom.axis = ModelAxis::Compaction;
    let config = config(EngineId::Claude, &model.id);
    let spec = model.specification(config.engine, &[custom.clone()]);
    assert!(
        spec.visible_choices(ModelAxis::Compaction, &config, false)
            .iter()
            .any(|choice| choice.binding == ConfigBinding::Compaction(Some(250_000)))
    );
    custom.native_id = "2000000".into();
    assert!(
        !model
            .specification(config.engine, &[custom])
            .visible_choices(ModelAxis::Compaction, &config, false)
            .iter()
            .any(|choice| choice.id == "2000000")
    );
    assert!(model_customizations(&serde_json::json!({"version": 2, "rules": []})).is_err());
}

#[test]
fn speeds_normalize_without_replacing_native_tier_ids_or_descriptions() {
    let mut model = model("gpt-test", &[]);
    model.speeds = ["standard", "priority", "ultrafast", "economy"]
        .iter()
        .map(|id| Speed {
            id: (*id).into(),
            label: (*id).into(),
            description: Some("Native usage information".into()),
            default: *id == "standard",
        })
        .collect();
    let config = config(EngineId::Codex, &model.id);
    let spec = model.specification(config.engine, &[]);
    let choices = spec.visible_choices(ModelAxis::Speed, &config, false);
    assert_eq!(
        choices
            .iter()
            .skip(1)
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        ["Normal", "Fast", "Superfast", "economy"]
    );
    assert_eq!(
        choices[3].binding.apply(&config).speed.as_deref(),
        Some("ultrafast")
    );
    assert_eq!(
        choices[3].description.as_deref(),
        Some("Native usage information")
    );
}

#[test]
fn preferred_intensity_survives_a_boolean_model_and_returns_on_a_graded_model() {
    let graded = model("gpt-test", &["medium", "xhigh"]);
    let initial = config(EngineId::Codex, &graded.id);
    let selected = graded
        .specification(initial.engine, &[])
        .visible_choices(ModelAxis::Thinking, &initial, false)
        .into_iter()
        .find(|choice| choice.id == "xhigh")
        .unwrap()
        .apply(&initial);
    let toggle = Model {
        id: "toggle".into(),
        variants: vec![
            Variant {
                id: "none".into(),
                label: "None".into(),
            },
            Variant {
                id: "thinking".into(),
                label: "Thinking".into(),
            },
        ],
        ..Model::default()
    };
    let binary =
        toggle.carry_thinking_preference(&selected, &config(EngineId::OpenCode, &toggle.id), &[]);
    assert_eq!(binary.variant.as_deref(), Some("thinking"));
    assert_eq!(binary.options["thinking_level"], "extra_high");
    let restored = graded.carry_thinking_preference(&binary, &initial, &[]);
    assert_eq!(restored.effort.as_deref(), Some("xhigh"));
}

#[test]
fn custom_reasoning_mapping_reclassifies_the_control_and_keeps_default_reset() {
    let model = Model {
        id: "native".into(),
        variants: vec![Variant {
            id: "deep".into(),
            label: "Deep".into(),
        }],
        ..Model::default()
    };
    let mut custom = rule(CustomizationScope::default(), "deep", false);
    custom.axis = ModelAxis::Variant;
    custom.semantic = Some(ChoiceSemantic::Intensity(ThinkingLevel::High));
    let config = config(EngineId::OpenCode, &model.id);
    let spec = model.specification(config.engine, &[custom]);
    let choices = spec.visible_choices(ModelAxis::Thinking, &config, false);
    assert_eq!(choices.len(), 2);
    let selected = choices[1].apply(&config);
    assert_eq!(selected.variant.as_deref(), Some("deep"));
    let reset = choices[0].apply(&selected);
    assert_eq!(reset.variant, None);
    assert!(!reset.options.contains_key("thinking_level"));
}

#[test]
fn discovery_failure_distinguishes_no_result_from_an_older_success() {
    let unavailable = DiscoveryState::Unknown.failed("offline");
    assert!(matches!(unavailable, DiscoveryState::Unavailable { .. }));
    let refreshing = DiscoveryState::Refreshing {
        last_success_at: Some(123),
    };
    let stale = refreshing.failed("offline");
    assert_eq!(stale.last_success_at(), Some(123));
    assert!(matches!(stale, DiscoveryState::Stale { error, .. } if error == "offline"));
}

#[test]
fn malformed_context_limits_do_not_become_supported_capabilities_or_choices() {
    for invalid in [0, 100_000_001] {
        let model = Model {
            context_window: Some(invalid),
            context_windows: vec![ContextWindow {
                tokens: invalid,
                override_tokens: Some(invalid),
            }],
            ..Model::default()
        };
        let spec = model.specification(EngineId::Codex, &[]);
        assert_eq!(spec.context_capacity, Capability::Unknown);
        assert!(
            spec.visible_choices(
                ModelAxis::Context,
                &EngineConfig::new(EngineId::Codex),
                false
            )
            .is_empty()
        );
    }
}

#[test]
fn light_has_one_native_binding_and_preserves_an_existing_minimal_selection() {
    let model = model("gpt-test", &["minimal", "low", "medium"]);
    let config = config(EngineId::Codex, &model.id);
    let spec = model.specification(config.engine, &[]);
    let choices = spec.visible_choices(ModelAxis::Thinking, &config, false);
    assert_eq!(choices.iter().filter(|choice| choice.semantic == ChoiceSemantic::Intensity(ThinkingLevel::Light)).count(), 1);
    assert_eq!(choices[1].binding.apply(&config).effort.as_deref(), Some("low"));
    let minimal = EngineConfig { effort:Some("minimal".into()), ..config };
    let choices = spec.visible_choices(ModelAxis::Thinking, &minimal, false);
    assert_eq!(choices.iter().find(|choice| choice.binding.selected(&minimal)).unwrap().label, "Light");
    assert_eq!(spec.resolve(&minimal).effective.effort.as_deref(), Some("minimal"));
}
