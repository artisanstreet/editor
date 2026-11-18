use editor_aqua::frame::inbox::history_height;

#[test]
fn open_history_fills_the_space_the_inbox_leaves_without_overflowing() {
    let open = history_height(800.0, 250.0, 900.0, 1.0);
    assert!(open > 0.0, "history gets the free space");
    assert!(open + 250.0 <= 800.0, "inbox and history fit the viewport");
    assert_eq!(
        history_height(800.0, 250.0, 200.0, 1.0),
        200.0,
        "a short history is only as tall as its rows"
    );
}

#[test]
fn a_crowded_inbox_still_leaves_history_visible_room() {
    let crowded = history_height(800.0, 1000.0, 900.0, 1.0);
    assert!(crowded > 0.0 && crowded < 800.0, "got {crowded}");
    assert_eq!(
        history_height(30.0, 1000.0, 900.0, 1.0),
        0.0,
        "no room, no history"
    );
}

#[test]
fn the_open_fraction_scales_the_height_and_clamps() {
    let open = history_height(800.0, 250.0, 900.0, 1.0);
    assert_eq!(history_height(800.0, 250.0, 900.0, 0.5), open / 2.0);
    assert_eq!(history_height(800.0, 250.0, 900.0, 0.0), 0.0);
    assert_eq!(history_height(800.0, 250.0, 900.0, 2.0), open);
    assert_eq!(history_height(800.0, 250.0, 900.0, -1.0), 0.0);
}
