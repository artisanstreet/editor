use editor_aqua::frame::scramble::{Rows, cells_between, frame_at, rows, squeeze, total_ms};

fn spelled(rows: &Rows) -> Vec<String> {
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|cell| cell.map_or_else(String::new, String::from))
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect()
}

#[test]
fn the_toggle_scrambles_through_other_letters_and_lands_on_the_target() {
    let cells = cells_between(&squeeze(&rows(&["AR", "TIS", "AN"])), &["TO", "GG", "LE"]);
    let total = total_ms(&cells);
    let target = ["T|O", "G|G", "L|E"];
    assert_ne!(
        spelled(&frame_at(&cells, total / 2.0)),
        target,
        "midway is still scrambling"
    );
    assert_eq!(spelled(&frame_at(&cells, total)), target);
    assert_eq!(
        spelled(&frame_at(&cells, total * 2.0)),
        target,
        "the label stays put"
    );
}

#[test]
fn rows_wider_than_the_target_scramble_their_extra_cells_away() {
    let cells = cells_between(&rows(&["AR", "TIS", "AN"]), &["TO", "GG", "LE"]);
    let settled = frame_at(&cells, total_ms(&cells));
    assert_eq!(spelled(&settled), ["T|O", "G|G|", "L|E"]);
}
