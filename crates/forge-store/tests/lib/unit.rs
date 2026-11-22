use super::Store;
use crate::recall::build::StepClock;
use crate::{Connections, DataLock, Environment, Pacing, RetryPace};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn retrying_at_once() -> Pacing {
    Pacing {
        retry: RetryPace {
            first: Duration::ZERO,
            last: Duration::ZERO,
        },
        ..Pacing::system()
    }
}

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
            recall: retrying_at_once(),
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
    let beats = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&beats);
    let lock = DataLock::acquire(dir.path()).expect("hold the data folder");

    let store = Store::open_restoring_in(
        &lock,
        Environment {
            progress: crate::Progress::every(Duration::ZERO).with_heartbeat(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
            ..Environment::system()
        },
    )
    .expect("the next start opens the current database");

    drop(store);
    assert_eq!(
        beats.load(Ordering::SeqCst),
        0,
        "only a start that backs up or upgrades the database extends systemd's start timeout"
    );
}

type Gate = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other statement is authorised as usual"
)]
fn connections_holding_the_first_recall_step(gate: Gate) -> Connections {
    let gate = Arc::new(Mutex::new(Some(gate)));
    Connections::new(move |path| {
        let mut conn = rusqlite::Connection::open(path)?;
        let gate = Arc::clone(&gate);
        conn.authorizer(Some(move |context: AuthContext<'_>| match context.action {
            AuthAction::Update {
                table_name: "recall_index",
                column_name: "building_after",
            } => {
                let first = gate
                    .lock()
                    .expect("the test never panics holding the gate")
                    .take();
                if let Some((reached, open)) = first {
                    reached.send(()).expect("the test waits for the step");
                    open.recv_timeout(test_support::wait::LIVENESS)
                        .expect("the test opens the gate");
                }
                Authorization::Allow
            }
            _ => Authorization::Allow,
        }))?;
        Ok(conn)
    })
}

fn ledger_entry(thread: &artisan_api::ThreadId, text: String) -> crate::NewItem {
    crate::NewItem {
        id: artisan_api::ItemId::new(),
        thread_id: thread.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status: artisan_api::Status::Done,
        kind: artisan_api::ItemKind::AssistantMessage {
            text: text.into(),
            phase: artisan_api::Phase::Final,
        },
    }
}

fn ticking_clock(tick: Duration) -> StepClock {
    let start = std::time::Instant::now();
    let ticks = std::sync::atomic::AtomicU32::new(0);
    StepClock::new(move || start + tick * ticks.fetch_add(1, Ordering::SeqCst))
}

fn count(store: &Store, query: &'static str) -> impl Future<Output = i64> + use<> {
    use crate::error::AtStep as _;
    let store = store.clone();
    async move {
        store
            .write(move |tx| {
                tx.query_row(query, [], |row| row.get::<_, i64>(0))
                    .at("count rows")
            })
            .await
            .expect("count rows")
    }
}

async fn ledger_to_rebuild(dir: &std::path::Path, backlog: usize) -> artisan_api::ThreadId {
    use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId};
    let store = Store::open(dir).expect("open the store");
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .expect("create a project");
    let thread = store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Claude),
            title: Some("Ledger".into()),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .expect("create a thread")
        .id;
    for number in 0..backlog {
        store
            .upsert_item(ledger_entry(&thread, format!("ledger entry {number}")))
            .await
            .expect("save a ledger entry");
    }
    drop(store);
    rusqlite::Connection::open(dir.join("forge.sqlite3"))
        .expect("open the database beside the store")
        .execute_batch(include_str!("../recall/forget_index.sql"))
        .expect("make the next start rebuild the recall index");
    thread
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_go_ahead_of_a_rebuild_step_once_its_time_runs_out_and_the_rebuild_misses_nothing() {
    const BACKLOG: usize = 600;
    const TICK: Duration = Duration::from_millis(1);
    const STEP_TICKS: u32 = 10;
    let dir = tempfile::tempdir().expect("create the data folder");
    let thread = ledger_to_rebuild(dir.path(), BACKLOG).await;

    let (reached, reaching) = std::sync::mpsc::channel();
    let (open, opening) = std::sync::mpsc::channel();
    let lock = DataLock::acquire(dir.path()).expect("hold the data folder");
    let store = Store::open_restoring_in(
        &lock,
        Environment {
            connections: connections_holding_the_first_recall_step((reached, opening)),
            recall: Pacing {
                step: TICK * STEP_TICKS,
                clock: ticking_clock(TICK),
                ..retrying_at_once()
            },
            ..Environment::system()
        },
    )
    .expect("Forge starts and rebuilds the recall index in the background");
    tokio::task::spawn_blocking(move || reaching.recv_timeout(test_support::wait::LIVENESS))
        .await
        .expect("wait for the first recall step")
        .expect("the first recall step reaches the gate");

    let indexed_when_written = tokio::spawn(count(&store, include_str!("indexed_items.sql")));
    test_support::wait::wait_until(
        "the count to queue behind the held recall step",
        test_support::wait::LIVENESS,
        || store.inner.jobs.capacity() == super::WRITE_QUEUE_CAPACITY - 1,
    );
    let written = tokio::spawn({
        let store = store.clone();
        let entry = ledger_entry(&thread, "written during the rebuild".to_owned());
        async move { store.upsert_item(entry).await }
    });
    test_support::wait::wait_until(
        "the write to queue behind the held recall step",
        test_support::wait::LIVENESS,
        || store.inner.jobs.capacity() == super::WRITE_QUEUE_CAPACITY - 2,
    );
    open.send(())
        .expect("the held recall step waits for the gate");

    assert_eq!(
        indexed_when_written.await.expect("the count finishes"),
        i64::from(STEP_TICKS - 2),
        "the queued writes ran once the step's time was up, counting its setup and keeping its slowest row in reserve, not after a whole batch of the backlog"
    );
    let written = written
        .await
        .expect("the write task finishes")
        .expect("the write made during the rebuild commits");

    let mut index = store.recall_index();
    let ended = *tokio::time::timeout(
        test_support::wait::LIVENESS,
        index.wait_for(|index| *index != crate::RecallIndex::Building),
    )
    .await
    .expect("the rebuild ends in time")
    .expect("the rebuild reports how it ended");
    assert_eq!(ended, crate::RecallIndex::Built);
    assert_eq!(
        (
            count(&store, include_str!("unindexed_items.sql")).await,
            count(&store, include_str!("indexed_items.sql")).await,
        ),
        (0, i64::try_from(BACKLOG).expect("a small backlog") + 1),
        "every item is indexed exactly once across the step boundaries, including {}",
        written.id
    );
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other statement is authorised as usual"
)]
fn connections_pausing_the_second_step(gate: Gate, begins: Arc<AtomicUsize>) -> Connections {
    let gate = Arc::new(Mutex::new(Some(gate)));
    let armed = Arc::new(AtomicBool::new(false));
    Connections::new(move |path| {
        let mut conn = rusqlite::Connection::open(path)?;
        let (gate, armed, begins) = (Arc::clone(&gate), Arc::clone(&armed), Arc::clone(&begins));
        conn.authorizer(Some(move |context: AuthContext<'_>| match context.action {
            AuthAction::Update {
                table_name: "recall_index",
                column_name: "building_after",
            } => {
                armed.store(true, Ordering::SeqCst);
                Authorization::Allow
            }
            AuthAction::Transaction {
                operation: TransactionOperation::Begin,
            } if armed.load(Ordering::SeqCst) => {
                begins.fetch_add(1, Ordering::SeqCst);
                let first = gate
                    .lock()
                    .expect("the test never panics holding the gate")
                    .take();
                if let Some((reached, open)) = first {
                    reached
                        .send(())
                        .expect("the test waits for the second step");
                    open.recv_timeout(test_support::wait::LIVENESS)
                        .expect("the test lets the second step begin");
                }
                Authorization::Allow
            }
            _ => Authorization::Allow,
        }))?;
        Ok(conn)
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rebuild_step_gives_up_on_a_write_lock_held_elsewhere_and_carries_on_once_it_is_free() {
    const BACKLOG: usize = 200;
    let dir = tempfile::tempdir().expect("create the data folder");
    ledger_to_rebuild(dir.path(), BACKLOG).await;
    let (reached, reaching) = std::sync::mpsc::channel();
    let (open, opening) = std::sync::mpsc::channel();
    let begins = Arc::new(AtomicUsize::new(0));
    let lock = DataLock::acquire(dir.path()).expect("hold the data folder");
    let store = Store::open_restoring_in(
        &lock,
        Environment {
            connections: connections_pausing_the_second_step(
                (reached, opening),
                Arc::clone(&begins),
            ),
            recall: Pacing {
                retry: RetryPace {
                    first: Duration::from_millis(20),
                    last: Duration::from_secs(10),
                },
                step: Duration::from_millis(50),
                clock: ticking_clock(Duration::from_millis(1)),
            },
            ..Environment::system()
        },
    )
    .expect("Forge starts and rebuilds the recall index in the background");
    tokio::task::spawn_blocking(move || reaching.recv_timeout(test_support::wait::LIVENESS))
        .await
        .expect("wait for the second recall step")
        .expect("the second recall step reaches the gate");

    let mut holder = rusqlite::Connection::open(dir.path().join("forge.sqlite3"))
        .expect("open a second connection to the database");
    let held = holder
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .expect("the second connection takes the write lock");
    let (holding, attempted) = (std::time::Instant::now(), begins.load(Ordering::SeqCst));
    open.send(()).expect("the second step waits for the gate");
    test_support::wait::wait_until(
        "ten more rebuild steps to try for the held write lock",
        test_support::wait::LIVENESS,
        || begins.load(Ordering::SeqCst) >= attempted + 10,
    );
    assert!(
        holding.elapsed() < super::BUSY_TIMEOUT,
        "each step gave up on the held lock within its budget and tried again at the first retry pace, without backing off, instead of waiting {:?}: took {:?}",
        super::BUSY_TIMEOUT,
        holding.elapsed()
    );
    assert_eq!(
        *store.recall_index().borrow(),
        crate::RecallIndex::Building,
        "a held lock pauses the rebuild without ending it"
    );
    held.commit()
        .expect("the second connection lets the write lock go");

    let mut index = store.recall_index();
    let ended = *tokio::time::timeout(
        test_support::wait::LIVENESS,
        index.wait_for(|index| *index != crate::RecallIndex::Building),
    )
    .await
    .expect("the rebuild ends in time")
    .expect("the rebuild reports how it ended");
    assert_eq!(ended, crate::RecallIndex::Built);
    assert_eq!(
        count(&store, include_str!("unindexed_items.sql")).await,
        0,
        "the rebuild picks up where it left off once the lock is free"
    );
    assert_eq!(
        count(&store, include_str!("busy_timeout.sql")).await,
        i64::try_from(super::BUSY_TIMEOUT.as_millis()).expect("a small timeout"),
        "user writes wait the full busy timeout again after the rebuild's short waits"
    );
}
