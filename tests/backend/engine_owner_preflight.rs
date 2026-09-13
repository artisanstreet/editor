//! Fixture coverage for the owner-serialized configured-engine preflight.
//!
//! These tests exercise the existing configured fixture protocol only through
//! the new queue branch. The preflight has no settings/database input and must
//! stop after the single authenticated health request.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use artisan_transport::CancelHandle;
use tokio::time::Instant;

use super::{
    EngineBounds, EngineOwner, EngineOwnerHealth, EngineOwnerShutdown, FixtureConfiguredLaunch,
    FixturePreflightInput, PreflightDeadlines, reset_witnesses, witness_counts,
};
use crate::engine_owner::http::{HealthError, HealthSecret};
use crate::engine_owner::operation::{EngineOperationError, PreflightReap};
use crate::engine_owner::process::{
    ChildParts, CleanupObservation, LaunchRecipe, LifelineWriter, StderrCounter,
    cleanup_after_abort, eventual_wait_once, spawn_configured_fixture_engine, spawn_engine,
};
use artisan_domain::RootPath;

fn fixture_program_path() -> PathBuf {
    let path = std::env::var_os("ARTISAN_ENGINE_OWNER_FIXTURE").map_or_else(
        || {
            let test = std::env::current_exe().expect("test executable path");
            test.parent()
                .expect("test output directory")
                .parent()
                .expect("Cargo profile directory")
                .join("examples")
                .join(format!(
                    "engine-owner-fixture{}",
                    std::env::consts::EXE_SUFFIX
                ))
        },
        std::path::PathBuf::from,
    );
    assert!(
        path.is_absolute() && path.is_file(),
        "build fixture with cargo build -p artisan-backend --examples or set ARTISAN_ENGINE_OWNER_FIXTURE to an absolute file: {}",
        path.display()
    );
    path
}

fn assert_fixture_program_is_regular_file(fixture: &Path) {
    assert!(fixture.is_file(), "fixture must be a regular file");
    #[cfg(unix)]
    {
        let metadata = std::fs::symlink_metadata(fixture).expect("fixture metadata");
        assert!(!metadata.is_symlink(), "fixture must not be a symlink");
        assert!(metadata.is_file(), "fixture must be a regular file");
    }
}

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "artisan-engine-preflight-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("preflight root should be created");
        Self { path }
    }

    fn root_path(&self) -> RootPath {
        RootPath::parse(self.path.to_str().expect("preflight root should be utf8"))
            .expect("preflight root should be valid")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn preflight_bounds() -> EngineBounds {
    EngineBounds {
        max_json_body: 1_024,
        max_sse_line: 1_024,
        max_sse_event: 1_024,
        max_readiness_line: 256,
        max_headers: 32,
        max_buf_bytes: 8_192,
        stderr_cap_bytes: 4_096,
        sink_capacity: 4,
        control_capacity: 1,
    }
}

fn deadlines_with_close(close: Duration) -> PreflightDeadlines {
    let now = Instant::now();
    PreflightDeadlines {
        readiness: now + Duration::from_secs(3),
        health: now + Duration::from_secs(3),
        close: now + close,
        admission: now + Duration::from_secs(8),
    }
}

fn fixture_input(
    root: &RootPath,
    fixture: PathBuf,
    scenario: &'static str,
    deadlines: PreflightDeadlines,
) -> FixturePreflightInput {
    FixturePreflightInput {
        project_root: root.clone(),
        fixture: FixtureConfiguredLaunch {
            program: fixture,
            version: "0.0.0-fixture",
            profile_id: "fixture-preflight".to_owned(),
            scenario,
        },
        deadlines,
        bounds: preflight_bounds(),
    }
}

fn configured_owner() -> EngineOwner {
    EngineOwner::start_configured(
        NonZeroUsize::new(1).expect("one queue slot is nonzero"),
        &tokio::runtime::Handle::current(),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn successful_preflight_health_reaps_without_session_creation() {
    reset_witnesses();
    let fixture = fixture_program_path();
    assert_fixture_program_is_regular_file(&fixture);
    let temp_root = TempRoot::new("success");
    let root = temp_root.root_path();
    let input = fixture_input(
        &root,
        fixture.clone(),
        "ready_ok",
        deadlines_with_close(Duration::from_secs(2)),
    );
    let input_debug = format!("{input:?}");
    assert!(input_debug.contains("<redacted>"));
    assert!(!input_debug.contains("fixture-preflight"));
    assert!(!input_debug.contains(fixture.to_string_lossy().as_ref()));
    assert!(!input_debug.contains(root.as_str()));

    let mut owner = configured_owner();
    let accepted = owner
        .admit_fixture_preflight(input)
        .expect("preflight admission should succeed");
    let receipt = tokio::time::timeout(Duration::from_secs(5), accepted)
        .await
        .expect("preflight should settle")
        .expect("ready and health should succeed");

    assert_eq!(receipt.profile_id(), "fixture-preflight");
    assert_eq!(receipt.version(), "0.0.0-fixture");
    // Windows kill-first contract in process.rs request_group_termination_before_abort_wait:
    // configured spawns terminate the group before the abort wait, so the reap
    // is AfterKill on Windows and WithoutKill elsewhere (wait-first ordering).
    if cfg!(windows) {
        assert_eq!(receipt.reap(), PreflightReap::AfterKill);
    } else {
        assert_eq!(receipt.reap(), PreflightReap::WithoutKill);
    }
    let receipt_debug = format!("{receipt:?}");
    assert!(receipt_debug.contains("<redacted>"));
    assert!(!receipt_debug.contains("fixture-preflight"));
    assert!(!receipt_debug.contains("0.0.0-fixture"));

    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    // Windows kill-first contract in process.rs request_group_termination_before_abort_wait:
    // the pre-close group termination counts one requested kill on Windows.
    if cfg!(windows) {
        assert_eq!(counts.kills_requested, 1);
    } else {
        assert_eq!(counts.kills_requested, 0);
    }
    assert_eq!(owner.health(), EngineOwnerHealth::Active);
    assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn readiness_failure_is_bounded_and_reaped() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let root = TempRoot::new("readiness-failure").root_path();
    let mut owner = configured_owner();
    let accepted = owner
        .admit_fixture_preflight(fixture_input(
            &root,
            fixture,
            "ready_malformed",
            deadlines_with_close(Duration::from_secs(2)),
        ))
        .expect("preflight admission should succeed");

    let result = tokio::time::timeout(Duration::from_secs(5), accepted)
        .await
        .expect("readiness failure should settle");
    let error = result.expect_err("readiness failure should be typed");
    assert_eq!(
        error,
        EngineOperationError::ReadinessFailed(
            crate::engine_owner::readiness::ReadinessError::InvalidJson
        )
    );
    let error_debug = format!("{error:?}");
    assert!(!error_debug.contains("ready_malformed"));
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    // Windows kill-first contract in process.rs request_group_termination_before_abort_wait:
    // the pre-close group termination counts one requested kill on Windows.
    if cfg!(windows) {
        assert_eq!(counts.kills_requested, 1);
    } else {
        assert_eq!(counts.kills_requested, 0);
    }
    assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn incompatible_health_version_is_rejected_without_session_work() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let root = TempRoot::new("version-failure").root_path();
    let mut owner = configured_owner();
    let accepted = owner
        .admit_fixture_preflight(fixture_input(
            &root,
            fixture,
            "health_version_reject",
            deadlines_with_close(Duration::from_secs(2)),
        ))
        .expect("preflight admission should succeed");

    let result = tokio::time::timeout(Duration::from_secs(5), accepted)
        .await
        .expect("version failure should settle");
    assert_eq!(
        result.unwrap_err(),
        EngineOperationError::IncompatibleVersion
    );
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    // Windows kill-first contract in process.rs request_group_termination_before_abort_wait:
    // the pre-close group termination counts one requested kill on Windows.
    if cfg!(windows) {
        assert_eq!(counts.kills_requested, 1);
    } else {
        assert_eq!(counts.kills_requested, 0);
    }
    assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
}

async fn direct_fixture_health_with_wrong_secret(fixture: &Path) -> HealthError {
    let spawn_secret = HealthSecret::generate().expect("fixture spawn secret");
    let wrong_secret = HealthSecret::generate().expect("health secret");
    let mut child = spawn_configured_fixture_engine(fixture, "ready_ok", spawn_secret.as_str())
        .expect("fixture should spawn");
    let mut stdout = child.stdout.take().expect("fixture stdout");
    let lifeline = LifelineWriter::take(&mut child);
    let stderr_counter = StderrCounter::new(child.stderr.take(), 4_096);
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let endpoint = crate::engine_owner::readiness::read_readiness(
        &mut stdout,
        256,
        Instant::now() + Duration::from_secs(3),
        &shutdown,
        &cancel,
    )
    .await
    .expect("fixture readiness should validate");
    let result = crate::engine_owner::http::perform_health(
        &endpoint,
        &wrong_secret,
        &preflight_bounds(),
        Instant::now() + Duration::from_secs(3),
        &cancel,
        &shutdown,
        Some(crate::engine_owner::http::FIXTURE_EXPECTED_VERSION),
    )
    .await;
    drop(stdout);

    match cleanup_after_abort(
        ChildParts {
            child,
            lifeline,
            stdout: None,
            stderr_counter,
        },
        Duration::from_secs(2),
    )
    .await
    {
        CleanupObservation::ReapedWithoutKill(_) | CleanupObservation::ReapedAfterKill(_) => {}
        CleanupObservation::Retained(retained) => {
            assert!(eventual_wait_once(retained).await.is_ok());
        }
    }
    result.expect_err("wrong fixture authorization must fail")
}

#[tokio::test(flavor = "current_thread")]
async fn health_auth_failure_is_typed_and_redacted() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let error = direct_fixture_health_with_wrong_secret(&fixture).await;
    assert_eq!(error, HealthError::StatusNotSuccess);
    let debug = format!("{error:?}");
    assert!(!debug.contains("Basic"));
    assert!(!debug.contains("fixture"));
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    // Windows kill-first contract in process.rs request_group_termination_before_abort_wait:
    // the pre-close group termination counts one requested kill on Windows.
    if cfg!(windows) {
        assert_eq!(counts.kills_requested, 1);
    } else {
        assert_eq!(counts.kills_requested, 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_before_dequeue_precedes_deadline_and_does_not_spawn() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let root = TempRoot::new("cancel").root_path();
    let mut deadlines = deadlines_with_close(Duration::from_secs(2));
    deadlines.admission = Instant::now() - Duration::from_secs(1);
    let mut owner = configured_owner();
    let accepted = owner
        .admit_fixture_preflight(fixture_input(&root, fixture, "ready_ok", deadlines))
        .expect("preflight admission should succeed");
    accepted.cancel();

    assert_eq!(accepted.await.unwrap_err(), EngineOperationError::Cancelled);
    assert_eq!(witness_counts().spawned, 0);
    assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn expired_admission_deadline_is_rejected_before_spawn() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let root = TempRoot::new("deadline").root_path();
    let mut deadlines = deadlines_with_close(Duration::from_secs(2));
    deadlines.admission = Instant::now() - Duration::from_secs(1);
    let mut owner = configured_owner();
    let accepted = owner
        .admit_fixture_preflight(fixture_input(&root, fixture, "ready_ok", deadlines))
        .expect("preflight admission should succeed");

    assert_eq!(accepted.await.unwrap_err(), EngineOperationError::Deadline);
    assert_eq!(witness_counts().spawned, 0);
    assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn owner_shutdown_precedes_cancelled_expired_preflight() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let root = TempRoot::new("shutdown-precedence").root_path();
    let mut deadlines = deadlines_with_close(Duration::from_secs(2));
    deadlines.admission = Instant::now() - Duration::from_secs(1);
    let mut owner = configured_owner();
    let accepted = owner
        .admit_fixture_preflight(fixture_input(&root, fixture, "ready_ok", deadlines))
        .expect("preflight admission should succeed");
    accepted.cancel();
    drop(owner.shutdown());

    assert_eq!(accepted.await.unwrap_err(), EngineOperationError::Shutdown);
    assert_eq!(witness_counts().spawned, 0);
    assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn abrupt_child_exit_is_reported_after_observed_reap() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let root = TempRoot::new("child-exit").root_path();
    let mut owner = configured_owner();
    let accepted = owner
        .admit_fixture_preflight(fixture_input(
            &root,
            fixture,
            "abrupt_child_exit_nonzero",
            deadlines_with_close(Duration::from_secs(2)),
        ))
        .expect("preflight admission should succeed");

    let result = tokio::time::timeout(Duration::from_secs(5), accepted)
        .await
        .expect("child exit should settle");
    let error = result.expect_err("child exit should be typed");
    assert_eq!(
        error,
        EngineOperationError::ReadinessFailed(
            crate::engine_owner::readiness::ReadinessError::EofBeforeNewline
        )
    );
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn close_deadline_uses_existing_kill_and_custody_sequence() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let root = TempRoot::new("close-kill").root_path();
    let mut deadlines = deadlines_with_close(Duration::from_secs(2));
    deadlines.readiness = Instant::now() - Duration::from_secs(1);
    deadlines.close = Instant::now() - Duration::from_secs(1);
    let mut owner = configured_owner();
    let accepted = owner
        .admit_fixture_preflight(fixture_input(
            &root,
            fixture,
            "hang_until_lifeline",
            deadlines,
        ))
        .expect("preflight admission should succeed");

    let result = tokio::time::timeout(Duration::from_secs(5), accepted)
        .await
        .expect("close sequence should settle the response");
    assert!(matches!(
        result,
        Err(EngineOperationError::Deadline | EngineOperationError::UnresolvedReapDuring { .. })
    ));
    let first_shutdown = owner.shutdown().await;
    if first_shutdown == EngineOwnerShutdown::Quarantined {
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    } else {
        assert_eq!(first_shutdown, EngineOwnerShutdown::Joined);
    }
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    assert!(counts.kills_requested <= 1);
}

/// Legacy-lane spawn (no pre-wait group termination) so the cleanup's own
/// kill/observation sequence is what is exercised on every platform.
fn ungrouped_lifeline_fixture() -> crate::engine_owner::process::EngineChild {
    let fixture = fixture_program_path();
    let secret = HealthSecret::generate().expect("fixture secret");
    spawn_engine(
        &LaunchRecipe::Fixture {
            program: fixture,
            args: Vec::new(),
            scenario: "hang_until_lifeline",
        },
        secret.as_str(),
    )
    .expect("fixture should spawn")
}

#[tokio::test(flavor = "current_thread")]
async fn zero_budget_cleanup_kills_then_reaps_within_the_defined_grace() {
    reset_witnesses();
    let mut child = ungrouped_lifeline_fixture();
    let lifeline = LifelineWriter::take(&mut child);
    let parts = ChildParts {
        child,
        lifeline,
        stdout: None,
        stderr_counter: StderrCounter::new(None, 4_096),
    };
    let settled = tokio::time::timeout(
        Duration::from_secs(5),
        cleanup_after_abort(parts, Duration::ZERO),
    )
    .await
    .expect("zero-budget cleanup must settle bounded");
    assert!(
        matches!(settled, CleanupObservation::ReapedAfterKill(_)),
        "a live child must be killed and then observed inside the post-kill grace"
    );
    let counts = witness_counts();
    assert_eq!(counts.kills_requested, 1);
    assert_eq!(counts.reaps_observed, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn lifeline_close_is_the_single_eof_for_an_ungrouped_child() {
    reset_witnesses();
    let mut child = ungrouped_lifeline_fixture();
    // The sole lifeline writer is established before any failure cleanup, so
    // it must own the child's only stdin handle: closing it must be the
    // child's EOF. The fixture exits with its lifeline-lost code (3) when it
    // observes EOF, and the ungrouped launch keeps that exit observable on
    // every platform instead of masking it with a pre-wait group kill.
    let lifeline = LifelineWriter::take(&mut child);
    assert!(lifeline.is_open(), "the lifeline must own the child stdin");
    let parts = ChildParts {
        child,
        lifeline,
        stdout: None,
        stderr_counter: StderrCounter::new(None, 4_096),
    };
    let settled = tokio::time::timeout(
        Duration::from_secs(5),
        cleanup_after_abort(parts, Duration::from_secs(2)),
    )
    .await
    .expect("lifeline close must settle bounded");
    let CleanupObservation::ReapedWithoutKill(status) = settled else {
        panic!("lifeline close must reap the fixture without a kill");
    };
    assert_eq!(status.code(), Some(3), "fixture lifeline-lost exit code");
    let counts = witness_counts();
    assert_eq!(counts.kills_requested, 0);
    assert_eq!(counts.reaps_observed, 1);
}
