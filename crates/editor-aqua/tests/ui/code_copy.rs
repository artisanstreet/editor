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

#[test]
fn copy_shows_the_checkmark_only_after_the_clipboard_accepts_the_code() {
    use editor_aqua::ui::code_copy::{CodeCopyButton, CopyEffect};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::{cell::RefCell, rc::Rc};
    let controller = CodeCopyController::default();
    let effects = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&effects);
    let button = CodeCopyButton::new(
        "copy",
        "cargo test",
        ArtisanTheme::resolved(),
        controller.clone(),
        false,
        move |effect| sink.borrow_mut().push(effect),
    )
    .build(Instant::now());
    let mut app = test_support::editor::ui::mounted(200.0, 100.0, button);
    test_support::editor::ui::click(&mut app, "copy");
    let requests: Vec<_> = effects
        .borrow()
        .iter()
        .filter_map(|effect| match effect {
            CopyEffect::Clipboard(request) => Some(request.clone()),
            CopyEffect::RequestFrame | CopyEffect::WakeAt(_) => None,
        })
        .collect();
    assert_eq!(
        requests
            .iter()
            .map(editor_aqua::ui::code_copy::CopyRequest::text)
            .collect::<Vec<_>>(),
        ["cargo test"]
    );
    let later = Instant::now() + Duration::from_millis(500);
    assert_eq!(
        controller.frame(later, false).progress,
        0.0,
        "no checkmark while the clipboard has not accepted the code"
    );
    let copied_at = Instant::now();
    let deadline = requests[0].copied(copied_at);
    assert_eq!(deadline, copied_at + Duration::from_millis(1_500));
    assert_eq!(
        controller
            .frame(copied_at + Duration::from_millis(500), false)
            .progress,
        1.0
    );
}
