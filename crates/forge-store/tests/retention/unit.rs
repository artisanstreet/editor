use super::still_removable;
use crate::Store;
use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId, now};
use std::collections::HashSet;

#[tokio::test]
async fn a_family_whose_idleness_could_not_be_checked_is_not_removed() {
    let dir = tempfile::tempdir().expect("create the data folder");
    let store = Store::open(dir.path()).expect("open the store");
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .expect("add a project")
        .id;
    let idle = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project,
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .expect("add an idle thread")
        .id;
    drop(store);
    let mut conn = rusqlite::Connection::open(dir.path().join("forge.sqlite3"))
        .expect("open the database beside the store");
    conn.prepare_cached(super::ELIGIBLE)
        .expect("prepare the eligibility check before interrupting it");
    let mut interrupted = false;
    conn.progress_handler(
        1,
        Some(move || {
            let interrupt = !interrupted;
            interrupted = true;
            interrupt
        }),
    )
    .expect("interrupt the eligibility check once");

    let checked = still_removable(&conn, now() + 60_000, 0, &HashSet::new(), &[idle]);

    assert!(
        checked.is_err(),
        "a failed eligibility check must fail retention, not remove the family: {checked:?}"
    );
}
