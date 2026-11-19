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
