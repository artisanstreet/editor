use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId};
use forge_store::{OpenStoreError, Store};
use std::path::Path;

const NEWER: usize = 1000;

async fn thread(store: &Store, title: &str) {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .expect("create the project");
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Claude),
            title: Some(title.into()),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .expect("create the thread");
}

async fn titles(store: &Store) -> Vec<String> {
    store
        .state()
        .await
        .expect("read the state")
        .threads
        .into_iter()
        .filter_map(|thread| thread.title)
        .collect()
}

fn raw(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.join("forge.sqlite3")).expect("open the database directly")
}

fn version(dir: &Path) -> usize {
    raw(dir)
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read the schema version")
}

async fn upgraded_by_a_newer_forge(dir: &Path) {
    thread(
        &Store::open(dir).expect("this Forge creates its database"),
        "Before the upgrade",
    )
    .await;
    let backup = dir.join(format!("forge.sqlite3.schema-{}", Store::schema_version()));
    raw(dir)
        .execute(
            include_str!("rollbacks/back_up.sql"),
            [backup.to_str().expect("a Unicode temp path")],
        )
        .expect("the newer Forge backs up the older schema");
    thread(&Store::open(dir).expect("reopen"), "After the upgrade").await;
    raw(dir)
        .execute_batch(include_str!("rollbacks/newer.sql"))
        .expect("the newer Forge stamps its schema");
}

#[tokio::test(flavor = "multi_thread")]
async fn ae_and_other_openers_refuse_a_newer_database_and_leave_it_alone() {
    let dir = tempfile::tempdir().expect("create the data folder");
    upgraded_by_a_newer_forge(dir.path()).await;

    let refused = Store::open(dir.path()).expect_err("a plain open refuses");
    assert!(
        matches!(refused, OpenStoreError::Migrate { .. }),
        "{refused:?}"
    );
    let refused = Store::open_current(dir.path()).expect_err("ae's open refuses");
    assert!(
        matches!(refused, OpenStoreError::Mismatch { found: NEWER, .. }),
        "{refused:?}"
    );
    assert_eq!(version(dir.path()), NEWER, "the newer database stays");
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .expect("list the data folder")
        .map(|entry| {
            entry
                .expect("read an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        !names.iter().any(|name| name.contains("rolled-back")),
        "nothing was rolled back: {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn forge_rolls_back_a_newer_database_and_reports_it_until_dismissed() {
    let dir = tempfile::tempdir().expect("create the data folder");
    upgraded_by_a_newer_forge(dir.path()).await;
    let lock = forge_store::DataLock::acquire(dir.path()).expect("hold the data lock");

    let store = Store::open_restoring(dir.path(), &lock).expect("Forge rolls back");
    assert_eq!(titles(&store).await, ["Before the upgrade"]);
    let rollback = store
        .state()
        .await
        .expect("read the state")
        .database_rollback
        .expect("the state says the database was rolled back");
    assert_eq!(
        (rollback.found, rollback.restored),
        (
            u32::try_from(NEWER).expect("small"),
            u32::try_from(Store::schema_version()).expect("small")
        )
    );
    assert!(
        Path::new(&rollback.set_aside).is_file(),
        "the newer data is kept at {}",
        rollback.set_aside
    );
    drop(store);

    let store = Store::open_restoring(dir.path(), &lock).expect("Forge starts again");
    assert!(
        store
            .state()
            .await
            .expect("read the state")
            .database_rollback
            .is_some(),
        "the notice survives a restart"
    );
    store.dismiss_rollback().await.expect("dismiss the notice");
    assert_eq!(
        store
            .state()
            .await
            .expect("read the state")
            .database_rollback,
        None
    );
}
