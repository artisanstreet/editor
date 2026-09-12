//! Finite L3 Claude continuation plus usage plus title plus cleanup proofs
//! without the real CLI.
//!
//! Pure coverage (settings, frames, tracker, domain bridging, stall
//! predicate, binding round trip, continuation gate, usage mapping, title
//! capture) plus fixture stdio script turns: a temporary `cmd`/`sh` script
//! types canned stream-JSON over stdout while the test writes the first user
//! message and drives the init gate plus the streaming pump through the
//! shared [`super::claude`] helpers. No real `claude` binary, no catalog
//! flag, no frontend selection, no model inventory.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use artisan_domain::{
    ApprovalKind, ApprovalMode, ByteLimit, ClaudeEffort, ClaudePermissionMode, ClaudeSelection,
    CountLimit, EngineAgentId, EngineConfigUpdatePrecondition, EngineId, EngineModelId,
    EnginePermissionPolicy, EngineProfileId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FilesystemAccess, FiniteMillis, ItemId,
    MessageBody, MessageId, MessagePhase, NetworkAccess, Observation, ObservationSequence, PatchId,
    PermissionId, ProjectId, RequestId, Revision, RunId, RunUsageBasis, SubagentState, ThreadId,
    ThreadTitle, TranscriptContent, TurnId, UnixMillis, WebSearchAccess,
};
use artisan_transport::CancelHandle;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::claude::{
    CLAUDE_MAX_ANSWERS, CLAUDE_MAX_FRAME_BYTES, ClaudeApplyOutcome, ClaudeContinuationDecision,
    ClaudeContinuationGateInput, ClaudeEvent, ClaudePendingTracker, ClaudeQuotaWindowKind,
    ClaudeSession, ClaudeSettings, ClaudeUsageAttribution, ClaudeUsageContext, ClaudeUsageScope,
    answer_approval, answer_questions, apply_event, approval_response_line,
    check_claude_native_continuation, clamp_claude_percent_used, classify_claude_quota_window_kind,
    classify_exit, claude_cli_meets_minimum, claude_cli_usage_args, claude_project_directory_name,
    claude_requires_group_termination, claude_resume_session, claude_session_title_from_lines,
    claude_session_transcript_path, claude_usage_report, has_stalled, new_session_id,
    parse_claude_assistant_usage, parse_claude_cli_reset_at, parse_claude_cli_usage_windows,
    parse_claude_result_usage, parse_frame, read_claude_session_title, steer_live_turn,
    terminal_observation, user_message_line, write_line,
};
use super::observation::{
    EngineObservation, SubagentLifecycleRow, SubagentTranscriptRow, TerminalState,
};
use crate::SystemCommandOrigin;
use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::native_run_dispatch::{
    NativeRunDispatcherConfig, NativeRunDispatcherConfigInput, SubagentCommitCursor,
    binding_bytes_vec, binding_matches_bytes, commit_subagent_observation,
};
use artisan_database::entities::ConversationItemKind;
use artisan_database::{
    BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch, CreateThreadInput,
    DispatchLeaseOwner, LaunchClaimedRun, LaunchClaimedRunOutcome, ProviderBindingBytes,
    QueueFirstMessageInput, Repository, RunBatchScope, RunLaunchCredentials, RunStartKey,
    SetThreadEngineConfigInput, SqliteConfig, connect, entities,
};
use artisan_migrations::migrate_to_current;
use artisan_native_engine::CLAUDE_NATIVE_CONTINUATION_VERSION;
use artisan_native_engine::NativeOpenCode2Authority;
use sea_orm::{ActiveValue::Set, EntityTrait};
use std::num::NonZeroUsize;

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
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}}"#,
        2,
    )
    .expect("delta decodes");
    match delta {
        ClaudeEvent::TextDelta { delta, phase, .. } => {
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
        ClaudeEvent::TextDelta { delta, phase, .. } => {
            assert_eq!(delta, "working");
            assert_eq!(phase, "commentary");
        }
        _ => panic!("expected commentary delta"),
    }

    let start = parse_frame(
        r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg-7"}}}"#,
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
    assert!(matches!(settled, ClaudeEvent::ReasoningSettled { .. }));
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
        r#"{"type":"stream_event","event":{"type":"ping"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"signature_delta","signature":"abc"}}}"#,
        r#"{"type":"user","message":{"content":[]},"tool_use_result":{}}"#,
        r#"{"type":"future-event"}"#,
    ] {
        let event = parse_frame(line, 1).expect("bookkeeping stays observable");
        assert!(
            matches!(event, ClaudeEvent::Unknown),
            "unexpected projection for {line}"
        );
    }

    // A usage-only assistant frame is canonical usage content, not
    // bookkeeping: the per-response sample projects a context gauge even
    // though no text streamed.
    let usage_only = parse_frame(
        r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":10}}}"#,
        2,
    )
    .expect("usage-only frame decodes");
    assert!(
        matches!(usage_only, ClaudeEvent::Usage { .. }),
        "usage without text must stay observable as usage"
    );

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
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"onward"}}}"#,
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
            None,
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

#[expect(
    clippy::assertions_on_constants,
    reason = "documents the domain answer ceiling against the engine constant"
)]
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

#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
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
        None,
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
        None,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert!(receiver.try_recv().is_err(), "no root observation");
    assert_eq!(tracker.child_frame_count(), 1);
    assert_eq!(active, None, "root turn untouched");

    // Discovery emitted exactly one validated subagent row; the textless
    // child frame projected nothing.
    let rows = tracker.take_subagent_rows();
    assert_eq!(rows.len(), 1);
    match &rows[0] {
        Observation::Subagent(obs) => {
            assert_eq!(obs.state(), SubagentState::Discovered);
            assert_eq!(obs.agent_native_thread_id().as_str(), "task-1");
            assert_eq!(obs.parent_native_thread_id().as_str(), "session-1");
            assert_eq!(
                obs.sequence(),
                ObservationSequence::new(1).expect("sequence")
            );
        }
        other => panic!("expected subagent row, got {}", other.tag()),
    }

    // A text-bearing child frame projects exactly one transcript row with
    // renderer-safe content and its own identity.
    let spoken = parse_frame(
        r#"{"type":"assistant","parent_tool_use_id":"tool-9","message":{"content":[{"type":"text","text":"child speaks"}]}}"#,
        6,
    )
    .expect("spoken child decodes");
    let outcome = apply_event(
        spoken,
        &run,
        "session-1",
        &mut tracker,
        &mut active,
        &sender,
        6,
        None,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert!(receiver.try_recv().is_err(), "no root observation");
    assert_eq!(tracker.child_frame_count(), 2);
    let rows = tracker.take_subagent_rows();
    assert_eq!(rows.len(), 1);
    match &rows[0] {
        Observation::SubagentTranscript(obs) => {
            assert_eq!(obs.agent_native_thread_id().as_str(), "tool-9");
            assert_eq!(obs.parent_native_thread_id().as_str(), "session-1");
            assert_eq!(
                obs.sequence(),
                ObservationSequence::new(6).expect("sequence")
            );
            match obs.content() {
                TranscriptContent::AgentMessageDelta(content) => {
                    assert_eq!(content.delta(), "child speaks");
                    assert_eq!(content.phase(), MessagePhase::Unspecified);
                }
                other => panic!("expected message delta, got {}", other.tag()),
            }
        }
        other => panic!("expected transcript row, got {}", other.tag()),
    }

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
        None,
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
        None,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert!(tracker.reasoning_settled());
    assert!(receiver.try_recv().is_err(), "no root observation");

    // A non-empty thinking delta counts without root text either.
    let thinking = parse_frame(
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"plan"}}}"#,
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
        None,
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
        None,
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
        None,
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
        None,
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
        r#"{"type":"control_request","request_id":"perm-10","request":{"subtype":"can_use_tool","tool_name":"Read","input":{"file_path":"a.txt"}}}"#,
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
    subagent_rows: Vec<Observation>,
    tracker: ClaudePendingTracker,
    session: Option<String>,
}

/// Drives one fixture script turn through the first user message, the init
/// gate, and the streaming pump with stall/cancel/EOF mapping, mirroring the
/// owner executor without spawning the real CLI.
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
    let script = FixtureScript::new(responses, tail);
    let mut child = script.spawn();
    let mut stdin = child.stdin.take().expect("fixture stdin");
    let stdout = child.stdout.take().expect("fixture stdout");
    let mut reader = BufReader::new(stdout);
    let shutdown = CancelHandle::new();
    let control = Arc::new(CancelHandle::new());
    let deadline = Instant::now() + Duration::from_secs(20);

    let session = ClaudeSession::Start(SESSION.to_owned());
    write_line(
        &mut stdin,
        &ClaudeSettings::user_message_line(&session, "hello"),
    )
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
    let mut subagent_rows = Vec::new();
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
                    if let Ok(event) = parse_frame(&trimmed, sequence) {
                        if let ClaudeEvent::Init { session_id } = &event {
                            session_seen = Some(session_id.clone());
                        }
                        if let ClaudeEvent::TextDelta { phase, .. } = &event {
                            phases.push(*phase);
                        }
                        match apply_event(
                            event, &run, SESSION, &mut tracker, &mut active, &sender, sequence,
                            None,
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
                        // Drain beside the text channel, mirroring the live
                        // pump: rows traverse the loop in emission order.
                        subagent_rows.extend(tracker.take_subagent_rows());
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
        subagent_rows,
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
        r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg-7"}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello "}}}"#,
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
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"kept"}}}"#,
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
    // The task discovery emitted one subagent row; the textless child frame
    // projected no transcript row.
    assert_eq!(outcome.subagent_rows.len(), 1);
    match &outcome.subagent_rows[0] {
        Observation::Subagent(obs) => {
            assert_eq!(obs.state(), SubagentState::Discovered);
            assert_eq!(obs.agent_native_thread_id().as_str(), "task-9");
            assert_eq!(obs.parent_native_thread_id().as_str(), SESSION);
        }
        other => panic!("expected subagent row, got {}", other.tag()),
    }
}

#[tokio::test]
async fn fixture_subagent_discovery_and_transcript_row_sequence() {
    let responses = format!(
        "{}\n{}\n{}\n{}\n{}\n",
        init_line(),
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"root "}}}"#,
        r#"{"type":"system","subtype":"task_started","task_id":"task-1","description":"Explore"}"#,
        r#"{"type":"assistant","parent_tool_use_id":"tool-9","message":{"content":[{"type":"text","text":"child speaks"}]}}"#,
        result_line(),
    );
    let mut outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture_turn(&responses, "", Duration::from_secs(5), None),
    )
    .await
    .expect("fixture finishes");
    assert_eq!(outcome.terminal, Some(TerminalState::Completed));
    // Child content never adopts the root turn.
    assert_eq!(joined(&outcome), "root ");
    assert_eq!(outcome.subagent_rows.len(), 2);
    match &outcome.subagent_rows[0] {
        Observation::Subagent(obs) => {
            assert_eq!(obs.state(), SubagentState::Discovered);
            assert_eq!(obs.agent_native_thread_id().as_str(), "task-1");
            assert_eq!(obs.parent_native_thread_id().as_str(), SESSION);
            assert_eq!(
                obs.sequence(),
                ObservationSequence::new(3).expect("sequence")
            );
        }
        other => panic!("expected subagent row, got {}", other.tag()),
    }
    match &outcome.subagent_rows[1] {
        Observation::SubagentTranscript(obs) => {
            assert_eq!(obs.agent_native_thread_id().as_str(), "tool-9");
            assert_eq!(obs.parent_native_thread_id().as_str(), SESSION);
            assert_eq!(
                obs.sequence(),
                ObservationSequence::new(4).expect("sequence")
            );
            match obs.content() {
                TranscriptContent::AgentMessageDelta(content) => {
                    assert_eq!(content.delta(), "child speaks");
                    assert_eq!(content.phase(), MessagePhase::Unspecified);
                }
                other => panic!("expected message delta, got {}", other.tag()),
            }
        }
        other => panic!("expected transcript row, got {}", other.tag()),
    }
    // The loop drained every row: nothing lingers in the tracker.
    assert!(outcome.tracker.take_subagent_rows().is_empty());
}

#[tokio::test]
async fn fixture_external_kill_reports_interruption() {
    let responses = format!(
        "{}\n{}\n",
        init_line(),
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"prefix "}}}"#,
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
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"warming"}}}"#,
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
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"durable-"}}}"#,
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
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"replayed"}}}"#,
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

// NOTE: a byte-identical duplicate of the test above existed at HEAD and is
// removed here so the target compiles; see the packet deviations.

// ---------------------------------------------------------------------------
// Owner channel plus dispatcher commit end to end (real repository)
// ---------------------------------------------------------------------------

fn subagent_test_engine_config() -> EngineRunConfig {
    let phase = FiniteMillis::new(1).expect("phase budget is valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget is valid"),
        readiness_budget: phase,
        health_budget: phase,
        prompt_budget: phase,
        stream_budget: phase,
        close_budget: phase,
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit is valid"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit is valid"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit is valid"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness line limit is valid"),
        max_header_count: CountLimit::new(8).expect("header count is valid"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer limit is valid"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr limit is valid"),
        observation_capacity: CountLimit::new(16).expect("observation capacity is valid"),
    })
    .expect("runtime relationships are valid");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-claude-sub").expect("permission id"),
        EngineAgentId::parse("agent-claude-sub").expect("agent id"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::Claude(
            ClaudeSelection::new(
                EngineProfileId::parse("claude-fixture").expect("profile id"),
                Some(EngineModelId::parse("model-claude-sub").expect("model id")),
                permission,
                None,
                None,
                false,
                false,
            )
            .expect("claude selection valid"),
        ),
        runtime,
    )
}

fn subagent_test_dispatcher_config() -> NativeRunDispatcherConfig {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::from_millis(10),
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_millis(10),
            queue_capacity: NonZeroUsize::new(1).expect("queue slot"),
            max_command_retries: NonZeroUsize::new(3).expect("retries"),
            prompt_delivery: "immediate".to_owned(),
            stream_after: 0,
        },
    )
    .expect("dispatcher config")
}

/// Proves one discovery plus one transcript row traverse the owner channel
/// plus the dispatcher S1b commit end to end: real owner boundary rows are
/// wrapped into channel events, cross a real channel in order, commit
/// through the real S1b batch path against a real repository with
/// sequencing, and leave root text durably untouched.
#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
#[tokio::test]
async fn fixture_subagent_rows_traverse_channel_plus_dispatcher_commit() {
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
    let repository = Repository::new(database.clone());

    let project = entities::attached_project::ActiveModel {
        project_id: Set("project-claude-sub".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    };
    entities::attached_project::Entity::insert(project)
        .exec(&database)
        .await
        .expect("project should insert");
    let thread = ThreadId::parse("thread-claude-sub").expect("thread id");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse("req-thread-claude-sub").expect("request id"),
            thread_id: thread.clone(),
            project_id: ProjectId::parse("project-claude-sub").expect("project id"),
            title: ThreadTitle::parse("Claude subagents").expect("title"),
            created_at: UnixMillis::from_millis(10),
            updated_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("thread should create");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("engine-thread-claude-sub").expect("request id"),
            thread_id: thread.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: subagent_test_engine_config(),
            accepted_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("engine configuration should create");

    let run = RunId::parse("run-claude-sub").expect("run id");
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("req-msg-claude-sub").expect("request id"),
            message_id: MessageId::parse("msg-claude-sub").expect("message id"),
            thread_id: thread.clone(),
            body: MessageBody::parse("hello").expect("body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("message should queue");
    let wall_millis = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis(),
    )
    .expect("wall millis fit");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x22; 32]),
            claimed_at: UnixMillis::from_millis(100),
            lease_expires_at: UnixMillis::from_millis(wall_millis + 60_000),
        })
        .await
        .expect("claim should read")
        .expect("message should claim");
    let turn = TurnId::parse("turn-claude-sub").expect("turn id");
    let item = ItemId::parse("item-run-claude-sub").expect("item id");
    let first_patch = PatchId::parse("patch-run-claude-sub-a").expect("patch id");
    let second_patch = PatchId::parse("patch-run-claude-sub-b").expect("patch id");
    let start_key = RunStartKey::new([0x5a; 32]);
    let credentials = RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]);
    let engine_settings = repository
        .read_thread_engine_settings(&thread)
        .await
        .expect("engine configuration should read")
        .expect("engine configuration should be present");
    let outcome = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run,
            turn_id: &turn,
            item_id: &item,
            first_patch_id: &first_patch,
            second_patch_id: &second_patch,
            operated_at: UnixMillis::from_millis(150),
            run_start_key: &start_key,
            credentials: &credentials,
            engine_settings: &engine_settings,
        })
        .await
        .expect("launch should write");
    let LaunchClaimedRunOutcome::Started(receipt) = outcome else {
        panic!("run should launch");
    };
    let binding = ProviderBindingBytes::new(
        binding_bytes_vec("claude", "claude-fixture", "session-native-1").expect("binding builds"),
    )
    .expect("binding wraps");
    let bound = match repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &receipt,
            run_start_key: &start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(150),
            bound_at: UnixMillis::from_millis(200),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await
        .expect("bind should write")
    {
        BindRunProviderOutcome::Bound(bound) | BindRunProviderOutcome::AlreadyBound(bound) => bound,
    };

    let config = subagent_test_dispatcher_config();
    let origin = SystemCommandOrigin;
    let mut cursor = SubagentCommitCursor {
        scope: RunBatchScope {
            claimed: &claimed,
            launched: &receipt,
            bound: &bound,
            run_start_key: &start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(150),
            expected_updated_at: UnixMillis::from_millis(200),
        },
        engine: EngineId::Claude,
        batch_sequence: 1,
        assistant_item: None,
        assistant_revision: Revision::new(0),
        assistant_body: "root ".to_owned(),
    };

    // Rows come from the real owner boundary: parse plus apply, then wrap
    // into channel events exactly like the pump settlement will.
    let mut tracker = ClaudePendingTracker::new();
    let (apply_sender, _) = mpsc::channel::<EngineObservation>(8);
    let mut active = None;
    for (line, sequence) in [
        (
            r#"{"type":"system","subtype":"task_started","task_id":"task-1","description":"Explore"}"#,
            3,
        ),
        (
            r#"{"type":"assistant","parent_tool_use_id":"tool-9","message":{"content":[{"type":"text","text":"child speaks"}]}}"#,
            4,
        ),
    ] {
        let event = parse_frame(line, sequence).expect("frame decodes");
        let outcome = apply_event(
            event,
            &run,
            "session-native-1",
            &mut tracker,
            &mut active,
            &apply_sender,
            sequence,
            None,
        )
        .await;
        assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    }
    let (channel_tx, mut channel_rx) = mpsc::channel::<EngineObservation>(8);
    for row in tracker.take_subagent_rows() {
        let event = match row {
            Observation::Subagent(observation) => {
                EngineObservation::Subagent(SubagentLifecycleRow::new(observation))
            }
            Observation::SubagentTranscript(observation) => {
                EngineObservation::SubagentTranscript(SubagentTranscriptRow::new(observation))
            }
            other => panic!("unexpected row kind {}", other.tag()),
        };
        channel_tx.send(event).await.expect("channel carries row");
    }
    drop(channel_tx);

    // The dispatcher commit drains the channel in order, one S1b batch per
    // row, advancing the durable chain across batches.
    let mut committed = 0;
    while let Some(event) = channel_rx.recv().await {
        let observation = match event {
            EngineObservation::Subagent(row) => Observation::Subagent(row.into_observation()),
            EngineObservation::SubagentTranscript(row) => {
                Observation::SubagentTranscript(row.into_observation())
            }
            _ => panic!("root rows never share the subagent assertions"),
        };
        assert!(
            commit_subagent_observation(&repository, &config, &origin, &mut cursor, observation)
                .await,
            "S1b commit persists row"
        );
        committed += 1;
    }
    assert_eq!(committed, 2);
    assert_eq!(cursor.batch_sequence, 3);
    assert!(cursor.assistant_item.is_some());
    assert_eq!(cursor.assistant_body, "root ");
    assert_eq!(
        repository
            .last_committed_observation_sequence(&run)
            .await
            .expect("sequence reads"),
        Some(2)
    );

    // The checkpoint blob is a latest-batch sequence cursor, not a history
    // log: every S1b observation batch commits with CheckpointUpdate::Replace
    // (see upsert_checkpoint), so each batch overwrites the blob with only
    // its own rows. Prefix history is proven durably by the per-batch
    // receipts below plus the advancing chain above (committed == 2,
    // batch_sequence == 3, sequence Some(2)). The blob therefore holds
    // exactly the latest one-row batch: the transcript row at sequence 2.
    let checkpoint = entities::run_checkpoint::Entity::find_by_id("run-claude-sub")
        .one(&database)
        .await
        .expect("checkpoint reads")
        .expect("checkpoint row");
    let decoded = artisan_database::decode_observation_checkpoint(
        checkpoint
            .engine_checkpoint_version
            .expect("checkpoint version"),
        checkpoint
            .engine_checkpoint_blob
            .as_ref()
            .expect("checkpoint blob")
            .as_slice(),
    )
    .expect("checkpoint decodes");
    assert_eq!(decoded.engine(), EngineId::Claude);
    assert_eq!(decoded.max_sequence(), Some(2));
    assert_eq!(decoded.observations().len(), 1);
    match &decoded.observations()[0] {
        Observation::SubagentTranscript(observation) => {
            assert_eq!(observation.agent_native_thread_id().as_str(), "tool-9");
            assert_eq!(
                observation.parent_native_thread_id().as_str(),
                "session-native-1"
            );
            match observation.content() {
                TranscriptContent::AgentMessageDelta(content) => {
                    assert_eq!(content.delta(), "child speaks");
                }
                other => panic!("expected message delta, got {}", other.tag()),
            }
        }
        other => panic!("expected transcript row, got {}", other.tag()),
    }

    // Both batches durably persisted: one committed receipt per batch
    // sequence, so the overwritten blob loses no committed prefix.
    for batch_sequence in [1, 2] {
        let receipt = entities::run_batch_receipt::Entity::find_by_id((
            "run-claude-sub".to_owned(),
            batch_sequence,
        ))
        .one(&database)
        .await
        .expect("receipt reads")
        .expect("batch receipt");
        assert!(receipt.committed);
    }

    // Root text committed verbatim beside the rows, never adopted.
    let items = entities::conversation_item::Entity::find()
        .all(&database)
        .await
        .expect("items read");
    let assistants: Vec<_> = items
        .iter()
        .filter(|item| item.item_kind == ConversationItemKind::AssistantMessage)
        .collect();
    assert_eq!(assistants.len(), 1);
    assert_eq!(assistants[0].body, "root ");
}

// ---------------------------------------------------------------------------
// L3: continuation gate matrix (same engine, explicit model, CLI >= 2.1.220)
// ---------------------------------------------------------------------------

fn gate_decision(
    cli_version: &str,
    target_model: Option<&str>,
    advertised_models: Option<&[&str]>,
    same_engine: bool,
) -> ClaudeContinuationDecision {
    check_claude_native_continuation(&ClaudeContinuationGateInput {
        cli_version,
        target_model,
        advertised_models,
        same_engine,
    })
}

#[test]
fn claude_continuation_gate_matrix() {
    assert_eq!(
        gate_decision("2.1.220", Some("claude-model"), None, true),
        ClaudeContinuationDecision::Compatible
    );
    assert_eq!(
        gate_decision("2.2.0", Some("claude-model"), None, true),
        ClaudeContinuationDecision::Compatible
    );
    assert_eq!(
        gate_decision("2.1.220 (Claude Code)", Some("claude-model"), None, true),
        ClaudeContinuationDecision::Compatible
    );
    assert_eq!(
        gate_decision(
            "2.1.220",
            Some("claude-model"),
            Some(&["claude-model", "other-model"]),
            true
        ),
        ClaudeContinuationDecision::Compatible
    );
    // The recorded constant is the floor the gate enforces.
    assert_eq!(CLAUDE_NATIVE_CONTINUATION_VERSION, "2.1.220");
    // Explicit target model is required before resume.
    assert!(matches!(
        gate_decision("2.1.220", None, None, true),
        ClaudeContinuationDecision::Incompatible { .. }
    ));
    assert!(matches!(
        gate_decision("2.1.220", Some(""), None, true),
        ClaudeContinuationDecision::Incompatible { .. }
    ));
    // Older releases never authorize continuation.
    for old in ["2.1.219", "2.0.0", "not a version", ""] {
        assert!(
            matches!(
                gate_decision(old, Some("claude-model"), None, true),
                ClaudeContinuationDecision::Incompatible { .. }
            ),
            "CLI {old} must not authorize continuation"
        );
    }
    // Cross-engine resume never proceeds, even with a fresh CLI and model.
    assert!(matches!(
        gate_decision("2.1.220", Some("claude-model"), None, false),
        ClaudeContinuationDecision::Incompatible { .. }
    ));
    // Advertisement is enforced only when an inventory is supplied.
    assert!(matches!(
        gate_decision(
            "2.1.220",
            Some("claude-model"),
            Some(&["other-model"]),
            true
        ),
        ClaudeContinuationDecision::Incompatible { .. }
    ));
}

#[test]
fn claude_cli_version_floor_parses_embedded_triples() {
    assert!(claude_cli_meets_minimum("claude 2.1.220", "2.1.220"));
    assert!(claude_cli_meets_minimum("2.1.220-beta+001", "2.1.220"));
    assert!(claude_cli_meets_minimum("2.2.0", "2.1.220"));
    assert!(!claude_cli_meets_minimum("claude 2.1.219", "2.1.220"));
    assert!(!claude_cli_meets_minimum("no version here", "2.1.220"));
    assert!(!claude_cli_meets_minimum("", "2.1.220"));
}

// ---------------------------------------------------------------------------
// L3: resume reopens the stored session over start flags
// ---------------------------------------------------------------------------

#[test]
fn claude_resume_reopens_the_stored_session() {
    let session = claude_resume_session("session-stored-1").expect("resume session");
    assert_eq!(session.session_id(), "session-stored-1");
    let settings = ClaudeSettings::from_selection(&claude_selection()).expect("settings valid");
    let args = settings.spawn_args(&session);
    assert!(args.contains(&"--resume".to_owned()));
    assert!(args.contains(&"session-stored-1".to_owned()));
    assert!(
        !args.contains(&"--session-id".to_owned()),
        "resume must not open a fresh session"
    );
    // Corrupt stored identities fail closed instead of resuming.
    assert!(claude_resume_session("").is_none());
    assert!(claude_resume_session(&"s".repeat(257)).is_none());
}

#[tokio::test]
async fn claude_resume_init_gate_accepts_only_the_stored_session() {
    let run = run_id();
    let mut tracker = ClaudePendingTracker::new();
    let (sender, _) = mpsc::channel(8);
    let mut active = None;
    // The reopened session announces itself: the gate opens.
    let init = parse_frame(
        r#"{"type":"system","subtype":"init","session_id":"session-stored-1"}"#,
        1,
    )
    .expect("init decodes");
    let outcome = apply_event(
        init,
        &run,
        "session-stored-1",
        &mut tracker,
        &mut active,
        &sender,
        1,
        None,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert!(tracker.init_seen());
    // Any other session fails closed instead of adopting a foreign session.
    let foreign = parse_frame(
        r#"{"type":"system","subtype":"init","session_id":"session-other"}"#,
        2,
    )
    .expect("init decodes");
    let outcome = apply_event(
        foreign,
        &run,
        "session-stored-1",
        &mut tracker,
        &mut active,
        &sender,
        2,
        None,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Terminal(TerminalState::Failed));
}

// ---------------------------------------------------------------------------
// L3: usage basis rules (cumulative totals, replacing context gauge)
// ---------------------------------------------------------------------------

#[test]
fn assistant_and_result_usage_decode_with_honest_shapes() {
    // Text plus per-response usage: delta carries the gauge beside the text.
    let event = parse_frame(
        r#"{"type":"assistant","message":{"id":"msg-1","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":100,"cache_creation_input_tokens":10,"cache_read_input_tokens":20,"output_tokens":5}}}"#,
        1,
    )
    .expect("assistant with usage decodes");
    match event {
        ClaudeEvent::TextDelta { delta, usage, .. } => {
            assert_eq!(delta, "hi");
            let sample = usage.expect("gauge travels with the delta");
            assert_eq!(sample.input, Some(100));
            assert_eq!(sample.cached_input, Some(20));
            assert_eq!(sample.output, Some(5));
            // The gauge is the response window only, never the running total.
            assert_eq!(sample.context, Some(130));
        }
        _ => panic!("expected text delta with usage"),
    }

    // Usage without text still projects: the gauge is canonical content.
    let usage_only = parse_frame(
        r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":40}}}"#,
        2,
    )
    .expect("usage-only frame decodes");
    match usage_only {
        ClaudeEvent::Usage { sample } => {
            assert_eq!(sample.input, Some(40));
            assert_eq!(sample.context, Some(40));
        }
        _ => panic!("expected usage event"),
    }

    // Terminal totals never become a gauge.
    let result = parse_frame(
        r#"{"type":"result","subtype":"success","session_id":"session-1","is_error":false,"usage":{"input_tokens":1000,"cache_read_input_tokens":200,"output_tokens":50}}"#,
        3,
    )
    .expect("result with usage decodes");
    match result {
        ClaudeEvent::TurnResult { usage, .. } => {
            let sample = usage.expect("totals travel with the result");
            assert_eq!(sample.input, Some(1000));
            assert_eq!(sample.cached_input, Some(200));
            assert_eq!(sample.output, Some(50));
            assert_eq!(sample.context, None);
        }
        _ => panic!("expected turn result with usage"),
    }

    // The typeless terminal summary carries totals the same way.
    let typeless = parse_frame(
        r#"{"is_error":false,"session_id":"session-1","usage":{"output_tokens":7}}"#,
        4,
    )
    .expect("typeless terminal with usage decodes");
    assert!(
        matches!(typeless, ClaudeEvent::TurnResult { usage: Some(_), .. }),
        "typeless usage must stay observable"
    );

    // Empty measurements stay observable without a report.
    let empty = parse_frame(
        r#"{"type":"assistant","message":{"content":[],"usage":{}}}"#,
        5,
    )
    .expect("empty usage decodes");
    assert!(matches!(empty, ClaudeEvent::Unknown));

    // Non-u64 numerics fail closed to absent instead of poisoning the turn.
    for line in [
        r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":-1}}}"#,
        r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":1.5}}}"#,
        r#"{"type":"result","subtype":"success","session_id":"s","usage":{"output_tokens":"many"}}"#,
    ] {
        let event = parse_frame(line, 6).expect("corrupt usage decodes");
        assert!(
            matches!(event, ClaudeEvent::Unknown),
            "corrupt usage must not project for {line}"
        );
    }
}

#[test]
fn claude_usage_report_is_cumulative_with_a_replacing_gauge() {
    let run = run_id();
    let thread = ThreadId::parse("thread-usage-1").expect("thread id");
    let model = EngineModelId::parse("model-usage-1").expect("model id");
    let context = ClaudeUsageContext {
        run_id: &run,
        thread_id: &thread,
        provider_session_id: "session-stored-1",
        model_id: &model,
        observed_at: UnixMillis::from_millis(7),
    };
    let usage: serde_json::Value = serde_json::from_str(
        r#"{"input_tokens":100,"cache_creation_input_tokens":10,"cache_read_input_tokens":20,"output_tokens":5}"#,
    )
    .expect("usage json");
    let sample = parse_claude_assistant_usage(&usage)
        .expect("sample parses")
        .expect("sample present");
    let report = claude_usage_report(&context, 3, &sample).expect("report builds");
    assert_eq!(report.basis(), RunUsageBasis::Cumulative);
    assert_eq!(report.provider_session_id(), "session-stored-1");
    assert_eq!(report.provider_turn_id(), None);
    assert_eq!(report.source_sequence(), 3);
    assert_eq!(report.input_tokens(), Some(100));
    assert_eq!(report.output_tokens(), Some(5));
    assert_eq!(report.cached_input_tokens(), Some(20));
    // The window gauge is the one response only, never the running total.
    assert_eq!(report.context_tokens(), Some(130));
    assert_eq!(report.context_window_tokens(), None);

    // Terminal totals carry no gauge: absent stays absent, never a wrong zero.
    let totals: serde_json::Value = serde_json::from_str(
        r#"{"input_tokens":1000,"cache_read_input_tokens":200,"output_tokens":50}"#,
    )
    .expect("totals json");
    let sample = parse_claude_result_usage(&totals)
        .expect("totals sample parses")
        .expect("totals sample present");
    let report = claude_usage_report(&context, 4, &sample).expect("totals report");
    assert_eq!(report.context_tokens(), None);

    // Zero is preserved and distinct from absent.
    let zero: serde_json::Value =
        serde_json::from_str(r#"{"input_tokens":0,"output_tokens":0}"#).expect("zero json");
    let sample = parse_claude_result_usage(&zero)
        .expect("zero sample parses")
        .expect("zero sample present");
    let report = claude_usage_report(&context, 5, &sample).expect("zero report");
    assert_eq!(report.input_tokens(), Some(0));
    assert_eq!(report.output_tokens(), Some(0));

    // Empty measurements are never reports.
    let empty: serde_json::Value = serde_json::from_str(r"{}").expect("empty json");
    assert_eq!(parse_claude_result_usage(&empty), Ok(None));
    assert_eq!(parse_claude_assistant_usage(&empty), Ok(None));
}

#[tokio::test]
async fn usage_projects_a_usage_observation_without_blocking_the_turn() {
    let event = parse_frame(
        r#"{"type":"assistant","message":{"id":"msg-9","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":100,"cache_read_input_tokens":20}}}"#,
        9,
    )
    .expect("usage decodes");
    let run = run_id();
    let attribution = ClaudeUsageAttribution {
        thread_id: ThreadId::parse("thread-usage-1").expect("thread id"),
        model_id: EngineModelId::parse("model-usage-1").expect("model id"),
    };
    let scope = ClaudeUsageScope {
        thread_id: &attribution.thread_id,
        model_id: &attribution.model_id,
        provider_session_id: "session-stored-1",
    };
    let (sender, mut receiver) = mpsc::channel(8);
    let mut tracker = ClaudePendingTracker::new();
    let mut active = None;
    let outcome = apply_event(
        event,
        &run,
        "session-stored-1",
        &mut tracker,
        &mut active,
        &sender,
        9,
        Some(&scope),
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    // Text first, gauge second, both on the shared channel.
    let EngineObservation::TextDelta(chunk) = receiver.try_recv().expect("delta observed") else {
        panic!("expected a text delta");
    };
    assert_eq!(chunk.delta(), "hi");
    let EngineObservation::Usage(observation) = receiver.try_recv().expect("usage observed") else {
        panic!("expected a usage observation");
    };
    assert_eq!(observation.report().basis(), RunUsageBasis::Cumulative);
    assert_eq!(observation.report().context_tokens(), Some(120));
    assert_eq!(observation.report().source_sequence(), 9);
    assert_eq!(
        observation.report().provider_session_id(),
        "session-stored-1"
    );

    // Without attribution the same frame is a diagnostic: text flows, no
    // usage observation, no terminal, and the turn continues.
    let event = parse_frame(
        r#"{"type":"assistant","message":{"content":[],"usage":{"input_tokens":40}}}"#,
        10,
    )
    .expect("usage decodes");
    let (sender, mut receiver) = mpsc::channel(8);
    let outcome = apply_event(
        event,
        &run,
        "session-stored-1",
        &mut tracker,
        &mut active,
        &sender,
        10,
        None,
    )
    .await;
    assert_eq!(outcome, ClaudeApplyOutcome::Continue { end_input: false });
    assert!(receiver.try_recv().is_err(), "no usage without scope");
}

// ---------------------------------------------------------------------------
// L3: /usage bucket mapping (clamp, kinds, resets) plus invocation shape
// ---------------------------------------------------------------------------

/// Fixed now for reset-clause tests: 2026-01-01T00:00:00Z.
const USAGE_AT_MS: i64 = 1_767_225_600_000;

#[test]
fn claude_cli_usage_buckets_map_with_clamp_and_kinds() {
    let text = [
        "Current session: 120% used, resets Jan 2, 3:04 pm (UTC)",
        "Current week (all models): 33% used",
        "Current week (GPT-5): 7% used, resets Feb 3, 1:02 am (UTC)",
        "Current week (GPT-5): 9% used",
    ]
    .join("\n");
    let windows = parse_claude_cli_usage_windows(&text, USAGE_AT_MS);
    assert_eq!(windows.len(), 3);

    assert_eq!(windows[0].id, "five_hour");
    assert_eq!(windows[0].kind, ClaudeQuotaWindowKind::Session);
    assert!(
        (windows[0].percent_used - 100.0).abs() < f64::EPSILON,
        "over-full gauge clamps to 100"
    );
    assert_eq!(
        windows[0].resets_at.as_deref(),
        Some("2026-01-02T15:04:00Z")
    );
    assert_eq!(windows[0].window_minutes, Some(300));
    assert_eq!(windows[0].scope, "shared");

    assert_eq!(windows[1].id, "seven_day");
    assert_eq!(windows[1].kind, ClaudeQuotaWindowKind::Weekly);
    assert!(
        (windows[1].percent_used - 33.0).abs() < f64::EPSILON,
        "in-range gauge passes through"
    );
    assert_eq!(windows[1].resets_at, None);
    assert_eq!(windows[1].scope, "shared");

    // The duplicate labeled bucket keeps the first row.
    assert_eq!(windows[2].id, "seven_day:gpt-5");
    assert_eq!(windows[2].kind, ClaudeQuotaWindowKind::Weekly);
    assert_eq!(windows[2].label.as_deref(), Some("GPT-5"));
    assert!(
        (windows[2].percent_used - 7.0).abs() < f64::EPSILON,
        "first duplicate wins"
    );
    assert_eq!(
        windows[2].resets_at.as_deref(),
        Some("2026-02-03T01:02:00Z")
    );
    assert_eq!(windows[2].scope, "model");

    // Unknown kinds are never guessed.
    assert_eq!(
        classify_claude_quota_window_kind(None),
        ClaudeQuotaWindowKind::Unknown
    );
    assert_eq!(
        classify_claude_quota_window_kind(Some(999)),
        ClaudeQuotaWindowKind::Unknown
    );
    assert_eq!(
        classify_claude_quota_window_kind(Some(300)),
        ClaudeQuotaWindowKind::Session
    );
    assert_eq!(
        classify_claude_quota_window_kind(Some(10_080)),
        ClaudeQuotaWindowKind::Weekly
    );
    assert!(
        clamp_claude_percent_used(None).abs() < f64::EPSILON,
        "absent gauge becomes 0"
    );
    assert!(
        (clamp_claude_percent_used(Some(33.5)) - 33.5).abs() < f64::EPSILON,
        "in-range gauge passes through"
    );
}

#[test]
fn claude_cli_usage_malformed_lines_yield_nothing() {
    // A named IANA zone never becomes an invented instant, but the window
    // still reports its gauge.
    let zoned = parse_claude_cli_usage_windows(
        "Current session: 42% used, resets Jan 2, 3:04 pm (CET)",
        USAGE_AT_MS,
    );
    assert_eq!(zoned.len(), 1);
    assert_eq!(zoned[0].resets_at, None);
    assert!((zoned[0].percent_used - 42.0).abs() < f64::EPSILON);

    // Malformed lines yield no windows.
    for line in [
        "Current session: used",
        "Current session: many% used",
        "Current session Vienna: 10% used",
        "Current week: 10% used",
        "Current week (): 10% used",
        "Current week (GPT-5) 10% used",
        "Last session: 10% used",
        "",
    ] {
        assert!(
            parse_claude_cli_usage_windows(line, USAGE_AT_MS).is_empty(),
            "malformed line must yield nothing: {line}"
        );
    }

    // The non-billable invocation keeps the documented argv shape.
    assert_eq!(
        claude_cli_usage_args(),
        ["-p", "/usage", "--output-format", "json"]
    );
}

#[test]
fn claude_cli_reset_clause_parses_utc_and_rejects_the_rest() {
    assert_eq!(
        parse_claude_cli_reset_at(
            "Current session: 1% used, resets Dec 31, 11:59 pm (UTC)",
            USAGE_AT_MS
        )
        .as_deref(),
        Some("2026-12-31T23:59:00Z")
    );
    // Minutes are optional; midnight rolls to hour zero.
    assert_eq!(
        parse_claude_cli_reset_at(
            "Current session: 1% used, resets Jan 2, 12 am (GMT)",
            USAGE_AT_MS
        )
        .as_deref(),
        Some("2026-01-02T00:00:00Z")
    );
    // A December now rolls a January reset into next year.
    let december_now = USAGE_AT_MS + 334_i64 * 86_400_000;
    assert_eq!(
        parse_claude_cli_reset_at(
            "Current session: 1% used, resets Jan 2, 1 pm (UT)",
            december_now
        )
        .as_deref(),
        Some("2027-01-02T13:00:00Z")
    );
    // Impossible dates, bad hours, and trailing junk never become instants.
    for line in [
        "Current session: 1% used, resets Feb 31, 1 pm (UTC)",
        "Current session: 1% used, resets Jan 2, 0 pm (UTC)",
        "Current session: 1% used, resets Jan 2, 1 xm (UTC)",
        "Current session: 1% used, resets Jan 2, 1 pm (UTC) tomorrow",
        "Current session: 1% used",
        "no reset clause here",
    ] {
        assert_eq!(
            parse_claude_cli_reset_at(line, USAGE_AT_MS),
            None,
            "must not invent an instant for {line}"
        );
    }
}

// ---------------------------------------------------------------------------
// L3: generated title capture (newest ai-title wins, total function)
// ---------------------------------------------------------------------------

#[test]
fn claude_session_title_takes_the_newest_valid_record() {
    let lines = [
        r#"{"type":"system","subtype":"init","session_id":"s"}"#,
        r#"{"type":"ai-title","aiTitle":"First name"}"#,
        "not json",
        r#"{"type":"ai-title","aiTitle":""}"#,
        r#"{"type":"ai-title","aiTitle":"Current name"}"#,
        // A corrupt newer record is skipped instead of ending the scan.
        r#"{"type":"ai-title","aiTitle":"truncated"#,
    ];
    assert_eq!(
        claude_session_title_from_lines(&lines).as_deref(),
        Some("Current name")
    );
    // No record means no title yet.
    assert_eq!(claude_session_title_from_lines(&lines[..1]), None,);
    // Oversize titles never become observations.
    let big = format!(r#"{{"type":"ai-title","aiTitle":"{}"}}"#, "t".repeat(257));
    assert_eq!(claude_session_title_from_lines(&[big.as_str()]), None);
}

#[test]
fn claude_transcript_path_shapes_config_home_segments() {
    // Per-character slug with no collapsing: `:` and each separator become
    // one dash. Mirrors `claude_project_directory_name` in
    // `modules/engines/src/claude/session-title.ts`
    // (`working_directory.replace(/[^A-Za-z0-9]/gu, "-")`).
    assert_eq!(
        claude_project_directory_name("E:\\work\\artisan"),
        "E--work-artisan"
    );
    assert_eq!(
        claude_project_directory_name("/home/sander/work"),
        "-home-sander-work"
    );
    let path = claude_session_transcript_path("/home/test/.claude", "/work/app", "session-9");
    assert_eq!(
        path,
        std::path::Path::new("/home/test/.claude")
            .join("projects")
            .join("-work-app")
            .join("session-9.jsonl"),
        "unexpected transcript path {path:?}"
    );
}

#[test]
fn claude_session_title_reads_one_transcript_total() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("artisan-claude-title-{nonce}"));
    std::fs::create_dir_all(&directory).expect("title dir");
    let transcript = directory.join("session-9.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"session-9\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"First name\"}\n",
            "broken line\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Kept name\"}\n",
        ),
    )
    .expect("transcript writes");
    assert_eq!(
        read_claude_session_title(&transcript).as_deref(),
        Some("Kept name")
    );
    // A missing transcript means no title yet, never a failure.
    assert_eq!(
        read_claude_session_title(&directory.join("missing.jsonl")),
        None
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn claude_terminal_carries_the_captured_title() {
    let mut tracker = ClaudePendingTracker::new();
    assert_eq!(tracker.summary_title(), None);
    tracker.note_summary_title("Kept name".to_owned());
    assert_eq!(tracker.summary_title(), Some("Kept name"));
    // Later captures replace earlier ones, mirroring the newest-record rule.
    tracker.note_summary_title("Newer name".to_owned());
    let terminal = terminal_observation(&run_id(), 3, TerminalState::Completed)
        .with_summary_title(tracker.summary_title().map(str::to_owned));
    assert_eq!(terminal.state(), TerminalState::Completed);
    assert_eq!(terminal.summary_title(), Some("Newer name"));
    // Engines that capture no title leave the observation unchanged.
    let bare = terminal_observation(&run_id(), 4, TerminalState::Failed);
    assert_eq!(bare.summary_title(), None);
}

// ---------------------------------------------------------------------------
// L3: teardown kills the whole group; quarantine stays on unobserved reaps
// ---------------------------------------------------------------------------

#[test]
fn claude_teardown_requires_group_termination() {
    assert!(
        claude_requires_group_termination(),
        "Windows teardown must kill the whole Job Object so no claude grandchild \
         holding a pipe is orphaned; unobserved reaps quarantine through \
         cleanup_after_abort and finish_turn_result"
    );
}

#[tokio::test]
async fn fixture_kill_reports_interruption_with_durable_prefix() {
    let responses = format!(
        "{}\n{}\n",
        init_line(),
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"prefix-"}}}"#,
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
    let mut tracker = ClaudePendingTracker::new();
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
                    // Kill the leader once the durable prefix has landed.
                    if sequence == 2 {
                        let _ = child.kill().await;
                    }
                    let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                    if let Ok(event) = parse_frame(&trimmed, sequence)
                        && let ClaudeApplyOutcome::Terminal(state) = apply_event(
                            event,
                            &run,
                            SESSION,
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
async fn fixture_restart_after_kill_replays_prefix_on_the_same_session() {
    let first = format!(
        "{}\n{}\n",
        init_line(),
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"durable-"}}}"#,
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

    // The restart resumes provider-owned state: the stored session reopens
    // through `--resume` instead of duplicating provider effects with a
    // second session, and the init gate accepts exactly that session.
    let stored = claude_resume_session(SESSION).expect("stored session resumes");
    assert_eq!(stored.session_id(), SESSION);
    let args = ClaudeSettings::from_selection(&claude_selection())
        .expect("settings valid")
        .spawn_args(&stored);
    assert!(args.contains(&"--resume".to_owned()));

    let second = format!(
        "{}\n{}\n{}\n",
        init_line(),
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"replayed"}}}"#,
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

#[tokio::test(flavor = "current_thread")]
async fn oversized_claude_line_fails_typed_before_allocation() {
    // The init gate reads through the bounded reader: an over-cap provider
    // line must fail typed while retaining no bytes in the line buffer.
    let oversized = vec![b'{'; CLAUDE_MAX_FRAME_BYTES + 1];
    let mut reader = BufReader::new(&oversized[..]);
    let mut line = String::from("stale");
    let shutdown = Arc::new(CancelHandle::new());
    let control = Arc::new(CancelHandle::new());
    let error = crate::engine_owner::operation::read_claude_line(
        &mut reader,
        &mut line,
        Instant::now() + Duration::from_secs(5),
        &shutdown,
        &control,
    )
    .await
    .expect_err("an over-cap claude line must reject");
    assert_eq!(
        error,
        crate::engine_owner::operation::EngineOperationError::FrameTooLarge
    );
    assert!(
        line.is_empty(),
        "over-cap provider bytes must never be retained"
    );
}
