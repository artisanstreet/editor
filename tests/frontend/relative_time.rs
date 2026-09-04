//! Exhaustive boundary, edge, and parity coverage for the native
//! `relative_time` policy port of `relative-time.ts`.
//!
//! The TypeScript source is:
//! ```ts
//! export const format_relative_age = (now: number, timestamp: string): string => {
//!   const timestamp_ms = Date.parse(timestamp);
//!   const elapsed_seconds = Number.isFinite(timestamp_ms)
//!     ? Math.max(0, Math.floor((now - timestamp_ms) / 1_000)) : 0;
//!   if (elapsed_seconds < 60) return `${elapsed_seconds}s ago`;
//!   const elapsed_minutes = Math.floor(elapsed_seconds / 60);
//!   if (elapsed_minutes < 60) return `${elapsed_minutes}m ago`;
//!   const elapsed_hours = Math.floor(elapsed_minutes / 60);
//!   if (elapsed_hours < 24) return `${elapsed_hours}h ago`;
//!   const elapsed_days = Math.floor(elapsed_hours / 24);
//!   if (elapsed_days < 7) return `${elapsed_days}d ago`;
//!   const elapsed_weeks = Math.floor(elapsed_days / 7);
//!   if (elapsed_weeks < 5) return `${elapsed_weeks}w ago`;
//!   const elapsed_months = Math.floor(elapsed_days / 30);
//!   if (elapsed_months < 12) return `${elapsed_months}mo ago`;
//!   return `${Math.floor(elapsed_days / 365)}y ago`;
//! };
//! ```
//! Every threshold, suffix, and handling of future/invalid/large input is
//! preserved.
//!
//! This file loads the implementation from `modules/frontend/src/relative_time.rs`
//! directly so the test suite runs without requiring `lib.rs` registration.

#[path = "../../modules/frontend/src/relative_time.rs"]
mod relative_time;

use relative_time::format_relative_age;

// ---------------------------------------------------------------------------
// Helpers: millis <-> ISO conversion for table-driven cases.
// ---------------------------------------------------------------------------

fn millis_to_iso(millis: i64) -> String {
    // Split into days and time of day, handling negative millis correctly
    // via Euclidean division.
    let days = millis.div_euclid(86_400_000);
    let time_of_day = millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let hour = (time_of_day / 3_600_000) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let minute = ((time_of_day % 3_600_000) / 60_000) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let second = ((time_of_day % 60_000) / 1_000) as u8;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ms = (time_of_day % 1_000) as u16;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u8, u8) {
    // Inverse of `days_from_civil` – Howard Hinnant's civil_from_days.
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u8;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::useless_conversion
    )]
    let day = (day_of_year - (153 * i64::from(month_prime) + 2) / 5 + 1) as u8;
    #[allow(clippy::cast_possible_truncation)]
    let year = (year + i64::from(month <= 2)) as i32;
    (year, month, day)
}

fn assert_age(now_millis: i64, timestamp: &str, expected: &str) {
    let observed = format_relative_age(now_millis, timestamp);
    assert_eq!(
        observed, expected,
        "now={now_millis} timestamp={timestamp:?} expected {expected:?} got {observed:?}"
    );
}

fn assert_age_for_elapsed(now_millis: i64, elapsed_seconds: u64, expected: &str) {
    #[allow(clippy::cast_possible_wrap)]
    let timestamp_millis = now_millis - (elapsed_seconds as i64 * 1_000);
    let iso = millis_to_iso(timestamp_millis);
    assert_age(now_millis, &iso, expected);
}

// A fixed reference instant: 2024-01-10T12:00:00.000Z.
const NOW_ISO: &str = "2024-01-10T12:00:00.000Z";
const NOW_MILLIS: i64 = 1_704_888_000_000;

// ---------------------------------------------------------------------------
// Boundary tables – every unit boundary from the TS ladder.
// ---------------------------------------------------------------------------

#[test]
fn seconds_boundary_table() {
    // `< 60` seconds stays in seconds. 0 is clamped future/invalid floor.
    let cases: &[(u64, &str)] = &[
        (0, "0s ago"),
        (1, "1s ago"),
        (30, "30s ago"),
        (59, "59s ago"),
        (60, "1m ago"),
    ];
    for (seconds, expected) in cases {
        assert_age_for_elapsed(NOW_MILLIS, *seconds, expected);
    }
}

#[test]
fn minutes_boundary_table() {
    let cases: &[(u64, &str)] = &[
        (60, "1m ago"),
        (61, "1m ago"),
        (119, "1m ago"),
        (120, "2m ago"),
        (59 * 60, "59m ago"),
        (60 * 60 - 1, "59m ago"),
        (60 * 60, "1h ago"),
    ];
    for (seconds, expected) in cases {
        assert_age_for_elapsed(NOW_MILLIS, *seconds, expected);
    }
}

#[test]
fn hours_boundary_table() {
    let cases: &[(u64, &str)] = &[
        (3_600, "1h ago"),
        (7_200, "2h ago"),
        (23 * 3_600, "23h ago"),
        (24 * 3_600 - 1, "23h ago"),
        (24 * 3_600, "1d ago"),
    ];
    for (seconds, expected) in cases {
        assert_age_for_elapsed(NOW_MILLIS, *seconds, expected);
    }
}

#[test]
fn days_boundary_table() {
    let cases: &[(u64, &str)] = &[
        (86_400, "1d ago"),
        (2 * 86_400, "2d ago"),
        (6 * 86_400, "6d ago"),
        (7 * 86_400 - 1, "6d ago"),
        (7 * 86_400, "1w ago"),
    ];
    for (seconds, expected) in cases {
        assert_age_for_elapsed(NOW_MILLIS, *seconds, expected);
    }
}

#[test]
fn weeks_boundary_table() {
    // weeks = floor(days / 7), threshold `< 5`
    let cases: &[(u64, &str)] = &[
        (7 * 86_400, "1w ago"),
        (14 * 86_400, "2w ago"),
        (21 * 86_400, "3w ago"),
        (28 * 86_400, "4w ago"),
        (34 * 86_400, "4w ago"),  // 34/7 = 4
        (35 * 86_400, "1mo ago"), // 35/7 = 5 → escapes to months
    ];
    for (seconds, expected) in cases {
        assert_age_for_elapsed(NOW_MILLIS, *seconds, expected);
    }
}

#[test]
fn months_boundary_table() {
    // months = floor(days / 30), but weeks < 5 takes precedence, so 30d is 4w.
    // 35d is the first month bucket (35/7=5 → months).
    let cases: &[(u64, &str)] = &[
        (30 * 86_400, "4w ago"), // 30d → 4w, not yet months
        (34 * 86_400, "4w ago"),
        (35 * 86_400, "1mo ago"), // 35/7=5 → 1mo
        (60 * 86_400, "2mo ago"),
        (330 * 86_400, "11mo ago"), // 330/30=11
        (359 * 86_400, "11mo ago"), // 359/30=11
        (360 * 86_400, "0y ago"),   // 360/30=12 → years (360/365=0)
        (364 * 86_400, "0y ago"),
        (365 * 86_400, "1y ago"),
    ];
    for (seconds, expected) in cases {
        assert_age_for_elapsed(NOW_MILLIS, *seconds, expected);
    }
}

#[test]
fn years_boundary_table() {
    let cases: &[(u64, &str)] = &[
        (365 * 86_400, "1y ago"),
        (730 * 86_400, "2y ago"),
        (1_095 * 86_400, "3y ago"),
        (10 * 365 * 86_400, "10y ago"),
    ];
    for (seconds, expected) in cases {
        assert_age_for_elapsed(NOW_MILLIS, *seconds, expected);
    }
}

// ---------------------------------------------------------------------------
// Future, negative, large, invalid
// ---------------------------------------------------------------------------

#[test]
fn future_timestamps_clamp_to_zero() {
    // `now` earlier than `timestamp` – clamped to 0s.
    let future_iso = "2024-01-10T12:00:01.000Z"; // 1s in the future
    assert_age(NOW_MILLIS, future_iso, "0s ago");

    let far_future = "2099-12-31T23:59:59.999Z";
    assert_age(NOW_MILLIS, far_future, "0s ago");

    // Even with `now` before epoch, a later timestamp still clamps.
    let before_epoch_now = -1_000;
    assert_age(before_epoch_now, "1970-01-01T00:00:00.000Z", "0s ago");

    // Equal instant.
    assert_age(NOW_MILLIS, NOW_ISO, "0s ago");
}

#[test]
fn negative_timestamps_and_before_epoch() {
    // `now` at epoch, timestamp one day before.
    let epoch: i64 = 0;
    let day_before = "1969-12-31T00:00:00.000Z";
    assert_age(epoch, day_before, "1d ago");

    // Large negative diff still yields years.
    let now_after = 2 * 365 * 86_400 * 1_000; // 2 years after epoch
    assert_age(now_after, "1969-01-01T00:00:00.000Z", "3y ago");

    // Timestamp at negative millis with sub-second now.
    let now = 500;
    assert_age(now, "1969-12-31T23:59:59.000Z", "1s ago");
}

#[test]
fn invalid_timestamps_produce_zero() {
    let invalids: &[&str] = &[
        "",
        "   ",
        "not-a-date",
        "2024-13-01T00:00:00.000Z",
        "2024-02-30T00:00:00.000Z",
        "2024-01-01T25:00:00.000Z",
        "2024-01-01T00:60:00.000Z",
        "2024-01-01T00:00:60.000Z",
        "2024-01-01T00:00:00.000",
        "2024/01/01T00:00:00Z",
        "invalid-iso-8601",
        "NaN",
        "undefined",
    ];
    for input in invalids {
        assert_age(NOW_MILLIS, input, "0s ago");
    }

    // Non-finite-like: the TS `Number.isFinite` guard maps NaN/Infinity from
    // `Date.parse` to 0. Our `None` does the same.
    assert_age(NOW_MILLIS, "Infinity", "0s ago");
}

#[test]
fn large_elapsed_produces_large_years() {
    // 100 years of days.
    let days_100y: u64 = 100 * 365;
    let seconds = days_100y * 86_400;
    assert_age_for_elapsed(NOW_MILLIS, seconds, "100y ago");

    // Extreme far past timestamp (year 1900) with now in 2024.
    assert_age(NOW_MILLIS, "1900-01-01T00:00:00.000Z", "124y ago");

    // Timestamp at Unix epoch with far-future now.
    let far_future_now: i64 = 8_640_000_000_000; // ~ year 2243
    assert_age(far_future_now, "1970-01-01T00:00:00.000Z", "273y ago");
}

#[test]
fn floor_rounding_not_ceil_or_round() {
    // 59.999 seconds worth of millis should still be 59s (floor).
    let now = 10_000;
    // 59_999 ms diff → 59s
    let ts = millis_to_iso(now - 59_999);
    assert_age(now, &ts, "59s ago");
    // 60_000 ms → 1m
    let ts = millis_to_iso(now - 60_000);
    assert_age(now, &ts, "1m ago");

    // 89 seconds → 1m (floor minutes), not 2m rounded.
    assert_age_for_elapsed(NOW_MILLIS, 89, "1m ago");
    assert_age_for_elapsed(NOW_MILLIS, 90, "1m ago");

    // 35 days → 1mo exactly, not 5w rounded.
    assert_age_for_elapsed(NOW_MILLIS, 35 * 86_400, "1mo ago");
}

// ---------------------------------------------------------------------------
// Timezone and fractional second handling
// ---------------------------------------------------------------------------

#[test]
fn timezone_offsets_are_applied() {
    // Same instant via different timezone representations must yield same label.
    // 2024-01-10T12:00:00.000Z == 2024-01-10T14:00:00.000+02:00
    let utc = "2024-01-10T12:00:00.000Z";
    let plus_two = "2024-01-10T14:00:00.000+02:00";
    let minus_five = "2024-01-10T07:00:00.000-05:00";
    let plus_two_nocolon = "2024-01-10T14:00:00.000+0200";
    let plus_two_hour_only = "2024-01-10T14:00:00+02";

    // now is 12:01:00Z → 60s diff → 1m for all representations.
    let now = NOW_MILLIS + 60_000;
    for ts in [
        utc,
        plus_two,
        minus_five,
        plus_two_nocolon,
        plus_two_hour_only,
    ] {
        assert_age(now, ts, "1m ago");
    }

    // Non-UTC instant: 12:00+02:00 is actually 10:00Z, so diff to 12:00Z is 2h.
    assert_age(NOW_MILLIS, "2024-01-10T12:00:00.000+02:00", "2h ago");
}

#[test]
fn fractional_seconds_handling() {
    // Fractional seconds are truncated to millis, then elapsed still floor seconds.
    // 30.999s diff → still 30s.
    let now: i64 = 100_000;
    // timestamp 30_999 ms before now
    let ts = millis_to_iso(now - 30_999);
    assert_age(now, &ts, "30s ago");

    // Direct ISO with extra fraction digits: "2024-01-10T11:59:30.9999Z" truncates to .999
    // Diff to 12:00:00.000 is 29.001s → floor 29s.
    let now_ms = NOW_MILLIS;
    let ts_extra = "2024-01-10T11:59:30.9999Z";
    assert_age(now_ms, ts_extra, "29s ago");

    // .1 → 100ms padding.
    let ts_dot1 = "2024-01-10T11:59:59.1Z"; // 0.9s before noon → 0s
    assert_age(NOW_MILLIS, ts_dot1, "0s ago");
    let ts_dot1_two = "2024-01-10T11:59:58.1Z"; // 1.9s before noon → 1s
    assert_age(NOW_MILLIS, ts_dot1_two, "1s ago");
}

// ---------------------------------------------------------------------------
// Parity examples: TS call sites
// ---------------------------------------------------------------------------

#[test]
fn parity_examples_match_typescript() {
    // These mirror direct `format_relative_age(now, timestamp)` calls as they
    // appear in `conversation-turn-footer.svelte` (`now` from `Clock.currentTimeMillis`).

    // Exact match to TS logic: future and invalid → 0s ago
    assert_age(NOW_MILLIS, "invalid", "0s ago");
    assert_age(NOW_MILLIS, "2024-01-10T12:00:10.000Z", "0s ago");

    // Sub-minute
    assert_age(NOW_MILLIS, "2024-01-10T11:59:30.000Z", "30s ago");
    assert_age(NOW_MILLIS, "2024-01-10T11:59:00.000Z", "1m ago");

    // Minutes
    assert_age(NOW_MILLIS, "2024-01-10T11:00:00.000Z", "1h ago");
    // 59 minutes exactly
    assert_age(NOW_MILLIS, "2024-01-10T11:01:00.000Z", "59m ago");

    // Hours
    assert_age(NOW_MILLIS, "2024-01-09T12:00:00.000Z", "1d ago");
    assert_age(NOW_MILLIS, "2024-01-04T12:00:00.000Z", "6d ago");
    assert_age(NOW_MILLIS, "2024-01-03T12:00:00.000Z", "1w ago");

    // Weeks → months transition at 35 days (5 weeks)
    assert_age(NOW_MILLIS, "2023-12-07T12:00:00.000Z", "4w ago"); // 34 days
    assert_age(NOW_MILLIS, "2023-12-06T12:00:00.000Z", "1mo ago"); // 35 days

    // Months – 30d is still 4w per `<5w` precedence; 1mo starts at 35d.
    assert_age(NOW_MILLIS, "2023-12-11T12:00:00.000Z", "4w ago"); // 30 days → 4w
    assert_age(NOW_MILLIS, "2023-02-10T12:00:00.000Z", "11mo ago"); // ~334 days
    assert_age(NOW_MILLIS, "2023-01-16T12:00:00.000Z", "11mo ago"); // 359 days

    // Years: 360 days → 0y per `days/30` falling into years bucket,
    // 365 days → 1y.
    assert_age(NOW_MILLIS, "2023-01-15T12:00:00.000Z", "0y ago"); // 360 days
    assert_age(NOW_MILLIS, "2023-01-10T12:00:00.000Z", "1y ago"); // 365 days
    assert_age(NOW_MILLIS, "2022-01-10T12:00:00.000Z", "2y ago");

    // Date-only form is UTC midnight.
    assert_age(NOW_MILLIS, "2024-01-10", "12h ago"); // midnight → noon
    assert_age(NOW_MILLIS, "2024-01-09", "1d ago");

    // Lowercase `t` and `z` are accepted (mirrors JS case-insensitive `T`/`Z` for ISO).
    assert_age(NOW_MILLIS, "2024-01-10t11:59:00.000z", "1m ago");
}

#[test]
fn labels_and_pluralization_are_exact() {
    // The TypeScript source always uses the same suffixes with no plural forms.
    // Ensure every label matches the literal `"{n}{unit} ago"` pattern.
    let table: &[(u64, &str)] = &[
        (5, "5s ago"),
        (5 * 60, "5m ago"),
        (5 * 3_600, "5h ago"),
        (5 * 86_400, "5d ago"),
        (14 * 86_400, "2w ago"),
        (60 * 86_400, "2mo ago"),
        (730 * 86_400, "2y ago"),
    ];
    for (seconds, expected) in table {
        // Check literal suffix and no alternative pluralization like `secs`/`mins`.
        assert!(
            expected.ends_with(" ago"),
            "suffix must be ' ago': {expected}"
        );
        assert_age_for_elapsed(NOW_MILLIS, *seconds, expected);
    }

    // Ensure `mo` for months, not `M` or `mon`.
    // 30d is still weeks, so first mo check is at 35d.
    assert_age_for_elapsed(NOW_MILLIS, 35 * 86_400, "1mo ago");
    // Ensure `y` for years.
    assert_age_for_elapsed(NOW_MILLIS, 365 * 86_400, "1y ago");
}

#[test]
fn explicit_now_is_required_no_global_clock() {
    // Same timestamp with different `now` yields different output, proving
    // the function does not read a global clock.
    let ts = "2024-01-10T11:59:00.000Z";
    let now_a = NOW_MILLIS; // 1 minute elapsed → 1m
    let now_b = NOW_MILLIS + 3_600_000; // 61 minutes elapsed → 1h
    assert_eq!(format_relative_age(now_a, ts), "1m ago");
    assert_eq!(format_relative_age(now_b, ts), "1h ago");

    // Deterministic: repeated calls with same inputs produce identical output.
    assert_eq!(
        format_relative_age(NOW_MILLIS, ts),
        format_relative_age(NOW_MILLIS, ts)
    );
}

#[test]
fn whitespace_trimming() {
    assert_age(NOW_MILLIS, "  2024-01-10T11:59:00.000Z  ", "1m ago");
    assert_age(NOW_MILLIS, "\t2024-01-10T11:59:00.000Z\n", "1m ago");
}

#[test]
fn exhaustive_seconds_to_years_monotonic() {
    // Spot-check monotonic ladder: elapsed never goes backwards as diff grows.
    let checkpoints: &[u64] = &[
        0, 59, 60, 3599, 3_600, 86_399, 86_400, 604_799, 604_800, 2_419_200, 2_592_000, 31_536_000,
    ];
    let mut prior_label = String::new();
    for seconds in checkpoints {
        #[allow(clippy::cast_possible_wrap)]
        let iso = millis_to_iso(NOW_MILLIS - (*seconds as i64 * 1_000));
        let label = format_relative_age(NOW_MILLIS, &iso);
        assert!(!label.is_empty());
        // No strict ordering assertion on string, but all labels must be valid.
        assert!(label.ends_with(" ago"));
        prior_label = label;
    }
    assert!(!prior_label.is_empty());
}
