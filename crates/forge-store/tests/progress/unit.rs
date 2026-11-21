use super::{Progress, Step, unwatch, watch};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn beats_while_busy(step: Step) -> usize {
    let mut conn = rusqlite::Connection::open_in_memory().expect("open a database");
    let beats = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&beats);
    let pace = Progress::every(Duration::ZERO).with_heartbeat(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    watch(&mut conn, step, &pace).expect("watch the step");
    let counted: i64 = conn
        .query_row(include_str!("busy.sql"), [], |row| row.get(0))
        .expect("run a long statement");
    assert_eq!(counted, 200_000, "the statement ran to the end");
    unwatch(&mut conn).expect("stop watching");
    beats.load(Ordering::SeqCst)
}

#[test]
fn a_long_recall_index_reset_never_asks_systemd_for_more_time() {
    assert_eq!(beats_while_busy(Step::RecallIndex), 0);
}

#[test]
fn a_long_upgrade_keeps_asking_systemd_for_more_time_while_it_runs() {
    assert!(beats_while_busy(Step::Migration { next: 2 }) > 1);
}
