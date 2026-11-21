use super::*;
use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId};

async fn live_thread(data: &std::path::Path) -> ThreadId {
    let store = forge_store::Store::open(data).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/sweep".into(),
            name: None,
        })
        .await
        .unwrap();
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            title: None,
            config: EngineConfig::new(EngineId::Claude),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap()
        .id
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_gateway_sweeps_leftovers_before_any_backend_runs() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let thread = live_thread(data).await;
    let images = data.join("run-images");
    let kept = images.join(thread.as_str().replace('-', ""));
    let orphan = images.join(ThreadId::new().as_str().replace('-', ""));
    for folder in [&kept, &orphan] {
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(folder.join("image.png"), b"png").unwrap();
    }
    let server = data
        .join("engines")
        .join(EngineId::OpenCode.as_str())
        .join("home.artisan")
        .join("opencode")
        .join("server-left");
    std::fs::create_dir_all(&server).unwrap();

    let config = crate::ServeConfig {
        data_dir: data.to_path_buf(),
        listen: "127.0.0.1:0".parse().unwrap(),
    };
    let registry = Registry::open(data).unwrap();
    let bound = binding(&config, &registry, None).await.unwrap();

    assert!(
        kept.join("image.png").exists(),
        "the sweep removed a live thread's images"
    );
    assert!(
        !orphan.exists(),
        "the gateway left a removed thread's images"
    );
    assert!(
        !server.exists(),
        "the gateway left an OpenCode server folder"
    );
    drop(bound);
}
