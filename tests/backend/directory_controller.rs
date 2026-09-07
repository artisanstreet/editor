//! Private headless behavior tests for the directory controller, driving
//! the TEST-ONLY protocol fixture executable.
//!
//! Everything here compiles as the private `cfg(test)` module
//! `directory_controller::directory_controller_tests` of the real backend
//! crate, so parent-side assertions reach the actual public controller API
//! and the private witnesses on the true owner/spawn/pipe/reap path.
//!
//! The child role is the declared TEST-ONLY
//! `//tests/backend:directory_controller_fixture` executable (one ordinary
//! `main`, `testonly = True`, never shipped): it is resolved exclusively
//! through the pinned official Bazel runfiles library from the exact
//! rlocationpath exported by `backend_unit_test.env`. No sibling, PATH,
//! source-tree fallback, homemade manifest parser, or guessed output
//! directory exists, the parent environment is never mutated, no chooser
//! opens, and an unknown or non-Unicode child scenario fails nonzero.
//!
//! Every scenario holds one process-local serialization lock so global
//! witness counts stay attributable, establishes causal readiness through
//! the fixture's stderr readiness byte before cancelling or replacing
//! work, verifies observed reaps WITH their actual exit codes (a watchdog
//! exit is always failure, never cancellation/reap proof), and keeps the
//! runtime plus controller OUTSIDE `catch_unwind`: each scenario runs in a
//! guarded borrow, shutdown is explicitly awaited while the runtime lives
//! on BOTH normal and caught-panic paths, and only afterwards is a caught
//! panic resumed.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use artisan_database::SqliteConfig;
use artisan_domain::ROOT_PATH_MAX_BYTES;
use artisan_domain::{AttachProject, Command, DirectoryId, ReceiptDisposition, RequestId};
use artisan_protocol::{
    ClientRequest, DirectoryPickOutcome as ProtocolDirectoryPickOutcome, ErrorCode, ResponsePayload,
};
use artisan_transport::CancelHandle;
use runfiles::{Runfiles, rlocation};

use super::process::WitnessCounts;
use super::{
    AdmissionError, ControllerStartError, DirectoryController, DirectoryControllerConfig,
    DirectoryPickOutcome, GenerationAllocator, HealthState, HelperOperationError, Job,
    LaunchRecipe, QUEUE_CAPACITY, ShutdownReport, reset_witnesses, witness_counts,
};
use crate::directory_helper_codec::{
    HEADER_LEN, PROTOCOL_VERSION, REQUEST_MAGIC, RESPONSE_MAGIC, RequestEncodeFault, RequestKind,
    Response, ResponseEncodeFault, ResponseHeaderFault, encode_request, encode_response,
    parse_request_header, parse_response_header,
};
use crate::{ForgeStorage, RequestHandler};

/// Serializes child-owning scenarios so the process-global private
/// witnesses stay attributable to the single tested controller.
static CHILD_SCENARIO_LOCK: Mutex<()> = Mutex::new(());

/// Acquires the child-scenario serialization lock, recovering the guard
/// even from poisoning (a panicked predecessor leaves no state behind).
fn serialize_child_scenarios() -> MutexGuard<'static, ()> {
    CHILD_SCENARIO_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Environment name carrying the fixture's exact rlocationpath; declared by
/// `backend_unit_test.env` and resolved ONLY through the pinned official
/// runfiles library below.
const FIXTURE_MAPPING_ENV: &str = "ARTISAN_DIRECTORY_CONTROLLER_FIXTURE";

/// Generous default budget for operations expected to succeed.
const SUCCESS_BUDGET: Duration = Duration::from_secs(30);

/// Finite budget proving deadline precedence against a hanging child:
/// generous enough for real spawn plus witnessed readiness, yet far below
/// the causal bounds, so expiry — not startup racing — ends the operation.
const DEADLINE_BUDGET: Duration = Duration::from_secs(2);

/// Outer causal bound for awaited outcomes; a lapse means the controller or
/// fixture misbehaved and the assertion fails instead of hanging forever.
const AWAIT_BOUND: Duration = Duration::from_secs(60);

/// Witness-polling cadence; determinism comes from the witnessed facts, not
/// from these delays.
const WITNESS_POLL: Duration = Duration::from_millis(10);

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A uniquely named temporary directory removed again on drop.
///
/// Owned by the PARENT test for the whole life of any child that receives
/// its text over the wire, so the child's own `process::exit` can never
/// skip this destructor and leak resources.
struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "artisan-dir-controller {label} héllo 🦈 {}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary directory should be created");
        Self { path }
    }

    /// Exact text of the temporary path; never trimmed or rewritten.
    fn text(&self) -> String {
        self.path
            .to_str()
            .expect("temporary path stays unicode")
            .to_owned()
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Parent-side harness helpers driving the REAL controller
// ---------------------------------------------------------------------------

/// Resolves the ONE declared fixture artifact through the pinned official
/// runfiles library. Missing environment mapping or runfile resolution is a
/// hard test failure; there is deliberately no other discovery path.
fn resolved_fixture_program() -> PathBuf {
    let mapping = std::env::var(FIXTURE_MAPPING_ENV)
        .expect("backend_unit_test must export the fixture rlocationpath");
    let runfiles = Runfiles::create().expect("official runfiles discovery should succeed");
    let resolved = rlocation!(runfiles, mapping.as_str());
    resolved.unwrap_or_else(|| panic!("declared fixture artifact must resolve: {mapping}"))
}

/// Builds the caller-owned runtime; the controller never creates one.
fn build_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build")
}

/// Starts a real controller whose children are the TEST-ONLY fixture
/// executable playing the named deterministic scenario.
fn started_fixture_controller(
    runtime: &tokio::runtime::Runtime,
    scenario: &'static str,
) -> DirectoryController {
    reset_witnesses();
    let recipe = LaunchRecipe::Fixture {
        program: resolved_fixture_program(),
        args: Vec::new(),
        scenario,
    };
    DirectoryController::start_with_recipe(recipe, runtime.handle())
}

/// Polls the phase witnesses until the causal fact holds or the bounded
/// witness budget lapses (a lapse fails the test; it never spins forever).
async fn wait_for_witness(check: impl Fn(WitnessCounts) -> bool, label: &'static str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if check(witness_counts()) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "witness '{label}' was never established"
        );
        tokio::time::sleep(WITNESS_POLL).await;
    }
}

/// Awaits one operation under the outer causal bound.
async fn settle_within(operation: super::PickOperation) -> OperationOutcome {
    tokio::time::timeout(AWAIT_BOUND, operation)
        .await
        .expect("operation exceeded the outer causal bound")
}

type OperationOutcome = Result<DirectoryPickOutcome, HelperOperationError>;

/// Shuts down while the runtime is alive and requires an observed join.
async fn joined_shutdown(controller: &mut DirectoryController) {
    let report = tokio::time::timeout(AWAIT_BOUND, controller.shutdown()).await;
    let report = report.expect("shutdown exceeded the outer causal bound");
    assert_eq!(report, ShutdownReport::Joined, "expected an observed join");
}

/// The frozen lifetime discipline, applied identically to EVERY child-owning
/// scenario: runtime and controller stay OUTSIDE `catch_unwind` (each test
/// owns them as locals), the guarded closure only BORROWS them, shutdown is
/// explicitly awaited while the runtime still lives on BOTH normal and
/// caught-panic paths, and only afterwards is a caught panic resumed so
/// genuine assertion failures still fail the test.
///
/// Written inline per scenario (no generic helper): the shutdown call after
/// the unwind scope replays the cached join verdict when the scenario
/// already shut down cleanly, and performs the first real shutdown when the
/// scenario panicked early.
macro_rules! finish_scoped_scenario {
    ($caught:expr, $runtime:expr, $controller:expr) => {{
        // Explicit cleanup while the runtime is alive, whatever happened
        // above; a repeat call after Joined replays the cached observation.
        // Incomplete cleanup is SURFACED, never discarded: a quarantined or
        // lost owner task fails the scenario loudly while the runtime,
        // controller, and any retained resources stay alive for audit.
        let report = $runtime.block_on($controller.shutdown());
        if report != ShutdownReport::Joined {
            panic!("scoped scenario cleanup did not complete cleanly: {report:?}");
        }
        // Watchdog containment is ALWAYS failure: no observed reap in this
        // scenario may have ended with the fixture's watchdog exit.
        let counts = witness_counts();
        assert_eq!(
            counts.watchdog_failures_seen, 0,
            "a fixture watchdog exit was witnessed; containment endings are failure evidence"
        );
        match $caught {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }};
}

// ---------------------------------------------------------------------------
// Success paths, path preservation, and every typed outcome tag
// ---------------------------------------------------------------------------

#[test]
fn pick_success_roundtrips_the_real_canonical_path() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "pick_success");

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("pick should be admitted");
            let outcome = settle_within(operation).await;
            let DirectoryPickOutcome::Selected { canonical_path } =
                outcome.expect("successful pick exchange should settle Ok")
            else {
                panic!("expected Selected");
            };

            assert!(PathBuf::from(&canonical_path).is_absolute());
            // The published result is a local handoff only: no UI consumption
            // or selection-authority registration is claimed anywhere above.
            joined_shutdown(&mut controller).await;
            let counts = witness_counts();
            assert!(counts.spawned >= 1);
            assert!(counts.reaps_observed >= 1);
            assert_eq!(counts.kills_requested, 0, "healthy success needs no kill");
            assert_eq!(
                counts.last_exit_code, 0,
                "the observed reap must be the helper's real exit-0"
            );
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn request_handler_composes_picker_attach_replay_and_cancellation() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let storage = runtime.block_on(async {
        ForgeStorage::open(SqliteConfig::in_memory().sqlx_logging(false))
            .await
            .expect("composition storage should open and migrate")
    });
    let handler = composition_handler(&runtime, &storage, "pick_success");

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_successful_project_intake(&runtime, &handler);
    }));
    let storage = finish_composition_handler(&runtime, handler, storage, caught);

    let cancelled_handler = composition_handler(&runtime, &storage, "cancelled");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_cancelled_project_intake(&runtime, &cancelled_handler);
    }));
    let storage = finish_composition_handler(&runtime, cancelled_handler, storage, caught);
    runtime
        .block_on(storage.close())
        .expect("composition storage should close");
}

fn composition_handler(
    runtime: &tokio::runtime::Runtime,
    storage: &ForgeStorage,
    scenario: &'static str,
) -> RequestHandler {
    RequestHandler::with_directory_picker(
        storage.repository().clone(),
        started_fixture_controller(runtime, scenario),
        SUCCESS_BUDGET,
    )
}

fn assert_successful_project_intake(runtime: &tokio::runtime::Runtime, handler: &RequestHandler) {
    runtime.block_on(async {
        let pick_request_id =
            RequestId::parse("frame-composition-pick").expect("pick frame id should be valid");
        let (picked, receipt) = handler
            .respond_with_receipt(&pick_request_id, &ClientRequest::PickDirectory)
            .await
            .into_parts();
        assert!(receipt.is_no_work());
        let picked = picked.expect("the fixture pick should succeed");
        let directory_id = match picked.payload {
            ResponsePayload::DirectoryPicked(ProtocolDirectoryPickOutcome::Selected(id)) => id,
            _other => panic!("expected an opaque selected directory id"),
        };
        assert!(!directory_id.as_str().contains('/'));
        assert!(!directory_id.as_str().contains('\\'));

        let attach_request_id =
            RequestId::parse("request-composition-attach").expect("attach id should be valid");
        let attach = ClientRequest::Command(Command::AttachProject(AttachProject {
            request_id: attach_request_id.clone(),
            directory_id: directory_id.clone(),
        }));
        let (attached, receipt) = handler
            .respond_with_receipt(&attach_request_id, &attach)
            .await
            .into_parts();
        assert!(receipt.is_no_work());
        let attached = attached.expect("first attach should succeed");
        let (project, disposition) = match attached.payload {
            ResponsePayload::AttachedProject {
                project,
                disposition,
            } => (project, disposition),
            _other => panic!("expected an attached project"),
        };
        assert_eq!(disposition, ReceiptDisposition::Accepted);
        assert!(PathBuf::from(project.root_path.as_str()).is_absolute());

        let (replayed, receipt) = handler
            .respond_with_receipt(&attach_request_id, &attach)
            .await
            .into_parts();
        assert!(receipt.is_no_work());
        let replayed = replayed.expect("the same attach should replay");
        let ResponsePayload::AttachedProject { disposition, .. } = replayed.payload else {
            panic!("expected an attached-project replay");
        };
        assert_eq!(disposition, ReceiptDisposition::Duplicate);

        let other_request_id =
            RequestId::parse("request-composition-other").expect("other id should be valid");
        let other_attach = ClientRequest::Command(Command::AttachProject(AttachProject {
            request_id: other_request_id.clone(),
            directory_id,
        }));
        let (unknown, receipt) = handler
            .respond_with_receipt(&other_request_id, &other_attach)
            .await
            .into_parts();
        assert!(receipt.is_no_work());
        let unknown = unknown.expect_err("a consumed id must be unknown to another request");
        assert_eq!(unknown.code, ErrorCode::DirectoryUnknown);
        assert!(!unknown.retryable);
    });
}

fn assert_cancelled_project_intake(runtime: &tokio::runtime::Runtime, handler: &RequestHandler) {
    runtime.block_on(async {
        let pick_request_id =
            RequestId::parse("frame-composition-cancel").expect("cancel frame id should be valid");
        let (cancelled, receipt) = handler
            .respond_with_receipt(&pick_request_id, &ClientRequest::PickDirectory)
            .await
            .into_parts();
        assert!(receipt.is_no_work());
        assert_eq!(
            cancelled
                .expect("cancelled picker should return a protocol response")
                .payload,
            ResponsePayload::DirectoryPicked(ProtocolDirectoryPickOutcome::Cancelled)
        );

        let attach_request_id =
            RequestId::parse("request-composition-cancelled").expect("attach id should be valid");
        let attach = ClientRequest::Command(Command::AttachProject(AttachProject {
            request_id: attach_request_id.clone(),
            directory_id: DirectoryId::parse("cancelled-directory")
                .expect("directory id should be valid"),
        }));
        let (unknown, receipt) = handler
            .respond_with_receipt(&attach_request_id, &attach)
            .await
            .into_parts();
        assert!(receipt.is_no_work());
        let unknown = unknown.expect_err("a cancelled pick must register nothing");
        assert_eq!(unknown.code, ErrorCode::DirectoryUnknown);
        assert!(!unknown.retryable);
    });
}

fn finish_composition_handler(
    runtime: &tokio::runtime::Runtime,
    mut handler: RequestHandler,
    storage: ForgeStorage,
    caught: std::thread::Result<()>,
) -> ForgeStorage {
    let report = runtime.block_on(handler.shutdown_directory_controller());
    drop(handler);
    if let Err(payload) = caught {
        runtime
            .block_on(storage.close())
            .expect("composition storage should close after a panic");
        std::panic::resume_unwind(payload);
    }
    assert_eq!(report, Some(ShutdownReport::Joined));
    storage
}

#[test]
fn validate_success_preserves_exact_path_bytes_end_to_end() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "validate_success");

    // PARENT-OWNED anchor: alive from before spawn until after the child's
    // observed reap, so the child itself creates and leaks nothing.
    let anchor = TemporaryDirectory::new("validate-anchor");
    let candidate = anchor.text();

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .validate_directory(SUCCESS_BUDGET, &candidate)
                .expect("validate should be admitted");
            let outcome = settle_within(operation).await;
            let DirectoryPickOutcome::Selected { canonical_path } =
                outcome.expect("successful validate exchange should settle Ok")
            else {
                panic!("expected Selected");
            };
            let independent = fs::canonicalize(&anchor.path)
                .expect("independent canonicalization should succeed");
            assert_eq!(PathBuf::from(canonical_path), independent);

            joined_shutdown(&mut controller).await;
            assert!(witness_counts().reaps_observed >= 1);
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn every_empty_payload_outcome_tag_maps_onto_its_public_variant() {
    let cases: [(&'static str, DirectoryPickOutcome); 4] = [
        ("cancelled", DirectoryPickOutcome::Cancelled),
        ("invalid_path", DirectoryPickOutcome::InvalidPath),
        (
            "unsupported_platform",
            DirectoryPickOutcome::UnsupportedPlatform,
        ),
        ("dialog_failed", DirectoryPickOutcome::DialogFailed),
    ];
    for (scenario, expected) in cases {
        let _child_scenario_guard = serialize_child_scenarios();
        let runtime = build_runtime();
        let mut controller = started_fixture_controller(&runtime, scenario);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runtime.block_on(async {
                let operation = controller
                    .pick_directory(SUCCESS_BUDGET)
                    .expect("typed-outcome pick should be admitted");
                let outcome = settle_within(operation).await;
                assert_eq!(outcome.as_ref(), Ok(&expected), "scenario {scenario}");
                joined_shutdown(&mut controller).await;
                assert!(
                    witness_counts().reaps_observed >= 1,
                    "scenario {scenario} must have observed a reap"
                );
            });
        }));
        finish_scoped_scenario!(caught, runtime, controller);
    }

    // UnsupportedEncoding shares the empty-payload shape; cover it too.
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "unsupported_encoding");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("encoding-tag pick should be admitted");
            let outcome = settle_within(operation).await;
            assert_eq!(outcome, Ok(DirectoryPickOutcome::UnsupportedEncoding));
            joined_shutdown(&mut controller).await;
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

// ---------------------------------------------------------------------------
// Malformed, truncated, trailing, stale, oversized, capped, nonzero exits
// ---------------------------------------------------------------------------

#[test]
fn malformed_magic_yields_the_typed_frame_fault() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "malformed_magic");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("malformed probe should be admitted");
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::MalformedFrame)
            );
            joined_shutdown(&mut controller).await;
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn truncated_response_is_never_a_partial_success() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "truncated");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("truncated probe should be admitted");
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::TruncatedFrame)
            );
            joined_shutdown(&mut controller).await;
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn trailing_bytes_after_a_complete_frame_are_rejected() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "trailing");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("trailing probe should be admitted");
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::TrailingOutput)
            );
            joined_shutdown(&mut controller).await;
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn stale_generations_never_settle_this_or_any_later_operation() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "stale_generation");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("stale probe should be admitted");
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::StaleGeneration)
            );

            // A later operation on the same controller must still work and
            // receive its OWN generation's answer (the fixture echoes every
            // generation except 1 exactly), proving no cross-settlement.
            let follow_up = {
                let operation = controller
                    .pick_directory(SUCCESS_BUDGET)
                    .expect("follow-up should be admitted");
                settle_within(operation).await
            };
            assert!(
                matches!(follow_up, Ok(DirectoryPickOutcome::Cancelled)),
                "the follow-up operation must settle independently"
            );
            joined_shutdown(&mut controller).await;
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn oversized_declared_payloads_are_refused_before_allocation() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "oversized_payload");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("oversized probe should be admitted");
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::OversizedOutput)
            );
            joined_shutdown(&mut controller).await;
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn stderr_crossing_the_count_only_cap_fails_the_operation() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "stderr_flood");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("flood probe should be admitted");
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::StderrCapExceeded)
            );
            joined_shutdown(&mut controller).await;
            // The primary failure stands regardless of the separately
            // observed cleanup status; cleanup itself still reaps the child.
            // The flood fixture exits 0 on its own before any abort path, so
            // the witnessed ending is exactly that — and never a watchdog.
            let counts = witness_counts();
            assert!(counts.reaps_observed >= 1);
            assert_eq!(
                counts.last_exit_code, 0,
                "stderr-flood ending must be the fixture's own exit-0, never a watchdog"
            );
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn nonzero_exits_fail_even_after_a_well_formed_exchange() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "exit_nonzero");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("exit probe should be admitted");
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::ExitFailure)
            );
            joined_shutdown(&mut controller).await;
            assert_eq!(
                witness_counts().last_exit_code,
                7,
                "the nonzero ending itself must be the witnessed exit"
            );
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

// ---------------------------------------------------------------------------
// Abandonment, explicit cancellation, deadlines, zero budgets, shutdown
// ---------------------------------------------------------------------------

#[test]
fn dropping_an_accepted_future_cleans_up_and_reaps() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "hang_until_lifeline");

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("hanging pick should be admitted");
            wait_for_witness(
                |counts| counts.stderr_bytes_seen >= 1,
                "fixture consumed its request (readiness byte counted)",
            )
            .await;
            // Dropping the unpolled future fires the installed-before-admission
            // abandonment signal; this is plumbing, never chooser-Cancelled.
            drop(operation);
            wait_for_witness(
                |counts| counts.reaps_observed >= 1,
                "abandonment reaped the child",
            )
            .await;
            assert_eq!(
                witness_counts().kills_requested,
                0,
                "lifeline close alone must end the watching fixture"
            );
            assert_eq!(
                witness_counts().last_exit_code,
                i32::from(crate::directory_helper::EXIT_CODE_LIFELINE_LOST),
                "abandonment must end via the lifeline watcher, never a watchdog"
            );
            joined_shutdown(&mut controller).await;
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn explicit_cancellation_resolves_cancelled_and_reaps() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "hang_until_lifeline");

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("cancellation probe should be admitted");
            wait_for_witness(
                |counts| counts.stderr_bytes_seen >= 1,
                "fixture consumed its request (readiness byte counted)",
            )
            .await;
            operation.cancel();
            let outcome = settle_within(operation).await;
            assert_eq!(outcome, Err(HelperOperationError::Cancelled));
            joined_shutdown(&mut controller).await;
            let counts = witness_counts();
            assert!(counts.reaps_observed >= 1);
            assert_eq!(counts.kills_requested, 0);
            assert_eq!(
                counts.last_exit_code,
                i32::from(crate::directory_helper::EXIT_CODE_LIFELINE_LOST),
                "explicit cancellation must end via the lifeline watcher"
            );
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

/// Safe `std::task::Wake` witness recording whether a waker actually fired.
struct PollWitness {
    woken: AtomicBool,
}

impl Wake for PollWitness {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::SeqCst);
    }
}

/// PURE injected private health/join probe for the shutdown future's wake
/// retention — explicitly allowed in this private cfg(test) child module.
///
/// It constructs the exact private [`DirectoryController`] fields (bounded
/// channels, signal, watch, and a oneshot-controlled owner-task stand-in on
/// the caller's runtime), then polls the ACTUAL `shutdown()` future:
///
/// 1. Pending while the controlled join parks, with NO spurious wake;
/// 2. an ACTUAL waker delivery through the retained registration when
///    quarantine is published while the join is still parked, followed by a
///    `Quarantined` verdict that consumes neither facade nor join;
/// 3. after the borrow ends and the controlled task completes,
///    completion-first observation of `Joined`, then repeated calls replaying
///    the cached verdict.
///
/// This is NOT real OS wait-failure or child-reap proof: no OS child is
/// involved, and production code is untouched by the injection.
#[test]
fn shutdown_future_pending_to_quarantined_then_cached_joined() {
    // This probe asserts process-global witness zeros, so it holds the
    // existing serialization lock for its whole body and starts from
    // freshly reset counters.
    let _child_scenario_guard = serialize_child_scenarios();
    reset_witnesses();
    let runtime = build_runtime();

    // Exact private facade construction: same channel/signal/watch/join
    // wiring production assembly uses, with a fully controlled owner task.
    let (jobs_tx, jobs_rx) = tokio::sync::mpsc::channel::<Job>(QUEUE_CAPACITY);
    let shutdown_signal = Arc::new(CancelHandle::new());
    // The facade's watch field stores the OWNER-side health enum; use that
    // exact type here so construction and later publication line up.
    let (health_tx, health_rx) = tokio::sync::watch::channel(super::operation::HealthState::Active);
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
    // Completion acknowledgement for the controlled owner task.
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    // The controlled owner stays parked until the test releases it; the
    // probe-side receiver half is dropped so admission reads as closed.
    let join = runtime.spawn(async move {
        drop(jobs_rx);
        let _parked = gate_rx.await;
        // FINAL synchronous operation of this controlled task, sent
        // immediately before returning: proof that the owner actually ran
        // to completion rather than the gate-send merely being accepted.
        let _acknowledged = done_tx.send(());
    });

    let mut controller = DirectoryController {
        jobs: jobs_tx,
        shutdown: Arc::clone(&shutdown_signal),
        health: health_rx,
        join,
        observed_join: None,
    };

    runtime.block_on(async {
        let witness = Arc::new(PollWitness {
            woken: AtomicBool::new(false),
        });
        let waker = Waker::from(Arc::clone(&witness));
        let mut context = Context::from_waker(&waker);

        let mut shutdown_future = Box::pin(controller.shutdown());

        // Phase A: Pending while the join parks; nothing wakes spuriously.
        assert!(
            matches!(shutdown_future.as_mut().poll(&mut context), Poll::Pending),
            "shutdown must stay Pending while the controlled join parks"
        );
        assert!(
            !witness.woken.load(Ordering::SeqCst),
            "no wake may fire before quarantine is published"
        );

        // Phase B: quarantine published while STILL parked. The retained
        // registration inside the future must deliver the actual wake and
        // then resolve to the incomplete Quarantined report without
        // consuming facade or join.
        health_tx
            .send(super::operation::HealthState::Quarantined)
            .expect("watch is open");
        assert!(
            witness.woken.load(Ordering::SeqCst),
            "the retained registration must deliver the quarantine wake across Pending"
        );
        match shutdown_future.as_mut().poll(&mut context) {
            Poll::Ready(ShutdownReport::Quarantined) => {}
            other => panic!("expected Quarantined while join stays parked, got {other:?}"),
        }
        drop(shutdown_future); // ends the exclusive controller borrow

        // Phase C: release the controlled owner task and require an ACTUAL
        // completion acknowledgement — observed with the existing causal bound
        // on this current-thread runtime — BEFORE asserting Joined. Gate-send
        // alone proves nothing: a timeout or a dropped sender (owner never ran
        // to its final operation) fails loudly right here. The JoinHandle is
        // never consumed outside the facade.
        let _released = gate_tx.send(());
        let acknowledged = tokio::time::timeout(AWAIT_BOUND, done_rx)
            .await
            .expect("controlled owner task never completed within the causal bound");
        acknowledged.expect(
            "the controlled owner task's completion sender was dropped without acknowledging",
        );
    });

    let first = runtime.block_on(controller.shutdown());
    assert_eq!(
        first,
        ShutdownReport::Joined,
        "eventual completion must be observable after Quarantined"
    );
    let replay = runtime.block_on(controller.shutdown());
    assert_eq!(
        replay,
        ShutdownReport::Joined,
        "repeated calls must replay the cached completion"
    );

    // No fixture/child was involved: only the controlled task ran.
    let counts = witness_counts();
    assert_eq!(counts.spawned, 0);
    assert_eq!(counts.reaps_observed, 0);
    assert_eq!(counts.watchdog_failures_seen, 0);
}

#[test]
fn deadline_expiry_beats_a_hanging_helper_and_reaps() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "hang_until_lifeline");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(DEADLINE_BUDGET)
                .expect("deadline probe should be admitted");
            // Causal readiness FIRST: the fixture's stderr readiness byte
            // proves the child consumed its request BEFORE settlement is
            // awaited; a missing witness fails loudly here instead of
            // claiming deadline/reap proof for a child that never started.
            wait_for_witness(
                |counts| counts.stderr_bytes_seen >= 1,
                "fixture consumed its request (readiness byte counted)",
            )
            .await;
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::Deadline)
            );
            joined_shutdown(&mut controller).await;
            let counts = witness_counts();
            assert!(counts.reaps_observed >= 1);
            assert_eq!(counts.kills_requested, 0);
            assert_eq!(
                counts.last_exit_code,
                i32::from(crate::directory_helper::EXIT_CODE_LIFELINE_LOST),
                "deadline cleanup must end via the lifeline watcher, never a watchdog"
            );
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn zero_budgets_expire_before_any_child_is_launched() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "hang_until_lifeline");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(Duration::ZERO)
                .expect("zero-budget probe should be admitted");
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::Deadline)
            );
            joined_shutdown(&mut controller).await;
            let counts = witness_counts();
            assert_eq!(counts.spawned, 0, "expired budgets must not launch work");
            assert_eq!(counts.reaps_observed, 0);
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn shutdown_during_an_active_operation_aborts_it_and_joins() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "hang_until_lifeline");

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("active-shutdown probe should be admitted");
            wait_for_witness(
                |counts| counts.stderr_bytes_seen >= 1,
                "fixture consumed its request (readiness byte counted)",
            )
            .await;
            let settled = settle_within(operation);
            let shutdown = async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                controller.shutdown().await
            };
            let (outcome, report) = tokio::join!(settled, shutdown);
            assert_eq!(outcome, Err(HelperOperationError::Shutdown));
            assert_eq!(report, ShutdownReport::Joined);
            let counts = witness_counts();
            assert!(counts.reaps_observed >= 1);
            assert_eq!(
                counts.last_exit_code,
                i32::from(crate::directory_helper::EXIT_CODE_LIFELINE_LOST),
                "shutdown-abort cleanup must end via the lifeline watcher, never a watchdog"
            );
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

#[test]
fn admission_after_shutdown_is_unavailable() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "hang_until_lifeline");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            joined_shutdown(&mut controller).await;
            assert!(matches!(
                controller.pick_directory(SUCCESS_BUDGET),
                Err(AdmissionError::Unavailable)
            ));
            assert_eq!(controller.health(), HealthState::Active);
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

// ---------------------------------------------------------------------------
// Bounded queue: immediate Busy, skipped abandoned queued work
// ---------------------------------------------------------------------------

#[test]
fn queue_capacity_four_returns_busy_immediately() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "hang_until_lifeline");

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            // One active child occupies the single-operation slot...
            let active = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("active job should be admitted");
            wait_for_witness(
                |counts| counts.stderr_bytes_seen >= 1,
                "active fixture consumed its request (readiness byte counted)",
            )
            .await;

            // ...then exactly four more jobs fill the bounded queue...
            let mut queued = Vec::new();
            for index in 0..4 {
                let operation = controller
                    .pick_directory(SUCCESS_BUDGET)
                    .unwrap_or_else(|error| panic!("queued job {index} should fit: {error:?}"));
                queued.push(operation);
            }
            // ...and the sixth admission hits immediate backpressure.
            assert!(
                matches!(
                    controller.pick_directory(SUCCESS_BUDGET),
                    Err(AdmissionError::Busy)
                ),
                "capacity beyond four queued jobs must return Busy"
            );

            // Dropping everything abandons each job; the owner skips abandoned
            // queued work before spawn, so no further child ever appears.
            drop(active);
            drop(queued);
            wait_for_witness(
                |counts| counts.reaps_observed >= 1,
                "active child reaped after abandonment",
            )
            .await;
            assert_eq!(
                witness_counts().last_exit_code,
                i32::from(crate::directory_helper::EXIT_CODE_LIFELINE_LOST),
                "the abandoned active child must end via its lifeline, never a watchdog"
            );

            joined_shutdown(&mut controller).await;
            let counts = witness_counts();
            assert_eq!(
                counts.spawned, 1,
                "queued work must never spawn after abandonment"
            );
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

// ---------------------------------------------------------------------------
// Start-time refusals and spawn failures
// ---------------------------------------------------------------------------

#[test]
fn relative_executables_are_rejected_before_any_spawn() {
    // Witness resets touch process-global counters, so this reset-only test
    // serializes against the child-owning scenarios like every other user.
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    reset_witnesses();
    let error = DirectoryController::start(
        DirectoryControllerConfig::new(PathBuf::from("relative/forge.exe")),
        runtime.handle(),
    )
    .expect_err("relative paths must be refused");
    assert_eq!(error, ControllerStartError::RelativeExecutable);
    assert_eq!(witness_counts().spawned, 0);
}

#[cfg(windows)]
#[test]
fn spawn_failure_settles_typed_without_a_reap_claim() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    reset_witnesses();
    let mut controller = DirectoryController::start(
        DirectoryControllerConfig::new(PathBuf::from(
            "\\\\?\\C:\\artisan-definitely-missing\\forge.exe",
        )),
        runtime.handle(),
    )
    .expect("an absolute missing executable still starts the controller");

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("spawn-failure probe should be admitted");
            assert_eq!(
                settle_within(operation).await,
                Err(HelperOperationError::SpawnFailed)
            );
            joined_shutdown(&mut controller).await;
            let counts = witness_counts();
            assert_eq!(counts.spawned, 0);
            assert_eq!(counts.reaps_observed, 0, "nothing existed to reap");
        });
    }));
    finish_scoped_scenario!(caught, runtime, controller);
}

// ---------------------------------------------------------------------------
// Forced generation-space end case (private allocator state only)
// ---------------------------------------------------------------------------

#[test]
fn generation_allocation_is_checked_nonzero_and_burned_once() {
    let mut allocator = GenerationAllocator::new();
    assert_eq!(allocator.mint(), Some(1));
    assert_eq!(allocator.mint(), Some(2));

    // Forced end of space: the maximum value mints exactly once, then the
    // allocator refuses forever. There is no public setter; this reaches
    // private cfg(test)-only state as the root contract allows.
    allocator.force_next(u64::MAX);
    assert_eq!(allocator.mint(), Some(u64::MAX));
    assert_eq!(allocator.mint(), None);
    assert_eq!(allocator.mint(), None);
}

// ---------------------------------------------------------------------------
// Honest caught-unwind cleanup: runtime and controller stay OUTSIDE
// ---------------------------------------------------------------------------

#[test]
fn caught_panic_between_operations_still_shuts_down_cleanly() {
    let _child_scenario_guard = serialize_child_scenarios();
    let runtime = build_runtime();
    let mut controller = started_fixture_controller(&runtime, "hang_until_lifeline");

    // Runtime and controller live OUTSIDE catch_unwind; the guarded closure
    // only borrows them through Runtime::block_on. The panic fires while the
    // helper is ACTIVELY hanging (readiness witnessed first) and unwinding
    // drops the accepted future — so the shutdown/reap evidence afterwards
    // genuinely happened AFTER the panic.
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.block_on(async {
            let operation = controller
                .pick_directory(SUCCESS_BUDGET)
                .expect("panic-scenario pick should be admitted");
            wait_for_witness(
                |counts| counts.stderr_bytes_seen >= 1,
                "fixture consumed its request (readiness byte counted)",
            )
            .await;
            drop(operation);
            panic!("deliberate mid-scenario panic while the helper was active");
        });
    }));
    assert!(caught.is_err(), "the deliberate panic must be observable");

    // While the runtime is STILL ALIVE: explicitly shut down on this
    // caught-unwind path before reporting anything.
    let report = runtime.block_on(async {
        tokio::time::timeout(AWAIT_BOUND, controller.shutdown())
            .await
            .expect("post-panic shutdown exceeded the causal bound")
    });
    assert_eq!(report, ShutdownReport::Joined);
    let counts = witness_counts();
    assert!(counts.spawned >= 1);
    assert!(
        counts.stderr_bytes_seen >= 1,
        "readiness was causally established"
    );
    assert!(
        counts.reaps_observed >= 1,
        "cleanup after the panic must reap"
    );
    assert_eq!(
        counts.last_exit_code,
        i32::from(crate::directory_helper::EXIT_CODE_LIFELINE_LOST),
        "post-panic cleanup must end via the lifeline watcher, never a watchdog"
    );
}

// ---------------------------------------------------------------------------
// Pure framing tests over the real shared parent-side codec
// ---------------------------------------------------------------------------

#[test]
fn request_encoding_follows_the_v1_layout() {
    let frame =
        encode_request(0x0102_0304_0506_0708, RequestKind::Pick, &[]).expect("empty pick encodes");
    assert_eq!(&frame[..4], &REQUEST_MAGIC);
    assert_eq!(frame[4], PROTOCOL_VERSION);
    assert_eq!(frame[5], crate::directory_helper_codec::REQUEST_TAG_PICK);
    assert_eq!(&frame[6..14], &0x0102_0304_0506_0708_u64.to_le_bytes());
    assert_eq!(&frame[14..18], &0_u32.to_le_bytes());
    assert_eq!(frame.len(), HEADER_LEN);

    let payload = b"C\\some path";
    let framed =
        encode_request(9, RequestKind::Validate, payload).expect("bounded validate encodes");
    assert_eq!(
        framed[5],
        crate::directory_helper_codec::REQUEST_TAG_VALIDATE
    );
    let declared = u32::try_from(payload.len()).expect("test payload length fits u32");
    assert_eq!(&framed[14..18], &declared.to_le_bytes());
    assert_eq!(&framed[HEADER_LEN..], payload);
}

#[test]
fn request_encoding_refuses_out_of_contract_shapes() {
    assert_eq!(
        encode_request(1, RequestKind::Pick, &[0_u8]),
        Err(RequestEncodeFault::PickCarriesPayload)
    );
    assert_eq!(
        encode_request(1, RequestKind::Validate, &[]),
        Err(RequestEncodeFault::EmptyValidatePayload)
    );
    let beyond = vec![b'a'; ROOT_PATH_MAX_BYTES + 1];
    assert_eq!(
        encode_request(1, RequestKind::Validate, &beyond),
        Err(RequestEncodeFault::ValidatePayloadBeyondBound {
            length: ROOT_PATH_MAX_BYTES + 1
        })
    );
}

#[test]
fn response_header_parsing_accepts_every_legal_shape() {
    let mut header = RESPONSE_MAGIC.to_vec();
    header.push(PROTOCOL_VERSION);
    header.push(1);
    header.extend_from_slice(&42_u64.to_le_bytes());
    header.extend_from_slice(&3_u32.to_le_bytes());

    let parsed = parse_response_header(&header).expect("selected header parses");
    assert_eq!(parsed.generation, 42);
    assert_eq!(parsed.payload_len, 3);
    assert_eq!(parsed.tag, 1);

    header[5] = 2;
    header[14..18].copy_from_slice(&0_u32.to_le_bytes());
    let empty = parse_response_header(&header).expect("empty-payload tag parses");
    assert_eq!(empty.tag, 2);
    assert_eq!(empty.payload_len, 0);
}

#[test]
fn response_header_parsing_classifies_every_structural_fault() {
    let sound = {
        let mut header = RESPONSE_MAGIC.to_vec();
        header.push(PROTOCOL_VERSION);
        header.push(2);
        header.extend_from_slice(&5_u64.to_le_bytes());
        header.extend_from_slice(&0_u32.to_le_bytes());
        header
    };

    assert_eq!(
        parse_response_header(&[]),
        Err(ResponseHeaderFault::Truncated)
    );
    assert_eq!(
        parse_response_header(&sound[..17]),
        Err(ResponseHeaderFault::Truncated)
    );

    let mut foreign = sound.clone();
    foreign[0] = b'X';
    assert_eq!(
        parse_response_header(&foreign),
        Err(ResponseHeaderFault::ForeignMagic)
    );

    let mut version = sound.clone();
    version[4] = 9;
    assert_eq!(
        parse_response_header(&version),
        Err(ResponseHeaderFault::UnsupportedVersion { found: 9 })
    );

    let mut tag = sound.clone();
    tag[5] = 7;
    assert_eq!(
        parse_response_header(&tag),
        Err(ResponseHeaderFault::UnsupportedTag { found: 7 })
    );

    let mut oversized = sound.clone();
    let beyond_bound =
        u32::try_from(ROOT_PATH_MAX_BYTES + 1).expect("shared bound plus one fits u32");
    oversized[14..18].copy_from_slice(&beyond_bound.to_le_bytes());
    assert_eq!(
        parse_response_header(&oversized),
        Err(ResponseHeaderFault::PayloadBeyondBound {
            declared: beyond_bound
        })
    );

    let mut carrying = sound.clone();
    carrying[14..18].copy_from_slice(&2_u32.to_le_bytes());
    assert_eq!(
        parse_response_header(&carrying),
        Err(ResponseHeaderFault::TagCarriesPayload { found: 2 })
    );

    let mut overlong = sound.clone();
    overlong.push(0);
    assert_eq!(
        parse_response_header(&overlong),
        Err(ResponseHeaderFault::Truncated)
    );
}

#[test]
fn response_encoder_still_refuses_selected_paths_beyond_the_bound() {
    let response = Response::Selected {
        canonical_path: "x".repeat(ROOT_PATH_MAX_BYTES + 1),
    };
    assert_eq!(
        encode_response(1, &response),
        Err(ResponseEncodeFault::SelectedBeyondBound {
            length: ROOT_PATH_MAX_BYTES + 1
        })
    );
}

#[test]
fn request_header_parser_remains_available_to_shared_tests() {
    // Guard the shared parser surface the helper side relies on so the
    // parent additions cannot silently drift the request direction.
    let mut header = REQUEST_MAGIC.to_vec();
    header.push(PROTOCOL_VERSION);
    header.push(crate::directory_helper_codec::REQUEST_TAG_PICK);
    header.extend_from_slice(&1_u64.to_le_bytes());
    header.extend_from_slice(&0_u32.to_le_bytes());
    let prelude = parse_request_header(&header).expect("pick request header parses");
    assert_eq!(prelude.generation, 1);
    assert_eq!(prelude.payload_len, 0);
}
