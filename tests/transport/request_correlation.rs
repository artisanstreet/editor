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
fn zero_capacity_is_rejected_at_construction() {
    assert_eq!(
        RequestCorrelationRegistry::new(0),
        Err(RequestCorrelationError::ZeroCapacity)
    );
}

#[test]
fn fresh_registry_starts_empty_within_capacity() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    assert_eq!(registry.capacity(), 2);
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);
    assert_eq!(registry.pending().count(), 0);

    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn response_completes_the_registered_request() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    registry.complete_on_response(&response_settling("request-1")?)?;
    assert!(registry.is_empty());
    assert!(!registry.is_pending(&sent));
    Ok(())
}

#[test]
fn correlated_failure_completes_the_registered_request() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    registry.complete_on_failure(&failure_settling(Some(sent.clone()))?)?;
    assert!(registry.is_empty());
    assert!(!registry.is_pending(&sent));
    Ok(())
}

#[test]
fn uncorrelated_failure_preserves_pending_state() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let rejected = registry.complete_on_failure(&failure_settling(None)?);
    assert_eq!(rejected, Err(RequestCorrelationError::Uncorrelated));
    assert_eq!(registry.len(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn duplicate_admission_is_rejected_without_mutation() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let rejected = registry.register(sent.clone());
    assert_eq!(rejected, Err(RequestCorrelationError::Duplicate));
    assert_eq!(registry.len(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn unknown_response_completion_preserves_pending_state() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let rejected = registry.complete_on_response(&response_settling("request-other")?);
    assert_eq!(rejected, Err(RequestCorrelationError::Unknown));
    assert_eq!(registry.len(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn unknown_failure_completion_preserves_pending_state() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    let sent = request_id("request-1")?;
    registry.register(sent.clone())?;

    let rejected =
        registry.complete_on_failure(&failure_settling(Some(request_id("request-other")?))?);
    assert_eq!(rejected, Err(RequestCorrelationError::Unknown));
    assert_eq!(registry.len(), 1);
    assert!(registry.is_pending(&sent));
    Ok(())
}

#[test]
fn duplicate_completion_is_unknown_and_preserves_freed_capacity() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    registry.register(request_id("request-1")?)?;
    registry.complete_on_response(&response_settling("request-1")?)?;

    let replayed = registry.complete_on_response(&response_settling("request-1")?);
    assert_eq!(replayed, Err(RequestCorrelationError::Unknown));
    assert!(registry.is_empty());
    Ok(())
}

#[test]
fn full_registry_rejects_admission_without_mutation() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    registry.register(first.clone())?;
    registry.register(second.clone())?;

    let rejected = registry.register(request_id("request-3")?);
    assert_eq!(
        rejected,
        Err(RequestCorrelationError::AtCapacity { capacity: 2 })
    );
    assert_eq!(registry.capacity(), 2);
    assert_eq!(registry.len(), 2);
    assert!(registry.is_pending(&first));
    assert!(registry.is_pending(&second));
    let pending = registry
        .pending()
        .map(RequestId::as_str)
        .collect::<Vec<_>>();
    assert_eq!(pending, ["request-1", "request-2"]);
    Ok(())
}

#[test]
fn completion_frees_capacity_for_reuse() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    registry.register(request_id("request-1")?)?;
    registry.register(request_id("request-2")?)?;

    registry.complete_on_failure(&failure_settling(Some(request_id("request-1")?))?)?;
    let replacement = request_id("request-3")?;
    registry.register(replacement.clone())?;
    assert_eq!(registry.len(), 2);
    assert!(registry.is_pending(&replacement));

    registry.complete_on_response(&response_settling("request-2")?)?;
    registry.complete_on_response(&response_settling("request-3")?)?;
    assert!(registry.is_empty());
    Ok(())
}

#[test]
fn every_rejection_leaves_the_registry_unchanged() -> Result<(), Box<dyn Error>> {
    let mut registry = RequestCorrelationRegistry::new(2)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    registry.register(first.clone())?;
    registry.register(second.clone())?;

    let observed = |registry: &RequestCorrelationRegistry| {
        (
            registry.capacity(),
            registry.len(),
            registry.is_pending(&first),
            registry.is_pending(&second),
            registry.pending().count(),
        )
    };
    let before = observed(&registry);

    assert_eq!(
        registry.register(first.clone()),
        Err(RequestCorrelationError::Duplicate)
    );
    assert_eq!(observed(&registry), before);

    assert_eq!(
        registry.register(request_id("request-9")?),
        Err(RequestCorrelationError::AtCapacity { capacity: 2 })
    );
    assert_eq!(observed(&registry), before);

    assert_eq!(
        registry.complete_on_response(&response_settling("request-9")?),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(observed(&registry), before);

    assert_eq!(
        registry.complete_on_failure(&failure_settling(None)?),
        Err(RequestCorrelationError::Uncorrelated)
    );
    assert_eq!(observed(&registry), before);

    assert_eq!(
        registry.complete_on_failure(&failure_settling(Some(request_id("request-9")?))?),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(observed(&registry), before);

    assert_eq!(
        RequestCorrelationRegistry::new(0),
        Err(RequestCorrelationError::ZeroCapacity)
    );

    assert_eq!(observed(&registry), before);
    Ok(())
}
