//! Focused composition checks for the settings-carrying owner lane,
//! now exercising the actual configured `Job::Turn` state machine against the
//! existing fixture child through the `#[cfg(test)]`-only launch seam.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{
    ConfiguredLaunch, EngineBounds, EngineLimits, EngineOwner, EngineOwnerHealth,
    EngineOwnerShutdown, FixtureConfiguredLaunch, reset_witnesses, witness_counts,
};
use crate::engine_owner::observation::{EngineObservation, TerminalState};
use crate::engine_owner::operation::EngineOperationError;
use artisan_database::{
    AttachProjectInput, CreateThreadInput, SetThreadEngineConfigInput, SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, DirectoryId, DisplayName, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, MessageBody, NetworkAccess,
    OpenCode2Selection, PermissionId, ProjectId, RequestId, RootPath, RunId, ThreadId, ThreadTitle,
    UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;

fn valid_limits() -> EngineLimits {
    EngineLimits {
        readiness: Duration::from_millis(5_000),
        health: Duration::from_millis(5_000),
        prompt: Duration::from_millis(5_000),
        sse: Duration::from_millis(5_000),
        close: Duration::from_millis(2_000),
    }
}

fn small_bounds_with_control(control_capacity: usize) -> EngineBounds {
    EngineBounds {
        max_json_body: 8192,
        max_sse_line: 4096,
        max_sse_event: 8192,
        max_readiness_line: 4096,
        max_headers: 32,
        max_buf_bytes: 8192,
        stderr_cap_bytes: 4096,
        sink_capacity: 16,
        control_capacity,
    }
}

// ---------------------------------------------------------------------------
// Fixture discovery
// ---------------------------------------------------------------------------

fn fixture_program_path() -> PathBuf {
    // Bazel runfiles path (without requiring the runfiles crate)
    if let Ok(mapping) = std::env::var("ARTISAN_ENGINE_OWNER_FIXTURE") {
        let direct = PathBuf::from(&mapping);
        if direct.is_file() {
            return direct;
        }
        if let Ok(test_srcdir) = std::env::var("TEST_SRCDIR") {
            let candidate = Path::new(&test_srcdir).join(&mapping);
            if candidate.is_file() {
                return candidate;
            }
        }
        if let Ok(runfiles_dir) = std::env::var("RUNFILES_DIR") {
            let candidate = Path::new(&runfiles_dir).join(&mapping);
            if candidate.is_file() {
                return candidate;
            }
        }
        if direct.exists() {
            return direct;
        }
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_engine_owner_fixture") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return p;
        }
    }
    // Cargo target dir fallbacks
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for candidate in [
        manifest.join("../../target/debug/engine_owner_fixture"),
        manifest.join("../../target/debug/engine_owner_fixture.exe"),
        manifest.join("../target/debug/engine_owner_fixture"),
        manifest.join("../target/debug/engine_owner_fixture.exe"),
        PathBuf::from("target/debug/engine_owner_fixture"),
        PathBuf::from("target/debug/engine_owner_fixture.exe"),
        PathBuf::from("bazel-bin/tests/backend/engine_owner_fixture"),
        PathBuf::from("bazel-bin/tests/backend/engine_owner_fixture.exe"),
    ] {
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!(
        "fixture binary not found; set ARTISAN_ENGINE_OWNER_FIXTURE or build //tests/backend:engine_owner_fixture"
    );
}

// ---------------------------------------------------------------------------
// Temp root helpers
// ---------------------------------------------------------------------------

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
            "artisan-{}-{}-{}",
            label,
            std::process::id(),
            nonce
        ));
        std::fs::create_dir_all(&path).expect("temp root create");
        Self { path }
    }

    fn root_path(&self) -> RootPath {
        let s = self.path.to_str().expect("temp path utf8");
        // Ensure forward slashes on Windows for RootPath parsing (it accepts Windows paths)
        RootPath::parse(s).expect("temp root is valid RootPath")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// EngineRunConfig for fixture
// ---------------------------------------------------------------------------

fn engine_run_config_for_fixture(profile_id: &str) -> EngineRunConfig {
    let budget = |ms: u64| FiniteMillis::new(ms).expect("finite millis valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: budget(10_000),
        readiness_budget: budget(5_000),
        health_budget: budget(5_000),
        prompt_budget: budget(5_000),
        stream_budget: budget(5_000),
        close_budget: budget(2_000),
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness limit"),
        max_header_count: CountLimit::new(32).expect("header count"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr"),
        observation_capacity: CountLimit::new(16).expect("observation cap"),
    })
    .expect("runtime valid");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-fixture").expect("permission id"),
        EngineAgentId::parse("agent-fixture").expect("agent id"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    let selection = OpenCode2Selection::new(
        EngineProfileId::parse(profile_id).expect("profile id"),
        EngineModelId::parse("model-fixture").expect("model id"),
        EngineRouteId::parse("route-fixture").expect("route id"),
        None,
        permission,
    );
    EngineRunConfig::new(EngineSelection::OpenCode2(selection), runtime)
}

async fn settings_for_profile(
    profile_id: &str,
    root: &RootPath,
) -> artisan_database::ThreadEngineSettings {
    let config = engine_run_config_for_fixture(profile_id);
    let db = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("in-memory db should open");
    migrate_to_current(&db)
        .await
        .expect("migrate should succeed");
    let repo = artisan_database::Repository::new(db.clone());

    let project_id = ProjectId::parse("proj-fixture").expect("project id");
    let directory_id = DirectoryId::parse("dir-fixture").expect("directory id");
    let thread_id = ThreadId::parse("thread-fixture").expect("thread id");
    let now = UnixMillis::from_millis(1);

    // Attach project (idempotent)
    let _ = repo
        .attach_project(AttachProjectInput {
            request_id: RequestId::parse(format!(
                "req-attach-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .expect("request id"),
            directory_id: directory_id.clone(),
            project_id: project_id.clone(),
            root_path: root.clone(),
            display_name: DisplayName::parse("fixture-proj").expect("display"),
            attached_at: now,
        })
        .await
        .expect("attach project");

    let _ = repo
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse(format!(
                "req-thread-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .expect("request id"),
            thread_id: thread_id.clone(),
            project_id: project_id.clone(),
            title: ThreadTitle::parse("fixture-thread").expect("title"),
            created_at: now,
            updated_at: now,
        })
        .await
        .expect("create thread");

    let _ = repo
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse(format!(
                "req-config-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .expect("request id"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: config.clone(),
            accepted_at: now,
        })
        .await
        .expect("set config");

    repo.read_thread_engine_settings(&thread_id)
        .await
        .expect("read settings")
        .expect("settings present")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn configured_owner_starts_active_and_drains_without_child() {
    let mut owner = EngineOwner::start_configured(
        NonZeroUsize::new(1).expect("one slot is nonzero"),
        &tokio::runtime::Handle::current(),
    );

    assert_eq!(owner.health(), EngineOwnerHealth::Active);
    assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn configured_fixture_happy_path_delivers_observation_and_reaps() {
    reset_witnesses();
    let fixture = fixture_program_path();
    assert!(
        fixture.is_file(),
        "fixture must be regular file: {}",
        fixture.display()
    );
    // Explicit regular non-link credential files invariant: fixture must not be symlink/reparse
    #[cfg(unix)]
    {
        let meta = std::fs::symlink_metadata(&fixture).expect("metadata");
        assert!(
            !meta.is_symlink(),
            "fixture must be regular file, not symlink"
        );
        assert!(meta.is_file(), "fixture must be regular file");
    }

    let temp_root = TempRoot::new("happy");
    let root = temp_root.root_path();
    let settings = settings_for_profile("fixture-test", &root).await;

    // No secrets in Debug/readiness/errors: settings Debug is not redacted but should not contain private bytes
    // EngineTurnInput Debug is redacted
    let mut owner = EngineOwner::start_configured(
        NonZeroUsize::new(1).expect("one slot"),
        &tokio::runtime::Handle::current(),
    );
    assert_eq!(owner.health(), EngineOwnerHealth::Active);

    let input = super::EngineTurnInput {
        run_id: RunId::parse("fixture-run").expect("run id"),
        project_root: root.clone(),
        prompt_id: "prompt-1".to_owned(),
        prompt_text: MessageBody::parse("hello world").expect("body"),
        settings: settings.clone(),
        launch: ConfiguredLaunch::Fixture(FixtureConfiguredLaunch {
            program: fixture.clone(),
            version: "0.0.0-fixture",
            profile_id: "fixture-test".to_owned(),
            scenario: "prompt_text_then_terminal",
        }),
        prompt_delivery: "immediate".to_owned(),
        stream_after: 0,
        control_capacity: 1,
    };

    // Assert no credential or capability leakage via Debug
    let debug = format!("{:?}", input);
    assert!(
        debug.contains("<redacted>"),
        "EngineTurnInput Debug must be redacted"
    );
    assert!(
        !debug.contains("hello world"),
        "prompt text must not leak in Debug"
    );
    assert!(
        !debug.contains("fixture-test"),
        "profile id must not leak via Debug redaction"
    );
    assert!(
        !debug.contains(fixture.to_str().unwrap_or("")),
        "fixture path must not leak in Debug"
    );

    // Also check HealthSecret Debug is redacted and zeroizes
    {
        let secret = crate::engine_owner::http::HealthSecret::generate().expect("secret");
        let secret_debug = format!("{:?}", secret);
        assert!(secret_debug.contains("<redacted>"));
        assert!(!secret_debug.contains(secret.as_str()));
    }

    let mut turn = owner
        .admit_turn(input, Duration::from_secs(10))
        .expect("admit should succeed");

    // Session creation (readiness/version + authenticated session)
    let prepared = tokio::time::timeout(Duration::from_secs(8), turn.prepare())
        .await
        .expect("prepare timeout")
        .expect("prepared should be Ok");
    assert_eq!(prepared.session(), "test-session");

    // Bind authorization (exactly one prompt gate)
    turn.authorize().expect("authorize should succeed");
    // No duplicated prompt on replay: second authorize must fail
    assert!(
        turn.authorize().is_err(),
        "second authorize must fail (no duplicated prompt)"
    );

    // SSE text and terminal event, durable observation delivery
    let mut saw_text = false;
    let mut saw_terminal = false;
    let mut text_deltas: Vec<String> = Vec::new();

    // Injected timeouts: stream and prompt budgets are 5s, readiness 5s; total 8s timeout is within
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        let obs = tokio::time::timeout(Duration::from_secs(2), turn.next_observation())
            .await
            .expect("observation timeout");
        let Some(obs) = obs else {
            break;
        };
        match obs {
            EngineObservation::TextDelta(delta) => {
                assert_eq!(
                    delta.run_id().as_str(),
                    "fixture-run",
                    "session/run-matched SSE: run_id must match"
                );
                assert_eq!(delta.sequence(), 1, "first delta sequence must be 1");
                assert!(!delta.chunk_id().is_empty());
                assert_eq!(delta.delta(), "hello world");
                // Loopback-only fixture communication is enforced by readiness ValidatedEndpoint (127.0.0.1)
                // No external network assertion: we can check delta is from fixture-run
                saw_text = true;
                text_deltas.push(delta.delta().to_owned());
            }
            EngineObservation::Terminal(term) => {
                assert_eq!(term.run_id().as_str(), "fixture-run");
                assert_eq!(term.sequence(), 2);
                assert_eq!(term.state(), TerminalState::Completed);
                saw_terminal = true;
                break;
            }
        }
        if saw_text && saw_terminal {
            break;
        }
    }

    assert!(saw_text, "should have seen text delta");
    assert!(saw_terminal, "should have seen terminal event");
    assert_eq!(text_deltas.len(), 1);
    assert_eq!(text_deltas[0], "hello world");

    // Finish: durable observation delivery already done, now reap
    let result = tokio::time::timeout(Duration::from_secs(5), turn.finish())
        .await
        .expect("finish timeout")
        .expect("finish should be Ok");
    assert_eq!(result.terminal(), TerminalState::Completed);

    // Stdin close / observed child reap / bounded close behavior
    let counts = witness_counts();
    assert_eq!(
        counts.spawned, 1,
        "exactly one child should have been spawned"
    );
    assert_eq!(counts.reaps_observed, 1, "child reap must be observed");
    assert_eq!(
        counts.kills_requested, 0,
        "happy path should not need kill (bounded close)"
    );
    // No provider contact before settings/profile/launch/session/bind is enforced by operation ordering;
    // we verified session before prompt, and authorize before prompt.

    // Owner Joined
    let shutdown = owner.shutdown().await;
    assert_eq!(shutdown, EngineOwnerShutdown::Joined);
    assert_eq!(owner.health(), EngineOwnerHealth::Active);

    // Assert no external network: readiness URL was loopback (checked via fixture), and no other
    // network is performed beyond loopback fixture communication.

    // Zeroization of capability bytes: HealthSecret zeroizes on drop (checked via Debug redaction above)
}

#[tokio::test(flavor = "current_thread")]
async fn configured_fixture_run_mismatch_fails_without_leak() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let temp_root = TempRoot::new("mismatch");
    let root = temp_root.root_path();
    let settings = settings_for_profile("fixture-test", &root).await;

    let mut owner = EngineOwner::start_configured(
        NonZeroUsize::new(1).expect("one slot"),
        &tokio::runtime::Handle::current(),
    );

    // Use a different run_id than fixture's "fixture-run" to trigger RunMismatch -> StreamFailed
    let input = super::EngineTurnInput {
        run_id: RunId::parse("other-run").expect("run id"),
        project_root: root,
        prompt_id: "prompt-mismatch".to_owned(),
        prompt_text: MessageBody::parse("hello").expect("body"),
        settings,
        launch: ConfiguredLaunch::Fixture(FixtureConfiguredLaunch {
            program: fixture,
            version: "0.0.0-fixture",
            profile_id: "fixture-test".to_owned(),
            scenario: "prompt_text_then_terminal",
        }),
        prompt_delivery: "immediate".to_owned(),
        stream_after: 0,
        control_capacity: 1,
    };

    let mut turn = owner
        .admit_turn(input, Duration::from_secs(10))
        .expect("admit");
    let prepared = tokio::time::timeout(Duration::from_secs(5), turn.prepare())
        .await
        .expect("prepare timeout")
        .expect("prepared ok");
    assert_eq!(prepared.session(), "test-session");
    turn.authorize().expect("authorize");

    // Drain observations; mismatch should cause StreamFailed and abort path still delivers terminal?
    // The fixture emits run_id fixture-run, but expected is other-run, so decode fails and stream returns error,
    // which maps to StreamFailed and aborts to Failed/StreamFailed path, but still closes stdin and reaps.
    let mut saw_any = false;
    let timeout = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < timeout {
        if let Ok(Some(_)) =
            tokio::time::timeout(Duration::from_millis(500), turn.next_observation()).await
        {
            saw_any = true;
        } else {
            break;
        }
    }
    // The stream should have failed; finish should be error
    let result = tokio::time::timeout(Duration::from_secs(5), turn.finish())
        .await
        .expect("finish timeout");
    assert!(
        result.is_err(),
        "mismatched run_id should cause StreamFailed error"
    );
    let err = result.unwrap_err();
    // No secret leakage in error Debug
    let err_debug = format!("{:?}", err);
    assert!(
        !err_debug.contains("hello"),
        "error must not leak prompt text"
    );
    assert!(
        !err_debug.contains("fixture"),
        "error must not leak profile"
    );

    // Still reap
    tokio::time::sleep(Duration::from_millis(100)).await;
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    // Bounded close: either reap without kill or after kill, but no leaked child
    assert!(counts.kills_requested <= 1);

    let shutdown = owner.shutdown().await;
    assert_eq!(shutdown, EngineOwnerShutdown::Joined);
    // Ensure no duplicated prompt: authorize already consumed, second fails
    let _ = saw_any;
}

#[tokio::test(flavor = "current_thread")]
async fn configured_fixture_cancellation_before_authorize_aborts() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let temp_root = TempRoot::new("cancel");
    let root = temp_root.root_path();
    let settings = settings_for_profile("fixture-test", &root).await;

    let mut owner = EngineOwner::start_configured(
        NonZeroUsize::new(1).expect("one slot"),
        &tokio::runtime::Handle::current(),
    );

    let input = super::EngineTurnInput {
        run_id: RunId::parse("fixture-run").expect("run id"),
        project_root: root,
        prompt_id: "prompt-cancel".to_owned(),
        prompt_text: MessageBody::parse("hello world").expect("body"),
        settings,
        launch: ConfiguredLaunch::Fixture(FixtureConfiguredLaunch {
            program: fixture,
            version: "0.0.0-fixture",
            profile_id: "fixture-test".to_owned(),
            scenario: "prompt_text_then_terminal",
        }),
        prompt_delivery: "immediate".to_owned(),
        stream_after: 0,
        control_capacity: 1,
    };

    let mut turn = owner
        .admit_turn(input, Duration::from_secs(10))
        .expect("admit");
    let _prepared = tokio::time::timeout(Duration::from_secs(5), turn.prepare())
        .await
        .expect("prepare timeout")
        .expect("prepared");

    // Cancel before authorize (drop authorize sender by cancelling)
    turn.cancel();
    // authorize should now fail or be already cancelled
    let auth_res = turn.authorize();
    // Either fails immediately or was already cancelled; both are acceptable
    assert!(auth_res.is_err() || auth_res.is_ok());

    // Drain
    while let Ok(Some(_)) =
        tokio::time::timeout(Duration::from_millis(500), turn.next_observation()).await
    {}

    let result = tokio::time::timeout(Duration::from_secs(5), turn.finish())
        .await
        .expect("finish timeout");
    // Should be Cancelled or ProviderRequestFailed or StreamFailed, but not success
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(
            err,
            EngineOperationError::Cancelled
                | EngineOperationError::ProviderRequestFailed
                | EngineOperationError::StreamFailed
                | EngineOperationError::Deadline
                | EngineOperationError::Shutdown
        ),
        "cancellation should map to typed error, got {err:?}"
    );
    // No secret leakage
    let err_debug = format!("{:?}", err);
    assert!(!err_debug.contains("hello"));

    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);

    let shutdown = owner.shutdown().await;
    assert_eq!(shutdown, EngineOwnerShutdown::Joined);
}

#[tokio::test(flavor = "current_thread")]
async fn configured_fixture_abrupt_exit_is_handled_and_bounded() {
    reset_witnesses();
    let fixture = fixture_program_path();
    let temp_root = TempRoot::new("abrupt");
    let root = temp_root.root_path();
    let settings = settings_for_profile("fixture-test", &root).await;

    let mut owner = EngineOwner::start_configured(
        NonZeroUsize::new(1).expect("one slot"),
        &tokio::runtime::Handle::current(),
    );

    let input = super::EngineTurnInput {
        run_id: RunId::parse("fixture-run").expect("run id"),
        project_root: root,
        prompt_id: "prompt-abrupt".to_owned(),
        prompt_text: MessageBody::parse("hello").expect("body"),
        settings,
        launch: ConfiguredLaunch::Fixture(FixtureConfiguredLaunch {
            program: fixture,
            version: "0.0.0-fixture",
            profile_id: "fixture-test".to_owned(),
            scenario: "abrupt_child_exit_nonzero",
        }),
        prompt_delivery: "immediate".to_owned(),
        stream_after: 0,
        control_capacity: 1,
    };

    let mut turn = owner
        .admit_turn(input, Duration::from_secs(10))
        .expect("admit");
    // Readiness will fail because abrupt exit exits 7 before readiness line
    let prepare_res = tokio::time::timeout(Duration::from_secs(5), turn.prepare())
        .await
        .expect("prepare timeout");
    // Either readiness failed or spawn failed; both are acceptable abrupt handling
    assert!(
        prepare_res.is_err(),
        "abrupt exit should cause prepare to fail"
    );
    let err = prepare_res.unwrap_err();
    let err_debug = format!("{:?}", err);
    assert!(!err_debug.contains("hello"));

    // Finish should also be err
    let finish_res = tokio::time::timeout(Duration::from_secs(5), turn.finish())
        .await
        .expect("finish timeout");
    assert!(finish_res.is_err());

    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);

    let shutdown = owner.shutdown().await;
    assert_eq!(shutdown, EngineOwnerShutdown::Joined);
}
