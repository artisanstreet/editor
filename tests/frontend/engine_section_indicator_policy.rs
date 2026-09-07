//! Dependency-free transition coverage for the engine-section indicator.
//!
//! The production module is included directly so this focused harness can be
//! compiled with pinned Rust 1.98 without Cargo, Bazel, or frontend
//! registration changes.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/engine_section_indicator_policy.rs"]
mod engine_section_indicator_policy;

use engine_section_indicator_policy::{
    EngineSectionIndicatorMeasurement, EngineSectionIndicatorPolicy,
};

fn measurement(
    surface_left: f64,
    tab_left: f64,
    tab_width: f64,
) -> EngineSectionIndicatorMeasurement {
    EngineSectionIndicatorMeasurement::new(surface_left, tab_left, tab_width)
}

fn initial_policy() -> EngineSectionIndicatorPolicy {
    EngineSectionIndicatorPolicy::new()
}

fn assert_exact_float(actual: f64, expected: f64) {
    assert_eq!(actual.to_bits(), expected.to_bits());
}

#[test]
fn initial_state_is_hidden_still_and_unmeasured() {
    let policy = initial_policy();

    assert_eq!(policy.lit_engine(), None);
    assert!(!policy.indicator_visible());
    assert!(!policy.indicator_animated());
    assert_exact_float(policy.indicator_left(), 0.0);
    assert_exact_float(policy.indicator_width(), 0.0);
    assert_eq!(policy.resize_revision(), 0);
}

#[test]
fn first_valid_measurement_lights_without_animation() {
    let mut policy = initial_policy();

    assert!(policy.measure("codex", Some(measurement(10.0, 42.0, 32.0))));

    assert_eq!(policy.lit_engine(), Some("codex"));
    assert!(policy.indicator_visible());
    assert!(!policy.indicator_animated());
    assert_exact_float(policy.indicator_left(), 32.0);
    assert_exact_float(policy.indicator_width(), 32.0);
    assert_eq!(policy.resize_revision(), 0);
}

#[test]
fn remeasuring_the_same_engine_moves_without_animation() {
    let mut policy = initial_policy();
    policy.measure("codex", Some(measurement(10.0, 42.0, 32.0)));

    assert!(policy.measure("codex", Some(measurement(100.0, 105.5, 0.0))));

    assert_eq!(policy.lit_engine(), Some("codex"));
    assert!(policy.indicator_visible());
    assert!(!policy.indicator_animated());
    assert_exact_float(policy.indicator_left(), 5.5);
    assert_exact_float(policy.indicator_width(), 0.0);
}

#[test]
fn a_different_engine_animates_only_after_a_visible_measurement() {
    let mut policy = initial_policy();
    policy.measure("codex", Some(measurement(0.0, 1.0, 2.0)));

    assert!(policy.measure("claude", Some(measurement(1.0, 8.0, 3.0))));
    assert_eq!(policy.lit_engine(), Some("claude"));
    assert!(policy.indicator_visible());
    assert!(policy.indicator_animated());
    assert_exact_float(policy.indicator_left(), 7.0);
    assert_exact_float(policy.indicator_width(), 3.0);

    // Once the new engine is lit, another measurement of it is still a
    // remeasurement rather than another engine transition.
    policy.measure("claude", Some(measurement(2.0, 4.0, 5.0)));
    assert!(!policy.indicator_animated());
    assert_exact_float(policy.indicator_left(), 2.0);
    assert_exact_float(policy.indicator_width(), 5.0);
}

#[test]
fn every_missing_measurement_is_a_strict_no_op_before_and_after_visibility() {
    let mut fresh = initial_policy();
    assert!(!fresh.measure("codex", None));
    assert_eq!(fresh, initial_policy());

    let mut visible = initial_policy();
    visible.measure("codex", Some(measurement(10.0, 25.0, 15.0)));
    let before = visible.clone();

    // Missing surface, missing tab, and a failed geometry read all arrive at
    // this leaf as the same absent measurement.
    for missing_surface_or_tab_or_measurement in [None, None, None] {
        assert!(!visible.measure("claude", missing_surface_or_tab_or_measurement));
        assert_eq!(visible, before);
    }
}

#[test]
fn a_different_engine_after_a_missing_measurement_still_animates_from_the_last_lit_engine() {
    let mut policy = initial_policy();
    policy.measure("codex", Some(measurement(0.0, 4.0, 8.0)));
    let before_missing = policy.clone();

    assert!(!policy.measure("claude", None));
    assert_eq!(policy, before_missing);

    policy.measure("claude", Some(measurement(10.0, 3.0, 6.0)));
    assert_eq!(policy.lit_engine(), Some("claude"));
    assert!(policy.indicator_animated());
    assert_exact_float(policy.indicator_left(), -7.0);
    assert_exact_float(policy.indicator_width(), 6.0);
}

#[test]
fn resize_advances_only_the_revision_and_preserves_geometry_and_flags() {
    let mut policy = initial_policy();
    policy.measure("codex", Some(measurement(-5.0, 2.0, 7.0)));
    policy.measure("claude", Some(measurement(10.0, 4.0, 9.0)));
    let before_resize = policy.clone();

    policy.resize();
    assert_eq!(policy.resize_revision(), 1);
    assert_eq!(policy.lit_engine(), before_resize.lit_engine());
    assert_eq!(
        policy.indicator_visible(),
        before_resize.indicator_visible()
    );
    assert_eq!(
        policy.indicator_animated(),
        before_resize.indicator_animated()
    );
    assert_exact_float(policy.indicator_left(), before_resize.indicator_left());
    assert_exact_float(policy.indicator_width(), before_resize.indicator_width());

    policy.resize();
    assert_eq!(policy.resize_revision(), 2);
    assert_exact_float(policy.indicator_left(), before_resize.indicator_left());
    assert_exact_float(policy.indicator_width(), before_resize.indicator_width());
}

#[test]
fn same_engine_after_resize_remeasures_without_animation() {
    let mut policy = initial_policy();
    policy.measure("codex", Some(measurement(100.0, 110.0, 20.0)));
    policy.resize();
    policy.measure("codex", Some(measurement(250.0, 251.25, 0.5)));

    assert_eq!(policy.resize_revision(), 1);
    assert!(!policy.indicator_animated());
    assert_exact_float(policy.indicator_left(), 1.25);
    assert_exact_float(policy.indicator_width(), 0.5);
}

#[test]
fn zero_negative_and_fractional_geometry_is_preserved() {
    let mut policy = initial_policy();

    policy.measure("zero", Some(measurement(0.0, 0.0, 0.0)));
    assert_exact_float(policy.indicator_left(), 0.0);
    assert_exact_float(policy.indicator_width(), 0.0);

    policy.measure("negative", Some(measurement(20.5, -3.25, -7.75)));
    assert_exact_float(policy.indicator_left(), -23.75);
    assert_exact_float(policy.indicator_width(), -7.75);

    policy.measure("fractional", Some(measurement(-10.25, 3.5, 0.125)));
    assert_exact_float(policy.indicator_left(), 13.75);
    assert_exact_float(policy.indicator_width(), 0.125);
}

#[test]
fn empty_and_unicode_engine_identifiers_are_retained_and_compared_exactly() {
    let mut policy = initial_policy();
    let unicode_engine = "引擎 🚀 — café";

    policy.measure("", Some(measurement(0.0, 1.0, 2.0)));
    assert_eq!(policy.lit_engine(), Some(""));
    assert!(!policy.indicator_animated());

    policy.measure(unicode_engine, Some(measurement(0.0, 2.0, 3.0)));
    assert_eq!(policy.lit_engine(), Some(unicode_engine));
    assert!(policy.indicator_animated());

    policy.measure(unicode_engine, Some(measurement(0.0, 4.0, 5.0)));
    assert_eq!(policy.lit_engine(), Some(unicode_engine));
    assert!(!policy.indicator_animated());
}

#[test]
fn clone_is_independent_and_debug_exposes_the_policy_state() {
    let mut policy = initial_policy();
    policy.measure("codex", Some(measurement(1.0, 6.0, 5.0)));
    policy.resize();

    let mut clone = policy.clone();
    assert_eq!(clone, policy);

    let debug = format!("{policy:?}");
    assert!(debug.contains("EngineSectionIndicatorPolicy"));
    assert!(debug.contains("codex"));
    assert!(debug.contains("resize_revision: 1"));

    clone.measure("claude", Some(measurement(0.0, 10.0, 4.0)));
    assert_eq!(policy.lit_engine(), Some("codex"));
    assert_eq!(policy.resize_revision(), 1);
    assert_ne!(clone, policy);
}

#[test]
fn default_matches_new_and_transition_sequence_keeps_revision_independent() {
    let mut policy = EngineSectionIndicatorPolicy::default();
    assert_eq!(policy, initial_policy());

    policy.resize();
    policy.measure("first", Some(measurement(10.0, 15.0, 2.0)));
    policy.measure("first", None);
    policy.measure("second", Some(measurement(10.0, 5.0, 0.0)));
    policy.resize();
    policy.measure("second", Some(measurement(-1.0, -2.5, 1.5)));

    assert_eq!(policy.lit_engine(), Some("second"));
    assert!(policy.indicator_visible());
    assert!(!policy.indicator_animated());
    assert_exact_float(policy.indicator_left(), -1.5);
    assert_exact_float(policy.indicator_width(), 1.5);
    assert_eq!(policy.resize_revision(), 2);
}
