//! Focused coverage for the synchronous thread-session retention boundary.

#[path = "../../modules/frontend/src/session_projection.rs"]
mod session_projection;

use session_projection::{MAX_RETAINED_SESSIONS, ThreadSessionProjection, ThreadSessionSnapshot};

type Projection = ThreadSessionProjection<String, usize>;
type Snapshot = ThreadSessionSnapshot<String, usize>;

fn key(id: usize) -> String {
    format!("thread-{id}")
}

fn snapshot(id: usize, value: usize) -> Snapshot {
    Snapshot::new(key(id), value)
}

fn retained_ids(projection: &Projection) -> Vec<usize> {
    projection
        .snapshots()
        .iter()
        .map(|snapshot| {
            snapshot
                .thread_id()
                .strip_prefix("thread-")
                .expect("fixture key has a thread prefix")
                .parse()
                .expect("fixture key has a numeric suffix")
        })
        .collect()
}

#[test]
fn empty_projection_has_no_current_value() {
    let projection = Projection::new();

    assert!(projection.is_empty());
    assert_eq!(projection.len(), 0);
    assert!(projection.current(&key(0)).is_none());
    assert!(projection.current_value("thread-0").is_none());
    assert!(projection.snapshots().is_empty());
}

#[test]
fn publish_returns_and_current_reads_the_published_snapshot() {
    let mut projection = Projection::new();
    let published = snapshot(7, 70);

    assert_eq!(projection.publish(published.clone()), published);
    assert_eq!(projection.current(&key(7)), Some(&published));
    assert_eq!(projection.current_value("thread-7"), Some(&70));
}

#[test]
fn replacement_keeps_one_current_value_for_the_thread() {
    let mut projection = Projection::new();
    let _ = projection.publish(snapshot(4, 40));
    let replacement = snapshot(4, 400);

    assert_eq!(projection.publish(replacement.clone()), replacement);
    assert_eq!(projection.len(), 1);
    assert_eq!(projection.current(&key(4)), Some(&replacement));
    assert_eq!(projection.current_value("thread-4"), Some(&400));
}

#[test]
fn exact_limit_retains_all_sixty_four_snapshots() {
    let mut projection = Projection::new();

    for id in 0..MAX_RETAINED_SESSIONS {
        let _ = projection.publish(snapshot(id, id));
    }

    assert_eq!(projection.len(), MAX_RETAINED_SESSIONS);
    assert_eq!(
        retained_ids(&projection),
        (0..MAX_RETAINED_SESSIONS).collect::<Vec<_>>()
    );
    for id in 0..MAX_RETAINED_SESSIONS {
        assert_eq!(projection.current_value(&key(id)), Some(&id));
    }
}

#[test]
fn sixty_fifth_publish_evicts_the_oldest_insertion() {
    let mut projection = Projection::new();

    for id in 0..=MAX_RETAINED_SESSIONS {
        let _ = projection.publish(snapshot(id, id));
    }

    assert_eq!(projection.len(), MAX_RETAINED_SESSIONS);
    assert!(projection.current(&key(0)).is_none());
    assert_eq!(
        retained_ids(&projection),
        (1..=MAX_RETAINED_SESSIONS).collect::<Vec<_>>()
    );
}

#[test]
fn replacement_refreshes_recency_before_eviction() {
    let mut projection = Projection::new();

    for id in 0..MAX_RETAINED_SESSIONS {
        let _ = projection.publish(snapshot(id, id));
    }
    let _ = projection.publish(snapshot(0, 10_000));
    let _ = projection.publish(snapshot(MAX_RETAINED_SESSIONS, MAX_RETAINED_SESSIONS));

    assert!(projection.current(&key(1)).is_none());
    assert_eq!(projection.current_value(&key(0)), Some(&10_000));
    assert_eq!(
        retained_ids(&projection),
        (2..MAX_RETAINED_SESSIONS)
            .chain(std::iter::once(0))
            .chain(std::iter::once(MAX_RETAINED_SESSIONS))
            .collect::<Vec<_>>()
    );
}

#[test]
fn repeated_new_publishes_evict_each_oldest_snapshot_in_order() {
    let mut projection = Projection::new();

    for id in 0..MAX_RETAINED_SESSIONS {
        let _ = projection.publish(snapshot(id, id));
    }
    for id in MAX_RETAINED_SESSIONS..=MAX_RETAINED_SESSIONS + 2 {
        let _ = projection.publish(snapshot(id, id));
    }

    assert_eq!(projection.len(), MAX_RETAINED_SESSIONS);
    assert!((0..3).all(|id| projection.current(&key(id)).is_none()));
    assert_eq!(
        retained_ids(&projection),
        (3..=MAX_RETAINED_SESSIONS + 2).collect::<Vec<_>>()
    );
}

#[test]
fn key_and_value_remain_coupled_during_replacement() {
    let mut projection = Projection::new();
    let _ = projection.publish(snapshot(1, 101));
    let _ = projection.publish(snapshot(2, 202));
    let _ = projection.publish(snapshot(1, 1_001));

    let first = projection
        .current(&key(1))
        .expect("replacement is retained");
    let second = projection
        .current(&key(2))
        .expect("other thread is retained");
    assert_eq!(first.thread_id(), "thread-1");
    assert_eq!(first.value(), &1_001);
    assert_eq!(second.thread_id(), "thread-2");
    assert_eq!(second.value(), &202);

    let (first_key, first_value) = first.clone().into_parts();
    assert_eq!(first_key, key(1));
    assert_eq!(first_value, 1_001);
}

#[test]
fn insertion_order_is_deterministic_and_replacement_moves_to_the_back() {
    let mut projection = Projection::new();
    for id in [7, 2, 9, 1] {
        let _ = projection.publish(snapshot(id, id * 10));
    }
    let _ = projection.publish(snapshot(2, 200));

    assert_eq!(retained_ids(&projection), vec![7, 9, 1, 2]);
    assert_eq!(
        projection
            .snapshots()
            .iter()
            .map(|snapshot| *snapshot.value())
            .collect::<Vec<_>>(),
        vec![70, 90, 10, 200]
    );
}
