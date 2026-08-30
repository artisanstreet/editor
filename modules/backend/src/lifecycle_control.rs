//! Authenticated native lifecycle-control state and activity boundary.
//!
//! The controller owns only the small state machine needed to authorize one
//! globally serialized stop. Concrete activity accounting is supplied by the
//! later activity packet through the crate-private [`ActivityGate`] seam. The
//! public constructor intentionally has no gate and therefore fails closed.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::fmt;
use std::sync::Arc;

use artisan_domain::RequestId;
use artisan_protocol::{
    ErrorCode, ErrorDetail, LifecycleRequest, LifecycleResponse, LifecycleState, LifecycleStatus,
    LifecycleStopDisposition, LifecycleStopReceipt, ProtocolFailure,
};
use artisan_transport::CancelHandle;
use thiserror::Error;
use tokio::sync::{Mutex, OwnedMutexGuard};

const UNAVAILABLE_DETAIL: &str = "native lifecycle control is unavailable in this build";
const ACTIVITY_UNAVAILABLE_DETAIL: &str = "native lifecycle activity is unavailable";
const ACTIVITY_COUNT_DETAIL: &str = "native lifecycle activity count is not representable";
const BUSY_DETAIL: &str = "native lifecycle control requires idle activity";
const IDEMPOTENCY_CONFLICT_DETAIL: &str =
    "native lifecycle stop request conflicts with the accepted stop";
const INVALID_STATUS_DETAIL: &str = "native lifecycle produced an invalid status";
const PENDING_STATE_DETAIL: &str = "native lifecycle control is settling another stop";

/// A backend lifecycle controller shared by all authenticated connections.
///
/// `LifecycleController::new` is deliberately fail-closed. The next activity
/// packet supplies a concrete [`ActivityGate`] through the crate-private
/// constructor before lifecycle control is advertised to a peer.
pub struct LifecycleController {
    state: Arc<Mutex<ControlState>>,
    activity: Option<Arc<dyn ActivityGate>>,
}

/// The narrow activity seam owned by the lifecycle activity packet.
pub(crate) trait ActivityGate: Send + Sync {
    fn snapshot(&self) -> Result<ActivitySnapshot, ActivityGateError>;

    fn begin_stop(&self, require_idle: bool) -> Result<StopAdmission, ActivityGateError>;
}

/// An atomic snapshot of activity owned by the activity gate.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct ActivitySnapshot {
    active_work_count: u32,
}

impl ActivitySnapshot {
    /// Creates an activity snapshot for a crate-local gate implementation.
    pub(crate) const fn new(active_work_count: u32) -> Self {
        Self { active_work_count }
    }

    /// Returns the count captured by this snapshot.
    pub(crate) const fn active_work_count(self) -> u32 {
        self.active_work_count
    }
}

/// Result of the activity gate's linear stop-admission operation.
pub(crate) enum StopAdmission {
    Busy {
        active_work_count: u32,
    },
    Accepted {
        reservation: Box<dyn ActivityStopReservation>,
    },
}

/// A linear provisional stop admission.
pub(crate) trait ActivityStopReservation: Send {
    /// Commits the provisional admission synchronously and infallibly.
    fn commit(self: Box<Self>);
}

/// Payload-free failure returned by the activity boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum ActivityGateError {
    /// The activity implementation is temporarily unavailable.
    #[error("lifecycle activity is unavailable")]
    Unavailable,

    /// The activity implementation could not represent its count as `u32`.
    #[error("lifecycle activity count is not representable")]
    CountOutOfRange,
}

enum ControlState {
    Ready,
    Pending(StopKey),
    Draining(StopKey),
}

#[derive(Clone)]
struct StopKey {
    request_id: RequestId,
    require_idle: bool,
}

/// Local result of one lifecycle dispatch.
pub(crate) enum LifecycleDispatch {
    Reply {
        response: LifecycleResponse,
        receipt: LifecycleControlReceipt,
    },
    Failure(ProtocolFailure),
}

/// Local custody proving that an accepted stop's response and FIN succeeded.
///
/// The pending controller mutex guard remains inside this value until the
/// connection releases it after successful response FIN. Dropping it before
/// commit restores `Ready` and rolls back the activity reservation.
pub(crate) struct LifecycleControlReceipt {
    pending: Option<PendingStop>,
}

struct PendingStop {
    guard: OwnedMutexGuard<ControlState>,
    key: StopKey,
    reservation: Box<dyn ActivityStopReservation>,
}

impl fmt::Debug for LifecycleController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LifecycleController")
            .field("implementation_available", &self.implementation_available())
            .finish()
    }
}

impl Default for LifecycleController {
    fn default() -> Self {
        Self::new()
    }
}

impl LifecycleController {
    /// Creates an unconfigured, fail-closed lifecycle controller.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ControlState::Ready)),
            activity: None,
        }
    }

    /// Installs the crate-local activity implementation used for negotiation
    /// and lifecycle dispatch.
    pub(crate) fn with_activity_gate(activity: Arc<dyn ActivityGate>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ControlState::Ready)),
            activity: Some(activity),
        }
    }

    /// Returns whether this controller has an installed activity boundary.
    pub(crate) fn implementation_available(&self) -> bool {
        self.activity.is_some()
    }

    /// Dispatches one authenticated lifecycle request through the serialized
    /// controller state machine.
    pub(crate) async fn dispatch(
        &self,
        request_id: RequestId,
        request: LifecycleRequest,
    ) -> LifecycleDispatch {
        let Some(activity) = self.activity.as_ref().map(Arc::clone) else {
            return LifecycleDispatch::Failure(protocol_failure(
                ErrorCode::UnsupportedFeature,
                UNAVAILABLE_DETAIL,
                false,
                request_id,
            ));
        };

        let mut guard = Arc::clone(&self.state).lock_owned().await;
        let observed = match &*guard {
            ControlState::Ready => ObservedState::Ready,
            ControlState::Pending(_) => ObservedState::Pending,
            ControlState::Draining(key) => ObservedState::Draining(key.clone()),
        };

        match observed {
            ObservedState::Ready => match request {
                LifecycleRequest::Status => {
                    status_dispatch(activity.as_ref(), LifecycleState::Ready, &request_id)
                }
                LifecycleRequest::Stop { require_idle } => {
                    match activity.begin_stop(require_idle) {
                        Ok(StopAdmission::Busy { .. }) => {
                            LifecycleDispatch::Failure(protocol_failure(
                                ErrorCode::LifecycleBusy,
                                BUSY_DETAIL,
                                true,
                                request_id,
                            ))
                        }
                        Ok(StopAdmission::Accepted { reservation }) => {
                            let key = StopKey {
                                request_id: request_id.clone(),
                                require_idle,
                            };
                            *guard = ControlState::Pending(key.clone());
                            LifecycleDispatch::Reply {
                                response: LifecycleResponse::Stop(LifecycleStopReceipt {
                                    disposition: LifecycleStopDisposition::Accepted,
                                    state: LifecycleState::Draining,
                                }),
                                receipt: LifecycleControlReceipt {
                                    pending: Some(PendingStop {
                                        guard,
                                        key,
                                        reservation,
                                    }),
                                },
                            }
                        }
                        Err(error) => {
                            LifecycleDispatch::Failure(activity_failure(error, request_id))
                        }
                    }
                }
            },
            ObservedState::Pending => LifecycleDispatch::Failure(protocol_failure(
                ErrorCode::Internal,
                PENDING_STATE_DETAIL,
                true,
                request_id,
            )),
            ObservedState::Draining(key) => match request {
                LifecycleRequest::Status => {
                    status_dispatch(activity.as_ref(), LifecycleState::Draining, &request_id)
                }
                LifecycleRequest::Stop { require_idle } => {
                    let disposition = if key.request_id == request_id {
                        if key.require_idle == require_idle {
                            LifecycleStopDisposition::Duplicate
                        } else {
                            return LifecycleDispatch::Failure(protocol_failure(
                                ErrorCode::IdempotencyConflict,
                                IDEMPOTENCY_CONFLICT_DETAIL,
                                false,
                                request_id,
                            ));
                        }
                    } else {
                        LifecycleStopDisposition::AlreadyStopping
                    };
                    LifecycleDispatch::Reply {
                        response: LifecycleResponse::Stop(LifecycleStopReceipt {
                            disposition,
                            state: LifecycleState::Draining,
                        }),
                        receipt: LifecycleControlReceipt::none(),
                    }
                }
            },
        }
    }
}

impl LifecycleControlReceipt {
    pub(crate) const fn none() -> Self {
        Self { pending: None }
    }

    /// Commits the accepted stop after its response and FIN have succeeded.
    pub(crate) fn commit_after_response(mut self, cancel: &CancelHandle) {
        let Some(PendingStop {
            mut guard,
            key,
            reservation,
        }) = self.pending.take()
        else {
            return;
        };

        *guard = ControlState::Draining(key);
        reservation.commit();
        drop(guard);
        cancel.cancel();
    }
}

impl Drop for LifecycleControlReceipt {
    fn drop(&mut self) {
        let Some(PendingStop {
            mut guard,
            reservation,
            key: _,
        }) = self.pending.take()
        else {
            return;
        };

        *guard = ControlState::Ready;
        drop(reservation);
        drop(guard);
    }
}

enum ObservedState {
    Ready,
    Pending,
    Draining(StopKey),
}

fn status_dispatch(
    activity: &dyn ActivityGate,
    controller_state: LifecycleState,
    request_id: &RequestId,
) -> LifecycleDispatch {
    let snapshot = match activity.snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return LifecycleDispatch::Failure(activity_failure(error, request_id.clone()));
        }
    };
    let active_work_count = snapshot.active_work_count();
    let state = match controller_state {
        LifecycleState::Ready if active_work_count == 0 => LifecycleState::Ready,
        LifecycleState::Ready => LifecycleState::Busy,
        LifecycleState::Busy => LifecycleState::Busy,
        LifecycleState::Draining => LifecycleState::Draining,
    };
    match LifecycleStatus::new(state, active_work_count) {
        Ok(status) => LifecycleDispatch::Reply {
            response: LifecycleResponse::Status(status),
            receipt: LifecycleControlReceipt::none(),
        },
        Err(_) => LifecycleDispatch::Failure(protocol_failure(
            ErrorCode::Internal,
            INVALID_STATUS_DETAIL,
            false,
            request_id.clone(),
        )),
    }
}

fn activity_failure(error: ActivityGateError, request_id: RequestId) -> ProtocolFailure {
    match error {
        ActivityGateError::Unavailable => protocol_failure(
            ErrorCode::Internal,
            ACTIVITY_UNAVAILABLE_DETAIL,
            true,
            request_id,
        ),
        ActivityGateError::CountOutOfRange => protocol_failure(
            ErrorCode::Internal,
            ACTIVITY_COUNT_DETAIL,
            false,
            request_id,
        ),
    }
}

fn protocol_failure(
    code: ErrorCode,
    detail: &'static str,
    retryable: bool,
    request_id: RequestId,
) -> ProtocolFailure {
    ProtocolFailure {
        code,
        detail: ErrorDetail::parse(detail).expect("lifecycle detail is within the protocol bound"),
        retryable,
        request_id: Some(request_id),
    }
}

#[cfg(test)]
#[path = "../../../tests/backend/lifecycle_control.rs"]
mod lifecycle_control_tests;
