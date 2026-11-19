use forge_store::{SETTINGS_BYTES, SettingFailure, Store, WriteFailure};

#[tokio::test]
async fn a_setting_that_would_overflow_the_settings_budget_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let half = serde_json::Value::String("s".repeat(usize::try_from(SETTINGS_BYTES / 2).unwrap()));
    store
        .set_setting("first".into(), half.clone())
        .await
        .unwrap();
    let error = store
        .set_setting("second".into(), half.clone())
        .await
        .unwrap_err();
    assert!(
        matches!(
            *error.cause,
            SettingFailure::Write(WriteFailure::SettingsFull { .. })
        ),
        "{error:?}"
    );
    let state = store.state().await.unwrap();
    assert!(state.settings.contains_key("first"));
    assert!(
        !state.settings.contains_key("second"),
        "a refused setting is not saved"
    );
    store
        .set_setting("first".into(), serde_json::Value::Bool(true))
        .await
        .unwrap();
    store.set_setting("second".into(), half).await.unwrap();
}

#[tokio::test]
async fn opening_a_model_group_repairs_a_groups_setting_that_is_not_an_object() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .set_setting(
            artisan_api::MODEL_GROUPS_KEY.into(),
            serde_json::Value::String("damaged".into()),
        )
        .await
        .unwrap();
    store
        .set_model_group(
            artisan_api::EngineId::Claude,
            artisan_api::ModelGroup {
                name: "Opus".into(),
                open: true,
            },
        )
        .await
        .expect("the group is saved over the damaged setting");
    assert_eq!(
        store.setting(artisan_api::MODEL_GROUPS_KEY).await.unwrap(),
        Some(serde_json::json!({"claude": {"Opus": true}}))
    );
}

#[tokio::test]
async fn choosing_a_model_repairs_remembered_configs_that_are_not_an_object() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    for key in [
        artisan_api::MODEL_CONFIGS_KEY,
        artisan_api::HARNESS_CONFIGS_KEY,
    ] {
        store
            .set_setting(key.into(), serde_json::json!(["damaged"]))
            .await
            .unwrap();
    }
    let model = artisan_api::Model {
        id: "opus".into(),
        ..artisan_api::Model::default()
    };
    store
        .set_model_config(
            artisan_api::EngineConfig::new(artisan_api::EngineId::Claude),
            &model,
        )
        .await
        .expect("the model's config is remembered over the damaged settings");
    let remembered = store
        .setting(artisan_api::MODEL_CONFIGS_KEY)
        .await
        .unwrap()
        .expect("the configs are saved");
    assert!(
        artisan_api::saved_model_config(&remembered, artisan_api::EngineId::Claude, "opus")
            .is_some(),
        "{remembered}"
    );
}
