use editor_aqua::shell::live_throttle::{INTERVAL, LiveThrottle};
use std::time::{Duration, Instant};

#[test]
fn the_first_delta_applies_and_the_following_ones_wait_for_the_interval() {
    let start = Instant::now();
    let mut throttle = LiveThrottle::default();
    assert!(throttle.arrive(start));
    assert!(!throttle.arrive(start + (INTERVAL / 3)));
    assert!(!throttle.arrive(start + (INTERVAL * 2 / 3)));
    assert!(!throttle.due(start + INTERVAL - Duration::from_millis(1)));
    assert!(throttle.due(start + INTERVAL));
    assert!(!throttle.due(start + INTERVAL * 2));
}

#[test]
fn a_deferred_delta_reports_how_long_it_still_waits() {
    let start = Instant::now();
    let mut throttle = LiveThrottle::default();
    assert_eq!(throttle.remaining(start), None);
    assert!(throttle.arrive(start));
    assert_eq!(throttle.remaining(start), None);
    assert!(!throttle.arrive(start + (INTERVAL / 2)));
    assert_eq!(
        throttle.remaining(start + (INTERVAL / 2)),
        Some(INTERVAL - (INTERVAL / 2))
    );
    assert_eq!(
        throttle.remaining(start + INTERVAL * 2),
        Some(Duration::ZERO)
    );
}

#[test]
fn a_rebuild_for_another_reason_satisfies_the_deferred_delta() {
    let start = Instant::now();
    let mut throttle = LiveThrottle::default();
    assert!(throttle.arrive(start));
    assert!(!throttle.arrive(start + (INTERVAL / 4)));
    throttle.flushed(start + (INTERVAL / 2));
    assert_eq!(throttle.remaining(start + (INTERVAL * 2 / 3)), None);
    assert!(!throttle.due(start + INTERVAL * 3));
    assert!(!throttle.arrive(start + (INTERVAL * 3 / 4)));
}

#[test]
fn a_delta_after_a_quiet_interval_applies_immediately() {
    let start = Instant::now();
    let mut throttle = LiveThrottle::default();
    assert!(throttle.arrive(start));
    assert!(throttle.arrive(start + INTERVAL));
    assert!(throttle.arrive(start + INTERVAL * 5));
}
