//! Focused, dependency-free coverage for authoritative subscription recovery.
//!
//! The production module is path-linked deliberately. This packet owns no
//! frontend registration, runtime, transport, stream, scope, or timer code.

#[path = "../../modules/frontend/src/authoritative_subscription_policy.rs"]
mod authoritative_subscription_policy;

use std::time::Duration;

use authoritative_subscription_policy::{
    retry_delay, AuthoritativeSubscriptionAction as Action,
    AuthoritativeSubscriptionFailure as Failure, AuthoritativeSubscriptionPolicy as Policy,
    AuthoritativeSubscriptionState as State, ConversationSubscriptionAction,
    ConversationSubscriptionAttempt, ConversationSubscriptionPolicy, ConversationSubscriptionState,
    RunConversationSubscriptionPolicy, AUTHORITATIVE_SUBSCRIPTION_LOST_MESSAGE,
    AUTHORITATIVE_SUBSCRIPTION_RETRY_INITIAL_DELAY,
    AUTHORITATIVE_SUBSCRIPTION_RETRY_INITIAL_DELAY_MS, AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY,
    AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY_MS,
};

fn begin_policy() -> (
    Policy,
    authoritative_subscription_policy::SubscriptionAttempt,
) {
    let mut policy = Policy::new();
    let transition = policy.begin_attempt().expect("first attempt begins");
    let Some(Action::StartAttempt { attempt }) = transition.action else {
        panic!("first transition must start an attempt");
    };
    (policy, attempt)
}

fn finalize_failed_attempt(
    policy: &mut Policy,
    attempt: authoritative_subscription_policy::SubscriptionAttempt,
) -> authoritative_subscription_policy::AuthoritativeSubscriptionTransition {
    let finalized = policy
        .scope_finalized(attempt)
        .expect("failed attempt scope finalizes");
    assert_eq!(
        finalized.action,
        Some(Action::Recover {
            attempt,
            failure: match finalized.state {
                State::Recovering { failure, .. } => failure,
                state => panic!("expected recovering state, got {state:?}"),
            },
        })
    );
    finalized
}

#[test]
fn retry_constants_and_lost_message_are_exact() {
    assert_eq!(AUTHORITATIVE_SUBSCRIPTION_RETRY_INITIAL_DELAY_MS, 100);
    assert_eq!(AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY_MS, 5_000);
    assert_eq!(
        AUTHORITATIVE_SUBSCRIPTION_RETRY_INITIAL_DELAY,
        Duration::from_millis(100)
    );
    assert_eq!(
        AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY,
        Duration::from_millis(5_000)
    );
    assert_eq!(
        AUTHORITATIVE_SUBSCRIPTION_LOST_MESSAGE,
        "Authoritative subscription ended unexpectedly."
    );
}

#[test]
fn subscribe_error_ends_attempt_and_requests_scope_finalization() {
    let (mut policy, attempt) = begin_policy();

    let transition = policy.subscribe_failed(attempt).expect("subscribe fails");

    assert_eq!(
        transition.state,
        State::FinalizingScope {
            attempt,
            failure: Failure::Subscribe,
        }
    );
    assert_eq!(transition.action, Some(Action::FinalizeScope { attempt }));
}

#[test]
fn stream_error_ends_a_started_attempt() {
    let (mut policy, attempt) = begin_policy();
    policy
        .subscribe_succeeded(attempt)
        .expect("subscribe succeeds");

    let transition = policy.stream_failed(attempt).expect("stream fails");

    assert_eq!(
        transition.state,
        State::FinalizingScope {
            attempt,
            failure: Failure::Stream,
        }
    );
    assert_eq!(transition.action, Some(Action::FinalizeScope { attempt }));
}

#[test]
fn update_error_ends_the_streaming_attempt() {
    let (mut policy, attempt) = begin_policy();
    policy
        .subscribe_succeeded(attempt)
        .expect("subscribe succeeds");
    policy
        .stream_update(attempt)
        .expect("one update is accepted");

    let transition = policy.update_failed(attempt).expect("update fails");

    assert_eq!(
        transition.state,
        State::FinalizingScope {
            attempt,
            failure: Failure::Update,
        }
    );
    assert_eq!(transition.action, Some(Action::FinalizeScope { attempt }));
}

#[test]
fn stream_and_update_failures_cannot_bypass_subscribe_phase() {
    let (mut policy, attempt) = begin_policy();

    assert!(policy.stream_failed(attempt).is_err());
    assert!(policy.update_failed(attempt).is_err());
    assert_eq!(
        policy.state(),
        State::Subscribing { attempt },
        "rejected failures must not mutate the attempt"
    );
}

#[test]
fn normal_stream_end_becomes_the_exact_lost_subscription_failure() {
    let (mut policy, attempt) = begin_policy();
    policy
        .subscribe_succeeded(attempt)
        .expect("subscribe succeeds");

    let transition = policy.stream_ended(attempt).expect("stream ends");
    let failure = match transition.state {
        State::FinalizingScope { failure, .. } => failure,
        state => panic!("expected finalization, got {state:?}"),
    };
    assert_eq!(failure, Failure::LostSubscription);

    assert_eq!(
        Failure::LostSubscription.lost_subscription_message(),
        Some(AUTHORITATIVE_SUBSCRIPTION_LOST_MESSAGE)
    );
    assert!(Failure::LostSubscription.is_lost_subscription());
    assert_eq!(transition.action, Some(Action::FinalizeScope { attempt }));
}

#[test]
fn finalization_precedes_recovery_and_retry() {
    let (mut policy, attempt) = begin_policy();
    let failed = policy.subscribe_failed(attempt).expect("attempt fails");

    assert!(policy.recovery_succeeded(attempt).is_err());
    assert!(policy.retry_ready(attempt).is_err());

    let recovered = finalize_failed_attempt(&mut policy, attempt);
    assert_eq!(
        recovered.state.kind(),
        authoritative_subscription_policy::AuthoritativeSubscriptionStateKind::Recovering
    );
    assert!(policy.retry_ready(attempt).is_err());

    let retry = policy
        .recovery_succeeded(attempt)
        .expect("recovery follows finalization");
    assert_eq!(
        retry.action,
        Some(Action::RetryAfterDelay {
            attempt,
            delay: Duration::from_millis(100),
        })
    );
    assert_eq!(failed.action, Some(Action::FinalizeScope { attempt }));
}

#[test]
fn successful_and_failing_recovery_are_both_absorbed_into_the_same_retry() {
    let (mut successful, successful_attempt) = begin_policy();
    successful
        .subscribe_failed(successful_attempt)
        .expect("attempt fails");
    finalize_failed_attempt(&mut successful, successful_attempt);

    let (mut failing, failing_attempt) = begin_policy();
    failing
        .subscribe_failed(failing_attempt)
        .expect("attempt fails");
    finalize_failed_attempt(&mut failing, failing_attempt);

    let successful_retry = successful
        .recovery_succeeded(successful_attempt)
        .expect("successful recovery schedules retry");
    let failing_retry = failing
        .recovery_failed(failing_attempt)
        .expect("failed recovery is absorbed");

    assert_eq!(successful_retry, failing_retry);
    assert_eq!(successful.state(), failing.state());
    assert_eq!(successful.next_retry_index(), 1);
}

#[test]
fn retry_delay_progresses_exponentially_then_caps() {
    let expected = [100_u64, 200, 400, 800, 1_600, 3_200, 5_000, 5_000];

    for (retry_index, expected_ms) in expected.into_iter().enumerate() {
        assert_eq!(
            retry_delay(retry_index as u64),
            Duration::from_millis(expected_ms),
            "retry index {retry_index}"
        );
    }
}

#[test]
fn retry_delay_is_safe_at_integer_extremes() {
    for retry_index in [u64::MAX - 1, u64::MAX] {
        assert_eq!(
            retry_delay(retry_index),
            AUTHORITATIVE_SUBSCRIPTION_RETRY_MAX_DELAY,
            "retry index {retry_index} must stay capped"
        );
    }
}

#[test]
fn repeated_attempts_get_fresh_scope_identities_without_a_retry_limit() {
    let (mut policy, first_attempt) = begin_policy();
    let mut attempt = first_attempt;

    for expected_attempt_number in 1..=32_u64 {
        let failed = policy
            .subscribe_failed(attempt)
            .expect("each attempt can fail");
        assert_eq!(
            failed.state,
            State::FinalizingScope {
                attempt,
                failure: Failure::Subscribe,
            }
        );
        finalize_failed_attempt(&mut policy, attempt);
        policy
            .recovery_failed(attempt)
            .expect("recovery failure is absorbed");

        let retry = policy
            .retry_ready(attempt)
            .expect("retry remains available");
        let Some(Action::StartAttempt { attempt: next }) = retry.action else {
            panic!("retry must start a fresh attempt");
        };
        assert_eq!(next.attempt_id().get(), expected_attempt_number + 1);
        assert_eq!(next.scope_id().get(), expected_attempt_number + 1);
        assert_ne!(next.attempt_id(), attempt.attempt_id());
        assert_ne!(next.scope_id(), attempt.scope_id());
        attempt = next;
    }

    assert_eq!(policy.current_attempt(), Some(attempt));
    assert_eq!(policy.next_retry_index(), 32);
}

#[test]
fn conversation_policy_is_an_alias_of_the_authoritative_policy() {
    let mut conversation: ConversationSubscriptionPolicy = Policy::new();
    let authoritative = conversation.begin_attempt().expect("alias starts equally");

    assert_eq!(
        authoritative.state.kind(),
        authoritative_subscription_policy::AuthoritativeSubscriptionStateKind::Subscribing
    );
    let _: ConversationSubscriptionState = authoritative.state;
    let _: Option<ConversationSubscriptionAction> = authoritative.action;
    let Some(Action::StartAttempt { attempt }) = authoritative.action else {
        panic!("alias must start an attempt");
    };
    let _: ConversationSubscriptionAttempt = attempt;

    let mut run_named: RunConversationSubscriptionPolicy = Policy::new();
    assert_eq!(run_named.state(), State::Ready);
    assert!(run_named.begin_attempt().is_ok());
}
