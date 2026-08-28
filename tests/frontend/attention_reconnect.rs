//! Dependency-free exhaustive coverage for the pure attention reconnect policy.
//!
//! The implementation is loaded directly so this focused harness does not
//! require shared module registration or host/runtime integration.

#[path = "../../modules/frontend/src/attention_reconnect.rs"]
mod attention_reconnect;

use attention_reconnect::{AttentionReconnectIntent, AttentionReconnectPolicy};

#[test]
fn first_observation_only_establishes_state() {
    for watching in [false, true] {
        let mut policy = AttentionReconnectPolicy::new();

        assert_eq!(policy.last_watching(), None);
        assert_eq!(policy.observe(watching), AttentionReconnectIntent::NoRetry);
        assert_eq!(policy.last_watching(), Some(watching));
    }
}

#[test]
fn repeated_values_are_ignored() {
    for watching in [false, true] {
        let mut policy = AttentionReconnectPolicy::new();

        assert_eq!(policy.observe(watching), AttentionReconnectIntent::NoRetry);
        assert_eq!(policy.observe(watching), AttentionReconnectIntent::NoRetry);
        assert_eq!(policy.observe(watching), AttentionReconnectIntent::NoRetry);
    }
}

#[test]
fn every_later_two_value_transition_matches_the_truth_table() {
    let cases = [
        (false, false, AttentionReconnectIntent::NoRetry),
        (false, true, AttentionReconnectIntent::RetryConnection),
        (true, false, AttentionReconnectIntent::NoRetry),
        (true, true, AttentionReconnectIntent::NoRetry),
    ];

    for (previous, current, expected) in cases {
        let mut policy = AttentionReconnectPolicy::new();

        assert_eq!(policy.observe(previous), AttentionReconnectIntent::NoRetry);
        assert_eq!(policy.observe(current), expected);
        assert_eq!(policy.last_watching(), Some(current));
    }
}

#[test]
fn each_false_to_true_edge_authorizes_exactly_one_retry() {
    let observations = [
        (false, AttentionReconnectIntent::NoRetry),
        (true, AttentionReconnectIntent::RetryConnection),
        (true, AttentionReconnectIntent::NoRetry),
        (false, AttentionReconnectIntent::NoRetry),
        (false, AttentionReconnectIntent::NoRetry),
        (true, AttentionReconnectIntent::RetryConnection),
        (true, AttentionReconnectIntent::NoRetry),
        (false, AttentionReconnectIntent::NoRetry),
        (true, AttentionReconnectIntent::RetryConnection),
    ];
    let mut policy = AttentionReconnectPolicy::default();

    for (watching, expected) in observations {
        assert_eq!(policy.observe(watching), expected);
    }
}
