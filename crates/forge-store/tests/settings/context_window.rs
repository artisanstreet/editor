use super::*;
use artisan_api::{
    ContextWindow, CreateProject, CreateThread, EngineConfig, Model, ProjectId, ThreadId,
};

#[tokio::test]
async fn saved_context_choice_survives_roundtrip_and_clears_on_incompatible_model() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut config = EngineConfig::new(EngineId::Codex);
    config
        .options
        .insert("context_window".to_owned(), serde_json::json!(1_000_000));
    store
        .set_setting(
            "default_engine_config".into(),
            serde_json::to_value(config).unwrap(),
        )
        .await
        .unwrap();
    let extended = Model {
        id: "extended".to_owned(),
        context_windows: vec![
            ContextWindow {
                tokens: 272_000,
                override_tokens: None,
            },
            ContextWindow {
                tokens: 1_000_000,
                override_tokens: Some(1_000_000),
            },
        ],
        ..Model::default()
    };
    let saved = store
        .set_default_model(EngineId::Codex, extended)
        .await
        .unwrap();
    assert_eq!(saved.context_window_override(), Some(1_000_000));
    let reread: EngineConfig = serde_json::from_value(
        store
            .setting("default_engine_config")
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(reread, saved);
    let standard = Model {
        id: "standard".to_owned(),
        context_windows: vec![ContextWindow {
            tokens: 400_000,
            override_tokens: None,
        }],
        ..Model::default()
    };
    let saved = store
        .set_default_model(EngineId::Codex, standard)
        .await
        .unwrap();
    assert!(!saved.options.contains_key("context_window"));
}

#[tokio::test]
async fn each_harness_remembers_its_model_and_preferences_after_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut opencode = EngineConfig::new(EngineId::OpenCode);
    let muse = Model {
        id: "gateway/muse-spark-1.3".into(),
        ..Model::default()
    };
    opencode.model = Some(muse.id.clone());
    opencode.permissions = artisan_api::Permissions::Full;
    opencode
        .options
        .insert("custom".into(), serde_json::json!(true));
    let opencode = store.set_model_config(opencode, &muse).await.unwrap();
    let claude = store
        .set_default_model(
            EngineId::Claude,
            Model {
                id: "opus".into(),
                ..Model::default()
            },
        )
        .await
        .unwrap();
    let other = store
        .set_default_model(
            EngineId::OpenCode,
            Model {
                id: "gateway/other".into(),
                ..Model::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(other.permissions, opencode.permissions);
    assert_eq!(other.options, opencode.options);
    store
        .set_model_config(opencode.clone(), &muse)
        .await
        .unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let saved = store
        .setting(artisan_api::HARNESS_CONFIGS_KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved["opencode"], serde_json::to_value(opencode).unwrap());
    assert_eq!(saved["claude"], serde_json::to_value(claude).unwrap());
}

#[tokio::test]
async fn invalid_context_config_is_rejected_before_thread_is_saved() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/context-validation".into(),
            name: None,
        })
        .await
        .unwrap();
    for (engine, value) in [
        (EngineId::Codex, serde_json::json!(0)),
        (EngineId::Claude, serde_json::json!(100_000_001_u64)),
        (EngineId::Codex, serde_json::json!("1000000")),
        (EngineId::OpenCode, serde_json::json!(1_000_000)),
    ] {
        let mut config = EngineConfig::new(engine);
        config.options.insert("context_window".to_owned(), value);
        let result = store
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project.id.clone(),
                title: None,
                config,
                parent_thread_id: None,
                spawned_by: None,
            })
            .await;
        result.expect_err("an out-of-range context window is refused");
    }
}
