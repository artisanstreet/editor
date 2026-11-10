use super::*;

#[test]
fn legacy_service_tiers_preserve_ultrafast_and_unknown_native_ids() {
    let speeds = codex_speeds(&json!({
        "additionalSpeedTiers": ["fast", "ultrafast", "future-tier", "", null]
    }));
    assert_eq!(
        speeds
            .iter()
            .map(|speed| speed.id.as_str())
            .collect::<Vec<_>>(),
        ["standard", "fast", "ultrafast", "future-tier"]
    );
    let model = Model {
        speeds,
        ..Model::default()
    };
    let spec = model.specification(EngineId::Codex, &[]);
    let config = artisan_api::EngineConfig::new(EngineId::Codex);
    let choices = spec.visible_choices(artisan_api::ModelAxis::Speed, &config, false);
    let superfast = choices
        .iter()
        .find(|choice| choice.label == "Superfast")
        .unwrap();
    assert_eq!(superfast.apply(&config).speed.as_deref(), Some("ultrafast"));
    assert!(choices.iter().any(|choice| choice.id == "future-tier"));
}
