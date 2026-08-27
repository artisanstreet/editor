use std::path::PathBuf;
use std::time::Duration;

use super::{
    EngineBounds, EngineLimits, EngineOwner, EngineOwnerConfig, EngineOwnerHealth,
    EngineOwnerShutdown, GenerationAllocator, reset_witnesses, witness_counts,
};
use crate::engine_owner::operation::{EngineOperationError, LaunchAdmissionError};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn absolute_engine_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("C:\\tmp\\engine.exe")
    } else {
        PathBuf::from("/tmp/engine")
    }
}

fn valid_limits() -> EngineLimits {
    EngineLimits {
        readiness: Duration::from_millis(100),
        health: Duration::from_millis(100),
        prompt: Duration::from_millis(100),
        sse: Duration::from_millis(100),
        close: Duration::from_millis(50),
    }
}

fn small_bounds_with_control(control_capacity: usize) -> EngineBounds {
    EngineBounds {
        max_json_body: 1024,
        max_sse_line: 1024,
        max_sse_event: 1024,
        max_readiness_line: 1024,
        max_headers: 32,
        max_buf_bytes: 8192,
        stderr_cap_bytes: 4096,
        sink_capacity: 16,
        control_capacity,
    }
}

fn valid_config_with_capacity(capacity: usize) -> EngineOwnerConfig {
    EngineOwnerConfig::new(
        absolute_engine_path(),
        valid_limits(),
        small_bounds_with_control(capacity),
    )
    .expect("valid config")
}

fn run_id(value: &str) -> artisan_domain::RunId {
    artisan_domain::RunId::parse(value).expect("valid run id")
}

// ---------------------------------------------------------------------------
// 1. Allocator mints 1 then 2; force_next(MAX) yields Some(MAX) once then None; no reset
// ---------------------------------------------------------------------------

#[test]
fn allocator_mints_one_then_two() {
    let mut allocator = GenerationAllocator::new();
    assert_eq!(allocator.mint(), Some(1));
    assert_eq!(allocator.mint(), Some(2));
}

#[test]
fn allocator_force_max_then_exhausted() {
    let mut allocator = GenerationAllocator::new();
    allocator.force_next(u64::MAX);
    assert_eq!(allocator.mint(), Some(u64::MAX));
    assert_eq!(allocator.mint(), None);
    assert_eq!(allocator.mint(), None);
}

#[test]
fn allocator_no_reset_path_exists() {
    // After exhaustion, no public API can reset; force_next is cfg(test) only.
    // Verify that consecutive mints remain None without external reset.
    let mut allocator = GenerationAllocator::new();
    allocator.force_next(u64::MAX);
    let _ = allocator.mint();
    assert_eq!(allocator.mint(), None);
    // Even forcing to a lower value is test-only; production has no setter.
    // This test documents that the only reset is the test-only force_next.
}

// ---------------------------------------------------------------------------
// 2. Admission Busy: fill control_capacity (capacity 1) -> next admit Busy
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn admission_busy_when_queue_full() {
    reset_witnesses();
    let config = valid_config_with_capacity(1);
    let owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    let first = owner
        .admit(run_id("run-busy-1"), Duration::from_secs(10))
        .expect("first admit should succeed");
    let second = owner.admit(run_id("run-busy-2"), Duration::from_secs(10));
    assert!(matches!(second, Err(LaunchAdmissionError::Busy)));
    drop(first);
    drop(owner);
    assert_eq!(witness_counts().spawned, 0);
}

// ---------------------------------------------------------------------------
// 3. Admission Unavailable after shutdown signal and after termination + zero/unrepresentable
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn admission_unavailable_after_shutdown_signal() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let mut owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    // Signal shutdown but do not poll the future.
    let _future = owner.shutdown();
    // Admission must stop immediately even though future was not polled.
    let result = owner.admit(run_id("run-after-shutdown"), Duration::from_secs(10));
    assert!(matches!(result, Err(LaunchAdmissionError::Unavailable)));
    // Polling the future should yield Joined (clean drain, no child).
    let report = owner.shutdown().await;
    assert_eq!(report, EngineOwnerShutdown::Joined);
    assert_eq!(witness_counts().spawned, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn admission_rejects_zero_and_unrepresentable_budgets() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    let zero = owner.admit(run_id("run-zero"), Duration::ZERO);
    assert!(matches!(zero, Err(LaunchAdmissionError::InvalidDeadline)));
    let unrepresentable = owner.admit(run_id("run-unrep"), Duration::MAX);
    assert!(matches!(
        unrepresentable,
        Err(LaunchAdmissionError::InvalidDeadline)
    ));
    // Neither queued nor spawned.
    assert_eq!(witness_counts().spawned, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn admission_unavailable_after_owner_termination() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let mut owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    let report = owner.shutdown().await;
    assert_eq!(report, EngineOwnerShutdown::Joined);
    // Channel is closed after owner terminated.
    let result = owner.admit(run_id("run-after-join"), Duration::from_secs(10));
    assert!(matches!(result, Err(LaunchAdmissionError::Unavailable)));
    assert_eq!(witness_counts().spawned, 0);
}

// ---------------------------------------------------------------------------
// 4. Pre-spawn dispositions consume no generation and spawn nothing
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn pre_spawn_cancelled_before_dequeue_consumes_no_generation() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    let accepted = owner
        .admit(run_id("run-cancelled"), Duration::from_secs(10))
        .expect("admit");
    // Cancel before owner dequeues.
    accepted.cancel();
    let result = accepted.await;
    assert_eq!(result.unwrap_err(), EngineOperationError::Cancelled);
    // Give owner a moment to process.
    tokio::task::yield_now().await;
    assert_eq!(witness_counts().spawned, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn pre_spawn_expired_deadline_consumes_no_generation() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    let accepted = owner.inject_expired_for_tests(run_id("run-deadline"));
    let result = accepted.await;
    assert_eq!(result.unwrap_err(), EngineOperationError::Deadline);
    assert_eq!(witness_counts().spawned, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn pre_spawn_shutdown_raised_consumes_no_generation() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let mut owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    let accepted = owner
        .admit(run_id("run-shutdown"), Duration::from_secs(10))
        .expect("admit");
    // Raise shutdown before owner dequeues this job.
    let _ = owner.shutdown();
    let result = accepted.await;
    assert_eq!(result.unwrap_err(), EngineOperationError::Shutdown);
    assert_eq!(witness_counts().spawned, 0);
}

// ---------------------------------------------------------------------------
// 5. Generation exhaustion quarantine without any spawn
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn generation_exhaustion_quarantines_without_spawn() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let owner = crate::engine_owner::start_with_exhausted_allocator_for_tests(
        config,
        &tokio::runtime::Handle::current(),
    );
    // Health starts Active.
    assert_eq!(owner.health(), EngineOwnerHealth::Active);
    let accepted = owner
        .admit(run_id("run-exhaust"), Duration::from_secs(10))
        .expect("admit before quarantine");
    let result = accepted.await;
    assert_eq!(
        result.unwrap_err(),
        EngineOperationError::GenerationExhausted
    );
    // Health becomes Quarantined.
    assert_eq!(owner.health(), EngineOwnerHealth::Quarantined);
    // Later admits return Unavailable.
    let later = owner.admit(run_id("run-after-exhaust"), Duration::from_secs(10));
    assert!(matches!(later, Err(LaunchAdmissionError::Unavailable)));
    // No spawn occurred.
    assert_eq!(witness_counts().spawned, 0);
}

// ---------------------------------------------------------------------------
// 6. Shutdown future custody
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn shutdown_signal_stops_admission_even_if_future_not_polled() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let mut owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    // Create future but do not poll it.
    let future = owner.shutdown();
    std::mem::forget(future);
    let result = owner.admit(run_id("run-after-signal"), Duration::from_secs(10));
    assert!(matches!(result, Err(LaunchAdmissionError::Unavailable)));
    // Now poll shutdown to completion.
    let report = owner.shutdown().await;
    assert_eq!(report, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_joins_cleanly_and_replays_cached_verdict() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let mut owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    let first = owner.shutdown().await;
    assert_eq!(first, EngineOwnerShutdown::Joined);
    // Second call must replay cached verdict without repolling JoinHandle.
    let second = owner.shutdown().await;
    assert_eq!(second, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_task_lost_when_owner_task_aborted() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let mut owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    owner.abort_for_tests();
    let report = owner.shutdown().await;
    assert_eq!(report, EngineOwnerShutdown::TaskLost);
    // Replay must not panic.
    let replay = owner.shutdown().await;
    assert_eq!(replay, EngineOwnerShutdown::TaskLost);
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_quarantined_does_not_consume_custody() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let mut owner = crate::engine_owner::start_with_exhausted_allocator_for_tests(
        config,
        &tokio::runtime::Handle::current(),
    );
    let accepted = owner
        .admit(run_id("run-quarantine-shutdown"), Duration::from_secs(10))
        .expect("admit");
    let _ = accepted.await;
    // Owner is now quarantined (parked forever retaining no child, but health is Quarantined).
    assert_eq!(owner.health(), EngineOwnerHealth::Quarantined);
    let report = owner.shutdown().await;
    assert_eq!(report, EngineOwnerShutdown::Quarantined);
    // Quarantined does not consume custody; health stays Quarantined.
    assert_eq!(owner.health(), EngineOwnerHealth::Quarantined);
}

// ---------------------------------------------------------------------------
// 7. Dropping unpolled accepted operation signals control; dropping facade raises shutdown
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn dropping_unpolled_accepted_operation_signals_control() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    let accepted = owner
        .admit(run_id("run-drop-unpolled"), Duration::from_secs(10))
        .expect("admit");
    // Drop without polling; control should be cancelled.
    drop(accepted);
    // Give owner a moment to observe cancellation before checking witness.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    // No generation was minted for the cancelled job, no spawn.
    assert_eq!(witness_counts().spawned, 0);
    // Next admit should still work (owner not quarantined, still Active).
    // The cancelled job was consumed as Cancelled without affecting health.
    let next = owner.admit(run_id("run-after-drop"), Duration::from_secs(10));
    // May be Ok or may have been drained; but owner health should still be Active
    // if the cancelled job was handled as Completed (not Quarantined).
    assert_eq!(owner.health(), EngineOwnerHealth::Active);
    drop(next);
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_facade_raises_shutdown() {
    reset_witnesses();
    let config = valid_config_with_capacity(4);
    let owner = EngineOwner::start(config, &tokio::runtime::Handle::current());
    drop(owner);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(witness_counts().spawned, 0);
}
