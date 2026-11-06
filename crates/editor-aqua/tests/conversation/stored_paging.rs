use artisan_api::ItemId;
use editor_aqua::conversation::fold::RowKey;
use editor_aqua::conversation::stored::{edge_offset, retired_leaves, reverses_in_place};

#[test]
fn stored_edges_overlap_only_when_the_reader_reaches_the_loaded_window() {
    assert_eq!(edge_offset(64..128, 200, 64, true, false, false), Some(32));
    assert_eq!(edge_offset(64..128, 200, 64, false, false, false), None);
    assert_eq!(edge_offset(64..128, 200, 64, true, true, false), None);
    assert_eq!(edge_offset(64..128, 200, 64, true, false, true), None);
    assert_eq!(edge_offset(64..128, 200, 68, true, false, false), None);
    assert_eq!(edge_offset(64..128, 200, 124, false, true, true), Some(96));
    assert_eq!(edge_offset(64..128, 128, 127, false, false, false), None);
    assert_eq!(edge_offset(64..128, 200, 128, true, false, false), None);
}

#[test]
fn opposite_prefetch_waits_for_real_movement_and_matching_direction() {
    let first = RowKey::Item(ItemId("first".into()));
    let second = RowKey::Item(ItemId("second".into()));
    assert!(reverses_in_place(
        4,
        Some(&first),
        true,
        4,
        Some(&second),
        Some(false),
        false
    ));
    assert!(reverses_in_place(
        4,
        Some(&first),
        true,
        5,
        Some(&first),
        Some(false),
        false
    ));
    assert!(reverses_in_place(
        4,
        Some(&first),
        true,
        5,
        Some(&second),
        Some(true),
        false
    ));
    assert!(!reverses_in_place(
        4,
        Some(&first),
        true,
        5,
        Some(&second),
        Some(false),
        false
    ));
    assert!(!reverses_in_place(
        4,
        Some(&first),
        true,
        4,
        Some(&first),
        None,
        true
    ));
}

#[cfg(feature = "qualification")]
#[test]
fn paint_serial_advances_at_callback_and_survives_observation_clear() {
    use aqua_text::document::SelectionDocumentId;
    use editor_aqua::conversation::fold::Row;
    use editor_aqua::conversation::presentation::MarkdownOwners;
    use editor_aqua::conversation::view::transcript::{RowSnapshot, TranscriptObservations};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    let observations = TranscriptObservations::default();
    let snapshot = RowSnapshot {
        row: Row::Gap,
        version: 1,
        item_seq: None,
        source: None,
        active: Rc::new(Cell::new(true)),
        revealed: true,
        reveal_ready: None,
        live: None,
        stored: None,
    };
    let (_, paint) = observations.callbacks(
        snapshot,
        Rc::new(RefCell::new(MarkdownOwners::new(SelectionDocumentId(1)))),
    );
    let bounds = aqua_core::Rect {
        x: 0.0,
        y: 0.0,
        w: 10.0,
        h: 10.0,
    };
    let baseline = observations.qualification_paint_serial();
    paint(bounds);
    assert_eq!(observations.drain().painted[0].serial, baseline + 1);
    observations.clear();
    assert_eq!(observations.qualification_paint_serial(), baseline + 1);
    paint(bounds);
    assert_eq!(observations.drain().painted[0].serial, baseline + 2);
}

#[test]
fn leaves_outside_the_moved_window_retire_unless_mounted() {
    let mounted = |ordinal: usize| ordinal == 3;
    assert_eq!(
        retired_leaves(&(10..20), [3, 9, 10, 19, 20, 3, 30, 9], mounted),
        vec![9, 20, 30]
    );
    assert!(retired_leaves(&(0..64), [0, 31, 63], |_| false).is_empty());
    assert!(retired_leaves(&(10..20), std::iter::empty(), |_| false).is_empty());
}
