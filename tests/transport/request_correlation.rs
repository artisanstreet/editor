//! Per-session request-correlation registry coverage.

use std::error::Error;

use artisan_domain::{IdentifierError, RequestId, ThreadId};
use artisan_protocol::{
    ConversationSubscriptionStopped, ErrorCode, ErrorDetail, ProtocolFailure, ProtocolValueError,
    ResponsePayload, ServerResponse,
};
use artisan_transport::{RequestCorrelationError, RequestCorrelationRegistry};

const _: fn() = || {
    struct CloneMarker;
    trait AmbiguousIfClone<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<CloneMarker> for T {}
    let _ = <RequestCorrelationRegistry as AmbiguousIfClone<_>>::marker;
};

const _: fn() = || {
    struct CopyMarker;
    trait AmbiguousIfCopy<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfCopy<()> for T {}
    impl<T: Copy> AmbiguousIfCopy<CopyMarker> for T {}
    let _ = <RequestCorrelationRegistry as AmbiguousIfCopy<_>>::marker;
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
fn zero_pending_limit_is_rejected_at_construction() {
    assert_eq!(
        RequestCorrelationRegistry::new(0, 1),
        Err(RequestCorrelationError::ZeroPendingLimit)
    );
    assert_eq!(
        RequestCorrelationRegistry::new(0, 0),
        Err(RequestCorrelationError::ZeroPendingLimit),
        "the pending limit is diagnosed first"
    );
}

#[test]
fn zero_lifetime_budget_is_rejected_at_construction() {
    assert_eq!(
        RequestCorrelationRegistry::new(1, 0),
        Err(RequestCorrelationError::ZeroLifetimeBudget)
    );
}

#[test]
fn fresh_registry_starts_empty_within_limits() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 4)?;
    assert_eq!(registry.pending_capacity(), 2);
    assert_eq!(registry.admission_budget(), 4);
    assert_eq!(registry.admitted(), 0);
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    assert_eq!(registry.pending().count(), 0);

    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.admitted(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn response_completes_the_registered_request() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 4)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    registry.complete_on_response(&response_settling("request-1")?)?;
    assert!(registry.is_empty());
    assert!(!registry.is_pending(&sent));
    assert_eq!(
        registry.admitted(),
        1,
        "a completed identity stays remembered for the owner's lifetime"
    );
    Ok(())
}

#[test]
fn correlated_failure_completes_the_registered_request() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 4)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let settled = failure_settling(Some(sent.clone()))?;
    registry.complete_on_failure(&settled)?;
    assert!(registry.is_empty());
    assert!(!registry.is_pending(&sent));
    assert_eq!(registry.admitted(), 1);
    Ok(())
}

#[test]
fn uncorrelated_failure_preserves_pending_state() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 4)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let uncorrelated = failure_settling(None)?;
    let rejected = registry.complete_on_failure(&uncorrelated);
    assert_eq!(rejected, Err(RequestCorrelationError::Uncorrelated));
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.admitted(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn duplicate_admission_is_rejected_without_mutation() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 4)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let rejected = registry.register(sent.clone());
    assert_eq!(rejected, Err(RequestCorrelationError::Duplicate));
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.admitted(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn unknown_response_completion_preserves_pending_state() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 4)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let unknown_response = response_settling("request-other")?;
    let rejected = registry.complete_on_response(&unknown_response);
    assert_eq!(rejected, Err(RequestCorrelationError::Unknown));
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.admitted(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn unknown_failure_completion_preserves_pending_state() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 4)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let unknown_failure = failure_settling(Some(request_id("request-other")?))?;
    let rejected = registry.complete_on_failure(&unknown_failure);
    assert_eq!(rejected, Err(RequestCorrelationError::Unknown));
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.admitted(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn settled_response_retires_the_identity_against_readmission() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 2)?;
    let settled = request_id("request-1")?;
    registry.register(settled.clone())?;
    registry.complete_on_response(&response_settling("request-1")?)?;

    let rejected = registry.register(settled);
    assert_eq!(rejected, Err(RequestCorrelationError::Retired));
    assert!(registry.is_empty());
    assert_eq!(registry.admitted(), 1);
    Ok(())
}

#[test]
fn settled_failure_retires_the_identity_against_readmission() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 2)?;
    let settled = request_id("request-1")?;
    registry.register(settled.clone())?;

    let settled_failure = failure_settling(Some(settled.clone()))?;
    registry.complete_on_failure(&settled_failure)?;

    let rejected = registry.register(settled);
    assert_eq!(rejected, Err(RequestCorrelationError::Retired));
    assert!(registry.is_empty());
    assert_eq!(registry.admitted(), 1);
    Ok(())
}

#[test]
fn late_completions_for_a_retired_identity_reject_without_disturbance() -> Result<(), Box<dyn Error>>
{
    let mut registry = RequestCorrelationRegistry::new(2, 3)?;
    let retired = request_id("request-1")?;
    registry.register(retired.clone())?;
    registry.complete_on_response(&response_settling("request-1")?)?;

    let fresh = request_id("request-2")?;
    registry.register(fresh.clone())?;

    // Owned snapshots survive across the mutable calls below.
    let observed = |registry: &RequestCorrelationRegistry| {
        (
            registry.len(),
            registry.admitted(),
            registry.is_pending(&retired),
            registry.is_pending(&fresh),
            registry
                .pending()
                .map(|id| id.as_str().to_string())
                .collect::<Vec<_>>(),
        )
    };
    let before = observed(&registry);

    let late_response = response_settling("request-1")?;
    assert_eq!(
        registry.complete_on_response(&late_response),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(observed(&registry), before);

    let late_failure = failure_settling(Some(retired.clone()))?;
    assert_eq!(
        registry.complete_on_failure(&late_failure),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(observed(&registry), before);
    Ok(())
}

#[test]
fn completion_frees_capacity_only_for_a_fresh_identity() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 3)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    registry.register(first.clone())?;
    registry.register(second.clone())?;

    let first_failure = failure_settling(Some(first.clone()))?;
    registry.complete_on_failure(&first_failure)?;

    let replacement = request_id("request-3")?;
    registry.register(replacement.clone())?;
    assert_eq!(registry.len(), 2);
    assert!(registry.is_pending(&replacement));

    let readmitted = registry.register(first);
    assert_eq!(
        readmitted,
        Err(RequestCorrelationError::Retired),
        "freed capacity admits only a fresh identity"
    );
    assert_eq!(registry.len(), 2);

    registry.complete_on_response(&response_settling("request-2")?)?;
    registry.complete_on_response(&response_settling("request-3")?)?;
    assert!(registry.is_empty());
    assert_eq!(registry.admitted(), 3);
    Ok(())
}

#[test]
fn exhausted_lifetime_budget_rejects_after_all_requests_settle() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 2)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    registry.register(first.clone())?;
    registry.register(second.clone())?;
    registry.complete_on_response(&response_settling("request-1")?)?;

    let second_failure = failure_settling(Some(second.clone()))?;
    registry.complete_on_failure(&second_failure)?;
    assert!(registry.is_empty());

    let exhausted = registry.register(request_id("request-3")?);
    assert_eq!(
        exhausted,
        Err(RequestCorrelationError::LifetimeExhausted { budget: 2 })
    );
    assert!(registry.is_empty());
    assert_eq!(registry.admitted(), 2);

    let evicted_first = registry.register(first);
    assert_eq!(
        evicted_first,
        Err(RequestCorrelationError::Retired),
        "exhaustion never evicts a remembered identity back into eligibility"
    );

    let evicted_second = registry.register(second);
    assert_eq!(
        evicted_second,
        Err(RequestCorrelationError::Retired),
        "an identity retired by a correlated failure stays ineligible too"
    );
    assert!(registry.is_empty());
    assert_eq!(registry.admitted(), 2);

    let replayed = response_settling("request-1")?;
    assert_eq!(
        registry.complete_on_response(&replayed),
        Err(RequestCorrelationError::Unknown)
    );
    assert!(registry.is_empty());
    Ok(())
}

#[test]
fn duplicate_precedes_capacity_and_lifetime_rejection() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(1, 1)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let rejected = registry.register(sent);
    assert_eq!(
        rejected,
        Err(RequestCorrelationError::Duplicate),
        "duplicate diagnosis holds while every limit is full"
    );
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.admitted(), 1);
    Ok(())
}

#[test]
fn retired_precedes_capacity_and_lifetime_rejection() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(1, 2)?;
    let settled = request_id("request-1")?;
    registry.register(settled.clone())?;
    registry.complete_on_response(&response_settling("request-1")?)?;
    registry.register(request_id("request-2")?)?;
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.admitted(), 2);

    let rejected_retired = registry.register(settled);
    assert_eq!(
        rejected_retired,
        Err(RequestCorrelationError::Retired),
        "retirement is diagnosed even with pending capacity full and lifetime budget exhausted"
    );

    let rejected_fresh = registry.register(request_id("request-3")?);
    assert_eq!(
        rejected_fresh,
        Err(RequestCorrelationError::LifetimeExhausted { budget: 2 }),
        "a fresh identity still receives the budget diagnosis"
    );
    Ok(())
}

#[test]
fn every_rejection_leaves_the_registry_unchanged() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2, 3)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    registry.register(first.clone())?;
    registry.register(second.clone())?;

    // A twin advanced only by the same successful operations proves the
    // FULL state survives every rejection: derived equality compares both
    // limits, every remembered identity behind retirement, and pending
    // order — counts alone cannot prove the remembered set unchanged.
    let mut expected = RequestCorrelationRegistry::new(2, 3)?;
    expected.register(first.clone())?;
    expected.register(second.clone())?;
    assert_eq!(registry, expected);

    assert_eq!(
        registry.register(first.clone()),
        Err(RequestCorrelationError::Duplicate)
    );
    assert_eq!(registry, expected);

    assert_eq!(
        registry.register(request_id("request-9")?),
        Err(RequestCorrelationError::AtCapacity { capacity: 2 }),
        "a fresh id hits transient pending capacity before the unspent budget"
    );
    assert_eq!(registry, expected);

    let unknown_response = response_settling("request-9")?;
    assert_eq!(
        registry.complete_on_response(&unknown_response),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(registry, expected);

    let uncorrelated = failure_settling(None)?;
    assert_eq!(
        registry.complete_on_failure(&uncorrelated),
        Err(RequestCorrelationError::Uncorrelated)
    );
    assert_eq!(registry, expected);

    let unknown_failure = failure_settling(Some(request_id("request-9")?))?;
    assert_eq!(
        registry.complete_on_failure(&unknown_failure),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(registry, expected);

    assert_eq!(
        RequestCorrelationRegistry::new(0, 3),
        Err(RequestCorrelationError::ZeroPendingLimit)
    );
    assert_eq!(
        RequestCorrelationRegistry::new(2, 0),
        Err(RequestCorrelationError::ZeroLifetimeBudget)
    );
    assert_eq!(registry, expected);

    // Spend the remaining budget so later rejections cover both limits
    // being exhausted at once.
    let settling_first = response_settling("request-1")?;
    registry.complete_on_response(&settling_first)?;
    expected.complete_on_response(&settling_first)?;
    let third = request_id("request-3")?;
    registry.register(third)?;
    expected.register(request_id("request-3")?)?;
    assert_eq!(registry.admitted(), 3);
    assert_eq!(registry.len(), 2);
    assert_eq!(registry, expected);

    assert_eq!(
        registry.register(request_id("request-9")?),
        Err(RequestCorrelationError::LifetimeExhausted { budget: 3 })
    );
    assert_eq!(registry, expected);

    assert_eq!(
        registry.register(first.clone()),
        Err(RequestCorrelationError::Retired)
    );
    assert_eq!(registry, expected);

    let retired_failure = failure_settling(Some(first))?;
    assert_eq!(
        registry.complete_on_failure(&retired_failure),
        Err(RequestCorrelationError::Unknown),
        "a retired completion stays unknown after its settlement"
    );
    assert_eq!(
        registry, expected,
        "final structural equality pins both limits, pending order, and every remembered identity"
    );
    Ok(())
}
