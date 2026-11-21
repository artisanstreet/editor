use super::*;

fn goal(objective: &str) -> ComposerGoal {
    ComposerGoal {
        objective: objective.to_owned(),
        state: ComposerGoalState::Active,
        detail: None,
    }
}

fn painted(strip: &mut GoalStrip, now: Instant) -> (f32, f32) {
    let frame = strip.frame(now, false).unwrap();
    (frame.reveal, frame.label_opacity)
}

#[test]
fn a_strip_that_returns_while_leaving_grows_back_from_its_painted_height() {
    let start = Instant::now();
    let mut strip = GoalStrip::default();
    strip.set(Some(goal("Ship it")), start, false);
    let shown = start + Duration::from_secs(1);
    assert_eq!(painted(&mut strip, shown), (1.0, 1.0));
    strip.set(None, shown, false);
    let midway = shown + Duration::from_millis(300);
    let (reveal, label) = painted(&mut strip, midway);
    assert!(reveal > 0.0 && reveal < 1.0);
    strip.set(Some(goal("Ship it")), midway, false);
    assert_eq!(painted(&mut strip, midway), (reveal, label));
    let later = painted(&mut strip, midway + Duration::from_millis(100));
    assert!(later.0 > reveal);
    assert_eq!(
        painted(&mut strip, midway + Duration::from_millis(500)),
        (1.0, 1.0)
    );
}

#[test]
fn a_strip_that_leaves_while_growing_shrinks_from_its_painted_height() {
    let start = Instant::now();
    let mut strip = GoalStrip::default();
    strip.set(Some(goal("Ship it")), start, false);
    let early = start + Duration::from_millis(120);
    let (reveal, label) = painted(&mut strip, early);
    assert!(reveal > 0.0 && reveal < 1.0);
    strip.set(None, early, false);
    assert_eq!(painted(&mut strip, early), (reveal, label));
    let shrinking = painted(&mut strip, early + Duration::from_millis(300));
    assert!(shrinking.0 < reveal);
    assert!(
        strip
            .frame(early + Duration::from_millis(500), false)
            .is_none()
    );
}

#[test]
fn reduced_motion_shows_and_hides_the_strip_at_once() {
    let start = Instant::now();
    let mut strip = GoalStrip::default();
    strip.set(Some(goal("Ship it")), start, true);
    let frame = strip.frame(start, true).unwrap();
    assert_eq!(
        (frame.reveal, frame.label_opacity, frame.moving),
        (1.0, 1.0, false)
    );
    strip.set(None, start, true);
    assert!(strip.frame(start, true).is_none());
}
