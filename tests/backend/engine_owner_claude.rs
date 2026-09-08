//! Finite L1 Claude lifecycle proofs without the real CLI.
//!
//! Pure coverage (settings, frames, tracker, domain bridging, stall
//! predicate, binding round trip) plus fixture stdio script turns: a
//! temporary `cmd`/`sh` script types canned stream-JSON over stdout while the
//! test writes the first user message and drives the init gate plus the
//! streaming pump through the shared [`super::claude`] helpers. No real
//! `claude` binary, no catalog flag, no frontend selection, no continuation.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use artisan_domain::{
    ApprovalKind, ApprovalMode, ClaudeEffort, ClaudePermissionMode, ClaudeSelection, EngineAgentId,
    EngineModelId, EnginePermissionPolicy, EngineProfileId, FilesystemAccess, NetworkAccess,
    PermissionId, RunId, WebSearchAccess,
};
use artisan_transport::CancelHandle;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::claude::{
    CLAUDE_MAX_ANSWERS, CLAUDE_MAX_FRAME_BYTES, ClaudeApplyOutcome, ClaudeEvent,
    ClaudePendingTracker, ClaudeSession, ClaudeSettings, answer_approval, answer_questions,
    apply_event, approval_response_line, classify_exit, has_stalled, new_session_id, parse_frame,
    steer_live_turn, terminal_observation, user_message_line, write_line,
};
use super::observation::{EngineObservation, TerminalState};
use crate::native_run_dispatch::{binding_bytes_vec, binding_matches_bytes};
use artisan_native_engine::CLAUDE_NATIVE_CONTINUATION_VERSION;

// ---------------------------------------------------------------------------
// Selection and settings
// ---------------------------------------------------------------------------

fn permission(
    approval: ApprovalMode,
    filesystem: FilesystemAccess,
    network: NetworkAccess,
) -> EnginePermissionPolicy {
    EnginePermissionPolicy::new(
        PermissionId::parse("permission-claude").expect("permission id"),
        EngineAgentId::parse("agent-claude").expect("agent id"),
        approval,
        filesystem,
        network,
        WebSearchAccess::Disabled,
    )
}

fn claude_selection() -> ClaudeSelection {
    ClaudeSelection::new(
        EngineProfileId::parse("claude-fixture").expect("profile id"),
        Some(EngineModelId::parse("claude-model").expect("model id")),
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        Some(ClaudeEffort::High),
        Some(ClaudePermissionMode::Plan),
        false,
        false,
    )
    .expect("claude selection valid")
}

#[test]
fn claude_settings_map_selection_without_coercion() {
    let settings = ClaudeSettings::from_selection(&claude_selection()).expect("settings valid");
    assert_eq!(settings.profile_id(), "claude-fixture");
    let session = ClaudeSession::Start("session-1".to_owned());
    let args = settings.spawn_args(&session);
    assert!(args.len() > 10);
    assert_eq!(args[0], "-p");
    for required in [
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--forward-subagent-text",
        "--permission-prompt-tool",
        "stdio",
    ] {
        assert!(args.contains(&required.to_owned()), "missing {required}");
    }
    assert!(args.contains(&"--permission-mode".to_owned()));
    assert!(args.contains(&"plan".to_owned()));
    assert!(args.contains(&"--effort".to_owned()));
    assert!(args.contains(&"high".to_owned()));
    assert!(args.contains(&"--session-id".to_owned()));
    assert!(args.contains(&"session-1".to_owned()));
    assert!(args.contains(&"--model".to_owned()));
    assert!(args.contains(&"claude-model".to_owned()));
    assert!(
        !args.contains(&"--dangerously-skip-permissions".to_owned()),
        "on-request policy must not bypass"
    );
    assert!(
        !args.contains(&"--thinking-display".to_owned()),
        "continuation gating is a later packet"
    );
}

#[test]
fn claude_settings_resume_uses_resume_flag() {
    let settings = ClaudeSettings::from_selection(&claude_selection()).expect("settings valid");
    let args = settings.spawn_args(&ClaudeSession::Resume("session-9".to_owned()));
    assert!(args.contains(&"--resume".to_owned()));
    assert!(args.contains(&"session-9".to_owned()));
    assert!(
        !args.contains(&"--session-id".to_owned()),
        "resume must not open a fresh session"
    );
}

#[test]
fn claude_settings_bypass_tools_and_safe_modes_shape_flags() {
    let bypass = ClaudeSelection::new(
        EngineProfileId::parse("claude-fixture").expect("profile id"),
        None,
        permission(
            ApprovalMode::Never,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        None,
        None,
        true,
        true,
    )
    .expect("never-approval selection valid");
    let settings = ClaudeSettings::from_selection(&bypass).expect("settings valid");
    let args = settings.spawn_args(&ClaudeSession::Start("session-2".to_owned()));
    assert!(args.contains(&"--dangerously-skip-permissions".to_owned()));
    assert!(
        !args.contains(&"--permission-mode".to_owned()),
        "bypass carries no mode flag"
    );
    assert!(args.contains(&"--tools".to_owned()));
    assert!(args.contains(&"--safe-mode".to_owned()));
    assert!(
        !args.contains(&"--model".to_owned()),
        "absent model passes no flag"
    );

    let default_mode = ClaudeSelection::new(
        EngineProfileId::parse("claude-fixture").expect("profile id"),
        None,
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        None,
        Some(ClaudePermissionMode::Default),
        false,
        false,
    )
    .expect("default-mode selection valid");
    let settings = ClaudeSettings::from_selection(&default_mode).expect("settings valid");
    let args = settings.spawn_args(&ClaudeSession::Start("session-3".to_owned()));
    assert!(
        !args.contains(&"--permission-mode".to_owned()),
        "default mode passes no flag"
    );
}

#[test]
fn claude_selection_rejects_readonly_or_offline_policy() {
    let readonly = ClaudeSelection::new(
        EngineProfileId::parse("claude-fixture").expect("profile id"),
        None,
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::None,
            NetworkAccess::Enabled,
        ),
        None,
        None,
        false,
        false,
    );
    assert!(readonly.is_err(), "read-only policy must fail closed");
    let offline = ClaudeSelection::new(
        EngineProfileId::parse("claude-fixture").expect("profile id"),
        None,
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Disabled,
        ),
        None,
        None,
        false,
        false,
    );
    assert!(offline.is_err(), "offline policy must fail closed");
}

#[test]
fn user_message_line_shapes_stream_input() {
    let line = user_message_line("session-1", "hello");
    let value: serde_json::Value = serde_json::from_str(&line).expect("message json");
    assert_eq!(value["type"], "user");
    assert_eq!(value["session_id"], "session-1");
    assert!(value["parent_tool_use_id"].is_null());
    assert_eq!(value["message"]["role"], "user");
    assert_eq!(value["message"]["content"][0]["type"], "text");
    assert_eq!(value["message"]["content"][0]["text"], "hello");
}

#[test]
fn new_session_id_is_bounded_hex() {
    let first = new_session_id().expect("entropy available");
    assert_eq!(first.len(), 32);
    assert!(first.chars().all(|cell| cell.is_ascii_hexdigit()));
    let second = new_session_id().expect("entropy available");
    assert_ne!(first, second, "session identities must not repeat");
}

#[test]
fn continuation_version_is_recorded_as_data() {
    assert_eq!(CLAUDE_NATIVE_CONTINUATION_VERSION, "2.1.220");
}

// ---------------------------------------------------------------------------
// Frame normalization
// ---------------------------------------------------------------------------

fn run_id() -> RunId {
    RunId::parse("claude-run-1").expect("run id")
}

#[test]
fn init_delta_phases_and_message_start_decode() {
    let init = parse_frame(
        r#"{"type":"system","subtype":"init","session_id":"session-1","tools":[]}"#,
        1,
    )
    .expect("init decodes");
    assert!(matches!(init, ClaudeEvent::Init { .. }));

    let delta = parse_frame(
        r#"{"type":"stream-event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}}"#,
        2,
    )
    .expect("delta decodes");
    match delta {
        ClaudeEvent::TextDelta { delta, phase } => {
            assert_eq!(delta, "hi");
            assert_eq!(phase, "unspecified");
        }
        _ => panic!("expected text delta"),
    }

    let commentary = parse_frame(
        r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"working"},{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"echo hi"}}]}}"#,
        3,
    )
    .expect("assistant decodes");
    match commentary {
        ClaudeEvent::TextDelta { delta, phase } => {
            assert_eq!(delta, "working");
            assert_eq!(phase, "commentary");
        }
        _ => panic!("expected commentary delta"),
    }

    let start = parse_frame(
        r#"{"type":"stream-event","event":{"type":"message_start","message":{"id":"msg-7"}}}"#,
        4,
    )
    .expect("message start decodes");
    assert!(matches!(start, ClaudeEvent::MessageStart { .. }));

    let tokens = parse_frame(
        r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":42}"#,
        5,
    )
    .expect("thinking tokens decode");
    assert!(matches!(
        tokens,
        ClaudeEvent::ThinkingTokens {
            estimated_tokens: 42
        }
    ));

    let settled = parse_frame(
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":""}]}}"#,
        6,
    )
    .expect("reasoning block decodes");
    assert!(matches!(settled, ClaudeEvent::ReasoningSettled));
}

#[test]
fn approval_question_task_and_child_frames_decode() {
    let approval = parse_frame(
        r#"{"type":"control_request","request_id":"perm-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo hi"},"title":"Run echo"}}"#,
        1,
    )
    .expect("approval decodes");
    match approval {
        ClaudeEvent::ApprovalRequested(request) => {
            assert_eq!(request.approval_id(), "perm-1");
            assert_eq!(request.description(), "Run echo");
            let domain = request.to_domain_request().expect("domain request");
            assert_eq!(domain.command_text(), Some("echo hi"));
        }
        _ => panic!("expected approval"),
    }

    let question = parse_frame(
        r#"{"type":"control_request","request_id":"qreq-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which?","header":"Pick","options":[{"label":"A"}]}]}}}"#,
        2,
    )
    .expect("question decodes");
    match question {
        ClaudeEvent::QuestionRequested(request) => {
            assert_eq!(request.request_id(), "qreq-1");
            assert_eq!(request.questions().len(), 1);
            assert_eq!(request.questions()[0].question_id(), "qreq-1:0");
            let input = request.questions()[0]
                .to_domain_input()
                .expect("domain input");
            assert_eq!(input.text, "Which?");
            assert_eq!(input.header.as_deref(), Some("Pick"));
        }
        _ => panic!("expected question"),
    }

    // An AskUserQuestion with no question stays on the approval path.
    let empty_question = parse_frame(
        r#"{"type":"control_request","request_id":"qreq-2","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[]}}}"#,
        3,
    )
    .expect("empty question decodes");
    assert!(
        matches!(empty_question, ClaudeEvent::ApprovalRequested(_)),
        "unanswerable question must stay an approval"
    );

    let task = parse_frame(
        r#"{"type":"system","subtype":"task_started","task_id":"task-1","tool_use_id":"tool-9","description":"Explore"}"#,
        4,
    )
    .expect("task decodes");
    assert!(matches!(task, ClaudeEvent::SubagentLifecycle { .. }));

    let child = parse_frame(
        r#"{"type":"assistant","parent_tool_use_id":"tool-9","message":{"content":[]}}"#,
        5,
    )
    .expect("child decodes");
    assert!(matches!(child, ClaudeEvent::ChildTranscript { .. }));

    let result = parse_frame(
        r#"{"type":"result","subtype":"success","session_id":"session-1","is_error":false}"#,
        6,
    )
    .expect("result decodes");
    assert!(matches!(
        result,
        ClaudeEvent::TurnResult { success: true, .. }
    ));

    let typeless = parse_frame(
        r#"{"is_error":false,"stop_reason":null,"session_id":"session-1"}"#,
        7,
    )
    .expect("typeless terminal decodes");
    assert!(matches!(
        typeless,
        ClaudeEvent::TurnResult { success: true, .. }
    ));
}

#[test]
fn unknown_bookkeeping_and_malformed_frames_reject_safely() {
    for line in [
        r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"postTokens":10}}"#,
        r#"{"type":"system","subtype":"api_retry"}"#,
        r#"{"type":"system","subtype":"status"}"#,
        r#"{"type":"stream-event","event":{"type":"ping"}}"#,
        r#"{"type":"stream-event","event":{"type":"content_block_delta","delta":{"type":"signature_delta","signature":"abc"}}}"#,
        r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":10}}}"#,
        r#"{"type":"user","message":{"content":[]},"tool_use_result":{}}"#,
        r#"{"type":"future-event"}"#,
    ] {
        let event = parse_frame(line, 1).expect("bookkeeping stays observable");
        assert!(
            matches!(event, ClaudeEvent::Unknown),
            "unexpected projection for {line}"
        );
    }

    assert!(parse_frame("not json", 2).is_err());
    assert!(parse_frame(r#"{"nopetype":1}"#, 3).is_err());
    assert!(parse_frame("", 4).is_err());
    assert!(parse_frame(r#"{"type":""}"#, 5).is_err());
    let oversized = "x".repeat(CLAUDE_MAX_FRAME_BYTES + 1);
    assert!(parse_frame(oversized.as_str(), 6).is_err());
}

// ---------------------------------------------------------------------------
// Tracker, bridging, stall predicate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approval_deny_then_allow_resolves_without_side_effect() {
    let mut tracker = ClaudePendingTracker::new();
    let (mut client, server) = tokio::io::duplex(65_536);
    let mut server = BufReader::new(server);
    let run = run_id();

    for (request_id, approval_id, approved, behavior) in [
        ("perm-1", "perm-1", false, "deny"),
        ("perm-2", "perm-2", true, "allow"),
    ] {
        let event = parse_frame(
            &format!(
                r#"{{"type":"control_request","request_id":"{request_id}","request":{{"subtype":"can_use_tool","tool_name":"Bash","input":{{"command":"echo hi"}}}}}}"#
            ),
            1,
        )
        .expect("approval decodes");
        let ClaudeEvent::ApprovalRequested(request) = event else {
            panic!("expected approval")
        };
        assert!(tracker.note_approval(request));
        assert_eq!(tracker.pending_approvals(), 1);
        answer_approval(&mut client, &mut tracker, request_id, approval_id, approved)
            .await
            .expect("answer writes");
        assert_eq!(tracker.pending_approvals(), 0);
        let mut line = String::new();
        server
            .read_line(&mut line)
            .await
            .expect("response readable");
        let value: serde_json::Value = serde_json::from_str(line.trim()).expect("response json");
        assert_eq!(value["type"], "control_response");
        assert_eq!(value["response"]["request_id"], request_id);
        assert_eq!(value["response"]["response"]["behavior"], behavior);
        // The run continues: deltas still normalize after a deny.
        let delta = parse_frame(
            r#"{"type":"stream-event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"onward"}}}}"#,
            2,
        )
        .expect("delta decodes");
        let (sender, mut receiver) = mpsc::channel(8);
        let mut active = None;
        let outcome = apply_event(
            delta,
            &run,
            "session-1",
            &mut tracker,
            &mut active,
            &sender,
            2,
        )
        .await;
        assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
        let EngineObservation::TextDelta(chunk) = receiver.try_recv().expect("delta observed")
        else {
            panic!("expected text delta")
        };
        assert_eq!(chunk.delta(), "onward");
    }
    // A resolved target never answers twice.
    let (mut client, _) = tokio::io::duplex(65_536);
    let denied = answer_approval(&mut client, &mut tracker, "perm-1", "perm-1", true).await;
    assert!(denied.is_err(), "resolved approval must miss");
    // The helper shapes the exact control_response line.
    let line = approval_response_line("perm-9", true);
    let value: serde_json::Value = serde_json::from_str(&line).expect("helper json");
    assert_eq!(value["response"]["response"]["behavior"], "allow");
}

#[tokio::test]
async fn question_answer_and_steer_verbs_shape_lines() {
    let mut tracker = ClaudePendingTracker::new();
    let event = parse_frame(
        r#"{"type":"control_request","request_id":"qreq-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which?","options":[{"label":"Red"}]}]}}}"#,
        1,
    )
    .expect("question decodes");
    let ClaudeEvent::QuestionRequested(request) = event else {
        panic!("expected question")
    };
    assert_eq!(tracker.note_questions(&request), 1);

    let (mut client, server) = tokio::io::duplex(65_536);
    let mut server = BufReader::new(server);
    answer_questions(
        &mut client,
        &mut tracker,
        &request,
        &[("qreq-1:0".to_owned(), vec!["Red".to_owned()])],
    )
    .await
    .expect("question answer writes");
    assert_eq!(tracker.pending_questions(), 0);
    let mut line = String::new();
    server.read_line(&mut line).await.expect("answer readable");
    let value: serde_json::Value = serde_json::from_str(line.trim()).expect("answer json");
    assert_eq!(value["type"], "control_response");
    assert_eq!(value["response"]["request_id"], "qreq-1");
    assert_eq!(value["response"]["response"]["behavior"], "allow");
    assert_eq!(
        value["response"]["response"]["updatedInput"]["answers"]["Which?"],
        "Red"
    );
    assert!(
        value["response"]["response"]["updatedInput"]["questions"].is_array(),
        "verbatim input must be amended, not replaced"
    );

    steer_live_turn(&mut client, "session-1", "follow up")
        .await
        .expect("steer writes");
    line.clear();
    server.read_line(&mut line).await.expect("steer readable");
    let steer: serde_json::Value = serde_json::from_str(line.trim()).expect("steer json");
    assert_eq!(steer["type"], "user");
    assert_eq!(steer["session_id"], "session-1");
    assert_eq!(steer["message"]["role"], "user");
    assert_eq!(steer["message"]["content"][0]["text"], "follow up");

    // Answer option counts stay within the domain ceiling.
    assert!(CLAUDE_MAX_ANSWERS <= 16);
}

#[tokio::test]
async fn subagent_and_child_frames_never_adopt_the_root_turn() {
    let run = run_id();
    let mut tracker = ClaudePendingTracker::new();
    let (sender, mut receiver) = mpsc::channel(8);
    let mut active = None;
    let task = parse_frame(
        r#"{"type":"system","subtype":"task_started","task_id":"task-1","description":"Explore"}"#,
        1,
    )
    .expect("task decodes");
    let outcome = apply_event(
        task,
        &run,
        "session-1",
        &mut tracker,
        &mut active,
        &sender,
        1,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert!(receiver.try_recv().is_err(), "no root observation");
    assert_eq!(tracker.subagent_count(), 1);

    let child = parse_frame(
        r#"{"type":"assistant","parent_tool_use_id":"tool-9","message":{"content":[]}}"#,
        2,
    )
    .expect("child decodes");
    let outcome = apply_event(
        child,
        &run,
        "session-1",
        &mut tracker,
        &mut active,
        &sender,
        2,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert!(receiver.try_recv().is_err(), "no root observation");
    assert_eq!(tracker.child_frame_count(), 1);
    assert_eq!(active, None, "root turn untouched");

    // Encrypted-thinking plumbing is preserved without root text.
    let tokens = parse_frame(
        r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":7}"#,
        3,
    )
    .expect("tokens decode");
    let outcome = apply_event(
        tokens,
        &run,
        "session-1",
        &mut tracker,
        &mut active,
        &sender,
        3,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert_eq!(tracker.thinking_tokens(), Some(7));
    assert!(receiver.try_recv().is_err(), "no root observation");

    let settled = parse_frame(
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":""}]}}"#,
        4,
    )
    .expect("settled decodes");
    let outcome = apply_event(
        settled,
        &run,
        "session-1",
        &mut tracker,
        &mut active,
        &sender,
        4,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert!(tracker.reasoning_settled());
    assert!(receiver.try_recv().is_err(), "no root observation");

    // A non-empty thinking delta counts without root text either.
    let thinking = parse_frame(
        r#"{"type":"stream-event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"plan"}}}"#,
        5,
    )
    .expect("thinking delta decodes");
    assert!(matches!(thinking, ClaudeEvent::ReasoningDelta));
    let outcome = apply_event(
        thinking,
        &run,
        "session-1",
        &mut tracker,
        &mut active,
        &sender,
        5,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert_eq!(tracker.thinking_deltas(), 1);
    assert!(receiver.try_recv().is_err(), "no root observation");
}

#[tokio::test]
async fn session_mismatch_fails_the_turn_closed() {
    let run = run_id();
    let mut tracker = ClaudePendingTracker::new();
    let (sender, _) = mpsc::channel(8);
    let mut active = None;
    let init = parse_frame(
        r#"{"type":"system","subtype":"init","session_id":"session-1"}"#,
        1,
    )
    .expect("init decodes");
    let outcome = apply_event(
        init,
        &run,
        "session-1",
        &mut tracker,
        &mut active,
        &sender,
        1,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert!(tracker.init_seen());

    let wrong = parse_frame(
        r#"{"type":"system","subtype":"init","session_id":"session-wrong"}"#,
        2,
    )
    .expect("init decodes");
    let outcome = apply_event(
        wrong,
        &run,
        "session-1",
        &mut tracker,
        &mut active,
        &sender,
        2,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Terminal(TerminalState::Failed));

    // Denials ride the result frame without approval semantics.
    let denied = parse_frame(
        r#"{"type":"result","subtype":"success","session_id":"session-1","permission_denials":["x","y"]}"#,
        3,
    )
    .expect("denials decode");
    let outcome = apply_event(
        denied,
        &run,
        "session-1",
        &mut tracker,
        &mut active,
        &sender,
        3,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: true });
    assert_eq!(tracker.permission_denial_count(), 2);
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
    let file_approval = parse_frame(
        r#"{"type":"control_request","request_id":"perm-9","request":{"subtype":"can_use_tool","tool_name":"Edit","input":{},"description":"apply patch"}}"#,
        1,
    )
    .expect("file approval decodes");
    let ClaudeEvent::ApprovalRequested(request) = file_approval else {
        panic!("expected approval")
    };
    let domain_request = request.to_domain_request().expect("domain request");
    assert_eq!(domain_request.kind(), ApprovalKind::FileChange);

    let action_approval = parse_frame(
        r#"{"type":"control_request","request_id":"perm-10","request":{"subtype":"can_use_tool","tool_name":"Read","input":{"file_path":"a.txt"}}}}"#,
        2,
    )
    .expect("action approval decodes");
    let ClaudeEvent::ApprovalRequested(request) = action_approval else {
        panic!("expected approval")
    };
    let domain_request = request.to_domain_request().expect("domain request");
    assert_eq!(domain_request.kind(), ApprovalKind::Action);

    let question = parse_frame(
        r#"{"type":"control_request","request_id":"qreq-3","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which color?","header":"Pick","multiSelect":true,"options":[{"label":"Red","description":"warm"},{"label":"Blue"}]}]}}}"#,
        3,
    )
    .expect("question decodes");
    let ClaudeEvent::QuestionRequested(request) = question else {
        panic!("expected question")
    };
    let input = request.questions()[0]
        .to_domain_input()
        .expect("domain input");
    assert_eq!(input.text, "Which color?");
    assert!(input.multi_select);
    let options = input.options.expect("options carried");
    assert_eq!(options.len(), 2);
    assert_eq!(options[0].label(), "Red");
    assert_eq!(options[0].description(), Some("warm"));
}

// ---------------------------------------------------------------------------
// Binding tag/format round trip plus mismatch requeue
// ---------------------------------------------------------------------------

#[test]
fn claude_binding_round_trip_and_mismatch() {
    let raw =
        binding_bytes_vec("claude", "claude-fixture", "session-fixture-1").expect("binding builds");
    assert!(binding_matches_bytes(
        &raw,
        "claude",
        "claude-fixture",
        "session-fixture-1"
    ));
    for (engine, profile, session) in [
        ("opencode2", "claude-fixture", "session-fixture-1"),
        ("codex", "claude-fixture", "session-fixture-1"),
        ("claude", "other-profile", "session-fixture-1"),
        ("claude", "claude-fixture", "other-session"),
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
        "claude",
        "claude-fixture",
        "session-fixture-1"
    ));
    assert!(
        artisan_database::ProviderBindingBytes::new(raw).is_ok(),
        "wrapped bytes stay valid"
    );
    assert!(binding_bytes_vec("", "claude-fixture", "session-fixture-1").is_none());
    assert!(binding_bytes_vec("claude", "", "session-fixture-1").is_none());
    assert!(binding_bytes_vec("claude", "claude-fixture", "").is_none());
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
            std::env::temp_dir().join(format!("artisan-claude-{}-{nonce}", std::process::id()));
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

const SESSION: &str = "session-fixture-1";

struct FixtureOutcome {
    terminal: Option<TerminalState>,
    deltas: Vec<String>,
    phases: Vec<&'static str>,
    tracker: ClaudePendingTracker,
    session: Option<String>,
}

/// Drives one fixture script turn through the first user message, the init
/// gate, and the streaming pump with stall/cancel/EOF mapping, mirroring the
/// owner executor without spawning the real CLI.
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

    let settings = ClaudeSettings::from_selection(&claude_selection()).expect("settings");
    let session = ClaudeSession::Start(SESSION.to_owned());
    write_line(&mut stdin, &settings.user_message_line(&session, "hello"))
        .await
        .expect("prompt writes");
    let mut stdin = Some(stdin);

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
    let mut tracker = ClaudePendingTracker::new();
    let mut active: Option<String> = None;
    let mut sequence: u64 = 0;
    let mut last_activity = Instant::now();
    let mut session_seen = None;
    let mut phases = Vec::new();
    let mut line = String::new();
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
                Ok(0) => {
                    // EOF is the observed close: `result` before it is clean
                    // (modulo semantic failure); EOF before `result` is an
                    // external kill.
                    break Some(if tracker.result_seen() && !tracker.semantic_failure() {
                        TerminalState::Completed
                    } else if tracker.result_seen() {
                        TerminalState::Failed
                    } else {
                        TerminalState::Interrupted
                    });
                }
                Ok(_) => {
                    last_activity = Instant::now();
                    sequence += 1;
                    let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                    match parse_frame(&trimmed, sequence) {
                        Ok(event) => {
                            if let ClaudeEvent::Init { session_id } = &event {
                                session_seen = Some(session_id.clone());
                            }
                            if let ClaudeEvent::TextDelta { phase, .. } = &event {
                                phases.push(*phase);
                            }
                            match apply_event(
                                event, &run, SESSION, &mut tracker, &mut active, &sender, sequence,
                            )
                            .await
                            {
                                ClaudeApplyOutcome::Continue { end_input } => {
                                    if end_input {
                                        drop(stdin.take());
                                    }
                                }
                                ClaudeApplyOutcome::Terminal(state) => break Some(state),
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
        phases,
        tracker,
        session: session_seen,
    }
}

fn joined(outcome: &FixtureOutcome) -> String {
    outcome.deltas.join("")
}

fn init_line() -> String {
    format!(r#"{{"type":"system","subtype":"init","session_id":"{SESSION}","tools":[]}}"#)
}

fn result_line() -> String {
    format!(r#"{{"type":"result","subtype":"success","session_id":"{SESSION}","is_error":false}}"#)
}

#[tokio::test]
async fn fixture_start_deltas_phases_close() {
    let responses = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        init_line(),
        r#"{"type":"stream-event","event":{"type":"message_start","message":{"id":"msg-7"}}}"#,
        r#"{"type":"stream-event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello "}}}"#,
        r#"{"type":"assistant","message":{"id":"msg-7","content":[{"type":"text","text":"world"},{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"echo hi"}}]}}"#,
        r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":9}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":""}]}}"#,
        r#"{"type":"system","subtype":"task_started","task_id":"task-1","description":"Explore"}"#,
        result_line(),
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&responses, "", Duration::from_secs(5), None),
    )
    .await
    .expect("fixture finishes");
    assert_eq!(outcome.terminal, Some(TerminalState::Completed));
    assert_eq!(joined(&outcome), "hello world");
    assert_eq!(outcome.phases, vec!["unspecified", "commentary"]);
    assert_eq!(
        outcome.session.as_deref(),
        Some(SESSION),
        "exact native session identity rules"
    );
    assert_eq!(outcome.tracker.thinking_tokens(), Some(9));
    assert!(outcome.tracker.reasoning_settled());
    assert_eq!(outcome.tracker.subagent_count(), 1);
    let terminal = terminal_observation(&run_id(), 3, TerminalState::Completed);
    assert_eq!(terminal.state(), TerminalState::Completed);
}

#[tokio::test]
async fn fixture_malformed_frame_rejected_without_killing_turn() {
    let responses = format!(
        "{}\n{}\n{}\n{}\n",
        init_line(),
        "this is not json",
        r#"{"type":"stream-event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"kept"}}}"#,
        result_line(),
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
async fn fixture_session_mismatch_fails_closed() {
    let responses = format!(
        "{}\n",
        r#"{"type":"system","subtype":"init","session_id":"session-wrong","tools":[]}"#,
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&responses, "", Duration::from_secs(5), None),
    )
    .await
    .expect("fixture finishes");
    assert_eq!(outcome.terminal, Some(TerminalState::Failed));
    assert_eq!(outcome.session.as_deref(), Some("session-wrong"));
}

#[tokio::test]
async fn fixture_approval_question_steer_shapes_before_close() {
    let responses = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n",
        init_line(),
        r#"{"type":"control_request","request_id":"perm-21","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo hi"},"description":"say hi"}}"#,
        r#"{"type":"control_request","request_id":"qreq-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which?","options":[{"label":"Red"}]}]}}}"#,
        r#"{"type":"system","subtype":"task_started","task_id":"task-9","description":"Explore"}"#,
        r#"{"type":"assistant","parent_tool_use_id":"tool-9","message":{"content":[]}}"#,
        result_line(),
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
    assert_eq!(outcome.tracker.child_frame_count(), 1);
    assert!(outcome.deltas.is_empty(), "child frames never adopt root");
}

#[tokio::test]
async fn fixture_external_kill_reports_interruption() {
    let responses = format!(
        "{}\n{}\n",
        init_line(),
        r#"{"type":"stream-event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"prefix "}}}"#,
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
        "{}\n{}\n",
        init_line(),
        r#"{"type":"stream-event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"warming"}}}"#,
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
    let responses = format!("{}\n", init_line());
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
        "{}\n{}\n",
        init_line(),
        r#"{"type":"stream-event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"durable-"}}}"#,
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
        "{}\n{}\n{}\n",
        init_line(),
        r#"{"type":"stream-event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"replayed"}}}"#,
        result_line(),
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
fn duplex_write_shapes_stream_json_framing() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let (mut client, server) = tokio::io::duplex(65_536);
        let mut server = BufReader::new(server);
        write_line(&mut client, r#"{"type":"user"}"#)
            .await
            .expect("write");
        drop(client);
        let mut text = String::new();
        server.read_to_string(&mut text).await.expect("read");
        assert_eq!(text, "{\"type\":\"user\"}\n");
    });
}
