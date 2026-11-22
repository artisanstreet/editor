use super::Store;
use crate::{Connections, DataLock, Environment, RetryPace};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other statement is authorised as usual"
)]
fn connections_dropping_the_store_in_a_recall_batch(
    last: &Arc<Mutex<Option<Store>>>,
    dropped: &std::sync::mpsc::Sender<()>,
) -> Connections {
    let last = Arc::clone(last);
    let dropped = dropped.clone();
    Connections::new(move |path| {
        let mut conn = rusqlite::Connection::open(path)?;
        let last = Arc::clone(&last);
        let dropped = dropped.clone();
        conn.authorizer(Some(move |context: AuthContext<'_>| match context.action {
            AuthAction::Update {
                table_name: "recall_index",
                column_name: "building_after",
            } => {
                let taken = last
                    .lock()
                    .expect("the test never panics holding the store")
                    .take();
                if let Some(store) = taken {
                    drop(store);
                    dropped.send(()).expect("the test waits for the drop");
                }
                Authorization::Deny
            }
            _ => Authorization::Allow,
        }))?;
        Ok(conn)
    })
}

#[test]
fn the_last_store_dropped_on_the_writer_thread_does_not_wait_for_the_recall_builder() {
    let dir = tempfile::tempdir().expect("create the data folder");
    let last = Arc::new(Mutex::new(None));
    let (dropped, dropping) = std::sync::mpsc::channel();
    let lock = DataLock::acquire(dir.path()).expect("hold the data folder");
    let store = Store::open_restoring_in(
        &lock,
        Environment {
            connections: connections_dropping_the_store_in_a_recall_batch(&last, &dropped),
            recall_retry: RetryPace {
                first: Duration::ZERO,
                last: Duration::ZERO,
            },
            ..Environment::system()
        },
    )
    .expect("open the store");
    assert_eq!(
        *store.recall_index().borrow(),
        crate::RecallIndex::Building,
        "the recall builder is running"
    );

    *last.lock().expect("the store slot is free") = Some(store);

    dropping
        .recv_timeout(test_support::wait::LIVENESS)
        .expect("dropping the last store inside the builder's batch on the writer thread finishes");
}

#[test]
fn a_start_with_no_database_upgrade_never_asks_systemd_for_more_time() {
    let dir = tempfile::tempdir().expect("create the data folder");
    drop(Store::open(dir.path()).expect("the first start creates the database"));
    rusqlite::Connection::open(dir.path().join("forge.sqlite3"))
        .expect("open the database beside the store")
        .execute_batch(include_str!("../recall/forget_index.sql"))
        .expect("make the next start reset the recall index");
    let beats = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&beats);
    let lock = DataLock::acquire(dir.path()).expect("hold the data folder");

    let store = Store::open_restoring_in(
        &lock,
        Environment {
            progress: crate::Progress::every(Duration::ZERO).with_heartbeat(move || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
            ..Environment::system()
        },
    )
    .expect("the next start opens the current database");

    drop(store);
    assert_eq!(
        beats.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "only a start that backs up or upgrades the database extends systemd's start timeout"
    );
}
