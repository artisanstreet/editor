//! Hermetic deadline and cancellation coverage for transport operations.
//!
//! No scenario touches a socket: peer failures are synthetic typed
//! errors, timeouts come from never-settling futures, and every await
//! site is fenced by a watchdog so a regression cannot hang the suite.

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::future::{pending, ready};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use artisan_transport::{CancelHandle, DeadlineError, OperationKind, run_with_deadline};

/// Upper bound for any single scenario; every await site is fenced.
const WATCHDOG: Duration = Duration::from_secs(5);

/// Long park proving mid-flight cancellation wakes before its deadline.
const PARKED: Duration = Duration::from_secs(60);

/// Useful promptness bound: a broadcast cancellation must resolve every
/// shared waiter well within this budget — far below the outer
/// watchdog and any park or deadline in play, yet tolerant of loaded
/// Windows CI for a 25 ms cancellation.
const PROMPT: Duration = Duration::from_secs(1);

#[derive(Debug, Eq, PartialEq)]
struct SyntheticPeerFailure;

impl Error for SyntheticPeerFailure {}

impl fmt::Display for SyntheticPeerFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("synthetic peer failure")
    }
}

/// Never-settling operation that counts how often it was polled.
struct CountingPending {
    polls: Arc<AtomicUsize>,
}

impl Future for CountingPending {
    type Output = Result<(), SyntheticPeerFailure>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        // The counter is read only after the joined future finished on
        // the same thread, so relaxed ordering is sufficient here.
        self.polls.fetch_add(1, Ordering::Relaxed);
        Poll::Pending
    }
}

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(WATCHDOG, future)
        .await
        .expect("scenario exceeded its watchdog")
}

#[tokio::test]
async fn immediate_success_passes_through_unchanged() -> Result<(), Box<dyn Error>> {
    let cancel = CancelHandle::new();
    let outcome: Result<u32, DeadlineError<Infallible>> = bounded(run_with_deadline(
        OperationKind::Receive,
        Duration::from_millis(250),
        &cancel,
        ready(Ok(7)),
    ))
    .await;

    assert_eq!(outcome?, 7);
    Ok(())
}

#[tokio::test]
async fn typed_peer_failure_is_preserved_as_source() {
    let cancel = CancelHandle::new();
    let outcome = bounded(run_with_deadline(
        OperationKind::Send,
        Duration::from_secs(1),
        &cancel,
        ready::<Result<(), SyntheticPeerFailure>>(Err(SyntheticPeerFailure)),
    ))
    .await;

    let Err(failure) = outcome else {
        panic!("expected a typed peer failure");
    };
    let DeadlineError::Peer { operation, .. } = &failure else {
        panic!("peer failures must stay typed");
    };
    assert_eq!(*operation, OperationKind::Send);

    let source = failure
        .source()
        .expect("the underlying error must be exposed as source")
        .downcast_ref::<SyntheticPeerFailure>()
        .expect("the source must remain the typed peer error");
    assert_eq!(*source, SyntheticPeerFailure);
}

#[tokio::test]
async fn deadline_expiry_is_a_typed_timeout_without_networking() {
    let cancel = CancelHandle::new();
    let started = std::time::Instant::now();
    let outcome = bounded(run_with_deadline(
        OperationKind::Connect,
        Duration::from_millis(20),
        &cancel,
        pending::<Result<u8, Infallible>>(),
    ))
    .await;

    assert_eq!(
        outcome,
        Err(DeadlineError::Timeout {
            operation: OperationKind::Connect,
            limit: Duration::from_millis(20),
        })
    );
    assert!(started.elapsed() >= Duration::from_millis(20));
}

#[tokio::test]
async fn pre_started_cancellation_wins_without_polling_the_operation() {
    let cancel = CancelHandle::new();
    cancel.cancel();
    assert!(cancel.is_cancelled());

    let polls = Arc::new(AtomicUsize::new(0));
    let outcome = bounded(run_with_deadline(
        OperationKind::Handshake,
        Duration::ZERO,
        &cancel,
        CountingPending {
            polls: Arc::clone(&polls),
        },
    ))
    .await;

    assert_eq!(
        outcome,
        Err(DeadlineError::Cancelled {
            operation: OperationKind::Handshake,
        })
    );
    // Pre-start cancellation is decided before the first poll: even a
    // simultaneously ready success and an already-expired deadline must
    // not outrank it.
    assert_eq!(polls.load(Ordering::Relaxed), 0);

    let ready_success: Result<(), DeadlineError<Infallible>> = bounded(run_with_deadline(
        OperationKind::OpenStream,
        Duration::ZERO,
        &cancel,
        ready(Ok(())),
    ))
    .await;
    assert_eq!(
        ready_success,
        Err(DeadlineError::Cancelled {
            operation: OperationKind::OpenStream,
        })
    );
}

#[tokio::test]
async fn mid_flight_cancellation_wakes_promptly_without_hanging() {
    let cancel = Arc::new(CancelHandle::new());
    let canceller = {
        let cancel = Arc::clone(&cancel);
        async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            cancel.cancel();
        }
    };

    let started = std::time::Instant::now();
    let (outcome, ()) = bounded(async {
        tokio::join!(
            run_with_deadline::<(), Infallible, _>(
                OperationKind::Shutdown,
                Duration::from_secs(30),
                &cancel,
                async {
                    tokio::time::sleep(PARKED).await;
                    Ok(())
                },
            ),
            canceller,
        )
    })
    .await;
    let elapsed = started.elapsed();

    assert_eq!(
        outcome,
        Err(DeadlineError::Cancelled {
            operation: OperationKind::Shutdown,
        })
    );
    // A lost wakeup would surface as Timeout at the 30 s limit (or hit
    // the watchdog), so only a genuine prompt cancellation can pass.
    assert!(
        elapsed < PROMPT,
        "mid-flight cancellation must wake the parked operation promptly, took {elapsed:?}"
    );
}

#[tokio::test]
async fn simultaneous_outcomes_follow_documented_precedence() {
    // Deadline expiry outranks a success that is ready in the same poll
    // cycle: late results are reported as timeouts. (Cancellation
    // outranking both was proven by the pre-started scenario above.)
    let cancel = CancelHandle::new();
    let timed_out: Result<(), DeadlineError<Infallible>> = bounded(run_with_deadline(
        OperationKind::Receive,
        Duration::ZERO,
        &cancel,
        ready(Ok(())),
    ))
    .await;
    assert_eq!(
        timed_out,
        Err(DeadlineError::Timeout {
            operation: OperationKind::Receive,
            limit: Duration::ZERO,
        })
    );

    // With no competitor, an immediately failing operation resolves to
    // the typed peer failure rather than any string-shaped surrogate.
    let cancel = CancelHandle::new();
    let failed = bounded(run_with_deadline(
        OperationKind::Connect,
        Duration::from_secs(30),
        &cancel,
        ready::<Result<(), SyntheticPeerFailure>>(Err(SyntheticPeerFailure)),
    ))
    .await;
    assert!(matches!(failed, Err(DeadlineError::Peer { .. })));
}

#[tokio::test]
async fn unrepresentable_limit_is_typed_without_polling_the_operation() {
    let cancel = CancelHandle::new();

    let polls = Arc::new(AtomicUsize::new(0));
    let outcome = bounded(run_with_deadline(
        OperationKind::Handshake,
        Duration::MAX,
        &cancel,
        CountingPending {
            polls: Arc::clone(&polls),
        },
    ))
    .await;

    assert_eq!(
        outcome,
        Err(DeadlineError::InvalidLimit {
            operation: OperationKind::Handshake,
        })
    );
    assert_eq!(
        polls.load(Ordering::Relaxed),
        0,
        "an invalid limit must be rejected before any poll"
    );
}

#[tokio::test]
async fn pre_cancelled_handle_outranks_an_invalid_limit_and_ready_success() {
    let cancel = CancelHandle::new();
    cancel.cancel();

    let outcome: Result<(), DeadlineError<Infallible>> = bounded(run_with_deadline(
        OperationKind::OpenStream,
        Duration::MAX,
        &cancel,
        ready(Ok(())),
    ))
    .await;

    // Fixed order: cancellation first, then limit representability,
    // then completion — even with a success ready to hand back.
    assert_eq!(
        outcome,
        Err(DeadlineError::Cancelled {
            operation: OperationKind::OpenStream,
        })
    );
}

#[tokio::test]
async fn single_cancel_broadcast_wakes_every_shared_operation() {
    let cancel = Arc::new(CancelHandle::new());
    let canceller = {
        let cancel = Arc::clone(&cancel);
        async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            cancel.cancel();
        }
    };

    let started = std::time::Instant::now();
    let (connect, send, receive, ()) = bounded(async {
        tokio::join!(
            run_with_deadline::<(), Infallible, _>(
                OperationKind::Connect,
                Duration::from_secs(30),
                &cancel,
                async {
                    tokio::time::sleep(PARKED).await;
                    Ok(())
                },
            ),
            run_with_deadline::<(), Infallible, _>(
                OperationKind::Send,
                Duration::from_secs(30),
                &cancel,
                async {
                    tokio::time::sleep(PARKED).await;
                    Ok(())
                },
            ),
            run_with_deadline::<(), Infallible, _>(
                OperationKind::Receive,
                Duration::from_secs(30),
                &cancel,
                async {
                    tokio::time::sleep(PARKED).await;
                    Ok(())
                },
            ),
            canceller,
        )
    })
    .await;
    let elapsed = started.elapsed();

    for (outcome, kind) in [
        (connect, OperationKind::Connect),
        (send, OperationKind::Send),
        (receive, OperationKind::Receive),
    ] {
        assert_eq!(
            outcome,
            Err(DeadlineError::Cancelled { operation: kind }),
            "every shared waiter must observe the single broadcast"
        );
    }
    assert!(
        elapsed < PROMPT,
        "one cancel call must wake all waiters promptly, took {elapsed:?}"
    );
}
