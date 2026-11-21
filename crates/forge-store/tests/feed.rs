use artisan_api::{Event, ItemId, LiveCoverage, LiveTarget, ThreadId};
use tokio::sync::broadcast::error::RecvError;

const MIB: usize = 1024 * 1024;

fn live(text: &str) -> Event {
    Event::Live {
        thread_id: ThreadId("thread".into()),
        target: LiveTarget::Text {
            item_id: ItemId("item".into()),
        },
        text: text.into(),
        append: true,
        coverage: LiveCoverage {
            start: 0,
            total: text.len() as u64,
        },
    }
}

fn store() -> (tempfile::TempDir, forge_store::Store) {
    let directory = tempfile::tempdir().expect("a temporary folder");
    let store = forge_store::Store::open(directory.path()).expect("a fresh store opens");
    (directory, store)
}

#[tokio::test]
async fn a_stalled_subscriber_holds_at_most_the_change_budget() {
    let (_directory, store) = store();
    let mut stalled = store.subscribe();
    let chunk = "x".repeat(512 * 1024);
    for _ in 0..96 {
        store.relay_live(live(&chunk));
    }
    assert!(
        store.retained_change_bytes() <= 32 * MIB,
        "{} bytes retained for a stalled subscriber",
        store.retained_change_bytes()
    );
    assert!(matches!(stalled.recv().await, Err(RecvError::Lagged(_))));
    store.relay_live(live("after"));
    let mut caught_up = None;
    while let Ok(envelope) = stalled.try_recv() {
        caught_up = Some(envelope);
    }
    assert!(matches!(
        caught_up.map(|envelope| envelope.event),
        Some(Event::Live { text, .. }) if text == "after"
    ));
}

#[tokio::test]
async fn changes_every_subscriber_has_read_are_released() {
    let (_directory, store) = store();
    let mut first = store.subscribe();
    let mut second = store.subscribe();
    let chunk = "y".repeat(MIB);
    for _ in 0..4 {
        store.relay_live(live(&chunk));
    }
    for _ in 0..4 {
        first.recv().await.unwrap();
    }
    assert!(store.retained_change_bytes() >= 4 * MIB);
    for _ in 0..4 {
        second.recv().await.unwrap();
    }
    assert_eq!(store.retained_changes(), 0);
    let mut late = store.subscribe();
    store.relay_live(live(&chunk));
    drop(first);
    drop(second);
    late.recv().await.unwrap();
    assert_eq!(store.retained_change_bytes(), 0);
}

#[tokio::test]
async fn nobody_listening_retains_nothing() {
    let (_directory, store) = store();
    store.relay_live(live(&"z".repeat(MIB)));
    assert_eq!(store.retained_changes(), 0);
}
