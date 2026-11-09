use editor_aqua::frame::sign_in::pattern::{CELL, cells};

#[test]
fn cells_stay_inside_the_box_and_below_the_ridge() {
    let (width, height) = (400.0, 300.0);
    let found = cells(width, height);
    assert!(!found.is_empty());
    assert!(found.iter().all(|cell| {
        cell.x + CELL.0 / 2.0 < width
            && cell.y + CELL.1 / 2.0 < height
            && cell.strength > 0.0
            && cell.strength <= 1.0
    }));
    assert!(found
        .iter()
        .filter(|cell| (cell.x + CELL.0 / 2.0) / width >= 0.9)
        .all(|cell| (cell.y + CELL.1 / 2.0) / height >= 0.52));
}

#[test]
fn the_pattern_thins_out_towards_the_right() {
    let found = cells(840.0, 480.0);
    let left = found.iter().filter(|cell| cell.x < 280.0).map(|cell| cell.strength).sum::<f32>();
    let right = found.iter().filter(|cell| cell.x >= 560.0).map(|cell| cell.strength).sum::<f32>();
    assert!(left > right * 2.0);
}

#[test]
fn an_empty_box_has_no_cells() {
    assert!(cells(0.0, 100.0).is_empty());
}
