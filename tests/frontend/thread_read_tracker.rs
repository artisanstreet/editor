//! State-table coverage for the native thread read-tracking transition.
//!
//! The source is included directly so this focused harness remains dependency
//! free while the shared frontend module registration stays VP-owned.

#[path = "../../modules/frontend/src/thread_read_tracker.rs"]
mod thread_read_tracker;

use thread_read_tracker::{
    ThreadReadSnapshot, ThreadReadTrackingInput, ThreadReadTrackingState,
    advance_thread_read_tracking,
};

fn snapshot(
    thread_id: &str,
    reader_activity_at: &str,
    reader_acknowledged_activity_at: Option<&str>,
    has_active_work: bool,
) -> ThreadReadSnapshot {
    ThreadReadSnapshot::new(
        thread_id,
        reader_activity_at,
        reader_acknowledged_activity_at.map(str::to_owned),
        has_active_work,
    )
}

fn input(
    root_visible: bool,
    route_id: Option<&str>,
    thread: Option<ThreadReadSnapshot>,
) -> ThreadReadTrackingInput {
    ThreadReadTrackingInput::new(root_visible, route_id.map(str::to_owned), thread)
}

fn state(
    route_id: Option<&str>,
    thread: Option<ThreadReadSnapshot>,
    observed_activity_at: Option<&str>,
) -> ThreadReadTrackingState {
    ThreadReadTrackingState {
        route_id: route_id.map(str::to_owned),
        thread,
        observed_activity_at: observed_activity_at.map(str::to_owned),
    }
}

#[test]
fn departure_acknowledgement_state_table_covers_every_guard_branch() {
    struct Case {
        name: &'static str,
        departed: Option<ThreadReadSnapshot>,
        observed_activity_at: Option<&'static str>,
        expected_acknowledgement: Option<(&'static str, &'static str)>,
    }

    let cases = [
        Case {
            name: "missing departed thread",
            departed: None,
            observed_activity_at: Some("seen"),
            expected_acknowledgement: None,
        },
        Case {
            name: "missing observed activity",
            departed: Some(snapshot("old", "activity", None, false)),
            observed_activity_at: None,
            expected_acknowledgement: None,
        },
        Case {
            name: "active work",
            departed: Some(snapshot("old", "seen", None, true)),
            observed_activity_at: Some("seen"),
            expected_acknowledgement: None,
        },
        Case {
            name: "already acknowledged exact activity",
            departed: Some(snapshot("old", "seen", Some("seen"), false)),
            observed_activity_at: Some("seen"),
            expected_acknowledgement: None,
        },
        Case {
            name: "no prior acknowledgement",
            departed: Some(snapshot("old", "latest", None, false)),
            observed_activity_at: Some("seen"),
            expected_acknowledgement: Some(("old", "seen")),
        },
        Case {
            name: "different prior acknowledgement",
            departed: Some(snapshot("old", "latest", Some("older"), false)),
            observed_activity_at: Some("seen"),
            expected_acknowledgement: Some(("old", "seen")),
        },
    ];

    for case in cases {
        let previous = state(Some("old-route"), case.departed, case.observed_activity_at);
        let transition =
            advance_thread_read_tracking(&previous, &input(false, Some("new-route"), None));
        let actual = transition.acknowledgement.as_ref().map(|acknowledgement| {
            (
                acknowledgement.thread_id.as_str(),
                acknowledgement.reader_activity_at.as_str(),
            )
        });

        assert_eq!(
            actual, case.expected_acknowledgement,
            "unexpected acknowledgement for {}",
            case.name
        );
        assert_eq!(
            transition.state,
            state(Some("new-route"), None, None),
            "route departure must replace the tracked state for {}",
            case.name
        );
    }
}

#[test]
fn unchanged_route_merges_a_refreshed_thread_and_visible_activity() {
    let previous = state(
        Some("route"),
        Some(snapshot("thread", "old-activity", Some("old-ack"), false)),
        Some("old-activity"),
    );
    let refreshed = snapshot("thread", "new-activity", Some("new-ack"), true);

    let transition = advance_thread_read_tracking(
        &previous,
        &input(true, Some("route"), Some(refreshed.clone())),
    );

    assert_eq!(transition.acknowledgement, None);
    assert_eq!(
        transition.state,
        state(Some("route"), Some(refreshed), Some("new-activity"))
    );
}

#[test]
fn unchanged_route_without_a_new_thread_retains_the_previous_thread() {
    let previous = state(
        Some("route"),
        Some(snapshot("thread", "activity", None, false)),
        Some("activity"),
    );

    let transition = advance_thread_read_tracking(&previous, &input(true, Some("route"), None));

    assert_eq!(transition.acknowledgement, None);
    assert_eq!(transition.state, previous);
}

#[test]
fn unchanged_route_without_any_thread_retains_an_existing_observation() {
    let previous = state(Some("route"), None, Some("orphaned-observation"));

    let transition = advance_thread_read_tracking(&previous, &input(true, Some("route"), None));

    assert_eq!(transition.acknowledgement, None);
    assert_eq!(transition.state, previous);
}

#[test]
fn hidden_same_route_refreshes_thread_but_retains_the_last_visible_cursor() {
    let previous = state(
        Some("route"),
        Some(snapshot("thread", "visible-activity", None, false)),
        Some("visible-activity"),
    );
    let hidden_refresh = snapshot("thread", "hidden-activity", None, false);

    let transition = advance_thread_read_tracking(
        &previous,
        &input(false, Some("route"), Some(hidden_refresh.clone())),
    );

    assert_eq!(transition.acknowledgement, None);
    assert_eq!(
        transition.state,
        state(
            Some("route"),
            Some(hidden_refresh),
            Some("visible-activity")
        )
    );
}

#[test]
fn visible_refresh_after_hidden_retention_observes_the_new_activity() {
    let hidden = advance_thread_read_tracking(
        &state(
            Some("route"),
            Some(snapshot("thread", "visible", None, false)),
            Some("visible"),
        ),
        &input(
            false,
            Some("route"),
            Some(snapshot("thread", "hidden", None, false)),
        ),
    );

    let visible = advance_thread_read_tracking(
        &hidden.state,
        &input(
            true,
            Some("route"),
            Some(snapshot("thread", "visible-again", None, false)),
        ),
    );

    assert_eq!(
        hidden.state.observed_activity_at.as_deref(),
        Some("visible")
    );
    assert_eq!(
        visible.state.observed_activity_at.as_deref(),
        Some("visible-again")
    );
}

#[test]
fn route_disappearance_resets_observation_and_acknowledges_the_departed_thread() {
    let previous = state(
        Some("old-route"),
        Some(snapshot("old-thread", "latest", None, false)),
        Some("observed"),
    );

    let transition = advance_thread_read_tracking(&previous, &input(false, None, None));

    assert_eq!(
        transition.acknowledgement,
        Some(thread_read_tracker::ThreadReadAcknowledgement {
            thread_id: "old-thread".to_owned(),
            reader_activity_at: "observed".to_owned(),
        })
    );
    assert_eq!(transition.state, state(None, None, None));
}

#[test]
fn route_switch_acknowledges_old_observation_and_starts_new_visible_route() {
    let previous = state(
        Some("old-route"),
        Some(snapshot(
            "old-thread",
            "newer-than-seen",
            Some("older"),
            false,
        )),
        Some("exact-observed-old"),
    );
    let next_thread = snapshot("new-thread", "new-visible", None, false);

    let transition = advance_thread_read_tracking(
        &previous,
        &input(true, Some("new-route"), Some(next_thread.clone())),
    );

    assert_eq!(
        transition.acknowledgement,
        Some(thread_read_tracker::ThreadReadAcknowledgement {
            thread_id: "old-thread".to_owned(),
            reader_activity_at: "exact-observed-old".to_owned(),
        })
    );
    assert_eq!(
        transition.state,
        state(Some("new-route"), Some(next_thread), Some("new-visible"))
    );
}

#[test]
fn hidden_route_switch_clears_observation_even_when_the_new_thread_exists() {
    let previous = state(
        Some("old-route"),
        Some(snapshot("old-thread", "old", None, false)),
        Some("old"),
    );
    let next_thread = snapshot("new-thread", "not-observed", None, false);

    let transition = advance_thread_read_tracking(
        &previous,
        &input(false, Some("new-route"), Some(next_thread.clone())),
    );

    assert_eq!(
        transition.state,
        state(Some("new-route"), Some(next_thread), None)
    );
    assert_eq!(
        transition
            .acknowledgement
            .as_ref()
            .map(|acknowledgement| acknowledgement.reader_activity_at.as_str()),
        Some("old")
    );
}

#[test]
fn exact_acknowledged_activity_suppresses_only_the_matching_departure() {
    let previous = state(
        Some("route"),
        Some(snapshot("thread", "current", Some("different"), false)),
        Some("observed"),
    );
    let first_departure =
        advance_thread_read_tracking(&previous, &input(false, Some("next"), None));
    assert!(first_departure.acknowledgement.is_some());

    let already_settled = state(
        Some("route"),
        Some(snapshot("thread", "current", Some("observed"), false)),
        Some("observed"),
    );
    let second_departure =
        advance_thread_read_tracking(&already_settled, &input(false, Some("next"), None));
    assert_eq!(second_departure.acknowledgement, None);
}
