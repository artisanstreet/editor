//! Focused, dependency-free coverage for Forge recovery-health reachability.

#[path = "../../modules/frontend/src/forge_recovery_health.rs"]
mod forge_recovery_health;

use std::time::Duration;

use forge_recovery_health::{
    FORGE_RECOVERY_HEALTH_DEADLINE, ProbeObservation, probe_forge_recovery_health,
};

#[test]
fn deadline_is_the_exact_typed_millisecond_value() {
    assert_eq!(FORGE_RECOVERY_HEALTH_DEADLINE, Duration::from_millis(1_500));
    assert_eq!(FORGE_RECOVERY_HEALTH_DEADLINE.as_secs(), 1);
    assert_eq!(FORGE_RECOVERY_HEALTH_DEADLINE.subsec_millis(), 500);
}

#[test]
fn status_table_preserves_signed_and_large_outcomes() {
    let cases = [
        (i64::MIN, false),
        (-1, false),
        (0, false),
        (199, false),
        (200, true),
        (201, true),
        (250, true),
        (298, true),
        (299, true),
        (300, false),
        (599, false),
        (i64::MAX, false),
    ];

    for (status, expected) in cases {
        let observation = ProbeObservation::HttpStatus { status };
        assert_eq!(
            observation.is_reachable(),
            expected,
            "status {status} reachability"
        );
        assert_eq!(
            probe_forge_recovery_health(observation),
            expected,
            "status {status} free-function reachability"
        );
    }
}

#[test]
fn every_status_in_the_inclusive_success_interval_is_reachable() {
    for status in 200_i64..=299 {
        assert!(
            probe_forge_recovery_health(ProbeObservation::HttpStatus { status }),
            "status {status} must be reachable"
        );
    }
}

#[test]
fn non_response_observations_are_always_unreachable() {
    let cases = [
        (ProbeObservation::Timeout, false),
        (ProbeObservation::TransportFailure, false),
    ];

    for (observation, expected) in cases {
        assert_eq!(
            probe_forge_recovery_health(observation),
            expected,
            "observation {observation:?} reachability"
        );
    }
}
