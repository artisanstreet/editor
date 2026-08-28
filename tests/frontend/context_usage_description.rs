//! Focused parity tests for the pure context-usage description port.

#[path = "../../modules/frontend/src/context_usage_description.rs"]
mod context_usage_description;

use context_usage_description::{
    ContextUsageAggregate, context_usage_description, format_token_count,
};

fn usage(
    context_tokens: Option<u64>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
) -> ContextUsageAggregate {
    ContextUsageAggregate {
        context_tokens,
        input_tokens,
        cached_input_tokens,
        output_tokens,
    }
}

#[test]
fn no_breakdown_uses_the_exact_sentence() {
    let aggregate = ContextUsageAggregate::default();

    assert_eq!(
        context_usage_description(&aggregate, 1_000_000),
        "Context window contains 0 of 1,000,000 tokens. No detailed token breakdown is available."
    );
}

#[test]
fn each_optional_breakdown_field_is_emitted_independently() {
    let cases = [
        (
            usage(None, Some(1_234), None, None),
            "Context window contains 0 of 10,000 tokens. Input: 1,234 tokens.",
        ),
        (
            usage(None, None, Some(5_678), None),
            "Context window contains 0 of 10,000 tokens. Cached input: 5,678 tokens.",
        ),
        (
            usage(None, None, None, Some(9_012)),
            "Context window contains 0 of 10,000 tokens. Output: 9,012 tokens.",
        ),
    ];

    for (aggregate, expected) in cases {
        assert_eq!(context_usage_description(&aggregate, 10_000), expected);
    }
}

#[test]
fn all_fields_keep_the_typescript_order() {
    let aggregate = usage(Some(1_234_567), Some(12), Some(3_456), Some(78_901));

    assert_eq!(
        context_usage_description(&aggregate, 9_876_543),
        "Context window contains 1,234,567 of 9,876,543 tokens. Input: 12 tokens. Cached input: 3,456 tokens. Output: 78,901 tokens."
    );
}

#[test]
fn zero_is_a_reported_value_not_an_omitted_breakdown() {
    let aggregate = usage(Some(0), Some(0), Some(0), Some(0));

    assert_eq!(
        context_usage_description(&aggregate, 0),
        "Context window contains 0 of 0 tokens. Input: 0 tokens. Cached input: 0 tokens. Output: 0 tokens."
    );
}

#[test]
fn grouping_matches_english_thousands_boundaries() {
    let cases = [
        (0, "0"),
        (1, "1"),
        (999, "999"),
        (1_000, "1,000"),
        (1_001, "1,001"),
        (9_999, "9,999"),
        (10_000, "10,000"),
        (999_999, "999,999"),
        (1_000_000, "1,000,000"),
        (1_000_001, "1,000,001"),
        (1_234_567_890, "1,234,567,890"),
    ];

    for (value, expected) in cases {
        assert_eq!(format_token_count(value), expected, "value={value}");
    }
}

#[test]
fn large_u64_values_remain_exact_and_grouped() {
    let maximum = u64::MAX;
    assert_eq!(format_token_count(maximum), "18,446,744,073,709,551,615");

    let aggregate = usage(Some(maximum), None, None, Some(maximum));
    assert_eq!(
        context_usage_description(&aggregate, maximum),
        "Context window contains 18,446,744,073,709,551,615 of 18,446,744,073,709,551,615 tokens. Output: 18,446,744,073,709,551,615 tokens."
    );
}

#[test]
fn missing_context_tokens_default_to_zero() {
    let aggregate = usage(None, Some(7), None, None);

    assert_eq!(
        context_usage_description(&aggregate, 128_000),
        "Context window contains 0 of 128,000 tokens. Input: 7 tokens."
    );
}
