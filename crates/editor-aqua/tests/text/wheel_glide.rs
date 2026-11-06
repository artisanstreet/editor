use editor_aqua::ui::text::wheel_glide::{WheelGlide, WheelInput};
use std::time::Duration;

const FRAME: Duration = Duration::from_millis(16);

fn settle(glide: &mut WheelGlide, mut offset: f32, max: f32) -> (f32, usize) {
    let mut frames = 0;
    while let Some(next) = glide.step_down_elapsed(offset, max, FRAME) {
        offset = next;
        frames += 1;
        assert!(frames < 400, "a glide settles");
    }
    (offset, frames)
}

#[test]
fn whole_lines_are_notches_and_fractions_are_precise() {
    assert!(matches!(
        WheelInput::classify(16.0),
        Some(WheelInput::Notch(distance)) if (distance - 77.665_64).abs() < 0.01
    ));
    assert!(matches!(
        WheelInput::classify(-32.0),
        Some(WheelInput::Notch(distance)) if distance < -150.0
    ));
    assert_eq!(WheelInput::classify(7.5), Some(WheelInput::Precise(7.5)));
    assert_eq!(WheelInput::classify(24.0), Some(WheelInput::Precise(24.0)));
    assert_eq!(WheelInput::classify(0.0), None);
    assert_eq!(WheelInput::classify(f32::NAN), None);
}

#[test]
fn a_notch_glides_down_over_several_frames_to_its_distance() {
    let mut glide = WheelGlide::default();
    glide.push_down(100.0, 60.0, 500.0);
    assert!(glide.active());
    let (offset, frames) = settle(&mut glide, 100.0, 500.0);
    assert!((offset - 160.0).abs() < 0.001);
    assert!(frames > 5);
    assert!(!glide.active());
}

#[test]
fn ticks_accumulate_in_one_direction() {
    let mut glide = WheelGlide::default();
    glide.push_down(0.0, 50.0, 500.0);
    glide.push_down(0.0, 50.0, 500.0);
    let (offset, _) = settle(&mut glide, 0.0, 500.0);
    assert!((offset - 100.0).abs() < 0.001);
}

#[test]
fn reversing_the_wheel_drops_the_queued_distance() {
    let mut glide = WheelGlide::default();
    glide.push_down(250.0, 100.0, 500.0);
    glide.push_down(250.0, -30.0, 500.0);
    let (offset, _) = settle(&mut glide, 250.0, 500.0);
    assert!((offset - 220.0).abs() < 0.001);
}

#[test]
fn the_glide_stops_at_both_edges() {
    let mut glide = WheelGlide::default();
    glide.push_down(450.0, 400.0, 500.0);
    assert!((settle(&mut glide, 450.0, 500.0).0 - 500.0).abs() < 0.001);
    glide.push_down(30.0, -400.0, 500.0);
    assert!(settle(&mut glide, 30.0, 500.0).0.abs() < 0.001);
}

#[test]
fn a_glide_in_an_unscrollable_list_never_starts() {
    let mut glide = WheelGlide::default();
    glide.push_down(0.0, 80.0, 0.0);
    assert!(!glide.active());
    assert_eq!(glide.step_down_elapsed(0.0, 0.0, FRAME), None);
}

#[test]
fn cancelling_drops_the_backlog() {
    let mut glide = WheelGlide::default();
    glide.push_down(0.0, 80.0, 500.0);
    glide.cancel();
    assert!(!glide.active());
    assert_eq!(glide.step_down_elapsed(0.0, 500.0, FRAME), None);
}

#[test]
fn content_shrinking_under_a_glide_ends_it_at_the_new_edge() {
    let mut glide = WheelGlide::default();
    glide.push_down(100.0, 200.0, 500.0);
    let next = glide.step_down_elapsed(100.0, 120.0, FRAME).unwrap();
    assert!(next <= 120.0);
    let (offset, _) = settle(&mut glide, next, 120.0);
    assert!((offset - 120.0).abs() < 0.001);
}

#[test]
fn the_same_time_travels_the_same_distance_at_any_refresh_rate() {
    let travel = |frame_ms, frames| {
        let mut glide = WheelGlide::default();
        glide.push_down(0.0, 100.0, 500.0);
        let mut offset = 0.0;
        for _ in 0..frames {
            offset = glide
                .step_down_elapsed(offset, 500.0, Duration::from_millis(frame_ms))
                .unwrap();
        }
        offset
    };
    assert!((travel(8, 12) - travel(16, 6)).abs() < 0.001);
}
