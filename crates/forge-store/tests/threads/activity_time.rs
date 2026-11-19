use super::*;
use crate::Acceptance;
use artisan_api::{EngineConfig, EngineId, ItemId, RunState, SendMessage, Usage};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn only_accepted_activity_changes_thread_recency() {
    let dir = tempfile::tempdir().unwrap();
    let now = Arc::new(AtomicI64::new(10));
    let reading = Arc::clone(&now);
    let store = Store::open_in(
        dir.path(),
        crate::Environment {
            clock: artisan_api::WallClock::new(move || reading.load(Ordering::SeqCst)),
            ..crate::Environment::system()
        },
    )
    .unwrap();
    let at = |millis| now.store(millis, Ordering::SeqCst);
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/activity-test".into(),
            name: None,
        })
        .await
        .unwrap();
    at(10);
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            title: None,
            config: EngineConfig::new(EngineId::Codex),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    assert_eq!(thread.activity_at, 10);
    assert!(!thread.is_unread());
    at(20);
    let run = store
        .upsert_run(Run {
            config: None,
            id: RunId::new(),
            thread_id: thread.id.clone(),
            engine: EngineId::Codex,
            model: None,
            input_cutoff: thread.activity_seq,
            state: RunState::Running,
            usage: Usage::default(),
            session_id: None,
            seq: 0,
            started_at: 20,
            continuation: None,
        })
        .await
        .unwrap();
    let running = store.thread(&thread.id).await.unwrap().unwrap();
    assert_eq!((running.activity_seq, running.activity_at), (run.seq, 20));
    let mut usage = run.clone();
    usage.usage.input = 100;
    at(25);
    store.upsert_run(usage.clone()).await.unwrap();
    let unchanged = store.thread(&thread.id).await.unwrap().unwrap();
    assert_eq!(unchanged.activity_at, 20, "usage alone is not activity");
    usage
        .finish(artisan_api::RunOutcome::Done, None, 30)
        .unwrap();
    at(30);
    let done = store.upsert_run(usage).await.unwrap();
    let finished = store.thread(&thread.id).await.unwrap().unwrap();
    assert_eq!(
        (finished.activity_seq, finished.activity_at),
        (done.seq, 30)
    );
    at(80);
    let edited = store
        .update_thread(
            &thread.id,
            UpdateThread {
                title: Some("Renamed".into()),
                archived: Some(true),
                config: Some(EngineConfig::new(EngineId::Claude)),
                read_seq: Some(done.seq),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(edited.updated_at, 80);
    assert_eq!((edited.activity_seq, edited.activity_at), (done.seq, 30));
    assert!(!edited.is_unread());
    let unread = store.mark_unread(&thread.id).await.unwrap().unwrap();
    assert!(unread.is_unread());
    assert_eq!(unread.activity_at, 30);
    let settled = store
        .settle_thread(&thread.id, done.seq)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.activity_at, 30);
    at(90);
    let input = SendMessage {
        id: ItemId::new(),
        text: "Next".into(),
        images: Vec::new(),
        config: None,
        draft: None,
    };
    let (item, accepted, _) = store
        .send_message(thread.id.clone(), input.clone(), Acceptance::default())
        .await
        .unwrap();
    assert!(accepted);
    let accepted_thread = store.thread(&thread.id).await.unwrap().unwrap();
    assert_eq!(accepted_thread.activity_seq, item.seq);
    assert_eq!(accepted_thread.activity_at, accepted_thread.updated_at);
    assert!(accepted_thread.activity_at > 30);
    assert!(
        !store
            .send_message(thread.id.clone(), input, Acceptance::default())
            .await
            .unwrap()
            .1
    );
    let replayed = store.thread(&thread.id).await.unwrap().unwrap();
    assert_eq!(
        replayed, accepted_thread,
        "replayed command does not reorder the thread"
    );
}
