#![expect(
    clippy::string_slice,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
use artisan_api::{
    BlobHash, CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ProjectId, RunId,
    SendMessage, ThreadId,
};
use forge_store::{Acceptance, Delivered, InputFailure, SendRejected, Store};

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn thread(store: &Store) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work".into(),
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
fn message(text: String, images: Vec<BlobHash>) -> SendMessage {
    SendMessage {
        id: ItemId::new(),
        text,
        images,
        config: None,
        draft: None,
    }
}
#[tokio::test]
async fn bounded_input_snapshot_retains_the_unloaded_observed_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store).await;
    let mut last = 0;
    for _ in 0..40 {
        last = store
            .send_message(
                thread.clone(),
                message("queued".into(), Vec::new()),
                Acceptance::default(),
            )
            .await
            .unwrap()
            .0
            .seq;
    }
    let pending = store.pending_inputs(&thread).await.unwrap();
    assert_eq!(pending.items.len(), 32);
    assert_eq!(pending.cutoff, last);
    assert!(pending.cutoff > pending.items.last().unwrap().seq);
    let ids = pending.items.iter().map(|item| item.id.clone()).collect();
    store
        .accept_inputs(thread.clone(), RunId::new(), ids, Delivered::Initial)
        .await
        .unwrap();
    let rest = store.pending_inputs(&thread).await.unwrap();
    assert_eq!(rest.items.len(), 8);
    assert_eq!(rest.cutoff, last);
}
#[tokio::test]
async fn byte_budget_preserves_one_oversized_message_without_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store).await;
    let text = "x".repeat(5 * 1024 * 1024);
    let first = store
        .send_message(
            thread.clone(),
            message(text.clone(), Vec::new()),
            Acceptance::default(),
        )
        .await
        .unwrap()
        .0;
    let later = store
        .send_message(
            thread.clone(),
            message("tail".into(), Vec::new()),
            Acceptance::default(),
        )
        .await
        .unwrap()
        .0;
    let pending = store.pending_inputs(&thread).await.unwrap();
    assert_eq!(pending.items.len(), 1);
    assert_eq!(pending.items[0], first);
    assert_eq!(pending.cutoff, later.seq);
    store
        .accept_inputs(
            thread.clone(),
            RunId::new(),
            vec![first.id],
            Delivered::Initial,
        )
        .await
        .unwrap();
    assert_eq!(store.pending_inputs(&thread).await.unwrap().items, [later]);
}
#[tokio::test]
async fn input_admission_rejects_resource_excess_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store).await;
    let head = store.head();
    let too_large = store
        .send_message(
            thread.clone(),
            message("x".repeat(32 * 1024 * 1024 + 1), Vec::new()),
            Acceptance::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        *too_large.cause,
        InputFailure::Rejected(SendRejected::TextTooLarge { .. })
    ));
    assert_eq!(store.head(), head);
    let hash = BlobHash("a".repeat(64));
    let missing = store
        .send_message(
            thread.clone(),
            message("missing".into(), vec![hash.clone()]),
            Acceptance::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        *missing.cause,
        InputFailure::Rejected(SendRejected::MissingBlob(_))
    ));
    assert_eq!(store.head(), head);
    let count = store
        .send_message(
            thread.clone(),
            message("too many".into(), vec![hash; 129]),
            Acceptance::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        *count.cause,
        InputFailure::Rejected(SendRejected::TooManyImages { count: 129 })
    ));
    assert_eq!(store.head(), head);
    let mut hashes = Vec::new();
    for n in 0..5 {
        let hash = BlobHash(format!("{n:064x}"));
        let path = dir
            .path()
            .join("blobs")
            .join(&hash.as_str()[..2])
            .join(hash.as_str())
            .with_extension("webp");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(path)
            .unwrap()
            .set_len(32 * 1024 * 1024)
            .unwrap();
        hashes.push(hash);
    }
    let total = store
        .send_message(
            thread,
            message("too much image data".into(), hashes),
            Acceptance::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        *total.cause,
        InputFailure::Rejected(SendRejected::ImagesTooLarge { .. })
    ));
    assert_eq!(store.head(), head);
}

#[tokio::test]
async fn initial_batch_image_metadata_budget_applies_across_messages() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let thread = thread(&store).await;
    let mut last = 0;
    for n in 0..5 {
        let hash = BlobHash(format!("{n:064x}"));
        let path = dir
            .path()
            .join("blobs")
            .join(&hash.as_str()[..2])
            .join(hash.as_str())
            .with_extension("webp");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(path)
            .unwrap()
            .set_len(32 * 1024 * 1024)
            .unwrap();
        last = store
            .send_message(
                thread.clone(),
                message(format!("image-{n}"), vec![hash]),
                Acceptance::default(),
            )
            .await
            .unwrap()
            .0
            .seq;
    }
    let pending = store.pending_inputs(&thread).await.unwrap();
    assert_eq!(pending.items.len(), 4);
    assert_eq!(pending.cutoff, last);
    store
        .accept_inputs(
            thread.clone(),
            RunId::new(),
            pending.items.into_iter().map(|item| item.id).collect(),
            Delivered::Initial,
        )
        .await
        .unwrap();
    assert_eq!(store.pending_inputs(&thread).await.unwrap().items.len(), 1);
    let hash = BlobHash("f".repeat(64));
    let path = dir
        .path()
        .join("blobs/ff")
        .join(hash.as_str())
        .with_extension("webp");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::File::create(path)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    let error = store
        .send_message(
            thread,
            message("invalid oversized blob".into(), vec![hash]),
            Acceptance::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        *error.cause,
        InputFailure::Rejected(SendRejected::ImageTooLarge { .. })
    ));
}
