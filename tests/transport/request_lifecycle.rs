//! Client-side request-to-waiter lifecycle coverage.

use std::error::Error;

use artisan_domain::{IdentifierError, RequestId, ThreadId};
use artisan_protocol::{
    ConversationSubscriptionStopped, ErrorCode, ErrorDetail, ProtocolFailure, ProtocolValueError,
    ResponsePayload, ServerResponse,
};
use artisan_transport::{
    ClientRequestLifecycle, OutcomeDelivery, OutcomeWaiter, OutcomeWaiterError,
    RequestLifecycleError, RequestOutcome,
};

const _: fn() = || {
    struct CloneMarker;
    trait AmbiguousIfClone<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<CloneMarker> for T {}
    let _ = <ClientRequestLifecycle as AmbiguousIfClone<_>>::marker;
    let _ = <OutcomeWaiter as AmbiguousIfClone<_>>::marker;
};

const _: fn() = || {
    struct CopyMarker;
    trait AmbiguousIfCopy<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfCopy<()> for T {}
    impl<T: Copy> AmbiguousIfCopy<CopyMarker> for T {}
    let _ = <ClientRequestLifecycle as AmbiguousIfCopy<_>>::marker;
    let _ = <OutcomeWaiter as AmbiguousIfCopy<_>>::marker;
};

fn request_id(value: &str) -> Result<RequestId, IdentifierError> {
    RequestId::parse(value)
}

fn thread_id(value: &str) -> Result<ThreadId, IdentifierError> {
    ThreadId::parse(value)
}

/// Minimal successful response whose payload carries no request data.
fn response_settling(request: &str) -> Result<ServerResponse, IdentifierError> {
    Ok(ServerResponse {
        request_id: request_id(request)?,
        payload: ResponsePayload::ConversationSubscriptionStopped(
            ConversationSubscriptionStopped {
                thread_id: thread_id("fixture-thread")?,
            },
        ),
    })
}

/// Minimal typed failure settling `request`, if any.
fn failure_settling(request: Option<RequestId>) -> Result<ProtocolFailure, ProtocolValueError> {
    Ok(ProtocolFailure {
        code: ErrorCode::Internal,
        detail: ErrorDetail::parse("fixture failure evidence")?,
        retryable: true,
        request_id: request,
    })
}

#[test]
fn zero_capacity_is_rejected_at_construction() {
    assert_eq!(
        ClientRequestLifecycle::new(0).err(),
        Some(RequestLifecycleError::ZeroCapacity)
    );
}

#[test]
fn fresh_lifecycle_admits_a_single_owner_waiter() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2)?;
    assert_eq!(lifecycle.capacity(), 2);
    assert!(lifecycle.is_empty());
    assert_eq!(lifecycle.len(), 0);

    let sent = request_id("request-1")?;
    let waiter = lifecycle.admit(sent.clone())?;
    assert_eq!(waiter.request_id(), &sent);
    assert!(!waiter.is_settled());
    assert!(!lifecycle.is_empty());
    assert_eq!(lifecycle.len(), 1);
    assert!(lifecycle.is_pending(&sent));
    Ok(())
}

#[test]
fn duplicate_admission_is_rejected_without_mutation() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2)?;
    let sent = request_id("request-1")?;
    let mut waiter = lifecycle.admit(sent.clone())?;

    let rejected = lifecycle.admit(sent.clone());
    assert_eq!(rejected.err(), Some(RequestLifecycleError::Duplicate));
    assert_eq!(lifecycle.len(), 1);
    assert!(lifecycle.is_pending(&sent));

    let settled = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&settled),
        Ok(OutcomeDelivery::Delivered)
    );
    let resolved = waiter.take_outcome()?;
    assert_eq!(resolved.request_id().as_str(), "request-1");
    Ok(())
}

#[test]
fn full_lifecycle_rejects_admission_without_mutation() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    let _first_waiter = lifecycle.admit(first.clone())?;
    let _second_waiter = lifecycle.admit(second.clone())?;

    let rejected = lifecycle.admit(request_id("request-3")?);
    assert_eq!(
        rejected.err(),
        Some(RequestLifecycleError::AtCapacity { capacity: 2 })
    );
    assert_eq!(lifecycle.capacity(), 2);
    assert_eq!(lifecycle.len(), 2);
    assert!(lifecycle.is_pending(&first));
    assert!(lifecycle.is_pending(&second));
    Ok(())
}

#[test]
fn response_resolves_exactly_its_own_request() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2)?;
    let mut settled_waiter = lifecycle.admit(request_id("request-1")?)?;
    let mut other_waiter = lifecycle.admit(request_id("request-2")?)?;

    let settled = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&settled),
        Ok(OutcomeDelivery::Delivered)
    );
    assert!(!lifecycle.is_pending(&request_id("request-1")?));
    assert!(lifecycle.is_pending(&request_id("request-2")?));

    let (resolved_id, outcome) = settled_waiter.take_outcome()?.into_parts();
    assert_eq!(resolved_id.as_str(), "request-1");
    assert_eq!(outcome, RequestOutcome::Response(settled));
    assert_eq!(
        other_waiter.take_outcome(),
        Err(OutcomeWaiterError::NotSettled)
    );
    Ok(())
}

#[test]
fn correlated_failure_resolves_exactly_its_own_request() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2)?;
    let mut settled_waiter = lifecycle.admit(request_id("request-1")?)?;
    let _other_waiter = lifecycle.admit(request_id("request-2")?)?;

    let failure = failure_settling(Some(request_id("request-1")?))?;
    assert_eq!(
        lifecycle.resolve_on_failure(&failure),
        Ok(OutcomeDelivery::Delivered)
    );

    let (resolved_id, outcome) = settled_waiter.take_outcome()?.into_parts();
    assert_eq!(resolved_id.as_str(), "request-1");
    assert_eq!(outcome, RequestOutcome::Failure(failure));
    assert_eq!(lifecycle.len(), 1);
    Ok(())
}

#[test]
fn uncorrelated_failure_preserves_pending_state() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(1)?;
    let sent = request_id("request-1")?;
    let mut waiter = lifecycle.admit(sent.clone())?;

    let rejected = lifecycle.resolve_on_failure(&failure_settling(None)?);
    assert_eq!(rejected, Err(RequestLifecycleError::Uncorrelated));
    assert_eq!(lifecycle.len(), 1);
    assert!(lifecycle.is_pending(&sent));
    assert_eq!(waiter.take_outcome(), Err(OutcomeWaiterError::NotSettled));
    Ok(())
}

#[test]
fn unknown_and_replayed_completions_reject_without_disturbance() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2)?;
    let sent = request_id("request-1")?;
    let mut waiter = lifecycle.admit(sent.clone())?;
    let _other = lifecycle.admit(request_id("request-2")?)?;

    let replayed = response_settling("request-9")?;
    assert_eq!(
        lifecycle.resolve_on_response(&replayed),
        Err(RequestLifecycleError::Unknown)
    );
    let unknown_failure = failure_settling(Some(request_id("request-9")?))?;
    assert_eq!(
        lifecycle.resolve_on_failure(&unknown_failure),
        Err(RequestLifecycleError::Unknown)
    );
    assert_eq!(lifecycle.len(), 2);
    assert!(lifecycle.is_pending(&sent));
    assert_eq!(waiter.take_outcome(), Err(OutcomeWaiterError::NotSettled));
    Ok(())
}

#[test]
fn completion_after_settlement_is_a_replay_rejection() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(1)?;
    let waiter = lifecycle.admit(request_id("request-1")?)?;
    assert_eq!(
        lifecycle.resolve_on_response(&response_settling("request-1")?),
        Ok(OutcomeDelivery::Delivered)
    );
    drop(waiter);

    assert_eq!(
        lifecycle.resolve_on_response(&response_settling("request-1")?),
        Err(RequestLifecycleError::Unknown)
    );
    assert!(lifecycle.is_empty());
    Ok(())
}

#[test]
fn completion_after_waiter_drop_settles_exactly_once_and_frees() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(1)?;
    drop(lifecycle.admit(request_id("request-1")?)?);

    let late = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&late),
        Ok(OutcomeDelivery::Abandoned)
    );
    assert!(lifecycle.is_empty());

    let readmitted = request_id("request-1")?;
    let replacement = lifecycle.admit(readmitted.clone())?;
    assert_eq!(replacement.request_id(), &readmitted);
    Ok(())
}

#[test]
fn abandoned_completion_does_not_disturb_other_waiters() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2)?;
    drop(lifecycle.admit(request_id("request-1")?)?);
    let mut live = lifecycle.admit(request_id("request-2")?)?;

    assert_eq!(
        lifecycle.resolve_on_response(&response_settling("request-1")?),
        Ok(OutcomeDelivery::Abandoned)
    );
    assert_eq!(
        lifecycle.resolve_on_failure(&failure_settling(Some(request_id("request-2")?))?),
        Ok(OutcomeDelivery::Delivered)
    );
    let (resolved_id, outcome) = live.take_outcome()?.into_parts();
    assert_eq!(resolved_id.as_str(), "request-2");
    assert!(matches!(outcome, RequestOutcome::Failure(_)));
    assert!(lifecycle.is_empty());
    Ok(())
}

#[test]
fn waiter_delivers_at_most_one_outcome() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(1)?;
    let mut waiter = lifecycle.admit(request_id("request-1")?)?;
    assert_eq!(
        waiter.take_outcome(),
        Err(OutcomeWaiterError::NotSettled),
        "polling before settlement keeps the waiter usable"
    );

    lifecycle.resolve_on_response(&response_settling("request-1")?)?;
    assert!(waiter.is_settled());
    let first = waiter.take_outcome()?;
    assert_eq!(first.request_id().as_str(), "request-1");
    assert_eq!(waiter.take_outcome(), Err(OutcomeWaiterError::AlreadyTaken));
    assert!(lifecycle.is_empty());
    Ok(())
}

#[test]
fn resolution_frees_capacity_for_reuse_in_order() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2)?;
    let _first = lifecycle.admit(request_id("request-1")?)?;
    let _second = lifecycle.admit(request_id("request-2")?)?;
    lifecycle.resolve_on_response(&response_settling("request-1")?)?;

    let third = lifecycle.admit(request_id("request-3")?)?;
    assert_eq!(third.request_id().as_str(), "request-3");
    assert_eq!(lifecycle.len(), 2);

    lifecycle.resolve_on_response(&response_settling("request-2")?)?;
    lifecycle.resolve_on_response(&response_settling("request-3")?)?;
    assert!(lifecycle.is_empty());
    Ok(())
}

#[test]
fn every_rejection_leaves_the_lifecycle_unchanged() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    let mut first_waiter = lifecycle.admit(first.clone())?;
    let _second_waiter = lifecycle.admit(second.clone())?;

    let observed = |lifecycle: &ClientRequestLifecycle| {
        (
            lifecycle.capacity(),
            lifecycle.len(),
            lifecycle.is_pending(&first),
            lifecycle.is_pending(&second),
        )
    };
    let before = observed(&lifecycle);

    assert_eq!(
        lifecycle.admit(first.clone()).err(),
        Some(RequestLifecycleError::Duplicate)
    );
    assert_eq!(observed(&lifecycle), before);

    assert_eq!(
        lifecycle.admit(request_id("request-9")?).err(),
        Some(RequestLifecycleError::AtCapacity { capacity: 2 })
    );
    assert_eq!(observed(&lifecycle), before);

    assert_eq!(
        lifecycle.resolve_on_response(&response_settling("request-9")?),
        Err(RequestLifecycleError::Unknown)
    );
    assert_eq!(observed(&lifecycle), before);

    assert_eq!(
        lifecycle.resolve_on_failure(&failure_settling(None)?),
        Err(RequestLifecycleError::Uncorrelated)
    );
    assert_eq!(observed(&lifecycle), before);

    assert_eq!(
        lifecycle.resolve_on_failure(&failure_settling(Some(request_id("request-9")?))?),
        Err(RequestLifecycleError::Unknown)
    );
    assert_eq!(observed(&lifecycle), before);

    assert_eq!(
        ClientRequestLifecycle::new(0).err(),
        Some(RequestLifecycleError::ZeroCapacity)
    );

    assert_eq!(observed(&lifecycle), before);
    assert_eq!(
        first_waiter.take_outcome(),
        Err(OutcomeWaiterError::NotSettled)
    );
    Ok(())
}
