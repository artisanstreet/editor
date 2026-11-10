#![cfg(test)]

use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, Run, RunId, RunStatus,
    ThreadId, Usage, now,
};
use forge_store::Store;

async fn running(store: &Store) -> Run {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/repo".into(),
            name: None,
        })
        .await
        .unwrap();
    let config = EngineConfig::new(EngineId::Codex);
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: config.clone(),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    store
        .upsert_run(Run {
            id: RunId::new(),
            thread_id: thread.id,
            config: Some(config),
            engine: EngineId::Codex,
            model: None,
            status: RunStatus::Running,
            usage: Usage::default(),
            session_id: Some("live-session".into()),
            error: None,
            seq: 0,
            input_cutoff: 0,
            started_at: now(),
            ended_at: None,
            continuation: None,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn joining_preserves_running_agents_and_serializes_independent_writers() {
    let directory = tempfile::tempdir().unwrap();
    let first = Store::open(directory.path()).unwrap();
    let run = running(&first).await;
    let second = Store::join(directory.path()).unwrap();
    assert_eq!(
        second.latest_run(&run.thread_id).await.unwrap().unwrap(),
        run
    );
    let write = |store: Store| async move {
        let mut sequences = Vec::new();
        for index in 0..50 {
            sequences.push(
                store
                    .create_project(CreateProject {
                        id: ProjectId::new(),
                        path: format!("/repo/{index}"),
                        name: None,
                    })
                    .await
                    .unwrap()
                    .seq,
            );
        }
        sequences
    };
    let (a, b) = tokio::join!(write(first.clone()), write(second.clone()));
    let mut sequences = a;
    sequences.extend(b);
    sequences.sort_unstable();
    sequences.dedup();
    assert_eq!(sequences.len(), 100);
    assert!(sequences[0] > run.seq);
    assert_eq!(
        first
            .latest_run(&run.thread_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        RunStatus::Running
    );
}

#[tokio::test]
async fn joined_generations_replay_external_and_local_commits_in_sequence_order() {
    let directory = tempfile::tempdir().unwrap();
    let first = Store::open(directory.path()).unwrap();
    let second = Store::join(directory.path()).unwrap();
    let mut cursor = second.head();
    let mut changes = second.subscribe();
    let a = first
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/a".into(),
            name: None,
        })
        .await
        .unwrap();
    let b = second
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/b".into(),
            name: None,
        })
        .await
        .unwrap();
    second.synchronize(&mut cursor).await.unwrap();
    assert_eq!(changes.recv().await.unwrap().seq, Some(a.seq));
    assert_eq!(changes.recv().await.unwrap().seq, Some(b.seq));
    assert_eq!(cursor, b.seq);
    second.synchronize(&mut cursor).await.unwrap();
    assert!(changes.try_recv().is_err());
}

#[tokio::test]
async fn abandoned_run_recovery_cannot_interrupt_another_generations_run() {
    let directory = tempfile::tempdir().unwrap();
    let first = Store::open(directory.path()).unwrap();
    let abandoned = running(&first).await;
    let healthy = running(&first).await;
    let replacement = Store::join(directory.path()).unwrap();
    replacement
        .recover_thread(abandoned.thread_id.clone())
        .await
        .unwrap();
    assert_eq!(
        first
            .latest_run(&abandoned.thread_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        RunStatus::Interrupted
    );
    assert_eq!(
        first.latest_run(&healthy.thread_id).await.unwrap().unwrap(),
        healthy
    );
}

#[tokio::test]
async fn incompatible_candidate_cannot_migrate_or_recover_the_active_store() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let run = running(&store).await;
    let connection = rusqlite::Connection::open(directory.path().join("forge.sqlite3")).unwrap();
    connection
        .pragma_update(None, "user_version", 9000)
        .unwrap();
    assert!(Store::join(directory.path()).is_err());
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u64>(0))
            .unwrap(),
        9000
    );
    assert_eq!(
        store.latest_run(&run.thread_id).await.unwrap().unwrap(),
        run
    );
}
