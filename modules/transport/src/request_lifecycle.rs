//! Client-side lifecycle from an outgoing request to its settled outcome.
//!
//! Where [`RequestCorrelationRegistry`](crate::request_correlation::RequestCorrelationRegistry)
//! tracks which [`RequestId`]s are merely pending, this layer binds each
//! admitted request to exactly one client-side waiter:
//! [`ClientRequestLifecycle::admit`] reserves a bounded slot and hands back
//! a single-owner [`OutcomeWaiter`], and every later
//! [`ServerResponse`] or correlated [`ProtocolFailure`] settles exactly
//! that one pending request by depositing a [`ResolvedRequest`] into its
//! waiter.
//!
//! Lifecycle rules:
//!
//! * Capacity is fixed at construction, must be nonzero, and bounds every
//!   internal collection; admission beyond capacity fails deterministically
//!   without mutation. Duplicate admission fails before capacity is
//!   consulted, identically at any occupancy.
//! * Completion removes exactly one pending entry and frees its capacity;
//!   unknown, replayed, and uncorrelated completions are typed rejections
//!   that leave every other pending request untouched.
//! * Delivered outcomes carry their settling [`RequestId`], so correlation
//!   identity survives past the resolution call.
//! * Dropping a waiter abandons only the delivery of its future outcome,
//!   never the session's correlation bookkeeping: when the completion later
//!   arrives, the pending request still settles exactly once and frees its
//!   capacity, reported as [`OutcomeDelivery::Abandoned`]. State therefore
//!   stays bounded whether or not any waiter outlives admission.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use artisan_domain::RequestId;
use artisan_protocol::{ProtocolFailure, ServerResponse};
use thiserror::Error;

/// Locks one waiter slot, recovering the data even from poisoning.
///
/// The guarded value is only ever mutated by all-or-nothing operations on
/// [`ClientRequestLifecycle`], so a poisoned guard cannot contain partially
/// applied state; the slot contents stay usable either way.
fn lock_slot(slot: &Mutex<WaiterSlot>) -> MutexGuard<'_, WaiterSlot> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Failure while admitting a request or resolving its completion.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum RequestLifecycleError {
    /// The requested lifecycle capacity was zero, so no request could ever
    /// have been admitted.
    #[error("client request lifecycle capacity must be nonzero")]
    ZeroCapacity,
    /// The request id already holds a waiter, so admitting it again would
    /// let one completion settle the same request twice.
    #[error("request is already pending a client-side outcome")]
    Duplicate,
    /// Every lifecycle slot held an unresolved request, so the request
    /// cannot be admitted until one completes.
    #[error("client request lifecycle reached its capacity of {capacity} pending requests")]
    AtCapacity {
        /// Fixed lifecycle capacity that was exhausted.
        capacity: usize,
    },
    /// The failure carried no request id, so it correlates to no pending
    /// request.
    #[error("protocol failure carries no request id to correlate")]
    Uncorrelated,
    /// The request id is not pending, because it was never admitted or was
    /// already settled.
    #[error("completion does not match any pending client request")]
    Unknown,
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

/// One admitted request awaiting its completion.
#[derive(Debug)]
struct PendingRequest {
    /// Admitted correlation identity.
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
/// waiters.
///
/// Admission and resolution are all-or-nothing: every rejected operation
/// returns a typed error and leaves every pending request unchanged.
/// Capacity is explicit at construction and enforced before insertion.
/// Deliberately implements neither [`Clone`] nor [`Copy`]: the lifecycle is
/// the single mutable owner of the session's pending correlations, and
/// duplicating it could let two owners settle the same request.
#[derive(Debug)]
pub struct ClientRequestLifecycle {
    /// Maximum number of simultaneously pending requests.
    capacity: usize,
    /// Pending requests in admission order.
    pending: Vec<PendingRequest>,
}

impl ClientRequestLifecycle {
    /// Creates a lifecycle admitting at most `capacity` pending requests.
    ///
    /// # Errors
    ///
    /// Returns [`RequestLifecycleError::ZeroCapacity`] when `capacity` is
    /// zero.
    pub fn new(capacity: usize) -> Result<Self, RequestLifecycleError> {
        if capacity == 0 {
            return Err(RequestLifecycleError::ZeroCapacity);
        }
        Ok(Self {
            capacity,
            pending: Vec::new(),
        })
    }

    /// Returns the fixed maximum number of pending requests.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the number of currently pending requests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Returns whether no request is currently pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Returns whether `request_id` currently awaits completion.
    #[must_use]
    pub fn is_pending(&self, request_id: &RequestId) -> bool {
        self.pending
            .iter()
            .any(|pending| &pending.request_id == request_id)
    }

    /// Admits one outgoing request as pending and returns its waiter.
    ///
    /// Duplicate detection precedes capacity enforcement, so a repeated id
    /// is diagnosed identically at any occupancy. On success exactly one
    /// new waiter exists for `request_id`; on failure the lifecycle is
    /// unchanged and no waiter is created.
    ///
    /// # Errors
    ///
    /// Returns [`RequestLifecycleError::Duplicate`] when `request_id` is
    /// already pending, and [`RequestLifecycleError::AtCapacity`] when
    /// every slot holds an unresolved request.
    pub fn admit(&mut self, request_id: RequestId) -> Result<OutcomeWaiter, RequestLifecycleError> {
        if self.is_pending(&request_id) {
            return Err(RequestLifecycleError::Duplicate);
        }
        if self.pending.len() >= self.capacity {
            return Err(RequestLifecycleError::AtCapacity {
                capacity: self.capacity,
            });
        }
        let slot = Arc::new(Mutex::new(WaiterSlot::default()));
        self.pending.push(PendingRequest {
            request_id: request_id.clone(),
            slot: Arc::clone(&slot),
        });
        Ok(OutcomeWaiter { request_id, slot })
    }

    /// Settles the pending request answered by a successful server
    /// response, depositing the outcome into its waiter.
    ///
    /// On success exactly the response's request stops pending and its
    /// capacity is freed, whatever the waiter's liveness; on failure the
    /// lifecycle is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`RequestLifecycleError::Unknown`] when the response's
    /// request id is not pending.
    pub fn resolve_on_response(
        &mut self,
        response: &ServerResponse,
    ) -> Result<OutcomeDelivery, RequestLifecycleError> {
        self.resolve(&response.request_id, || {
            RequestOutcome::Response(response.clone())
        })
    }

    /// Settles the pending request carried by a protocol failure,
    /// depositing the outcome into its waiter.
    ///
    /// On success exactly the failure's request stops pending and its
    /// capacity is freed, whatever the waiter's liveness; on failure the
    /// lifecycle is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`RequestLifecycleError::Uncorrelated`] when the failure
    /// carries no request id, and [`RequestLifecycleError::Unknown`] when
    /// the carried request id is not pending.
    pub fn resolve_on_failure(
        &mut self,
        failure: &ProtocolFailure,
    ) -> Result<OutcomeDelivery, RequestLifecycleError> {
        match &failure.request_id {
            Some(request_id) => {
                self.resolve(request_id, || RequestOutcome::Failure(failure.clone()))
            }
            None => Err(RequestLifecycleError::Uncorrelated),
        }
    }

    /// Deposits one outcome into exactly one pending request's waiter and
    /// removes that request.
    ///
    /// The delivery verdict distinguishes a live waiter from one dropped
    /// between admission and settlement; both settle the request exactly
    /// once and free its capacity.
    fn resolve<F>(
        &mut self,
        request_id: &RequestId,
        build_outcome: F,
    ) -> Result<OutcomeDelivery, RequestLifecycleError>
    where
        F: FnOnce() -> RequestOutcome,
    {
        let position = self
            .pending
            .iter()
            .position(|pending| &pending.request_id == request_id)
            .ok_or(RequestLifecycleError::Unknown)?;
        let pending = self.pending.remove(position);
        {
            let mut slot = lock_slot(&pending.slot);
            slot.settled = true;
            slot.outcome = Some(ResolvedRequest {
                request_id: pending.request_id.clone(),
                outcome: build_outcome(),
            });
        }
        if Arc::strong_count(&pending.slot) == 1 {
            Ok(OutcomeDelivery::Abandoned)
        } else {
            Ok(OutcomeDelivery::Delivered)
        }
    }
}
