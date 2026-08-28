//! Direct, dependency-free coverage for the shared hover-pill group policy.
//!
//! The production module is included directly so this focused harness can be
//! compiled with pinned Rust 1.98 without Cargo, Bazel, or frontend
//! registration changes.

#[path = "../../modules/frontend/src/hover_pill_group_policy.rs"]
mod hover_pill_group_policy;

use hover_pill_group_policy::{HoverPillGroupPolicy, HoverPillTargetObservation};

fn observation(contained: bool, hovered: bool, focus_within: bool) -> HoverPillTargetObservation {
    HoverPillTargetObservation::new(contained, hovered, focus_within)
}

fn active_policy() -> HoverPillGroupPolicy<String> {
    let mut policy = HoverPillGroupPolicy::new();
    policy.apply_target(Some(String::from("first")), true);
    policy
}

#[test]
fn first_in_surface_target_is_immediate_and_advances_geometry() {
    let mut policy = HoverPillGroupPolicy::new();

    policy.apply_target(Some(String::from("first")), true);

    assert_eq!(policy.active_target(), Some(&String::from("first")));
    assert_eq!(policy.target(), Some(&String::from("first")));
    assert!(!policy.animated());
    assert_eq!(policy.geometry_version(), 1);
    assert_eq!(policy.version(), 1);
}

#[test]
fn subsequent_in_surface_target_animates_and_advances_once_each_time() {
    let mut policy = HoverPillGroupPolicy::new();

    policy.apply_target(Some(String::from("first")), true);
    policy.apply_target(Some(String::from("second")), true);
    assert_eq!(policy.active_target(), Some(&String::from("second")));
    assert!(policy.animated());
    assert_eq!(policy.geometry_version(), 2);

    // Even reapplying the same identity is a new host target application and
    // therefore requests another geometry read.
    policy.apply_target(Some(String::from("second")), true);
    assert!(policy.animated());
    assert_eq!(policy.geometry_version(), 3);
}

#[test]
fn invalid_and_out_of_surface_targets_clear_without_inventing_geometry() {
    let mut policy = active_policy();
    assert_eq!(policy.geometry_version(), 1);

    policy.apply_target(None, true);
    assert_eq!(policy.active_target(), None);
    assert!(!policy.animated());
    assert_eq!(policy.geometry_version(), 1);

    policy.apply_target(Some(String::from("inside")), true);
    assert_eq!(policy.geometry_version(), 2);
    policy.apply_target(Some(String::from("outside")), false);
    assert_eq!(policy.active_target(), None);
    assert!(!policy.animated());
    assert_eq!(policy.geometry_version(), 2);
}

#[test]
fn pointer_departure_clears_target_and_animation_but_not_version() {
    let mut policy = active_policy();
    policy.apply_target(Some(String::from("second")), true);
    assert!(policy.animated());
    assert_eq!(policy.geometry_version(), 2);

    policy.pointer_departure();

    assert_eq!(policy.active_target(), None);
    assert!(!policy.animated());
    assert_eq!(policy.geometry_version(), 2);
}

#[test]
fn focus_move_within_group_retains_state_and_departure_outside_clears() {
    let mut policy = active_policy();
    policy.apply_target(Some(String::from("second")), true);
    let before = policy.clone();

    policy.focus_departure(true);
    assert_eq!(policy, before);

    policy.focus_departure(false);
    assert_eq!(policy.active_target(), None);
    assert!(!policy.animated());
    assert_eq!(policy.geometry_version(), before.geometry_version());
}

#[test]
fn reconciliation_of_a_missing_target_clears_without_remeasurement() {
    let mut policy = active_policy();
    assert_eq!(policy.geometry_version(), 1);

    policy.reconcile(None);

    assert_eq!(policy.active_target(), None);
    assert!(!policy.animated());
    assert_eq!(policy.geometry_version(), 1);
}

#[test]
fn reconciliation_of_a_not_contained_target_clears_even_when_active() {
    let mut policy = active_policy();

    policy.reconcile(Some(observation(false, true, true)));

    assert_eq!(policy.active_target(), None);
    assert!(!policy.animated());
    assert_eq!(policy.geometry_version(), 1);
}

#[test]
fn reconciliation_of_unhovered_and_unfocused_target_clears() {
    let mut policy = active_policy();

    policy.reconcile(Some(observation(true, false, false)));

    assert_eq!(policy.active_target(), None);
    assert!(!policy.animated());
    assert_eq!(policy.geometry_version(), 1);
}

#[test]
fn reconciliation_retains_hovered_or_focus_within_targets_and_remeasures() {
    for (hovered, focus_within) in [(true, false), (false, true), (true, true)] {
        let mut policy = active_policy();
        let before = policy.geometry_version();

        policy.reconcile(Some(observation(true, hovered, focus_within)));

        assert_eq!(policy.active_target(), Some(&String::from("first")));
        assert!(!policy.animated());
        assert_eq!(policy.geometry_version(), before + 1);
    }
}

#[test]
fn reconciliation_without_an_active_target_is_a_no_op() {
    let mut policy = HoverPillGroupPolicy::<String>::new();

    policy.reconcile(Some(observation(true, true, true)));
    policy.reconcile(None);

    assert_eq!(policy.active_target(), None);
    assert!(!policy.animated());
    assert_eq!(policy.geometry_version(), 0);
}

#[test]
fn geometry_version_is_monotonic_across_applies_reconciliation_and_clears() {
    let mut policy = HoverPillGroupPolicy::new();
    let mut versions = vec![policy.geometry_version()];

    policy.apply_target(Some(String::from("first")), true);
    versions.push(policy.geometry_version());
    policy.reconcile(Some(observation(true, true, false)));
    versions.push(policy.geometry_version());
    policy.apply_target(Some(String::from("second")), true);
    versions.push(policy.geometry_version());
    policy.apply_target(Some(String::from("outside")), false);
    versions.push(policy.geometry_version());
    policy.pointer_departure();
    versions.push(policy.geometry_version());
    policy.apply_target(Some(String::from("third")), true);
    versions.push(policy.geometry_version());

    assert_eq!(versions, [0, 1, 2, 3, 3, 3, 4]);
    assert!(versions.windows(2).all(|pair| pair[1] >= pair[0]));
}
