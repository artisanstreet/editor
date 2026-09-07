//! Dependency-free transition coverage for the active-thread light policy.
//!
//! The production module is included directly so these tests cover the pure
//! state boundary without Cargo, Bazel, browser APIs, or frontend
//! registration.

#![allow(clippy::float_cmp)]
#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/active_thread_light_policy.rs"]
mod active_thread_light_policy;

use active_thread_light_policy::{ActiveThreadLightMeasurement, ActiveThreadLightPolicy};

fn measurement(
    row_top: f64,
    row_height: f64,
    surface_top: f64,
    scroll_top: f64,
) -> ActiveThreadLightMeasurement {
    ActiveThreadLightMeasurement::new(row_top, row_height, surface_top, scroll_top)
}

fn assert_state(
    policy: &ActiveThreadLightPolicy,
    expected_id: Option<&str>,
    expected_visible: bool,
    expected_animated: bool,
    expected_top: f64,
    expected_height: f64,
    expected_revision: u64,
) {
    assert_eq!(policy.lit_thread_id(), expected_id);
    assert_eq!(policy.visible(), expected_visible);
    assert_eq!(policy.animated(), expected_animated);
    assert_eq!(policy.top(), expected_top);
    assert_eq!(policy.height(), expected_height);
    assert_eq!(policy.resize_revision(), expected_revision);
}

#[test]
fn new_and_default_policies_have_the_same_hidden_zero_state() {
    let new = ActiveThreadLightPolicy::new();
    let default = ActiveThreadLightPolicy::default();

    assert_state(&new, None, false, false, 0.0, 0.0, 0);
    assert_eq!(new, default);
}

#[test]
fn first_measurement_is_visible_without_animation_and_copies_geometry() {
    let mut policy = ActiveThreadLightPolicy::new();

    policy.measure(
        Some(String::from("first")),
        Some(measurement(120.0, 44.0, 20.0, 8.0)),
    );

    assert_state(&policy, Some("first"), true, false, 108.0, 44.0, 0);
}

#[test]
fn same_thread_remeasurement_stays_still_even_when_geometry_changes() {
    let mut policy = ActiveThreadLightPolicy::new();
    let initial = measurement(100.0, 30.0, 10.0, 4.0);
    policy.measure(Some(String::from("same")), Some(initial));

    // A layout-equivalent remeasurement is still a remeasurement, not a move.
    policy.measure(Some(String::from("same")), Some(initial));
    assert_state(&policy, Some("same"), true, false, 94.0, 30.0, 0);

    policy.measure(
        Some(String::from("same")),
        Some(measurement(140.0, 52.0, 18.0, 12.0)),
    );

    assert_state(&policy, Some("same"), true, false, 134.0, 52.0, 0);
}

#[test]
fn changing_thread_while_visible_requests_animation_once() {
    let mut policy = ActiveThreadLightPolicy::new();
    policy.measure(
        Some(String::from("first")),
        Some(measurement(10.0, 20.0, 0.0, 0.0)),
    );

    policy.measure(
        Some(String::from("second")),
        Some(measurement(40.0, 24.0, 5.0, 2.0)),
    );
    assert_state(&policy, Some("second"), true, true, 37.0, 24.0, 0);

    policy.measure(
        Some(String::from("second")),
        Some(measurement(42.0, 26.0, 5.0, 2.0)),
    );
    assert_state(&policy, Some("second"), true, false, 39.0, 26.0, 0);
}

#[test]
fn missing_measurement_before_and_after_visibility_changes_only_visibility() {
    let mut policy = ActiveThreadLightPolicy::new();
    policy.measure(
        Some(String::from("first")),
        Some(measurement(80.0, 16.0, 8.0, 1.0)),
    );
    policy.measure(
        Some(String::from("second")),
        Some(measurement(120.0, 18.0, 8.0, 1.0)),
    );
    assert!(policy.animated());

    policy.measure(Some(String::from("ignored")), None);
    assert_state(&policy, Some("second"), false, true, 113.0, 18.0, 0);

    let mut fresh = ActiveThreadLightPolicy::new();
    fresh.measure(None, None);
    assert_state(&fresh, None, false, false, 0.0, 0.0, 0);

    // The explicit hide operation is the same missing-observation transition.
    policy.hide();
    assert_state(&policy, Some("second"), false, true, 113.0, 18.0, 0);
}

#[test]
fn different_thread_after_missing_row_does_not_animate() {
    let mut policy = ActiveThreadLightPolicy::new();
    policy.measure(
        Some(String::from("old")),
        Some(measurement(20.0, 10.0, 2.0, 0.0)),
    );
    policy.measure(Some(String::from("old")), None);

    policy.measure(
        Some(String::from("new")),
        Some(measurement(200.0, 30.0, 12.0, 5.0)),
    );

    assert_state(&policy, Some("new"), true, false, 193.0, 30.0, 0);
}

#[test]
fn none_empty_and_unicode_thread_ids_are_distinct_exact_values() {
    let mut policy = ActiveThreadLightPolicy::new();

    policy.measure(None, Some(measurement(1.0, 2.0, 0.0, 0.0)));
    assert_state(&policy, None, true, false, 1.0, 2.0, 0);

    policy.measure(None, Some(measurement(3.0, 4.0, 0.0, 0.0)));
    assert_state(&policy, None, true, false, 3.0, 4.0, 0);

    policy.measure(Some(String::new()), Some(measurement(5.0, 6.0, 0.0, 0.0)));
    assert_state(&policy, Some(""), true, true, 5.0, 6.0, 0);

    let unicode_id = "🧵 café — 日本語";
    policy.measure(
        Some(String::from(unicode_id)),
        Some(measurement(7.0, 8.0, 0.0, 0.0)),
    );
    assert_state(&policy, Some(unicode_id), true, true, 7.0, 8.0, 0);

    policy.measure(
        Some(String::from(unicode_id)),
        Some(measurement(9.0, 10.0, 0.0, 0.0)),
    );
    assert_state(&policy, Some(unicode_id), true, false, 9.0, 10.0, 0);
}

#[test]
fn zero_negative_fractional_geometry_and_scroll_offsets_are_preserved() {
    let mut policy = ActiveThreadLightPolicy::new();
    let observed = measurement(-10.25, -3.5, 4.75, 100.5);

    assert_eq!(observed.row_top, -10.25);
    assert_eq!(observed.row_height, -3.5);
    assert_eq!(observed.surface_top, 4.75);
    assert_eq!(observed.scroll_top, 100.5);

    policy.measure(Some(String::from("geometry")), Some(observed));
    assert_state(&policy, Some("geometry"), true, false, 85.5, -3.5, 0);

    policy.measure(
        Some(String::from("zero")),
        Some(measurement(0.0, 0.0, 0.0, 0.0)),
    );
    assert_state(&policy, Some("zero"), true, true, 0.0, 0.0, 0);
}

#[test]
fn resize_changes_only_the_revision_token() {
    let mut policy = ActiveThreadLightPolicy::new();
    policy.measure(
        Some(String::from("thread")),
        Some(measurement(-3.0, 12.5, 7.0, 2.0)),
    );
    policy.measure(
        Some(String::from("other")),
        Some(measurement(30.0, 18.5, 7.0, 2.0)),
    );
    assert!(policy.animated());

    policy.resize();
    assert_state(&policy, Some("other"), true, true, 25.0, 18.5, 1);

    policy.resize();
    assert_state(&policy, Some("other"), true, true, 25.0, 18.5, 2);
}

#[test]
fn transition_sequence_matches_visibility_and_animation_invariants() {
    let mut policy = ActiveThreadLightPolicy::new();
    let transitions = [
        (None, false, false, 0.0, 0.0, 0),
        (Some("a"), true, false, 10.0, 5.0, 0),
        (Some("a"), true, false, 11.0, 6.0, 0),
        (Some("b"), true, true, 12.0, 7.0, 0),
        (Some("b"), false, true, 12.0, 7.0, 0),
        (Some("c"), true, false, 13.0, 8.0, 0),
    ];

    assert_state(&policy, None, false, false, 0.0, 0.0, 0);
    policy.measure(
        Some(String::from("a")),
        Some(measurement(10.0, 5.0, 0.0, 0.0)),
    );
    assert_state(
        &policy,
        transitions[1].0,
        transitions[1].1,
        transitions[1].2,
        transitions[1].3,
        transitions[1].4,
        transitions[1].5,
    );

    policy.measure(
        Some(String::from("a")),
        Some(measurement(11.0, 6.0, 0.0, 0.0)),
    );
    assert_state(
        &policy,
        transitions[2].0,
        transitions[2].1,
        transitions[2].2,
        transitions[2].3,
        transitions[2].4,
        transitions[2].5,
    );

    policy.measure(
        Some(String::from("b")),
        Some(measurement(12.0, 7.0, 0.0, 0.0)),
    );
    assert_state(
        &policy,
        transitions[3].0,
        transitions[3].1,
        transitions[3].2,
        transitions[3].3,
        transitions[3].4,
        transitions[3].5,
    );

    policy.measure(Some(String::from("b")), None);
    assert_state(
        &policy,
        transitions[4].0,
        transitions[4].1,
        transitions[4].2,
        transitions[4].3,
        transitions[4].4,
        transitions[4].5,
    );

    policy.measure(
        Some(String::from("c")),
        Some(measurement(13.0, 8.0, 0.0, 0.0)),
    );
    assert_state(
        &policy,
        transitions[5].0,
        transitions[5].1,
        transitions[5].2,
        transitions[5].3,
        transitions[5].4,
        transitions[5].5,
    );
}

#[test]
fn measurement_and_policy_clone_and_debug_preserve_state() {
    let observed = measurement(1.5, 2.5, -3.5, 4.5);
    let observed_clone = observed;
    assert_eq!(observed, observed_clone);
    assert!(format!("{observed:?}").contains("ActiveThreadLightMeasurement"));

    let mut policy = ActiveThreadLightPolicy::new();
    policy.measure(Some(String::from("debug 🚀")), Some(observed));
    policy.resize();

    let clone = policy.clone();
    assert_eq!(clone, policy);
    assert!(format!("{policy:?}").contains("ActiveThreadLightPolicy"));
}
