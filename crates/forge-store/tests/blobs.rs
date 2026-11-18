#![expect(
    clippy::string_slice,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]

use artisan_api::BlobHash;
use forge_store::Store;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn images_saved_before_extensions_are_found_by_agents_as_webp_files() {
    let dir = tempfile::tempdir().unwrap();
    let image = webp::Encoder::from_rgb(&[7, 7, 7], 1, 1)
        .encode_lossless()
        .to_vec();
    let hash = BlobHash(hex::encode(Sha256::digest(&image)));
    let shard = dir.path().join("blobs").join(&hash.as_str()[..2]);
    std::fs::create_dir_all(&shard).unwrap();
    let old = shard.join(hash.as_str());
    std::fs::write(&old, &image).unwrap();

    let store = Store::open(dir.path()).unwrap();
    let path = store.blob_path(&hash).unwrap();
    assert_eq!(path, old.with_extension("webp"));
    assert_eq!(std::fs::read(&path).unwrap(), image);
    assert!(!old.exists());

    let again = store.put_blob(image).await.unwrap();
    assert_eq!(again.hash, hash);
    assert_eq!(std::fs::read_dir(&shard).unwrap().count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn collection_never_deletes_an_image_a_message_just_referenced() {
    use artisan_api::{
        CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ProjectId, SendMessage,
        ThreadId,
    };
    use std::time::{Duration, SystemTime};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
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
        .id;
    for round in 0..64u8 {
        let image = webp::Encoder::from_rgb(&[round, 7, 7], 1, 1)
            .encode_lossless()
            .to_vec();
        let hash = store.put_blob(image).await.unwrap().hash;
        std::fs::File::options()
            .write(true)
            .open(store.blob_path(&hash).unwrap())
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_hours(2))
            .unwrap();
        let collecting = tokio::spawn({
            let store = store.clone();
            async move { store.gc_blobs().await }
        });
        let sent = store
            .send_message(
                thread.clone(),
                SendMessage {
                    id: ItemId::new(),
                    text: "look again".into(),
                    images: vec![hash.clone()],
                    config: None,
                    draft: None,
                },
                forge_store::Acceptance::default(),
            )
            .await;
        collecting.await.unwrap().unwrap();
        if sent.is_ok() {
            assert!(
                store.blob_path(&hash).is_some(),
                "round {round}: an accepted message lost its image"
            );
        }
    }
}
