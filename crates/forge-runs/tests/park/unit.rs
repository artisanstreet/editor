use super::*;
use artisan_api::{UsageCadence, UsageWindow};

#[test]
fn overload_retries_back_off_to_five_minutes_and_keep_going() {
    let waits: Vec<u32> = (0..7)
        .map(|tries| {
            u32::try_from(retry_delay(tries).as_millis() / FIRST_RETRY.as_millis()).unwrap()
        })
        .collect();
    assert_eq!(waits, [1, 2, 4, 8, 16, 20, 20]);
    assert_eq!(retry_delay(10_000), LONGEST_RETRY);
}

#[test]
fn the_dispatcher_rechecks_a_parked_turn_it_could_not_start_instead_of_spinning() {
    assert_eq!(
        wake(Parked::Due(900), 1_000, false),
        Wake::After(Duration::ZERO)
    );
    assert_eq!(wake(Parked::Due(900), 1_000, true), Wake::After(RECHECK));
    assert_eq!(
        wake(Parked::Due(1_000 + 60_000), 1_000, true),
        Wake::After(Duration::from_mins(1))
    );
    assert_eq!(wake(Parked::Unreadable, 1_000, false), Wake::After(RECHECK));
    assert_eq!(wake(Parked::Nothing, 1_000, true), Wake::Never);
}

#[test]
fn a_used_up_plan_waits_for_its_last_exhausted_window() {
    let window = |used_percent: f64, resets_at: Timestamp| UsageWindow {
        label: String::new(),
        cadence: UsageCadence::Other,
        scope: None,
        used_percent,
        resets_at: Some(resets_at),
    };
    let limits = UsageLimits {
        windows: vec![
            window(100.0, 5_000),
            window(40.0, 9_000),
            window(100.0, 7_000),
        ],
        spend: None,
        read_at: 0,
    };
    assert_eq!(exhausted_reset(&limits, 1_000), Some(7_000));
    assert_eq!(usage_due(Some(7_000), 1_000), 7_000 + millis(RESET_SLACK));
    assert_eq!(usage_due(None, 1_000), 1_000 + millis(UNKNOWN_RESET));
    assert_eq!(usage_due(Some(500), 1_000), 1_000 + millis(UNKNOWN_RESET));
}
