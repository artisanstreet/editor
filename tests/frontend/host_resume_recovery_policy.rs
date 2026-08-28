//! Focused dependency-free coverage for the host-resume recovery policy.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/host_resume_recovery_policy.rs"]
mod host_resume_recovery_policy;

use host_resume_recovery_policy::{
    DEFAULT_HEARTBEAT_MS, DEFAULT_MINIMUM_GAP_MS, HostResumeRecoveryConfig,
    HostResumeRecoveryConfigError, HostResumeRecoveryDecision, HostResumeRecoveryOptions,
    HostResumeRecoveryPolicy, host_resume_recovery_decision,
};

fn defaults() -> HostResumeRecoveryOptions {
    HostResumeRecoveryOptions::default()
}

#[test]
fn production_defaults_are_applied_and_validated() {
    let config = HostResumeRecoveryConfig::default();
    let policy = HostResumeRecoveryPolicy::default();

    assert_eq!(DEFAULT_HEARTBEAT_MS, 10_000);
    assert_eq!(DEFAULT_MINIMUM_GAP_MS, 30_000);
    assert_eq!(config.heartbeat_ms(), DEFAULT_HEARTBEAT_MS);
    assert_eq!(config.minimum_gap_ms(), DEFAULT_MINIMUM_GAP_MS);
    assert_eq!(policy.config(), config);
    assert_eq!(defaults().resolve(), Ok(config));
}

#[test]
fn exact_threshold_emits_one_reconnect_decision() {
    let policy = HostResumeRecoveryPolicy::default();
    let decision = policy.observe(1_000, 41_000);

    assert_eq!(
        decision,
        HostResumeRecoveryDecision::Reconnect {
            resumed_at_ms: 41_000,
            excess_gap_ms: 30_000,
        }
    );
    assert!(decision.is_reconnect());
    assert!(decision.is_recovery_authorized());
    assert_eq!(decision.excess_gap_ms(), Some(30_000));
}

#[test]
fn gap_below_threshold_and_scheduler_drift_do_not_reconnect() {
    let policy = HostResumeRecoveryPolicy::default();

    let one_below = policy.observe(1_000, 40_999);
    assert_eq!(
        one_below,
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms: 40_999,
            excess_gap_ms: 29_999,
        }
    );
    assert!(!one_below.is_reconnect());

    let scheduler_drift = policy.observe(100_000, 112_500);
    assert_eq!(
        scheduler_drift,
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms: 112_500,
            excess_gap_ms: 2_500,
        }
    );

    // A wake before the expected heartbeat saturates the excess at zero.
    let early_wake = policy.observe(100_000, 109_999);
    assert_eq!(
        early_wake,
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms: 109_999,
            excess_gap_ms: 0,
        }
    );
}

#[test]
fn backward_wall_clock_movement_never_reconnects() {
    let policy = HostResumeRecoveryPolicy::default();
    let decision = policy.observe(5_000, 4_999);

    assert_eq!(
        decision,
        HostResumeRecoveryDecision::ClockMovedBackward {
            started_at_ms: 5_000,
            resumed_at_ms: 4_999,
        }
    );
    assert!(!decision.is_reconnect());
    assert!(!decision.is_recovery_authorized());
    assert_eq!(decision.excess_gap_ms(), None);
}

#[test]
fn signed_timestamp_extremes_and_maximum_settings_do_not_wrap() {
    let almost_maximum_gap = HostResumeRecoveryConfig::new(1, u64::MAX - 1)
        .expect("positive heartbeat and maximum threshold validate");
    let maximum_span =
        HostResumeRecoveryPolicy::from_config(almost_maximum_gap).observe(i64::MIN, i64::MAX);
    assert_eq!(
        maximum_span,
        HostResumeRecoveryDecision::Reconnect {
            resumed_at_ms: i64::MAX,
            excess_gap_ms: u64::MAX - 1,
        }
    );

    // Saturating subtraction prevents a maximum heartbeat from becoming a
    // wrapped positive gap at the same timestamp boundary.
    let maximum_heartbeat =
        HostResumeRecoveryConfig::new(u64::MAX, 1).expect("maximum heartbeat is nonzero and valid");
    let no_wrapped_gap =
        HostResumeRecoveryPolicy::from_config(maximum_heartbeat).observe(i64::MIN, i64::MAX);
    assert_eq!(
        no_wrapped_gap,
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms: i64::MAX,
            excess_gap_ms: 0,
        }
    );

    let extreme_regression = HostResumeRecoveryPolicy::default().observe(i64::MAX, i64::MIN);
    assert!(matches!(
        extreme_regression,
        HostResumeRecoveryDecision::ClockMovedBackward { .. }
    ));
}

#[test]
fn custom_heartbeat_and_minimum_gap_are_retained() {
    let options = HostResumeRecoveryOptions {
        heartbeat_ms: Some(2_000),
        minimum_gap_ms: Some(5_000),
    };
    let policy = HostResumeRecoveryPolicy::new(options).expect("custom options validate");

    assert_eq!(policy.config().heartbeat_ms(), 2_000);
    assert_eq!(policy.config().minimum_gap_ms(), 5_000);
    assert_eq!(
        policy.observe(10_000, 17_000),
        HostResumeRecoveryDecision::Reconnect {
            resumed_at_ms: 17_000,
            excess_gap_ms: 5_000,
        }
    );
    assert_eq!(
        policy.observe(10_000, 16_999),
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms: 16_999,
            excess_gap_ms: 4_999,
        }
    );

    // A negative mathematical excess remains scheduler drift even when a
    // caller deliberately chooses a zero minimum gap.
    let zero_minimum = HostResumeRecoveryPolicy::new(HostResumeRecoveryOptions {
        heartbeat_ms: Some(2_000),
        minimum_gap_ms: Some(0),
    })
    .expect("zero minimum is valid with a positive heartbeat");
    assert_eq!(
        zero_minimum.observe(10_000, 11_999),
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms: 11_999,
            excess_gap_ms: 0,
        }
    );
    assert_eq!(
        zero_minimum.observe(10_000, 12_000),
        HostResumeRecoveryDecision::Reconnect {
            resumed_at_ms: 12_000,
            excess_gap_ms: 0,
        }
    );
}

#[test]
fn zero_heartbeat_is_rejected_before_a_policy_can_be_used() {
    let config_error = HostResumeRecoveryConfig::new(0, DEFAULT_MINIMUM_GAP_MS)
        .expect_err("zero heartbeat must be rejected");
    let options_error = HostResumeRecoveryOptions {
        heartbeat_ms: Some(0),
        minimum_gap_ms: None,
    }
    .resolve()
    .expect_err("zero heartbeat must be rejected after defaulting");
    let policy_error = HostResumeRecoveryPolicy::new(HostResumeRecoveryOptions {
        heartbeat_ms: Some(0),
        minimum_gap_ms: Some(1),
    })
    .expect_err("invalid settings must not construct a policy");

    assert_eq!(config_error, HostResumeRecoveryConfigError::ZeroHeartbeat);
    assert_eq!(options_error, HostResumeRecoveryConfigError::ZeroHeartbeat);
    assert_eq!(policy_error, HostResumeRecoveryConfigError::ZeroHeartbeat);
    assert_eq!(
        config_error.to_string(),
        "host-resume heartbeat must be greater than zero"
    );
}

#[test]
fn each_observation_returns_only_one_decision_without_retry_state() {
    let policy = HostResumeRecoveryPolicy::default();
    let decisions = [
        policy.observe(0, 30_000),
        policy.observe(0, 30_000),
        policy.observe(0, 30_001),
    ];

    assert_eq!(
        decisions[0],
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms: 30_000,
            excess_gap_ms: 20_000,
        }
    );
    assert_eq!(
        decisions[1],
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms: 30_000,
            excess_gap_ms: 20_000,
        }
    );
    assert_eq!(
        decisions[2],
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms: 30_001,
            excess_gap_ms: 20_001,
        }
    );

    let direct =
        host_resume_recovery_decision(0, 30_000, defaults()).expect("default options validate");
    assert_eq!(direct, decisions[0]);
}
