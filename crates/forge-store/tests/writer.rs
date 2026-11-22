use std::time::Duration;

const MIB: usize = 1024 * 1024;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_writer_queues_at_most_its_byte_budget() {
    let directory = tempfile::tempdir().unwrap();
    let store = forge_store::Store::open(directory.path()).unwrap();
    let blocker = rusqlite::Connection::open(directory.path().join("forge.sqlite3")).unwrap();
    blocker
        .execute_batch(include_str!("writer/begin_immediate.sql"))
        .unwrap();
    let writes: Vec<_> = (0..128)
        .map(|n| {
            let store = store.clone();
            tokio::spawn(async move {
                store
                    .set_setting(
                        format!("large-{n}"),
                        serde_json::Value::String("v".repeat(MIB)),
                    )
                    .await
            })
        })
        .collect();
    tokio::time::timeout(Duration::from_secs(4), async {
        while store.queued_write_bytes() < 60 * MIB {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stalled writer fills its byte budget");
    for _ in 0..1000 {
        tokio::task::yield_now().await;
    }
    assert!(
        store.queued_write_bytes() <= 64 * MIB,
        "{} bytes queued behind a stalled writer",
        store.queued_write_bytes()
    );
    blocker
        .execute_batch(include_str!("writer/rollback.sql"))
        .unwrap();
    let mut saved = 0;
    for write in writes {
        let result = tokio::time::timeout(Duration::from_secs(30), write)
            .await
            .expect("every queued write runs once the writer is free")
            .unwrap();
        saved += usize::from(result.is_ok());
    }
    assert!(saved > 0, "the settings fit in their own budget");
    assert_eq!(store.queued_write_bytes(), 0);
}

const ALLOW: u8 = 0;
const REFUSE_READING_THREAD_IDS: u8 = 1;
const REFUSE_SAVING_ITEMS: u8 = 2;

fn refusing_store(
    dir: &std::path::Path,
) -> (
    forge_store::Store,
    forge_store::DataLock,
    std::sync::Arc<std::sync::atomic::AtomicU8>,
) {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::atomic::{AtomicU8, Ordering};
    let refusal = std::sync::Arc::new(AtomicU8::new(ALLOW));
    let refusing = std::sync::Arc::clone(&refusal);
    let connections = forge_store::Connections::new(move |path| {
        let mut conn = rusqlite::Connection::open(path)?;
        let refusing = std::sync::Arc::clone(&refusing);
        conn.authorizer(Some(move |context: AuthContext<'_>| {
            match (refusing.load(Ordering::SeqCst), context.action) {
                (
                    REFUSE_READING_THREAD_IDS,
                    AuthAction::Read {
                        table_name: "threads",
                        column_name: "id",
                    },
                )
                | (
                    REFUSE_SAVING_ITEMS,
                    AuthAction::Insert {
                        table_name: "items",
                    },
                ) => Authorization::Deny,
                _ => Authorization::Allow,
            }
        }))?;
        Ok(conn)
    });
    let lock = forge_store::DataLock::acquire(dir).expect("hold the data folder");
    let store = forge_store::Store::open_restoring_in(
        &lock,
        forge_store::Environment {
            connections,
            ..forge_store::Environment::system()
        },
    )
    .expect("open the store whose connections refuse on request");
    (store, lock, refusal)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_item_save_says_which_step_of_saving_it_failed() {
    use artisan_api::{
        CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
        Status, ThreadId,
    };
    use forge_store::{StorageError, WriteFailure};
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let (store, _lock, refusal) = refusing_store(dir.path());
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/steps".into(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Codex),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    let reply = forge_store::NewItem {
        id: ItemId::new(),
        thread_id: thread.id.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status: Status::Done,
        kind: ItemKind::AssistantMessage {
            text: "done".into(),
            phase: Phase::Final,
        },
    };
    let failed_step = |error: forge_store::UpsertItemError| {
        let shown = diagnostics::chain(&error);
        let WriteFailure::Storage(StorageError::Sql { step, .. }) = *error.cause else {
            panic!("not a step of saving: {shown}")
        };
        step
    };

    refusal.store(REFUSE_READING_THREAD_IDS, Ordering::SeqCst);
    let checking = store.upsert_item(reply.clone()).await.unwrap_err();
    refusal.store(REFUSE_SAVING_ITEMS, Ordering::SeqCst);
    let saving = store.upsert_item(reply.clone()).await.unwrap_err();
    refusal.store(ALLOW, Ordering::SeqCst);

    assert_eq!(failed_step(checking), "state/thread_exists.sql");
    assert_eq!(failed_step(saving), "items/save.sql");
    store
        .upsert_item(reply)
        .await
        .expect("the item saves once nothing refuses it");
}

struct StalledWriter {
    _directory: tempfile::TempDir,
    store: forge_store::Store,
    parent: artisan_api::ThreadId,
    project: artisan_api::ProjectId,
    blocker: rusqlite::Connection,
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn stalled_writer() -> StalledWriter {
    use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId};
    let directory = tempfile::tempdir().unwrap();
    let store = forge_store::Store::open(directory.path()).unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/agents".into(),
            name: None,
        })
        .await
        .unwrap()
        .id;
    let parent = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.clone(),
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap()
        .id;
    let blocker = rusqlite::Connection::open(directory.path().join("forge.sqlite3")).unwrap();
    blocker
        .execute_batch(include_str!("writer/begin_immediate.sql"))
        .unwrap();
    StalledWriter {
        _directory: directory,
        store,
        parent,
        project,
        blocker,
    }
}

fn agent_item(
    parent: &artisan_api::ThreadId,
    child: &artisan_api::ThreadId,
) -> forge_store::NewItem {
    use artisan_api::{EngineId, ItemId, ItemKind, Status};
    forge_store::NewItem {
        id: ItemId::new(),
        thread_id: parent.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status: Status::Running,
        kind: ItemKind::Agent {
            name: None,
            description: "review".into(),
            prompt: None,
            agent_id: None,
            context_used: None,
            model: None,
            background: false,
            result: None,
            label: None,
            engine: Some(EngineId::Codex),
            thread: Some(child.clone()),
        },
    }
}

fn large_message() -> artisan_api::SendMessage {
    artisan_api::SendMessage {
        id: artisan_api::ItemId::new(),
        text: "p".repeat(8 * MIB),
        images: Vec::new(),
        config: None,
        draft: None,
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn released_after_measuring<F: Future + Send>(
    store: &forge_store::Store,
    blocker: rusqlite::Connection,
    write: F,
) -> (usize, F::Output) {
    let mut write = std::pin::pin!(write);
    let mut queued = 0;
    std::future::poll_fn(|cx| {
        assert!(
            write.as_mut().poll(cx).is_pending(),
            "the write waits behind the stalled writer"
        );
        queued = store.queued_write_bytes();
        std::task::Poll::Ready(())
    })
    .await;
    blocker
        .execute_batch(include_str!("writer/rollback.sql"))
        .unwrap();
    drop(blocker);
    let output = write.await;
    assert_eq!(
        store.queued_write_bytes(),
        0,
        "a finished write gives its bytes back"
    );
    (queued, output)
}

#[tokio::test]
async fn a_queued_agent_spawn_holds_its_prompt_against_the_write_budget() {
    use artisan_api::{CreateThread, EngineConfig, EngineId, ThreadId};
    let stalled = stalled_writer().await;
    let child = ThreadId::new();
    let spawn = stalled.store.spawn_agent(
        agent_item(&stalled.parent, &child),
        CreateThread {
            id: child.clone(),
            project_id: stalled.project.clone(),
            config: EngineConfig::new(EngineId::Codex),
            title: None,
            parent_thread_id: Some(stalled.parent.clone()),
            spawned_by: None,
        },
        large_message(),
        forge_store::Acceptance::default(),
    );
    let (queued, spawned) = released_after_measuring(&stalled.store, stalled.blocker, spawn).await;
    assert!(
        queued >= 8 * MIB,
        "a queued 8 MiB agent prompt counted as {queued} bytes"
    );
    let (_, thread, prompt) = spawned.unwrap();
    assert_eq!(thread.id, child);
    assert_eq!(prompt.thread_id, child);
}

#[tokio::test]
async fn a_queued_agent_report_holds_its_text_against_the_write_budget() {
    let stalled = stalled_writer().await;
    let settle = stalled.store.settle_agent(
        None,
        forge_store::AgentReport {
            thread: stalled.parent.clone(),
            message: large_message(),
            acceptance: forge_store::Acceptance::default(),
        },
    );
    let (queued, settled) = released_after_measuring(&stalled.store, stalled.blocker, settle).await;
    assert!(
        queued >= 8 * MIB,
        "a queued 8 MiB agent report counted as {queued} bytes"
    );
    assert_eq!(settled.unwrap().0.thread_id, stalled.parent);
}

#[tokio::test]
async fn a_queued_first_setting_holds_its_value_against_the_write_budget() {
    let stalled = stalled_writer().await;
    let write = stalled
        .store
        .set_setting_if_absent("large".into(), serde_json::Value::String("v".repeat(MIB)));
    let (queued, saved) = released_after_measuring(&stalled.store, stalled.blocker, write).await;
    assert!(
        queued >= MIB,
        "a queued 1 MiB setting counted as {queued} bytes"
    );
    assert!(saved.unwrap());
}

#[tokio::test]
async fn a_queued_secret_holds_its_value_against_the_write_budget() {
    let stalled = stalled_writer().await;
    let write = stalled.store.set_secret("large", Some("s".repeat(8 * MIB)));
    let (queued, saved) = released_after_measuring(&stalled.store, stalled.blocker, write).await;
    assert!(
        queued >= 8 * MIB,
        "a queued 8 MiB secret counted as {queued} bytes"
    );
    saved.unwrap();
}
