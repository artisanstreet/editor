use editor_aqua::ui::scroll_edge::{EdgeFade, target};
use std::time::{Duration, Instant};

#[test]
fn an_edge_shows_in_proportion_to_the_content_hidden_past_it() {
    assert_eq!(target(0.0), 0.0);
    assert_eq!(target(8.0), 0.5);
    assert_eq!(target(16.0), 1.0);
    assert_eq!(target(10_000.0), 1.0);
    assert_eq!(target(-5.0), 0.0);
}

#[test]
fn a_fade_eases_toward_its_target_and_settles_exactly() {
    let start = Instant::now();
    let (mut fade, mut moving) = EdgeFade::default().step(1.0, false, start);
    assert!(moving);
    assert_eq!(fade.shown(), 0.0);
    let mut now = start;
    let mut previous = 0.0;
    for _ in 0..200 {
        now += Duration::from_millis(16);
        (fade, moving) = fade.step(1.0, false, now);
        assert!(fade.shown() >= previous);
        previous = fade.shown();
        if !moving {
            break;
        }
    }
    assert!(!moving);
    assert_eq!(fade.shown(), 1.0);
}

#[test]
fn a_fade_can_reverse_midway_without_jumping() {
    let start = Instant::now();
    let (fade, _) = EdgeFade::default().step(1.0, false, start);
    let (fade, _) = fade.step(1.0, false, start + Duration::from_millis(100));
    let before = fade.shown();
    let (fade, moving) = fade.step(0.0, false, start + Duration::from_millis(116));
    assert!(moving);
    assert!(fade.shown() < before + 0.5);
    assert!(fade.shown() > 0.0);
}

#[test]
fn reduced_motion_jumps_straight_to_the_target() {
    let (fade, moving) = EdgeFade::default().step(0.75, true, Instant::now());
    assert!(!moving);
    assert_eq!(fade.shown(), 0.75);
}
