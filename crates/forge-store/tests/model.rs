use artisan_api::{ContextWindow, EngineConfig, EngineId, MODEL_CUSTOMIZATIONS_KEY, Model};
use forge_store::Store;
use serde_json::json;

#[tokio::test]
async fn custom_compaction_survives_persistence_and_invalid_customizations_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let rules = json!({"version":1,"rules":[{
        "name":"Smaller working conversation", "scope":{"harness":"claude","model":"claude-test"},
        "axis":"compaction", "native_id":"250000", "label":null, "semantic":null, "hidden":null
    }]});
    store
        .set_setting(MODEL_CUSTOMIZATIONS_KEY.into(), rules.clone())
        .await
        .unwrap();
    let model = Model {
        id: "claude-test".into(),
        context_window: Some(1_000_000),
        context_windows: vec![ContextWindow {
            tokens: 1_000_000,
            override_tokens: None,
        }],
        ..Model::default()
    };
    let mut requested = EngineConfig {
        model: Some(model.id.clone()),
        ..EngineConfig::new(EngineId::Claude)
    };
    requested
        .options
        .insert("compaction_window".into(), 250_000.into());
    let saved = store
        .set_model_config(requested.clone(), &model)
        .await
        .unwrap();
    assert_eq!(saved, requested);
    assert!(
        store
            .set_setting(
                MODEL_CUSTOMIZATIONS_KEY.into(),
                json!({"version":99,"rules":[]})
            )
            .await
            .is_err()
    );
    assert_eq!(
        store.setting(MODEL_CUSTOMIZATIONS_KEY).await.unwrap(),
        Some(rules)
    );
}

#[tokio::test]
async fn saving_default_does_not_pin_a_reported_effort_or_variant() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let model = Model {
        id: "gpt-test".into(),
        efforts: vec!["medium".into(), "high".into()],
        default_effort: Some("medium".into()),
        ..Model::default()
    };
    let requested = EngineConfig::new(EngineId::Codex);
    let saved = store.set_model_config(requested, &model).await.unwrap();
    assert_eq!(saved.effort, None);
    let setting = store
        .setting(artisan_api::MODEL_CONFIGS_KEY)
        .await
        .unwrap()
        .unwrap();
    let restored = artisan_api::saved_model_config(&setting, EngineId::Codex, &model.id).unwrap();
    assert_eq!(restored.effort, None);
}
