//! Exhaustive direct coverage for the dependency-free usage reset policy.

#[path = "../../modules/frontend/src/usage_reset_duration.rs"]
mod usage_reset_duration;

use usage_reset_duration::{EngineUsageWindow, usage_reset_duration};

const OBSERVATION_MS: i64 = 1_787_918_400_000;

fn window(resets_at: Option<&str>) -> EngineUsageWindow<'_> {
    EngineUsageWindow::new(resets_at)
}

#[test]
fn empty_groups_are_silent() {
    assert_eq!(usage_reset_duration(&[], OBSERVATION_MS), None);
}

#[test]
fn missing_stale_and_unparseable_timestamps_are_silent() {
    let cases = [
        [window(None)],
        [window(Some("2026-08-28T12:00:00.000Z"))],
        [window(Some("2026-08-28T11:59:59.999Z"))],
        [window(Some("not an ISO timestamp"))],
        [window(Some("2026-02-29T12:00:00.000Z"))],
        [window(Some("2026-08-28T13:00:00"))],
    ];

    for windows in cases {
        assert_eq!(usage_reset_duration(&windows, OBSERVATION_MS), None);
    }
}

#[test]
fn one_invalid_member_silences_an_otherwise_valid_group() {
    let windows = [
        window(Some("2026-08-28T13:00:00.000Z")),
        window(None),
        window(Some("2026-08-28T14:00:00.000Z")),
    ];

    assert_eq!(usage_reset_duration(&windows, OBSERVATION_MS), None);
}

#[test]
fn a_stale_member_silences_future_members() {
    let windows = [
        window(Some("2026-08-28T13:00:00.000Z")),
        window(Some("2026-08-28T12:00:00.000Z")),
        window(Some("2026-08-28T17:00:00.000Z")),
    ];

    assert_eq!(usage_reset_duration(&windows, OBSERVATION_MS), None);
}

#[test]
fn latest_future_window_controls_the_duration() {
    let windows = [
        window(Some("2026-08-28T13:00:00.000Z")),
        window(Some("2026-08-28T17:00:00.000Z")),
        window(Some("2026-08-28T14:00:00.000Z")),
    ];

    assert_eq!(
        usage_reset_duration(&windows, OBSERVATION_MS),
        Some(String::from("5 hours"))
    );
}

#[test]
fn minute_hour_and_day_boundaries_round_up_in_a_table() {
    let cases = [
        ("2026-08-28T12:00:00.001Z", "1 minute"),
        ("2026-08-28T12:00:59.999Z", "1 minute"),
        ("2026-08-28T12:01:00.000Z", "1 minute"),
        ("2026-08-28T12:01:00.001Z", "2 minutes"),
        ("2026-08-28T12:59:59.999Z", "1 hour"),
        ("2026-08-28T13:00:00.000Z", "1 hour"),
        ("2026-08-28T13:00:00.0009Z", "1 hour"),
        ("2026-08-28T13:00:00.001Z", "2 hours"),
        ("2026-08-29T11:59:59.999Z", "1 day"),
        ("2026-08-29T12:00:00.000Z", "1 day"),
        ("2026-08-29T12:00:00.0009Z", "1 day"),
        ("2026-08-29T12:00:00.001Z", "2 days"),
        ("2026-08-30T12:00:00.000Z", "2 days"),
    ];

    for (reset_at, expected) in cases {
        assert_eq!(
            usage_reset_duration(&[window(Some(reset_at))], OBSERVATION_MS),
            Some(String::from(expected)),
            "reset_at={reset_at}"
        );
    }
}

#[test]
fn explicit_iso_offsets_are_converted_before_rounding() {
    let cases = [
        ("2026-08-28T14:00:00.001+02:00", "1 minute"),
        ("2026-08-28T07:00:00.001-05:00", "1 minute"),
        ("2026-08-28T14:01:00.001+02:00", "2 minutes"),
        ("2026-08-28T08:31:00.001-03:30", "2 minutes"),
        ("2026-08-28T14:01:00.001+0200", "2 minutes"),
        ("2026-08-29T00:00:00.001+12:00", "1 minute"),
        ("2026-08-28T00:00:00.001-12:00", "1 minute"),
    ];

    for (reset_at, expected) in cases {
        assert_eq!(
            usage_reset_duration(&[window(Some(reset_at))], OBSERVATION_MS),
            Some(String::from(expected)),
            "reset_at={reset_at}"
        );
    }
}

#[test]
fn far_future_values_stay_finite_and_out_of_range_dates_are_silent() {
    assert_eq!(
        usage_reset_duration(&[window(Some("9999-12-31T23:59:59.999Z"))], OBSERVATION_MS),
        Some(String::from("2912204 days"))
    );
    assert_eq!(
        usage_reset_duration(
            &[window(Some("+275760-09-13T00:00:00.000Z"))],
            OBSERVATION_MS
        ),
        Some(String::from("99979307 days"))
    );
    assert_eq!(
        usage_reset_duration(
            &[window(Some("+275760-09-13T00:00:00.001Z"))],
            OBSERVATION_MS
        ),
        None
    );
}
