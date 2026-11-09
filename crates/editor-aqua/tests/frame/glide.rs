use editor_aqua::frame::glide::{CubicBezier, Glide};
use editor_aqua::frame::sidebar::{COLLAPSED_WIDTH, OPEN_WIDTH, RESIZE};
use std::time::{Duration, Instant};

#[test]
fn css_ease_curve_matches_known_points() {
    let ease = CubicBezier::new(0.25, 0.1, 0.25, 1.0);
    assert!((ease.sample(0.5) - 0.802_403).abs() < 1e-4);
    assert_eq!(ease.sample(0.0), 0.0);
    assert_eq!(ease.sample(1.0), 1.0);
}

#[test]
fn collapsing_runs_from_open_to_collapsed_in_three_hundred_ms() {
    let start = Instant::now();
    let glide = Glide::Resting(OPEN_WIDTH).retarget(COLLAPSED_WIDTH, start, RESIZE, false);
    assert_eq!(glide.value(start, RESIZE), OPEN_WIDTH);
    let middle = glide.value(start + Duration::from_millis(150), RESIZE);
    assert!(middle < OPEN_WIDTH && middle > COLLAPSED_WIDTH);
    assert!(middle < f32::midpoint(OPEN_WIDTH, COLLAPSED_WIDTH), "ease-out front-loads the motion");
    let end = start + Duration::from_millis(300);
    assert_eq!(glide.value(end, RESIZE), COLLAPSED_WIDTH);
    assert_eq!(glide.settle(end), Glide::Resting(COLLAPSED_WIDTH));
}

#[test]
fn reversing_mid_flight_returns_from_where_it_is_in_less_time() {
    let start = Instant::now();
    let closing = Glide::Resting(OPEN_WIDTH).retarget(COLLAPSED_WIDTH, start, RESIZE, false);
    let turn = start + Duration::from_millis(30);
    let at_turn = closing.value(turn, RESIZE);
    let opening = closing.retarget(OPEN_WIDTH, turn, RESIZE, false);
    assert_eq!(opening.value(turn, RESIZE), at_turn);
    assert!(opening.settle(turn + Duration::from_millis(1)).running());
    assert_eq!(
        opening.settle(turn + Duration::from_millis(150)),
        Glide::Resting(OPEN_WIDTH)
    );
}

#[test]
fn reduced_motion_jumps_to_the_target() {
    let start = Instant::now();
    let glide = Glide::Resting(OPEN_WIDTH).retarget(COLLAPSED_WIDTH, start, RESIZE, true);
    assert_eq!(glide, Glide::Resting(COLLAPSED_WIDTH));
}
