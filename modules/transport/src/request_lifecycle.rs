//! Client-side lifecycle from an outgoing request to its settled outcome.
//!
//! All correlation decisions — identity admission, pending state, and
//! retirement — belong to the single-owner
//! [`RequestCorrelationRegistry`](crate::request_correlation::RequestCorrelationRegistry).
//! This layer binds each admitted request to exactly one client-side
//! waiter: [`ClientRequestLifecycle::admit`] first asks the registry to
//! admit the id, then hands back a single-owner [`OutcomeWaiter`], and
//! every later [`ServerResponse`] or correlated [`ProtocolFailure`] is
//! first gated by the registry's completion rules before being deposited
//! as a [`ResolvedRequest`] into exactly that one waiter.
//!
//! Lifecycle rules:
//!
//! * Construction takes two explicit nonzero limits — maximum
//!   simultaneously pending requests and total successful admissions for
//!   the owner's entire lifetime — and delegates them unchanged to the
//!   registry; see its documentation for the single-use correlation
//!   contract, the retirement rule, and the one-authenticated-connection
//!   lifetime boundary.
//! * Admission and resolution are all-or-nothing: the registry rejects
//!   before this layer mutates any waiter bookkeeping, so every rejected
//!   operation returns a typed error and leaves every pending request and
//!   waiter unchanged.
//! * Delivered outcomes carry their settling [`RequestId`], so correlation
//!   identity survives past the resolution call.
//! * Dropping a waiter abandons only the delivery of its future outcome,
//!   never the session's correlation bookkeeping: when the completion later
//!   arrives, the pending request still settles exactly once and frees its
//!   pending capacity, reported as [`OutcomeDelivery::Abandoned`]. State
//!   therefore stays bounded whether or not any waiter outlives admission.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use artisan_domain::RequestId;
use artisan_protocol::{ProtocolFailure, ServerResponse};
use thiserror::Error;

use crate::request_correlation::{RequestCorrelationError, RequestCorrelationRegistry};

/// Locks one waiter slot, recovering the data even from poisoning.
///
/// The guarded value is only ever mutated by all-or-nothing operations on
/// [`ClientRequestLifecycle`], so a poisoned guard cannot contain partially
/// applied state; the slot contents stay usable either way.
fn lock_slot(slot: &Mutex<WaiterSlot>) -> MutexGuard<'_, WaiterSlot> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Failure while reading a settled outcome out of an [`OutcomeWaiter`].
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum OutcomeWaiterError {
    /// No matching response or failure has been resolved yet.
    #[error("the awaited request has not settled yet")]
    NotSettled,
    /// This waiter already delivered its outcome once, and each waiter
    /// delivers at most one.
    #[error("the resolved outcome was already taken from this waiter")]
    AlreadyTaken,
}

/// What happened to a completion that settled exactly one pending request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutcomeDelivery {
    /// A live [`OutcomeWaiter`] received the outcome.
    Delivered,
    /// The waiter was dropped before settlement, so the settled outcome had
    /// no receiver and was discarded after freeing its capacity.
    Abandoned,
}

/// The settled result of one client request, preserving its correlation
/// identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedRequest {
    /// The request this completion settles, as admitted.
    request_id: RequestId,
    /// The matched server answer.
    outcome: RequestOutcome,
}

impl ResolvedRequest {
    /// Returns the request identity this completion settles.
    #[must_use]
    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns the matched server answer.
    #[must_use]
    pub const fn outcome(&self) -> &RequestOutcome {
        &self.outcome
    }

    /// Consumes the settlement into its request id and matched answer.
    #[must_use]
    pub fn into_parts(self) -> (RequestId, RequestOutcome) {
        (self.request_id, self.outcome)
    }
}

/// The matched server answer carried by a [`ResolvedRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestOutcome {
    /// Forge answered the request successfully.
    Response(ServerResponse),
    /// Forge rejected the request with a correlated failure.
    Failure(ProtocolFailure),
}

/// Single-owner handle receiving the outcome of exactly one admitted
/// request.
///
/// Deliberately implements neither [`Clone`] nor [`Copy`]: the waiter is the
/// sole receiver of its request's settled outcome, and duplicating it could
/// let two owners believe they each observed the one delivery. Dropping the
/// waiter abandons future delivery only; see the [module
/// documentation](self) for the deterministic late-completion rule.
#[derive(Debug)]
pub struct OutcomeWaiter {
    /// Request whose outcome this waiter receives.
    request_id: RequestId,
    /// Shared slot the lifecycle deposits the outcome into.
    slot: Arc<Mutex<WaiterSlot>>,
}

impl OutcomeWaiter {
    /// Returns the admitted request this waiter awaits.
    #[must_use]
    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns whether a settled outcome is available to take.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        lock_slot(&self.slot).outcome.is_some()
    }

    /// Takes the settled outcome, if it arrived and was not taken before.
    ///
    /// The waiter stays owned and may be polled again after an unsettled
    /// attempt; only a successful take consumes the delivery.
    ///
    /// # Errors
    ///
    /// Returns [`OutcomeWaiterError::NotSettled`] while no matching
    /// completion has been resolved, and
    /// [`OutcomeWaiterError::AlreadyTaken`] after this waiter already
    /// delivered its one outcome.
    pub fn take_outcome(&mut self) -> Result<ResolvedRequest, OutcomeWaiterError> {
        let mut slot = lock_slot(&self.slot);
        if !slot.settled {
            return Err(OutcomeWaiterError::NotSettled);
        }
        slot.outcome.take().ok_or(OutcomeWaiterError::AlreadyTaken)
    }
}

/// One admitted request's delivery plumbing: the shared slot its waiter
/// reads from.
///
/// Correlation identity itself lives only in the delegated
/// [`RequestCorrelationRegistry`]; this entry exists solely so a completion
/// can find the exact waiter to deposit into.
#[derive(Debug)]
struct PendingWaiter {
    /// Admitted request whose waiter shares this slot.
    request_id: RequestId,
    /// Slot shared with this request's waiter.
    slot: Arc<Mutex<WaiterSlot>>,
}

/// Bounded per-request delivery state shared by the lifecycle and its
/// waiter.
#[derive(Debug, Default)]
struct WaiterSlot {
    /// Whether the lifecycle has deposited this request's outcome.
    settled: bool,
    /// The deposited outcome until its waiter takes it.
    outcome: Option<ResolvedRequest>,
}

/// Bounded owner of one client session's pending requests and their
/// waiters, delegating correlation to
/// [`RequestCorrelationRegistry`].
///
/// Admission and resolution are all-or-nothing: the registry gates every
/// decision, and every rejected operation returns a typed error leaving
/// every pending request and waiter unchanged. Deliberately implements
/// neither [`Clone`] nor [`Copy`]: the lifecycle is the single mutable
/// owner of the session's waiters, and duplicating it could let two owners
/// settle the same request. Like the registry, one instance serves exactly
/// one authenticated connection for that live connection's whole lifetime.
#[derive(Debug)]
pub struct ClientRequestLifecycle {
    /// Single owner of identity admission, pending state, and retirement.
    registry: RequestCorrelationRegistry,
    /// Delivery entries mirroring exactly the registry's pending requests:
    /// pushed only after a successful admission, removed only after a
    /// successful completion.
    waiters: Vec<PendingWaiter>,
}

impl ClientRequestLifecycle {
    /// Creates a lifecycle admitting at most `pending_capacity`
    /// simultaneously pending requests and at most `admission_budget`
    /// successful requests over the owning connection's entire lifetime.
    ///
    /// # Errors
    ///
    /// Returns the registry's construction errors:
    /// [`RequestCorrelationError::ZeroPendingLimit`] and
    /// [`RequestCorrelationError::ZeroLifetimeBudget`].
    pub fn new(
        pending_capacity: usize,
        admission_budget: usize,
    ) -> Result<Self, RequestCorrelationError> {
        Ok(Self {
            registry: RequestCorrelationRegistry::new(pending_capacity, admission_budget)?,
            waiters: Vec::new(),
        })
    }

    /// Returns the fixed maximum number of simultaneously pending requests.
    #[must_use]
    pub const fn pending_capacity(&self) -> usize {
        self.registry.pending_capacity()
    }

    /// Returns the fixed total number of successful admissions allowed
    /// during the owning connection's entire lifetime.
    #[must_use]
    pub const fn admission_budget(&self) -> usize {
        self.registry.admission_budget()
    }

    /// Returns how many distinct requests were successfully admitted during
    /// the owning connection's entire lifetime.
    ///
    /// This counts consumed lifetime budget, not pending occupancy; see
    /// [`len`](Self::len) for the latter. It never decreases.
    #[must_use]
    pub fn admitted(&self) -> usize {
        self.registry.admitted()
    }

    /// Returns the number of currently pending requests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.registry.len()
    }

    /// Returns whether no request is currently pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }

    /// Returns whether `request_id` currently awaits completion.
    #[must_use]
    pub fn is_pending(&self, request_id: &RequestId) -> bool {
        self.registry.is_pending(request_id)
    }

    /// Admits one outgoing request as pending and returns its waiter.
    ///
    /// The registry decides admission — duplicate, retired-identity,
    /// lifetime-budget, and capacity diagnoses in its documented precedence
    /// — before this layer creates anything, so a rejected admission
    /// changes no waiter state and creates no waiter. On success exactly
    /// one new waiter exists for `request_id`.
    ///
    /// # Errors
    ///
    /// Returns the registry's admission errors; see
    /// [`RequestCorrelationRegistry::register`].
    pub fn admit(
        &mut self,
        request_id: RequestId,
    ) -> Result<OutcomeWaiter, RequestCorrelationError> {
        self.registry.register(request_id.clone())?;
        let slot = Arc::new(Mutex::new(WaiterSlot::default()));
        self.waiters.push(PendingWaiter {
            request_id: request_id.clone(),
            slot: Arc::clone(&slot),
        });
        Ok(OutcomeWaiter { request_id, slot })
    }

    /// Settles the pending request answered by a successful server
    /// response, depositing the outcome into its waiter.
    ///
    /// On success exactly the response's request stops pending, its
    /// pending capacity is freed, and its identity retires against any
    /// further use, whatever the waiter's liveness; on failure nothing
    /// changes, including every other waiter.
    ///
    /// # Errors
    ///
    /// Returns [`RequestCorrelationError::Unknown`] when the response's
    /// request id is not pending, as with a late replay of an already
    /// retired completion.
    pub fn resolve_on_response(
        &mut self,
        response: &ServerResponse,
    ) -> Result<OutcomeDelivery, RequestCorrelationError> {
        self.registry.complete_on_response(response)?;
        Ok(self.deliver(
            |pending: &PendingWaiter| pending.request_id == response.request_id,
            || RequestOutcome::Response(response.clone()),
        ))
    }

    /// Settles the pending request carried by a protocol failure,
    /// depositing the outcome into its waiter.
    ///
    /// On success exactly the failure's request stops pending, its pending
    /// capacity is freed, and its identity retires against any further
    /// use, whatever the waiter's liveness; on failure nothing changes,
    /// including every other waiter.
    ///
    /// # Errors
    ///
    /// Returns [`RequestCorrelationError::Uncorrelated`] when the failure
    /// carries no request id, and [`RequestCorrelationError::Unknown`]
    /// when the carried request id is not pending.
    pub fn resolve_on_failure(
        &mut self,
        failure: &ProtocolFailure,
    ) -> Result<OutcomeDelivery, RequestCorrelationError> {
        self.registry.complete_on_failure(failure)?;
        Ok(self.deliver(
            |pending: &PendingWaiter| {
                failure
                    .request_id
                    .as_ref()
                    .is_some_and(|request_id| pending.request_id == *request_id)
            },
            || RequestOutcome::Failure(failure.clone()),
        ))
    }

    /// Deposits one outcome into exactly one pending request's waiter and
    /// removes that delivery entry.
    ///
    /// Called only after the registry accepted the completion, so exactly
    /// one mirrored entry correlates. The delivery verdict distinguishes a
    /// live waiter from one dropped between admission and settlement; both
    /// settle the request exactly once and free its pending capacity.
    fn deliver<M, F>(&mut self, correlates: M, build_outcome: F) -> OutcomeDelivery
    where
        M: Fn(&PendingWaiter) -> bool,
        F: FnOnce() -> RequestOutcome,
    {
        let position = self
            .waiters
            .iter()
            .position(correlates)
            .expect("waiter entries mirror the registry's accepted pending requests");
        let pending = self.waiters.remove(position);
        {
            let mut slot = lock_slot(&pending.slot);
            slot.settled = true;
            slot.outcome = Some(ResolvedRequest {
                request_id: pending.request_id.clone(),
                outcome: build_outcome(),
            });
        }
        if Arc::strong_count(&pending.slot) == 1 {
            OutcomeDelivery::Abandoned
        } else {
            OutcomeDelivery::Delivered
        }
    }
}
