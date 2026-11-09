use aqua_core::Rect;
use editor_aqua::conversation::admission::RevealQueue;
use editor_aqua::conversation::fold::RowKey;
use editor_aqua::conversation::read::painted;

#[test]
fn read_evidence_requires_a_revealed_active_row_with_its_bottom_inside_the_clip() {
    let clip = Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    };
    let visible = Rect {
        x: 10.0,
        y: -5.0,
        w: 50.0,
        h: 40.0,
    };
    assert!(painted(visible, clip, true, true));
    assert!(!painted(visible, clip, false, true));
    assert!(!painted(visible, clip, true, false));
    assert!(!painted(
        Rect {
            y: 75.0,
            h: 30.0,
            ..visible
        },
        clip,
        true,
        true
    ));
    assert!(!painted(
        Rect {
            y: -40.0,
            h: 40.0,
            ..visible
        },
        clip,
        true,
        true
    ));
    assert!(!painted(
        Rect {
            x: 120.0,
            ..visible
        },
        clip,
        true,
        true
    ));
}

#[test]
fn reveal_queue_holds_later_rows_until_the_prior_row_settles() {
    let mut queue = RevealQueue::default();
    let first = RowKey::Gap;
    let second = RowKey::Item(artisan_api::ItemId("second".into()));
    assert!(!queue.begin(first.clone(), (1, 1, 0), true));
    queue.finish(&first, true);
    assert!(queue.begin(second.clone(), (2, 1, 0), true));
    assert_eq!(queue.frontier_key(), Some(first.clone()));
    queue.finish(&first, false);
    assert!(!queue.begin(second, (2, 1, 0), true));
    queue.rush(2);
    assert!(queue.rushed(1));
    assert!(!queue.rushed(2));
}

#[test]
fn reader_interruption_releases_loaded_rows_without_rushing_future_replies() {
    let mut queue = RevealQueue::default();
    let first = RowKey::Item(artisan_api::ItemId("first".into()));
    let second = RowKey::Item(artisan_api::ItemId("second".into()));
    assert!(!queue.begin(first.clone(), (1, 1, 0), true));
    queue.finish(&first, true);
    assert!(queue.begin(second.clone(), (2, 1, 0), true));
    queue.finish(&second, true);
    queue.rush(2);
    assert_eq!(queue.frontier_key(), Some(second));
    queue.rush(3);
    assert_eq!(queue.frontier_key(), None);
    assert!(!queue.rushed(3));
    assert!(!queue.begin(
        RowKey::Item(artisan_api::ItemId("next".into())),
        (3, 1, 0),
        true
    ));
}
