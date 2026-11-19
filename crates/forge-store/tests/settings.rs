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
async fn a_model_group_is_not_saved_over_a_setting_that_is_not_an_object() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let foreign = serde_json::Value::String("written by something else".into());
    store
        .set_setting(artisan_api::MODEL_GROUPS_KEY.into(), foreign.clone())
        .await
        .unwrap();
    let error = store
        .set_model_group(
            artisan_api::EngineId::Claude,
            artisan_api::ModelGroup {
                name: "Opus".into(),
                open: true,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            &*error.cause,
            WriteFailure::SettingNotAnObject { key, found: "a string" }
                if key == artisan_api::MODEL_GROUPS_KEY
        ),
        "{error:?}"
    );
    assert_eq!(
        store
            .state()
            .await
            .unwrap()
            .settings
            .get(artisan_api::MODEL_GROUPS_KEY),
        Some(&foreign),
        "the refused write left the setting as it was"
    );
}
