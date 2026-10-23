use artisan_api::{
    BlobHash, CreateProject, CreateThread, EngineConfig, EngineId, Event, Item, ItemId, ItemKind,
    ItemsQuery, MessageDirection, Permissions, Phase, ProjectId, Question, Run, RunId, RunStatus,
    Status, ThreadId, ToolKind, Usage, now,
};
use forge_store::{Acceptance, BlobRejected, NewItem, Store};
use std::path::{Path, PathBuf};

struct TempDir(PathBuf);

impl TempDir {
    #[expect(
        clippy::unwrap_used,
        reason = "test helper; a failure fails the calling test"
    )]
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("forge-store-test-{}", ItemId::new()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a leftover temp dir is harmless.
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn add_thread(store: &Store) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .unwrap();
    assert_eq!(
        project.name, "app",
        "a project is named after its directory"
    );
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

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn test_store(dir: &Path) -> (Store, ThreadId) {
    let store = Store::open(dir).unwrap();
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
            from_agent: None,
            sender: None,
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
        config: None,
        id: RunId::new(),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        status,
        usage: Usage::default(),
        session_id: None,
        error: None,
        seq: 0,
        started_at: now(),
        ended_at: None,
        continuation: None,
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
fn text(item: &Item) -> &str {
    item.preview().unwrap()
}

#[tokio::test]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "any other event fails the test"
)]
async fn a_client_resuming_after_a_seq_gets_every_later_change_in_order() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
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
    assert_eq!(kinds, ["run", "thread", "item", "setting"]);

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
    let (store, thread) = test_store(&dir.0).await;
    let initial_activity = store.thread(&thread).await.unwrap().unwrap().activity_seq;
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
    assert_eq!(
        thread.activity_seq, initial_activity,
        "raw Item updates do not mark activity"
    );
}

#[tokio::test]
async fn a_retried_send_stores_the_message_once() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let (sent, created) = store
        .insert_user_message(message(&thread, "m1", "hello"))
        .await
        .unwrap();
    assert!(created);

    let mut events = store.subscribe();
    let (again, created) = store
        .insert_user_message(message(&thread, "m1", "hello"))
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
    let (store, thread) = test_store(&dir.0).await;
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
                watch: false,
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
            answer_text: None,
            blocking: true,
            questions: vec![Question {
                id: "q".into(),
                header: None,
                text: "Which?".into(),
                multi_select: false,
                options: Vec::new().into(),
            }]
            .into(),
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
        store.native_id(&tool.id).await.unwrap().as_deref(),
        Some("toolu_1")
    );
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
    let (store, thread) = test_store(&dir.0).await;
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

    // Raw Item writes publish exactly one durable change.
    let mut seqs = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        seqs.push(envelope.seq.unwrap());
    }
    let expected: Vec<u64> = (start + 1..=start + 200).collect();
    assert_eq!(seqs, expected);
    assert_eq!(store.head(), start + 200);
}

#[tokio::test]
async fn blobs_are_stored_once_by_content_and_only_webp_is_accepted() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let webp = webp::Encoder::from_rgb(&[7, 7, 7], 1, 1)
        .encode_lossless()
        .to_vec();

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
    let store = Store::open(&dir.0).unwrap();
    let now = now();
    store
        .add_pair_code("fresh".into(), now + 600_000)
        .await
        .unwrap();
    store.add_pair_code("stale".into(), now - 1).await.unwrap();

    let pair =
        |code: &str, key: &str| store.pair_device(code.to_owned(), "Laptop".into(), key.to_owned());
    let device = pair("fresh", "key-a").await.unwrap().unwrap();
    assert_eq!(pair("fresh", "key-b").await.unwrap(), None);
    assert_eq!(pair("stale", "key-c").await.unwrap(), None);
    assert_eq!(store.is_authorized("key-a").unwrap(), Some(device.clone()));
    assert_eq!(store.is_authorized("key-b").unwrap(), None);

    let mut revocations = store.subscribe_revocations();
    assert!(store.revoke_device(&device).await.unwrap());
    assert_eq!(store.is_authorized("key-a").unwrap(), None);
    assert_eq!(revocations.recv().await.unwrap(), device);
}

/// `ae forge devices revoke` writes from another process while Forge runs;
/// the device's next request is refused and its open streams are told.
#[tokio::test]
async fn a_device_revoked_by_another_process_is_refused_next() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let device = store
        .add_device("Laptop".into(), "key-a".into())
        .await
        .unwrap();
    assert_eq!(store.is_authorized("key-a").unwrap(), Some(device.clone()));
    let mut revocations = store.subscribe_revocations();
    let admin = forge_store::admin::Admin::open(&dir.0).unwrap();
    assert!(admin.revoke_device(device.as_str()).unwrap().is_some());
    assert_eq!(store.is_authorized("key-a").unwrap(), None);
    assert_eq!(revocations.try_recv().unwrap(), device);
}

/// Threads saved with the old access and approvals keep running the way
/// they did: never asking stays Full, everything else becomes Auto.
#[tokio::test]
async fn old_permission_choices_become_auto_or_full() {
    let dir = TempDir::new();
    let seed = TempDir::new();
    let (asks, never_asks) = {
        let store = Store::open(&seed.0).unwrap();
        (add_thread(&store).await, add_thread(&store).await)
    };
    let conn = rusqlite::Connection::open(dir.0.join("forge.sqlite3")).unwrap();
    conn.execute_batch(include_str!("fixtures/schema-v3.sql"))
        .unwrap();
    conn.execute(
        "ATTACH DATABASE ?1 AS seed",
        [seed.0.join("forge.sqlite3").to_str().unwrap()],
    )
    .unwrap();
    conn.execute_batch("INSERT INTO projects SELECT id,seq,body FROM seed.projects; INSERT INTO threads SELECT id,seq,body FROM seed.threads; DETACH DATABASE seed;").unwrap();
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

#[tokio::test]
async fn thread_creation_rejects_a_missing_project_without_writes_and_replays_existing_ids() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let missing_project = ProjectId::new();
    let mut request = CreateThread {
        id: ThreadId::new(),
        project_id: missing_project.clone(),
        config: EngineConfig::new(EngineId::Claude),
        title: Some("must not be saved".into()),
        parent_thread_id: None,
        spawned_by: None,
    };
    let head = store.head();
    let error = store.create_thread(request.clone()).await.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<forge_store::ProjectNotFound>()
            .unwrap()
            .0,
        missing_project,
    );
    assert_eq!(store.head(), head);
    assert!(store.thread(&request.id).await.unwrap().is_none());

    // Existing IDs return their canonical stored thread. A route must not
    // override that operation with an earlier check of the replayed request.
    request.id = thread.clone();
    let existing = store.thread(&thread).await.unwrap().unwrap();
    assert_eq!(store.create_thread(request).await.unwrap(), existing);
    assert_eq!(store.head(), head);
}

#[tokio::test]
async fn unchanged_values_publish_nothing_and_read_markers_never_regress() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let input = message(&thread, "unchanged", "hello");
    let item = store.upsert_item(input.clone()).await.unwrap();
    let run = store
        .upsert_run(run(&thread, RunStatus::Running))
        .await
        .unwrap();
    store
        .set_setting("theme".into(), serde_json::json!("dark"))
        .await
        .unwrap();
    let update = artisan_api::UpdateThread {
        read_seq: Some(store.head()),
        ..Default::default()
    };
    store.update_thread(&thread, update.clone()).await.unwrap();
    let head = store.head();
    let mut events = store.subscribe();
    assert_eq!(store.upsert_item(input).await.unwrap(), item);
    assert_eq!(store.upsert_run(run.clone()).await.unwrap(), run);
    store
        .set_setting("theme".into(), serde_json::json!("dark"))
        .await
        .unwrap();
    store.update_thread(&thread, update).await.unwrap();
    store
        .update_thread(
            &thread,
            artisan_api::UpdateThread {
                read_seq: Some(0),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(store.head(), head);
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn restart_preserves_accepted_input_for_delivery() {
    let dir = TempDir::new();
    let thread;
    {
        let (store, id) = test_store(&dir.0).await;
        thread = id;
        let mut input = message(&thread, "pending", "not delivered");
        input.status = Status::Running;
        store.insert_user_message(input).await.unwrap();
    }
    let reopened = Store::open(&dir.0).unwrap();
    let pending = reopened.pending_inputs(&thread).await.unwrap().items;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id.as_str(), "pending");
    assert_eq!(pending[0].status, Status::Running);
}

#[tokio::test]
async fn a_message_id_cannot_change_content_or_thread() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    store
        .insert_user_message(message(&thread, "fixed", "original"))
        .await
        .unwrap();
    let head = store.head();
    assert!(
        store
            .insert_user_message(message(&thread, "fixed", "changed"))
            .await
            .is_err()
    );
    let other = add_thread(&store).await;
    let after_thread = store.head();
    assert!(
        store
            .insert_user_message(message(&other, "fixed", "original"))
            .await
            .is_err()
    );
    assert_eq!(store.head(), after_thread);
    assert!(after_thread > head);
}

#[tokio::test]
async fn pairing_failure_does_not_consume_the_invitation() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    store
        .add_pair_code("invite".into(), now() + 60_000)
        .await
        .unwrap();
    let conn = rusqlite::Connection::open(dir.0.join("forge.sqlite3")).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_device BEFORE INSERT ON authorized_keys
        BEGIN SELECT RAISE(ABORT, 'injected device failure'); END;",
    )
    .unwrap();
    assert!(
        store
            .pair_device("invite".into(), "Laptop".into(), "certificate".into())
            .await
            .is_err()
    );
    conn.execute_batch("DROP TRIGGER reject_device;").unwrap();
    let id = store
        .pair_device("invite".into(), "Laptop".into(), "certificate".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(store.is_authorized("certificate").unwrap(), Some(id));
    assert!(
        store
            .pair_device("invite".into(), "Laptop".into(), "certificate".into())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn replaying_a_send_cannot_change_config_or_title() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let command = artisan_api::SendMessage {
        draft: None,
        id: ItemId::new(),
        text: "First title\nbody".into(),
        images: Vec::new(),
        config: Some(EngineConfig::new(EngineId::Codex)),
    };
    let (input, created, _) = store
        .send_message(
            thread.clone(),
            command.clone(),
            false,
            forge_store::Acceptance {
                title: Some("First title".into()),
                remember_config: true,
                from_agent: None,
                sender: None,
            },
        )
        .await
        .unwrap();
    assert!(created);
    let head = store.head();
    let mut replay = command;
    replay.config = Some(EngineConfig::new(EngineId::OpenCode));
    let (again, created, _) = store
        .send_message(
            thread.clone(),
            replay,
            true,
            forge_store::Acceptance {
                title: Some("Replayed title".into()),
                remember_config: true,
                from_agent: None,
                sender: None,
            },
        )
        .await
        .unwrap();
    assert!(!created);
    assert_eq!(again, input);
    assert_eq!(store.head(), head);
    let thread = store.thread(&thread).await.unwrap().unwrap();
    assert_eq!(thread.config.engine, EngineId::Codex);
    assert_eq!(thread.title.as_deref(), Some("First title"));
}

#[tokio::test]
async fn concurrent_skill_changes_and_unrelated_mcp_edits_survive() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    let (left, right) = tokio::join!(
        store.set_skill_enabled("local.review".into(), false),
        store.set_skill_enabled("local.deploy".into(), false)
    );
    left.unwrap();
    right.unwrap();
    assert_eq!(
        store.setting("disabled_skills").await.unwrap(),
        Some(serde_json::json!(["local.deploy", "local.review"]))
    );
    let left = artisan_api::McpServer {
        name: "left".into(),
        command: "left-server".into(),
        ..Default::default()
    };
    let right = artisan_api::McpServer {
        name: "right".into(),
        command: "right-server".into(),
        ..Default::default()
    };
    let (a, b) = tokio::join!(
        store.mutate_mcp(artisan_api::McpMutation::Upsert {
            previous: None,
            server: left.clone()
        }),
        store.mutate_mcp(artisan_api::McpMutation::Upsert {
            previous: None,
            server: right.clone()
        })
    );
    a.unwrap();
    b.unwrap();
    let mut edited = left.clone();
    edited.command = "new-left-server".into();
    let servers = store
        .mutate_mcp(artisan_api::McpMutation::Upsert {
            previous: Some(left.clone()),
            server: edited.clone(),
        })
        .await
        .unwrap();
    assert_eq!(servers, vec![edited, right]);
    assert!(
        store
            .mutate_mcp(artisan_api::McpMutation::Remove { previous: left })
            .await
            .is_err(),
        "stale deletion must not remove a newer edit"
    );
}

#[tokio::test]
async fn draft_compare_and_swap_and_acceptance_preserve_newer_input() {
    use artisan_api::{DraftRevision, SendMessage, WriteDraft};
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let saved = store
        .set_draft(
            "scope".into(),
            WriteDraft {
                expected_revision: 0,
                body: serde_json::json!({"text":"first"}),
            },
        )
        .await
        .unwrap();
    assert!(saved.saved);
    let stale = store
        .set_draft(
            "scope".into(),
            WriteDraft {
                expected_revision: 0,
                body: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();
    assert!(!stale.saved);
    let newer = store
        .set_draft(
            "scope".into(),
            WriteDraft {
                expected_revision: saved.current.revision,
                body: serde_json::json!({"text":"newer"}),
            },
        )
        .await
        .unwrap();
    let command = SendMessage {
        id: ItemId::new(),
        text: "first".into(),
        images: vec![],
        config: None,
        draft: Some(DraftRevision {
            scope: "scope".into(),
            revision: saved.current.revision,
        }),
    };
    store
        .send_message(
            thread.clone(),
            command.clone(),
            false,
            Acceptance::default(),
        )
        .await
        .unwrap();
    assert_eq!(store.draft("scope").await.unwrap(), newer.current);
    let mut accepted = command;
    accepted.id = ItemId::new();
    accepted.draft.as_mut().unwrap().revision = newer.current.revision;
    store
        .send_message(thread, accepted, false, Acceptance::default())
        .await
        .unwrap();
    let cleared = store.draft("scope").await.unwrap();
    assert!(cleared.body.is_null());
    assert!(cleared.revision > newer.current.revision);
    assert!(
        !store
            .set_draft(
                "scope".into(),
                WriteDraft {
                    expected_revision: newer.current.revision,
                    body: newer.current.body
                }
            )
            .await
            .unwrap()
            .saved
    );
}

#[tokio::test]
async fn settlement_is_monotonic_and_cannot_acknowledge_future_activity() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    store
        .insert_user_message(message(&thread, "settle-input", "hello"))
        .await
        .unwrap();
    let settled = store
        .settle_thread(&thread, u64::MAX)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.settled_seq, settled.activity_seq);
    let head = store.head();
    assert_eq!(
        store.settle_thread(&thread, 0).await.unwrap().unwrap(),
        settled
    );
    assert_eq!(store.head(), head);
}

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one scenario walked step by step")]
async fn inspector_controls_survive_history_paging_and_root_plan_owns_checklist() {
    use artisan_api::{FileChange, FileOp, PlanStep, StepStatus};
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let mut shell = message(&thread, "background-shell", "");
    shell.status = Status::Running;
    shell.kind = ItemKind::Tool {
        tool: ToolKind::Shell {
            command: "long job".into(),
            cwd: Some("/work/app".into()),
            background: true,
            watch: false,
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    };
    store.upsert_item(shell).await.unwrap();
    let mut foreground = message(&thread, "foreground-shell", "");
    foreground.status = Status::Running;
    foreground.kind = ItemKind::Tool {
        tool: ToolKind::Shell {
            command: "cargo test".into(),
            cwd: None,
            background: false,
            watch: false,
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    };
    store.upsert_item(foreground).await.unwrap();
    let mut workflow = message(&thread, "workflow", "");
    workflow.status = Status::Running;
    workflow.kind = ItemKind::Tool {
        tool: ToolKind::Other {
            name: "Workflow".into(),
            input: Some(
                serde_json::json!({"script": "export const meta = { name: 'review-changes' }"})
                    .to_string()
                    .into(),
            ),
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    };
    store.upsert_item(workflow).await.unwrap();
    let mut root = message(&thread, "root-plan", "");
    root.kind = ItemKind::Plan {
        steps: vec![PlanStep {
            text: "root task".into(),
            status: StepStatus::Pending,
        }]
        .into(),
        proposal: None,
        approval: None,
    };
    store.upsert_item(root.clone()).await.unwrap();
    let mut child = root;
    child.id = "child-plan".into();
    child.parent_id = Some("agent".into());
    child.kind = ItemKind::Plan {
        steps: vec![PlanStep {
            text: "child task".into(),
            status: StepStatus::Pending,
        }]
        .into(),
        proposal: None,
        approval: None,
    };
    store.upsert_item(child).await.unwrap();
    let mut edit = message(&thread, "edit", "");
    edit.kind = ItemKind::Tool {
        tool: ToolKind::Edit {
            changes: vec![FileChange {
                path: "file".into(),
                op: FileOp::Modify,
                diff: Some(
                    "--- a/file\n+++ b/file\n@@ -1 +1 @@\n---old content\n+++new content\n".into(),
                ),
            }]
            .into(),
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    };
    store.upsert_item(edit).await.unwrap();
    for i in 0..20 {
        store
            .upsert_item(message(&thread, &format!("new-{i}"), "new text"))
            .await
            .unwrap();
    }
    let tail = store
        .items_page(
            &thread,
            ItemsQuery {
                before: None,
                after: None,
                limit: Some(2),
            },
        )
        .await
        .unwrap();
    assert!(
        !tail
            .items
            .iter()
            .any(|item| item.id.as_str() == "background-shell")
    );
    let summary = store.inspector(&thread).await.unwrap();
    // The panel splits what the turn waits on from what runs on without it.
    let terminals: Vec<_> = summary
        .terminals
        .iter()
        .map(|terminal| (terminal.command.clone(), terminal.background))
        .collect();
    assert_eq!(
        terminals,
        [("cargo test".into(), false), ("long job".into(), true)]
    );
    assert_eq!(summary.workflows[0].name.as_deref(), Some("review-changes"));
    assert_eq!(summary.plan.inline().unwrap()[0].text, "root task".into());
    assert_eq!(
        summary.edits.unwrap(),
        artisan_api::EditTotals {
            added: 1,
            removed: 1
        }
    );
    assert_eq!(
        store.inspector(&thread).await.unwrap().revision,
        summary.revision
    );
}

#[tokio::test]
async fn transcript_pages_do_not_spend_the_root_window_on_agent_output() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    store
        .upsert_item(message(&thread, "root-first", "root"))
        .await
        .unwrap();
    let agent: ItemId = "owner-agent".into();
    for n in 0..125 {
        let mut item = reply(&thread, &format!("child-{n}"), None, "agent", Status::Done);
        item.parent_id = Some(agent.clone());
        store.upsert_item(item).await.unwrap();
    }
    let query = ItemsQuery {
        before: None,
        after: None,
        limit: Some(10),
    };
    let root = store
        .transcript_page(&thread, None, query.clone())
        .await
        .unwrap();
    assert_eq!(root.items.len(), 1);
    assert_eq!(root.items[0].id.as_str(), "root-first");
    assert!(!root.more);
    let tail = store
        .transcript_page(&thread, Some(&agent), query)
        .await
        .unwrap();
    assert_eq!(tail.items.len(), 10);
    assert!(tail.more);
    assert!(
        tail.items
            .iter()
            .all(|item| item.parent_id.as_ref() == Some(&agent))
    );
    let older = store
        .transcript_page(
            &thread,
            Some(&agent),
            ItemsQuery {
                before: Some(tail.items[0].position),
                after: None,
                limit: Some(10),
            },
        )
        .await
        .unwrap();
    assert!(older.items.last().unwrap().position < tail.items[0].position);
}

#[tokio::test]
async fn large_sources_do_not_spend_item_page_budget_and_keep_both_cursor_orders() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let text = "x".repeat(3 * 1024 * 1024);
    let mut positions = Vec::new();
    for n in 0..4 {
        positions.push(
            store
                .upsert_item(reply(
                    &thread,
                    &format!("large-{n}"),
                    None,
                    &text,
                    Status::Done,
                ))
                .await
                .unwrap()
                .position,
        );
    }
    let tail = store
        .transcript_page(
            &thread,
            None,
            ItemsQuery {
                before: None,
                after: None,
                limit: Some(100),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        tail.items
            .iter()
            .map(|item| item.position)
            .collect::<Vec<_>>(),
        positions
    );
    assert!(!tail.more);
    assert!(tail.items.iter().all(|item| matches!(
        &item.kind,
        ItemKind::AssistantMessage {
            text: artisan_api::TextContent::Stored(_),
            ..
        }
    )));
    assert!(serde_json::to_vec(&tail).unwrap().len() < 4096);
    let forward = store
        .transcript_page(
            &thread,
            None,
            ItemsQuery {
                before: None,
                after: Some(0),
                limit: Some(100),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        forward
            .items
            .iter()
            .map(|item| item.position)
            .collect::<Vec<_>>(),
        positions
    );
    assert!(!forward.more);
}

#[tokio::test]
async fn thread_controls_survive_window_eviction_and_goal_completion() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let mut goal = message(&thread, "goal", "");
    goal.kind = ItemKind::Notice {
        kind: artisan_api::NoticeKind::Goal {
            state: "active".into(),
        },
        text: "finish the reform".into(),
    };
    store.upsert_item(goal.clone()).await.unwrap();
    let mut ask = message(&thread, "ask", "");
    ask.status = Status::AwaitingUser;
    ask.kind = ItemKind::Question {
        answer_text: None,
        blocking: true,
        questions: vec![Question {
            id: "q".into(),
            header: None,
            text: "Choose?".into(),
            multi_select: false,
            options: vec![].into(),
        }]
        .into(),
        answers: None,
    };
    store.upsert_item(ask).await.unwrap();
    for n in 0..101 {
        store
            .upsert_item(message(&thread, &format!("later-{n}"), "later"))
            .await
            .unwrap();
    }
    let summary = store.inspector(&thread).await.unwrap();
    assert_eq!(summary.goal.unwrap().objective, "finish the reform".into());
    assert_eq!(summary.questions[0].id.as_str(), "ask");
    goal.id = "completed-goal".into();
    goal.kind = ItemKind::Notice {
        kind: artisan_api::NoticeKind::Goal {
            state: "complete".into(),
        },
        text: "finish the reform".into(),
    };
    store.upsert_item(goal).await.unwrap();
    assert!(store.inspector(&thread).await.unwrap().goal.is_none());
}

/// The user clears the goal (a new notice), then the engine sets it again
/// on its run's goal notice, which sits earlier: the goal is set.
#[tokio::test]
async fn a_goal_set_again_after_a_clear_shows_the_goal() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let goal = |id: &str, state: &str| {
        let mut item = message(&thread, id, "");
        item.kind = ItemKind::Notice {
            kind: artisan_api::NoticeKind::Goal {
                state: state.into(),
            },
            text: "ship the shooter".into(),
        };
        item
    };
    store
        .upsert_item(goal("run-goal", "blocked"))
        .await
        .unwrap();
    store.upsert_item(goal("cleared", "cleared")).await.unwrap();
    assert!(store.inspector(&thread).await.unwrap().goal.is_none());
    store.upsert_item(goal("run-goal", "active")).await.unwrap();
    let shown = store.inspector(&thread).await.unwrap().goal.expect("goal");
    assert_eq!(shown.state, artisan_api::ThreadGoalState::Active);
}

#[tokio::test]
async fn edit_totals_replace_instead_of_double_counting_item_updates() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let mut edit = message(&thread, "counted-edit", "");
    let kind = |diff: &str| ItemKind::Tool {
        tool: ToolKind::Edit {
            changes: vec![artisan_api::FileChange {
                path: "file".into(),
                op: artisan_api::FileOp::Modify,
                diff: Some(diff.into()),
            }]
            .into(),
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    };
    edit.kind = kind("--- a/file\n+++ b/file\n@@ -1 +1 @@\n-old\n+new\n");
    store.upsert_item(edit.clone()).await.unwrap();
    assert_eq!(
        store.inspector(&thread).await.unwrap().edits.unwrap(),
        artisan_api::EditTotals {
            added: 1,
            removed: 1
        }
    );
    edit.kind = kind("--- a/file\n+++ b/file\n@@ -1 +2 @@\n-old\n+first\n+second\n");
    store.upsert_item(edit.clone()).await.unwrap();
    let once = store.inspector(&thread).await.unwrap();
    assert_eq!(
        once.edits.unwrap(),
        artisan_api::EditTotals {
            added: 2,
            removed: 1
        }
    );
    store.upsert_item(edit).await.unwrap();
    assert_eq!(
        store.inspector(&thread).await.unwrap().revision,
        once.revision
    );
}

#[tokio::test]
async fn canonical_agent_message_appears_in_both_pages_without_another_row() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let mut agent = message(&thread, "agent-owner", "");
    agent.kind = ItemKind::Agent {
        engine: None,
        thread: None,
        name: None,
        description: "task".into(),
        prompt: None,
        agent_id: Some("opaque-address".into()),
        context_used: None,
        model: None,
        background: true,
        result: None,
        label: None,
    };
    let owner = store.upsert_item(agent).await.unwrap();
    let mut sent = message(&thread, "sent-message", "");
    sent.kind = ItemKind::Tool {
        tool: ToolKind::AgentMessage {
            recipient: "opaque-address".into(),
            target: Some(owner.id.clone()),
            summary: None,
            text: "hello".into(),
            direction: MessageDirection::Sent,
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    };
    let sent = store.upsert_item(sent).await.unwrap();
    let query = ItemsQuery {
        before: None,
        after: None,
        limit: Some(100),
    };
    let root = store
        .transcript_page(&thread, None, query.clone())
        .await
        .unwrap();
    let recipient = store
        .transcript_page(&thread, Some(&owner.id), query.clone())
        .await
        .unwrap();
    assert_eq!(recipient.items, vec![sent.clone()]);
    assert!(root.items.contains(&sent));
    assert_eq!(
        store.items_page(&thread, query).await.unwrap().items.len(),
        2
    );
}

#[tokio::test]
async fn inline_metadata_pages_enforce_byte_budget_without_cursor_gaps() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let text = "x".repeat(artisan_api::INLINE_TEXT_BYTES);
    let label = "n".repeat(4096);
    let mut positions = Vec::new();
    for index in 0..200 {
        let mut item = message(&thread, &format!("metadata-{index}"), "");
        item.kind = ItemKind::Agent {
            engine: None,
            thread: None,
            name: Some(label.clone()),
            description: text.clone().into(),
            prompt: Some(text.clone().into()),
            result: Some(text.clone().into()),
            agent_id: None,
            context_used: None,
            model: Some(label.clone()),
            background: false,
            label: Some(label.clone()),
        };
        positions.push(store.upsert_item(item).await.unwrap().position);
    }
    let first = store
        .transcript_page(
            &thread,
            None,
            ItemsQuery {
                before: None,
                after: None,
                limit: Some(200),
            },
        )
        .await
        .unwrap();
    assert!(
        first.more,
        "200 admitted inline Items must exceed one 8 MiB metadata page"
    );
    assert!(first.items.len() < 200);
    assert!(first.items.iter().all(|item| matches!(
        &item.kind,
        ItemKind::Agent {
            description: artisan_api::TextContent::Inline(_),
            ..
        }
    )));
    for backward in [false, true] {
        let mut cursor = None;
        let mut observed = Vec::new();
        loop {
            let page = store
                .transcript_page(
                    &thread,
                    None,
                    ItemsQuery {
                        before: if backward { cursor } else { None },
                        after: if backward {
                            None
                        } else {
                            Some(cursor.unwrap_or(0))
                        },
                        limit: Some(200),
                    },
                )
                .await
                .unwrap();
            assert!(
                page.items
                    .iter()
                    .map(|item| serde_json::to_vec(item).unwrap().len())
                    .sum::<usize>()
                    <= 8 * 1024 * 1024
            );
            assert!(
                page.items
                    .windows(2)
                    .all(|pair| pair[0].position < pair[1].position)
            );
            cursor = if backward {
                page.items.first()
            } else {
                page.items.last()
            }
            .map(|item| item.position);
            if backward {
                observed.splice(0..0, page.items.iter().map(|item| item.position));
            } else {
                observed.extend(page.items.iter().map(|item| item.position));
            }
            if !page.more {
                break;
            }
            assert!(cursor.is_some(), "bounded page must advance its cursor");
        }
        assert_eq!(
            observed, positions,
            "both cursor directions must preserve every Item exactly once"
        );
    }
}

#[tokio::test]
async fn a_settled_agent_leaves_the_inspector_at_once() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    // The agents belong to the turn still in progress.
    let turn = store
        .upsert_run(run(&thread, RunStatus::Running))
        .await
        .unwrap();
    let agent = |id: &str, status| {
        let mut item = message(&thread, id, "");
        item.run_id = Some(turn.id.clone());
        item.status = status;
        item.kind = ItemKind::Agent {
            engine: None,
            thread: None,
            name: None,
            description: format!("task {id}").into(),
            prompt: None,
            agent_id: None,
            context_used: None,
            model: None,
            background: true,
            result: None,
            label: Some(id.into()),
        };
        item
    };
    for id in ["working", "finishing", "stopping"] {
        store.upsert_item(agent(id, Status::Running)).await.unwrap();
    }
    let listed = |summary: artisan_api::InspectorSummary| -> Vec<String> {
        summary
            .agents
            .into_iter()
            .map(|agent| agent.label)
            .collect()
    };
    assert_eq!(
        listed(store.inspector(&thread).await.unwrap()),
        ["stopping", "finishing", "working"]
    );
    // Done and cancelled agents go now, not when the next turn starts.
    store
        .upsert_item(agent("finishing", Status::Done))
        .await
        .unwrap();
    store
        .upsert_item(agent("stopping", Status::Cancelled))
        .await
        .unwrap();
    assert_eq!(listed(store.inspector(&thread).await.unwrap()), ["working"]);
}

#[tokio::test]
async fn an_engine_subagent_shows_the_threads_settings_with_its_own_model() {
    let dir = TempDir::new();
    let (store, thread) = test_store(&dir.0).await;
    let mut item = message(&thread, "helper", "");
    item.status = Status::Running;
    item.kind = ItemKind::Agent {
        engine: None,
        thread: None,
        name: None,
        description: "Fix the laggy bottom bar".into(),
        prompt: None,
        agent_id: None,
        context_used: None,
        model: Some("claude-haiku-5-5".into()),
        background: true,
        result: None,
        label: Some("Solveig".into()),
    };
    store.upsert_item(item).await.unwrap();
    let summary = store.inspector(&thread).await.unwrap();
    let config = summary.agents[0].config.clone().expect("agent settings");
    assert_eq!(config.engine, EngineId::Claude);
    assert_eq!(config.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(config.permissions, artisan_api::Permissions::Auto);
}

#[tokio::test]
async fn a_newer_thread_page_slides_back_up_from_its_cursor() {
    let dir = TempDir::new();
    let store = Store::open(&dir.0).unwrap();
    for _ in 0..5 {
        add_thread(&store).await;
    }
    let all = store
        .threads_page(artisan_api::ThreadsQuery::default())
        .await
        .unwrap()
        .threads;
    assert_eq!(all.len(), 5);
    let ids = |page: &artisan_api::ThreadsPage| -> Vec<ThreadId> {
        page.threads
            .iter()
            .map(|thread| thread.id.clone())
            .collect()
    };
    let after = |thread: &artisan_api::Thread, limit| artisan_api::ThreadsQuery {
        after_activity: Some(thread.activity_seq),
        after_id: Some(thread.id.clone()),
        limit: Some(limit),
        ..Default::default()
    };
    // The two just above the fourth newest, newest first; more above them.
    let page = store.threads_page(after(&all[3], 2)).await.unwrap();
    assert_eq!(ids(&page), [all[1].id.clone(), all[2].id.clone()]);
    assert!(page.newer);
    assert!(page.next.is_none());
    // Up to the newest: nothing above it.
    let page = store.threads_page(after(&all[1], 2)).await.unwrap();
    assert_eq!(ids(&page), [all[0].id.clone()]);
    assert!(!page.newer);
}

/// A run left live with an engine session, as a Forge that died mid-turn
/// leaves it.
#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn live_run_with_session(store: &Store, thread: &ThreadId) -> Run {
    let mut live = run(thread, RunStatus::Running);
    live.session_id = Some("session".into());
    store.upsert_run(live).await.unwrap()
}

#[tokio::test]
async fn a_turn_a_restart_cut_short_carries_on_by_itself() {
    let dir = TempDir::new();
    let (thread, without_session, waiting) = {
        let (store, thread) = test_store(&dir.0).await;
        live_run_with_session(&store, &thread).await;
        // The engine never named a session: nothing to resume.
        let without_session = add_thread(&store).await;
        store
            .upsert_run(run(&without_session, RunStatus::Running))
            .await
            .unwrap();
        // An undelivered message starts the next run anyway.
        let waiting = add_thread(&store).await;
        live_run_with_session(&store, &waiting).await;
        let mut input = message(&waiting, "pending", "not delivered");
        input.status = Status::Running;
        store.insert_user_message(input).await.unwrap();
        (thread, without_session, waiting)
    };
    let store = Store::open(&dir.0).unwrap();
    assert!(store.continue_pending(&thread).await.unwrap());
    assert!(!store.continue_pending(&without_session).await.unwrap());
    assert!(!store.continue_pending(&waiting).await.unwrap());
    let next = store.pending_threads(8, Vec::new()).await.unwrap();
    assert!(next.contains(&thread));
    assert!(next.contains(&waiting));
    assert_eq!(
        store.take_continue(thread.clone()).await.unwrap(),
        Some(artisan_api::Continuation::Restart)
    );
    assert!(!store.continue_pending(&thread).await.unwrap());
}

#[tokio::test]
async fn a_turn_that_keeps_taking_forge_down_stops_carrying_on() {
    let dir = TempDir::new();
    let thread = {
        let (store, thread) = test_store(&dir.0).await;
        for _ in 0..forge_store::MAX_RESTART_CONTINUATIONS - 1 {
            let mut continued = run(&thread, RunStatus::Interrupted);
            continued.continuation = Some(artisan_api::Continuation::Restart);
            store.upsert_run(continued).await.unwrap();
        }
        let mut live = live_run_with_session(&store, &thread).await;
        live.continuation = Some(artisan_api::Continuation::Restart);
        store.upsert_run(live).await.unwrap();
        thread
    };
    let store = Store::open(&dir.0).unwrap();
    assert!(!store.continue_pending(&thread).await.unwrap());
}

#[tokio::test]
async fn turning_continue_after_restart_off_leaves_interrupted_turns_alone() {
    let dir = TempDir::new();
    let thread = {
        let (store, thread) = test_store(&dir.0).await;
        store
            .set_setting(
                forge_store::CONTINUE_AFTER_RESTART_KEY.into(),
                serde_json::Value::Bool(false),
            )
            .await
            .unwrap();
        live_run_with_session(&store, &thread).await;
        thread
    };
    let store = Store::open(&dir.0).unwrap();
    assert_eq!(
        store.latest_run(&thread).await.unwrap().unwrap().status,
        RunStatus::Interrupted
    );
    assert!(!store.continue_pending(&thread).await.unwrap());
}

#[tokio::test]
async fn an_agent_forge_started_keeps_working_through_a_restart() {
    let dir = TempDir::new();
    let (parent, child) = {
        let (store, parent) = test_store(&dir.0).await;
        let child = add_thread(&store).await;
        let live = live_run_with_session(&store, &parent).await;
        let mut agent = reply(&parent, "agent", Some(&live.id), "", Status::Running);
        agent.kind = ItemKind::Agent {
            name: None,
            description: "Review the diff".into(),
            prompt: None,
            agent_id: None,
            context_used: None,
            model: None,
            background: true,
            result: None,
            label: None,
            engine: Some(EngineId::Codex),
            thread: Some(child.clone()),
        };
        store.upsert_item(agent).await.unwrap();
        live_run_with_session(&store, &child).await;
        (parent, child)
    };
    let store = Store::open(&dir.0).unwrap();
    let agent = store.item(&ItemId::from("agent")).await.unwrap().unwrap();
    assert_eq!(agent.status, Status::Running);
    assert!(store.continue_pending(&parent).await.unwrap());
    assert!(store.continue_pending(&child).await.unwrap());
}
