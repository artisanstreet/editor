//! Finite X1 Codex lifecycle proofs without the real CLI.
//!
//! Pure coverage (settings, frames, tracker, domain bridging, stall
//! predicate, binding round trip) plus fixture stdio script turns: a
//! temporary `cmd`/`sh` script types canned app-server JSONL over stdout
//! while the test drives initialize, thread/start, one authorized prompt,
//! and the streaming pump through the shared [`super::codex`] helpers. No
//! real `codex` binary, no catalog flag, no frontend selection.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use artisan_database::{
    AttachProjectInput, CreateThreadInput, SetThreadEngineConfigInput, SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CodexModelContextWindow, CodexReasoningEffort, CodexSelection,
    CodexServiceTier, CountLimit, DirectoryId, DisplayName, EngineAgentId, EngineConfigUpdatePrecondition,
    EngineId, EngineModelId, EnginePermissionPolicy, EngineProfileId, EngineRunConfig,
    EngineRuntimeControls, EngineRuntimeControlsInput, EngineSelection, FilesystemAccess,
    FiniteMillis, NetworkAccess, ObservationId, ObservationSequence, PermissionId, ProjectId,
    QueueMessagePayload, RequestId, RootPath, RunId, RunUsageBasis, ThreadId, ThreadTitle,
    UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use artisan_native_engine::NativeCodexAuthority;
use artisan_transport::CancelHandle;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::codex::{
    CODEX_MAX_FRAME_BYTES, CodexContinuationDecision, CodexContinuationGateInput, CodexEvent,
    CodexPendingTracker, CodexQuotaWindowKind, CodexSettings, CodexTurnState,
    CodexUsageAttribution, CodexUsageContext, CodexUsageScope, answer_approval, answer_questions,
    apply_event, check_codex_native_continuation, clamp_codex_percent_used,
    classify_codex_quota_window_kind, classify_exit, codex_account_read_line,
    codex_cli_meets_minimum, codex_rate_limits_read_line, codex_requires_group_termination,
    codex_reset_at_iso, codex_usage_report, has_stalled, initialize_params, interrupt_live_turn,
    is_codex_error_response, map_codex_rate_limit_windows, notification_line, parse_frame,
    parse_thread_token_usage, request_line, steer_live_turn, terminal_observation,
    thread_resume_params, write_line,
};
use super::observation::{EngineObservation, TerminalState};
use super::operation::{
    AcceptedTurn, EngineOperationError, codex_response_id_matches, codex_resumed_thread_id,
    codex_thread_id, codex_turn_id, is_codex_result_for,
};
use super::{EngineCodexTurnInput, EngineOwner, EngineOwnerShutdown};
use crate::native_run_dispatch::{binding_bytes_vec, binding_matches_bytes};

// ---------------------------------------------------------------------------
// Selection and settings
// ---------------------------------------------------------------------------

fn permission(
    approval: ApprovalMode,
    filesystem: FilesystemAccess,
    network: NetworkAccess,
) -> EnginePermissionPolicy {
    EnginePermissionPolicy::new(
        PermissionId::parse("permission-codex").expect("permission id"),
        EngineAgentId::parse("agent-codex").expect("agent id"),
        approval,
        filesystem,
        network,
        WebSearchAccess::Disabled,
    )
}

fn codex_selection() -> CodexSelection {
    CodexSelection::new(
        EngineProfileId::parse("codex-fixture").expect("profile id"),
        Some(EngineModelId::parse("codex-model").expect("model id")),
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        Some(CodexReasoningEffort::High),
        Some(CodexServiceTier::Fast),
        Some(CodexModelContextWindow::new(1_000).expect("window")),
    )
    .expect("codex selection valid")
}

#[test]
fn codex_settings_map_selection_without_coercion() {
    let settings = CodexSettings::from_selection(&codex_selection()).expect("settings valid");
    assert_eq!(settings.profile_id(), "codex-fixture");
    let root_path = std::env::temp_dir().join("codex-fixture-settings");
    let root = RootPath::parse(root_path.to_str().expect("temp path utf8")).expect("root");
    let params = settings.thread_params(&root);
    assert_eq!(params["approvalPolicy"], "on-request");
    assert_eq!(params["sandbox"], "workspace-write");
    assert_eq!(params["cwd"], root_path.to_str().expect("temp path utf8"));
    assert_eq!(params["model"], "codex-model");
    assert_eq!(params["serviceTier"], "fast");
    assert_eq!(params["config"]["model_reasoning_effort"], "high");
    assert_eq!(params["config"]["model_context_window"], 1_000);
}

#[test]
fn codex_settings_reject_always_approval() {
    let selection = CodexSelection::new(
        EngineProfileId::parse("codex-fixture").expect("profile id"),
        None,
        permission(
            ApprovalMode::Always,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        None,
        None,
        None,
    );
    assert!(selection.is_err(), "always approval must fail closed");
}

#[test]
fn initialize_params_carry_opt_out_notifications() {
    let params = initialize_params("artisan-editor", "0.3.0");
    let opted = params["capabilities"]["optOutNotificationMethods"]
        .as_array()
        .expect("opt-out list")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        opted,
        vec![
            "account/rateLimits/updated",
            "mcpServer/startupStatus/updated",
            "remoteControl/status/changed",
        ]
    );
    let line = request_line(1, "initialize", &params);
    assert!(line.contains("\"method\":\"initialize\""));
}

#[test]
fn initialized_notification_carries_no_id_or_params() {
    // Official handshake order (`Handshake` in
    // `modules/engines/src/codex/app-server-session.ts`): `Notify("initialized")`
    // writes `{ method }` with no id and no params, and never consumes a
    // request id.
    let line = notification_line("initialized");
    assert_eq!(line, r#"{"method":"initialized"}"#);
    let value: serde_json::Value = serde_json::from_str(&line).expect("valid json");
    assert!(value.get("id").is_none());
    assert!(value.get("params").is_none());
    assert!(!is_codex_error_response(&line));
}

#[test]
fn turn_start_params_bind_thread_id_and_fast_tier() {
    let settings = CodexSettings::from_selection(&codex_selection()).expect("settings valid");
    let params = settings.turn_start_params("thread-fixture-1", "hello");
    assert_eq!(params["threadId"], "thread-fixture-1");
    assert_eq!(params["input"][0]["text"], "hello");
    assert_eq!(params["input"][0]["type"], "text");
    assert_eq!(params["serviceTier"], "fast");

    // A standard-tier selection carries no service tier on the turn either.
    let standard = CodexSelection::new(
        EngineProfileId::parse("codex-fixture").expect("profile id"),
        Some(EngineModelId::parse("codex-model").expect("model id")),
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        None,
        None,
        None,
    )
    .expect("standard selection valid");
    let standard_settings = CodexSettings::from_selection(&standard).expect("settings valid");
    let standard_params = standard_settings.turn_start_params("thread-fixture-1", "hello");
    assert_eq!(standard_params["threadId"], "thread-fixture-1");
    assert!(standard_params.get("serviceTier").is_none());
}

/// Emulates the real server's strict `turn/start` validation: the request
/// is accepted only with a non-empty `threadId`, otherwise the server
/// answers `-32600 Invalid request: missing field threadId` and starts no
/// inference.
fn strict_turn_start_result(id: u64, params: &serde_json::Value) -> String {
    let thread_bound = params
        .get("threadId")
        .and_then(|value| value.as_str())
        .is_some_and(|thread| !thread.is_empty());
    if thread_bound {
        serde_json::json!({"id": id, "result": {"turn": {"id": "turn-1"}}}).to_string()
    } else {
        serde_json::json!({"id": id, "error": {"code": -32600, "message": "Invalid request: missing field threadId"}})
            .to_string()
    }
}

#[test]
fn strict_server_rejects_turn_start_without_thread_id() {
    // The production builder always binds the native thread id, so the
    // strict server accepts it and names the turn.
    let settings = CodexSettings::from_selection(&codex_selection()).expect("settings valid");
    let bound = settings.turn_start_params("thread-fixture-1", "hello");
    let accepted = strict_turn_start_result(3, &bound);
    assert!(!is_codex_error_response(&accepted));
    assert!(is_codex_result_for(&accepted, 3));
    assert_eq!(codex_turn_id(&accepted, 3).as_deref(), Some("turn-1"));

    // The pre-fix shape (`input` without `threadId`) is rejected with the
    // exact real-CLI error and yields no turn identity to pump.
    let unbound = serde_json::json!({"input": [{"text": "hello", "text_elements": [], "type": "text"}]});
    let rejected = strict_turn_start_result(3, &unbound);
    let rejected_value: serde_json::Value =
        serde_json::from_str(&rejected).expect("error envelope is valid json");
    assert_eq!(rejected_value["error"]["code"], -32600);
    assert_eq!(
        rejected_value["error"]["message"],
        "Invalid request: missing field threadId"
    );
    assert!(is_codex_error_response(&rejected));
    assert!(!is_codex_result_for(&rejected, 3));
    assert_eq!(codex_turn_id(&rejected, 3), None);
}

#[test]
fn turn_start_error_response_fails_fast() {
    // A matching error envelope fails the wait fast: it is an error
    // response, never a result, and never a turn identity.
    assert!(is_codex_error_response(TURN_MISSING_THREAD_ID_ERROR_LINE));
    assert!(!is_codex_result_for(TURN_MISSING_THREAD_ID_ERROR_LINE, 3));
    assert!(!codex_response_id_matches(TURN_MISSING_THREAD_ID_ERROR_LINE, 2));
    assert!(codex_response_id_matches(TURN_MISSING_THREAD_ID_ERROR_LINE, 3));
    assert_eq!(codex_turn_id(TURN_MISSING_THREAD_ID_ERROR_LINE, 3), None);

    // The success envelope is the opposite on every discriminant.
    assert!(!is_codex_error_response(TURN_LINE));
    assert!(is_codex_result_for(TURN_LINE, 3));
    assert!(!is_codex_result_for(TURN_LINE, 2));
    assert_eq!(codex_turn_id(TURN_LINE, 3).as_deref(), Some("turn-1"));
    assert_eq!(codex_turn_id(TURN_LINE, 2), None);

    // Method envelopes (notifications and server requests) are never error
    // responses, so the pump still routes them to the event pipeline.
    let notification = r#"{"method":"thread/started","params":{"threadId":"thread-fixture-1"}}"#;
    assert!(!is_codex_error_response(notification));
}

// ---------------------------------------------------------------------------
// Frame normalization
// ---------------------------------------------------------------------------

fn run_id() -> RunId {
    RunId::parse("codex-run-1").expect("run id")
}

#[test]
fn delta_turn_approval_question_subagent_frames_decode() {
    let delta = parse_frame(
        r#"{"method":"item/agentMessage/delta","params":{"delta":"hi","itemId":"item-1","threadId":"t-1","turnId":"turn-1"}}"#,
        1,
    )
    .expect("delta decodes");
    assert!(matches!(delta, CodexEvent::AgentMessageDelta { .. }));

    let completed = parse_frame(
        r#"{"method":"turn/completed","params":{"threadId":"t-1","turn":{"id":"turn-1","status":"completed"}}}"#,
        2,
    )
    .expect("turn decodes");
    assert!(matches!(
        completed,
        CodexEvent::TurnState {
            state: CodexTurnState::Completed,
            ..
        }
    ));

    let approval = parse_frame(
        r#"{"id":10,"method":"item/commandExecution/requestApproval","params":{"itemId":"cmd-1","command":"echo hi","cwd":"/tmp","reason":"say hi"}}"#,
        3,
    )
    .expect("approval decodes");
    match approval {
        CodexEvent::ApprovalRequested(request) => {
            assert_eq!(request.approval_id(), "10");
            assert_eq!(request.description(), "say hi");
            let domain = request.to_domain_request().expect("domain request");
            assert_eq!(domain.command_text(), Some("echo hi"));
        }
        _ => panic!("expected approval"),
    }

    let question = parse_frame(
        r#"{"method":"item/tool/requestUserInput","params":{"itemId":"q-1","threadId":"t-1","turnId":"turn-1","questions":[{"header":"Pick","id":"q1","question":"Which?"}]}}"#,
        4,
    )
    .expect("question decodes");
    match question {
        CodexEvent::QuestionRequested(request) => {
            assert_eq!(request.questions().len(), 1);
            let input = request.questions()[0]
                .to_domain_input()
                .expect("domain input");
            assert_eq!(input.text, "Which?");
        }
        _ => panic!("expected question"),
    }

    let subagent = parse_frame(
        r#"{"method":"item/subAgent/discovered","params":{"agentThreadId":"child-1","parentThreadId":"t-1"}}"#,
        5,
    )
    .expect("subagent decodes");
    assert!(matches!(subagent, CodexEvent::SubagentDiscovered { .. }));
}

#[test]
fn unknown_opt_out_and_malformed_frames_reject_safely() {
    let unknown = parse_frame(r#"{"method":"future/method","params":{}}"#, 1)
        .expect("unknown stays observable");
    assert!(matches!(unknown, CodexEvent::UnknownMethod));

    let opted = parse_frame(
        r#"{"method":"mcpServer/startupStatus/updated","params":{}}"#,
        2,
    )
    .expect("opt-out stays observable");
    assert!(matches!(opted, CodexEvent::OptedOut));

    assert!(parse_frame("not json", 3).is_err());
    assert!(parse_frame(r#"{"params":{}}"#, 4).is_err());
    assert!(parse_frame("", 5).is_err());
    let oversized = "x".repeat(CODEX_MAX_FRAME_BYTES + 1);
    assert!(parse_frame(oversized.as_str(), 6).is_err());
}

// ---------------------------------------------------------------------------
// Tracker, bridging, stall predicate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approval_deny_then_allow_resolves_without_side_effect() {
    let mut tracker = CodexPendingTracker::new();
    let (mut client, server) = tokio::io::duplex(65_536);
    let mut server = BufReader::new(server);
    let run = run_id();

    for (approval_id, approved, decision) in [
        ("approval-1", false, "denied"),
        ("approval-2", true, "approved"),
    ] {
        let event = parse_frame(
            &format!(
                r#"{{"method":"item/commandExecution/requestApproval","params":{{"itemId":"{approval_id}","command":"echo hi"}}}}"#
            ),
            1,
        )
        .expect("approval decodes");
        let CodexEvent::ApprovalRequested(request) = event else {
            panic!("expected approval")
        };
        assert!(tracker.note_approval(request));
        assert_eq!(tracker.pending_approvals(), 1);
        answer_approval(&mut client, &mut tracker, "7", approval_id, approved)
            .await
            .expect("answer writes");
        assert_eq!(tracker.pending_approvals(), 0);
        let mut line = String::new();
        server
            .read_line(&mut line)
            .await
            .expect("response readable");
        let value: serde_json::Value = serde_json::from_str(line.trim()).expect("response json");
        assert_eq!(value["id"], "7");
        assert_eq!(value["result"]["decision"], decision);
        // The run continues: deltas still normalize after a deny.
        let delta = parse_frame(
            r#"{"method":"item/agentMessage/delta","params":{"delta":"onward","itemId":"item-1","threadId":"t-1","turnId":"turn-1"}}"#,
            2,
        )
        .expect("delta decodes");
        let (sender, mut receiver) = mpsc::channel(8);
        let mut active = None;
        let terminal = apply_event(delta, &run, &mut tracker, &mut active, &sender, 2, None).await;
        assert_eq!(terminal, None);
        let EngineObservation::TextDelta(chunk) = receiver.try_recv().expect("delta observed")
        else {
            panic!("expected text delta")
        };
        assert_eq!(chunk.delta(), "onward");
    }
    // A resolved target never answers twice.
    let (mut client, _) = tokio::io::duplex(65_536);
    let denied = answer_approval(&mut client, &mut tracker, "8", "approval-1", true).await;
    assert!(denied.is_err(), "resolved approval must miss");
}

#[tokio::test]
async fn question_answer_and_steer_verbs_shape_lines() {
    let mut tracker = CodexPendingTracker::new();
    let event = parse_frame(
        r#"{"method":"item/tool/requestUserInput","params":{"itemId":"q-1","threadId":"t-1","turnId":"turn-1","questions":[{"header":"Pick","id":"q1","question":"Which?"}]}}"#,
        1,
    )
    .expect("question decodes");
    let CodexEvent::QuestionRequested(request) = event else {
        panic!("expected question")
    };
    assert_eq!(tracker.note_questions(&request), 1);

    let (mut client, server) = tokio::io::duplex(65_536);
    let mut server = BufReader::new(server);
    answer_questions(
        &mut client,
        &mut tracker,
        "9",
        &[("q1".to_owned(), vec!["first".to_owned()])],
    )
    .await
    .expect("question answer writes");
    let mut line = String::new();
    server.read_line(&mut line).await.expect("answer readable");
    let value: serde_json::Value = serde_json::from_str(line.trim()).expect("answer json");
    assert_eq!(value["result"]["answers"]["q1"]["answers"][0], "first");

    let mut request_id = 41;
    steer_live_turn(&mut client, &mut request_id, "t-1", "turn-1", "follow up")
        .await
        .expect("steer writes");
    assert_eq!(request_id, 42);
    line.clear();
    server.read_line(&mut line).await.expect("steer readable");
    let steer: serde_json::Value = serde_json::from_str(line.trim()).expect("steer json");
    assert_eq!(steer["method"], "turn/steer");
    assert_eq!(steer["params"]["expectedTurnId"], "turn-1");

    interrupt_live_turn(&mut client, &mut request_id, "t-1", "turn-1")
        .await
        .expect("interrupt writes");
    line.clear();
    server
        .read_line(&mut line)
        .await
        .expect("interrupt readable");
    let interrupt: serde_json::Value = serde_json::from_str(line.trim()).expect("interrupt json");
    assert_eq!(interrupt["method"], "turn/interrupt");
}

#[tokio::test]
async fn subagent_frames_never_adopt_the_root_turn() {
    let run = run_id();
    let mut tracker = CodexPendingTracker::new();
    let event = parse_frame(
        r#"{"method":"item/subAgent/discovered","params":{"agentThreadId":"child-1","parentThreadId":"t-1"}}"#,
        1,
    )
    .expect("subagent decodes");
    let (sender, mut receiver) = mpsc::channel(8);
    let mut active = None;
    let terminal = apply_event(event, &run, &mut tracker, &mut active, &sender, 1, None).await;
    assert_eq!(terminal, None);
    assert!(receiver.try_recv().is_err(), "no root observation");
    assert_eq!(tracker.subagent_count(), 1);
    assert_eq!(active, None, "root turn untouched");
}

#[test]
fn stall_predicate_requires_an_active_turn() {
    let now = Instant::now();
    assert!(!has_stalled(false, now, Duration::from_millis(1), now));
    assert!(has_stalled(
        true,
        now - Duration::from_secs(1),
        Duration::from_millis(100),
        now
    ));
    assert!(!has_stalled(true, now, Duration::from_secs(60), now));
}

#[test]
fn domain_bridging_validates_approval_and_question_rows() {
    let approval = parse_frame(
        r#"{"method":"item/fileChange/requestApproval","params":{"itemId":"approval-9","reason":"apply patch"}}"#,
        1,
    )
    .expect("file approval decodes");
    let CodexEvent::ApprovalRequested(request) = approval else {
        panic!("expected approval")
    };
    let domain_request = request.to_domain_request().expect("domain request");
    let observation = artisan_domain::ApprovalObservation::requested(
        ObservationId::parse("obs-1").expect("obs id"),
        ObservationSequence::new(1).expect("sequence"),
        ObservationId::parse(request.approval_id()).expect("approval id"),
        request.description().to_owned(),
        domain_request,
    );
    assert!(observation.is_ok());

    let question = parse_frame(
        r#"{"method":"item/tool/requestUserInput","params":{"itemId":"q-1","threadId":"t-1","turnId":"turn-1","questions":[{"id":"q2","question":"Which color?"}]}}"#,
        2,
    );
    assert!(question.is_ok());
}

// ---------------------------------------------------------------------------
// Binding tag/format round trip plus mismatch requeue
// ---------------------------------------------------------------------------

#[test]
fn codex_binding_round_trip_and_mismatch() {
    let raw =
        binding_bytes_vec("codex", "codex-fixture", "thread-fixture-1").expect("binding builds");
    assert!(binding_matches_bytes(
        &raw,
        "codex",
        "codex-fixture",
        "thread-fixture-1"
    ));
    for (engine, profile, session) in [
        ("opencode2", "codex-fixture", "thread-fixture-1"),
        ("codex", "other-profile", "thread-fixture-1"),
        ("codex", "codex-fixture", "other-thread"),
    ] {
        assert!(
            !binding_matches_bytes(&raw, engine, profile, session),
            "mismatch must requeue"
        );
    }
    let mut tampered = raw.clone();
    tampered.push(b'}');
    assert!(!binding_matches_bytes(
        &tampered,
        "codex",
        "codex-fixture",
        "thread-fixture-1"
    ));
    assert!(
        artisan_database::ProviderBindingBytes::new(raw).is_ok(),
        "wrapped bytes stay valid"
    );
    assert!(binding_bytes_vec("", "codex-fixture", "thread-fixture-1").is_none());
    assert!(binding_bytes_vec("codex", "", "thread-fixture-1").is_none());
    assert!(binding_bytes_vec("codex", "codex-fixture", "").is_none());
}

// ---------------------------------------------------------------------------
// Fixture stdio script turns (no real CLI)
// ---------------------------------------------------------------------------

struct FixtureScript {
    directory: PathBuf,
}

impl FixtureScript {
    fn new(responses: &str, tail: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("artisan-codex-{}-{}", std::process::id(), nonce));
        std::fs::create_dir_all(&directory).expect("fixture dir");
        std::fs::write(directory.join("responses.jsonl"), responses).expect("responses");
        #[cfg(windows)]
        {
            let script = format!("@echo off\r\ntype \"%~dp0responses.jsonl\"\r\n{tail}\r\n");
            std::fs::write(directory.join("fixture.cmd"), script).expect("script");
        }
        #[cfg(not(windows))]
        {
            let script = format!("#!/bin/sh\ncat \"$(dirname \"$0\")/responses.jsonl\"\n{tail}\n");
            let path = directory.join("fixture.sh");
            std::fs::write(&path, script).expect("script");
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = std::fs::metadata(&path).expect("meta").permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&path, permissions).expect("chmod");
        }
        Self { directory }
    }

    fn spawn(&self) -> tokio::process::Child {
        #[cfg(windows)]
        {
            let script = self.directory.join("fixture.cmd");
            tokio::process::Command::new("cmd")
                .arg("/C")
                .arg(&script)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .expect("fixture child spawns")
        }
        #[cfg(not(windows))]
        {
            tokio::process::Command::new(self.directory.join("fixture.sh"))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .expect("fixture child spawns")
        }
    }
}

impl Drop for FixtureScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

const INIT_LINE: &str = r#"{"id":1,"result":{"codexHome":"C:\\x","platformFamily":"windows","platformOs":"windows","userAgent":"test"}}"#;
const THREAD_LINE: &str = r#"{"id":2,"result":{"thread":{"id":"thread-fixture-1"}}}"#;
const TURN_LINE: &str = r#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#;
const TURN_MISSING_THREAD_ID_ERROR_LINE: &str = r#"{"id":3,"error":{"code":-32600,"message":"Invalid request: missing field threadId"}}"#;

struct FixtureOutcome {
    terminal: Option<TerminalState>,
    deltas: Vec<String>,
    tracker: CodexPendingTracker,
    thread_id: Option<String>,
}

/// Drives one fixture script turn through the official sequence —
/// initialize, the `initialized` notification, thread/start, the id-bound
/// `turn/start` (with its synchronously awaited result), and the streaming
/// pump with stall/cancel/EOF mapping.
async fn run_fixture_turn(
    responses: &str,
    tail: &str,
    inactivity: Duration,
    cancel_after: Option<Duration>,
) -> FixtureOutcome {
    let script = FixtureScript::new(responses, tail);
    let mut child = script.spawn();
    let mut stdin = child.stdin.take().expect("fixture stdin");
    let stdout = child.stdout.take().expect("fixture stdout");
    let mut reader = BufReader::new(stdout);
    let shutdown = CancelHandle::new();
    let control = Arc::new(CancelHandle::new());
    let deadline = Instant::now() + Duration::from_secs(20);

    write_line(
        &mut stdin,
        &request_line(1, "initialize", &initialize_params("a", "0")),
    )
    .await
    .expect("init writes");
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("init reads");
    assert!(is_codex_result_for(&line, 1));
    // Official order: `initialized` notification (no id, no params) before
    // any `thread/*` request. The canned script ignores stdin, so no reply
    // is read for it — the write itself is the sequence under test.
    write_line(&mut stdin, &notification_line("initialized"))
        .await
        .expect("initialized writes");
    let root_path = std::env::temp_dir().join("codex-fixture-turn");
    let root = RootPath::parse(root_path.to_str().expect("temp path utf8")).expect("root");
    let settings = CodexSettings::from_selection(&codex_selection()).expect("settings");
    write_line(
        &mut stdin,
        &request_line(2, "thread/start", &settings.thread_params(&root)),
    )
    .await
    .expect("thread writes");
    line.clear();
    reader.read_line(&mut line).await.expect("thread reads");
    let thread_id = codex_thread_id(&line, 2).expect("thread id");
    // Production binds the native thread id on `turn/start`; the strict
    // server rejects a missing `threadId` with `-32600`, so the harness
    // awaits the turn result exactly like the owner and fails the turn
    // fast on an error envelope instead of pumping silence.
    write_line(
        &mut stdin,
        &request_line(3, "turn/start", &settings.turn_start_params(&thread_id, "")),
    )
    .await
    .expect("turn writes");
    line.clear();
    reader.read_line(&mut line).await.expect("turn reads");
    assert!(
        !is_codex_error_response(&line),
        "turn/start must not answer with a JSON-RPC error"
    );
    let provider_turn_id = codex_turn_id(&line, 3).expect("turn id");

    if let Some(after) = cancel_after {
        let task_control = Arc::clone(&control);
        // Detached: the pump below observes the cancellation mid-stream.
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            task_control.cancel();
        });
    }

    let run = run_id();
    let (sender, mut receiver) = mpsc::channel(64);
    let mut tracker = CodexPendingTracker::new();
    // The turn is in flight from the server's `turn/start` result, mirroring
    // the owner: silence from here on owes output to the inactivity deadline.
    let mut active: Option<String> = Some(provider_turn_id);
    let mut sequence: u64 = 0;
    let mut last_activity = Instant::now();
    let terminal = loop {
        if control.is_cancelled() {
            break Some(TerminalState::Cancelled);
        }
        if has_stalled(active.is_some(), last_activity, inactivity, Instant::now()) {
            break Some(TerminalState::Failed);
        }
        let stall_at = last_activity
            .checked_add(inactivity)
            .unwrap_or(deadline)
            .min(deadline);
        line.clear();
        tokio::select! {
            biased;
            () = shutdown.wait() => break None,
            () = control.wait() => break Some(TerminalState::Cancelled),
            () = tokio::time::sleep_until(deadline) => break None,
            () = tokio::time::sleep_until(stall_at) => {
                if has_stalled(active.is_some(), last_activity, inactivity, Instant::now()) {
                    break Some(TerminalState::Failed);
                }
            }
            read = reader.read_line(&mut line) => match read {
                Ok(0) => break Some(TerminalState::Interrupted),
                Ok(_) => {
                    last_activity = Instant::now();
                    sequence += 1;
                    let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                    match parse_frame(&trimmed, sequence) {
                        Ok(event) => {
                            if let Some(state) = apply_event(
                                event, &run, &mut tracker, &mut active, &sender, sequence, None,
                            )
                            .await
                            {
                                break Some(state);
                            }
                        }
                        Err(_) => continue,
                    }
                }
                Err(_) => break None,
            },
        }
    };
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    let mut deltas = Vec::new();
    while let Ok(observation) = receiver.try_recv() {
        if let EngineObservation::TextDelta(delta) = observation {
            deltas.push(delta.delta().to_owned());
        }
    }
    FixtureOutcome {
        terminal,
        deltas,
        tracker,
        thread_id: Some(thread_id),
    }
}

fn joined(outcome: &FixtureOutcome) -> String {
    outcome.deltas.join("")
}

#[tokio::test]
async fn fixture_start_deltas_close() {
    let responses = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n{}\n{}\n",
        r#"{"method":"item/agentMessage/delta","params":{"delta":"hello ","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
        r#"{"method":"item/agentMessage/delta","params":{"delta":"world","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
        r#"{"method":"turn/completed","params":{"threadId":"thread-fixture-1","turn":{"id":"turn-1","status":"completed"}}}"#,
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&responses, "", Duration::from_secs(5), None),
    )
    .await
    .expect("fixture finishes");
    assert_eq!(outcome.terminal, Some(TerminalState::Completed));
    assert_eq!(joined(&outcome), "hello world");
    assert_eq!(
        outcome.thread_id.as_deref(),
        Some("thread-fixture-1"),
        "exact native thread identity rules"
    );
    let terminal = terminal_observation(&run_id(), 3, TerminalState::Completed);
    assert_eq!(terminal.state(), TerminalState::Completed);
}

#[tokio::test]
async fn fixture_malformed_frame_rejected_without_killing_turn() {
    let responses = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n{}\n{}\n",
        "this is not json",
        r#"{"method":"item/agentMessage/delta","params":{"delta":"kept","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
        r#"{"method":"turn/completed","params":{"threadId":"thread-fixture-1","turn":{"id":"turn-1","status":"completed"}}}"#,
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&responses, "", Duration::from_secs(5), None),
    )
    .await
    .expect("fixture finishes");
    assert_eq!(outcome.terminal, Some(TerminalState::Completed));
    assert_eq!(joined(&outcome), "kept");
}

#[tokio::test]
async fn fixture_approval_question_steer_shapes_before_close() {
    let responses = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n{}\n{}\n{}\n",
        r#"{"id":20,"method":"item/commandExecution/requestApproval","params":{"itemId":"approval-21","command":"echo hi","reason":"say hi"}}"#,
        r#"{"method":"item/tool/requestUserInput","params":{"itemId":"q-1","threadId":"thread-fixture-1","turnId":"turn-1","questions":[{"id":"q1","question":"Which?"}]}}"#,
        r#"{"method":"item/subAgent/discovered","params":{"agentThreadId":"child-9","parentThreadId":"thread-fixture-1"}}"#,
        r#"{"method":"turn/completed","params":{"threadId":"thread-fixture-1","turn":{"id":"turn-1","status":"completed"}}}"#,
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&responses, "", Duration::from_secs(5), None),
    )
    .await
    .expect("fixture finishes");
    assert_eq!(outcome.terminal, Some(TerminalState::Completed));
    assert_eq!(outcome.tracker.pending_approvals(), 1);
    assert_eq!(outcome.tracker.pending_questions(), 1);
    assert_eq!(outcome.tracker.subagent_count(), 1);
    assert!(outcome.deltas.is_empty(), "child frames never adopt root");
}

#[tokio::test]
async fn fixture_external_kill_reports_interruption() {
    let responses = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n",
        r#"{"method":"item/agentMessage/delta","params":{"delta":"prefix ","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&responses, "", Duration::from_secs(5), None),
    )
    .await
    .expect("fixture finishes");
    assert_eq!(outcome.terminal, Some(TerminalState::Interrupted));
    assert_eq!(joined(&outcome), "prefix ");
}

#[tokio::test]
async fn fixture_inactivity_stall_fails_turn() {
    // One delta puts the turn in flight then the script goes silent: the
    // inactivity deadline settles the turn as stalled (failed).
    let responses = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n",
        r#"{"method":"item/agentMessage/delta","params":{"delta":"warming","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
    );
    #[cfg(windows)]
    let tail = "ping -n 6 127.0.0.1 >nul";
    #[cfg(not(windows))]
    let tail = "sleep 5";
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&responses, tail, Duration::from_millis(400), None),
    )
    .await
    .expect("fixture finishes");
    assert_eq!(outcome.terminal, Some(TerminalState::Failed));
    assert_eq!(joined(&outcome), "warming");
}

#[tokio::test]
async fn fixture_cancel_reports_cancellation() {
    let responses = format!("{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n");
    #[cfg(windows)]
    let tail = "ping -n 6 127.0.0.1 >nul";
    #[cfg(not(windows))]
    let tail = "sleep 5";
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(
            &responses,
            tail,
            Duration::from_secs(30),
            Some(Duration::from_millis(300)),
        ),
    )
    .await
    .expect("fixture finishes");
    assert_eq!(outcome.terminal, Some(TerminalState::Cancelled));
}

#[tokio::test]
async fn fixture_restart_replays_durable_prefix() {
    let first = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n",
        r#"{"method":"item/agentMessage/delta","params":{"delta":"durable-","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
    );
    let interrupted = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&first, "", Duration::from_secs(5), None),
    )
    .await
    .expect("first attempt finishes");
    assert_eq!(interrupted.terminal, Some(TerminalState::Interrupted));
    let durable_prefix = joined(&interrupted);
    assert_eq!(durable_prefix, "durable-");

    let completed = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&second, "", Duration::from_secs(5), None),
    )
    .await
    .expect("restart finishes");
    assert_eq!(completed.terminal, Some(TerminalState::Completed));
    assert_eq!(
        format!("{durable_prefix}{}", joined(&completed)),
        "durable-replayed"
    );
}

#[test]
fn exit_classification_keeps_cancel_and_interruption_distinct() {
    let program = if cfg!(windows) { "cmd" } else { "sh" };
    let success = std::process::Command::new(program)
        .args(if cfg!(windows) {
            vec!["/C", "exit 0"]
        } else {
            vec!["-c", "exit 0"]
        })
        .status()
        .expect("exit probe runs");
    let failure = std::process::Command::new(program)
        .args(if cfg!(windows) {
            vec!["/C", "exit 3"]
        } else {
            vec!["-c", "exit 3"]
        })
        .status()
        .expect("exit probe runs");
    assert_eq!(classify_exit(success), TerminalState::Completed);
    assert_eq!(classify_exit(failure), TerminalState::Failed);
}

#[test]
fn duplex_write_shapes_jsonl_framing() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let (mut client, server) = tokio::io::duplex(65_536);
        let mut server = BufReader::new(server);
        write_line(&mut client, r#"{"id":1}"#).await.expect("write");
        drop(client);
        let mut text = String::new();
        server.read_to_string(&mut text).await.expect("read");
        assert_eq!(text, "{\"id\":1}\n");
    });
}

// ---------------------------------------------------------------------------
// Continuation compat: old 3-key and new 4-key binding rows both decode
// ---------------------------------------------------------------------------

fn continuation_config_blob() -> Vec<u8> {
    r#"{"version":1,"engine":"opencode2","profile_id":"profile-fixture","model_id":"model-fixture","route_id":"route-fixture","variant_id":null,"permission":{"permission_id":"permission-fixture","agent_id":"agent-fixture","approval":"on_request","filesystem":"workspace","network":"enabled","web_search":"disabled"},"runtime":{"attempt_budget_ms":100,"readiness_budget_ms":1,"health_budget_ms":1,"prompt_budget_ms":1,"stream_budget_ms":1,"close_budget_ms":1,"max_json_body_bytes":8192,"max_sse_line_bytes":4096,"max_sse_event_bytes":8192,"max_readiness_line_bytes":4096,"max_header_count":8,"max_http_buffer_bytes":8192,"max_stderr_bytes":4096,"observation_capacity":16}}"#.as_bytes().to_vec()
}

async fn seed_binding_run(
    database: &sea_orm::DatabaseConnection,
    run_id: &str,
    created_at_ms: i64,
    binding_json: &str,
) {
    use artisan_database::entities::{self, AssistantRunLifecycle, EntityLifecycle, OrdinalKind};
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};

    // The bound run references its origin message and turn, which reference
    // their ordinal row: seed the whole parent chain first.
    let ordinal = created_at_ms;
    entities::message::ActiveModel {
        message_id: Set(format!("message-{run_id}")),
        thread_id: Set("thread-codex-binding".to_owned()),
        ordinal: Set(ordinal),
        body: Set(format!("message for {run_id}")),
        accepted_at_ms: Set(created_at_ms),
    }
    .insert(database)
    .await
    .expect("message should insert");
    entities::conversation_ordinal::ActiveModel {
        thread_id: Set("thread-codex-binding".to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Turn),
        entity_id: Set(format!("turn-{run_id}")),
    }
    .insert(database)
    .await
    .expect("turn ordinal should insert");
    entities::conversation_turn::ActiveModel {
        turn_id: Set(format!("turn-{run_id}")),
        thread_id: Set("thread-codex-binding".to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Turn),
        revision: Set(0),
        lifecycle: Set(EntityLifecycle::Completed),
        created_at_ms: Set(created_at_ms),
        updated_at_ms: Set(created_at_ms + 10),
    }
    .insert(database)
    .await
    .expect("turn should insert");
    entities::assistant_run::ActiveModel {
        run_id: Set(run_id.to_owned()),
        thread_id: Set("thread-codex-binding".to_owned()),
        run_start_key: Set(artisan_database::entities::OpaqueBytes::new({
            // Distinct per seeded run: the column is unique, so the shared
            // fixture constant would collide on the second insert.
            let mut key = [created_at_ms as u8; 32];
            for (index, byte) in run_id.bytes().enumerate() {
                let slot = index % key.len();
                key[slot] = key[slot].wrapping_add(byte);
            }
            key.to_vec()
        })),
        origin_message_id: Set(format!("message-{run_id}")),
        origin_turn_id: Set(format!("turn-{run_id}")),
        lifecycle: Set(AssistantRunLifecycle::Completed),
        generation: Set(1),
        owner: Set(None),
        lease: Set(None),
        claim_token: Set(None),
        provider_binding_version: Set(Some(1)),
        provider_binding: Set(Some(artisan_database::entities::OpaqueBytes::new(
            binding_json.as_bytes().to_vec(),
        ))),
        provider_bound_at_ms: Set(Some(created_at_ms + 2)),
        error_code: Set(None),
        error_message: Set(None),
        created_at_ms: Set(created_at_ms),
        updated_at_ms: Set(created_at_ms + 10),
        terminal_at_ms: Set(Some(created_at_ms + 10)),
        engine_run_config_version: Set(Some(1)),
        engine_run_config_revision: Set(Some(1)),
        engine_run_config: Set(Some(artisan_database::entities::OpaqueBytes::new(
            continuation_config_blob(),
        ))),
    }
    .insert(database)
    .await
    .expect("bound run should insert");
}

#[tokio::test]
async fn continuation_decodes_old_and_new_binding_rows() {
    use artisan_database::{
        Repository, SessionContinuationLookup, SessionContinuationQuery, SqliteConfig, connect,
    };
    use artisan_migrations::migrate_to_current;

    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database)
        .await
        .expect("memory database should migrate");
    {
        use artisan_database::entities;
        use sea_orm::{ActiveModelTrait, ActiveValue::Set};
        entities::attached_project::ActiveModel {
            project_id: Set("project-codex-binding".to_owned()),
            root_path: Set("C:/repos/artisan".to_owned()),
            display_name: Set("Binding compat".to_owned()),
            attached_at_ms: Set(1),
        }
        .insert(&database)
        .await
        .expect("project should insert");
        entities::thread::ActiveModel {
            thread_id: Set("thread-codex-binding".to_owned()),
            project_id: Set("project-codex-binding".to_owned()),
            title: Set("Binding compat".to_owned()),
            created_at_ms: Set(10),
            updated_at_ms: Set(10),
            engine_run_config_version: Set(Some(1)),
            engine_run_config_revision: Set(1),
            engine_run_config: Set(Some(artisan_database::entities::OpaqueBytes::new(
                continuation_config_blob(),
            ))),
        }
        .insert(&database)
        .await
        .expect("thread should insert");
    }
    // Old row: bound before the format key existed.
    seed_binding_run(
        &database,
        "run-binding-old",
        100,
        r#"{"engine":"opencode2","profile_id":"profile-fixture","session_id":"session-old"}"#,
    )
    .await;
    // New row: bound with the format key.
    seed_binding_run(
        &database,
        "run-binding-new",
        200,
        r#"{"engine":"opencode2","format":1,"profile_id":"profile-fixture","session_id":"session-new"}"#,
    )
    .await;

    let repository = Repository::new(database);
    let query = |exclude: Option<&str>| SessionContinuationQuery {
        thread_id: ThreadId::parse("thread-codex-binding").expect("thread id"),
        engine_id: EngineId::OpenCode2,
        profile_id: EngineProfileId::parse("profile-fixture").expect("profile id"),
        exclude_run_id: exclude.map(|value| RunId::parse(value).expect("run id")),
    };
    let SessionContinuationLookup::Usable(new) = repository
        .read_session_continuation(query(None))
        .await
        .expect("continuation read should succeed")
    else {
        panic!("new 4-key binding row should be usable");
    };
    assert_eq!(new.session_id.as_str(), "session-new");
    let SessionContinuationLookup::Usable(old) = repository
        .read_session_continuation(query(Some("run-binding-new")))
        .await
        .expect("continuation read should succeed")
    else {
        panic!("old 3-key binding row should stay usable");
    };
    assert_eq!(old.session_id.as_str(), "session-old");
}

// ---------------------------------------------------------------------------
// X3: continuation gate matrix (same engine, explicit model, CLI >= 0.145.0)
// ---------------------------------------------------------------------------

fn gate_decision(
    cli_version: &str,
    target_model: Option<&str>,
    advertised_models: Option<&[&str]>,
    same_engine: bool,
) -> CodexContinuationDecision {
    check_codex_native_continuation(&CodexContinuationGateInput {
        cli_version,
        target_model,
        advertised_models,
        same_engine,
    })
}

#[test]
fn codex_continuation_gate_matrix() {
    assert_eq!(
        gate_decision("0.145.0", Some("codex-model"), None, true),
        CodexContinuationDecision::Compatible
    );
    assert_eq!(
        gate_decision("0.146.2", Some("codex-model"), None, true),
        CodexContinuationDecision::Compatible
    );
    assert_eq!(
        gate_decision(
            "0.145.0",
            Some("codex-model"),
            Some(&["codex-model", "other-model"]),
            true
        ),
        CodexContinuationDecision::Compatible
    );
    // Explicit target model is required before resume.
    assert!(matches!(
        gate_decision("0.145.0", None, None, true),
        CodexContinuationDecision::Incompatible { .. }
    ));
    assert!(matches!(
        gate_decision("0.145.0", Some(""), None, true),
        CodexContinuationDecision::Incompatible { .. }
    ));
    // The continuation floor is newer than the transport floor.
    for old in ["0.142.5", "0.144.9", "not a version", ""] {
        assert!(
            matches!(
                gate_decision(old, Some("codex-model"), None, true),
                CodexContinuationDecision::Incompatible { .. }
            ),
            "CLI {old} must not authorize continuation"
        );
    }
    // Cross-engine resume never proceeds, even with a fresh CLI and model.
    assert!(matches!(
        gate_decision("0.145.0", Some("codex-model"), None, false),
        CodexContinuationDecision::Incompatible { .. }
    ));
    // Advertisement is enforced only when an inventory is supplied.
    assert!(matches!(
        gate_decision("0.145.0", Some("codex-model"), Some(&["other-model"]), true),
        CodexContinuationDecision::Incompatible { .. }
    ));
}

#[test]
fn codex_cli_version_floor_parses_embedded_triples() {
    assert!(codex_cli_meets_minimum("codex-cli 0.145.0", "0.145.0"));
    assert!(codex_cli_meets_minimum("0.145.0-alpha", "0.145.0"));
    assert!(codex_cli_meets_minimum("0.146.0", "0.145.0"));
    assert!(!codex_cli_meets_minimum("codex-cli 0.144.9", "0.145.0"));
    assert!(!codex_cli_meets_minimum("no version here", "0.145.0"));
    assert!(!codex_cli_meets_minimum("", "0.145.0"));
}

// ---------------------------------------------------------------------------
// X3: resume reopens the same thread id over start options
// ---------------------------------------------------------------------------

#[test]
fn codex_resume_reopens_the_same_thread_id() {
    let line = r#"{"id":2,"result":{"thread":{"id":"thread-fixture-1"}}}"#;
    assert_eq!(
        codex_resumed_thread_id(line, 2, "thread-fixture-1").as_deref(),
        Some("thread-fixture-1")
    );
    // A foreign thread is never adopted.
    assert_eq!(codex_resumed_thread_id(line, 2, "thread-other"), None);
    // Id mismatch fails closed.
    assert_eq!(codex_resumed_thread_id(line, 3, "thread-fixture-1"), None);

    let settings = CodexSettings::from_selection(&codex_selection()).expect("settings valid");
    let root_path = std::env::temp_dir().join("codex-fixture-resume");
    let root = RootPath::parse(root_path.to_str().expect("temp path utf8")).expect("root");
    let params = thread_resume_params(&settings, &root, "thread-fixture-1").expect("resume params");
    assert_eq!(params["threadId"], "thread-fixture-1");
    assert_eq!(params["approvalPolicy"], "on-request");
    assert_eq!(params["sandbox"], "workspace-write");
    assert!(thread_resume_params(&settings, &root, "").is_none());
    assert!(thread_resume_params(&settings, &root, &"t".repeat(257)).is_none());
}

// ---------------------------------------------------------------------------
// X3: token-usage basis rules (cumulative, gauge never additive)
// ---------------------------------------------------------------------------

fn token_usage_params(payload: &str) -> serde_json::Value {
    serde_json::from_str(payload).expect("usage params json")
}

#[test]
fn token_usage_frames_decode_to_a_cumulative_sample() {
    let event = parse_frame(
        r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"t-1","turnId":"turn-1","tokenUsage":{"total":{"inputTokens":40,"outputTokens":9,"cachedInputTokens":12},"last":{"totalTokens":41},"modelContextWindow":200000}}}"#,
        1,
    )
    .expect("usage decodes");
    let CodexEvent::TokenUsage { turn_id, sample } = event else {
        panic!("expected token usage");
    };
    assert_eq!(turn_id, "turn-1");
    assert_eq!(sample.input, Some(40));
    assert_eq!(sample.output, Some(9));
    assert_eq!(sample.cached_input, Some(12));
    assert_eq!(sample.context, Some(41));
    assert_eq!(sample.context_window, Some(200000));

    // An empty measurement stays observable without a report.
    let empty = parse_frame(
        r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"t-1","turnId":"turn-1","tokenUsage":{}}}"#,
        2,
    )
    .expect("empty usage decodes");
    assert!(matches!(empty, CodexEvent::UnknownMethod));

    // Non-u64 numerics fail closed to absent instead of poisoning the turn.
    let negative = parse_frame(
        r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"t-1","turnId":"turn-1","tokenUsage":{"total":{"inputTokens":-1}}}}"#,
        3,
    )
    .expect("negative usage decodes");
    assert!(matches!(negative, CodexEvent::UnknownMethod));

    // Missing turn scope fails closed.
    let unscoped = parse_frame(
        r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"t-1","tokenUsage":{"total":{"inputTokens":1}}}}"#,
        4,
    )
    .expect("unscoped usage decodes");
    assert!(matches!(unscoped, CodexEvent::UnknownMethod));
}

#[test]
fn codex_usage_report_is_cumulative_with_a_replacing_gauge() {
    let run = run_id();
    let thread = ThreadId::parse("thread-usage-1").expect("thread id");
    let model = EngineModelId::parse("model-usage-1").expect("model id");
    let context = CodexUsageContext {
        run_id: &run,
        thread_id: &thread,
        provider_session_id: "thread-fixture-1",
        model_id: &model,
        observed_at: UnixMillis::from_millis(7),
    };
    let params = token_usage_params(
        r#"{"threadId":"t-1","turnId":"turn-1","tokenUsage":{"total":{"inputTokens":40,"outputTokens":9,"cachedInputTokens":12},"last":{"totalTokens":41},"modelContextWindow":200000}}"#,
    );
    let sample = parse_thread_token_usage(&params).expect("sample");
    let report =
        codex_usage_report(&context, Some("turn-1".to_owned()), 3, &sample).expect("report builds");
    assert_eq!(report.basis(), RunUsageBasis::Cumulative);
    assert_eq!(report.provider_session_id(), "thread-fixture-1");
    assert_eq!(report.provider_turn_id(), Some("turn-1"));
    assert_eq!(report.source_sequence(), 3);
    assert_eq!(report.input_tokens(), Some(40));
    assert_eq!(report.output_tokens(), Some(9));
    assert_eq!(report.cached_input_tokens(), Some(12));
    // The window gauge is the last request only, never the running total.
    assert_eq!(report.context_tokens(), Some(41));
    assert_eq!(report.context_window_tokens(), Some(200000));

    // Absent gauge stays absent rather than becoming a wrong zero.
    let no_gauge = token_usage_params(
        r#"{"threadId":"t-1","turnId":"turn-1","tokenUsage":{"total":{"inputTokens":40}}}"#,
    );
    let sample = parse_thread_token_usage(&no_gauge).expect("partial sample");
    let report = codex_usage_report(&context, None, 4, &sample).expect("partial report");
    assert_eq!(report.context_tokens(), None);
    assert_eq!(report.provider_turn_id(), None);

    // Zero is preserved and distinct from absent.
    let zero = token_usage_params(
        r#"{"threadId":"t-1","turnId":"turn-1","tokenUsage":{"total":{"inputTokens":0,"outputTokens":0}}}"#,
    );
    let sample = parse_thread_token_usage(&zero).expect("zero sample");
    let report = codex_usage_report(&context, None, 5, &sample).expect("zero report");
    assert_eq!(report.input_tokens(), Some(0));
    assert_eq!(report.output_tokens(), Some(0));

    // Empty measurements are never reports.
    assert!(parse_thread_token_usage(&token_usage_params(r#"{"tokenUsage":{}}"#)).is_none());
}

#[tokio::test]
async fn token_usage_projects_a_usage_observation_without_blocking_the_turn() {
    let event = parse_frame(
        r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"thread-fixture-1","turnId":"turn-9","tokenUsage":{"total":{"inputTokens":40,"outputTokens":9},"last":{"totalTokens":41}}}}"#,
        9,
    )
    .expect("usage decodes");
    let run = run_id();
    let attribution = CodexUsageAttribution {
        thread_id: ThreadId::parse("thread-usage-1").expect("thread id"),
        model_id: EngineModelId::parse("model-usage-1").expect("model id"),
    };
    let scope = CodexUsageScope {
        thread_id: &attribution.thread_id,
        model_id: &attribution.model_id,
        provider_session_id: "thread-fixture-1",
    };
    let (sender, mut receiver) = mpsc::channel(8);
    let mut tracker = CodexPendingTracker::new();
    let mut active = None;
    let terminal = apply_event(
        event,
        &run,
        &mut tracker,
        &mut active,
        &sender,
        9,
        Some(&scope),
    )
    .await;
    assert_eq!(terminal, None, "usage never settles the turn");
    let EngineObservation::Usage(observation) = receiver.try_recv().expect("usage observed") else {
        panic!("expected a usage observation");
    };
    assert_eq!(observation.report().basis(), RunUsageBasis::Cumulative);
    assert_eq!(observation.report().context_tokens(), Some(41));
    assert_eq!(observation.report().source_sequence(), 9);

    // Without attribution the same frame is a diagnostic: no observation,
    // no terminal, and the turn continues.
    let event = parse_frame(
        r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"thread-fixture-1","turnId":"turn-9","tokenUsage":{"total":{"inputTokens":40}}}}"#,
        10,
    )
    .expect("usage decodes");
    let (sender, mut receiver) = mpsc::channel(8);
    let terminal = apply_event(event, &run, &mut tracker, &mut active, &sender, 10, None).await;
    assert_eq!(terminal, None);
    assert!(receiver.try_recv().is_err(), "no usage without scope");
}

// ---------------------------------------------------------------------------
// X3: rate-limit bucket mapping (clamp, kinds, resets) plus request lines
// ---------------------------------------------------------------------------

#[test]
fn codex_rate_limit_buckets_map_with_clamp_and_kinds() {
    let result: serde_json::Value = serde_json::from_str(
        r#"{
            "rateLimitsByLimitId": {
                "codex": {"limitId":"codex","limitName":null,"primary":{"usedPercent":120,"resetsAt":0,"windowDurationMins":300},"secondary":null},
                "gpt-5": {"limitId":"gpt-5","limitName":"GPT-5","primary":{"usedPercent":-5,"resetsAt":null,"windowDurationMins":10080},"secondary":{"usedPercent":50,"windowDurationMins":43200}}
            }
        }"#,
    )
    .expect("rate limits json");
    let windows = map_codex_rate_limit_windows(&result);
    assert_eq!(windows.len(), 3);

    assert_eq!(windows[0].id, "codex:primary");
    assert_eq!(windows[0].kind, CodexQuotaWindowKind::Session);
    assert!(
        (windows[0].percent_used - 100.0).abs() < 1e-9,
        "over-full gauge clamps to 100"
    );
    assert_eq!(
        windows[0].resets_at.as_deref(),
        Some("1970-01-01T00:00:00Z")
    );
    assert_eq!(windows[0].window_minutes, Some(300));
    assert_eq!(windows[0].scope, "unknown");

    assert_eq!(windows[1].id, "gpt-5:primary");
    assert_eq!(windows[1].kind, CodexQuotaWindowKind::Weekly);
    assert!(
        (windows[1].percent_used - 0.0).abs() < 1e-9,
        "negative gauge clamps to 0"
    );
    assert_eq!(windows[1].resets_at, None);
    assert_eq!(windows[1].scope, "model");

    assert_eq!(windows[2].id, "gpt-5:secondary");
    assert_eq!(windows[2].kind, CodexQuotaWindowKind::Monthly);
    assert!(
        (windows[2].percent_used - 50.0).abs() < 1e-9,
        "in-range gauge passes through"
    );
    assert_eq!(windows[2].scope, "model");

    // Unknown kinds are never guessed, and the single-snapshot fallback
    // keeps the codex bucket identity.
    assert_eq!(
        classify_codex_quota_window_kind(None),
        CodexQuotaWindowKind::Unknown
    );
    assert_eq!(
        classify_codex_quota_window_kind(Some(999)),
        CodexQuotaWindowKind::Unknown
    );
    assert_eq!(
        classify_codex_quota_window_kind(Some(300)),
        CodexQuotaWindowKind::Session
    );
    assert_eq!(
        classify_codex_quota_window_kind(Some(10_080)),
        CodexQuotaWindowKind::Weekly
    );
    assert_eq!(
        classify_codex_quota_window_kind(Some(43_200)),
        CodexQuotaWindowKind::Monthly
    );
    assert!(
        clamp_codex_percent_used(None).abs() < 1e-9,
        "absent gauge becomes 0"
    );
    assert!(
        (clamp_codex_percent_used(Some(33.5)) - 33.5).abs() < 1e-9,
        "in-range gauge passes through"
    );

    let single: serde_json::Value = serde_json::from_str(
        r#"{"rateLimits":{"primary":{"usedPercent":10,"windowDurationMins":300}}}"#,
    )
    .expect("single snapshot json");
    let windows = map_codex_rate_limit_windows(&single);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].id, "codex:primary");

    let malformed: serde_json::Value =
        serde_json::from_str(r#"{"rateLimitsByLimitId":{"broken":42}}"#).expect("malformed json");
    assert!(map_codex_rate_limit_windows(&malformed).is_empty());

    assert_eq!(codex_reset_at_iso(0), "1970-01-01T00:00:00Z");
    assert!(codex_account_read_line(7).contains("\"method\":\"account/read\""));
    assert!(codex_rate_limits_read_line(8).contains("\"method\":\"account/rateLimits/read\""));
}

// ---------------------------------------------------------------------------
// X3: teardown kills the whole group; quarantine stays on unobserved reaps
// ---------------------------------------------------------------------------

#[test]
fn codex_teardown_requires_group_termination() {
    assert!(
        codex_requires_group_termination(),
        "Windows teardown must kill the whole Job Object so no codex grandchild \
         holding a pipe is orphaned; unobserved reaps quarantine through \
         cleanup_after_abort and finish_turn_result"
    );
}

#[tokio::test]
async fn fixture_kill_reports_interruption_with_durable_prefix() {
    let responses = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n",
        r#"{"method":"item/agentMessage/delta","params":{"delta":"prefix-","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
    );
    // Pipe-holding grandchild: the leader is killed below while this holder
    // still inherits stdout, so EOF (and the interruption) must arrive
    // bounded without an orphan wedging the pump.
    #[cfg(windows)]
    let tail = "ping -n 4 127.0.0.1 >nul";
    #[cfg(not(windows))]
    let tail = "sleep 3";
    let script = FixtureScript::new(&responses, tail);
    let mut child = script.spawn();
    let stdout = child.stdout.take().expect("fixture stdout");
    drop(child.stdin.take());
    let mut reader = BufReader::new(stdout);

    let run = run_id();
    let (sender, mut receiver) = mpsc::channel(64);
    let mut tracker = CodexPendingTracker::new();
    let mut active: Option<String> = None;
    let mut sequence: u64 = 0;
    let mut line = String::new();
    let terminal = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break Some(TerminalState::Interrupted),
                Ok(_) => {
                    sequence += 1;
                    // Kill the leader once the durable prefix has landed:
                    // INIT, THREAD, and TURN results precede the first
                    // notification, so the prefix delta is sequence 4.
                    if sequence == 4 {
                        let _ = child.kill().await;
                    }
                    let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                    match parse_frame(&trimmed, sequence) {
                        Ok(event) => {
                            if let Some(state) = apply_event(
                                event,
                                &run,
                                &mut tracker,
                                &mut active,
                                &sender,
                                sequence,
                                None,
                            )
                            .await
                            {
                                break Some(state);
                            }
                        }
                        Err(_) => continue,
                    }
                }
                Err(_) => break None,
            }
        }
    })
    .await
    .expect("kill fixture finishes bounded");
    let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    assert_eq!(terminal, Some(TerminalState::Interrupted));
    let mut deltas = Vec::new();
    while let Ok(observation) = receiver.try_recv() {
        if let EngineObservation::TextDelta(delta) = observation {
            deltas.push(delta.delta().to_owned());
        }
    }
    assert_eq!(deltas.join(""), "prefix-");
}

#[tokio::test]
async fn fixture_restart_after_kill_replays_prefix_on_the_same_thread() {
    let first = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n",
        r#"{"method":"item/agentMessage/delta","params":{"delta":"durable-","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
    );
    let interrupted = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&first, "", Duration::from_secs(5), None),
    )
    .await
    .expect("first attempt finishes");
    assert_eq!(interrupted.terminal, Some(TerminalState::Interrupted));
    let durable_prefix = joined(&interrupted);
    assert_eq!(durable_prefix, "durable-");

    // The restart resumes provider-owned state: the same thread id reopens
    // through thread/resume instead of duplicating provider effects with a
    // second thread.
    let resume_line = r#"{"id":2,"result":{"thread":{"id":"thread-fixture-1"}}}"#;
    assert_eq!(
        codex_resumed_thread_id(resume_line, 2, "thread-fixture-1").as_deref(),
        Some("thread-fixture-1")
    );

    let completed = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&second, "", Duration::from_secs(5), None),
    )
    .await
    .expect("restart finishes");
    assert_eq!(completed.terminal, Some(TerminalState::Completed));
    assert_eq!(
        format!("{durable_prefix}{}", joined(&completed)),
        "durable-replayed"
    );
}

// ---------------------------------------------------------------------------
// Production-executor wire proofs (real `execute_codex_turn`, no mocks)
// ---------------------------------------------------------------------------
//
// The canned-stdout harness above bypasses the owner executor by design. The
// tests below admit real `EngineCodexTurnInput` turns into a live
// `EngineOwner` whose verified launch points at the `codex_wire_fixture`
// executable, so the production handshake, threadId binding, reply
// correlation, and streaming pump all execute. The parent never mutates
// global environment: each test copies the built fixture to a per-test
// executable whose basename names the scenario, and the fixture records the
// received `turn/start` params inside the spawned project-root cwd.

/// Scratch project root plus database path for one wire turn.
struct WireTempRoot {
    dir: PathBuf,
    root: RootPath,
    db_path: PathBuf,
}

impl WireTempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "artisan-codex-wire-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("wire temp root");
        let root = RootPath::parse(dir.to_str().expect("temp path utf8")).expect("root");
        let db_path = dir.join("wire.sqlite");
        Self { dir, root, db_path }
    }
}

impl Drop for WireTempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Resolves the built wire-fixture executable without touching global state.
fn codex_wire_fixture_program() -> PathBuf {
    if let Ok(path) = std::env::var("ARTISAN_CODEX_WIRE_FIXTURE") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "declared wire fixture must be a regular file"
        );
        return path;
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codex_wire_fixture") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return path;
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for candidate in [
        manifest.join("../../target/debug/codex_wire_fixture"),
        manifest.join("../../target/debug/codex_wire_fixture.exe"),
    ] {
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("wire fixture binary not found; build the codex-wire-fixture example");
}

/// Copies the built fixture to a per-test executable whose basename names
/// the scenario (`strict`, `reject_always`, or `interleave`).
fn codex_wire_scenario_program(fixture: &PathBuf, dir: &PathBuf, scenario: &str) -> PathBuf {
    let named = dir.join(format!(
        "codex-wire-{scenario}{}",
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::copy(fixture, &named).expect("wire fixture copies per test");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(&named).expect("meta").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&named, permissions).expect("chmod");
    }
    named
}

fn codex_wire_selection(profile_id: &str, fast: bool) -> CodexSelection {
    CodexSelection::new(
        EngineProfileId::parse(profile_id).expect("profile id"),
        Some(EngineModelId::parse("codex-model").expect("model id")),
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        Some(CodexReasoningEffort::High),
        if fast {
            Some(CodexServiceTier::Fast)
        } else {
            None
        },
        Some(CodexModelContextWindow::new(1_000).expect("window")),
    )
    .expect("codex wire selection valid")
}

/// Read-only live-probe selection: the exact requested model at medium
/// effort, no service tier, no context-window override, never approve, no
/// filesystem, no network. The probe performs no tools and reads no files;
/// its prompt is text-only.
fn codex_live_selection(profile_id: &str) -> CodexSelection {
    CodexSelection::new(
        EngineProfileId::parse(profile_id).expect("profile id"),
        Some(EngineModelId::parse("gpt-5.6-luna").expect("model id")),
        permission(
            ApprovalMode::Never,
            FilesystemAccess::None,
            NetworkAccess::Disabled,
        ),
        Some(CodexReasoningEffort::Medium),
        None,
        None,
    )
    .expect("codex live selection valid")
}

fn codex_wire_runtime() -> EngineRuntimeControls {
    let budget = |ms: u64| FiniteMillis::new(ms).expect("finite millis valid");
    EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: budget(45_000),
        readiness_budget: budget(5_000),
        health_budget: budget(5_000),
        prompt_budget: budget(15_000),
        stream_budget: budget(25_000),
        close_budget: budget(5_000),
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness limit"),
        max_header_count: CountLimit::new(32).expect("header count"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr"),
        observation_capacity: CountLimit::new(16).expect("observation cap"),
    })
    .expect("runtime valid")
}

async fn codex_wire_settings(
    selection: CodexSelection,
    root: &RootPath,
    thread_id: &ThreadId,
) -> artisan_database::ThreadEngineSettings {
    let config = EngineRunConfig::new(
        EngineSelection::Codex(selection),
        codex_wire_runtime(),
    );
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
    let now = UnixMillis::from_millis(1);
    repo.attach_project(AttachProjectInput {
        request_id: RequestId::parse("req-wire-attach").expect("request id"),
        directory_id: DirectoryId::parse("dir-wire").expect("directory id"),
        project_id: ProjectId::parse("proj-wire").expect("project id"),
        root_path: root.clone(),
        display_name: DisplayName::parse("wire-proj").expect("display"),
        attached_at: now,
    })
    .await
    .expect("attach project");
    repo.create_thread(CreateThreadInput {
        request_id: RequestId::parse("req-wire-thread").expect("request id"),
        thread_id: thread_id.clone(),
        project_id: ProjectId::parse("proj-wire").expect("project id"),
        title: ThreadTitle::parse("wire-thread").expect("title"),
        created_at: now,
        updated_at: now,
    })
    .await
    .expect("create thread");
    repo.set_thread_engine_config(SetThreadEngineConfigInput {
        request_id: RequestId::parse("req-wire-config").expect("request id"),
        thread_id: thread_id.clone(),
        precondition: EngineConfigUpdatePrecondition::Unconfigured,
        config: config.clone(),
        accepted_at: now,
    })
    .await
    .expect("set config");
    repo.read_thread_engine_settings(thread_id)
        .await
        .expect("read settings")
        .expect("settings present")
}

struct WireTurnOutcome {
    session: String,
    text: String,
    terminal: TerminalState,
}

/// Prepares, authorizes once, and drains one live owner turn to its terminal
/// observation, proving the production handshake and pump end to end.
async fn drive_codex_wire_turn(turn: &mut AcceptedTurn) -> WireTurnOutcome {
    let prepared = turn.prepare().await.expect("wire turn prepares");
    let session = prepared.session().to_owned();
    turn.authorize().expect("wire turn authorizes once");
    let mut text = String::new();
    let terminal = loop {
        let observation = tokio::time::timeout(Duration::from_secs(20), turn.next_observation())
            .await
            .expect("wire observation arrives")
            .expect("observation stream stays open until terminal");
        match observation {
            EngineObservation::TextDelta(delta) => text.push_str(delta.delta()),
            EngineObservation::Usage(_) => {}
            EngineObservation::Terminal(terminal) => break terminal.state(),
            EngineObservation::TextSnapshot(_)
            | EngineObservation::Subagent(_)
            | EngineObservation::SubagentTranscript(_) => {
                panic!("unexpected wire observation")
            }
        }
    };
    let result = turn.finish().await.expect("wire turn finishes");
    assert_eq!(result.terminal(), terminal);
    WireTurnOutcome {
        session,
        text,
        terminal,
    }
}

async fn admit_codex_wire_turn(
    owner: &EngineOwner,
    settings: artisan_database::ThreadEngineSettings,
    launch: artisan_native_engine::VerifiedCodexLaunch,
    thread_id: ThreadId,
    run_id: RunId,
    root: &RootPath,
    prompt: &str,
) -> AcceptedTurn {
    owner
        .admit_codex_turn(
            EngineCodexTurnInput {
                run_id,
                thread_id,
                project_root: root.clone(),
                prompt_id: "prompt-wire-1".to_owned(),
                prompt: QueueMessagePayload::text_only(prompt).expect("payload"),
                settings,
                launch,
                continuation: None,
                prompt_delivery: "immediate".to_owned(),
                stream_after: 0,
                control_capacity: 1,
            },
            Duration::from_secs(50),
        )
        .expect("wire turn admits")
}

#[tokio::test]
async fn codex_wire_owner_accepts_bound_turn_with_text_and_completion() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let fixture = codex_wire_fixture_program();
        let temp = WireTempRoot::new("accept");
        let program = codex_wire_scenario_program(&fixture, &temp.dir, "strict");
        let thread_id = ThreadId::parse("thread-wire-accept").expect("thread id");
        let settings = codex_wire_settings(codex_wire_selection("codex-fixture", true), &temp.root, &thread_id).await;
        let launch = NativeCodexAuthority::new()
            .resolve_launch_with_executable(
                &temp.db_path,
                &EngineProfileId::parse("codex-fixture").expect("profile id"),
                &program,
                "codex-cli 0.145.0",
            )
            .expect("wire launch resolves");
        let mut owner = EngineOwner::start_configured(
            NonZeroUsize::new(1).expect("one slot"),
            &tokio::runtime::Handle::current(),
        );
        let mut turn = admit_codex_wire_turn(
            &owner,
            settings,
            launch,
            thread_id.clone(),
            RunId::parse("wire-run-accept").expect("run id"),
            &temp.root,
            "hello wire",
        )
        .await;
        let started = std::time::Instant::now();
        let wire = drive_codex_wire_turn(&mut turn).await;
        assert!(
            started.elapsed() < Duration::from_secs(50),
            "accepted turn settles well inside budget"
        );
        assert_eq!(wire.session, "thread-fixture-1");
        assert_eq!(wire.text, "hello wire");
        assert_eq!(wire.terminal, TerminalState::Completed);
        // The strict fixture accepts only with a bound threadId: the
        // recorded params prove the production payload carried it.
        let recorded: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp.dir.join("turn-start-params.json"))
                .expect("fixture records turn params"),
        )
        .expect("recorded params are valid json");
        assert_eq!(recorded["threadId"], "thread-fixture-1");
        assert_eq!(recorded["input"][0]["text"], "hello wire");
        assert_eq!(recorded["serviceTier"], "fast");
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("wire turn completes inside 60s");
}

#[tokio::test]
async fn codex_wire_owner_fails_fast_on_turn_start_rejection() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let fixture = codex_wire_fixture_program();
        let temp = WireTempRoot::new("reject");
        let program = codex_wire_scenario_program(&fixture, &temp.dir, "reject_always");
        let thread_id = ThreadId::parse("thread-wire-reject").expect("thread id");
        let settings = codex_wire_settings(codex_wire_selection("codex-fixture", true), &temp.root, &thread_id).await;
        let launch = NativeCodexAuthority::new()
            .resolve_launch_with_executable(
                &temp.db_path,
                &EngineProfileId::parse("codex-fixture").expect("profile id"),
                &program,
                "codex-cli 0.145.0",
            )
            .expect("wire launch resolves");
        let mut owner = EngineOwner::start_configured(
            NonZeroUsize::new(1).expect("one slot"),
            &tokio::runtime::Handle::current(),
        );
        let mut turn = admit_codex_wire_turn(
            &owner,
            settings,
            launch,
            thread_id.clone(),
            RunId::parse("wire-run-reject").expect("run id"),
            &temp.root,
            "hello wire",
        )
        .await;
        let started = std::time::Instant::now();
        let prepared = turn.prepare().await;
        // The `-32600` error envelope fails preparation fast (seconds, not
        // lease expiry) with the typed provider failure.
        assert!(
            matches!(prepared, Err(EngineOperationError::ProviderRequestFailed)),
            "rejected turn/start must fail preparation fast"
        );
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "rejection settles fast instead of waiting out the lease"
        );
        drop(turn);
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("rejection settles inside 60s");
}

#[tokio::test]
async fn codex_wire_owner_survives_interleaved_thread_started() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let fixture = codex_wire_fixture_program();
        let temp = WireTempRoot::new("interleave");
        let program = codex_wire_scenario_program(&fixture, &temp.dir, "interleave");
        let thread_id = ThreadId::parse("thread-wire-interleave").expect("thread id");
        let settings = codex_wire_settings(codex_wire_selection("codex-fixture", true), &temp.root, &thread_id).await;
        let launch = NativeCodexAuthority::new()
            .resolve_launch_with_executable(
                &temp.db_path,
                &EngineProfileId::parse("codex-fixture").expect("profile id"),
                &program,
                "codex-cli 0.145.0",
            )
            .expect("wire launch resolves");
        let mut owner = EngineOwner::start_configured(
            NonZeroUsize::new(1).expect("one slot"),
            &tokio::runtime::Handle::current(),
        );
        let mut turn = admit_codex_wire_turn(
            &owner,
            settings,
            launch,
            thread_id.clone(),
            RunId::parse("wire-run-interleave").expect("run id"),
            &temp.root,
            "hello wire",
        )
        .await;
        // The real CLI emits `thread/started` between the `thread/*` result
        // and the `turn/start` result: id correlation (not next-line
        // assumption) still accepts the turn and delivers its text.
        let wire = drive_codex_wire_turn(&mut turn).await;
        assert_eq!(wire.session, "thread-fixture-1");
        assert_eq!(wire.text, "hello wire");
        assert_eq!(wire.terminal, TerminalState::Completed);
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("interleaved turn completes inside 60s");
}

/// Opt-in production acceptance against the installed authenticated CLI.
///
/// Ignored by default: requires the real `codex` binary plus an
/// authenticated account, and performs one tiny read-only turn
/// (`Return SEND_PROBE_OK only. Do not use tools and do not read files.`)
/// in a fresh temp project root. Root runs
/// it explicitly after the build gate; it never touches a live user
/// database. The profile comes from `ARTISAN_CODEX_LIVE_PROFILE_ID` when
/// set, otherwise `codex-live-probe`.
#[tokio::test]
#[ignore = "requires installed authenticated codex CLI"]
async fn codex_live_owner_completes_real_turn_within_budget() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let executable = NativeCodexAuthority::new()
            .resolve_executable()
            .expect("live test requires an installed codex executable");
        let probe = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new(&executable)
                .arg("--version")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output(),
        )
        .await
        .expect("version probe settles")
        .expect("version probe spawns");
        assert!(probe.status.success(), "codex --version must succeed");
        let stdout = String::from_utf8(probe.stdout).expect("version output is utf8");
        let profile_name = std::env::var("ARTISAN_CODEX_LIVE_PROFILE_ID")
            .unwrap_or_else(|_| "codex-live-probe".to_owned());
        let temp = WireTempRoot::new("live");
        let thread_id = ThreadId::parse("thread-live-probe").expect("thread id");
        let settings = codex_wire_settings(codex_live_selection(&profile_name), &temp.root, &thread_id).await;
        let launch = NativeCodexAuthority::new()
            .resolve_launch(
                &temp.db_path,
                &EngineProfileId::parse(profile_name.as_str()).expect("profile id"),
                &stdout,
            )
            .expect("live launch resolves");
        let mut owner = EngineOwner::start_configured(
            NonZeroUsize::new(1).expect("one slot"),
            &tokio::runtime::Handle::current(),
        );
        let mut turn = admit_codex_wire_turn(
            &owner,
            settings,
            launch,
            thread_id.clone(),
            RunId::parse("wire-run-live").expect("run id"),
            &temp.root,
            "Return SEND_PROBE_OK only. Do not use tools and do not read files.",
        )
        .await;
        let wire = drive_codex_wire_turn(&mut turn).await;
        assert_eq!(
            wire.text.trim(),
            "SEND_PROBE_OK",
            "live turn answers with exactly the probe marker"
        );
        assert_eq!(wire.terminal, TerminalState::Completed);
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("live turn completes inside 60s");
}
