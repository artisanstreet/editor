use super::{Job, Store};
use crate::{Connections, DataLock, Environment};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::time::Instant;

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other statement is authorised as usual"
)]
fn store_whose_recall_batches_fail(dir: &std::path::Path) -> (Store, DataLock) {
    let connections = Connections::new(|path| {
        let mut conn = rusqlite::Connection::open(path)?;
        conn.authorizer(Some(|context: AuthContext<'_>| match context.action {
            AuthAction::Update {
                table_name: "recall_index",
                column_name: "building_after",
            } => Authorization::Deny,
            _ => Authorization::Allow,
        }))?;
        Ok(conn)
    });
    let lock = DataLock::acquire(dir).expect("hold the data folder");
    let store = Store::open_restoring_in(
        dir,
        &lock,
        Environment {
            connections,
            ..Environment::system()
        },
    )
    .expect("open the store");
    (store, lock)
}

#[test]
fn the_last_store_dropped_on_the_writer_thread_does_not_wait_for_the_recall_builder() {
    let dir = tempfile::tempdir().expect("create the data folder");
    let (store, _lock) = store_whose_recall_batches_fail(dir.path());
    assert_eq!(
        *store.recall_index().borrow(),
        crate::RecallIndex::Building,
        "the recall builder is running"
    );
    let jobs = store.inner.jobs.clone();
    let room = jobs.max_capacity();
    let (running, writer_held) = std::sync::mpsc::channel::<()>();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (dropped, dropping) = std::sync::mpsc::channel::<()>();
    let last: Job = Box::new(move |_writer| {
        running.send(()).expect("the test waits for the writer");
        released
            .recv_timeout(test_support::wait::LIVENESS)
            .expect("the test releases the writer");
        drop(store);
        dropped.send(()).expect("the test waits for the drop");
    });
    jobs.blocking_send(last)
        .expect("queue the job holding the last store");
    writer_held
        .recv_timeout(test_support::wait::LIVENESS)
        .expect("the writer runs the job holding the last store");
    let deadline = Instant::now() + test_support::wait::LIVENESS;
    while jobs.capacity() == room {
        assert!(
            Instant::now() < deadline,
            "the recall builder queues its next batch behind the writer"
        );
        std::thread::yield_now();
    }
    drop(jobs);

    release.send(()).expect("release the writer");

    dropping
        .recv_timeout(test_support::wait::LIVENESS)
        .expect("dropping the last store on the writer thread finishes");
}
