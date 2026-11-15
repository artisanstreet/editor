use super::*;

fn at(fraction: f64) -> StickerFrame {
    StickerFrame::at(DURATION.mul_f64(fraction))
}

#[test]
fn the_sticker_starts_lifted_up_and_to_the_right() {
    let start = at(0.0);
    assert_eq!(start.opacity, 0.0);
    assert_eq!(start.shift, Point::new(OFFSET_X, -OFFSET_Y));
    assert!(start.scale > 1.0);
    assert_eq!(start.glue, 0.0);
    assert_eq!(start.curl, 1.0);
}

#[test]
fn the_top_right_lands_before_the_bottom_left_corner_flattens() {
    let landed = at(APPROACH);
    assert!(landed.shift.x.abs() < 0.01 && landed.shift.y.abs() < 0.01);
    assert!(landed.glue < 1.0, "the press is still sweeping");
    assert_eq!(landed.curl, 1.0, "the bottom-left corner is still lifted");
    let swept = at(GLUE_START + GLUE);
    assert_eq!(swept.glue, 1.0);
    assert!(swept.curl > 0.0, "the corner is the last part to flatten");
}

#[test]
fn the_press_sweeps_forward_only() {
    let glue: Vec<f32> = (0..=20).map(|step| at(f64::from(step) / 20.0).glue).collect();
    assert!(glue.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn the_sticker_ends_flat_and_in_place() {
    let end = at(1.0);
    assert_eq!(end.opacity, 1.0);
    assert_eq!(end.shift, Point::new(0.0, 0.0));
    assert!((end.scale - 1.0).abs() < 1e-6);
    assert_eq!(end.glue, 1.0);
    assert_eq!(end.curl, 0.0);
}
