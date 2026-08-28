//! Focused dependency-free coverage for host-suspend detection policy.

#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "../../modules/backend/src/host_suspend_detection_policy.rs"]
mod host_suspend_detection_policy;

use host_suspend_detection_policy::{
    DEFAULT_HEARTBEAT_MS, DEFAULT_MINIMUM_GAP_MS, HostResumeObservation,
    HostSuspendDetectionConfig, HostSuspendDetectionConfigError, HostSuspendDetectionDecision,
    HostSuspendDetectionOptions, HostSuspendDetectionPolicy, host_suspend_detection_decision,
};

fn default_options() -> HostSuspendDetectionOptions {
    HostSuspendDetectionOptions::default()
}

#[test]
fn production_defaults_are_applied_independently() {
    let config = HostSuspendDetectionConfig::default();
    let policy = HostSuspendDetectionPolicy::default();

    assert_eq!(DEFAULT_HEARTBEAT_MS, 10_000);
    assert_eq!(DEFAULT_MINIMUM_GAP_MS, 30_000);
    assert_eq!(config.heartbeat_ms(), DEFAULT_HEARTBEAT_MS);
    assert_eq!(config.minimum_gap_ms(), DEFAULT_MINIMUM_GAP_MS);
    assert_eq!(policy.config(), config);
    assert_eq!(default_options().resolve(), Ok(config));

    let heartbeat_override = HostSuspendDetectionOptions {
        heartbeat_ms: Some(2_000),
        minimum_gap_ms: None,
    }
    .resolve()
    .expect("heartbeat override must validate");
    assert_eq!(heartbeat_override.heartbeat_ms(), 2_000);
    assert_eq!(heartbeat_override.minimum_gap_ms(), DEFAULT_MINIMUM_GAP_MS);

    let minimum_override = HostSuspendDetectionOptions {
        heartbeat_ms: None,
        minimum_gap_ms: Some(5_000),
    }
    .resolve()
    .expect("minimum-gap override must validate");
    assert_eq!(minimum_override.heartbeat_ms(), DEFAULT_HEARTBEAT_MS);
    assert_eq!(minimum_override.minimum_gap_ms(), 5_000);
}

#[test]
fn below_equal_and_above_threshold_are_decisive() {
    let policy = HostSuspendDetectionPolicy::default();

    let below = policy.observe(1_000, 40_999);
    assert_eq!(
        below,
        HostSuspendDetectionDecision::NoResume {
            resumed_at_ms: 40_999,
            suspended_ms: 29_999,
        }
    );
    assert!(!below.is_resume());
    assert_eq!(below.suspended_ms(), Some(29_999));

    let equal = policy.observe(1_000, 41_000);
    assert_eq!(
        equal,
        HostSuspendDetectionDecision::Resume(HostResumeObservation::new(41_000, 30_000))
    );
    assert!(equal.is_resume());
    assert!(equal.is_suspend_detected());

    let above = policy.observe(1_000, 41_001);
    assert_eq!(
        above,
        HostSuspendDetectionDecision::Resume(HostResumeObservation::new(41_001, 30_001))
    );
}

#[test]
fn ordinary_scheduler_drift_and_early_wakes_do_not_resume() {
    let policy = HostSuspendDetectionPolicy::default();

    // Twelve and a half seconds elapsed on the wall clock; only 2.5 seconds
    // remained after the expected ten-second heartbeat.
    assert_eq!(
        policy.observe(100_000, 112_500),
        HostSuspendDetectionDecision::NoResume {
            resumed_at_ms: 112_500,
            suspended_ms: 2_500,
        }
    );

    // An early wake produces a negative candidate, not a wrapped large gap.
    assert_eq!(
        policy.observe(100_000, 109_999),
        HostSuspendDetectionDecision::NoResume {
            resumed_at_ms: 109_999,
            suspended_ms: -1,
        }
    );
    assert!(!policy.observe(100_000, 100_000).is_resume());
}

#[test]
fn custom_heartbeat_and_minimum_gap_are_used_verbatim() {
    let options = HostSuspendDetectionOptions {
        heartbeat_ms: Some(2_000),
        minimum_gap_ms: Some(5_000),
    };
    let policy = HostSuspendDetectionPolicy::new(options).expect("custom options must validate");

    assert_eq!(policy.config().heartbeat_ms(), 2_000);
    assert_eq!(policy.config().minimum_gap_ms(), 5_000);
    assert_eq!(
        policy.observe(10_000, 16_999),
        HostSuspendDetectionDecision::NoResume {
            resumed_at_ms: 16_999,
            suspended_ms: 4_999,
        }
    );
    assert_eq!(
        policy.observe(10_000, 17_000),
        HostSuspendDetectionDecision::Resume(HostResumeObservation::new(17_000, 5_000))
    );

    let zero_minimum = HostSuspendDetectionPolicy::new(HostSuspendDetectionOptions {
        heartbeat_ms: Some(2_000),
        minimum_gap_ms: Some(0),
    })
    .expect("zero minimum gap is valid with a positive heartbeat");
    assert_eq!(
        zero_minimum.observe(10_000, 12_000),
        HostSuspendDetectionDecision::Resume(HostResumeObservation::new(12_000, 0))
    );
    assert!(!zero_minimum.observe(10_000, 11_999).is_resume());
}

#[test]
fn backward_wall_clock_movement_is_explicit_and_never_resumes() {
    let decision = HostSuspendDetectionPolicy::default().observe(5_000, 4_999);

    assert_eq!(
        decision,
        HostSuspendDetectionDecision::ClockMovedBackward {
            started_at_ms: 5_000,
            resumed_at_ms: 4_999,
        }
    );
    assert!(!decision.is_resume());
    assert!(!decision.is_suspend_detected());
    assert_eq!(decision.resume_observation(), None);
    assert_eq!(decision.suspended_ms(), None);
}

#[test]
fn signed_timestamp_extremes_use_wide_non_wrapping_arithmetic() {
    // i64::MAX - i64::MIN is u64::MAX, which cannot be represented by an
    // i64 duration but fits exactly in the policy's i128 intermediate.
    let maximum_span = HostSuspendDetectionPolicy::from_config(
        HostSuspendDetectionConfig::new(1, u64::MAX - 1)
            .expect("positive heartbeat and maximum threshold must validate"),
    )
    .observe(i64::MIN, i64::MAX);
    assert_eq!(
        maximum_span,
        HostSuspendDetectionDecision::Resume(HostResumeObservation::new(
            i64::MAX,
            i128::from(u64::MAX - 1),
        ))
    );

    let maximum_heartbeat = HostSuspendDetectionPolicy::from_config(
        HostSuspendDetectionConfig::new(u64::MAX, 1)
            .expect("maximum nonzero heartbeat must validate"),
    )
    .observe(i64::MIN, i64::MAX);
    assert_eq!(
        maximum_heartbeat,
        HostSuspendDetectionDecision::NoResume {
            resumed_at_ms: i64::MAX,
            suspended_ms: 0,
        }
    );

    let no_wrapped_underflow = HostSuspendDetectionPolicy::from_config(
        HostSuspendDetectionConfig::new(u64::MAX, 0)
            .expect("maximum nonzero heartbeat must validate"),
    )
    .observe(0, 0);
    assert_eq!(
        no_wrapped_underflow,
        HostSuspendDetectionDecision::NoResume {
            resumed_at_ms: 0,
            suspended_ms: -i128::from(u64::MAX),
        }
    );
}

#[test]
fn extreme_clock_regression_does_not_enter_arithmetic_or_resume_path() {
    let decision = HostSuspendDetectionPolicy::default().observe(i64::MAX, i64::MIN);

    assert_eq!(
        decision,
        HostSuspendDetectionDecision::ClockMovedBackward {
            started_at_ms: i64::MAX,
            resumed_at_ms: i64::MIN,
        }
    );
    assert!(!decision.is_resume());
    assert_eq!(decision.resume_observation(), None);
}

#[test]
fn resume_observation_custody_preserves_exact_values() {
    let options = HostSuspendDetectionOptions {
        heartbeat_ms: Some(100),
        minimum_gap_ms: Some(50),
    };
    let decision = host_suspend_detection_decision(-10_000, -9_850, options)
        .expect("custom options must validate");
    let observation = decision
        .resume_observation()
        .expect("threshold must resume");

    assert_eq!(
        observation,
        HostResumeObservation {
            resumed_at_ms: -9_850,
            suspended_ms: 50,
        }
    );
    assert_eq!(observation.resumed_at_ms(), -9_850);
    assert_eq!(observation.suspended_ms(), 50);
    assert_eq!(decision.suspended_ms(), Some(50));
}

#[test]
fn repeated_ticks_are_independent_and_do_not_consume_observations() {
    let policy = HostSuspendDetectionPolicy::default();
    let decisions = [
        policy.observe(0, 40_999),
        policy.observe(0, 41_000),
        policy.observe(0, 41_000),
        policy.observe(0, 41_001),
    ];

    assert_eq!(decisions[0].suspended_ms(), Some(30_999));
    assert_eq!(
        decisions[1].resume_observation(),
        Some(HostResumeObservation::new(41_000, 31_000)),
    );
    assert_eq!(decisions[1], decisions[2]);
    assert_eq!(
        decisions[3].resume_observation(),
        Some(HostResumeObservation::new(41_001, 31_001)),
    );
}

#[test]
fn zero_heartbeat_is_rejected_before_policy_use() {
    let config_error = HostSuspendDetectionConfig::new(0, DEFAULT_MINIMUM_GAP_MS)
        .expect_err("zero heartbeat must be rejected");
    let options_error = HostSuspendDetectionOptions {
        heartbeat_ms: Some(0),
        minimum_gap_ms: None,
    }
    .resolve()
    .expect_err("zero heartbeat must be rejected after defaulting");
    let policy_error = HostSuspendDetectionPolicy::new(HostSuspendDetectionOptions {
        heartbeat_ms: Some(0),
        minimum_gap_ms: Some(1),
    })
    .expect_err("invalid settings must not construct a policy");

    assert_eq!(config_error, HostSuspendDetectionConfigError::ZeroHeartbeat);
    assert_eq!(
        options_error,
        HostSuspendDetectionConfigError::ZeroHeartbeat
    );
    assert_eq!(policy_error, HostSuspendDetectionConfigError::ZeroHeartbeat);
    assert_eq!(
        config_error.to_string(),
        "host-suspend heartbeat must be greater than zero"
    );
}
