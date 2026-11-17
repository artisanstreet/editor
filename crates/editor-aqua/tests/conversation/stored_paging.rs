use editor_aqua::conversation::stored::retired_leaves;

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
