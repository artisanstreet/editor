//! Per-session correlation of server replies to pending client requests.
//!
//! While one authenticated session awaits Forge's answer to a client
//! request, that request's [`RequestId`] stays pending in the session's
//! [`RequestCorrelationRegistry`]. Every [`ServerResponse`] and every
//! correlated [`ProtocolFailure`] carries the id of the request it settles,
//! so completion removes exactly that pending request and frees its
//! capacity. Admission and completion are all-or-nothing: every rejected
//! operation returns a typed error and leaves the registry unchanged.
//! Capacity is explicit at construction and enforced before insertion.

use artisan_domain::RequestId;
use artisan_protocol::{ProtocolFailure, ServerResponse};
use thiserror::Error;

/// Failure while admitting or completing one correlated request.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum RequestCorrelationError {
    /// The requested registry capacity was zero, so no request could ever
    /// have been admitted.
    #[error("request correlation registry capacity must be nonzero")]
    ZeroCapacity,
    /// The request id is already pending, so admitting it again would let
    /// one reply settle the same request twice.
    #[error("request is already pending correlation")]
    Duplicate,
    /// Every registry slot held an unresolved request, so the incoming
    /// request cannot be admitted until one completes.
    #[error("request correlation registry reached its capacity of {capacity} pending requests")]
    AtCapacity {
        /// Fixed registry capacity that was exhausted.
        capacity: usize,
    },
    /// The failure carried no request id, so it correlates to no pending
    /// request.
    #[error("protocol failure carries no request id to correlate")]
    Uncorrelated,
    /// The request id is not pending, because it was never admitted or was
    /// already completed.
    #[error("request is not pending correlation")]
    Unknown,
}

/// Bounded in-memory registry of one session's pending request ids.
///
/// Capacity is chosen once at construction and never changes. Deliberately
/// implements neither [`Clone`] nor [`Copy`]: the registry is the single
/// mutable owner of the session's correlation state, and duplicating it
/// could let two owners settle the same request.
#[derive(Debug, Eq, PartialEq)]
pub struct RequestCorrelationRegistry {
    /// Maximum number of simultaneously pending requests.
    capacity: usize,
    /// Pending request ids in admission order.
    pending: Vec<RequestId>,
}

impl RequestCorrelationRegistry {
    /// Creates a registry admitting at most `capacity` pending requests.
    ///
    /// # Errors
    ///
    /// Returns [`RequestCorrelationError::ZeroCapacity`] when `capacity` is
    /// zero.
    pub fn new(capacity: usize) -> Result<Self, RequestCorrelationError> {
        if capacity == 0 {
            return Err(RequestCorrelationError::ZeroCapacity);
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
        self.pending.contains(request_id)
    }

    /// Iterates the pending request ids in admission order.
    pub fn pending(&self) -> impl Iterator<Item = &RequestId> {
        self.pending.iter()
    }

    /// Admits one request as pending.
    ///
    /// Duplicate detection precedes capacity enforcement, so a repeated id
    /// is diagnosed identically at any occupancy. On success the registry
    /// grows by one pending request; on failure it is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`RequestCorrelationError::Duplicate`] when `request_id` is
    /// already pending, and [`RequestCorrelationError::AtCapacity`] when
    /// every slot holds an unresolved request.
    pub fn register(&mut self, request_id: RequestId) -> Result<(), RequestCorrelationError> {
        if self.pending.contains(&request_id) {
            return Err(RequestCorrelationError::Duplicate);
        }
        if self.pending.len() >= self.capacity {
            return Err(RequestCorrelationError::AtCapacity {
                capacity: self.capacity,
            });
        }
        self.pending.push(request_id);
        Ok(())
    }

    /// Completes the pending request settled by a successful server
    /// response.
    ///
    /// On success exactly the response's request stops pending and its
    /// capacity is freed; on failure the registry is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`RequestCorrelationError::Unknown`] when the response's
    /// request id is not pending.
    pub fn complete_on_response(
        &mut self,
        response: &ServerResponse,
    ) -> Result<(), RequestCorrelationError> {
        self.complete(&response.request_id)
    }

    /// Completes the pending request carried by a protocol failure.
    ///
    /// On success exactly the failure's request stops pending and its
    /// capacity is freed; on failure the registry is unchanged.
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
