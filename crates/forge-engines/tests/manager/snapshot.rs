use super::*;
#[tokio::test]
async fn download_progress_shares_catalog_allocations_and_preserves_prior_snapshots() {
    let folder = tempfile::tempdir().unwrap();
    let engines = Engines::new(folder.path());
    let engine = EngineId::Codex;
    lock(&engines.slot(engine).models).listing = Some(CachedListing::from(catalog::Listing {
        models: vec![artisan_api::Model {
            id: "fixture-model1".into(),
            ..artisan_api::Model::default()
        }],
        default: Some("fixture-model1".into()),
        ..catalog::Listing::default()
    }));
    *lock(&engines.slot(engine).versions) = vec![PublishedVersion {
        version: "1.0.0".into(),
        installed: true,
        current: true,
        unsupported: false,
    }]
    .into();
    let before = engines.status(engine);
    let mut events = engines.subscribe();
    engines.apply(
        engine,
        &Event::Start {
            version: "2.0.0".into(),
        },
    );
    engines.apply(
        engine,
        &Event::Progress {
            received: 100,
            total: Some(1000),
        },
    );
    let EngineChange::Full(started) = events.recv().await.unwrap() else {
        panic!("start must publish metadata");
    };
    let EngineChange::Install {
        engine: progress_engine,
        install: progress_install,
    } = events.recv().await.unwrap()
    else {
        panic!("progress must publish only installation");
    };
    assert_eq!(progress_engine, engine);
    let progress = engines.status(engine);
    assert!(Arc::ptr_eq(&before.models, &started.models));
    assert!(Arc::ptr_eq(&before.models, &progress.models));
    assert!(Arc::ptr_eq(&before.versions, &progress.versions));
    assert!(matches!(
        progress_install,
        artisan_api::InstallState::Downloading { received: 100, .. }
    ));
    engines.reconcile_install(engine);
    assert!(before.versions[0].installed);
    assert!(!engines.status(engine).versions[0].installed);
}
