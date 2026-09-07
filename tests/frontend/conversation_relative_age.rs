//! Exhaustive dependency-free parity coverage for conversation relative age.
//!
//! The test target includes the production file directly so it can run with
//! `rustc --test` without module registration, Cargo metadata, or UI/runtime
//! dependencies.

#[path = "../../modules/frontend/src/conversation_relative_age.rs"]
mod conversation_relative_age;

use conversation_relative_age::format_relative_age;

const REFERENCE_TIMESTAMP: &str = "2024-01-10T12:00:00.000Z";
const REFERENCE_MILLIS: i64 = 1_704_888_000_000;
const MARCH_FIRST_MILLIS: i64 = 1_709_251_200_000;
const MILLIS_PER_DAY: i64 = 86_400_000;

fn assert_age(now_millis: i64, timestamp: &str, expected: &str) {
    assert_eq!(
        format_relative_age(now_millis, timestamp),
        expected,
        "now={now_millis} timestamp={timestamp:?}"
    );
}

fn assert_age_after(elapsed_seconds: u64, expected: &str) {
    let elapsed_millis = i64::try_from(elapsed_seconds)
        .expect("table value fits in i64 seconds")
        .checked_mul(1_000)
        .expect("table value fits in i64 milliseconds");
    assert_age(
        REFERENCE_MILLIS + elapsed_millis,
        REFERENCE_TIMESTAMP,
        expected,
    );
}

#[test]
fn seconds_boundary_table_has_exact_strings() {
    let cases = [
        (0, "0s ago"),
        (1, "1s ago"),
        (59, "59s ago"),
        (60, "1m ago"),
    ];

    for (elapsed_seconds, expected) in cases {
        assert_age_after(elapsed_seconds, expected);
    }
}

#[test]
fn minutes_boundary_table_has_exact_strings() {
    let cases = [
        (59 * 60 - 1, "58m ago"),
        (59 * 60, "59m ago"),
        (60 * 60 - 1, "59m ago"),
        (60 * 60, "1h ago"),
    ];

    for (elapsed_seconds, expected) in cases {
        assert_age_after(elapsed_seconds, expected);
    }
}

#[test]
fn hours_boundary_table_has_exact_strings() {
    let cases = [
        (23 * 60 * 60 - 1, "22h ago"),
        (23 * 60 * 60, "23h ago"),
        (24 * 60 * 60 - 1, "23h ago"),
        (24 * 60 * 60, "1d ago"),
    ];

    for (elapsed_seconds, expected) in cases {
        assert_age_after(elapsed_seconds, expected);
    }
}

#[test]
fn days_boundary_table_has_exact_strings() {
    let cases = [
        (6 * 86_400 - 1, "5d ago"),
        (6 * 86_400, "6d ago"),
        (7 * 86_400 - 1, "6d ago"),
        (7 * 86_400, "1w ago"),
    ];

    for (elapsed_seconds, expected) in cases {
        assert_age_after(elapsed_seconds, expected);
    }
}

#[test]
fn weeks_boundary_table_has_exact_strings() {
    let cases = [
        (4 * 7 * 86_400 - 1, "3w ago"),
        (4 * 7 * 86_400, "4w ago"),
        (5 * 7 * 86_400 - 1, "4w ago"),
        (5 * 7 * 86_400, "1mo ago"),
    ];

    for (elapsed_seconds, expected) in cases {
        assert_age_after(elapsed_seconds, expected);
    }
}

#[test]
fn month_and_year_boundaries_preserve_the_source_precedence() {
    let cases = [
        (30 * MILLIS_PER_DAY, "4w ago"),
        (35 * MILLIS_PER_DAY, "1mo ago"),
        (11 * 30 * MILLIS_PER_DAY, "11mo ago"),
        (12 * 30 * MILLIS_PER_DAY - 1_000, "11mo ago"),
        (12 * 30 * MILLIS_PER_DAY, "0y ago"),
        (364 * MILLIS_PER_DAY, "0y ago"),
        (365 * MILLIS_PER_DAY - 1_000, "0y ago"),
        (365 * MILLIS_PER_DAY, "1y ago"),
    ];

    for (elapsed_millis, expected) in cases {
        assert_age(
            REFERENCE_MILLIS + elapsed_millis,
            REFERENCE_TIMESTAMP,
            expected,
        );
    }
}

#[test]
fn exact_unit_strings_do_not_pluralize_or_change_suffixes() {
    let cases = [
        (5, "5s ago"),
        (5 * 60, "5m ago"),
        (5 * 3_600, "5h ago"),
        (5 * 86_400, "5d ago"),
        (2 * 7 * 86_400, "2w ago"),
        (2 * 30 * 86_400, "2mo ago"),
        (2 * 365 * 86_400, "2y ago"),
    ];

    for (elapsed_seconds, expected) in cases {
        assert_age_after(elapsed_seconds, expected);
    }
}

#[test]
fn invalid_and_future_timestamps_fall_back_to_zero_seconds() {
    for timestamp in [
        "",
        "not-a-timestamp",
        "2024-13-01T00:00:00.000Z",
        "2024-02-30T00:00:00.000Z",
        "2024-01-10T12:00:00.000",
    ] {
        assert_age(REFERENCE_MILLIS, timestamp, "0s ago");
    }

    assert_age(REFERENCE_MILLIS, "2024-01-10T12:00:01.000Z", "0s ago");
    assert_age(REFERENCE_MILLIS, "2099-12-31T23:59:59.999Z", "0s ago");
}

#[test]
fn fractional_seconds_are_padded_truncated_and_floored() {
    assert_age(
        REFERENCE_MILLIS + 1_000,
        "2024-01-10T12:00:00.001Z",
        "0s ago",
    );
    assert_age(
        REFERENCE_MILLIS + 1_001,
        "2024-01-10T12:00:00.001Z",
        "1s ago",
    );
    assert_age(
        REFERENCE_MILLIS + 30_000,
        "2024-01-10T12:00:00.9999Z",
        "29s ago",
    );
    assert_age(REFERENCE_MILLIS + 1_000, "2024-01-10T12:00:00.1Z", "0s ago");
    assert_age(
        REFERENCE_MILLIS + 1_001,
        "2024-01-10T12:00:00.12Z",
        "0s ago",
    );
}

#[test]
fn leap_day_and_date_only_forms_use_utc_calendar_arithmetic() {
    assert_age(MARCH_FIRST_MILLIS, "2024-02-29T00:00:00.000Z", "1d ago");
    assert_age(MARCH_FIRST_MILLIS, "2024-02-29T23:59:59.999Z", "0s ago");
    assert_age(REFERENCE_MILLIS, "2024-01-10", "12h ago");
    assert_age(REFERENCE_MILLIS, "2024-01-09", "1d ago");
}

#[test]
fn numeric_offsets_describe_the_same_instant() {
    assert_age(REFERENCE_MILLIS, "2024-01-10T14:00:00.000+02:00", "0s ago");
    assert_age(REFERENCE_MILLIS, "2024-01-10T14:00:00.000+0200", "0s ago");
    assert_age(REFERENCE_MILLIS, "2024-01-10T07:00:00.000-05:00", "0s ago");
    assert_age(REFERENCE_MILLIS, "2024-01-10T12:00:00.000+00:00", "0s ago");
    assert_age(REFERENCE_MILLIS, "2024-01-10T12:00:00.000+02:00", "2h ago");
}

#[test]
fn explicit_observation_time_keeps_the_policy_deterministic() {
    let timestamp = "2024-01-10T11:59:00.000Z";

    assert_eq!(format_relative_age(REFERENCE_MILLIS, timestamp), "1m ago");
    assert_eq!(
        format_relative_age(REFERENCE_MILLIS + 3_600_000, timestamp),
        "1h ago"
    );
    assert_eq!(
        format_relative_age(REFERENCE_MILLIS, timestamp),
        format_relative_age(REFERENCE_MILLIS, timestamp)
    );
}
