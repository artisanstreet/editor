use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ProjectId, ThreadId,
};
use forge_store::{MAX_RESOURCE_SNAPSHOT_BYTES, Store, keep_resource_snapshot};

async fn thread(store: &Store, folder: &std::path::Path) -> anyhow::Result<ThreadId> {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: folder.display().to_string(),
            name: None,
        })
        .await?;
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await?;
    Ok(thread.id)
}

#[tokio::test]
async fn snapshots_are_kept_per_item_and_path_and_the_first_capture_wins() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store, dir.path()).await.unwrap();
    let (first, second) = (ItemId::new(), ItemId::new());
    let target = |item: &ItemId, path: &str, from_home: bool| {
        store
            .resource_snapshot_target(&thread, item, path, from_home)
            .unwrap()
    };
    let shot = target(&first, "shots/latest.png", false);
    assert!(keep_resource_snapshot(&shot, b"first").unwrap());
    assert!(!keep_resource_snapshot(&shot, b"changed").unwrap());
    assert_eq!(std::fs::read(&shot).unwrap(), b"first");
    assert_ne!(shot, target(&second, "shots/latest.png", false));
    assert_ne!(shot, target(&first, "shots/latest.png", true));
    assert_ne!(shot, target(&first, "shots/other.png", false));

    let oversized = vec![0; MAX_RESOURCE_SNAPSHOT_BYTES + 1];
    let large = target(&first, "huge.tiff", false);
    assert!(!keep_resource_snapshot(&large, &oversized).unwrap());
    assert!(!large.exists());

    let unsafe_item = ItemId("../../outside".to_owned());
    assert_eq!(
        store.resource_snapshot_target(&thread, &unsafe_item, "a.png", false),
        None
    );
}

#[tokio::test]
async fn collection_removes_only_the_snapshots_of_threads_that_are_gone() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.gc_resource_snapshots().await.unwrap(), 0);
    let live = thread(&store, dir.path()).await.unwrap();
    let gone = ThreadId::new();
    let item = ItemId::new();
    let kept = store
        .resource_snapshot_target(&live, &item, "a.png", false)
        .unwrap();
    let orphan = store
        .resource_snapshot_target(&gone, &item, "a.png", false)
        .unwrap();
    keep_resource_snapshot(&kept, b"kept").unwrap();
    keep_resource_snapshot(&orphan, b"orphan").unwrap();

    assert_eq!(store.gc_resource_snapshots().await.unwrap(), 1);
    assert_eq!(std::fs::read(&kept).unwrap(), b"kept");
    assert!(!orphan.exists());
    assert!(!dir.path().join("resources").join(gone.as_str()).exists());
}
