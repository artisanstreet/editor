use artisan_api::{
    BlobHash, CreateProject, CreateThread, EngineConfig, EngineId, Event, Item, ItemId, ItemKind,
    ItemsQuery, Permissions, Phase, ProjectId, Question, Run, RunId, RunStatus, Status, ThreadId,
    ToolKind, Usage, now,
};
use forge_store::{BlobRejected, NewItem, Store};
use std::path::{Path, PathBuf};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("forge-store-test-{}", ItemId::new()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn add_thread(store: &Store) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .unwrap();
    assert_eq!(project.name, "app");
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
        })
        .await
        .unwrap()
        .id
}

async fn memory_store(dir: &Path) -> (Store, ThreadId) {
    let store = Store::open_in_memory(dir).unwrap();
    let thread = add_thread(&store).await;
    (store, thread)
}

fn message(thread: &ThreadId, id: &str, text: &str) -> NewItem {
    NewItem {
        id: ItemId::from(id),
        thread_id: thread.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status: Status::Done,
        kind: ItemKind::UserMessage {
            text: text.into(),
            images: Vec::new(),
            steer: false,
        },
    }
}

fn reply(thread: &ThreadId, id: &str, run: Option<&RunId>, text: &str, status: Status) -> NewItem {
    NewItem {
        id: ItemId::from(id),
        thread_id: thread.clone(),
        run_id: run.cloned(),
        parent_id: None,
        native_id: None,
        status,
        kind: ItemKind::AssistantMessage {
            text: text.into(),
            phase: Phase::Final,
        },
    }
}

fn run(thread: &ThreadId, status: RunStatus) -> Run {
    Run {
        id: RunId::new(),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        status,
        usage: Usage::default(),
        session_id: None,
        error: None,
        seq: 0,
        started_at: now(),
        ended_at: None,
    }
}

fn text(item: &Item) -> &str {
    item.preview().unwrap()
}

#[tokio::test]
async fn a_client_resuming_after_a_seq_gets_every_later_change_in_order() {
    let dir = TempDir::new();
    let (store, thread) = memory_store(&dir.0).await;
    store
        .upsert_item(message(&thread, "m1", "first"))
        .await
        .unwrap();
    let seen = store.head();

    let live = store
        .upsert_run(run(&thread, RunStatus::Running))
        .await
        .unwrap();
    store
        .upsert_item(reply(
            &thread,
            "a1",
            Some(&live.id),
            "partial",
            Status::Running,
        ))
        .await
        .unwrap();
    store
        .upsert_item(reply(
            &thread,
            "a1",
            Some(&live.id),
            "whole answer",
            Status::Done,
        ))
        .await
        .unwrap();
    store
        .set_setting("theme".into(), "dark".into())
        .await
        .unwrap();

    let changes = store.changes_since(seen, 100).await.unwrap().unwrap();
    let seqs: Vec<u64> = changes
        .iter()
        .map(|envelope| envelope.seq.unwrap())
        .collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "out of order: {seqs:?}"
    );
    assert!(seqs[0] > seen);
    assert_eq!(*seqs.last().unwrap(), store.head());

    // Only the latest version of each entity, and nothing from before `seen`.
    let kinds: Vec<&str> = changes
        .iter()
        .map(|envelope| match &envelope.event {
            Event::Run { .. } => "run",
            Event::Item { item } => {
                assert_eq!(text(item), "whole answer");
                "item"
            }
            Event::Thread { .. } => "thread",
            Event::Setting { key, value } => {
                assert_eq!((key.as_str(), value.as_str()), ("theme", Some("dark")));
                "setting"
            }
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["run", "item", "thread", "setting"]);

    // Too far behind: the caller resets instead.
    assert_eq!(store.changes_since(0, 3).await.unwrap(), None);
    let state = store.state().await.unwrap();
    assert_eq!(state.head, store.head());
    assert_eq!(
        state.runs,
        vec![store.run(&live.id).await.unwrap().unwrap()]
    );
    assert_eq!(state.settings["theme"], "dark");
}

#[tokio::test]
async fn updating_an_item_keeps_its_place_and_a_finished_item_never_reopens() {
    let dir = TempDir::new();
    let (store, thread) = memory_store(&dir.0).await;
    let first = store
        .upsert_item(reply(&thread, "a1", None, "draft", Status::Running))
        .await
        .unwrap();
    let second = store
        .upsert_item(message(&thread, "m2", "steer"))
        .await
        .unwrap();

    let done = store
        .upsert_item(reply(&thread, "a1", None, "final", Status::Done))
        .await
        .unwrap();
    assert_eq!(
        (done.position, done.created_at),
        (first.position, first.created_at)
    );
    assert!(done.seq > second.seq);

    // A late event from the engine must not resurrect the finished message.
    let late = store
        .upsert_item(reply(&thread, "a1", None, "final!", Status::Running))
        .await
        .unwrap();
    assert_eq!(late.status, Status::Done);
    assert_eq!(text(&late), "final!");

    let page = store
        .items_page(
            &thread,
            ItemsQuery {
                before: None,
                after: None,
                limit: None,
            },
        )
        .await
        .unwrap();
    let ids: Vec<&str> = page.items.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["a1", "m2"]);

    let thread = store.thread(&thread).await.unwrap().unwrap();
    assert_eq!(thread.activity_seq, late.seq);
    assert!(thread.is_unread());
}

#[tokio::test]
async fn a_retried_send_stores_the_message_once() {
    let dir = TempDir::new();
    let (store, thread) = memory_store(&dir.0).await;
    let (sent, created) = store
        .insert_user_message(message(&thread, "m1", "hello"))
        .await
        .unwrap();
    assert!(created);

    let mut events = store.subscribe();
    let (again, created) = store
        .insert_user_message(message(&thread, "m1", "hello?"))
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(again, sent);
    assert!(
        events.try_recv().is_err(),
        "a retry must not publish anything"
    );

    let page = store
        .items_page(
            &thread,
            ItemsQuery {
                before: None,
                after: None,
                limit: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(text(&page.items[0]), "hello");
}

#[tokio::test]
async fn paging_walks_back_from_the_tail_and_forward_with_more() {
    let dir = TempDir::new();
    let (store, thread) = memory_store(&dir.0).await;
    let mut positions = Vec::new();
    for index in 0..7 {
        let item = store
            .upsert_item(message(&thread, &format!("m{index}"), "x"))
            .await
            .unwrap();
        positions.push(item.position);
    }
    let page = |before, after| {
        let store = store.clone();
        let thread = thread.clone();
        async move {
            let page = store
                .items_page(
                    &thread,
                    ItemsQuery {
                        before,
                        after,
                        limit: Some(3),
                    },
                )
                .await
                .unwrap();
            let ids: Vec<String> = page.items.iter().map(|item| item.id.0.clone()).collect();
            (ids, page.more)
        }
    };

    assert_eq!(
        page(None, None).await,
        (vec!["m4".into(), "m5".into(), "m6".into()], true)
    );
    assert_eq!(
        page(Some(positions[4]), None).await,
        (vec!["m1".into(), "m2".into(), "m3".into()], true)
    );
    assert_eq!(
        page(Some(positions[1]), None).await,
        (vec!["m0".into()], false)
    );
    assert_eq!(
        page(None, Some(positions[0])).await,
        (vec!["m1".into(), "m2".into(), "m3".into()], true)
    );
    assert_eq!(
        page(None, Some(positions[3])).await,
        (vec!["m4".into(), "m5".into(), "m6".into()], false)
    );
    assert_eq!(page(None, Some(positions[6])).await, (vec![], false));
}

#[tokio::test]
async fn restarting_forge_interrupts_live_runs_and_cancels_their_waiting_items() {
    let dir = TempDir::new();
    let (thread, live, finished, seen) = {
        let store = Store::open(&dir.0).unwrap();
        let thread = add_thread(&store).await;
        let finished = store
            .upsert_run(run(&thread, RunStatus::Done))
            .await
            .unwrap();
        let live = store
            .upsert_run(run(&thread, RunStatus::Running))
            .await
            .unwrap();
        store
            .upsert_item(message(&thread, "m1", "go"))
            .await
            .unwrap();
        let mut tool = reply(&thread, "t1", Some(&live.id), "", Status::Running);
        tool.kind = ItemKind::Tool {
            tool: ToolKind::Shell {
                command: "cargo test".into(),
                cwd: None,
                background: false,
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        };
        tool.native_id = Some("toolu_1".into());
        store.upsert_item(tool).await.unwrap();
        let mut question = reply(&thread, "q1", Some(&live.id), "", Status::AwaitingUser);
        question.kind = ItemKind::Question {
            blocking: true,
            questions: vec![Question {
                id: "q".into(),
                header: None,
                text: "Which?".into(),
                multi_select: false,
                options: Vec::new(),
            }],
            answers: None,
        };
        store.upsert_item(question).await.unwrap();
        (thread, live, finished, store.head())
    };

    let store = Store::open(&dir.0).unwrap();
    let run = store.run(&live.id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Interrupted);
    assert!(run.ended_at.is_some());
    assert_eq!(store.run(&finished.id).await.unwrap().unwrap(), finished);

    let tool = store
        .item_by_native(&thread, Some(&live.id), "toolu_1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tool.status, Status::Cancelled);
    assert_eq!(
        store
            .item(&ItemId::from("q1"))
            .await
            .unwrap()
            .unwrap()
            .status,
        Status::Cancelled
    );
    assert_eq!(
        store
            .item(&ItemId::from("m1"))
            .await
            .unwrap()
            .unwrap()
            .status,
        Status::Done
    );

    // An Editor that was connected before the restart learns about it.
    let changes = store.changes_since(seen, 100).await.unwrap().unwrap();
    assert!(changes.iter().any(|envelope| matches!(&envelope.event,
        Event::Run { run } if run.status == RunStatus::Interrupted)));
    assert_eq!(
        store.latest_run(&thread).await.unwrap().unwrap().id,
        live.id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribers_see_concurrent_writes_in_seq_order() {
    let dir = TempDir::new();
    let (store, thread) = memory_store(&dir.0).await;
    let start = store.head();
    let mut events = store.subscribe();

    let writers: Vec<_> = (0..8)
        .map(|writer| {
            let store = store.clone();
            let thread = thread.clone();
            tokio::spawn(async move {
                for index in 0..25 {
                    let id = format!("w{writer}-{index}");
                    store.upsert_item(message(&thread, &id, "x")).await.unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.await.unwrap();
    }

    // Each item write is an item change plus a thread change.
    let mut seqs = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        seqs.push(envelope.seq.unwrap());
    }
    let expected: Vec<u64> = (start + 1..=start + 400).collect();
    assert_eq!(seqs, expected);
    assert_eq!(store.head(), start + 400);
}

#[tokio::test]
async fn blobs_are_stored_once_by_content_and_only_webp_is_accepted() {
    let dir = TempDir::new();
    let store = Store::open_in_memory(&dir.0).unwrap();
    let mut webp = b"RIFF\x10\0\0\0WEBPVP8 ".to_vec();
    webp.extend_from_slice(&[7; 64]);

    let first = store.put_blob(webp.clone()).await.unwrap();
    let again = store.put_blob(webp.clone()).await.unwrap();
    assert_eq!(first, again);
    assert_eq!(first.bytes, webp.len() as u64);
    let path = store.blob_path(&first.hash).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), webp);
    assert_eq!(
        path.parent().unwrap().file_name().unwrap().to_str(),
        Some(&first.hash.as_str()[..2])
    );

    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    let error = store.put_blob(png).await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<BlobRejected>(),
        Some(&BlobRejected::NotWebp)
    );

    assert_eq!(store.blob_path(&BlobHash("../../etc/passwd".into())), None);
    assert_eq!(store.blob_path(&BlobHash("0".repeat(64))), None);
}

#[tokio::test]
async fn a_pairing_code_works_once_and_not_after_it_expires() {
    let dir = TempDir::new();
    let store = Store::open_in_memory(&dir.0).unwrap();
    let now = now();
    store
        .add_pair_code("fresh".into(), now + 600_000)
        .await
        .unwrap();
    store.add_pair_code("stale".into(), now - 1).await.unwrap();

    assert!(store.take_pair_code("fresh".into(), now).await.unwrap());
    assert!(!store.take_pair_code("fresh".into(), now).await.unwrap());
    assert!(!store.take_pair_code("stale".into(), now).await.unwrap());

    let device = store
        .add_device("Laptop".into(), "key-a".into())
        .await
        .unwrap();
    assert_eq!(
        store.is_authorized("key-a").await.unwrap(),
        Some(device.clone())
    );
    assert!(store.revoke_device(&device).await.unwrap());
    assert_eq!(store.is_authorized("key-a").await.unwrap(), None);
}

/// Threads saved with the old access and approvals keep running the way
/// they did: never asking stays Full, everything else becomes Auto.
#[tokio::test]
async fn old_permission_choices_become_auto_or_full() {
    let dir = TempDir::new();
    let (asks, never_asks) = {
        let store = Store::open(&dir.0).unwrap();
        (add_thread(&store).await, add_thread(&store).await)
    };
    let conn = rusqlite::Connection::open(dir.0.join("forge.sqlite3")).unwrap();
    for (thread, approvals) in [(&asks, "on_request"), (&never_asks, "never")] {
        conn.execute(
            "UPDATE threads SET body = json_set(json_remove(body, '$.config.permissions'),
                 '$.config.access', 'workspace', '$.config.approvals', ?2) WHERE id = ?1",
            rusqlite::params![thread.to_string(), approvals],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT OR REPLACE INTO kv (key, seq, value) VALUES ('default_engine_config', 0, ?1)",
        [r#"{"engine":"codex","model":null,"effort":null,"access":"full","approvals":"never","options":{}}"#],
    )
    .unwrap();
    conn.pragma_update(None, "user_version", 3).unwrap();
    drop(conn);

    let state = Store::open(&dir.0).unwrap().state().await.unwrap();
    let permissions = |id: &ThreadId| {
        state
            .threads
            .iter()
            .find(|thread| &thread.id == id)
            .map(|thread| thread.config.permissions)
    };
    assert_eq!(permissions(&asks), Some(Permissions::Auto));
    assert_eq!(permissions(&never_asks), Some(Permissions::Full));
    let default: EngineConfig =
        serde_json::from_value(state.settings["default_engine_config"].clone()).unwrap();
    assert_eq!(default.permissions, Permissions::Full);
}
