use std::time::{Duration, Instant};

use editor_aqua::ui::code_copy::CodeCopyController;

#[test]
fn repeated_copy_restarts_the_hold_and_both_swaps() {
    let controller = CodeCopyController::default();
    let start = Instant::now();
    controller.copied(start);
    let enter = controller.frame(start + Duration::from_millis(125), false);
    assert!(enter.animating && enter.progress > 0.0 && enter.progress < 1.0);
    let held = controller.frame(start + Duration::from_millis(500), false);
    assert!(!held.animating);
    assert_eq!(held.progress, 1.0);
    assert_eq!(held.wake_at, Some(start + Duration::from_millis(1_500)));
    let repeated = start + Duration::from_millis(1_000);
    controller.copied(repeated);
    let old_deadline = controller.frame(start + Duration::from_millis(1_500), false);
    assert_eq!(old_deadline.progress, 1.0);
    assert_eq!(
        old_deadline.wake_at,
        Some(repeated + Duration::from_millis(1_500))
    );
    let exit = controller.frame(repeated + Duration::from_millis(1_625), false);
    assert!(exit.animating && exit.progress > 0.0 && exit.progress < 1.0);
    let settled = controller.frame(repeated + Duration::from_millis(1_750), false);
    assert_eq!(settled.progress, 0.0);
    assert!(!settled.animating && settled.wake_at.is_none());
}

#[test]
fn reduced_motion_steps_and_schedules_only_hold_deadline() {
    let controller = CodeCopyController::default();
    let start = Instant::now();
    controller.copied(start);
    let held = controller.frame(start, true);
    assert_eq!(held.progress, 1.0);
    assert!(!held.animating);
    assert_eq!(held.wake_at, Some(start + Duration::from_millis(1_500)));
    let settled = controller.frame(start + Duration::from_millis(1_500), true);
    assert_eq!(settled.progress, 0.0);
    assert!(!settled.animating && settled.wake_at.is_none());
}
