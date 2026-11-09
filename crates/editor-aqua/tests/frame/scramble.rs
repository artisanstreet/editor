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
fn toggle_scramble_matches_the_dashboard_frame_for_frame() {
    let cells = cells_between(&squeeze(&rows(&["AR", "TIS", "AN"])), &["TO", "GG", "LE"]);
    assert_eq!(total_ms(&cells), 260.0);
    for (elapsed, expected) in [
        (0.0, ["N|R", "T|S", "A|N"]),
        (20.0, ["N|O", "T|S", "A|N"]),
        (60.0, ["O|P", "U|Z", "A|N"]),
        (120.0, ["R|S", "A|A", "C|L"]),
        (200.0, ["T|O", "D|E", "E|P"]),
        (260.0, ["T|O", "G|G", "L|E"]),
    ] {
        assert_eq!(spelled(&frame_at(&cells, elapsed)), expected, "at {elapsed} ms");
    }
}

#[test]
fn rows_wider_than_the_target_scramble_their_extra_cells_away() {
    let cells = cells_between(&rows(&["AR", "TIS", "AN"]), &["TO", "GG", "LE"]);
    let settled = frame_at(&cells, total_ms(&cells));
    assert_eq!(spelled(&settled), ["T|O", "G|G|", "L|E"]);
}
