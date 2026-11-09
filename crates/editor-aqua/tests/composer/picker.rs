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
    assert_eq!(options.len(), 3);
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
fn choosing_default_clears_an_explicit_effort_and_its_cross_model_preference() {
    let picker = PickerState::default();
    let model = Model {
        efforts: vec!["high".into()],
        ..Model::default()
    };
    let mut config = EngineConfig {
        effort: Some("high".into()),
        ..EngineConfig::new(EngineId::Codex)
    };
    config
        .options
        .insert("thinking_level".into(), "high".into());
    let options = picker.axis_options(Axis::Thinking, &config, Some(&model));
    assert_eq!(options[0].apply.effort, None);
    assert!(!options[0].apply.options.contains_key("thinking_level"));
}
