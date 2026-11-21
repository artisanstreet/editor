use crate::{NewItem, Store};
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, ProjectId, Status,
    ThreadId,
};
use std::path::Path;

fn image_bytes() -> Vec<u8> {
    webp::Encoder::from_rgb(&[21, 72, 131], 1, 1)
        .encode_lossless()
        .to_vec()
}
fn age(path: &Path) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_hours(2))
        .unwrap();
}
async fn thread(store: &Store) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/temporary-blob-race".into(),
            name: None,
        })
        .await
        .unwrap();
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn reusing_an_old_blob_refreshes_grace_until_the_new_reference_commits() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let thread = thread(&store).await;
    let bytes = image_bytes();
    let blob = store.put_blob(bytes.clone()).await.unwrap();
    let path = store.blob_path(&blob.hash).unwrap();
    age(&path);
    assert_eq!(store.put_blob(bytes).await.unwrap(), blob);
    assert_eq!(
        store.gc_blobs().await.unwrap(),
        0,
        "reuse must protect an old blob before its new Item is persisted"
    );
    store
        .upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread,
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::Image {
                blob: blob.hash.clone(),
                caption: None,
            },
        })
        .await
        .unwrap();
    age(&path);
    assert_eq!(
        store.gc_blobs().await.unwrap(),
        0,
        "committed typed reference protects the blob after grace ends"
    );
    assert_eq!(std::fs::read(path).unwrap(), image_bytes());
}
