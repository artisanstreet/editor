//! Focused, dependency-free coverage for dropdown highlight settlement.
//!
//! The production module is path-linked deliberately: this packet does not
//! edit frontend registration, so the state machine can be checked with plain
//! `rustc --test` and no host, DOM, or runtime dependencies.

#![allow(clippy::float_cmp)]

#[path = "../../modules/frontend/src/dropdown_highlight_settle.rs"]
mod dropdown_highlight_settle;

use dropdown_highlight_settle::{
    DropdownHighlightAction, DropdownHighlightCommand, DropdownHighlightObservation,
    DropdownHighlightSettleState, SETTLE_WINDOW_MS,
};

fn geometry(now_ms: f64, marker_present: bool, value: &str) -> DropdownHighlightObservation<'_> {
    DropdownHighlightObservation::with_geometry(now_ms, marker_present, value)
}

fn no_geometry(now_ms: f64, marker_present: bool) -> DropdownHighlightObservation<'static> {
    DropdownHighlightObservation::without_geometry(now_ms, marker_present)
}

fn apply(value: &str) -> DropdownHighlightAction {
    DropdownHighlightAction::ApplyGeometry(value.to_owned())
}

#[test]
fn command_vocabulary_and_settle_window_are_exact() {
    assert_eq!(SETTLE_WINDOW_MS, 250.0);
}

#[test]
fn dispatch_exposes_each_source_command_as_the_matching_transition() {
    let mut state = DropdownHighlightSettleState::new();

    assert_eq!(
        state.dispatch(DropdownHighlightCommand::Watch, no_geometry(100.0, false),),
        vec![DropdownHighlightAction::ScheduleFrame]
    );
    assert_eq!(
        state.dispatch(
            DropdownHighlightCommand::Settle,
            geometry(110.0, false, "ignored"),
        ),
        vec![DropdownHighlightAction::ScheduleFrame]
    );
    assert!(
        state
            .dispatch(
                DropdownHighlightCommand::Highlighted,
                geometry(120.0, false, "ignored"),
            )
            .is_empty()
    );
}

#[test]
fn first_reveal_requires_two_consecutive_equal_geometry_strings() {
    let mut state = DropdownHighlightSettleState::new();

    assert_eq!(
        state.watch(1_000.0),
        vec![DropdownHighlightAction::ScheduleFrame]
    );
    assert_eq!(
        state.settle(geometry(1_010.0, true, "0:0:80:24")),
        vec![DropdownHighlightAction::ScheduleFrame]
    );
    assert_eq!(state.applied_geometry(), None);
    assert_eq!(state.sampled_geometry(), Some("0:0:80:24"));

    assert_eq!(
        state.settle(geometry(1_020.0, true, "0:0:80:24")),
        vec![apply("0:0:80:24"), DropdownHighlightAction::ScheduleFrame,]
    );
    assert_eq!(state.applied_geometry(), Some("0:0:80:24"));
    assert!(state.is_revealed());
}

#[test]
fn changing_layout_keeps_waiting_until_a_pair_agrees() {
    let mut state = DropdownHighlightSettleState::new();
    let _ = state.watch(0.0);

    for (now_ms, value) in [(10.0, "0:0:80:24"), (20.0, "0:0:82:24")] {
        assert_eq!(
            state.settle(geometry(now_ms, true, value)),
            vec![DropdownHighlightAction::ScheduleFrame]
        );
        assert_eq!(state.applied_geometry(), None);
    }

    assert_eq!(
        state.settle(geometry(30.0, true, "0:0:82:24")),
        vec![apply("0:0:82:24"), DropdownHighlightAction::ScheduleFrame,]
    );
}

#[test]
fn revealed_settlement_applies_only_changed_geometry() {
    let mut state = DropdownHighlightSettleState::new();
    let _ = state.watch(0.0);
    let _ = state.settle(geometry(1.0, true, "0:0:80:24"));
    let _ = state.settle(geometry(2.0, true, "0:0:80:24"));

    assert_eq!(
        state.settle(geometry(3.0, true, "0:0:84:28")),
        vec![apply("0:0:84:28"), DropdownHighlightAction::ScheduleFrame,]
    );
    assert_eq!(
        state.settle(geometry(4.0, true, "0:0:84:28")),
        vec![DropdownHighlightAction::ScheduleFrame]
    );
    assert_eq!(state.applied_geometry(), Some("0:0:84:28"));
}

#[test]
fn marker_removal_is_a_noop_for_highlighted_and_skips_settle_sampling() {
    let mut state = DropdownHighlightSettleState::new();
    let _ = state.watch(100.0);
    let _ = state.settle(geometry(110.0, true, "0:0:80:24"));

    let deadline_before = state.deadline_ms();
    let frame_before = state.frame_pending();
    assert!(
        state
            .highlighted(geometry(120.0, false, "removed geometry"))
            .is_empty()
    );
    assert_eq!(state.deadline_ms(), deadline_before);
    assert_eq!(state.frame_pending(), frame_before);

    assert_eq!(
        state.settle(geometry(130.0, false, "0:0:999:999")),
        vec![DropdownHighlightAction::ScheduleFrame]
    );
    assert_eq!(state.sampled_geometry(), Some("0:0:80:24"));
    assert_eq!(state.applied_geometry(), None);
}

#[test]
fn highlighted_after_reveal_applies_now_then_renews_the_watch() {
    let mut state = DropdownHighlightSettleState::new();
    let _ = state.watch(0.0);
    let _ = state.settle(geometry(1.0, true, "0:0:80:24"));
    let _ = state.settle(geometry(2.0, true, "0:0:80:24"));
    assert!(state.frame_pending());

    assert_eq!(
        state.highlighted(geometry(500.0, true, "0:0:96:30")),
        vec![apply("0:0:96:30")]
    );
    assert_eq!(state.applied_geometry(), Some("0:0:96:30"));
    assert_eq!(state.deadline_ms(), 750.0);

    // The highlighted command renewed the existing frame rather than
    // scheduling a second one.
    assert!(state.frame_pending());

    // Once that renewed window reaches its boundary, a later highlight still
    // applies immediately and starts a fresh single-frame watch.
    let _ = state.settle(no_geometry(750.0, false));
    assert!(!state.frame_pending());
    assert_eq!(
        state.highlighted(geometry(800.0, true, "0:0:100:32")),
        vec![apply("0:0:100:32"), DropdownHighlightAction::ScheduleFrame,]
    );
    assert_eq!(state.deadline_ms(), 1_050.0);
}

#[test]
fn highlighted_without_a_marker_does_not_renew_or_apply() {
    let mut state = DropdownHighlightSettleState::new();

    assert!(
        state
            .highlighted(geometry(500.0, false, "0:0:96:30"))
            .is_empty()
    );
    assert_eq!(state.deadline_ms(), 0.0);
    assert_eq!(state.applied_geometry(), None);
    assert!(!state.frame_pending());
}

#[test]
fn watch_replaces_deadline_but_owns_at_most_one_frame() {
    let mut state = DropdownHighlightSettleState::new();

    assert_eq!(
        state.watch(100.0),
        vec![DropdownHighlightAction::ScheduleFrame]
    );
    assert_eq!(state.watch(200.0), Vec::new());
    assert_eq!(state.deadline_ms(), 200.0 + SETTLE_WINDOW_MS);
    assert!(state.frame_pending());

    // A settle consumes the current frame and replaces it with one next
    // frame while the renewed deadline remains open.
    assert_eq!(
        state.settle(geometry(449.0, false, "ignored")),
        vec![DropdownHighlightAction::ScheduleFrame]
    );
    assert!(state.frame_pending());
}

#[test]
fn deadline_equality_stops_scheduling_but_just_before_it_continues() {
    let mut before = DropdownHighlightSettleState::new();
    let _ = before.watch(1_000.0);
    assert_eq!(
        before.settle(no_geometry(1_249.999, false)),
        vec![DropdownHighlightAction::ScheduleFrame]
    );

    let mut at = DropdownHighlightSettleState::new();
    let _ = at.watch(1_000.0);
    assert_eq!(
        at.settle(no_geometry(1_000.0 + SETTLE_WINDOW_MS, false)),
        Vec::new()
    );
    assert!(!at.frame_pending());
}

#[test]
fn deadline_equality_still_applies_a_changed_settle_geometry() {
    let mut state = DropdownHighlightSettleState::new();
    let _ = state.watch(0.0);
    let _ = state.settle(geometry(1.0, true, "0:0:80:24"));
    let _ = state.settle(geometry(2.0, true, "0:0:80:24"));
    let _ = state.watch(1_000.0);

    assert_eq!(
        state.settle(geometry(1_250.0, true, "0:0:88:24")),
        vec![apply("0:0:88:24")]
    );
    assert_eq!(state.applied_geometry(), Some("0:0:88:24"));
    assert!(!state.frame_pending());
}

#[test]
fn cleanup_cancels_the_current_frame_and_disconnects_both_observers() {
    let mut state = DropdownHighlightSettleState::new();
    let _ = state.watch(100.0);

    assert_eq!(
        state.cleanup(),
        vec![
            DropdownHighlightAction::CancelFrame,
            DropdownHighlightAction::DisconnectHighlightObserver,
            DropdownHighlightAction::DisconnectSizeObserver,
        ]
    );
    assert!(!state.frame_pending());
    assert!(state.is_cleaned_up());
    assert!(state.cleanup().is_empty());
    assert!(state.watch(1_000.0).is_empty());
}
