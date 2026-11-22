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
    let shard = dir.path().join("blobs").join(hash.as_str().split_at(2).0);
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

#[tokio::test]
async fn collection_keeps_images_of_messages_parked_in_a_saved_draft() {
    use std::time::{Duration, SystemTime};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let parked = store.put_blob(image(1)).await.unwrap().hash;
    let attached = store.put_blob(image(2)).await.unwrap().hash;
    let dropped = store.put_blob(image(3)).await.unwrap().hash;
    for hash in [&parked, &attached, &dropped] {
        std::fs::File::options()
            .write(true)
            .open(store.blob_path(hash).unwrap())
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_hours(2))
            .unwrap();
    }
    store
        .set_draft(
            "home".to_owned(),
            artisan_api::WriteDraft {
                expected_revision: 0,
                body: serde_json::json!({
                    "text": "next",
                    "images": [attached.as_str()],
                    "follow_ups": [{"text": "refused", "images": [parked.as_str()]}],
                }),
            },
        )
        .await
        .unwrap();

    store.gc_blobs().await.unwrap();

    assert!(
        store.blob_path(&parked).is_some(),
        "a message parked in a draft keeps its image"
    );
    assert!(store.blob_path(&attached).is_some());
    assert!(
        store.blob_path(&dropped).is_none(),
        "an image nothing refers to is collected"
    );
}

#[tokio::test]
async fn an_image_too_wide_is_refused_for_its_size_not_as_a_broken_webp() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let wide = webp::Encoder::from_rgb(&vec![9_u8; 9000 * 3], 9000, 1)
        .encode_lossless()
        .to_vec();
    let refused = store.put_blob(wide).await.unwrap_err();
    assert!(
        matches!(
            *refused.cause,
            forge_store::BlobFailure::TooManyPixels {
                width: 9000,
                height: 1
            }
        ),
        "a valid WebP over the side limit names its size: {refused}"
    );
}

fn image(shade: u8) -> Vec<u8> {
    webp::Encoder::from_rgb(&[shade, shade, shade], 1, 1)
        .encode_lossless()
        .to_vec()
}

#[test]
fn uploads_waiting_their_turn_leave_the_blocking_threads_free() {
    use fs2::FileExt as _;
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Store::open(dir.path()).unwrap();
        let other_process = std::fs::File::create(dir.path().join("blobs.lock")).unwrap();
        other_process.lock_exclusive().unwrap();

        let uploads: Vec<_> = (0..4)
            .map(|shade| {
                let store = store.clone();
                tokio::spawn(async move { store.put_blob(image(shade)).await })
            })
            .collect();
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::task::spawn_blocking(|| {}),
        )
        .await
        .expect("a blocking task runs beside uploads queued behind a held blob lock")
        .unwrap();

        fs2::FileExt::unlock(&other_process).unwrap();
        for upload in uploads {
            upload.await.unwrap().unwrap();
        }
    });
}
