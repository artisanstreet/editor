//! Client-side request-to-waiter lifecycle coverage.

use std::{error::Error, sync::mpsc, thread, time::Duration};

use artisan_domain::{IdentifierError, RequestId, ThreadId};
use artisan_protocol::{
    ConversationSubscriptionStopped, ErrorCode, ErrorDetail, ProtocolFailure, ProtocolValueError,
    ResponsePayload, ServerResponse,
};
use artisan_transport::{
    ClientRequestLifecycle, OutcomeDelivery, OutcomeWaiter, OutcomeWaiterError,
    RequestCorrelationError, RequestOutcome, ResolvedRequest,
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
fn zero_pending_limit_is_rejected_at_construction() {
    assert_eq!(
        ClientRequestLifecycle::new(0, 1).err(),
        Some(RequestCorrelationError::ZeroPendingLimit)
    );
    assert_eq!(
        ClientRequestLifecycle::new(0, 0).err(),
        Some(RequestCorrelationError::ZeroPendingLimit),
        "the pending limit is diagnosed first"
    );
}

#[test]
fn zero_lifetime_budget_is_rejected_at_construction() {
    assert_eq!(
        ClientRequestLifecycle::new(1, 0).err(),
        Some(RequestCorrelationError::ZeroLifetimeBudget)
    );
}

#[test]
fn fresh_lifecycle_admits_a_single_owner_waiter() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 4)?;
    assert_eq!(lifecycle.pending_capacity(), 2);
    assert_eq!(lifecycle.admission_budget(), 4);
    assert_eq!(lifecycle.admitted(), 0);
    assert!(lifecycle.is_empty());
    assert_eq!(lifecycle.len(), 0);

    let sent = request_id("request-1")?;
    let waiter = lifecycle.admit(sent.clone())?;
    assert_eq!(waiter.request_id(), &sent);
    assert!(!waiter.is_settled());
    assert!(!lifecycle.is_empty());
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(lifecycle.admitted(), 1);
    assert!(lifecycle.is_pending(&sent));
    Ok(())
}

#[test]
fn duplicate_admission_is_rejected_without_mutation() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 4)?;
    let sent = request_id("request-1")?;
    let mut waiter = lifecycle.admit(sent.clone())?;

    let rejected = lifecycle.admit(sent.clone());
    assert_eq!(rejected.err(), Some(RequestCorrelationError::Duplicate));
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(lifecycle.admitted(), 1);
    assert!(lifecycle.is_pending(&sent));

    let settled = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&settled),
        Ok(OutcomeDelivery::Delivered),
        "the original waiter keeps its exactly-one delivery after a rejected duplicate"
    );
    let resolved = waiter.take_outcome()?;
    assert_eq!(resolved.request_id().as_str(), "request-1");
    Ok(())
}

#[test]
fn full_lifecycle_rejects_admission_without_mutation() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 4)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    let _first_waiter = lifecycle.admit(first.clone())?;
    let _second_waiter = lifecycle.admit(second.clone())?;

    let rejected = lifecycle.admit(request_id("request-3")?);
    assert_eq!(
        rejected.err(),
        Some(RequestCorrelationError::AtCapacity { capacity: 2 })
    );
    assert_eq!(lifecycle.pending_capacity(), 2);
    assert_eq!(lifecycle.len(), 2);
    assert_eq!(lifecycle.admitted(), 2);
    assert!(lifecycle.is_pending(&first));
    assert!(lifecycle.is_pending(&second));
    Ok(())
}

#[test]
fn response_resolves_exactly_its_own_request() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 4)?;
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
    let mut lifecycle = ClientRequestLifecycle::new(2, 4)?;
    let mut settled_waiter = lifecycle.admit(request_id("request-1")?)?;
    let _other_waiter = lifecycle.admit(request_id("request-2")?)?;

    let settled_failure = failure_settling(Some(request_id("request-1")?))?;
    assert_eq!(
        lifecycle.resolve_on_failure(&settled_failure),
        Ok(OutcomeDelivery::Delivered)
    );

    let (resolved_id, outcome) = settled_waiter.take_outcome()?.into_parts();
    assert_eq!(resolved_id.as_str(), "request-1");
    assert_eq!(outcome, RequestOutcome::Failure(settled_failure));
    assert_eq!(lifecycle.len(), 1);
    Ok(())
}

#[test]
fn uncorrelated_failure_preserves_pending_state() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(1, 2)?;
    let sent = request_id("request-1")?;
    let mut waiter = lifecycle.admit(sent.clone())?;

    let uncorrelated = failure_settling(None)?;
    let rejected = lifecycle.resolve_on_failure(&uncorrelated);
    assert_eq!(rejected, Err(RequestCorrelationError::Uncorrelated));
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(lifecycle.admitted(), 1);
    assert!(lifecycle.is_pending(&sent));
    assert_eq!(waiter.take_outcome(), Err(OutcomeWaiterError::NotSettled));
    Ok(())
}

#[test]
fn unknown_and_replayed_completions_reject_without_disturbance() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 4)?;
    let sent = request_id("request-1")?;
    let mut waiter = lifecycle.admit(sent.clone())?;
    let _other = lifecycle.admit(request_id("request-2")?)?;

    let replayed_response = response_settling("request-9")?;
    assert_eq!(
        lifecycle.resolve_on_response(&replayed_response),
        Err(RequestCorrelationError::Unknown)
    );
    let unknown_failure = failure_settling(Some(request_id("request-9")?))?;
    assert_eq!(
        lifecycle.resolve_on_failure(&unknown_failure),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(lifecycle.len(), 2);
    assert_eq!(lifecycle.admitted(), 2);
    assert!(lifecycle.is_pending(&sent));
    assert_eq!(waiter.take_outcome(), Err(OutcomeWaiterError::NotSettled));
    Ok(())
}

#[test]
fn completion_after_settlement_is_a_replay_rejection() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(1, 2)?;
    let waiter = lifecycle.admit(request_id("request-1")?)?;
    assert_eq!(
        lifecycle.resolve_on_response(&response_settling("request-1")?),
        Ok(OutcomeDelivery::Delivered)
    );
    drop(waiter);

    let replayed = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&replayed),
        Err(RequestCorrelationError::Unknown)
    );
    assert!(lifecycle.is_empty());
    assert_eq!(
        lifecycle.admitted(),
        1,
        "the settled identity stays remembered after its retirement"
    );
    Ok(())
}

#[test]
fn completion_after_waiter_drop_settles_once_and_frees_only_a_fresh_identity()
-> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 2)?;
    drop(lifecycle.admit(request_id("request-1")?)?);

    let late = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&late),
        Ok(OutcomeDelivery::Abandoned)
    );
    assert!(lifecycle.is_empty());

    let readmitted = request_id("request-1")?;
    let rejected = lifecycle.admit(readmitted);
    assert_eq!(
        rejected.err(),
        Some(RequestCorrelationError::Retired),
        "freed capacity never restores a retired identity"
    );
    assert!(lifecycle.is_empty());
    assert_eq!(lifecycle.admitted(), 1);

    let fresh = request_id("request-2")?;
    let mut replacement = lifecycle.admit(fresh.clone())?;
    assert_eq!(replacement.request_id(), &fresh);
    assert_eq!(lifecycle.len(), 1);

    lifecycle.resolve_on_response(&response_settling("request-2")?)?;
    let (resolved_id, _) = replacement.take_outcome()?.into_parts();
    assert_eq!(resolved_id.as_str(), "request-2");
    assert!(lifecycle.is_empty());
    Ok(())
}

#[test]
fn late_completions_for_retired_request_reject_without_disturbing_live_waiter()
-> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 3)?;
    let retired = request_id("request-1")?;
    let mut settled_waiter = lifecycle.admit(retired.clone())?;
    let live = request_id("request-2")?;
    let mut live_waiter = lifecycle.admit(live.clone())?;

    let settling_retired = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&settling_retired),
        Ok(OutcomeDelivery::Delivered)
    );
    let (resolved_id, outcome) = settled_waiter.take_outcome()?.into_parts();
    assert_eq!(resolved_id.as_str(), "request-1");
    assert_eq!(outcome, RequestOutcome::Response(settling_retired));

    // Owned snapshots survive across the mutable calls below.
    let observed = |lifecycle: &ClientRequestLifecycle| {
        (
            lifecycle.len(),
            lifecycle.admitted(),
            lifecycle.is_pending(&retired),
            lifecycle.is_pending(&live),
        )
    };
    let before = observed(&lifecycle);

    let late_response = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&late_response),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(observed(&lifecycle), before);

    let late_failure = failure_settling(Some(retired.clone()))?;
    assert_eq!(
        lifecycle.resolve_on_failure(&late_failure),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(observed(&lifecycle), before);

    assert_eq!(
        live_waiter.take_outcome(),
        Err(OutcomeWaiterError::NotSettled),
        "the live waiter keeps waiting, unsettled and undisturbed"
    );
    Ok(())
}

#[test]
fn exhausted_lifetime_budget_rejects_after_everything_settles() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 2)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    drop(lifecycle.admit(first.clone())?);
    drop(lifecycle.admit(second.clone())?);
    lifecycle.resolve_on_response(&response_settling("request-1")?)?;

    let second_failure = failure_settling(Some(second.clone()))?;
    lifecycle.resolve_on_failure(&second_failure)?;
    assert!(lifecycle.is_empty());
    assert_eq!(lifecycle.admitted(), 2);

    let exhausted = lifecycle.admit(request_id("request-3")?);
    assert_eq!(
        exhausted.err(),
        Some(RequestCorrelationError::LifetimeExhausted { budget: 2 })
    );
    assert!(lifecycle.is_empty());
    assert_eq!(lifecycle.admitted(), 2);

    assert_eq!(
        lifecycle.admit(first).err(),
        Some(RequestCorrelationError::Retired),
        "exhaustion never evicts a response-retired identity back into eligibility"
    );
    assert_eq!(
        lifecycle.admit(second).err(),
        Some(RequestCorrelationError::Retired),
        "an identity retired by a correlated failure stays ineligible too"
    );
    assert!(lifecycle.is_empty());
    assert_eq!(lifecycle.admitted(), 2);
    Ok(())
}

#[test]
fn duplicate_and_retired_precede_capacity_and_budget_rejections() -> Result<(), Box<dyn Error>> {
    let mut single = ClientRequestLifecycle::new(1, 1)?;
    let only = request_id("request-1")?;
    single.admit(only.clone())?;
    assert_eq!(
        single.admit(only).err(),
        Some(RequestCorrelationError::Duplicate),
        "duplicate diagnosis holds while every limit is full"
    );

    let mut lifecycle = ClientRequestLifecycle::new(1, 2)?;
    let settled = request_id("request-1")?;
    drop(lifecycle.admit(settled.clone())?);
    lifecycle.resolve_on_response(&response_settling("request-1")?)?;
    let _pending_waiter = lifecycle.admit(request_id("request-2")?)?;
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(lifecycle.admitted(), 2);

    assert_eq!(
        lifecycle.admit(settled).err(),
        Some(RequestCorrelationError::Retired),
        "retirement is diagnosed even with pending capacity full and lifetime budget exhausted"
    );
    assert_eq!(lifecycle.len(), 1);
    assert_eq!(lifecycle.admitted(), 2);

    assert_eq!(
        lifecycle.admit(request_id("request-3")?).err(),
        Some(RequestCorrelationError::LifetimeExhausted { budget: 2 }),
        "a fresh identity still receives the budget diagnosis"
    );
    Ok(())
}

#[test]
fn abandoned_completion_does_not_disturb_other_waiters() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 4)?;
    drop(lifecycle.admit(request_id("request-1")?)?);
    let mut live = lifecycle.admit(request_id("request-2")?)?;

    assert_eq!(
        lifecycle.resolve_on_response(&response_settling("request-1")?),
        Ok(OutcomeDelivery::Abandoned)
    );
    let live_failure = failure_settling(Some(request_id("request-2")?))?;
    assert_eq!(
        lifecycle.resolve_on_failure(&live_failure),
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
    let mut lifecycle = ClientRequestLifecycle::new(1, 2)?;
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
fn post_handoff_receiver_drop_keeps_the_committed_handoff() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(1, 2)?;
    let waiter = lifecycle.admit(request_id("request-1")?)?;

    let settled = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&settled),
        Ok(OutcomeDelivery::Delivered),
        "a live receiver at the handoff point receives a committed Delivered handoff"
    );

    // The receiver drops after the handoff without ever consuming: the
    // committed verdict stands and no correlation state regresses.
    drop(waiter);
    assert!(lifecycle.is_empty());
    assert_eq!(lifecycle.admitted(), 1);

    let replayed = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&replayed),
        Err(RequestCorrelationError::Unknown),
        "the retired identity stays retired whatever its receiver did"
    );
    Ok(())
}

/// Races one consumer take against publication across `trials` fresh
/// lifecycles, using an explicit start handshake and channel completion
/// instead of timed polling. A racing take that finds the slot settled
/// consumes and then drops promptly before any reporting handshake; a take
/// that loses the race synchronizes on the completed handoff and takes
/// exactly once more. Every trial must classify
/// [`OutcomeDelivery::Delivered`] — the waiter exists from spawn until a
/// post-publication drop, so no interleaving can legitimately classify
/// otherwise — and this bounded harness cannot exhaustively prove every
/// schedule, which remains the source-level ordering's role.
fn raced_consume_then_drop_implies_delivered(
    trials: u32,
    mut settle: impl FnMut(
        &mut ClientRequestLifecycle,
    ) -> Result<OutcomeDelivery, RequestCorrelationError>,
) -> Result<Vec<ResolvedRequest>, Box<dyn Error>> {
    // Generous failure bound for the fallback synchronization only; it
    // never manufactures a schedule.
    const CONSUMER_PATIENCE: Duration = Duration::from_secs(30);

    let mut delivered = Vec::new();
    for _ in 0..trials {
        let mut lifecycle = ClientRequestLifecycle::new(1, 2)?;
        let mut waiter = lifecycle.admit(request_id("request-1")?)?;

        // A zero-capacity rendezvous proves the consumer reached its start
        // receive, making settlement and the racing take eligible from
        // that common point onward. It does not prove the consumer already
        // reached `take_outcome`, pin it there during resolution, or
        // guarantee physical overlap: the scheduler may still serialize
        // either order.
        let (start_tx, start_rx) = mpsc::sync_channel::<()>(0);
        let (completion_tx, completion_rx) = mpsc::channel::<OutcomeDelivery>();
        let (resolved_tx, resolved_rx) = mpsc::channel();
        let consumer = thread::spawn(move || {
            assert!(
                start_rx.recv().is_ok(),
                "producer dropped the start channel"
            );
            // Exactly one racing take per trial.
            match waiter.take_outcome() {
                Ok(resolved) => {
                    // Won the race: consume, then drop promptly before
                    // any reporting handshake.
                    drop(waiter);
                    let _ = resolved_tx.send(resolved);
                }
                Err(OutcomeWaiterError::NotSettled) => {
                    // Lost the race: synchronize on the producer's
                    // completed handoff, then take exactly once more.
                    match completion_rx.recv_timeout(CONSUMER_PATIENCE) {
                        Ok(_) => {}
                        other => panic!("producer completion unusable: {other:?}"),
                    }
                    let second = waiter
                        .take_outcome()
                        .expect("the reported handoff completes the fallback take");
                    drop(waiter);
                    let _ = resolved_tx.send(second);
                }
                Err(err) => panic!("unexpected waiter failure: {err}"),
            }
        });

        start_tx
            .send(())
            .expect("consumer dropped the start channel");
        let verdict = settle(&mut lifecycle)?;
        assert_eq!(
            verdict,
            OutcomeDelivery::Delivered,
            "a committed handoff stays Delivered whatever the racing take did"
        );
        // Ignoring this send result permits the expected successful-first-
        // take path, where the consumer exits and closes the channel
        // first, but it can also mask other channel closures such as a
        // consumer panic; the join below and the exact resolved-outcome
        // assertions still reject any consumer failure.
        let _ = completion_tx.send(verdict);

        assert!(consumer.join().is_ok(), "consumer thread panicked");
        delivered.push(resolved_rx.recv()?);
        assert!(lifecycle.is_empty());
        assert_eq!(lifecycle.admitted(), 1);
    }
    Ok(delivered)
}

#[test]
fn consumed_response_outcome_implies_delivered_despite_consumer_drop() -> Result<(), Box<dyn Error>>
{
    const TRIALS: u32 = 8;

    let settled = response_settling("request-1")?;
    let resolved = raced_consume_then_drop_implies_delivered(TRIALS, |lifecycle| {
        lifecycle.resolve_on_response(&settled)
    })?;
    for outcome in resolved {
        let (resolved_id, carried) = outcome.into_parts();
        assert_eq!(resolved_id.as_str(), "request-1");
        assert_eq!(carried, RequestOutcome::Response(settled.clone()));
    }
    Ok(())
}

#[test]
fn consumed_failure_outcome_implies_delivered_despite_consumer_drop() -> Result<(), Box<dyn Error>>
{
    const TRIALS: u32 = 8;

    let settled_failure = failure_settling(Some(request_id("request-1")?))?;
    let resolved = raced_consume_then_drop_implies_delivered(TRIALS, |lifecycle| {
        lifecycle.resolve_on_failure(&settled_failure)
    })?;
    for outcome in resolved {
        let (resolved_id, carried) = outcome.into_parts();
        assert_eq!(resolved_id.as_str(), "request-1");
        assert_eq!(carried, RequestOutcome::Failure(settled_failure.clone()));
    }
    Ok(())
}

#[test]
fn concurrent_receiver_drop_without_consumption_keeps_state_exact() -> Result<(), Box<dyn Error>> {
    // Bounded, start-synchronized trials: either linearized verdict is
    // legitimate on every trial, so passing never requires a lucky
    // interleaving.
    const TRIALS: u32 = 8;

    for _ in 0..TRIALS {
        let mut lifecycle = ClientRequestLifecycle::new(1, 2)?;
        let waiter = lifecycle.admit(request_id("request-1")?)?;

        // A zero-capacity rendezvous proves the dropper reached its start
        // receive, making its drop and the producer's resolution eligible
        // from that common point onward. It does not pin the drop inside
        // resolution or guarantee physical overlap: the scheduler may
        // serialize either order.
        let (start_tx, start_rx) = mpsc::sync_channel::<()>(0);
        let (dropped_tx, dropped_rx) = mpsc::channel::<()>();
        let dropper = thread::spawn(move || {
            assert!(
                start_rx.recv().is_ok(),
                "producer dropped the start channel"
            );
            // Drop without consuming; the Arc decrement may land before or
            // after the producer's liveness classification.
            drop(waiter);
            let _ = dropped_tx.send(());
        });

        start_tx
            .send(())
            .expect("dropper dropped the start channel");
        let verdict = lifecycle.resolve_on_response(&response_settling("request-1")?)?;
        assert!(matches!(
            verdict,
            OutcomeDelivery::Delivered | OutcomeDelivery::Abandoned
        ));
        assert!(dropper.join().is_ok(), "dropper thread panicked");
        dropped_rx.recv()?;

        // State invariants hold under either linearized verdict.
        assert!(lifecycle.is_empty());
        assert_eq!(lifecycle.admitted(), 1);

        // Retirement is unaffected by the racing receiver drop.
        let readmitted = lifecycle.admit(request_id("request-1")?);
        assert_eq!(readmitted.err(), Some(RequestCorrelationError::Retired));
        let replayed = response_settling("request-1")?;
        assert_eq!(
            lifecycle.resolve_on_response(&replayed),
            Err(RequestCorrelationError::Unknown)
        );
    }
    Ok(())
}

#[test]
fn resolution_frees_capacity_for_fresh_identities_in_order() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 4)?;
    let _first = lifecycle.admit(request_id("request-1")?)?;
    let _second = lifecycle.admit(request_id("request-2")?)?;
    lifecycle.resolve_on_response(&response_settling("request-1")?)?;

    let third = lifecycle.admit(request_id("request-3")?)?;
    assert_eq!(third.request_id().as_str(), "request-3");
    assert_eq!(lifecycle.len(), 2);
    assert_eq!(lifecycle.admitted(), 3);

    lifecycle.resolve_on_response(&response_settling("request-2")?)?;
    lifecycle.resolve_on_response(&response_settling("request-3")?)?;
    assert!(lifecycle.is_empty());
    assert_eq!(lifecycle.admitted(), 3);
    Ok(())
}

#[test]
fn every_rejection_leaves_the_lifecycle_unchanged() -> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 3)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    let mut first_waiter = lifecycle.admit(first.clone())?;
    let mut second_waiter = lifecycle.admit(second.clone())?;

    // Owned snapshots survive across the mutable calls below.
    let observed = |lifecycle: &ClientRequestLifecycle| {
        (
            lifecycle.pending_capacity(),
            lifecycle.admission_budget(),
            lifecycle.admitted(),
            lifecycle.len(),
            lifecycle.is_pending(&first),
            lifecycle.is_pending(&second),
        )
    };
    let before = observed(&lifecycle);

    assert_eq!(
        lifecycle.admit(first.clone()).err(),
        Some(RequestCorrelationError::Duplicate)
    );
    assert_eq!(observed(&lifecycle), before);

    assert_eq!(
        lifecycle.admit(request_id("request-9")?).err(),
        Some(RequestCorrelationError::AtCapacity { capacity: 2 }),
        "a fresh id hits transient pending capacity before the unspent budget"
    );
    assert_eq!(observed(&lifecycle), before);

    let unknown_response = response_settling("request-9")?;
    assert_eq!(
        lifecycle.resolve_on_response(&unknown_response),
        Err(RequestCorrelationError::Unknown)
    );
    assert_eq!(observed(&lifecycle), before);

    let uncorrelated = failure_settling(None)?;
    assert_eq!(
        lifecycle.resolve_on_failure(&uncorrelated),
        Err(RequestCorrelationError::Uncorrelated)
    );
    assert_eq!(observed(&lifecycle), before);

    let unknown_failure = failure_settling(Some(request_id("request-9")?))?;
    assert_eq!(
        lifecycle.resolve_on_failure(&unknown_failure).err(),
        Some(RequestCorrelationError::Unknown)
    );
    assert_eq!(observed(&lifecycle), before);

    assert_eq!(
        ClientRequestLifecycle::new(0, 3).err(),
        Some(RequestCorrelationError::ZeroPendingLimit)
    );
    assert_eq!(
        ClientRequestLifecycle::new(2, 0).err(),
        Some(RequestCorrelationError::ZeroLifetimeBudget)
    );
    assert_eq!(observed(&lifecycle), before);

    // Immediate typed reads prove both live waiters are genuinely
    // unsettled right now: no rejected operation deposited anything.
    assert_eq!(
        first_waiter.take_outcome(),
        Err(OutcomeWaiterError::NotSettled)
    );
    assert_eq!(
        second_waiter.take_outcome(),
        Err(OutcomeWaiterError::NotSettled)
    );

    // Every rejection left both live waiters unsettled; each still delivers
    // exactly its own outcome, exactly once.
    let settling_first = response_settling("request-1")?;
    let settling_second = response_settling("request-2")?;
    assert_eq!(
        lifecycle.resolve_on_response(&settling_first),
        Ok(OutcomeDelivery::Delivered)
    );
    assert_eq!(
        lifecycle.resolve_on_response(&settling_second),
        Ok(OutcomeDelivery::Delivered)
    );
    assert!(lifecycle.is_empty());

    let (resolved_first, first_outcome) = first_waiter.take_outcome()?.into_parts();
    assert_eq!(resolved_first.as_str(), "request-1");
    assert_eq!(first_outcome, RequestOutcome::Response(settling_first));
    assert_eq!(
        first_waiter.take_outcome(),
        Err(OutcomeWaiterError::AlreadyTaken)
    );

    let (resolved_second, second_outcome) = second_waiter.take_outcome()?.into_parts();
    assert_eq!(resolved_second.as_str(), "request-2");
    assert_eq!(second_outcome, RequestOutcome::Response(settling_second));
    assert_eq!(
        second_waiter.take_outcome(),
        Err(OutcomeWaiterError::AlreadyTaken)
    );
    Ok(())
}

#[test]
fn exhausted_budget_rejections_leave_live_waiters_exactly_one_delivery()
-> Result<(), Box<dyn Error>> {
    let mut lifecycle = ClientRequestLifecycle::new(2, 3)?;
    let first = request_id("request-1")?;
    let second = request_id("request-2")?;
    drop(lifecycle.admit(first.clone())?);
    let mut second_waiter = lifecycle.admit(second.clone())?;

    // Spend the entire lifetime budget across three admissions. The first
    // waiter was dropped, so its settlement proves the abandoned path
    // still settles exactly once and consumes budget.
    let settling_first = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&settling_first),
        Ok(OutcomeDelivery::Abandoned)
    );
    let third = request_id("request-3")?;
    let mut third_waiter = lifecycle.admit(third.clone())?;
    let second_failure = failure_settling(Some(second.clone()))?;
    assert_eq!(
        lifecycle.resolve_on_failure(&second_failure),
        Ok(OutcomeDelivery::Delivered),
        "a correlated failure settles its live pending request"
    );

    // Owned snapshots survive across the mutable calls below.
    let observed = |lifecycle: &ClientRequestLifecycle| {
        (
            lifecycle.pending_capacity(),
            lifecycle.admission_budget(),
            lifecycle.admitted(),
            lifecycle.len(),
            lifecycle.is_pending(&third),
        )
    };
    let spent = observed(&lifecycle);
    assert_eq!(spent.2, 3);
    assert_eq!(spent.3, 1);

    assert_eq!(
        lifecycle.admit(request_id("request-9")?).err(),
        Some(RequestCorrelationError::LifetimeExhausted { budget: 3 })
    );
    assert_eq!(observed(&lifecycle), spent);

    assert_eq!(
        lifecycle.admit(first).err(),
        Some(RequestCorrelationError::Retired),
        "retirement is diagnosed even when the lifetime budget is exhausted"
    );
    assert_eq!(observed(&lifecycle), spent);

    let replayed_first = response_settling("request-1")?;
    assert_eq!(
        lifecycle.resolve_on_response(&replayed_first).err(),
        Some(RequestCorrelationError::Unknown),
        "a retired completion stays unknown after its settlement replay"
    );

    let replayed_second_failure = failure_settling(Some(second.clone()))?;
    assert_eq!(
        lifecycle.resolve_on_failure(&replayed_second_failure).err(),
        Some(RequestCorrelationError::Unknown)
    );
    let replayed_second_response = response_settling("request-2")?;
    assert_eq!(
        lifecycle
            .resolve_on_response(&replayed_second_response)
            .err(),
        Some(RequestCorrelationError::Unknown)
    );
    assert_eq!(observed(&lifecycle), spent);

    // Only the third waiter is still pending: an immediate typed read
    // proves no rejected operation deposited anything into it. The second
    // waiter was already settled by the correlated failure above; its
    // stored exact failure is unchanged and delivers exactly once below.
    assert_eq!(
        third_waiter.take_outcome(),
        Err(OutcomeWaiterError::NotSettled)
    );

    let settling_third = response_settling("request-3")?;
    assert_eq!(
        lifecycle.resolve_on_response(&settling_third),
        Ok(OutcomeDelivery::Delivered)
    );
    assert!(lifecycle.is_empty());
    assert_eq!(lifecycle.admitted(), 3);

    let (resolved_third, third_outcome) = third_waiter.take_outcome()?.into_parts();
    assert_eq!(resolved_third.as_str(), "request-3");
    assert_eq!(third_outcome, RequestOutcome::Response(settling_third));
    assert_eq!(
        third_waiter.take_outcome(),
        Err(OutcomeWaiterError::AlreadyTaken)
    );

    let (resolved_second, second_outcome) = second_waiter.take_outcome()?.into_parts();
    assert_eq!(resolved_second.as_str(), "request-2");
    assert_eq!(second_outcome, RequestOutcome::Failure(second_failure));
    assert_eq!(
        second_waiter.take_outcome(),
        Err(OutcomeWaiterError::AlreadyTaken)
    );
    Ok(())
}
