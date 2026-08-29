//! Focused, dependency-free coverage for the editor view-state policy.

#![allow(clippy::float_cmp)]

#[path = "../../modules/frontend/src/editor_view_state_policy.rs"]
mod editor_view_state_policy;

use editor_view_state_policy::{
    EditorViewState, EditorViewStateInput, RestoreViewStateResult, clamp_position,
    normalize_scroll_top, restore_view_state, save_view_state,
};

fn applied(result: RestoreViewStateResult) -> EditorViewState {
    match result {
        RestoreViewStateResult::Applied(state) => state,
        RestoreViewStateResult::NoOp => panic!("expected a compatible state to be applied"),
    }
}

#[test]
fn saving_preserves_forward_reversed_and_caret_selections_exactly() {
    let cases = [(2, 7), (7, 2), (4, 4)];

    for (anchor, head) in cases {
        let state = save_view_state(anchor, head, 37.5).expect("finite scroll should save");

        assert_eq!(state.anchor, anchor);
        assert_eq!(state.head, head);
        assert_eq!(state.scroll_top, 37.5);
    }
}

#[test]
fn empty_document_clamps_both_endpoints_to_zero() {
    let state = applied(restore_view_state(
        "",
        EditorViewStateInput::owned(EditorViewState::new(1, usize::MAX, 19.0)),
    ));

    assert_eq!(state.anchor, 0);
    assert_eq!(state.head, 0);
    assert_eq!(state.scroll_top, 19.0);
}

#[test]
fn ascii_positions_at_length_and_beyond_length_are_clamped_independently() {
    let document = "hello";
    let state = applied(restore_view_state(
        document,
        EditorViewStateInput::owned(EditorViewState::new(5, 99, 8.0)),
    ));

    assert_eq!(state.anchor, document.len());
    assert_eq!(state.head, document.len());
    assert_eq!(state.scroll_top, 8.0);
}

#[test]
fn restoring_preserves_forward_reversed_and_caret_selection_shapes() {
    let document = "abcdef";

    for (anchor, head) in [(1, 4), (4, 1), (3, 3)] {
        let state = applied(restore_view_state(
            document,
            EditorViewStateInput::owned(EditorViewState::new(anchor, head, 6.0)),
        ));

        assert_eq!((state.anchor, state.head), (anchor, head));
    }
}

#[test]
fn unicode_positions_floor_inside_scalars_and_keep_reversed_selection() {
    let document = "aé🐍z";
    assert_eq!(document.len(), 8);
    // Valid boundaries are 0, 1, 3, 7, and 8. Byte 2 is inside `é`, and
    // byte 5 is inside the four-byte snake scalar.
    let state = applied(restore_view_state(
        document,
        EditorViewStateInput::owned(EditorViewState::new(5, 2, 31.25)),
    ));

    assert_eq!(state.anchor, 3);
    assert_eq!(state.head, 1);
    assert!(
        state.anchor > state.head,
        "reversed selection must stay reversed"
    );
    assert_eq!(state.scroll_top, 31.25);
}

#[test]
fn positions_exactly_on_unicode_boundaries_and_at_end_are_unchanged() {
    let document = "é🐍";
    let boundaries = [0, 2, 6];

    for position in boundaries {
        assert_eq!(clamp_position(document, position), position);
    }
    assert_eq!(clamp_position(document, usize::MAX), document.len());
}

#[test]
fn save_rejects_scroll_values_outside_the_browser_snapshot_domain() {
    for scroll_top in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(save_view_state(4, 1, scroll_top), None);
    }
}

#[test]
fn finite_oversized_scroll_is_preserved_until_layout_can_clamp_it() {
    let state = save_view_state(0, 0, f64::MAX).expect("finite oversized scroll is accepted");

    assert_eq!(state.scroll_top, f64::MAX);
    let restored = applied(restore_view_state(
        "content",
        EditorViewStateInput::owned(state),
    ));
    assert_eq!(restored.scroll_top, f64::MAX);
}

#[test]
fn restore_normalizes_negative_and_non_finite_scroll_without_selection_coupling() {
    let cases = [-4.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY];

    for scroll_top in cases {
        let state = applied(restore_view_state(
            "abc",
            EditorViewStateInput::owned(EditorViewState::new(99, 1, scroll_top)),
        ));

        assert_eq!(state.anchor, 3);
        assert_eq!(state.head, 1);
        assert_eq!(state.scroll_top, 0.0);
    }

    for scroll_top in [0.0, 0.5, 72.25] {
        assert_eq!(normalize_scroll_top(scroll_top), scroll_top);
    }
}

#[test]
fn missing_and_opaque_inputs_are_explicit_no_ops() {
    let document = "unchanged";

    for input in [
        EditorViewStateInput::missing(),
        EditorViewStateInput::opaque(),
    ] {
        let result = restore_view_state(document, input);

        assert_eq!(result, RestoreViewStateResult::NoOp);
        assert!(result.is_no_op());
        assert_eq!(result.state(), None);
    }
}

#[test]
fn optional_input_maps_absence_to_no_op_and_presence_to_owned_state() {
    let document = "abc";
    let missing: Option<EditorViewState> = None;
    let present = Some(EditorViewState::new(1, 2, 4.0));

    assert_eq!(
        restore_view_state(document, missing.into()),
        RestoreViewStateResult::NoOp
    );
    assert_eq!(
        restore_view_state(document, present.into()),
        RestoreViewStateResult::Applied(EditorViewState::new(1, 2, 4.0))
    );
}

#[test]
fn restoring_the_same_normalized_state_repeatedly_is_idempotent() {
    let document = "aé🐍z";
    let first = applied(restore_view_state(
        document,
        EditorViewStateInput::owned(EditorViewState::new(usize::MAX, 2, f64::NAN)),
    ));
    let second = applied(restore_view_state(
        document,
        EditorViewStateInput::owned(first),
    ));

    assert_eq!(first, second);
}

#[test]
fn restoration_never_mutates_document_text() {
    let mut document = String::from("before é🐍 after");
    let original = document.clone();
    let state = EditorViewState::new(7, 1, 12.0);

    let restored = restore_view_state(&document, EditorViewStateInput::owned(state));

    assert_eq!(
        restored.state().expect("state should apply").scroll_top,
        12.0
    );
    assert_eq!(document, original);
    document.push('!');
    assert_eq!(document, "before é🐍 after!");
}
