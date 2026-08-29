//! Focused parity tests for the pure context auto-compaction policy.

#[path = "../../modules/frontend/src/context_auto_compaction.rs"]
mod context_auto_compaction;

use context_auto_compaction::{
    AutoCompactionModel, CLAUDE_SONNET_5_COMPACTION_TOKENS, ContextUsageAggregate,
    ContextUsageOrigin, UNKNOWN_COMPACTION_PERCENT, context_auto_compaction_percent,
    context_compaction_is_imminent, context_usage_auto_compaction_percent,
};

fn assert_approx_eq(got: f64, expected: f64, context: impl std::fmt::Display) {
    let tolerance = 16.0 * f64::EPSILON * got.abs().max(expected.abs()).max(1.0);
    assert!(
        (got - expected).abs() <= tolerance,
        "{context}: expected {expected:?}, got {got:?} (tolerance {tolerance:?})"
    );
}

fn model(
    harness: &'static str,
    native_model_id: Option<&'static str>,
    window_tokens: f64,
) -> AutoCompactionModel<'static> {
    AutoCompactionModel::new(harness, native_model_id, window_tokens)
}

fn usage(
    engine_id: Option<&'static str>,
    model_id: Option<&'static str>,
    context_tokens: Option<f64>,
) -> ContextUsageAggregate<'static> {
    ContextUsageAggregate::new(
        Some(ContextUsageOrigin::new(engine_id, model_id)),
        context_tokens,
    )
}

#[test]
fn documented_and_unknown_harness_thresholds_match_the_typescript_policy() {
    assert_approx_eq(
        context_auto_compaction_percent(model("codex", None, 1_000_000.0)),
        90.0,
        "Codex threshold",
    );
    assert_approx_eq(
        context_auto_compaction_percent(model("claude", None, 1_000_000.0)),
        100.0,
        "normal Claude threshold",
    );
    assert_approx_eq(
        context_auto_compaction_percent(model("unknown", Some("claude-sonnet-5"), 1_000_000.0)),
        UNKNOWN_COMPACTION_PERCENT,
        "unknown harness threshold",
    );
    assert_approx_eq(
        context_auto_compaction_percent(model("", None, 1_000_000.0)),
        UNKNOWN_COMPACTION_PERCENT,
        "missing harness threshold",
    );
}

#[test]
fn sonnet_uses_window_when_smaller_than_its_documented_capacity() {
    for window_tokens in [1.0, 200_000.0, 966_999.0, 967_000.0] {
        assert_approx_eq(
            context_auto_compaction_percent(model(
                "claude",
                Some("claude-sonnet-5"),
                window_tokens,
            )),
            100.0,
            format_args!("window={window_tokens}"),
        );
    }
}

#[test]
fn sonnet_uses_its_capacity_as_a_percentage_for_larger_windows() {
    let threshold =
        context_auto_compaction_percent(model("claude", Some("claude-sonnet-5"), 1_000_000.0));
    assert_approx_eq(threshold, 96.7, "Sonnet 1,000,000-token window");
    assert_approx_eq(
        context_auto_compaction_percent(model("claude", Some("claude-sonnet-5"), 2_000_000.0)),
        CLAUDE_SONNET_5_COMPACTION_TOKENS / 2_000_000.0 * 100.0,
        "Sonnet 2,000,000-token window",
    );
}

#[test]
fn threshold_equality_and_adjacent_values_are_inclusive() {
    let codex_window = 100.0;
    assert!(!context_compaction_is_imminent(
        Some(&usage(Some("codex"), None, Some(89.999))),
        Some(codex_window),
    ));
    assert!(context_compaction_is_imminent(
        Some(&usage(Some("codex"), None, Some(90.0))),
        Some(codex_window),
    ));
    assert!(context_compaction_is_imminent(
        Some(&usage(Some("codex"), None, Some(90.001))),
        Some(codex_window),
    ));

    let sonnet_window = 1_000_000.0;
    let sonnet_capacity = CLAUDE_SONNET_5_COMPACTION_TOKENS;
    assert!(!context_compaction_is_imminent(
        Some(&usage(
            Some("claude"),
            Some("claude-sonnet-5"),
            Some(sonnet_capacity - 1.0),
        )),
        Some(sonnet_window),
    ));
    assert!(context_compaction_is_imminent(
        Some(&usage(
            Some("claude"),
            Some("claude-sonnet-5"),
            Some(sonnet_capacity),
        )),
        Some(sonnet_window),
    ));
    assert!(context_compaction_is_imminent(
        Some(&usage(
            Some("claude"),
            Some("claude-sonnet-5"),
            Some(sonnet_capacity + 1.0),
        )),
        Some(sonnet_window),
    ));
}

#[test]
fn normal_claude_compacts_at_the_window_boundary() {
    assert!(!context_compaction_is_imminent(
        Some(&usage(Some("claude"), None, Some(99_999.0))),
        Some(100_000.0),
    ));
    assert!(context_compaction_is_imminent(
        Some(&usage(Some("claude"), None, Some(100_000.0))),
        Some(100_000.0),
    ));
    assert!(context_compaction_is_imminent(
        Some(&usage(Some("claude"), None, Some(100_001.0))),
        Some(100_000.0),
    ));
}

#[test]
fn threshold_resolution_uses_reporting_origin_not_a_current_policy() {
    let codex_usage = usage(Some("codex"), Some("gpt-5.6-sol"), Some(900_000.0));
    let sonnet_usage = usage(Some("claude"), Some("claude-sonnet-5"), Some(967_000.0));

    assert_approx_eq(
        context_usage_auto_compaction_percent(Some(&codex_usage), 1_000_000.0),
        90.0,
        "reporting Codex origin",
    );
    assert_approx_eq(
        context_usage_auto_compaction_percent(Some(&sonnet_usage), 1_000_000.0),
        96.7,
        "reporting Sonnet origin",
    );
    assert!(context_compaction_is_imminent(
        Some(&codex_usage),
        Some(1_000_000.0),
    ));
    assert!(context_compaction_is_imminent(
        Some(&sonnet_usage),
        Some(1_000_000.0),
    ));
}

#[test]
fn missing_or_incomplete_origin_is_the_unknown_engine_case() {
    let no_origin = ContextUsageAggregate::new(None, Some(100.0));
    let missing_engine = usage(None, Some("claude-sonnet-5"), Some(100.0));
    let missing_model = usage(Some("claude"), None, Some(100.0));

    for aggregate in [&no_origin, &missing_engine, &missing_model] {
        assert_approx_eq(
            context_usage_auto_compaction_percent(Some(aggregate), 100.0),
            UNKNOWN_COMPACTION_PERCENT,
            format_args!("incomplete reporting origin: {aggregate:?}"),
        );
    }
}

#[test]
fn non_positive_and_non_finite_windows_return_the_conservative_boundary() {
    for window_tokens in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for harness in ["codex", "claude", "unknown"] {
            assert_approx_eq(
                context_auto_compaction_percent(model(harness, None, window_tokens)),
                UNKNOWN_COMPACTION_PERCENT,
                format_args!("harness={harness} window={window_tokens}"),
            );
        }
    }
}

#[test]
fn imminence_is_false_without_values_or_for_invalid_numbers() {
    let no_usage = None;
    assert!(!context_compaction_is_imminent(no_usage, Some(100.0)));
    assert!(!context_compaction_is_imminent(
        Some(&usage(Some("codex"), None, None)),
        Some(100.0),
    ));
    assert!(!context_compaction_is_imminent(
        Some(&usage(Some("codex"), None, Some(100.0))),
        None,
    ));

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(!context_compaction_is_imminent(
            Some(&usage(Some("codex"), None, Some(bad))),
            Some(100.0),
        ));
        assert!(!context_compaction_is_imminent(
            Some(&usage(Some("codex"), None, Some(90.0))),
            Some(bad),
        ));
    }

    for invalid_window in [0.0, -1.0] {
        assert!(!context_compaction_is_imminent(
            Some(&usage(Some("codex"), None, Some(100.0))),
            Some(invalid_window),
        ));
    }
}

#[test]
fn non_finite_ratio_from_extreme_finite_inputs_is_not_imminent() {
    // The inputs are finite, but division overflows. The explicit ratio guard
    // keeps this malformed reading deterministic rather than comparing an
    // intermediate infinity.
    assert!(!context_compaction_is_imminent(
        Some(&usage(Some("codex"), None, Some(f64::MAX))),
        Some(f64::MIN_POSITIVE),
    ));
}

#[test]
fn large_token_counts_remain_finite_and_compare_without_panicking() {
    let window = 1_000_000_000_000_000.0;
    let aggregate = usage(Some("codex"), None, Some(900_000_000_000_000.0));
    assert!(context_compaction_is_imminent(
        Some(&aggregate),
        Some(window)
    ));

    let maximum_window = f64::MAX;
    let maximum_usage = usage(Some("claude"), None, Some(f64::MAX));
    assert!(context_compaction_is_imminent(
        Some(&maximum_usage),
        Some(maximum_window),
    ));
}
