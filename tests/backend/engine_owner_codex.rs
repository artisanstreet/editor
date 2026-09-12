//! Finite X1 Codex lifecycle proofs without the real CLI.
//!
//! Pure coverage (settings, frames, tracker, domain bridging, stall
//! predicate, binding round trip) plus fixture stdio script turns: a
//! temporary `cmd`/`sh` script types canned app-server JSONL over stdout
//! while the test drives initialize, thread/start, one authorized prompt,
//! and the streaming pump through the shared [`super::codex`] helpers. No
//! real `codex` binary, no catalog flag, no frontend selection.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use artisan_database::{
    AttachProjectInput, CreateThreadInput, SetThreadEngineConfigInput, SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CodexModelContextWindow, CodexReasoningEffort, CodexSelection,
    CodexServiceTier, CountLimit, DirectoryId, DisplayName, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineId, EngineModelId, EngineObservationTag, EngineOpenError,
    EngineOpenInput, EngineOpenOutcome, EnginePermissionPolicy, EngineProfileId, EngineResumeToken,
    EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput, EngineSelection,
    FilesystemAccess, FiniteMillis, NetworkAccess, ObservationId, ObservationSequence,
    PermissionId, ProjectId, QueueMessagePayload, RequestId, RootPath, RunId, RunUsageBasis,
    ThreadId, ThreadTitle, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use artisan_native_engine::NativeCodexAuthority;
use artisan_transport::CancelHandle;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::bounded_line::{BoundedLineError, read_bounded_line};
use super::codex::{
    CODEX_MAX_FRAME_BYTES, CodexContinuationDecision, CodexContinuationGateInput, CodexEvent,
    CodexLineError, CodexPendingTracker, CodexQuotaWindowKind, CodexSettings,
    CodexTerminalLifecycle, CodexToolAction, CodexTurnState, CodexUsageAttribution,
    CodexUsageContext, CodexUsageScope, answer_approval, answer_questions, apply_event,
    check_codex_native_continuation, clamp_codex_percent_used, classify_codex_quota_window_kind,
    classify_exit, codex_account_read_line, codex_cli_meets_minimum, codex_rate_limits_read_line,
    codex_requires_group_termination, codex_reset_at_iso, codex_usage_report, has_stalled,
    initialize_params, interrupt_live_turn, is_codex_error_response, map_codex_rate_limit_windows,
    notification_line, parse_frame, parse_thread_token_usage, read_codex_line, request_line,
    steer_live_turn, terminal_observation, thread_resume_params, write_line,
};
use super::observation::{EngineObservation, TerminalState};
use super::operation::{
    AcceptedTurn, EngineOperationError, SteerDelivery, SteerError, ack_codex_steer_response,
    codex_response_id_matches, codex_resumed_thread_id, codex_thread_id, codex_turn_id,
    is_codex_result_for, service_codex_steer_delivery,
};
use super::{EngineCodexTurnInput, EngineContinuation, EngineOwner, EngineOwnerShutdown};
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
    let unbound =
        serde_json::json!({"input": [{"text": "hello", "text_elements": [], "type": "text"}]});
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
    assert!(!codex_response_id_matches(
        TURN_MISSING_THREAD_ID_ERROR_LINE,
        2
    ));
    assert!(codex_response_id_matches(
        TURN_MISSING_THREAD_ID_ERROR_LINE,
        3
    ));
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

#[tokio::test(flavor = "current_thread")]
async fn interrupt_live_turn_saturates_max_sentinel_id() {
    // The pump's cancellation path issues the interrupt with a u64::MAX
    // sentinel id (operation.rs). A plain `+= 1` panics debug builds and
    // wraps release builds to 0, colliding with handshake ids and wedging
    // the cancelled run. The interrupt must still send with id MAX and the
    // counter must saturate so repeated cancels stay harmless.
    let mut sink: Vec<u8> = Vec::new();
    let mut request_id = u64::MAX;
    interrupt_live_turn(&mut sink, &mut request_id, "thread-1", "turn-1")
        .await
        .expect("sentinel interrupt must send without overflow");
    assert_eq!(
        request_id,
        u64::MAX,
        "sentinel id saturates instead of wrapping"
    );
    let line = String::from_utf8(sink).expect("interrupt line is utf8");
    let value: serde_json::Value =
        serde_json::from_str(line.trim()).expect("interrupt line is json");
    assert_eq!(value["id"], u64::MAX);
    assert_eq!(value["method"], "turn/interrupt");

    // Normal ids still advance by exactly one with the consumed id on wire.
    let mut sink: Vec<u8> = Vec::new();
    let mut request_id = 7u64;
    interrupt_live_turn(&mut sink, &mut request_id, "thread-1", "turn-1")
        .await
        .expect("normal interrupt sends");
    assert_eq!(request_id, 8);
    let line = String::from_utf8(sink).expect("interrupt line is utf8");
    let value: serde_json::Value =
        serde_json::from_str(line.trim()).expect("interrupt line is json");
    assert_eq!(value["id"], 7);
}

// ---------------------------------------------------------------------------
// Bounded provider line reads
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn bounded_line_reader_rejects_oversized_and_unterminated_lines() {
    // A cap of 8 bytes: the unterminated 64-byte line must fail typed
    // BEFORE its bytes are copied into the caller's line.
    let oversized = [b'x'; 64];
    let mut reader = BufReader::new(&oversized[..]);
    let mut line = String::from("stale");
    let error = read_bounded_line(&mut reader, &mut line, 8)
        .await
        .expect_err("an over-cap line must reject");
    assert_eq!(error, BoundedLineError::LineTooLong);
    assert!(line.is_empty(), "no over-cap bytes may be retained");

    // A terminated line of exactly the cap is accepted, including the LF,
    // and the next read observes the clean EOF.
    let exact = b"12345678\n".to_vec();
    let mut reader = BufReader::new(&exact[..]);
    let mut line = String::new();
    let read = read_bounded_line(&mut reader, &mut line, 8)
        .await
        .expect("exact-cap line reads");
    assert_eq!(read, 9);
    assert_eq!(line, "12345678\n");
    let mut tail = String::new();
    assert_eq!(
        read_bounded_line(&mut reader, &mut tail, 8)
            .await
            .expect("clean eof reads"),
        0
    );

    // An EOF-terminated partial line is returned verbatim, mirroring
    // `read_line`, and non-UTF-8 bytes reject typed.
    let partial = b"tail".to_vec();
    let mut reader = BufReader::new(&partial[..]);
    let mut line = String::new();
    assert_eq!(
        read_bounded_line(&mut reader, &mut line, 8)
            .await
            .expect("partial eof line reads"),
        4
    );
    assert_eq!(line, "tail");
    let invalid = [0xff, 0xfe];
    let mut reader = BufReader::new(&invalid[..]);
    let mut line = String::new();
    assert_eq!(
        read_bounded_line(&mut reader, &mut line, 8)
            .await
            .expect_err("invalid utf-8 must reject"),
        BoundedLineError::InvalidUtf8
    );
}

#[tokio::test(flavor = "current_thread")]
async fn codex_wire_line_reads_are_bounded_before_allocation() {
    let oversized = vec![b'{'; CODEX_MAX_FRAME_BYTES + 1];
    let mut reader = BufReader::new(&oversized[..]);
    let mut line = String::new();
    let shutdown = Arc::new(CancelHandle::new());
    let control = Arc::new(CancelHandle::new());
    let error = read_codex_line(
        &mut reader,
        &mut line,
        Instant::now() + Duration::from_secs(5),
        &shutdown,
        &control,
    )
    .await
    .expect_err("an over-bound codex line must reject typed");
    assert_eq!(error, CodexLineError::FrameTooLarge);
    assert!(
        line.is_empty(),
        "over-bound provider bytes must never land in the line buffer"
    );
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

#[test]
fn steer_ack_routing_resolves_typed_never_as_turn_event() {
    use std::collections::HashMap;

    // A correlated result resolves the delivery successfully and the line
    // is consumed: it must never reach the turn-event pipeline as a
    // completion.
    let mut pending: HashMap<u64, tokio::sync::oneshot::Sender<Result<(), SteerError>>> =
        HashMap::new();
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    pending.insert(41, ack_tx);
    assert!(ack_codex_steer_response(
        r#"{"id":41,"result":{"ok":true}}"#,
        &mut pending
    ));
    assert!(pending.is_empty());
    assert_eq!(
        ack_rx.try_recv(),
        Ok(Ok(())),
        "correlated result acks the steer"
    );

    // A correlated error — for example an `expectedTurnId` mismatch
    // rejection — resolves the SAME delivery as typed failure, never as
    // success, and still never as a turn event.
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    pending.insert(42, ack_tx);
    assert!(ack_codex_steer_response(
        r#"{"id":42,"error":{"code":-32600,"message":"Invalid request: wrong expectedTurnId"}}"#,
        &mut pending
    ));
    assert!(pending.is_empty());
    assert_eq!(
        ack_rx.try_recv(),
        Ok(Err(SteerError::DeliveryFailed)),
        "provider rejection fails typed, never success"
    );

    // String-form ids correlate exactly like numeric ones.
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    pending.insert(43, ack_tx);
    assert!(ack_codex_steer_response(
        r#"{"id":"43","result":{"ok":true}}"#,
        &mut pending
    ));
    assert_eq!(ack_rx.try_recv(), Ok(Ok(())));

    // A matching id with NEITHER a valid result nor a valid error (a bare
    // `{id}` with no result) is a malformed provider reply: it still
    // consumes the entry so the line never becomes a turn event, but it
    // resolves typed failure, never success.
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    pending.insert(45, ack_tx);
    assert!(ack_codex_steer_response(r#"{"id":45}"#, &mut pending));
    assert!(pending.is_empty());
    assert_eq!(
        ack_rx.try_recv(),
        Ok(Err(SteerError::DeliveryFailed)),
        "malformed matching reply fails typed, never success"
    );

    // Anything uncorrelated keeps the existing turn handling: unknown ids,
    // method notifications, and error envelopes for other requests all
    // return false and leave pending entries intact.
    let (ack_tx, _ack_rx) = tokio::sync::oneshot::channel();
    pending.insert(44, ack_tx);
    assert!(!ack_codex_steer_response(
        r#"{"id":7,"result":{"turn":{"id":"turn-9"}}}"#,
        &mut pending
    ));
    assert!(!ack_codex_steer_response(
        r#"{"method":"turn/completed","params":{"turn":{"id":"turn-1"}}}"#,
        &mut pending
    ));
    assert!(!ack_codex_steer_response(
        r#"{"id":9,"error":{"code":-32600,"message":"other request failed"}}"#,
        &mut pending
    ));
    assert_eq!(pending.len(), 1, "uncorrelated lines never disturb pending");
}

#[tokio::test]
async fn steer_servicing_registers_ack_without_resolving_on_write() {
    // The write alone must not resolve: the ack waits for the correlated
    // provider result even though the exact `turn/steer` bytes (with the
    // actual provider turn id as `expectedTurnId`) already reached the
    // transport. A withheld provider reply leaves the delivery pending,
    // never spuriously successful.
    let (mut pump_end, provider_end) = tokio::io::duplex(65_536);
    let mut provider_end = BufReader::new(provider_end);
    let mut pending = std::collections::HashMap::new();
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    let mut next_id = 41u64;
    service_codex_steer_delivery(
        &mut pump_end,
        &mut next_id,
        "t-1",
        Some("turn-1"),
        SteerDelivery::new("req-steer-1".to_owned(), "follow up".to_owned(), ack_tx),
        &mut pending,
    )
    .await;
    assert_eq!(next_id, 42, "one request id consumed per steer");
    assert_eq!(pending.len(), 1, "ack registered for the correlated result");
    assert!(
        ack_rx.try_recv().is_err(),
        "withheld provider reply leaves the ack pending"
    );
    let mut line = String::new();
    provider_end
        .read_line(&mut line)
        .await
        .expect("steer bytes readable");
    let steer: serde_json::Value = serde_json::from_str(line.trim()).expect("steer json");
    assert_eq!(steer["id"], 41);
    assert_eq!(steer["method"], "turn/steer");
    assert_eq!(steer["params"]["expectedTurnId"], "turn-1");
    assert_eq!(steer["params"]["threadId"], "t-1");

    // The correlated result then resolves the registered ack.
    assert!(ack_codex_steer_response(
        r#"{"id":41,"result":{"ok":true}}"#,
        &mut pending
    ));
    assert_eq!(
        ack_rx.try_recv(),
        Ok(Ok(())),
        "correlated result settles the delivery"
    );
}

#[tokio::test]
async fn steer_servicing_rejects_missing_turn_id_without_inventing_one() {
    // Pre-turn-start input with no known provider id rejects typed and
    // consumes no request id: the pump never fabricates an `expectedTurnId`.
    let (mut pump_end, _provider_end) = tokio::io::duplex(65_536);
    let mut pending = std::collections::HashMap::new();
    let (ack_tx, mut ack_rx) = tokio::sync::oneshot::channel();
    let mut next_id = 41u64;
    service_codex_steer_delivery(
        &mut pump_end,
        &mut next_id,
        "t-1",
        None,
        SteerDelivery::new("req-steer-2".to_owned(), "follow up".to_owned(), ack_tx),
        &mut pending,
    )
    .await;
    assert_eq!(next_id, 41, "rejected steer allocates nothing");
    assert!(pending.is_empty());
    assert_eq!(
        ack_rx.try_recv(),
        Ok(Err(SteerError::DeliveryFailed)),
        "missing turn id fails typed"
    );
}

#[tokio::test]
async fn steer_text_without_channel_is_typed_unsupported() {
    // Cursor/grok/opencode2 turns never carry a sender: the attempt
    // resolves `Unsupported` without touching any pump or hanging.
    let (_prepared_tx, prepared_rx) = tokio::sync::oneshot::channel::<
        Result<super::operation::PreparedSession, EngineOperationError>,
    >();
    let (authorize_tx, authorize_rx) = tokio::sync::oneshot::channel::<()>();
    let (_obs_tx, obs_rx) = mpsc::channel(8);
    let (_respond_tx, respond_rx) = tokio::sync::oneshot::channel::<
        Result<super::operation::EngineTurnResult, EngineOperationError>,
    >();
    let control = Arc::new(CancelHandle::new());
    let turn = AcceptedTurn::from_parts(
        run_id(),
        prepared_rx,
        authorize_tx,
        obs_rx,
        respond_rx,
        control,
        None,
    );
    drop(authorize_rx);
    assert_eq!(
        turn.steer_text("req-unsupported", "follow up").await,
        Err(SteerError::Unsupported)
    );
}

#[tokio::test]
async fn steer_text_future_holds_no_turn_borrow() {
    // Frozen split-borrow shape (§1/§7): the dispatch arm builds the
    // steer future, then drains observations through `&mut turn` while
    // driving it. This compiles only if the future owns its clones and
    // retains no `turn` borrow — precisely what the `use<>` capture on
    // the return position pins. `require_send_static` additionally pins
    // the frozen `Send + 'static` bounds at the call site.
    fn require_send_static<T: Send + 'static>(value: T) -> T {
        value
    }
    let (_prepared_tx, prepared_rx) = tokio::sync::oneshot::channel::<
        Result<super::operation::PreparedSession, EngineOperationError>,
    >();
    let (authorize_tx, _authorize_rx) = tokio::sync::oneshot::channel::<()>();
    let (_obs_tx, obs_rx) = mpsc::channel(8);
    let (_respond_tx, respond_rx) = tokio::sync::oneshot::channel::<
        Result<super::operation::EngineTurnResult, EngineOperationError>,
    >();
    let control = Arc::new(CancelHandle::new());
    let mut turn = AcceptedTurn::from_parts(
        run_id(),
        prepared_rx,
        authorize_tx,
        obs_rx,
        respond_rx,
        control,
        None,
    );
    let pending = require_send_static(turn.steer_text("req-borrow", "follow up"));
    // Mutable drain-side use BEFORE the first poll: borrows `turn`
    // mutably while the future is alive but unpolled.
    turn.authorize()
        .expect("mutable turn use alongside the live future");
    assert_eq!(pending.await, Err(SteerError::Unsupported));
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
const TURN_MISSING_THREAD_ID_ERROR_LINE: &str =
    r#"{"id":3,"error":{"code":-32600,"message":"Invalid request: missing field threadId"}}"#;

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
#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
async fn run_fixture_turn(
    responses: &str,
    tail: &str,
    inactivity: Duration,
    cancel_after: Option<Duration>,
) -> FixtureOutcome {
    // Keep the canned producer alive until all client writes are finished.
    // It can then close stdout deterministically for the EOF test cases.
    #[cfg(windows)]
    let synchronized_tail = format!("more >nul\r\n{tail}");
    #[cfg(not(windows))]
    let synchronized_tail = format!("cat >/dev/null\n{tail}");
    let script = FixtureScript::new(responses, &synchronized_tail);
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
    drop(stdin);

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
                    if let Ok(event) = parse_frame(&trimmed, sequence)
                        && let Some(state) = apply_event(
                            event, &run, &mut tracker, &mut active, &sender, sequence, None,
                        )
                        .await
                    {
                        break Some(state);
                    }
                }
                Err(_) => break None,
            },
        }
    };
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

    let second = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n{}\n",
        r#"{"method":"item/agentMessage/delta","params":{"delta":"replayed","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
        r#"{"method":"turn/completed","params":{"threadId":"thread-fixture-1","turn":{"id":"turn-1","status":"completed"}}}"#,
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
            let mut key = [u8::try_from(created_at_ms.rem_euclid(256)).unwrap_or_default(); 32];
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
    assert_eq!(sample.context_window, Some(200_000));

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
    assert_eq!(report.context_window_tokens(), Some(200_000));

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
    let tail = "powershell.exe -NoProfile -NonInteractive -Command \"Start-Sleep -Seconds 3\"";
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
                    if let Ok(event) = parse_frame(&trimmed, sequence)
                        && let Some(state) = apply_event(
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

    let second = format!(
        "{INIT_LINE}\n{THREAD_LINE}\n{TURN_LINE}\n{}\n{}\n",
        r#"{"method":"item/agentMessage/delta","params":{"delta":"replayed","itemId":"item-1","threadId":"thread-fixture-1","turnId":"turn-1"}}"#,
        r#"{"method":"turn/completed","params":{"threadId":"thread-fixture-1","turn":{"id":"turn-1","status":"completed"}}}"#,
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
        let mapping = PathBuf::from(&path);
        let path = if mapping.is_absolute() {
            mapping
        } else {
            let runfiles = runfiles::Runfiles::create().expect("runfiles discovery");
            runfiles::rlocation!(runfiles, path.as_str()).expect("wire fixture runfile")
        };
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
    let test_executable = std::env::current_exe().expect("test executable path");
    let cargo_example = test_executable
        .parent()
        .and_then(|deps| deps.parent())
        .expect("Cargo target directory")
        .join("examples")
        .join(format!(
            "codex-wire-fixture{}",
            std::env::consts::EXE_SUFFIX
        ));
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for candidate in [
        cargo_example,
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
fn codex_wire_scenario_program(fixture: &Path, dir: &Path, scenario: &str) -> PathBuf {
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
        stream_budget: budget(15_000),
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
    let config = EngineRunConfig::new(EngineSelection::Codex(selection), codex_wire_runtime());
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
async fn drive_codex_wire_turn(mut turn: AcceptedTurn) -> WireTurnOutcome {
    let prepared = turn.prepare().await.expect("wire turn prepares");
    let session = prepared.session().to_owned();
    turn.authorize().expect("wire turn authorizes once");
    let mut text = String::new();
    let mut observed_terminal = None;
    while let Some(observation) =
        tokio::time::timeout(Duration::from_secs(20), turn.next_observation())
            .await
            .expect("wire observation settles")
    {
        match observation {
            EngineObservation::TextDelta(delta) => text.push_str(delta.delta()),
            EngineObservation::Usage(_) => {}
            EngineObservation::Terminal(terminal) => observed_terminal = Some(terminal.state()),
            _ => panic!("unexpected wire observation"),
        }
    }
    let result = turn.finish().await.expect("wire turn finishes");
    let terminal = result.terminal();
    if let Some(observed) = observed_terminal {
        assert_eq!(observed, terminal);
    }
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
    admit_codex_wire_resume_turn(
        owner, settings, launch, thread_id, run_id, root, prompt, None,
    )
    .await
}

/// Admits one wire turn with an optional gated provider continuation, so
/// resume paths drive `thread/resume` instead of `thread/start`.
#[expect(
    clippy::too_many_arguments,
    reason = "fixture admission helper mirrors the turn-input fields one-for-one; a wrapper struct would only rename them"
)]
#[expect(
    clippy::unused_async,
    reason = "kept async so every wire-turn admission helper awaits uniformly at call sites"
)]
async fn admit_codex_wire_resume_turn(
    owner: &EngineOwner,
    settings: artisan_database::ThreadEngineSettings,
    launch: artisan_native_engine::VerifiedCodexLaunch,
    thread_id: ThreadId,
    run_id: RunId,
    root: &RootPath,
    prompt: &str,
    continuation: Option<EngineContinuation>,
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
                continuation,
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
        let settings = codex_wire_settings(
            codex_wire_selection("codex-fixture", true),
            &temp.root,
            &thread_id,
        )
        .await;
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
        super::socket::reset_socket_open_count_for_tests();
        let turn = admit_codex_wire_turn(
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
        let wire = drive_codex_wire_turn(turn).await;
        assert!(
            started.elapsed() < Duration::from_secs(50),
            "accepted turn settles well inside budget"
        );
        assert_eq!(
            super::socket::socket_open_count_for_tests(),
            1,
            "the live Codex configured turn must open through the socket seam exactly once"
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
        let settings = codex_wire_settings(
            codex_wire_selection("codex-fixture", true),
            &temp.root,
            &thread_id,
        )
        .await;
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
        turn.prepare()
            .await
            .expect("thread prepared before turn authorization");
        turn.authorize()
            .expect("authorize the rejected turn request");
        let result = turn.finish().await;
        assert!(
            matches!(result, Err(EngineOperationError::ProviderRequestFailed)),
            "rejected turn/start must fail the authorized turn promptly: {result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
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
        let settings = codex_wire_settings(
            codex_wire_selection("codex-fixture", true),
            &temp.root,
            &thread_id,
        )
        .await;
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
        let turn = admit_codex_wire_turn(
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
        let wire = drive_codex_wire_turn(turn).await;
        assert_eq!(wire.session, "thread-fixture-1");
        assert_eq!(wire.text, "hello wire");
        assert_eq!(wire.terminal, TerminalState::Completed);
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("interleaved turn completes inside 60s");
}

#[tokio::test]
async fn codex_wire_owner_resumes_through_interleaved_notifications() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let fixture = codex_wire_fixture_program();
        let temp = WireTempRoot::new("resume-interleave");
        let program = codex_wire_scenario_program(&fixture, &temp.dir, "resume_interleave");
        let thread_id = ThreadId::parse("thread-wire-resume").expect("thread id");
        let settings = codex_wire_settings(
            codex_wire_selection("codex-fixture", true),
            &temp.root,
            &thread_id,
        )
        .await;
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
        let turn = admit_codex_wire_resume_turn(
            &owner,
            settings,
            launch,
            thread_id.clone(),
            RunId::parse("wire-run-resume").expect("run id"),
            &temp.root,
            "hello wire",
            EngineContinuation::new("thread-fixture-1".to_owned()),
        )
        .await;
        // The real CLI emits its notification burst (remoteControl,
        // deprecation, mcp, thread status) before the id-matched
        // `thread/resume` result: the correlated preflight wait still
        // reopens the same provider thread and delivers the turn.
        let wire = drive_codex_wire_turn(turn).await;
        assert_eq!(wire.session, "thread-fixture-1");
        assert_eq!(wire.text, "hello wire");
        assert_eq!(wire.terminal, TerminalState::Completed);
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("interleaved resume completes inside 60s");
}

#[tokio::test]
async fn codex_wire_owner_rejects_foreign_resume_thread_without_fresh_start() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let fixture = codex_wire_fixture_program();
        let temp = WireTempRoot::new("resume-mismatch");
        let program = codex_wire_scenario_program(&fixture, &temp.dir, "resume_mismatch");
        let thread_id = ThreadId::parse("thread-wire-resume-foreign").expect("thread id");
        let settings = codex_wire_settings(
            codex_wire_selection("codex-fixture", true),
            &temp.root,
            &thread_id,
        )
        .await;
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
        let mut turn = admit_codex_wire_resume_turn(
            &owner,
            settings,
            launch,
            thread_id.clone(),
            RunId::parse("wire-run-resume-foreign").expect("run id"),
            &temp.root,
            "hello wire",
            EngineContinuation::new("thread-fixture-1".to_owned()),
        )
        .await;
        // A resume result naming another thread fails closed: no silent
        // fresh start, so preparation itself fails and no prompt is ever
        // authorized and no turn starts.
        let started = std::time::Instant::now();
        let prepared = turn.prepare().await;
        assert!(
            matches!(prepared, Err(EngineOperationError::ProviderRequestFailed)),
            "foreign resume thread must fail preparation fast: {prepared:?}"
        );
        let result = turn.finish().await;
        assert!(
            matches!(result, Err(EngineOperationError::ProviderRequestFailed)),
            "foreign resume thread must fail the turn promptly: {result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
        assert!(
            !temp.dir.join("turn-start-params.json").exists(),
            "no turn may start after a rejected resume"
        );
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("rejected resume settles inside 60s");
}

// ---------------------------------------------------------------------------
// Socket open phase (per-engine EngineSocket `open`/drive split)
// ---------------------------------------------------------------------------

/// Builds one immutable socket open context for the direct open-path tests.
fn codex_socket_context<'a>(
    settings: &'a artisan_database::ThreadEngineSettings,
    shutdown: &'a Arc<CancelHandle>,
    control: &'a Arc<CancelHandle>,
) -> super::socket::SocketTurnContext<'a> {
    let runtime = settings.config().runtime();
    let budget = |value: FiniteMillis| Duration::from_millis(value.get());
    super::socket::SocketTurnContext {
        settings,
        limits: super::EngineLimits {
            readiness: budget(runtime.readiness_budget()),
            health: budget(runtime.health_budget()),
            prompt: budget(runtime.prompt_budget()),
            sse: budget(runtime.stream_budget()),
            close: budget(runtime.close_budget()),
        },
        bounds: super::EngineBounds {
            max_json_body: usize::try_from(runtime.max_json_body_bytes().get())
                .expect("json body bound"),
            max_sse_line: usize::try_from(runtime.max_sse_line_bytes().get())
                .expect("sse line bound"),
            max_sse_event: usize::try_from(runtime.max_sse_event_bytes().get())
                .expect("sse event bound"),
            max_readiness_line: usize::try_from(runtime.max_readiness_line_bytes().get())
                .expect("readiness line bound"),
            max_headers: usize::try_from(runtime.max_header_count().get())
                .expect("header count bound"),
            max_buf_bytes: usize::try_from(runtime.max_http_buffer_bytes().get())
                .expect("http buffer bound"),
            stderr_cap_bytes: usize::try_from(runtime.max_stderr_bytes().get())
                .expect("stderr bound"),
            sink_capacity: usize::try_from(runtime.observation_capacity().get())
                .expect("sink bound"),
            control_capacity: 1,
        },
        attempt_deadline: Instant::now() + Duration::from_secs(30),
        shutdown,
        control,
    }
}

/// Opens one Codex session directly through the socket seam and returns the
/// typed outcome (the child is cleaned up by the returned outcome/session).
async fn open_codex_socket(
    program: &Path,
    temp: &WireTempRoot,
    thread_id: &ThreadId,
    prompt: &str,
    resume: Option<EngineResumeToken>,
) -> EngineOpenOutcome {
    let settings = codex_wire_settings(
        codex_wire_selection("codex-fixture", false),
        &temp.root,
        thread_id,
    )
    .await;
    let launch = NativeCodexAuthority::new()
        .resolve_launch_with_executable(
            &temp.db_path,
            &EngineProfileId::parse("codex-fixture").expect("profile id"),
            program,
            "codex-cli 0.145.0",
        )
        .expect("wire launch resolves");
    let shutdown = Arc::new(CancelHandle::new());
    let control = Arc::new(CancelHandle::new());
    let context = codex_socket_context(&settings, &shutdown, &control);
    let internal = super::InternalLaunch::Codex(Box::new(launch));
    super::socket::adapter_for(&internal, context)
        .open(EngineOpenInput {
            working_directory: temp.root.as_str().to_owned(),
            prompt: prompt.to_owned(),
            resume,
        })
        .await
}

#[tokio::test]
async fn codex_socket_open_maps_spawn_failure() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let fixture = codex_wire_fixture_program();
        let temp = WireTempRoot::new("socket-spawn");
        let program = codex_wire_scenario_program(&fixture, &temp.dir, "strict");
        let thread_id = ThreadId::parse("thread-socket-spawn").expect("thread id");
        let settings = codex_wire_settings(
            codex_wire_selection("codex-fixture", false),
            &temp.root,
            &thread_id,
        )
        .await;
        let launch = NativeCodexAuthority::new()
            .resolve_launch_with_executable(
                &temp.db_path,
                &EngineProfileId::parse("codex-fixture").expect("profile id"),
                &program,
                "codex-cli 0.145.0",
            )
            .expect("wire launch resolves");
        // Removing the resolved fixture after certification makes the
        // protected spawn revalidation fail deterministically on every
        // platform, without depending on operating-system spawn errors.
        std::fs::remove_file(&program).expect("fixture program removal");
        let shutdown = Arc::new(CancelHandle::new());
        let control = Arc::new(CancelHandle::new());
        let context = codex_socket_context(&settings, &shutdown, &control);
        let internal = super::InternalLaunch::Codex(Box::new(launch));
        let outcome = super::socket::adapter_for(&internal, context)
            .open(EngineOpenInput {
                working_directory: temp.root.as_str().to_owned(),
                prompt: "hello socket".to_owned(),
                resume: None,
            })
            .await;
        assert!(
            matches!(
                outcome,
                EngineOpenOutcome::Failed {
                    error: EngineOpenError::SpawnFailed,
                    custody: None,
                }
            ),
            "spawn failure must map typed: {outcome:?}"
        );
    })
    .await
    .expect("socket spawn failure settles inside 30s");
}

#[tokio::test]
async fn codex_socket_open_maps_resume_token_to_provider_thread() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let fixture = codex_wire_fixture_program();
        let temp = WireTempRoot::new("socket-resume");
        let program = codex_wire_scenario_program(&fixture, &temp.dir, "resume_interleave");
        let thread_id = ThreadId::parse("thread-socket-resume").expect("thread id");
        let outcome = open_codex_socket(
            &program,
            &temp,
            &thread_id,
            "hello socket",
            Some(EngineResumeToken {
                native_thread_id: "thread-fixture-1".to_owned(),
            }),
        )
        .await;
        let EngineOpenOutcome::Opened(run) = outcome else {
            panic!("gated resume must open: {outcome:?}");
        };
        // A resumed thread maps to the provider thread id the app-server
        // returns, and the run names the shared run-state observation tag.
        assert_eq!(run.native_thread_id.native_thread_id, "thread-fixture-1");
        assert_eq!(run.observation_tag, EngineObservationTag::RunState);
        drop(run);
    })
    .await
    .expect("socket resume open settles inside 30s");
}

#[tokio::test]
async fn codex_socket_open_rejects_foreign_resume_thread() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let fixture = codex_wire_fixture_program();
        let temp = WireTempRoot::new("socket-resume-foreign");
        let program = codex_wire_scenario_program(&fixture, &temp.dir, "resume_mismatch");
        let thread_id = ThreadId::parse("thread-socket-resume-foreign").expect("thread id");
        let outcome = open_codex_socket(
            &program,
            &temp,
            &thread_id,
            "hello socket",
            Some(EngineResumeToken {
                native_thread_id: "thread-fixture-1".to_owned(),
            }),
        )
        .await;
        // A resume result naming another thread fails closed: no silent
        // fresh start, so the open maps the rejection typed.
        assert!(
            matches!(
                outcome,
                EngineOpenOutcome::Failed {
                    error: EngineOpenError::ResumeRejected,
                    custody: None,
                }
            ),
            "foreign resume thread must map typed: {outcome:?}"
        );
    })
    .await
    .expect("socket resume rejection settles inside 30s");
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
        let settings =
            codex_wire_settings(codex_live_selection(&profile_name), &temp.root, &thread_id).await;
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
        let turn = admit_codex_wire_turn(
            &owner,
            settings,
            launch,
            thread_id.clone(),
            RunId::parse("wire-run-live").expect("run id"),
            &temp.root,
            "Return SEND_PROBE_OK only. Do not use tools and do not read files.",
        )
        .await;
        let wire = drive_codex_wire_turn(turn).await;
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

// ---------------------------------------------------------------------------
// Rich activity normalization (Codex visual parity)
//
// Decoder, normalizer, and authority proofs for the activity subset the Rust
// owner previously discarded. Every emission is asserted as
// `EngineObservation::Activity` carrying one validated domain observation
// with the provider item/turn identities preserved verbatim, a stable
// generated observation id, and the source-local frame sequence. The
// dispatcher `Activity` arm arrives in a separate packet; these tests prove
// the owner channel side only.
// ---------------------------------------------------------------------------

/// Applies one event and drains exactly the emitted activity rows.
///
/// Panics when the activity path emits anything but `Activity` rows: the
/// plain delta, usage, and terminal shapes keep their own channels.
async fn apply_activity(
    event: CodexEvent,
    run: &RunId,
    tracker: &mut CodexPendingTracker,
    active: &mut Option<String>,
    sequence: u64,
) -> (Option<TerminalState>, Vec<artisan_domain::Observation>) {
    let (sender, mut receiver) = mpsc::channel(64);
    let terminal = apply_event(event, run, tracker, active, &sender, sequence, None).await;
    drop(sender);
    let mut rows = Vec::new();
    while let Ok(observation) = receiver.try_recv() {
        let EngineObservation::Activity(row) = observation else {
            panic!("activity path must emit only Activity rows");
        };
        rows.push(row);
    }
    (terminal, rows)
}

/// One tracker bound to the fixture root thread used across activity tests.
fn bound_activity_tracker() -> CodexPendingTracker {
    let mut tracker = CodexPendingTracker::new();
    tracker.bind_native_thread("t-1");
    tracker
}

#[test]
fn reasoning_summary_delta_and_boundary_decode_with_scope() {
    let event = parse_frame(
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"thinking aloud","itemId":"item-r1","summaryIndex":2,"threadId":"t-1","turnId":"turn-1"}}"#,
        7,
    )
    .expect("summary delta decodes");
    let CodexEvent::ReasoningSummaryDelta {
        turn_id,
        item_id,
        summary_index,
        delta,
        ..
    } = event
    else {
        panic!("expected reasoning summary delta");
    };
    assert_eq!(turn_id, "turn-1");
    assert_eq!(item_id, "item-r1");
    assert_eq!(summary_index, 2);
    assert_eq!(delta, "thinking aloud");

    // A nonzero section opener is a structural boundary, never silent.
    let boundary = parse_frame(
        r#"{"method":"item/reasoning/summaryPartAdded","params":{"itemId":"item-r1","summaryIndex":1,"threadId":"t-1","turnId":"turn-1"}}"#,
        8,
    )
    .expect("boundary decodes");
    assert!(matches!(
        boundary,
        CodexEvent::ReasoningSummaryBoundary {
            summary_index: 1,
            ..
        }
    ));

    // The section-zero opener and private reasoning content stay silent.
    let zero = parse_frame(
        r#"{"method":"item/reasoning/summaryPartAdded","params":{"itemId":"item-r1","summaryIndex":0,"threadId":"t-1","turnId":"turn-1"}}"#,
        9,
    )
    .expect("section-zero decodes");
    assert!(matches!(zero, CodexEvent::ActivitySilent));
    let private = parse_frame(
        r#"{"method":"item/reasoning/textDelta","params":{"contentIndex":0,"delta":"hidden","itemId":"item-r1","threadId":"t-1","turnId":"turn-1"}}"#,
        10,
    )
    .expect("private reasoning decodes");
    assert!(matches!(private, CodexEvent::ActivitySilent));

    // Empty deltas and missing scope never become typed activity.
    for line in [
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"","itemId":"item-r1","summaryIndex":0,"threadId":"t-1","turnId":"turn-1"}}"#,
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"x","itemId":"item-r1","summaryIndex":0,"threadId":"t-1"}}"#,
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"x","itemId":"","summaryIndex":0,"threadId":"t-1","turnId":"turn-1"}}"#,
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"x","itemId":"item-r1","summaryIndex":-1,"threadId":"t-1","turnId":"turn-1"}}"#,
    ] {
        assert!(
            matches!(
                parse_frame(line, 11).expect("frame parses"),
                CodexEvent::UnknownMethod
            ),
            "malformed summary frame stays observable: {line}"
        );
    }
}

#[tokio::test]
async fn reasoning_summary_emits_published_text_only() {
    let run = run_id();
    let mut tracker = bound_activity_tracker();
    let mut active = None;

    let event = parse_frame(
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"thinking aloud","itemId":"item-r1","summaryIndex":2,"threadId":"t-1","turnId":"turn-1"}}"#,
        7,
    )
    .expect("summary delta decodes");
    let (terminal, rows) = apply_activity(event, &run, &mut tracker, &mut active, 7).await;
    assert_eq!(terminal, None, "activity never settles the turn");
    assert_eq!(
        active.as_deref(),
        Some("turn-1"),
        "turn adopted like deltas"
    );
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::ReasoningSummaryDelta(row) = &rows[0] else {
        panic!("expected a reasoning summary delta");
    };
    assert_eq!(row.item_id().as_str(), "item-r1");
    assert_eq!(row.turn_id().as_str(), "turn-1");
    assert_eq!(row.summary_index(), 2);
    assert_eq!(row.delta(), "thinking aloud");
    assert_eq!(row.sequence().get(), 7, "source-local ordering preserved");
    assert_eq!(
        row.id().as_str(),
        "codex-run-1:codex:7:rsum:item-r1:2:0",
        "stable generated observation id"
    );

    // The nonzero section boundary emits the readable paragraph separator.
    let boundary = parse_frame(
        r#"{"method":"item/reasoning/summaryPartAdded","params":{"itemId":"item-r1","summaryIndex":1,"threadId":"t-1","turnId":"turn-1"}}"#,
        8,
    )
    .expect("boundary decodes");
    let (terminal, rows) = apply_activity(boundary, &run, &mut tracker, &mut active, 8).await;
    assert_eq!(terminal, None);
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::ReasoningSummaryDelta(row) = &rows[0] else {
        panic!("expected a boundary delta");
    };
    assert_eq!(row.delta(), "\n\n");
    assert_eq!(row.summary_index(), 1);

    // Silent shapes emit nothing and never adopt or disturb the turn.
    let mut silent_active = None;
    for line in [
        r#"{"method":"item/reasoning/summaryPartAdded","params":{"itemId":"item-r1","summaryIndex":0,"threadId":"t-1","turnId":"turn-1"}}"#,
        r#"{"method":"item/reasoning/textDelta","params":{"contentIndex":0,"delta":"hidden chain","itemId":"item-r1","threadId":"t-1","turnId":"turn-1"}}"#,
    ] {
        let event = parse_frame(line, 9).expect("silent shape decodes");
        let (terminal, rows) =
            apply_activity(event, &run, &mut tracker, &mut silent_active, 9).await;
        assert_eq!(terminal, None);
        assert!(rows.is_empty(), "private reasoning never surfaces: {line}");
    }
    assert_eq!(silent_active, None, "silent frames adopt no turn");
}

#[tokio::test]
async fn reasoning_settled_joins_authoritative_summary() {
    let run = run_id();
    let mut tracker = bound_activity_tracker();
    let mut active = Some("turn-1".to_owned());

    let event = parse_frame(
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"item-r1","summary":["first point","second point"],"type":"reasoning"}}}"#,
        12,
    )
    .expect("reasoning completion decodes");
    let (terminal, rows) = apply_activity(event, &run, &mut tracker, &mut active, 12).await;
    assert_eq!(terminal, None);
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::ReasoningSummaryCompleted(row) = &rows[0] else {
        panic!("expected a settled reasoning phase");
    };
    assert_eq!(row.item_id().as_str(), "item-r1");
    assert_eq!(row.turn_id().as_str(), "turn-1");
    assert_eq!(row.text(), Some("first point\n\nsecond point"));

    // An empty published summary still settles the phase, with no text.
    let empty = parse_frame(
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"item-r1","summary":[],"type":"reasoning"}}}"#,
        13,
    )
    .expect("empty summary decodes");
    let (_, rows) = apply_activity(empty, &run, &mut tracker, &mut active, 13).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::ReasoningSummaryCompleted(row) = &rows[0] else {
        panic!("expected a settled reasoning phase");
    };
    assert_eq!(row.text(), None);

    // A started reasoning item and a summary-free envelope stay non-activity.
    let started = parse_frame(
        r#"{"method":"item/started","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"item-r1","summary":[],"type":"reasoning"}}}"#,
        14,
    )
    .expect("started reasoning decodes");
    assert!(matches!(started, CodexEvent::ActivitySilent));
    let missing = parse_frame(
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"item-r1","type":"reasoning"}}}"#,
        15,
    )
    .expect("summary-free envelope parses");
    assert!(matches!(missing, CodexEvent::UnknownMethod));
}

#[tokio::test]
async fn tool_begin_progress_complete_normalize() {
    let run = run_id();
    let mut tracker = bound_activity_tracker();
    let mut active = Some("turn-1".to_owned());

    let started = parse_frame(
        r#"{"method":"item/started","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"tool-1","server":"srv","tool":"read","status":"inProgress","type":"mcpToolCall"}}}"#,
        20,
    )
    .expect("tool start decodes");
    let (_, rows) = apply_activity(started, &run, &mut tracker, &mut active, 20).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::Tool(row) = &rows[0] else {
        panic!("expected a tool observation");
    };
    assert_eq!(row.tool_id().as_str(), "tool-1");
    assert_eq!(row.tool_name(), "srv/read");
    assert_eq!(row.action(), artisan_domain::ToolAction::Started);
    assert_eq!(row.detail(), None);

    let progress = parse_frame(
        r#"{"method":"item/mcpToolCall/progress","params":{"itemId":"tool-1","message":"reading files","threadId":"t-1","turnId":"turn-1"}}"#,
        21,
    )
    .expect("tool progress decodes");
    let (_, rows) = apply_activity(progress, &run, &mut tracker, &mut active, 21).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::Tool(row) = &rows[0] else {
        panic!("expected a tool observation");
    };
    assert_eq!(row.action(), artisan_domain::ToolAction::Progress);
    assert_eq!(row.detail(), Some("reading files"));

    let failed = parse_frame(
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"tool-1","server":"srv","tool":"read","status":"failed","type":"mcpToolCall"}}}"#,
        22,
    )
    .expect("tool failure decodes");
    let (_, rows) = apply_activity(failed, &run, &mut tracker, &mut active, 22).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::Tool(row) = &rows[0] else {
        panic!("expected a tool observation");
    };
    assert_eq!(row.action(), artisan_domain::ToolAction::Failed);

    // A dynamic tool without a namespace keeps the TypeScript fallback name.
    let dynamic = parse_frame(
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"tool-9","tool":"fetch","status":"completed","type":"dynamicToolCall"}}}"#,
        23,
    )
    .expect("dynamic tool decodes");
    let (_, rows) = apply_activity(dynamic, &run, &mut tracker, &mut active, 23).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::Tool(row) = &rows[0] else {
        panic!("expected a tool observation");
    };
    assert_eq!(row.tool_name(), "dynamic/fetch");
    assert_eq!(row.action(), artisan_domain::ToolAction::Completed);

    // A tool name the durable vocabulary cannot hold fails closed as a unit.
    let huge = parse_frame(
        &format!(
            r#"{{"method":"item/completed","params":{{"threadId":"t-1","turnId":"turn-1","item":{{"id":"tool-9","server":"{}","tool":"{}","status":"completed","type":"mcpToolCall"}}}}}}"#,
            "s".repeat(200),
            "t".repeat(200),
        ),
        24,
    )
    .expect("oversize tool name parses");
    let (_, rows) = apply_activity(huge, &run, &mut tracker, &mut active, 24).await;
    assert!(rows.is_empty(), "oversize tool names never persist");

    // An oversize progress message is omitted while the step is preserved.
    let loud = parse_frame(
        &format!(
            r#"{{"method":"item/mcpToolCall/progress","params":{{"itemId":"tool-1","message":"{}","threadId":"t-1","turnId":"turn-1"}}}}"#,
            "m".repeat(5_000),
        ),
        25,
    )
    .expect("oversize progress parses");
    let (_, rows) = apply_activity(loud, &run, &mut tracker, &mut active, 25).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::Tool(row) = &rows[0] else {
        panic!("expected a tool observation");
    };
    assert_eq!(row.action(), artisan_domain::ToolAction::Progress);
    assert_eq!(row.detail(), None, "oversize detail omitted, not truncated");
}

#[tokio::test]
async fn foreign_child_and_malformed_frames_never_reach_root() {
    let run = run_id();
    let mut tracker = bound_activity_tracker();
    let mut active = Some("turn-1".to_owned());

    // A foreign turn claims no authority over the current turn.
    let foreign = parse_frame(
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"elsewhere","itemId":"item-r9","summaryIndex":0,"threadId":"t-1","turnId":"turn-foreign"}}"#,
        30,
    )
    .expect("foreign turn frame parses");
    let (terminal, rows) = apply_activity(foreign, &run, &mut tracker, &mut active, 30).await;
    assert_eq!(terminal, None);
    assert!(rows.is_empty(), "foreign turn activity never emits");
    assert_eq!(
        active.as_deref(),
        Some("turn-1"),
        "foreign turn never adopts"
    );

    // Child-thread activity is never coerced into the root channel.
    tracker.note_subagent("child-9", "t-1");
    assert!(tracker.is_known_child_thread("child-9"));
    assert!(
        !tracker.is_known_child_thread("t-1"),
        "root thread is not a child"
    );
    let child = parse_frame(
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"child text","itemId":"item-c1","summaryIndex":0,"threadId":"child-9","turnId":"turn-1"}}"#,
        31,
    )
    .expect("child frame parses");
    let (_, rows) = apply_activity(child, &run, &mut tracker, &mut active, 31).await;
    assert!(
        rows.is_empty(),
        "child activity never becomes root activity"
    );

    // A provider identity outside the wire identifier rule fails closed.
    let spaced = parse_frame(
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"hi","itemId":"has space","summaryIndex":0,"threadId":"t-1","turnId":"turn-1"}}"#,
        32,
    )
    .expect("spaced item id parses");
    let (_, rows) = apply_activity(spaced, &run, &mut tracker, &mut active, 32).await;
    assert!(rows.is_empty(), "unidentifiable items never persist");

    // A sequence past the finite range emits nothing instead of wrapping.
    let delta = parse_frame(
        r#"{"method":"item/reasoning/summaryTextDelta","params":{"delta":"hi","itemId":"item-r1","summaryIndex":0,"threadId":"t-1","turnId":"turn-1"}}"#,
        u64::MAX,
    )
    .expect("delta parses");
    let (_, rows) = apply_activity(delta, &run, &mut tracker, &mut active, u64::MAX).await;
    assert!(rows.is_empty(), "overflowing sequences never persist");
}

#[tokio::test]
async fn activity_ids_are_stable_and_fragments_are_bounded() {
    let run = run_id();
    let mut tracker = bound_activity_tracker();
    let mut active = Some("turn-1".to_owned());

    // A 5,000-byte delta fragments losslessly at the domain delta ceiling.
    let big = "x".repeat(5_000);
    let event = parse_frame(
        &format!(
            r#"{{"method":"item/reasoning/summaryTextDelta","params":{{"delta":"{big}","itemId":"item-r1","summaryIndex":0,"threadId":"t-1","turnId":"turn-1"}}}}"#
        ),
        40,
    )
    .expect("large delta parses");
    let (_, rows) = apply_activity(event, &run, &mut tracker, &mut active, 40).await;
    assert_eq!(rows.len(), 2, "5000 bytes split into 4096 + 904");
    let mut joined = String::new();
    for (index, row) in rows.iter().enumerate() {
        let artisan_domain::Observation::ReasoningSummaryDelta(delta) = row else {
            panic!("expected delta fragments");
        };
        joined.push_str(delta.delta());
        assert_eq!(
            delta.id().as_str(),
            format!("codex-run-1:codex:40:rsum:item-r1:0:{index}"),
            "fragment suffix disambiguates without collision"
        );
    }
    assert_eq!(joined, big, "fragments reassemble exactly");

    // Replaying the same frame reproduces identical ids: no duplication.
    let replay = parse_frame(
        &format!(
            r#"{{"method":"item/reasoning/summaryTextDelta","params":{{"delta":"{big}","itemId":"item-r1","summaryIndex":0,"threadId":"t-1","turnId":"turn-1"}}}}"#
        ),
        40,
    )
    .expect("replay parses");
    let (_, replayed) = apply_activity(replay, &run, &mut tracker, &mut active, 40).await;
    assert_eq!(replayed, rows, "replay reproduces identical observations");

    // Multi-byte text fragments on UTF-8 boundaries without corruption.
    let wide = "é".repeat(5_000);
    let event = parse_frame(
        &format!(
            r#"{{"method":"item/commandExecution/outputDelta","params":{{"delta":"{wide}","itemId":"cmd-1","threadId":"t-1","turnId":"turn-1"}}}}"#
        ),
        41,
    )
    .expect("wide output parses");
    let (_, rows) = apply_activity(event, &run, &mut tracker, &mut active, 41).await;
    assert_eq!(
        rows.len(),
        2,
        "10000 bytes split at the 8192-byte output ceiling"
    );
    let mut joined = String::new();
    for (index, row) in rows.iter().enumerate() {
        let artisan_domain::Observation::TerminalActivity(activity) = row else {
            panic!("expected terminal output rows");
        };
        assert_eq!(
            activity.state(),
            artisan_domain::TerminalActivityState::Output
        );
        let output = activity.output().expect("output chunk present");
        assert!(
            output.len() <= artisan_domain::OBSERVATION_OUTPUT_MAX_BYTES,
            "each fragment respects the domain output ceiling"
        );
        assert_eq!(
            activity.id().as_str(),
            format!("codex-run-1:codex:41:term:cmd-1:out:{index}"),
            "fragment suffix disambiguates without collision"
        );
        joined.push_str(output);
    }
    assert_eq!(joined, wide, "wide fragments reassemble exactly");

    // An oversize authoritative summary still settles its phase, textless.
    let huge = "s".repeat(70_000);
    let event = parse_frame(
        &format!(
            r#"{{"method":"item/completed","params":{{"threadId":"t-1","turnId":"turn-1","item":{{"id":"item-r1","summary":["{huge}"],"type":"reasoning"}}}}}}"#
        ),
        42,
    )
    .expect("huge summary parses");
    let (_, rows) = apply_activity(event, &run, &mut tracker, &mut active, 42).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::ReasoningSummaryCompleted(row) = &rows[0] else {
        panic!("expected a settled phase");
    };
    assert_eq!(row.text(), None, "oversize text omitted, never truncated");
}

#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
#[tokio::test]
async fn command_search_plan_and_file_frames_normalize() {
    let run = run_id();
    let mut tracker = bound_activity_tracker();
    let mut active = Some("turn-1".to_owned());

    // Command start, output, and completion preserve provider evidence.
    let started = parse_frame(
        r#"{"method":"item/started","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"cmd-1","command":"echo hi","status":"inProgress","type":"commandExecution"}}}"#,
        50,
    )
    .expect("command start decodes");
    let (_, rows) = apply_activity(started, &run, &mut tracker, &mut active, 50).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::TerminalActivity(row) = &rows[0] else {
        panic!("expected terminal activity");
    };
    assert_eq!(row.activity_id().as_str(), "cmd-1");
    assert_eq!(row.command(), Some("echo hi"));
    assert_eq!(row.state(), artisan_domain::TerminalActivityState::Started);

    let output = parse_frame(
        r#"{"method":"item/commandExecution/outputDelta","params":{"delta":"hi\n","itemId":"cmd-1","threadId":"t-1","turnId":"turn-1"}}"#,
        51,
    )
    .expect("command output decodes");
    let (_, rows) = apply_activity(output, &run, &mut tracker, &mut active, 51).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::TerminalActivity(row) = &rows[0] else {
        panic!("expected terminal output");
    };
    assert_eq!(row.output(), Some("hi\n"));

    let completed = parse_frame(
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"aggregatedOutput":"hi\n","command":"echo hi","exitCode":3,"id":"cmd-1","status":"completed","type":"commandExecution"}}}"#,
        52,
    )
    .expect("command completion decodes");
    let (_, rows) = apply_activity(completed, &run, &mut tracker, &mut active, 52).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::TerminalActivity(row) = &rows[0] else {
        panic!("expected terminal completion");
    };
    assert_eq!(
        row.state(),
        artisan_domain::TerminalActivityState::Completed
    );
    assert_eq!(row.exit_code(), Some(3));
    assert_eq!(row.output(), Some("hi\n"));

    // A declined command reports failure, exactly like the TS vocabulary.
    let declined = parse_frame(
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"command":"rm -rf /","id":"cmd-2","status":"declined","type":"commandExecution"}}}"#,
        53,
    )
    .expect("declined command decodes");
    let (_, rows) = apply_activity(declined, &run, &mut tracker, &mut active, 53).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::TerminalActivity(row) = &rows[0] else {
        panic!("expected terminal failure");
    };
    assert_eq!(row.state(), artisan_domain::TerminalActivityState::Failed);
    assert_eq!(row.exit_code(), None);

    // Web search start and completion carry the web scope and search id.
    for (method, sequence, state) in [
        ("item/started", 54, artisan_domain::SearchState::Started),
        ("item/completed", 55, artisan_domain::SearchState::Completed),
    ] {
        let event = parse_frame(
            &format!(
                r#"{{"method":"{method}","params":{{"threadId":"t-1","turnId":"turn-1","item":{{"id":"search-1","query":"rust async","type":"webSearch"}}}}}}"#
            ),
            sequence,
        )
        .expect("search frame decodes");
        let (_, rows) = apply_activity(event, &run, &mut tracker, &mut active, sequence).await;
        assert_eq!(rows.len(), 1);
        let artisan_domain::Observation::Search(row) = &rows[0] else {
            panic!("expected a search observation");
        };
        assert_eq!(row.query(), "rust async");
        assert_eq!(row.scope(), Some(artisan_domain::SearchScope::Web));
        assert_eq!(row.search_id().expect("search id").as_str(), "search-1");
        assert_eq!(row.state(), state);
    }

    // Plan updates preserve provider steps with native entry identities.
    let plan = parse_frame(
        r#"{"method":"turn/plan/updated","params":{"explanation":null,"plan":[{"status":"completed","step":"survey"},{"status":"inProgress","step":"implement"},{"status":"pending","step":"verify"}],"threadId":"t-1","turnId":"turn-1"}}"#,
        56,
    )
    .expect("plan update decodes");
    let (_, rows) = apply_activity(plan, &run, &mut tracker, &mut active, 56).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::Plan(row) = &rows[0] else {
        panic!("expected a plan observation");
    };
    assert_eq!(row.turn_id().expect("plan turn").as_str(), "turn-1");
    let entries = row.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].id().as_str(), "turn-1:plan:0");
    assert_eq!(
        entries[1].status(),
        artisan_domain::PlanEntryStatus::InProgress
    );
    assert_eq!(entries[2].text(), "verify");

    // A plan the durable vocabulary cannot hold fails closed as a unit.
    let mut steps = String::new();
    for index in 0..65 {
        if index > 0 {
            steps.push(',');
        }
        std::fmt::Write::write_fmt(
            &mut steps,
            format_args!(r#"{{"status":"pending","step":"step {index}"}}"#),
        )
        .expect("string write cannot fail");
    }
    let oversize = parse_frame(
        &format!(
            r#"{{"method":"turn/plan/updated","params":{{"explanation":null,"plan":[{steps}],"threadId":"t-1","turnId":"turn-1"}}}}"#
        ),
        57,
    )
    .expect("oversize plan parses");
    let (_, rows) = apply_activity(oversize, &run, &mut tracker, &mut active, 57).await;
    assert!(rows.is_empty(), "65-entry plans never persist partially");

    // A single plan item normalizes with its own provider identity.
    let item_plan = parse_frame(
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"plan-item-1","text":"migrate schema","type":"plan"}}}"#,
        58,
    )
    .expect("plan item decodes");
    let (_, rows) = apply_activity(item_plan, &run, &mut tracker, &mut active, 58).await;
    assert_eq!(rows.len(), 1);
    let artisan_domain::Observation::Plan(row) = &rows[0] else {
        panic!("expected a plan observation");
    };
    assert_eq!(row.entries()[0].id().as_str(), "plan-item-1");

    // File changes count exactly like the TypeScript vocabulary.
    let files = parse_frame(
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"changes":[{"diff":"--- a/new.txt\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+line1\n+line2\n","kind":{"type":"add"},"path":"new.txt"},{"diff":"gone\nstill here\n","kind":{"type":"delete"},"path":"old.txt"},{"diff":"whole new content\n","kind":{"type":"update"},"path":"edited.txt"}],"id":"file-1","status":"completed","type":"fileChange"}}}"#,
        59,
    )
    .expect("file change decodes");
    let (_, rows) = apply_activity(files, &run, &mut tracker, &mut active, 59).await;
    assert_eq!(rows.len(), 3);
    let artisan_domain::Observation::File(created) = &rows[0] else {
        panic!("expected the created file");
    };
    assert_eq!(created.path(), "new.txt");
    assert_eq!(created.action(), artisan_domain::FileAction::Created);
    assert_eq!(
        (created.lines_added(), created.lines_deleted()),
        (Some(2), Some(0))
    );
    let artisan_domain::Observation::File(deleted) = &rows[1] else {
        panic!("expected the deleted file");
    };
    assert_eq!(deleted.action(), artisan_domain::FileAction::Deleted);
    // Source parity: TypeScript CountWrittenLines counts split("\n")
    // segments, so "gone\nstill here\n" is ["gone", "still here", ""] with
    // length 3 — the trailing newline contributes a trailing empty segment
    // rather than vanishing, and the Rust port counts identically.
    assert_eq!(
        (deleted.lines_added(), deleted.lines_deleted()),
        (Some(0), Some(3))
    );
    let artisan_domain::Observation::File(modified) = &rows[2] else {
        panic!("expected the modified file");
    };
    assert_eq!(modified.action(), artisan_domain::FileAction::Modified);
    assert_eq!(
        (modified.lines_added(), modified.lines_deleted()),
        (None, None),
        "modified content without a diff stays uncounted, never zero"
    );

    // Started and unfinished file changes have no completed observation.
    for line in [
        r#"{"method":"item/started","params":{"threadId":"t-1","turnId":"turn-1","item":{"changes":[],"id":"file-1","status":"inProgress","type":"fileChange"}}}"#,
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"changes":[],"id":"file-1","status":"failed","type":"fileChange"}}}"#,
        r#"{"method":"item/fileChange/outputDelta","params":{"delta":"stale","itemId":"file-1","threadId":"t-1","turnId":"turn-1"}}"#,
    ] {
        assert!(
            matches!(
                parse_frame(line, 60).expect("frame parses"),
                CodexEvent::UnknownMethod
            ),
            "unfinished file frames stay observable: {line}"
        );
    }
}

#[tokio::test]
async fn plain_message_and_terminal_paths_stay_unchanged() {
    let run = run_id();
    let mut tracker = CodexPendingTracker::new();
    let mut active = None;

    // Agent-message item envelopes never enter the activity vocabulary: the
    // plain delta path is the only reply-text channel, without any inferred
    // classification.
    for line in [
        r#"{"method":"item/started","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"msg-1","phase":"commentary","text":"","type":"agentMessage"}}}"#,
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"msg-1","phase":"final","text":"done","type":"agentMessage"}}}"#,
        r#"{"method":"item/started","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"u-1","type":"userMessage"}}}"#,
        r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"c-1","type":"contextCompaction"}}}"#,
    ] {
        let event = parse_frame(line, 70).expect("envelope parses");
        assert!(
            matches!(event, CodexEvent::UnknownMethod),
            "non-activity envelope stays observable: {line}"
        );
        let (sender, mut receiver) = mpsc::channel(8);
        let terminal = apply_event(event, &run, &mut tracker, &mut active, &sender, 70, None).await;
        assert_eq!(terminal, None);
        assert!(
            receiver.try_recv().is_err(),
            "no observation for non-activity envelopes"
        );
    }

    // The plain delta and terminal shapes behave exactly as before.
    let delta = parse_frame(
        r#"{"method":"item/agentMessage/delta","params":{"delta":"hello","itemId":"item-1","threadId":"t-1","turnId":"turn-1"}}"#,
        71,
    )
    .expect("delta decodes");
    let (sender, mut receiver) = mpsc::channel(8);
    let terminal = apply_event(delta, &run, &mut tracker, &mut active, &sender, 71, None).await;
    assert_eq!(terminal, None);
    let EngineObservation::TextDelta(chunk) = receiver.try_recv().expect("delta observed") else {
        panic!("plain deltas keep the text channel");
    };
    assert_eq!(chunk.delta(), "hello");

    let completed = parse_frame(
        r#"{"method":"turn/completed","params":{"threadId":"t-1","turn":{"id":"turn-1","status":"completed"}}}"#,
        72,
    )
    .expect("turn completion decodes");
    let (sender, _) = mpsc::channel(8);
    let terminal = apply_event(
        completed,
        &run,
        &mut tracker,
        &mut active,
        &sender,
        72,
        None,
    )
    .await;
    assert_eq!(terminal, Some(TerminalState::Completed));
}

#[tokio::test]
async fn activity_requires_exact_bound_root_thread() {
    let run = run_id();
    let summary = |thread: &str, turn: &str| {
        format!(
            r#"{{"method":"item/reasoning/summaryTextDelta","params":{{"delta":"x","itemId":"item-r1","summaryIndex":0,"threadId":"{thread}","turnId":"{turn}"}}}}"#
        )
    };

    // An unbound tracker fails closed for rich activity even when the turn
    // matches ...
    let mut unbound = CodexPendingTracker::new();
    assert_eq!(unbound.native_thread_id(), None);
    let mut active = Some("turn-1".to_owned());
    let event = parse_frame(&summary("t-1", "turn-1"), 80).expect("activity parses");
    let (terminal, rows) = apply_activity(event, &run, &mut unbound, &mut active, 80).await;
    assert_eq!(terminal, None);
    assert!(rows.is_empty(), "unbound trackers emit no activity");
    // ... while the plain delta path on the same tracker is untouched.
    let delta = parse_frame(
        r#"{"method":"item/agentMessage/delta","params":{"delta":"hi","itemId":"item-1","threadId":"t-1","turnId":"turn-1"}}"#,
        81,
    )
    .expect("delta parses");
    let (sender, mut receiver) = mpsc::channel(8);
    let terminal = apply_event(delta, &run, &mut unbound, &mut active, &sender, 81, None).await;
    assert_eq!(terminal, None);
    let EngineObservation::TextDelta(chunk) = receiver.try_recv().expect("delta observed") else {
        panic!("plain deltas keep the text channel without binding");
    };
    assert_eq!(chunk.delta(), "hi");

    // An unknown foreign thread sharing the turn never emits.
    let mut tracker = bound_activity_tracker();
    let mut active = Some("turn-1".to_owned());
    let event = parse_frame(&summary("t-foreign", "turn-1"), 82).expect("foreign parses");
    let (terminal, rows) = apply_activity(event, &run, &mut tracker, &mut active, 82).await;
    assert_eq!(terminal, None);
    assert!(
        rows.is_empty(),
        "foreign threads never emit on the root channel"
    );
    assert_eq!(active.as_deref(), Some("turn-1"));

    // A foreign first frame adopts no turn authority.
    let mut tracker = bound_activity_tracker();
    let mut first: Option<String> = None;
    let event = parse_frame(&summary("t-foreign", "turn-1"), 83).expect("foreign parses");
    let (terminal, rows) = apply_activity(event, &run, &mut tracker, &mut first, 83).await;
    assert_eq!(terminal, None);
    assert!(rows.is_empty());
    assert_eq!(first, None, "foreign frames never establish the turn");

    // The exact bound root emits both before the turn/start result ...
    let mut tracker = bound_activity_tracker();
    let mut first: Option<String> = None;
    let event = parse_frame(&summary("t-1", "turn-1"), 84).expect("root parses");
    let (terminal, rows) = apply_activity(event, &run, &mut tracker, &mut first, 84).await;
    assert_eq!(terminal, None);
    assert_eq!(
        rows.len(),
        1,
        "interleaved root frames normalize pre-result"
    );
    assert_eq!(first.as_deref(), Some("turn-1"));
    // ... and after it.
    let event = parse_frame(&summary("t-1", "turn-1"), 85).expect("root parses");
    let (terminal, rows) = apply_activity(event, &run, &mut tracker, &mut first, 85).await;
    assert_eq!(terminal, None);
    assert_eq!(rows.len(), 1);

    // An empty bind never pins authority.
    let mut tracker = CodexPendingTracker::new();
    tracker.bind_native_thread("");
    assert_eq!(tracker.native_thread_id(), None);
    let mut active = Some("turn-1".to_owned());
    let event = parse_frame(&summary("t-1", "turn-1"), 86).expect("activity parses");
    let (_, rows) = apply_activity(event, &run, &mut tracker, &mut active, 86).await;
    assert!(rows.is_empty(), "empty binds stay unbound");
}

#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
#[test]
fn malformed_lifecycle_status_never_fabricates_success() {
    // Unknown lifecycle spellings violate the provider schema (command:
    // completed|declined|failed|inProgress; tool: completed|failed|
    // inProgress) and stay observable without ever becoming a success row.
    for (method, item) in [
        (
            "item/completed",
            r#"{"command":"echo","id":"cmd-9","status":"succeeded","type":"commandExecution"}"#,
        ),
        (
            "item/completed",
            r#"{"command":"echo","id":"cmd-9","status":"cancelled","type":"commandExecution"}"#,
        ),
        (
            "item/started",
            r#"{"command":"echo","id":"cmd-9","status":"queued","type":"commandExecution"}"#,
        ),
        (
            "item/completed",
            r#"{"id":"tool-9","server":"srv","status":"cancelled","tool":"read","type":"mcpToolCall"}"#,
        ),
        (
            "item/started",
            r#"{"id":"tool-9","server":"srv","status":"weird","tool":"read","type":"mcpToolCall"}"#,
        ),
        (
            "item/completed",
            r#"{"id":"tool-9","status":"succeeded","tool":"fetch","type":"dynamicToolCall"}"#,
        ),
    ] {
        let line = format!(
            r#"{{"method":"{method}","params":{{"threadId":"t-1","turnId":"turn-1","item":{item}}}}}"#
        );
        assert!(
            matches!(
                parse_frame(&line, 90).expect("frame parses"),
                CodexEvent::UnknownMethod
            ),
            "unknown status stays observable: {line}"
        );
    }

    // Every schema-allowed spelling still decodes to its exact lifecycle.
    let command = |method: &str, status: &str| {
        parse_frame(
            &format!(
                r#"{{"method":"{method}","params":{{"threadId":"t-1","turnId":"turn-1","item":{{"command":"echo","id":"cmd-9","status":"{status}","type":"commandExecution"}}}}}}"#
            ),
            91,
        )
        .expect("valid command parses")
    };
    assert!(matches!(
        command("item/started", "inProgress"),
        CodexEvent::TerminalLifecycle {
            state: CodexTerminalLifecycle::Started,
            ..
        }
    ));
    assert!(matches!(
        command("item/completed", "completed"),
        CodexEvent::TerminalLifecycle {
            state: CodexTerminalLifecycle::Completed,
            ..
        }
    ));
    assert!(matches!(
        command("item/completed", "inProgress"),
        CodexEvent::TerminalLifecycle {
            state: CodexTerminalLifecycle::Completed,
            ..
        }
    ));
    assert!(matches!(
        command("item/completed", "failed"),
        CodexEvent::TerminalLifecycle {
            state: CodexTerminalLifecycle::Failed,
            ..
        }
    ));
    assert!(matches!(
        command("item/completed", "declined"),
        CodexEvent::TerminalLifecycle {
            state: CodexTerminalLifecycle::Failed,
            ..
        }
    ));

    let tool = |method: &str, status: &str| {
        parse_frame(
            &format!(
                r#"{{"method":"{method}","params":{{"threadId":"t-1","turnId":"turn-1","item":{{"id":"tool-9","server":"srv","status":"{status}","tool":"read","type":"mcpToolCall"}}}}}}"#
            ),
            92,
        )
        .expect("valid tool parses")
    };
    assert!(matches!(
        tool("item/started", "inProgress"),
        CodexEvent::ToolLifecycle {
            action: CodexToolAction::Started,
            ..
        }
    ));
    assert!(matches!(
        tool("item/completed", "completed"),
        CodexEvent::ToolLifecycle {
            action: CodexToolAction::Completed,
            ..
        }
    ));
    assert!(matches!(
        tool("item/completed", "inProgress"),
        CodexEvent::ToolLifecycle {
            action: CodexToolAction::Completed,
            ..
        }
    ));
    assert!(matches!(
        tool("item/completed", "failed"),
        CodexEvent::ToolLifecycle {
            action: CodexToolAction::Failed,
            ..
        }
    ));
}

/// One line-count case: wire kind, diff payload, expected `(added, deleted)`.
type CountWrittenLinesCase = (u64, &'static str, &'static str, (Option<u64>, Option<u64>));

#[tokio::test]
async fn file_line_counts_follow_split_segment_semantics() {
    let run = run_id();
    let mut tracker = bound_activity_tracker();
    let mut active = Some("turn-1".to_owned());

    // Each case is (wire kind, diff payload with JSON escapes, expected
    // (added, deleted)). Both TypeScript CountWrittenLines and the Rust port
    // count split("\n") segments, so a trailing newline contributes one
    // trailing empty segment; modified content without a diff stays
    // uncounted rather than zero.
    let cases: &[CountWrittenLinesCase] = &[
        (110, "add", r"a\nb", (Some(2), Some(0))),
        (111, "add", r"a\n", (Some(2), Some(0))),
        (112, "delete", r"gone\nstill here\n", (Some(0), Some(3))),
        (113, "delete", r"", (Some(0), Some(0))),
        (114, "update", r"whole content\n", (None, None)),
    ];
    for (sequence, kind, diff, expected) in cases {
        let line = format!(
            r#"{{"method":"item/completed","params":{{"threadId":"t-1","turnId":"turn-1","item":{{"changes":[{{"diff":"{diff}","kind":{{"type":"{kind}"}},"path":"edge.txt"}}],"id":"file-edge","status":"completed","type":"fileChange"}}}}}}"#
        );
        let event = parse_frame(&line, *sequence).expect("edge frame parses");
        let (terminal, rows) =
            apply_activity(event, &run, &mut tracker, &mut active, *sequence).await;
        assert_eq!(terminal, None);
        assert_eq!(rows.len(), 1, "edge case {kind}/{diff:?} emits one row");
        let artisan_domain::Observation::File(row) = &rows[0] else {
            panic!("expected a file observation for {kind}/{diff:?}");
        };
        assert_eq!(
            (row.lines_added(), row.lines_deleted()),
            *expected,
            "segment semantics for {kind}/{diff:?}"
        );
    }
}
