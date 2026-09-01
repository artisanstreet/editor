//! Focused state-machine tests for the backend lifecycle controller.

use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
};

use super::{
    ActivityGate, ActivityGateError, ActivityGateImpl, ActivitySnapshot, ActivityStopReservation,
    CancelHandle, ErrorCode, LifecycleControlReceipt, LifecycleController, LifecycleDispatch,
    LifecycleRequest, LifecycleResponse, LifecycleState, LifecycleStatus, LifecycleStopDisposition,
    RequestId, StopAdmission,
};
use artisan_protocol::ProtocolValueError;

struct TestGate {
    active_work_count: AtomicU32,
    snapshot_calls: AtomicUsize,
    begin_stop_calls: AtomicUsize,
    committed: Arc<AtomicUsize>,
    rolled_back: Arc<AtomicUsize>,
    reserved: Arc<AtomicBool>,
}

impl TestGate {
    fn new(active_work_count: u32) -> Arc<Self> {
        Arc::new(Self {
            active_work_count: AtomicU32::new(active_work_count),
            snapshot_calls: AtomicUsize::new(0),
            begin_stop_calls: AtomicUsize::new(0),
            committed: Arc::new(AtomicUsize::new(0)),
            rolled_back: Arc::new(AtomicUsize::new(0)),
            reserved: Arc::new(AtomicBool::new(false)),
        })
    }

    fn set_active_work_count(&self, count: u32) {
        self.active_work_count.store(count, Ordering::Relaxed);
    }
}

impl ActivityGate for TestGate {
    fn snapshot(&self) -> Result<ActivitySnapshot, ActivityGateError> {
        self.snapshot_calls.fetch_add(1, Ordering::Relaxed);
        Ok(ActivitySnapshot::new(
            self.active_work_count.load(Ordering::Relaxed),
        ))
    }

    fn begin_stop(&self, require_idle: bool) -> Result<StopAdmission, ActivityGateError> {
        self.begin_stop_calls.fetch_add(1, Ordering::Relaxed);
        let active_work_count = self.active_work_count.load(Ordering::Relaxed);
        if require_idle && active_work_count != 0 {
            return Ok(StopAdmission::Busy { active_work_count });
        }
        assert!(!self.reserved.swap(true, Ordering::AcqRel));
        Ok(StopAdmission::Accepted {
            reservation: Box::new(TestReservation {
                commit_count: Arc::clone(&self.committed),
                rolled_back: Arc::clone(&self.rolled_back),
                reserved: Arc::clone(&self.reserved),
                committed: false,
            }),
        })
    }
}

struct TestReservation {
    commit_count: Arc<AtomicUsize>,
    rolled_back: Arc<AtomicUsize>,
    reserved: Arc<AtomicBool>,
    committed: bool,
}

impl ActivityStopReservation for TestReservation {
    fn commit(mut self: Box<Self>) {
        self.committed = true;
        self.commit_count.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for TestReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.rolled_back.fetch_add(1, Ordering::Relaxed);
            self.reserved.store(false, Ordering::Release);
        }
    }
}

struct ErrorGate(ActivityGateError);

impl ActivityGate for ErrorGate {
    fn snapshot(&self) -> Result<ActivitySnapshot, ActivityGateError> {
        Err(self.0)
    }

    fn begin_stop(&self, _require_idle: bool) -> Result<StopAdmission, ActivityGateError> {
        Err(self.0)
    }
}

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("fixture request id should be valid")
}

fn controller(active_work_count: u32) -> (LifecycleController, Arc<TestGate>) {
    let gate = TestGate::new(active_work_count);
    let controller = LifecycleController::with_activity_gate(gate.clone());
    (controller, gate)
}

fn status_of(dispatch: LifecycleDispatch) -> LifecycleStatus {
    match dispatch {
        LifecycleDispatch::Reply {
            response: LifecycleResponse::Status(status),
            receipt,
        } => {
            assert!(receipt.pending.is_none());
            status
        }
        LifecycleDispatch::Reply { .. } => panic!("expected a lifecycle status"),
        LifecycleDispatch::Failure(failure) => panic!("unexpected lifecycle failure: {failure:?}"),
    }
}

fn stop_of(dispatch: LifecycleDispatch) -> (LifecycleStopDisposition, LifecycleControlReceipt) {
    match dispatch {
        LifecycleDispatch::Reply {
            response: LifecycleResponse::Stop(receipt),
            receipt: local,
        } => (receipt.disposition, local),
        LifecycleDispatch::Reply { .. } => panic!("expected a lifecycle stop"),
        LifecycleDispatch::Failure(failure) => panic!("unexpected lifecycle failure: {failure:?}"),
    }
}

fn failure_of(dispatch: LifecycleDispatch) -> artisan_protocol::ProtocolFailure {
    match dispatch {
        LifecycleDispatch::Failure(failure) => failure,
        LifecycleDispatch::Reply { .. } => panic!("expected a lifecycle failure"),
    }
}

#[tokio::test]
async fn unconfigured_controller_is_unavailable_and_fails_closed() {
    let controller = LifecycleController::new();
    assert!(!controller.implementation_available());
    let debug = format!("{controller:?}");
    assert!(debug.contains("implementation_available: false"));
    assert!(!debug.contains("request"));

    let failure = failure_of(
        controller
            .dispatch(request_id("unconfigured"), LifecycleRequest::Status)
            .await,
    );
    assert_eq!(failure.code, ErrorCode::UnsupportedFeature);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request_id("unconfigured")));
}

#[tokio::test]
async fn status_maps_ready_busy_and_draining_with_protocol_invariants() {
    let (controller, gate) = controller(0);
    assert_eq!(
        status_of(
            controller
                .dispatch(request_id("ready-status"), LifecycleRequest::Status)
                .await,
        ),
        LifecycleStatus::new(LifecycleState::Ready, 0).expect("ready status is valid")
    );

    gate.set_active_work_count(2);
    assert_eq!(
        status_of(
            controller
                .dispatch(request_id("busy-status"), LifecycleRequest::Status)
                .await,
        ),
        LifecycleStatus::new(LifecycleState::Busy, 2).expect("busy status is valid")
    );

    gate.set_active_work_count(1);
    let (disposition, receipt) = stop_of(
        controller
            .dispatch(
                request_id("drain-status-stop"),
                LifecycleRequest::Stop {
                    require_idle: false,
                },
            )
            .await,
    );
    assert_eq!(disposition, LifecycleStopDisposition::Accepted);
    receipt.commit_after_response(&CancelHandle::new());
    assert_eq!(
        status_of(
            controller
                .dispatch(request_id("draining-status"), LifecycleRequest::Status)
                .await,
        ),
        LifecycleStatus::new(LifecycleState::Draining, 1)
            .expect("draining status permits active work")
    );
}

#[test]
fn lifecycle_status_rejects_inconsistent_state_and_count_pairs() {
    assert_eq!(
        LifecycleStatus::new(LifecycleState::Ready, 1),
        Err(ProtocolValueError::InvalidLifecycleStatus {
            state: LifecycleState::Ready,
            active_work_count: 1,
        })
    );
    assert_eq!(
        LifecycleStatus::new(LifecycleState::Busy, 0),
        Err(ProtocolValueError::InvalidLifecycleStatus {
            state: LifecycleState::Busy,
            active_work_count: 0,
        })
    );
    assert!(LifecycleStatus::new(LifecycleState::Draining, 0).is_ok());
}

#[tokio::test]
async fn require_idle_busy_rejection_does_not_mutate_controller_state() {
    let (controller, gate) = controller(1);
    let failure = failure_of(
        controller
            .dispatch(
                request_id("busy-stop"),
                LifecycleRequest::Stop { require_idle: true },
            )
            .await,
    );
    assert_eq!(failure.code, ErrorCode::LifecycleBusy);
    assert!(failure.retryable);
    assert_eq!(gate.begin_stop_calls.load(Ordering::Relaxed), 1);
    assert_eq!(gate.committed.load(Ordering::Relaxed), 0);
    assert_eq!(gate.rolled_back.load(Ordering::Relaxed), 0);
    assert_eq!(
        status_of(
            controller
                .dispatch(request_id("still-busy"), LifecycleRequest::Status)
                .await,
        )
        .state,
        LifecycleState::Busy
    );
}

#[tokio::test]
async fn force_stop_reserves_active_work_and_stays_pending_until_commit() {
    let (controller, gate) = controller(3);
    let cancel = CancelHandle::new();
    let (disposition, receipt) = stop_of(
        controller
            .dispatch(
                request_id("force-stop"),
                LifecycleRequest::Stop {
                    require_idle: false,
                },
            )
            .await,
    );
    assert_eq!(disposition, LifecycleStopDisposition::Accepted);
    assert!(controller.state.try_lock().is_err());
    assert!(!cancel.is_cancelled());

    drop(receipt);
    assert_eq!(gate.rolled_back.load(Ordering::Relaxed), 1);
    assert_eq!(gate.committed.load(Ordering::Relaxed), 0);
    assert!(!cancel.is_cancelled());
    assert!(controller.state.try_lock().is_ok());
}

#[tokio::test]
async fn committed_receipt_transitions_to_draining_and_cancels_once() {
    let (controller, gate) = controller(0);
    let cancel = CancelHandle::new();
    let (disposition, receipt) = stop_of(
        controller
            .dispatch(
                request_id("commit-stop"),
                LifecycleRequest::Stop { require_idle: true },
            )
            .await,
    );
    assert_eq!(disposition, LifecycleStopDisposition::Accepted);
    assert!(!cancel.is_cancelled());
    receipt.commit_after_response(&cancel);
    assert!(cancel.is_cancelled());
    assert_eq!(gate.committed.load(Ordering::Relaxed), 1);
    assert_eq!(gate.rolled_back.load(Ordering::Relaxed), 0);

    let (duplicate, duplicate_receipt) = stop_of(
        controller
            .dispatch(
                request_id("commit-stop"),
                LifecycleRequest::Stop { require_idle: true },
            )
            .await,
    );
    assert_eq!(duplicate, LifecycleStopDisposition::Duplicate);
    drop(duplicate_receipt);
    assert_eq!(gate.committed.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn concurrent_stop_admissions_have_one_serialized_winner() {
    let (controller, gate) = controller(0);
    let controller = Arc::new(controller);
    let cancel = CancelHandle::new();
    let left_controller = Arc::clone(&controller);
    let right_controller = Arc::clone(&controller);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
    let left_sender = sender.clone();
    let left = tokio::spawn(async move {
        let dispatch = left_controller
            .dispatch(
                request_id("race-left"),
                LifecycleRequest::Stop {
                    require_idle: false,
                },
            )
            .await;
        left_sender
            .send(dispatch)
            .await
            .expect("race receiver should remain open");
    });
    let right_sender = sender.clone();
    let right = tokio::spawn(async move {
        let dispatch = right_controller
            .dispatch(
                request_id("race-right"),
                LifecycleRequest::Stop {
                    require_idle: false,
                },
            )
            .await;
        right_sender
            .send(dispatch)
            .await
            .expect("race receiver should remain open");
    });
    drop(sender);

    let winner = receiver
        .recv()
        .await
        .expect("one race task should be admitted");
    let (winner_disposition, winner_receipt) = stop_of(winner);
    assert_eq!(winner_disposition, LifecycleStopDisposition::Accepted);
    assert!(!cancel.is_cancelled());
    winner_receipt.commit_after_response(&cancel);

    let (loser_disposition, loser_receipt) = stop_of(
        receiver
            .recv()
            .await
            .expect("losing race task should settle after commit"),
    );
    assert_eq!(loser_disposition, LifecycleStopDisposition::AlreadyStopping);
    drop(loser_receipt);
    left.await.expect("left race task should settle");
    right.await.expect("right race task should settle");
    assert_eq!(gate.committed.load(Ordering::Relaxed), 1);
    assert_eq!(gate.rolled_back.load(Ordering::Relaxed), 0);
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn draining_stop_keys_are_idempotent_and_conflicts_are_bounded() {
    let (controller, _gate) = controller(0);
    let cancel = CancelHandle::new();
    let (_, receipt) = stop_of(
        controller
            .dispatch(
                request_id("accepted-stop"),
                LifecycleRequest::Stop {
                    require_idle: false,
                },
            )
            .await,
    );
    receipt.commit_after_response(&cancel);

    let conflict = failure_of(
        controller
            .dispatch(
                request_id("accepted-stop"),
                LifecycleRequest::Stop { require_idle: true },
            )
            .await,
    );
    assert_eq!(conflict.code, ErrorCode::IdempotencyConflict);
    assert!(!conflict.retryable);
    assert_eq!(conflict.request_id, Some(request_id("accepted-stop")));

    let (already_stopping, receipt) = stop_of(
        controller
            .dispatch(
                request_id("other-stop"),
                LifecycleRequest::Stop { require_idle: true },
            )
            .await,
    );
    assert_eq!(already_stopping, LifecycleStopDisposition::AlreadyStopping);
    drop(receipt);
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn activity_errors_are_payload_free_and_bounded() {
    let unavailable = LifecycleController::with_activity_gate(Arc::new(ErrorGate(
        ActivityGateError::Unavailable,
    )));
    let failure = failure_of(
        unavailable
            .dispatch(request_id("activity-error"), LifecycleRequest::Status)
            .await,
    );
    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(failure.retryable);
    assert!(failure.detail.as_str().len() <= artisan_protocol::ERROR_DETAIL_MAX_BYTES);
    assert!(!failure.detail.as_str().contains("activity-error"));

    let out_of_range = LifecycleController::with_activity_gate(Arc::new(ErrorGate(
        ActivityGateError::CountOutOfRange,
    )));
    let failure = failure_of(
        out_of_range
            .dispatch(request_id("count-error"), LifecycleRequest::Status)
            .await,
    );
    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(!failure.retryable);
    assert!(!format!("{out_of_range:?}").contains("count-error"));
}

#[test]
fn concrete_activity_gate_counts_and_rejects_overflow_without_mutation() {
    let gate = ActivityGateImpl::new();
    assert_eq!(
        gate.snapshot()
            .expect("a new activity gate should be readable")
            .active_work_count(),
        0
    );

    let lease = gate.acquire().expect("open activity should admit work");
    assert_eq!(
        gate.snapshot()
            .expect("an admitted activity gate should be readable")
            .active_work_count(),
        1
    );
    drop(lease);
    assert_eq!(
        gate.snapshot()
            .expect("released activity should be readable")
            .active_work_count(),
        0
    );

    let full = ActivityGateImpl::with_active_work_count(u32::MAX);
    assert!(matches!(
        full.acquire(),
        Err(ActivityGateError::CountOutOfRange)
    ));
    assert_eq!(
        full.snapshot()
            .expect("overflow must not poison the activity gate")
            .active_work_count(),
        u32::MAX
    );
}

#[test]
fn concrete_activity_gate_rolls_back_and_commits_stop_reservations() {
    let gate = ActivityGateImpl::new();
    let reservation = match gate
        .begin_stop(true)
        .expect("an open idle gate should accept a stop")
    {
        StopAdmission::Accepted { reservation } => reservation,
        StopAdmission::Busy { active_work_count } => {
            panic!("new gate unexpectedly reported {active_work_count} active units")
        }
    };
    assert!(matches!(
        gate.acquire(),
        Err(ActivityGateError::Unavailable)
    ));
    drop(reservation);

    let lease = gate
        .acquire()
        .expect("dropping a provisional stop should reopen admission");
    drop(lease);

    let reservation = match gate
        .begin_stop(true)
        .expect("the reopened idle gate should accept another stop")
    {
        StopAdmission::Accepted { reservation } => reservation,
        StopAdmission::Busy { active_work_count } => {
            panic!("reopened gate unexpectedly reported {active_work_count} active units")
        }
    };
    reservation.commit();
    assert!(matches!(
        gate.acquire(),
        Err(ActivityGateError::Unavailable)
    ));
    assert_eq!(
        gate.snapshot()
            .expect("a sealed gate should remain readable")
            .active_work_count(),
        0
    );
}

#[test]
fn concrete_activity_gate_forced_stop_seals_around_existing_work() {
    let gate = ActivityGateImpl::new();
    let lease = gate.acquire().expect("existing work should be admitted");
    match gate
        .begin_stop(true)
        .expect("idle stop should have a concrete busy result")
    {
        StopAdmission::Busy { active_work_count } => assert_eq!(active_work_count, 1),
        StopAdmission::Accepted { reservation } => {
            drop(reservation);
            panic!("idle stop unexpectedly reserved active work");
        }
    }
    let reservation = match gate
        .begin_stop(false)
        .expect("forced stop should reserve an active gate")
    {
        StopAdmission::Accepted { reservation } => reservation,
        StopAdmission::Busy { active_work_count } => {
            panic!("forced stop unexpectedly reported {active_work_count} active units")
        }
    };
    assert_eq!(
        gate.snapshot()
            .expect("forced reservation should preserve active work")
            .active_work_count(),
        1
    );
    assert!(matches!(
        gate.acquire(),
        Err(ActivityGateError::Unavailable)
    ));
    reservation.commit();
    drop(lease);
    assert_eq!(
        gate.snapshot()
            .expect("sealed forced stop should remain readable")
            .active_work_count(),
        0
    );
    assert!(matches!(
        gate.acquire(),
        Err(ActivityGateError::Unavailable)
    ));
}

#[test]
fn concrete_activity_gate_idle_stop_and_acquire_have_one_linearized_winner() {
    for _ in 0..16 {
        let gate = Arc::new(ActivityGateImpl::new());
        let barrier = Arc::new(Barrier::new(2));
        let (sender, receiver) = std::sync::mpsc::channel();

        let acquire_gate = Arc::clone(&gate);
        let acquire_barrier = Arc::clone(&barrier);
        let acquire_sender = sender.clone();
        let acquire_thread = std::thread::spawn(move || {
            let lease = acquire_gate.acquire();
            acquire_barrier.wait();
            let acquired = match lease {
                Ok(lease) => {
                    acquire_sender
                        .send(true)
                        .expect("linearization receiver should remain open");
                    drop(lease);
                    true
                }
                Err(ActivityGateError::Unavailable) => {
                    acquire_sender
                        .send(false)
                        .expect("linearization receiver should remain open");
                    false
                }
                Err(error) => panic!("unexpected acquisition error: {error:?}"),
            };
            acquired
        });

        let stop_gate = Arc::clone(&gate);
        let stop_barrier = Arc::clone(&barrier);
        let stop_thread = std::thread::spawn(move || {
            let admission = stop_gate
                .begin_stop(true)
                .expect("the concurrent stop should have a gate result");
            stop_barrier.wait();
            let accepted = matches!(&admission, StopAdmission::Accepted { .. });
            sender
                .send(accepted)
                .expect("linearization receiver should remain open");
            drop(admission);
            accepted
        });

        let acquire_won = acquire_thread
            .join()
            .expect("acquisition race thread should finish");
        let stop_won = stop_thread.join().expect("stop race thread should finish");
        let mut observed = vec![
            receiver.recv().expect("first race result should arrive"),
            receiver.recv().expect("second race result should arrive"),
        ];
        observed.sort_unstable();
        assert_eq!(observed, vec![false, true]);
        assert_ne!(acquire_won, stop_won);
        assert_eq!(
            gate.snapshot()
                .expect("rolled-back race should be readable")
                .active_work_count(),
            0
        );
        let lease = gate
            .acquire()
            .expect("the losing stop or released acquisition should leave the gate open");
        drop(lease);
    }
}
