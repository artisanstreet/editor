use artisan_api::{EngineConfig, EngineId, Model};
use editor_aqua::composer::picker::{Axis, AxisKey, AxisKeyResult, PickerState};

#[test]
fn keyboard_selection_uses_filtered_options_and_keeps_existing_hidden_choices() {
    let mut picker = PickerState::default();
    picker.hide_bad_model_options = true;
    let model = Model {
        id: "claude-opus-5-5".into(),
        efforts: ["low", "medium", "high", "xhigh", "max"]
            .map(str::to_owned)
            .into(),
        ..Model::default()
    };
    let config = EngineConfig::new(EngineId::Claude);
    let options = picker.axis_options(Axis::Thinking, &config, Some(&model));
    assert_eq!(options.len(), 2);
    picker.axis_key(AxisKey::End, 0, options.len());
    let AxisKeyResult::Choose(index) = picker.axis_key(AxisKey::Accept, 0, options.len()) else {
        panic!("expected a choice")
    };
    assert_eq!(options[index].apply.effort.as_deref(), Some("high"));
    let config = EngineConfig {
        effort: Some("max".into()),
        ..config
    };
    let options = picker.axis_options(Axis::Thinking, &config, Some(&model));
    let selected = options.iter().find(|option| option.selected).unwrap();
    assert_eq!(selected.apply.effort.as_deref(), Some("max"));
    assert!(
        selected
            .description
            .as_deref()
            .unwrap()
            .contains("hidden by preference")
    );
}

#[test]
fn axes_offer_only_concrete_choices_and_resolve_inherited_selections() {
    use artisan_api::{ContextWindow, Speed};
    let picker = PickerState::default();
    let model = Model {
        efforts: vec!["low".into(), "medium".into(), "high".into()],
        default_effort: Some("low".into()),
        speeds: vec![
            Speed {
                id: "standard".into(),
                label: "Normal".into(),
                description: None,
                default: true,
            },
            Speed {
                id: "fast".into(),
                label: "Fast".into(),
                description: None,
                default: false,
            },
        ],
        context_windows: vec![ContextWindow {
            tokens: 500_000,
            override_tokens: None,
        }],
        ..Model::default()
    };
    let config = EngineConfig::new(EngineId::Codex);
    let coverage = Model {
        efforts: vec!["medium".into(), "ultra".into()],
        variants: ["none", "thinking", "creative"]
            .map(|id| artisan_api::Variant {
                id: id.into(),
                label: id.into(),
            })
            .into(),
        context_window: Some(1_000_000),
        ..model.clone()
    };
    let claude = EngineConfig::new(EngineId::Claude);
    for axis in Axis::ALL {
        let options = picker.axis_options(axis, &claude, Some(&coverage));
        assert!(!options.is_empty(), "missing coverage for {axis:?}");
        assert!(
            options
                .iter()
                .all(|option| !option.label.starts_with("Default"))
        );
        assert!(options.iter().all(|option| option.description.as_deref()
            != Some("Use the harness default without pinning a value.")));
    }
    for (axis, expected, count) in [
        (Axis::Thinking, "Light", 3),
        (Axis::Speed, "Normal", 2),
        (Axis::Context, "500K", 1),
    ] {
        let options = picker.axis_options(axis, &config, Some(&model));
        assert_eq!(options.len(), count);
        let selected = options.iter().find(|option| option.selected).unwrap();
        assert_eq!(selected.label, expected);
        assert_eq!(selected.apply.variant, config.variant);
        assert_eq!(selected.apply.permissions, config.permissions);
    }
    let selected = picker
        .axis_options(Axis::Thinking, &config, Some(&model))
        .remove(0)
        .apply;
    assert_eq!(selected.effort.as_deref(), Some("low"));
    assert_eq!(selected.speed, None);
    assert_eq!(selected.options["thinking_level"], "light");
    let explicit = EngineConfig {
        effort: Some("high".into()),
        ..config
    };
    let options = picker.axis_options(Axis::Thinking, &explicit, Some(&model));
    assert_eq!(
        options.iter().find(|option| option.selected).unwrap().label,
        "High"
    );
}
