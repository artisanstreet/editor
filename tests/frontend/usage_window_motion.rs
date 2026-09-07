//! Focused numerical coverage for the browser-free usage-window motion policy.
//!
//! Including the source directly keeps this packet dependency-free until the
//! VP adds its shared module and test registrations.

#[path = "../../modules/frontend/src/usage_window_motion.rs"]
mod usage_window_motion;

use usage_window_motion::{
    CubicBezier, DEFAULT_MOTION_EASING, INVALID_DURATION_MILLISECONDS, MotionDurationInput,
    MotionEasingInput, cubic_bezier, motion_duration, motion_easing, parse_cubic_bezier_token,
};

const CURVE_TOLERANCE: f64 = 1e-10;
const BISECTION_TOLERANCE: f64 = 1e-4;

fn assert_close(actual: f64, expected: f64, tolerance: f64, label: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{label}: expected {expected:.17e}, got {actual:.17e}, tolerance {tolerance:.1e}"
    );
}

fn duration(token: Option<&str>, reduced_motion: bool) -> f64 {
    motion_duration(MotionDurationInput::new(token, reduced_motion))
}

#[test]
fn run_up_from_is_nonnegative_and_preserves_the_legacy_eight_percent_quip() {
    let targets = [
        (-100.0, 0.0),
        (0.0, 0.0),
        (1.0, 0.0),
        (10.0, 9.0),
        (100.0, 92.0),
        (1_000.0, 920.0),
        (1_000_000.0, 920_000.0),
    ];

    for (target, expected) in targets {
        let actual = usage_window_motion::run_up_from(target);
        assert!(
            actual >= 0.0,
            "negative run-up for target={target}: {actual}"
        );
        assert_close(actual, expected, 0.0, "run-up");
    }
}

#[test]
fn cubic_bezier_preserves_endpoints_and_outside_endpoint_behavior() {
    let curve = DEFAULT_MOTION_EASING;

    for input in [-1.0, 0.0, 1.0, 2.0] {
        assert_close(curve.sample(input), input, 0.0, "endpoint");
        assert_close(curve.evaluate(input), input, 0.0, "evaluate endpoint");
    }
}

#[test]
fn documented_curve_is_monotonic_at_numerical_sample_points() {
    let curve = DEFAULT_MOTION_EASING;
    let mut input = 0.0;
    let mut previous = curve.sample(input);

    for _ in 0..=100 {
        let current = curve.sample(input);
        assert!(
            current + CURVE_TOLERANCE >= previous,
            "curve decreased at x={input:.2}: previous={previous:.17e}, current={current:.17e}"
        );
        previous = current;
        input += 0.01;
    }

    assert_close(curve.sample(0.0), 0.0, CURVE_TOLERANCE, "zero");
    assert_close(curve.sample(1.0), 1.0, CURVE_TOLERANCE, "one");
}

#[test]
fn cubic_bezier_has_expected_intermediate_values_with_explicit_tolerance() {
    let curve = DEFAULT_MOTION_EASING;
    let expected = [
        (0.25, 0.764_864_719_058_868_1),
        (0.5, 0.961_382_547_804_317_8),
        (0.75, 0.996_894_217_325_146_2),
    ];

    for (input, expected) in expected {
        assert_close(
            curve.sample(input),
            expected,
            CURVE_TOLERANCE,
            "smooth-out sample",
        );
    }
}

#[test]
fn strict_parser_accepts_only_one_finite_css_curve_token() {
    let token = "  cubic-bezier(0.22, 1, 0.36, 1)  ";
    assert_eq!(parse_cubic_bezier_token(token), Some(DEFAULT_MOTION_EASING));
    assert_eq!(
        parse_cubic_bezier_token("cubic-bezier(.22, +1e0, .36, 1.)"),
        Some(DEFAULT_MOTION_EASING)
    );

    let invalid = [
        "",
        "linear",
        "cubic-bezier(0.22, 1, 0.36)",
        "cubic-bezier(0.22, 1, 0.36, 1, 0)",
        "cubic-bezier(0.22px, 1, 0.36, 1)",
        "cubic-bezier(-0.01, 1, 0.36, 1)",
        "cubic-bezier(0.22, 1, 1.01, 1)",
        "cubic-bezier(NaN, 1, 0.36, 1)",
        "cubic-bezier(0.22, 1, 0.36, 1) trailing",
        "prefix cubic-bezier(0.22, 1, 0.36, 1)",
    ];

    for token in invalid {
        assert_eq!(parse_cubic_bezier_token(token), None, "token={token:?}");
    }
}

#[test]
fn easing_input_uses_the_documented_fallback_for_absent_or_invalid_tokens() {
    assert_eq!(
        motion_easing(MotionEasingInput::new(None)),
        DEFAULT_MOTION_EASING
    );
    assert_eq!(
        motion_easing(MotionEasingInput::new(Some("not-a-curve"))),
        DEFAULT_MOTION_EASING
    );

    let custom = cubic_bezier(0.1, 0.2, 0.3, 0.4);
    assert_eq!(
        motion_easing(MotionEasingInput::new(Some(
            "cubic-bezier(0.1, 0.2, 0.3, 0.4)"
        ))),
        custom
    );
}

#[test]
fn flat_slope_uses_bounded_bisection_instead_of_returning_the_newton_guess() {
    let curve = CubicBezier::new(0.0, 0.0, -1.0, 1.0);

    // The first Newton guess is t=.5, where this deliberately degenerate
    // numerical curve has a zero x slope. Bisection then selects the bounded
    // increasing branch instead of returning that initial guess.
    assert_close(
        curve.sample(0.5),
        0.973_714_441_923_466_9,
        BISECTION_TOLERANCE,
        "flat slope",
    );
}

#[test]
fn duration_tokens_distinguish_milliseconds_from_seconds() {
    let durations = [
        (Some("250ms"), 250.0),
        (Some("0.25s"), 250.0),
        (Some("0.5"), 500.0),
        (Some(" 1.25ms "), 1.25),
        (Some("1.25s"), 1_250.0),
    ];

    for (token, expected) in durations {
        assert_close(
            duration(token, false),
            expected,
            CURVE_TOLERANCE,
            "duration",
        );
    }
}

#[test]
fn missing_empty_and_reduced_motion_duration_inputs_are_zero() {
    for (token, reduced_motion) in [
        (None, false),
        (Some(""), false),
        (Some("   "), false),
        (Some("250ms"), true),
        (None, true),
        (Some("not-a-duration"), true),
    ] {
        assert_close(duration(token, reduced_motion), 0.0, 0.0, "zero duration");
    }
}

#[test]
fn invalid_numeric_duration_uses_the_legacy_250_millisecond_fallback() {
    assert_close(
        INVALID_DURATION_MILLISECONDS,
        250.0,
        0.0,
        "invalid-duration fallback constant",
    );
    for token in ["not-a-duration", "ms", ".s"] {
        assert_close(
            duration(Some(token), false),
            INVALID_DURATION_MILLISECONDS,
            0.0,
            "invalid duration",
        );
    }
}
