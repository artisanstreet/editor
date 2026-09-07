//! Focused, dependency-free coverage for the synchronous composer gesture
//! classifier.
//!
//! The source is included directly so this harness exercises the owned leaf
//! without shared `lib.rs`, BUILD, or Cargo registration.

#[path = "../../modules/frontend/src/composer_gesture.rs"]
mod composer_gesture;

use composer_gesture::{
    ComposerDropPoint, ComposerFileMetadata, ComposerFileTransfer, ComposerGesture,
    ComposerGestureDecision, ComposerGestureState, ComposerSubmitKeyInput, accept_file_drag,
    classify_drop, classify_paste,
};

fn file(mime_type: &str) -> ComposerFileMetadata {
    ComposerFileMetadata::new(mime_type)
}

fn transfer(files: Vec<ComposerFileMetadata>, types: &[&str]) -> ComposerFileTransfer {
    ComposerFileTransfer::new(
        files,
        types
            .iter()
            .map(|transfer_type| (*transfer_type).to_owned())
            .collect(),
    )
}

fn submit() -> ComposerSubmitKeyInput<'static> {
    ComposerSubmitKeyInput::new(false, "Enter", false)
}

fn retained_images(
    decision: &ComposerGestureDecision,
    expected_files: Vec<ComposerFileMetadata>,
    expected_point: Option<ComposerDropPoint>,
) {
    assert!(decision.prevent_default);
    assert_eq!(
        decision.gesture,
        Some(ComposerGesture::Images {
            files: expected_files,
            point: expected_point,
        })
    );
}

#[test]
fn drag_acceptance_requires_the_exact_case_sensitive_files_type() {
    let accepted = transfer(vec![], &["text/plain", "Files"]);
    let accepted_decision = accept_file_drag(Some(&accepted));
    assert!(accepted_decision.prevent_default);
    assert_eq!(accepted_decision.gesture, None);

    for types in [
        vec!["files"],
        vec!["FILES"],
        vec![" Files"],
        vec!["Files "],
        vec!["application/octet-stream"],
    ] {
        let decision = accept_file_drag(Some(&transfer(vec![], &types)));
        assert!(!decision.prevent_default, "types={types:?}");
        assert_eq!(decision.gesture, None, "types={types:?}");
    }
}

#[test]
fn drag_acceptance_ignores_null_and_empty_transfers() {
    let null_decision = accept_file_drag(None);
    assert_eq!(null_decision, ComposerGestureDecision::ignored());

    let empty_decision = accept_file_drag(Some(&ComposerFileTransfer::default()));
    assert_eq!(empty_decision, ComposerGestureDecision::ignored());
}

#[test]
fn drop_filters_images_case_sensitively_and_preserves_file_order_and_point() {
    let incoming = transfer(
        vec![
            file("text/plain"),
            file("image/png"),
            file("Image/jpeg"),
            file("image/svg+xml"),
            file("image"),
            file("image/"),
        ],
        &["Files"],
    );
    let point = ComposerDropPoint::new(-12.5, 98.25);

    retained_images(
        &classify_drop(Some(&incoming), point),
        vec![file("image/png"), file("image/svg+xml"), file("image/")],
        Some(point),
    );
}

#[test]
fn drop_with_no_transfer_files_or_images_does_nothing() {
    let point = ComposerDropPoint::new(4.0, 9.0);
    for transfer in [
        ComposerFileTransfer::default(),
        transfer(vec![file("text/plain"), file("Image/png")], &["Files"]),
    ] {
        let decision = classify_drop(Some(&transfer), point);
        assert_eq!(decision, ComposerGestureDecision::ignored());
    }
    assert_eq!(
        classify_drop(None, point),
        ComposerGestureDecision::ignored()
    );
}

#[test]
fn paste_filters_in_order_prevents_default_and_has_no_point() {
    let incoming = transfer(
        vec![file("image/gif"), file("text/plain"), file("image/jpeg")],
        &["Files", "text/plain"],
    );

    retained_images(
        &classify_paste(Some(&incoming)),
        vec![file("image/gif"), file("image/jpeg")],
        None,
    );
}

#[test]
fn paste_with_null_empty_or_non_image_transfer_does_not_prevent_default() {
    let non_image = transfer(vec![file("text/plain")], &["Files"]);
    for decision in [
        classify_paste(None),
        classify_paste(Some(&ComposerFileTransfer::default())),
        classify_paste(Some(&non_image)),
    ] {
        assert_eq!(decision, ComposerGestureDecision::ignored());
    }
}

#[test]
fn composing_enter_does_not_submit_or_prevent_default() {
    let mut state = ComposerGestureState::new();
    let decision = state.submit_key(ComposerSubmitKeyInput::new(true, "Enter", false));

    assert_eq!(decision, ComposerGestureDecision::ignored());
    assert!(!state.is_submit_queued());
}

#[test]
fn non_enter_keys_do_not_submit_or_prevent_default() {
    let mut state = ComposerGestureState::new();
    for key in ["enter", "NumpadEnter", "Space", ""] {
        let decision = state.submit_key(ComposerSubmitKeyInput::new(false, key, false));
        assert_eq!(decision, ComposerGestureDecision::ignored(), "key={key:?}");
    }
    assert!(!state.is_submit_queued());
}

#[test]
fn shift_enter_does_not_submit_or_prevent_default() {
    let mut state = ComposerGestureState::new();
    let decision = state.submit_key(ComposerSubmitKeyInput::new(false, "Enter", true));

    assert_eq!(decision, ComposerGestureDecision::ignored());
    assert!(!state.is_submit_queued());
}

#[test]
fn plain_enter_submits_and_prevents_default() {
    let mut state = ComposerGestureState::new();
    let decision = state.submit_key(submit());

    assert_eq!(decision.gesture, Some(ComposerGesture::Submit));
    assert!(decision.prevent_default);
    assert!(state.is_submit_queued());
}

#[test]
fn repeated_plain_enter_is_prevented_but_not_submitted_twice() {
    let mut state = ComposerGestureState::new();
    let first = state.submit_key(submit());
    let second = state.submit_key(submit());

    assert_eq!(first.gesture, Some(ComposerGesture::Submit));
    assert!(first.prevent_default);
    assert_eq!(second.gesture, None);
    assert!(second.prevent_default);
    assert!(state.is_submit_queued());
}

#[test]
fn taking_an_image_does_not_release_a_pending_submit() {
    let mut state = ComposerGestureState::new();
    let _submit = state.submit_key(submit());
    let image = ComposerGesture::Images {
        files: vec![file("image/png")],
        point: None,
    };

    state.mark_taken(&image);
    assert!(state.is_submit_queued());
    assert_eq!(state.submit_key(submit()).gesture, None);
}

#[test]
fn taking_submit_releases_latch_before_failed_work_can_run() {
    let mut state = ComposerGestureState::new();
    let decision = state.submit_key(submit());
    let gesture = decision.gesture.expect("plain Enter retains a submit");

    // The worker takes the gesture before running it. A failure after this
    // point therefore cannot leave the de-duplication latch held.
    state.mark_taken(&gesture);
    assert!(!state.is_submit_queued());

    let retry = state.submit_key(submit());
    assert_eq!(retry.gesture, Some(ComposerGesture::Submit));
    assert!(retry.prevent_default);
    assert!(state.is_submit_queued());
}

#[test]
fn taking_submit_allows_a_later_plain_enter_after_successful_work_too() {
    let mut state = ComposerGestureState::new();
    let first = state.submit_key(submit()).gesture.expect("first submit");
    state.mark_taken(&first);

    let second = state.submit_key(submit());
    assert_eq!(second.gesture, Some(ComposerGesture::Submit));
    assert!(second.prevent_default);
}
