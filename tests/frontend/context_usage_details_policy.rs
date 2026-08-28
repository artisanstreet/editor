//! Direct parity tests for the dependency-free context-details policy.

#![allow(dead_code)]

#[path = "../../modules/frontend/src/context_usage_details_policy.rs"]
mod context_usage_details_policy;

use context_usage_details_policy::{
    context_usage_details, format_compact_tokens, ContextUsageDetails, ContextUsageDetailsError,
    FALLBACK_MODEL_NAME,
};

#[test]
fn fallback_and_provided_model_names_feed_both_distinct_prose_facts() {
    let fallback = ContextUsageDetails::new(None, 42.0, 258_400.0).expect("finite input");
    assert_eq!(fallback.model_name(), FALLBACK_MODEL_NAME);
    assert_eq!(
        fallback.percent_prose.sentence(),
        "The context window for this model is 42% full."
    );
    assert_eq!(
        fallback.percent_prose.accessible_label(),
        "Context window 42% full"
    );
    assert_eq!(
        fallback.model_capacity_prose.sentence(),
        "this model has a context window of 258K tokens."
    );
    assert_eq!(fallback.percent().model_name(), FALLBACK_MODEL_NAME);
    assert_eq!(fallback.capacity().model_name(), FALLBACK_MODEL_NAME);

    let provided =
        context_usage_details(Some("GPT-5.6 Luna"), 42.0, 258_400.0).expect("finite input");
    assert_eq!(provided.model_name(), "GPT-5.6 Luna");
    assert_eq!(
        provided.percent_prose.sentence(),
        "The context window for GPT-5.6 Luna is 42% full."
    );
    assert_eq!(
        provided.model_capacity_prose.sentence(),
        "GPT-5.6 Luna has a context window of 258K tokens."
    );

    // Nullish fallback is not an empty-string fallback.
    let empty = ContextUsageDetails::new(Some(""), 0.0, 0.0).expect("finite input");
    assert_eq!(empty.model_name(), "");
    assert_eq!(
        empty.model_capacity_prose.sentence(),
        " has a context window of 0 tokens."
    );
}

#[test]
fn percent_prose_rounds_the_raw_value_but_fill_clamps_the_raw_value() {
    let cases = [
        (49.4, "49", 49.4),
        (49.5, "50", 49.5),
        (-10.4, "-10", 0.0),
        (-10.5, "-10", 0.0),
        (-10.6, "-11", 0.0),
        (100.4, "100", 100.0),
        (100.5, "101", 100.0),
        (120.6, "121", 100.0),
    ];

    for (percent, rounded, fill) in cases {
        let details = ContextUsageDetails::new(None, percent, 1_000.0).expect("finite input");
        assert_eq!(
            details.percent_prose.rounded_percent(),
            rounded.parse::<f64>().expect("integer text"),
            "rounded percent for {percent}"
        );
        assert_eq!(
            details.percent_prose.accessible_label(),
            format!("Context window {rounded}% full"),
            "accessible label for {percent}"
        );
        assert_eq!(
            details.progress_fill.value(),
            fill,
            "progress fill for {percent}"
        );
        assert_eq!(details.progress_fill.max(), 100.0);
    }
}

#[test]
fn compact_tokens_keep_whole_english_units_and_promote_boundaries() {
    let cases = [
        (0.0, "0"),
        (999.0, "999"),
        (999.4, "999"),
        (999.5, "1K"),
        (999.499_999_999_999_9, "999"),
        (1_000.0, "1K"),
        (1_499.0, "1K"),
        (1_500.0, "2K"),
        (258_400.4, "258K"),
        (999_499.0, "999K"),
        (999_499.5, "999K"),
        (999_499.999_999_999_9, "999K"),
        (999_500.0, "1M"),
        (999_999.0, "1M"),
        (1_000_000.0, "1M"),
        (1_499_999.0, "1M"),
        (1_500_000.0, "2M"),
        (999_499_999.0, "999M"),
        (999_500_000.0, "1B"),
    ];

    for (tokens, expected) in cases {
        assert_eq!(
            format_compact_tokens(tokens).expect("finite input"),
            expected,
            "compact capacity for {tokens}"
        );
    }
}

#[test]
fn compact_tokens_handle_negative_and_fractional_values_without_decimal_noise() {
    let cases = [
        (-0.1, "-0"),
        (-0.5, "-1"),
        (-999.4, "-999"),
        (-999.5, "-1K"),
        (-999_500.0, "-1M"),
        (10_500.0, "11K"),
    ];

    for (tokens, expected) in cases {
        let formatted = format_compact_tokens(tokens).expect("finite input");
        assert_eq!(formatted, expected, "compact capacity for {tokens}");
        assert!(!formatted.contains('.'));
    }
}

#[test]
fn output_is_totals_only_and_retains_no_prompt_or_breakdown_fields() {
    let details = ContextUsageDetails::new(Some("Model"), 75.25, 200_000.0).expect("finite input");

    assert_eq!(
        details.percent_prose.text(),
        "The context window for Model is 75% full."
    );
    assert_eq!(
        details.model_capacity_prose.text(),
        "Model has a context window of 200K tokens."
    );
    assert!(!details.percent_prose.text().contains("Input"));
    assert!(!details.percent_prose.text().contains("prompt"));
    assert!(!details.model_capacity_prose.text().contains("category"));
}

#[test]
fn non_finite_inputs_are_rejected_at_the_projection_boundary() {
    assert_eq!(
        ContextUsageDetails::new(None, f64::NAN, 10_000.0),
        Err(ContextUsageDetailsError::NonFinitePercent)
    );
    assert_eq!(
        ContextUsageDetails::new(None, 10.0, f64::INFINITY),
        Err(ContextUsageDetailsError::NonFiniteWindowTokens)
    );
    assert_eq!(
        format_compact_tokens(f64::NEG_INFINITY),
        Err(ContextUsageDetailsError::NonFiniteWindowTokens)
    );
}
