//! Per-session correlation of server replies to pending client requests.
//!
//! While one authenticated session awaits Forge's answer to a client
//! request, that request's [`RequestId`] stays pending in the session's
//! [`RequestCorrelationRegistry`]. Every [`ServerResponse`] and every
//! correlated [`ProtocolFailure`] carries the id of the request it settles,
//! so completion removes exactly that pending request and frees its
//! capacity.
//!
//! Correlation is single-use, as required by
//! `docs/decisions/NATIVE_PRODUCT_SCOPE.md`: completing a request retires
//! its correlation identity for the rest of the owner's lifetime. A late
//! replay of an already-settled completion therefore finds nothing pending
//! and is rejected without disturbing any other request, and a retired id
//! is never readmitted, so no second admission can ever share its fate.
//!
//! Construction takes two separate explicit limits: how many requests may
//! be simultaneously pending and how many successful admissions the owner
//! may make over its entire lifetime. Completion frees pending capacity
//! but never restores identity eligibility, so the total budget declines
//! monotonically even when everything has settled; exhausted budgets
//! reject new admissions without evicting, clearing, wrapping, or
//! forgetting any remembered id. Admission and completion are
//! all-or-nothing: every rejected operation returns a typed error and
//! leaves the registry unchanged.
//!
//! One registry belongs to exactly one authenticated connection and lives
//! as long as that live connection; it is never replaced within it. A
//! fresh registry deliberately does not make same-connection id reuse
//! safe: a replacement forgets which identities retired, so a late
//! completion could settle an unrelated newer request. Handling
//! reconnection is a decision above this type, not hidden behavior here.

use std::collections::HashSet;

use artisan_domain::RequestId;
use artisan_protocol::{ProtocolFailure, ServerResponse};
use thiserror::Error;

/// Failure while admitting or completing one correlated request.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum RequestCorrelationError {
    /// The requested maximum number of simultaneously pending requests was
    /// zero, so no request could ever have been admitted.
    #[error("request correlation pending-request limit must be nonzero")]
    ZeroPendingLimit,
    /// The requested total number of lifetime admissions was zero, so no
    /// request could ever have been admitted.
    #[error("request correlation lifetime admission budget must be nonzero")]
    ZeroLifetimeBudget,
    /// The request id is already pending, so admitting it again would let
    /// one reply settle the same request twice.
    #[error("request is already pending correlation")]
    Duplicate,
    /// An earlier completion already settled this request id, and
    /// correlation identities are single-use: readmitting it would let a
    /// late reply reach a request that shares only the recycled identity.
    /// Diagnosed ahead of any capacity condition, at any budget state.
    #[error("request correlation identity was retired by an earlier completion")]
    Retired,
    /// Every pending slot held an unresolved request, so the incoming
    /// request cannot be admitted until one completes. Transient: unlike
    /// lifetime exhaustion, later completions restore this capacity.
    #[error("request correlation reached its capacity of {capacity} pending requests")]
    AtCapacity {
        /// Fixed maximum of simultaneously pending requests that was
        /// exhausted.
        capacity: usize,
    },
    /// The owner already made its entire lifetime allowance of successful
    /// admissions, so no further request can be admitted, whatever is
    /// currently pending. Permanent for this owner's lifetime: rejected
    /// without evicting or forgetting any remembered identity.
    #[error("request correlation exhausted its lifetime budget of {budget} admissions")]
    LifetimeExhausted {
        /// Total lifetime admission budget that was exhausted.
        budget: usize,
    },
    /// The failure carried no request id, so it correlates to no pending
    /// request.
    #[error("protocol failure carries no request id to correlate")]
    Uncorrelated,
    /// The request id is not pending, because it was never admitted, was
    /// never admitted under this owner, or already completed.
    #[error("request is not pending correlation")]
    Unknown,
}

/// Bounded in-memory registry of one session's correlated request
/// identities: the single owner of admission, pending state, and
/// retirement.
///
/// Both limits are chosen once at construction and never change. Deliberately
/// implements neither [`Clone`] nor [`Copy`]: the registry is the single
/// mutable owner of the session's correlation state, and duplicating it
/// could let two owners settle the same request.
#[derive(Debug, Eq, PartialEq)]
pub struct RequestCorrelationRegistry {
    /// Maximum number of simultaneously pending requests.
    pending_capacity: usize,
    /// Total successful admissions allowed over the owner's entire
    /// lifetime.
    admission_budget: usize,
    /// Every successfully admitted request id, retained until the owner is
    /// dropped. Bounded by `admission_budget`, because each admission
    /// inserts exactly one distinct id and readmissions are rejected.
    /// Membership minus `pending` is precisely the set of retired ids.
    remembered: HashSet<RequestId>,
    /// Pending request ids in admission order.
    pending: Vec<RequestId>,
}

impl RequestCorrelationRegistry {
    /// Creates a registry admitting at most `pending_capacity`
    /// simultaneously pending requests and at most `admission_budget`
    /// successful requests over its entire lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`RequestCorrelationError::ZeroPendingLimit`] when
    /// `pending_capacity` is zero, checked first, and
    /// [`RequestCorrelationError::ZeroLifetimeBudget`] when
    /// `admission_budget` is zero.
    pub fn new(
        pending_capacity: usize,
        admission_budget: usize,
    ) -> Result<Self, RequestCorrelationError> {
        if pending_capacity == 0 {
            return Err(RequestCorrelationError::ZeroPendingLimit);
        }
        if admission_budget == 0 {
            return Err(RequestCorrelationError::ZeroLifetimeBudget);
        }
        Ok(Self {
            pending_capacity,
            admission_budget,
            remembered: HashSet::new(),
            pending: Vec::new(),
        })
    }

    /// Returns the fixed maximum number of simultaneously pending requests.
    #[must_use]
    pub const fn pending_capacity(&self) -> usize {
        self.pending_capacity
    }

    /// Returns the fixed total number of successful admissions allowed
    /// during the owner's entire lifetime.
    #[must_use]
    pub const fn admission_budget(&self) -> usize {
        self.admission_budget
    }

    /// Returns how many distinct requests were successfully admitted during
    /// the owner's entire lifetime, including every currently pending one.
    ///
    /// This counts consumed lifetime budget, not pending occupancy; see
    /// [`len`](Self::len) for the latter. It never decreases.
    #[must_use]
    pub fn admitted(&self) -> usize {
        self.remembered.len()
    }

    /// Returns the number of currently pending requests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Returns whether no request is currently pending.
    ///
    /// An empty registry may still be unable to admit: retired identities
    /// and the lifetime budget outlive every settlement.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Returns whether `request_id` currently awaits completion.
    #[must_use]
    pub fn is_pending(&self, request_id: &RequestId) -> bool {
        self.pending.contains(request_id)
    }

    /// Iterates the pending request ids in admission order.
    pub fn pending(&self) -> impl Iterator<Item = &RequestId> {
        self.pending.iter()
    }

    /// Admits one request as pending and remembers its identity for the
    /// owner's entire lifetime.
    ///
    /// Rejections are diagnosed in a fixed precedence, each ahead of the
    /// next:
    ///
    /// 1. [`Duplicate`](RequestCorrelationError::Duplicate), for an id that
    ///    is currently pending, identically at any occupancy or budget
    ///    state;
    /// 2. [`Retired`](RequestCorrelationError::Retired), for an id an
    ///    earlier completion settled, even when every budget is exhausted;
    /// 3. [`LifetimeExhausted`](RequestCorrelationError::LifetimeExhausted),
    ///    permanent exhaustion of the total admission budget, ahead of the
    ///    transient pending condition because no future completion can
    ///    restore it;
    /// 4. [`AtCapacity`](RequestCorrelationError::AtCapacity), when every
    ///    pending slot is occupied.
    ///
    /// On success the registry grows by one pending request and one
    /// remembered identity; on failure it is entirely unchanged.
    ///
    /// # Errors
    ///
    /// Returns the precedence-diagnosed variants documented above.
    pub fn register(&mut self, request_id: RequestId) -> Result<(), RequestCorrelationError> {
        if self.pending.contains(&request_id) {
            return Err(RequestCorrelationError::Duplicate);
        }
        if self.remembered.contains(&request_id) {
            return Err(RequestCorrelationError::Retired);
        }
        if self.remembered.len() >= self.admission_budget {
            return Err(RequestCorrelationError::LifetimeExhausted {
                budget: self.admission_budget,
            });
        }
        if self.pending.len() >= self.pending_capacity {
            return Err(RequestCorrelationError::AtCapacity {
                capacity: self.pending_capacity,
            });
        }
        self.pending.push(request_id.clone());
        self.remembered.insert(request_id);
        Ok(())
    }

    /// Completes the pending request settled by a successful server
    /// response.
    ///
    /// On success exactly the response's request stops pending and its
    /// pending capacity is freed; its correlation identity retires and
    /// remains remembered. On failure the registry is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`RequestCorrelationError::Unknown`] when the response's
    /// request id is not pending, as with a replayed completion or an id
    /// belonging to another connection.
    pub fn complete_on_response(
        &mut self,
        response: &ServerResponse,
    ) -> Result<(), RequestCorrelationError> {
        self.complete(&response.request_id)
    }

    /// Completes the pending request carried by a protocol failure.
    ///
    /// On success exactly the failure's request stops pending and its
    /// pending capacity is freed; its correlation identity retires and
    /// remains remembered. On failure the registry is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`RequestCorrelationError::Uncorrelated`] when the failure
    /// carries no request id, and [`RequestCorrelationError::Unknown`] when
    /// the carried request id is not pending.
    pub fn complete_on_failure(
        &mut self,
        failure: &ProtocolFailure,
    ) -> Result<(), RequestCorrelationError> {
        match &failure.request_id {
            Some(request_id) => self.complete(request_id),
            None => Err(RequestCorrelationError::Uncorrelated),
        }
    }

    /// Removes exactly one pending request id, leaving the registry
    /// unchanged when it is not pending.
    ///
    /// The identity stays remembered: retirement is permanent for the
    /// owner's lifetime, so the freed capacity admits only fresh ids.
    fn complete(&mut self, request_id: &RequestId) -> Result<(), RequestCorrelationError> {
        let position = self
            .pending
            .iter()
            .position(|candidate| *candidate == *request_id)
            .ok_or(RequestCorrelationError::Unknown)?;
        self.pending.remove(position);
        Ok(())
    }
}
