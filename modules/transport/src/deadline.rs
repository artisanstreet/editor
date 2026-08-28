//! Transport-operation deadlines and caller cancellation.
//!
//! Every long-lived transport step (connecting, opening streams,
//! handshaking, sending, receiving, shutting down) runs under two caller
//! controls: a finite [`Duration`] deadline and a [`CancelHandle`]
//! signal. Outcomes are typed as [`DeadlineError`] and never require
//! matching on error strings:
//!
//! * [`DeadlineError::Timeout`] — the operation did not settle within
//!   its finite limit.
//! * [`DeadlineError::Cancelled`] — the caller signalled the handle
//!   before the operation settled.
//! * [`DeadlineError::InvalidLimit`] — the requested limit cannot be
//!   represented as a future instant.
//! * [`DeadlineError::Peer`] — the operation failed on its own; the
//!   typed underlying error is preserved verbatim as `#[source]`.
//!
//! # Cancellation safety
//!
//! [`CancelHandle`] combines an atomic flag with [`tokio::sync::Notify`].
//! [`CancelHandle::cancel`] publishes the flag with release ordering
//! *before* broadcasting `notify_waiters`. On the Tokio 1.53.1
//! mechanism this module relies on: a `Notified` future snapshots the
//! `notify_waiters` generation counter at creation time, so any
//! broadcast made after that creation is observed at the next poll even
//! if the waiter has not yet registered interest with the runtime.
//! [`CancelHandle::wait`] exploits exactly that window: it reads the
//! flag, creates the `Notified` future (taking the counter snapshot),
//! re-reads the flag, and only then awaits. A cancellation that
//! completes before the recheck is caught by the flag; one whose
//! broadcast lands after the snapshot is caught because the counter has
//! advanced by the time the future is polled. A cancellation raised
//! before the wrapped operation is first polled therefore wins
//! deterministically (the operation future is never polled), and no
//! interleaving of cancellation, deadline expiry, and completion can
//! leave a wrapped operation blocked past its controls.
//!
//! # Deterministic precedence
//!
//! Outcome resolution follows one fixed order, re-evaluated at the top
//! of every scheduling cycle and mirrored by a biased selection when
//! several sources wake in the same cycle:
//!
//! 1. caller cancellation (including pre-start cancellation, decided
//!    before the operation is polled at all),
//! 2. deadline expiry,
//! 3. the operation's own completion, successful or failed.
//!
//! A result that arrives only together with its expiry is late and is
//! reported as [`DeadlineError::Timeout`] — the clock is consulted
//! before the operation is polled again, so the decision never depends
//! on timer wakeup granularity. Likewise a failure that arrives only
//! together with a cancellation is subsumed by
//! [`DeadlineError::Cancelled`]. An unrepresentable limit is decided in
//! the same order: after a pre-start cancellation, before `fut` is ever
//! polled. Only this decision precedence is absolute; wall-clock
//! latency remains subject to task and runtime scheduling.
//!
//! Dropping an unresolved future on timeout or cancellation stops
//! polling it; it does **not** roll back side effects the operation has
//! already performed. Callers that own the underlying Quinn streams,
//! connections, or sessions must close, reset, or reconcile them
//! according to the operation's semantics once an outcome is observed.
//!
//! # Payload hygiene
//!
//! Data owned by this module — [`OperationKind`], the requested
//! [`Duration`], and the `Timeout`, `Cancelled`, and `InvalidLimit`
//! variants themselves — never copies or formats peer payloads,
//! credentials, or peer-provided text. The [`DeadlineError::Peer`]
//! variant deliberately preserves arbitrary typed diagnostics from the
//! underlying transport as its `#[source]` error; such an error type may
//! itself contain sensitive data, and this layer neither parses it nor
//! interpolates it into its own `Display`.

use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use thiserror::Error;
use tokio::pin;
use tokio::sync::Notify;

/// Race-free caller-cancellation signal for transport operations.
///
/// The handle is shared by reference with every operation it controls;
/// wrap it in `std::sync::Arc` when ownership must cross task
/// boundaries. `cancel` is idempotent, safe from any thread, and always
/// precedes its wake-up, so watchers can never miss both the flag and
/// the notification.
#[derive(Debug, Default)]
pub struct CancelHandle {
    /// Published before any wake-up; read with acquire ordering.
    cancelled: AtomicBool,
    /// Wakes registered waiters once per `cancel` broadcast.
    notified: Notify,
}

impl CancelHandle {
    /// Creates a handle that has not been cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels every operation watching this handle.
    ///
    /// Publication of the flag precedes the `notify_waiters` broadcast;
    /// combined with the generation-counter snapshot taken by each
    /// `Notified` future in [`CancelHandle::wait`], no watcher can miss
    /// both signals.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notified.notify_waiters();
    }

    /// Returns whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Resolves as soon as the handle is cancelled.
    ///
    /// Each iteration reads the flag, creates the `Notified` future —
    /// which snapshots the `notify_waiters` generation counter at
    /// creation — re-reads the flag, and only then awaits. A
    /// cancellation completed before the recheck is observed through the
    /// flag; a cancellation broadcast afterwards advances the counter
    /// past the snapshot and resolves this future at its next poll. No
    /// interleaving falls through both checks.
    pub async fn wait(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let woken = self.notified.notified();
            pin!(woken);
            if self.is_cancelled() {
                return;
            }
            woken.await;
        }
    }
}

/// Payload-free classification of one transport operation under a
/// deadline.
///
/// Diagnostics name the operation shape without carrying addresses,
/// payloads, credentials, or peer-provided text.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationKind {
    /// Establishing the QUIC connection to a peer.
    Connect,
    /// Opening a new send or receive stream on a live connection.
    OpenStream,
    /// Running the application handshake over a fresh connection.
    Handshake,
    /// Sending one framed outbound item.
    Send,
    /// Receiving one framed inbound item.
    Receive,
    /// Draining and closing the endpoint or session.
    Shutdown,
}

impl fmt::Display for OperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Connect => "connect",
            Self::OpenStream => "open-stream",
            Self::Handshake => "handshake",
            Self::Send => "send",
            Self::Receive => "receive",
            Self::Shutdown => "shutdown",
        };
        f.write_str(name)
    }
}

/// Typed failure of a deadline-bounded transport operation.
///
/// Variants are matched directly; error-string inspection is neither
/// required nor expected. Data owned by this module never copies or
/// formats peer payloads, credentials, or peer-provided text; see the
/// module-level payload-hygiene notes for how [`DeadlineError::Peer`]
/// treats its source.
#[derive(Debug, Eq, Error, PartialEq)]
pub enum DeadlineError<E> {
    /// The operation stayed unfinished past its finite limit.
    #[error("{operation} did not settle within {}ms", limit.as_millis())]
    Timeout {
        /// Classification of the abandoned operation.
        operation: OperationKind,
        /// The limit that expired.
        limit: Duration,
    },

    /// The caller cancelled the operation through its [`CancelHandle`],
    /// either before it started or while it was in flight.
    #[error("{operation} was cancelled by the caller")]
    Cancelled {
        /// Classification of the abandoned operation.
        operation: OperationKind,
    },

    /// The requested limit cannot be represented as a future instant
    /// (for example `Duration::MAX`); `fut` is never polled. Side
    /// effects from the caller's construction of the future argument
    /// are not controlled by this decision.
    #[error("{operation} was given an unrepresentable deadline limit")]
    InvalidLimit {
        /// Classification of the unstarted operation.
        operation: OperationKind,
    },

    /// The operation failed on its own; the typed underlying transport
    /// error is preserved as the source.
    #[error("{operation} failed at the transport boundary")]
    Peer {
        /// Classification of the failed operation.
        operation: OperationKind,
        /// Typed failure reported by the underlying transport layer.
        #[source]
        error: E,
    },
}

/// Runs one transport operation under a finite deadline and a
/// cancellation handle.
///
/// Pre-start cancellation is checked before `fut` is first polled and
/// wins without ever polling `fut` — including over an invalid limit.
/// Side effects the caller performs while constructing or evaluating
/// the future argument happen before this function can control them
/// and are unaffected by that outcome. The limit's representability is
/// checked next, also before `fut` is polled. The precedence order documented at the
/// module level — cancellation, then deadline, then completion — is
/// re-evaluated at the top of every scheduling cycle, so an operation
/// that completes exactly as its limit expires is reported as
/// [`DeadlineError::Timeout`], never as success. On timeout or
/// cancellation the unresolved future is dropped and never polled
/// again; already-performed side effects are not rolled back, and
/// callers owning the underlying Quinn streams, connections, or
/// sessions must reconcile them per the module-level notes.
///
/// # Errors
///
/// Returns [`DeadlineError::Cancelled`] when `cancel` fires first —
/// including before the operation starts —
/// [`DeadlineError::InvalidLimit`] when `limit` cannot be added to the
/// current instant (detected before the operation is polled),
/// [`DeadlineError::Timeout`] when `limit` elapses first, and
/// [`DeadlineError::Peer`] when `fut` itself resolves to `Err`, with
/// that typed error preserved as the source.
pub async fn run_with_deadline<T, E, F>(
    operation: OperationKind,
    limit: Duration,
    cancel: &CancelHandle,
    fut: F,
) -> Result<T, DeadlineError<E>>
where
    F: Future<Output = Result<T, E>>,
{
    if cancel.is_cancelled() {
        return Err(DeadlineError::Cancelled { operation });
    }

    let Some(deadline) = tokio::time::Instant::now().checked_add(limit) else {
        return Err(DeadlineError::InvalidLimit { operation });
    };
    pin!(fut);
    loop {
        // Fixed precedence: cancellation first...
        if cancel.is_cancelled() {
            return Err(DeadlineError::Cancelled { operation });
        }
        // ...then the clock, before the operation is polled again.
        if tokio::time::Instant::now() >= deadline {
            return Err(DeadlineError::Timeout { operation, limit });
        }
        // ...then completion. The biased selection mirrors the same
        // order for sources waking within one cycle; every branch that
        // does not settle falls through to re-check the ones above it.
        tokio::select! {
            biased;

            () = cancel.wait() => (),
            () = tokio::time::sleep_until(deadline) => (),
            settled = &mut fut => {
                return settled.map_err(|error| DeadlineError::Peer { operation, error });
            }
        }
    }
}
