//! Exhaustive coverage for the conversation duration policy
//! ported from `modules/frontend/src/lib/conversation/duration.ts`.
//!
//! The TypeScript contract floors `elapsed_ms / 1_000`, clamps negatives and
//! non-finites to zero, always shows seconds, shows minutes when either
//! minutes or hours are non-zero, and shows hours when non-zero. `FormatDuration`
//! differences two ISO timestamps via `Date.parse` before delegating.
//! These tests preserve those thresholds, rounding, invalid/negative/large
//! handling, output text, and parity examples table-driven and deterministically.

use artisan_frontend::conversation_duration::{format_duration, format_elapsed};

// ---------------------------------------------------------------------------
// format_elapsed — boundaries and rounding (floor after /1000)
// ---------------------------------------------------------------------------

#[test]
fn format_elapsed_boundaries_and_flooring() {
    let cases: &[(&str, f64, &str)] = &[
        ("zero", 0.0, "0s"),
        ("sub_ms", 0.5, "0s"),
        ("one_ms", 1.0, "0s"),
        ("500ms", 500.0, "0s"),
        ("999ms", 999.0, "0s"),
        ("999.9ms", 999.9, "0s"),
        ("1000ms_exact", 1_000.0, "1s"),
        ("1000.1ms", 1_000.1, "1s"),
        ("1000.9ms", 1_000.9, "1s"),
        ("1500ms", 1_500.0, "1s"),
        ("1999ms", 1_999.0, "1s"),
        ("2000ms", 2_000.0, "2s"),
        ("59s_exact", 59_000.0, "59s"),
        ("59_999ms", 59_999.0, "59s"),
        ("60s_exact", 60_000.0, "1m 0s"),
        ("60_001ms", 60_001.0, "1m 0s"),
        ("61s", 61_000.0, "1m 1s"),
        ("61_999ms", 61_999.0, "1m 1s"),
        ("119_999ms", 119_999.0, "1m 59s"),
        ("120s", 120_000.0, "2m 0s"),
        ("59m59s", 3_599_000.0, "59m 59s"),
        ("59m59.999s", 3_599_999.0, "59m 59s"),
        ("60m_exact", 3_600_000.0, "1h 0m 0s"),
        ("60m_plus_1s", 3_601_000.0, "1h 0m 1s"),
        ("parity_1h0m3s", 3_603_000.0, "1h 0m 3s"),
        ("1h1m1s", 3_661_000.0, "1h 1m 1s"),
        ("1h1m59s", 3_719_000.0, "1h 1m 59s"),
        ("1h59m59s", 7_199_000.0, "1h 59m 59s"),
        ("2h_exact", 7_200_000.0, "2h 0m 0s"),
        ("24h", 86_400_000.0, "24h 0m 0s"),
        ("fractional_seconds_floor", 1_500.9, "1s"),
        ("fractional_hour_floor", 3_600_999.9, "1h 0m 0s"),
    ];
    for (name, input, expected) in cases {
        assert_eq!(
            format_elapsed(*input),
            *expected,
            "case {name}: {input} -> {expected}"
        );
    }
}

#[test]
fn format_elapsed_shows_minutes_only_when_hours_present_or_minutes_nonzero() {
    // Seconds always present.
    assert_eq!(format_elapsed(0.0), "0s");
    assert_eq!(format_elapsed(1_000.0), "1s");
    // Minutes appear when >0, even without hours.
    assert_eq!(format_elapsed(60_000.0), "1m 0s");
    assert_eq!(format_elapsed(3_599_000.0), "59m 59s");
    // Hours force minutes even when minutes == 0.
    assert_eq!(format_elapsed(3_600_000.0), "1h 0m 0s");
    assert_eq!(format_elapsed(3_603_000.0), "1h 0m 3s");
    assert_eq!(format_elapsed(7_200_000.0), "2h 0m 0s");
    // No spurious hours/minutes.
    assert_eq!(format_elapsed(59_000.0), "59s");
    assert!(!format_elapsed(59_000.0).contains('h'));
    assert!(!format_elapsed(59_000.0).contains('m'));
    assert!(!format_elapsed(60_000.0).contains('h'));
}

#[test]
fn format_elapsed_negative_and_non_finite_floor_to_zero() {
    let cases: &[(&str, f64)] = &[
        ("negative_one", -1.0),
        ("negative_500", -500.0),
        ("negative_1000000", -1_000_000.0),
        ("negative_infinity", f64::NEG_INFINITY),
        ("positive_infinity", f64::INFINITY),
        ("nan", f64::NAN),
        ("negative_zero", -0.0),
    ];
    for (name, input) in cases {
        let out = format_elapsed(*input);
        assert_eq!(
            out, "0s",
            "case {name}: {input:?} must floor to 0s, got {out}"
        );
    }
}

#[test]
fn format_elapsed_large_values_remain_deterministic() {
    // Values far beyond a typical conversation but still finite must not panic
    // and must follow the same hours/minutes/seconds decomposition.
    let cases: &[(&str, f64, &str)] = &[
        // 100_000_000 seconds = 27_777h 46m 40s
        ("100M_seconds", 100_000_000_000.0, "27777h 46m 40s"),
        // 1_000_000 seconds = 277h 46m 40s
        ("1M_seconds", 1_000_000_000.0, "277h 46m 40s"),
        // Exactly 1 year of millis-ish (365 days) -> 8760h
        ("365_days", 31_536_000_000.0, "8760h 0m 0s"),
        // 9_007_199_254_740 seconds = 2_501_999_792h 59m 0s
        ("huge_finite", 9_007_199_254_740_000.0, "2501999792h 59m 0s"),
    ];
    for (name, input, expected) in cases {
        let out = format_elapsed(*input);
        assert_eq!(
            out, *expected,
            "case {name}: {input} -> {expected}, got {out}"
        );
    }

    // Extremely large finite values must not panic and must remain deterministic.
    // They clamp to u64::MAX inside the implementation, so the same input
    // always yields the same string and the output still ends with 's'.
    let huge_a = format_elapsed(f64::MAX);
    let huge_b = format_elapsed(f64::MAX);
    assert_eq!(huge_a, huge_b);
    assert!(huge_a.ends_with('s'));
    let huge_c = format_elapsed(1e308);
    assert!(huge_c.ends_with('s'));
    assert_eq!(huge_c, format_elapsed(1e308));
}

#[test]
fn format_elapsed_singular_plural_text_is_unit_stable() {
    // Units are always h/m/s, never long plural words. Singular and plural
    // share the same suffix; the numeric prefix carries count.
    assert_eq!(format_elapsed(0.0), "0s");
    assert_eq!(format_elapsed(1_000.0), "1s");
    assert_eq!(format_elapsed(2_000.0), "2s");
    assert_eq!(format_elapsed(60_000.0), "1m 0s");
    assert_eq!(format_elapsed(120_000.0), "2m 0s");
    assert_eq!(format_elapsed(3_600_000.0), "1h 0m 0s");
    assert_eq!(format_elapsed(7_200_000.0), "2h 0m 0s");
    // Seconds suffix always s, minutes m, hours h
    for out in [
        format_elapsed(0.0),
        format_elapsed(1_000.0),
        format_elapsed(60_000.0),
        format_elapsed(3_600_000.0),
    ] {
        assert!(out.ends_with('s'), "all outputs end with s: {out}");
    }
}

// ---------------------------------------------------------------------------
// format_duration — ISO timestamp differencing
// ---------------------------------------------------------------------------

#[test]
fn format_duration_parity_examples() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "same_instant",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T00:00:00.000Z",
            "0s",
        ),
        (
            "one_second",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T00:00:01.000Z",
            "1s",
        ),
        (
            "one_second_fractional_truncated",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T00:00:01.500Z",
            "1s",
        ),
        (
            "one_minute",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T00:01:00.000Z",
            "1m 0s",
        ),
        (
            "one_minute_one_second",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T00:01:01.000Z",
            "1m 1s",
        ),
        (
            "one_hour_exact",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T01:00:00.000Z",
            "1h 0m 0s",
        ),
        (
            "parity_1h0m3s",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T01:00:03.000Z",
            "1h 0m 3s",
        ),
        (
            "1h1m1s",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T01:01:01.000Z",
            "1h 1m 1s",
        ),
        (
            "cross_day_24h",
            "2024-01-01T00:00:00.000Z",
            "2024-01-02T00:00:00.000Z",
            "24h 0m 0s",
        ),
        (
            "cross_day_48h",
            "2024-01-01T00:00:00.000Z",
            "2024-01-03T00:00:00.000Z",
            "48h 0m 0s",
        ),
        ("date_only", "2024-01-01", "2024-01-02", "24h 0m 0s"),
        (
            "with_millis",
            "2024-01-01T00:00:00.123Z",
            "2024-01-01T00:00:01.123Z",
            "1s",
        ),
        (
            "fraction_beyond_millis_truncated",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T00:00:01.9999Z",
            "1s",
        ),
    ];
    for (name, started, ended, expected) in cases {
        assert_eq!(
            format_duration(started, ended),
            *expected,
            "case {name}: {started} -> {ended}"
        );
    }
}

#[test]
fn format_duration_invalid_and_negative_inputs_floor_to_zero() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("both_invalid", "not-a-date", "also-bad", "0s"),
        (
            "started_invalid",
            "invalid",
            "2024-01-01T00:00:01.000Z",
            "0s",
        ),
        ("ended_invalid", "2024-01-01T00:00:00.000Z", "bad", "0s"),
        ("empty_both", "", "", "0s"),
        ("empty_started", "", "2024-01-01T00:00:01.000Z", "0s"),
        ("empty_ended", "2024-01-01T00:00:00.000Z", "", "0s"),
        (
            "malformed_month",
            "2024-13-01T00:00:00.000Z",
            "2024-01-01T00:00:01.000Z",
            "0s",
        ),
        (
            "malformed_day",
            "2024-01-32T00:00:00.000Z",
            "2024-01-01T00:00:01.000Z",
            "0s",
        ),
        (
            "reversed_negative",
            "2024-01-01T00:01:00.000Z",
            "2024-01-01T00:00:00.000Z",
            "0s",
        ),
        (
            "reversed_one_ms",
            "2024-01-01T00:00:01.000Z",
            "2024-01-01T00:00:00.999Z",
            "0s",
        ),
        (
            "garbage_with_spaces",
            "   ",
            "2024-01-01T00:00:01.000Z",
            "0s",
        ),
        (
            "wrong_separator",
            "2024/01/01T00:00:00.000Z",
            "2024-01-01T00:00:01.000Z",
            "0s",
        ),
    ];
    for (name, started, ended, expected) in cases {
        assert_eq!(
            format_duration(started, ended),
            *expected,
            "case {name}: started={started:?} ended={ended:?}"
        );
    }
}

#[test]
fn format_duration_timezone_offsets() {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "same_instant_z_vs_plus02",
            "2024-01-01T00:00:00.000Z",
            "2024-01-01T02:00:00.000+02:00",
            "0s",
        ),
        (
            "plus01_one_hour",
            "2024-01-01T00:00:00.000+01:00",
            "2024-01-01T00:00:00.000Z",
            "1h 0m 0s",
        ),
        (
            "minus02_one_hour",
            "2024-01-01T00:00:00.000-02:00",
            "2024-01-01T00:00:00.000Z",
            // -02:00 means local is 2h behind UTC, so 00:00-02:00 = 02:00Z, diff to 00:00Z is -2h => floors 0s
            "0s",
        ),
        (
            "plus00_same_as_z",
            "2024-01-01T00:00:00.000+00:00",
            "2024-01-01T00:00:01.000Z",
            "1s",
        ),
        (
            "offset_without_colon",
            "2024-01-01T00:00:00.000+0200",
            "2024-01-01T00:00:00.000Z",
            // V8 parses +HHMM as +HH:MM (Date.parse is 2h earlier), so +2h.
            "2h 0m 0s",
        ),
        (
            "offset_hour_only",
            "2024-01-01T02:00:00+02",
            "2024-01-01T00:00:00.000Z",
            "0s",
        ),
    ];
    for (name, started, ended, expected) in cases {
        assert_eq!(format_duration(started, ended), *expected, "case {name}");
    }
}

#[test]
fn format_duration_large_span_remains_deterministic() {
    // Multi-day spans use the same hours decomposition (no day unit).
    assert_eq!(
        format_duration("2024-01-01T00:00:00.000Z", "2024-01-10T00:00:00.000Z"),
        "216h 0m 0s"
    );
    assert_eq!(
        format_duration("2024-01-01T00:00:00.000Z", "2024-02-01T00:00:00.000Z"),
        "744h 0m 0s"
    );
}

#[test]
fn format_elapsed_and_format_duration_agree_on_same_millis() {
    // Parity: format_duration of a known diff must equal format_elapsed of that diff.
    let diff_ms: f64 = 3_603_000.0;
    let elapsed = format_elapsed(diff_ms);
    let via_duration = format_duration("2024-01-01T00:00:00.000Z", "2024-01-01T01:00:03.000Z");
    assert_eq!(elapsed, via_duration);
    assert_eq!(elapsed, "1h 0m 3s");

    // Sub-second diff via timestamps floors to 0s, same as elapsed.
    assert_eq!(
        format_duration("2024-01-01T00:00:00.000Z", "2024-01-01T00:00:00.500Z"),
        format_elapsed(500.0)
    );
    assert_eq!(format_elapsed(500.0), "0s");
}
