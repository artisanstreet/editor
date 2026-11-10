use aqua_core::Rect;
use editor_aqua::frame::seam::motion::Motion;
use std::time::{Duration, Instant};

fn body() -> Rect {
    Rect::from_edges(0.0, 40.0, 900.0, 700.0)
}

fn tab(left: f32, width: f32) -> Rect {
    Rect::from_edges(left, 4.0, left + width, 40.0)
}

#[test]
fn switching_tabs_glides_both_edges_and_arrives_at_the_measured_shape() {
    let start = Instant::now();
    let first = tab(100.0, 120.0);
    let next = tab(400.0, 200.0);
    let mut motion = Motion::default();
    motion.target(body(), Some((1, first)), start, false);
    assert_eq!(motion.sample(start), Some(first));
    assert!(!motion.animating());
    motion.target(body(), Some((2, next)), start, false);
    assert_eq!(motion.sample(start), Some(first));
    let middle = motion.sample(start + Duration::from_millis(80)).unwrap();
    assert!(middle.x > first.x && middle.x < next.x);
    assert!(middle.right() > first.right() && middle.right() < next.right());
    assert_eq!(middle.y, first.y);
    let end = start + Duration::from_millis(250);
    assert_eq!(motion.sample(end), Some(next));
    assert!(motion.advance(end, false));
    assert!(!motion.animating());
    assert!(!motion.advance(end, false));
}

#[test]
fn entry_rises_at_the_selected_tab_and_exit_sinks_at_the_same_location() {
    let start = Instant::now();
    let selected = tab(500.0, 180.0);
    let mut motion = Motion::default();
    motion.target(body(), None, start, false);
    motion.target(body(), Some((1, selected)), start, false);
    assert_eq!(motion.sample(start), None);
    let rising = motion.sample(start + Duration::from_millis(60)).unwrap();
    assert_eq!(rising.x, selected.x);
    assert_eq!(rising.w, selected.w);
    assert!(rising.y > selected.y && rising.y < body().y);
    let opened = start + Duration::from_millis(250);
    motion.advance(opened, false);
    motion.target(body(), None, opened, false);
    assert_eq!(motion.sample(opened), Some(selected));
    let sinking = motion.sample(opened + Duration::from_millis(50)).unwrap();
    assert_eq!(sinking.x, selected.x);
    assert_eq!(sinking.w, selected.w);
    assert!(sinking.y > selected.y && sinking.y < body().y);
    let closed = opened + Duration::from_millis(150);
    motion.advance(closed, false);
    assert_eq!(motion.sample(closed), None);
    assert!(!motion.animating());
    let reopened = closed + Duration::from_secs(1);
    let next = tab(100.0, 120.0);
    motion.target(body(), Some((2, next)), reopened, false);
    let rising = motion.sample(reopened + Duration::from_millis(60)).unwrap();
    assert_eq!(rising.x, next.x);
    assert_eq!(rising.w, next.w);
}

#[test]
fn rapid_selection_changes_continue_from_the_current_contour() {
    let start = Instant::now();
    let mut motion = Motion::default();
    motion.target(body(), Some((1, tab(100.0, 120.0))), start, false);
    motion.target(body(), Some((2, tab(600.0, 160.0))), start, false);
    let interrupted = start + Duration::from_millis(70);
    let current = motion.sample(interrupted);
    let last = tab(300.0, 200.0);
    motion.target(body(), Some((3, last)), interrupted, false);
    assert_eq!(motion.sample(interrupted), current);
    assert_eq!(
        motion.sample(interrupted + Duration::from_millis(250)),
        Some(last)
    );
}

#[test]
fn reopening_after_an_expired_exit_rises_at_the_new_selection() {
    let start = Instant::now();
    let mut motion = Motion::default();
    motion.target(body(), Some((1, tab(500.0, 180.0))), start, false);
    motion.target(body(), None, start, false);
    let reopened = start + Duration::from_millis(300);
    let next = tab(100.0, 120.0);
    motion.target(body(), Some((2, next)), reopened, false);
    let rising = motion.sample(reopened + Duration::from_millis(30)).unwrap();
    assert_eq!(rising.x, next.x);
    assert_eq!(rising.w, next.w);
}

#[test]
fn resize_and_reduced_motion_snap_without_a_flight_from_the_old_layout() {
    let start = Instant::now();
    let mut motion = Motion::default();
    motion.target(body(), Some((1, tab(100.0, 120.0))), start, false);
    motion.target(body(), Some((2, tab(600.0, 160.0))), start, false);
    let resized = Rect::from_edges(0.0, 40.0, 700.0, 700.0);
    let selected = tab(250.0, 120.0);
    motion.target(resized, Some((2, selected)), start, false);
    assert_eq!(motion.sample(start), Some(selected));
    assert!(!motion.animating());
    let next = tab(400.0, 150.0);
    motion.target(resized, Some((3, next)), start, false);
    motion.advance(start, true);
    assert_eq!(motion.sample(start), Some(next));
    assert!(!motion.animating());
    motion.target(resized, None, start, true);
    assert_eq!(motion.sample(start), None);
    assert!(!motion.animating());
}
