use crate::events::Cursor;
use crate::tests::{forge, paired, read_events};
use artisan_api::{Envelope, Event, ItemId, LiveCoverage, LiveTarget, ThreadId};

fn live(text: &str, append: bool, total: u64, revision: u64) -> Envelope {
    Envelope {
        seq: None,
        live_revision: revision,
        event: Event::Live {
            thread_id: ThreadId::from("thread"),
            target: LiveTarget::Text {
                item_id: ItemId::from("reply"),
            },
            text: text.into(),
            append,
            coverage: LiveCoverage { start: 0, total },
        },
    }
}

fn render(snapshot: &[Event], queued: &[Envelope], cursor: &mut Cursor) -> String {
    let mut rendered = String::new();
    let snapshot = snapshot.iter().cloned().map(|event| Envelope {
        seq: None,
        live_revision: u64::MAX,
        event,
    });
    let delivered = queued
        .iter()
        .filter(|envelope| cursor.accept(envelope))
        .cloned();
    for envelope in snapshot.chain(delivered.collect::<Vec<_>>()) {
        if let Event::Live { text, append, .. } = envelope.event {
            if !append {
                rendered.clear();
            }
            rendered.push_str(&text);
        }
    }
    rendered
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_ahead_of_the_forge_database_is_told_to_reset() {
    let (_dir, address, store) = forge().await;
    let editor = paired(&store, address, "laptop").await;
    let ahead = store.head() + 1_000;
    let response = editor
        .http()
        .get(editor.url(&format!("/events?since={ahead}")))
        .send()
        .await
        .unwrap();
    let events = read_events(
        response,
        "the reset for a cursor from a newer database",
        |envelope| envelope.event == Event::Reset,
    )
    .await;
    assert!(matches!(events[0].event, Event::Hello { .. }));
    assert_eq!(events.len(), 2, "nothing is sent between Hello and Reset");
}

#[test]
fn a_reset_published_before_the_live_snapshot_still_reaches_the_client() {
    let mut cursor = Cursor::after_snapshot(10, 5, &[]);
    let reset = Envelope {
        seq: None,
        live_revision: 3,
        event: Event::Reset,
    };
    assert!(
        cursor.accept(&reset),
        "a reset stands for durable changes the snapshot never carries"
    );
}

#[test]
fn a_relayed_delta_already_in_a_peer_snapshot_is_not_appended_twice() {
    let snapshot = [live("hello world", false, 11, 0).event];
    let mut cursor = Cursor::after_snapshot(0, 5, &snapshot);
    let queued = [
        live("hello", false, 5, 6),
        live(" world", true, 11, 7),
        live("!", true, 12, 8),
    ];
    assert_eq!(render(&snapshot, &queued, &mut cursor), "hello world!");
}

#[test]
fn a_relayed_append_covered_by_the_snapshot_is_dropped() {
    let snapshot = [live("hello world", false, 11, 0).event];
    let mut cursor = Cursor::after_snapshot(0, 5, &snapshot);
    let queued = [live(" world", true, 11, 6), live("!", true, 12, 7)];
    assert_eq!(render(&snapshot, &queued, &mut cursor), "hello world!");
}
