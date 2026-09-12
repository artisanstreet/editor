#![allow(clippy::module_name_repetitions)]
#![allow(dead_code)]

use std::ffi::OsString;
use std::time::Duration;

use artisan_domain::{
    ApprovalKind, ApprovalMode, CursorPermissionMode, CursorReasoningEffort, CursorSelection,
    CursorSpeed, EngineAgentId, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    FilesystemAccess, NetworkAccess, PermissionId, PlanEntryStatus, WebSearchAccess,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader, split};

use super::{
    CURSOR_C1_UNPROBED_VERSION, CURSOR_ENGINE_ID, CursorLaunch, CursorSettings, CursorTurnError,
    answer_cursor_plan, check_cursor_steer, classify_cursor_startup_failure, cursor_plan_approval,
    cursor_plan_description, cursor_plan_entries, cursor_question_to_domain,
    cursor_selected_option_ids, parse_cursor_plan_request, parse_cursor_question_request,
};
use crate::engine_owner::acp::{
    AcpBounds, AcpTransport, ImageBlock, ImageMode, PromptPart, UpdateEvent, build_prompt_content,
};
use crate::engine_owner::acp_bridges::{
    PermissionOutcome, answer_permission, normalize_permission_request,
};
use crate::native_run_dispatch::{binding_bytes_vec, binding_matches_bytes};

fn permission(filesystem: FilesystemAccess) -> EnginePermissionPolicy {
    EnginePermissionPolicy::new(
        PermissionId::parse("permission-cursor").expect("permission id"),
        EngineAgentId::parse("agent-cursor").expect("agent id"),
        ApprovalMode::OnRequest,
        filesystem,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    )
}

fn cursor_selection(
    model: Option<&str>,
    effort: Option<&str>,
    speed: Option<CursorSpeed>,
    permission_mode: Option<CursorPermissionMode>,
    filesystem: FilesystemAccess,
) -> CursorSelection {
    CursorSelection::new(
        EngineProfileId::parse("cursor-fixture").expect("profile id"),
        model.map(|model| EngineModelId::parse(model).expect("model id")),
        permission(filesystem),
        effort.map(|effort| CursorReasoningEffort::parse(effort).expect("reasoning effort")),
        speed,
        permission_mode,
    )
}

fn settings(
    model: Option<&str>,
    effort: Option<&str>,
    speed: Option<CursorSpeed>,
    permission_mode: Option<CursorPermissionMode>,
    filesystem: FilesystemAccess,
) -> CursorSettings {
    CursorSettings::from_selection(&cursor_selection(
        model,
        effort,
        speed,
        permission_mode,
        filesystem,
    ))
}

fn strict_bounds() -> AcpBounds {
    AcpBounds::new(
        4096,
        16_384,
        256,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(2),
    )
    .expect("test bounds hold")
}

async fn agent_read_value(
    reader: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
) -> Option<Value> {
    let mut line = String::new();
    let count = reader.read_line(&mut line).await.expect("agent reads");
    if count == 0 {
        return None;
    }
    Some(serde_json::from_str(line.trim_end()).expect("driver frames stay valid json"))
}

async fn agent_write_line(writer: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>, line: &str) {
    use tokio::io::AsyncWriteExt as _;
    writer
        .write_all(line.as_bytes())
        .await
        .expect("agent writes");
    writer.write_all(b"\n").await.expect("agent writes");
    writer.flush().await.expect("agent flushes");
}

fn update_line(session: &str, index: u32) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": { "sessionId": session, "update": { "kind": "delta", "index": index } },
    })
    .to_string()
}

fn permission_params(tool_call_id: &str, command: &str) -> Value {
    json!({
        "toolCall": {
            "toolCallId": tool_call_id,
            "kind": "execute",
            "title": "Run tests",
            "rawInput": { "command": command, "cwd": "C:\\work" },
        },
        "options": [
            { "kind": "allow_once", "optionId": "allow-1", "label": "Allow" },
            { "kind": "reject_once", "optionId": "reject-1", "label": "Deny" },
        ],
    })
}

// -----------------------------------------------------------------------
// Definition row: model resolution, args, classifier, image-block mode
// -----------------------------------------------------------------------

/// Resolves the model actually handed to the CLI for one settings row.
fn resolved_model(settings: &CursorSettings) -> Option<String> {
    let args = settings.build_args();
    let position = args
        .iter()
        .position(|arg| arg.to_str() == Some("--model"))?;
    args.get(position + 1)
        .and_then(|model| model.to_str())
        .map(str::to_owned)
}

#[test]
fn model_resolution_matrix() {
    // No model stays absent; effort and speed never invent one. The raw
    // selection rides `launch_args` untouched.
    assert_eq!(
        settings(
            None,
            Some("high"),
            Some(CursorSpeed::Fast),
            None,
            FilesystemAccess::Workspace
        )
        .launch_args()
        .model,
        None
    );

    // Resolution lives in `cursor_build_args` (mirroring TS
    // `ResolveCursorModel` inside `CursorAcpArgs`), so the matrix asserts
    // through the resolved `--model` argv value, never the raw selection.

    // Effort appends unless the base already carries a suffix.
    assert_eq!(
        resolved_model(&settings(
            Some("composer-1"),
            Some("high"),
            None,
            None,
            FilesystemAccess::Workspace
        )),
        Some("composer-1-high".to_owned())
    );
    assert_eq!(
        resolved_model(&settings(
            Some("composer-1-high"),
            Some("low"),
            None,
            None,
            FilesystemAccess::Workspace
        )),
        Some("composer-1-high".to_owned())
    );
    assert_eq!(
        resolved_model(&settings(
            Some("composer-1-high-fast"),
            Some("low"),
            None,
            None,
            FilesystemAccess::Workspace
        )),
        Some("composer-1-high-fast".to_owned())
    );

    // Fast appends unless already present.
    assert_eq!(
        resolved_model(&settings(
            Some("composer-1"),
            Some("high"),
            Some(CursorSpeed::Fast),
            None,
            FilesystemAccess::Workspace
        )),
        Some("composer-1-high-fast".to_owned())
    );
    assert_eq!(
        resolved_model(&settings(
            Some("composer-1-fast"),
            None,
            Some(CursorSpeed::Fast),
            None,
            FilesystemAccess::Workspace
        )),
        Some("composer-1-fast".to_owned())
    );

    // Bracket models pass through untouched.
    assert_eq!(
        resolved_model(&settings(
            Some("cursor[fast]"),
            Some("high"),
            Some(CursorSpeed::Fast),
            None,
            FilesystemAccess::Workspace
        )),
        Some("cursor[fast]".to_owned())
    );
}

#[test]
fn args_matrix() {
    // Bare writable session is just the subcommand.
    assert_eq!(
        settings(None, None, None, None, FilesystemAccess::Workspace).build_args(),
        vec![OsString::from("acp")]
    );

    // Resolved model leads.
    assert_eq!(
        settings(
            Some("composer-1"),
            Some("high"),
            None,
            None,
            FilesystemAccess::Workspace
        )
        .build_args(),
        vec![
            OsString::from("--model"),
            OsString::from("composer-1-high"),
            OsString::from("acp"),
        ]
    );

    // Read-only maps to ask mode and wins over force.
    assert_eq!(
        settings(None, None, None, None, FilesystemAccess::None).build_args(),
        vec![
            OsString::from("--mode"),
            OsString::from("ask"),
            OsString::from("acp"),
        ]
    );
    assert_eq!(
        settings(
            Some("composer-1"),
            None,
            None,
            Some(CursorPermissionMode::Force),
            FilesystemAccess::None
        )
        .build_args(),
        vec![
            OsString::from("--model"),
            OsString::from("composer-1"),
            OsString::from("--mode"),
            OsString::from("ask"),
            OsString::from("acp"),
        ]
    );

    // Force maps only when writes are allowed.
    assert!(
        settings(
            None,
            None,
            None,
            Some(CursorPermissionMode::Force),
            FilesystemAccess::Workspace
        )
        .build_args()
        .contains(&OsString::from("--force"))
    );
}

#[test]
fn startup_rejection_captures_model_with_stable_code() {
    let failure = classify_cursor_startup_failure(
        "Cannot use this model: composer-1. Valid models: composer-1, composer-2",
    )
    .expect("known rejection classifies");
    assert_eq!(failure.model(), "composer-1");
    assert_eq!(failure.artisan_code(), "AE-PROVIDER-206");
    assert_eq!(failure.engine_id(), CURSOR_ENGINE_ID);
    assert_eq!(
        failure.message(),
        "Cursor does not make model composer-1 available to this account."
    );

    // Case-insensitive prefix with a line-break terminator.
    let newline = classify_cursor_startup_failure("cANNOT USE THIS MODEL:  sonar\nretry later")
        .expect("newline terminator classifies");
    assert_eq!(newline.model(), "sonar");

    // End-of-input terminator.
    let trailing = classify_cursor_startup_failure("Cannot use this model: nightly-x")
        .expect("trailing model classifies");
    assert_eq!(trailing.model(), "nightly-x");

    for silent in [
        "",
        "agent 2026.9.6-stable.1",
        "Error: not authenticated",
        "Cannot use this model:",
    ] {
        assert!(
            classify_cursor_startup_failure(silent).is_none(),
            "no rejection without a captured model: {silent:?}"
        );
    }
}

#[test]
fn definition_row_carries_cursor_shape() {
    let row = CursorSettings::definition();
    assert_eq!(row.engine_id, CURSOR_ENGINE_ID);
    assert!(row.executable.contains("agent"));
    assert_eq!(row.version_args, &["--version"][..]);
    assert_eq!(row.auth_probe_args, &["status"][..]);
    assert_eq!(row.image_mode, ImageMode::Image);
    assert_eq!(CursorSettings::image_mode(), ImageMode::Image);

    let available = ["cursor_login"];
    assert_eq!(
        (row.select_auth_method)(&available, false),
        Some("cursor_login")
    );
    assert_eq!((row.select_auth_method)(&[], false), None);
    assert!((row.is_authenticated_output)("signed in as s"));
    assert!(!(row.is_authenticated_output)("Error: not logged in"));

    // The settings project onto the same row builder the core spawns.
    let resolved = settings(
        Some("composer-1"),
        Some("high"),
        None,
        Some(CursorPermissionMode::Force),
        FilesystemAccess::Workspace,
    );
    assert_eq!(
        (row.build_args)(&resolved.launch_args()),
        resolved.build_args()
    );
}

// -----------------------------------------------------------------------
// Steer, continuation, usage honesty
// -----------------------------------------------------------------------

#[test]
fn active_steer_rejects_with_typed_unsupported_command() {
    assert_eq!(check_cursor_steer(false), Ok(()));
    assert_eq!(
        check_cursor_steer(true),
        Err(CursorTurnError::UnsupportedCommand)
    );
}

// -----------------------------------------------------------------------
// Launch identity (the C3 gate matrix lives in
// `tests/backend/engine_owner_cursor.rs`)
// -----------------------------------------------------------------------

#[test]
fn unprobed_launch_carries_profile_and_sentinel() {
    // The C1 launch carries no usage scope: profile plus sentinel only.
    // The sentinel fails the C3 certified floor, so any continuation
    // through an unprobed launch gates incompatible.
    let launch = CursorLaunch::unprobed("cursor-fixture".to_owned());
    assert_eq!(launch.profile_id(), "cursor-fixture");
    assert_eq!(launch.version(), CURSOR_C1_UNPROBED_VERSION);
}

// -----------------------------------------------------------------------
// Plan-approval extensions
// -----------------------------------------------------------------------

fn question_fixture() -> Value {
    json!({
        "toolCallId": "cursor-q-1",
        "title": "Pick",
        "questions": [
            {
                "id": "q1",
                "prompt": "Which?",
                "allowMultiple": false,
                "options": [
                    { "id": "o1", "label": "First" },
                    { "id": "o2", "label": "Second" },
                ],
            },
            {
                "id": "q2",
                "prompt": "Which tags?",
                "allowMultiple": true,
                "options": [{ "id": "t1", "label": "Tag" }],
            },
        ],
    })
}

#[test]
fn cursor_questions_map_to_domain_with_answer_identity() {
    let request = parse_cursor_question_request(&question_fixture()).expect("questions parse");
    assert_eq!(request.tool_call_id, "cursor-q-1");
    assert_eq!(request.title.as_deref(), Some("Pick"));
    assert_eq!(request.questions.len(), 2);

    let first = cursor_question_to_domain(&request, &request.questions[0]).expect("domain");
    assert_eq!(first.text, "Which?");
    assert_eq!(first.header.as_deref(), Some("Pick"));
    assert!(!first.multi_select);
    assert_eq!(first.options.as_ref().expect("options").len(), 2);

    let second = cursor_question_to_domain(&request, &request.questions[1]).expect("domain");
    assert!(second.multi_select);

    // Answers resolve by option id or label, verbatim otherwise.
    assert_eq!(
        cursor_selected_option_ids(
            &request.questions[0],
            &["o2".to_owned(), "First".to_owned(), "custom".to_owned()]
        ),
        vec!["o2".to_owned(), "o1".to_owned(), "custom".to_owned()]
    );

    for bad in [
        json!(null),
        json!({}),
        json!({ "toolCallId": "", "questions": [] }),
        json!({ "toolCallId": "x" }),
        json!({ "toolCallId": "x", "questions": [] }),
        json!({
            "toolCallId": "x",
            "questions": [{ "id": "q", "prompt": "p", "options": [] }],
        }),
        json!({
            "toolCallId": "x",
            "questions": [{ "id": "", "prompt": "p", "options": [{ "id": "o", "label": "l" }] }],
        }),
    ] {
        assert_eq!(
            parse_cursor_question_request(&bad),
            Err(CursorTurnError::Configuration),
            "question shape must fail closed: {bad}"
        );
    }
}

fn plan_fixture() -> Value {
    json!({
        "toolCallId": "cursor-plan-1",
        "name": "Plan",
        "overview": "Do things",
        "plan": "Steps to finish.",
        "todos": [
            { "id": "t1", "content": "Step one", "status": "pending" },
            { "id": "t2", "content": "Step two", "status": "in_progress" },
            { "id": "t3", "content": "Step three", "status": "completed" },
            { "id": "t4", "content": "Dropped", "status": "cancelled" },
            { "id": "", "content": "Junk", "status": "pending" },
            { "id": "t5", "content": "Weird", "status": "unknown" },
        ],
    })
}

#[test]
fn cursor_plan_maps_entries_and_action_approval() {
    let request = parse_cursor_plan_request(&plan_fixture()).expect("plan parses");
    assert_eq!(request.tool_call_id, "cursor-plan-1");
    // Malformed todos drop the TypeScript flatMap way; cancelled stays
    // parsed here and filters at emission.
    assert_eq!(request.todos.len(), 4);

    let entries = cursor_plan_entries(&request).expect("entries project");
    assert_eq!(entries.len(), 3);
    assert!(
        entries
            .iter()
            .all(|entry| entry.status() != PlanEntryStatus::Pending || entry.text() == "Step one")
    );
    assert_eq!(
        entries
            .iter()
            .map(artisan_domain::PlanEntry::status)
            .collect::<Vec<_>>(),
        vec![
            PlanEntryStatus::Pending,
            PlanEntryStatus::InProgress,
            PlanEntryStatus::Completed,
        ]
    );

    assert_eq!(cursor_plan_description(&request), "Do things");
    let (description, approval) = cursor_plan_approval(&request).expect("approval maps");
    assert_eq!(description, "Do things");
    assert_eq!(approval.reason(), Some("Steps to finish."));

    // Overview falls back to name, then to the shared prompt.
    let mut nameless = request.clone();
    nameless.overview = None;
    assert_eq!(cursor_plan_description(&nameless), "Plan");
    nameless.name = None;
    assert_eq!(cursor_plan_description(&nameless), "Approve this plan?");

    // The explicit wire outcome accepts or rejects; the turn continues.
    assert_eq!(
        answer_cursor_plan(true),
        json!({ "outcome": { "outcome": "accepted" } })
    );
    assert_eq!(
        answer_cursor_plan(false),
        json!({ "outcome": { "outcome": "rejected" } })
    );

    for bad in [
        json!(null),
        json!({}),
        json!({ "toolCallId": "x", "plan": "p" }),
        json!({ "toolCallId": "", "plan": "p", "todos": [] }),
        json!({ "toolCallId": "x", "plan": 42, "todos": [] }),
    ] {
        assert_eq!(
            parse_cursor_plan_request(&bad),
            Err(CursorTurnError::Configuration),
            "plan shape must fail closed: {bad}"
        );
    }
}

// -----------------------------------------------------------------------
// Binding tag/format round trip
// -----------------------------------------------------------------------

#[test]
fn cursor_binding_round_trip_and_mismatch() {
    let raw =
        binding_bytes_vec("cursor", "cursor-fixture", "sess-cursor-1").expect("binding builds");
    assert!(binding_matches_bytes(
        &raw,
        "cursor",
        "cursor-fixture",
        "sess-cursor-1"
    ));
    for (engine, profile, session) in [
        ("opencode2", "cursor-fixture", "sess-cursor-1"),
        ("codex", "cursor-fixture", "sess-cursor-1"),
        ("cursor", "other-profile", "sess-cursor-1"),
        ("cursor", "cursor-fixture", "other-session"),
    ] {
        assert!(
            !binding_matches_bytes(&raw, engine, profile, session),
            "mismatch must requeue"
        );
    }
    assert!(binding_bytes_vec("", "cursor-fixture", "sess-cursor-1").is_none());
    assert!(binding_bytes_vec("cursor", "", "sess-cursor-1").is_none());
    assert!(binding_bytes_vec("cursor", "cursor-fixture", "").is_none());
}

// -----------------------------------------------------------------------
// Fixture ACP turns: cursor-shaped args over the shared transport core
// -----------------------------------------------------------------------

#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end fixture turn proof; splitting would duplicate the duplex wiring"
)]
#[tokio::test(flavor = "current_thread")]
async fn fixture_cursor_start_deltas_approval_close() {
    let bounds = strict_bounds();
    let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, agent_write_half) = split(agent_io);
    let mut agent_read = BufReader::new(agent_read_half);
    let mut agent_write = agent_write_half;

    let agent = tokio::spawn(async move {
        let init = agent_read_value(&mut agent_read)
            .await
            .expect("initialize request");
        assert_eq!(
            init.get("method").and_then(Value::as_str),
            Some("initialize")
        );
        let init_id = init.get("id").cloned().expect("initialize id");
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": 1, "authMethods": [{ "id": "cursor_login" }] },
            })
            .to_string(),
        )
        .await;

        let auth = agent_read_value(&mut agent_read)
            .await
            .expect("authenticate request");
        assert_eq!(
            auth.get("method").and_then(Value::as_str),
            Some("authenticate")
        );
        let auth_id = auth.get("id").cloned().expect("authenticate id");
        agent_write_line(
            &mut agent_write,
            &json!({ "jsonrpc": "2.0", "id": auth_id, "result": {} }).to_string(),
        )
        .await;

        let new = agent_read_value(&mut agent_read)
            .await
            .expect("session/new request");
        assert_eq!(
            new.get("method").and_then(Value::as_str),
            Some("session/new")
        );
        let new_id = new.get("id").cloned().expect("session/new id");
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "id": new_id,
                "result": { "sessionId": "sess-cursor-1" },
            })
            .to_string(),
        )
        .await;

        let prompt = agent_read_value(&mut agent_read)
            .await
            .expect("session/prompt request");
        assert_eq!(
            prompt.get("method").and_then(Value::as_str),
            Some("session/prompt")
        );
        // Cursor carries images as native image blocks, never resources.
        let content = prompt
            .get("params")
            .and_then(|params| params.get("prompt"))
            .and_then(Value::as_array)
            .expect("prompt content");
        assert!(
            content.iter().any(
                |part| part.get("type").and_then(Value::as_str) == Some("image")
                    && part.get("mimeType").and_then(Value::as_str) == Some("image/png")
            ),
            "image-block mode must cross the wire"
        );
        let prompt_id = prompt.get("id").cloned().expect("prompt id");

        agent_write_line(&mut agent_write, &update_line("sess-cursor-1", 1)).await;
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "id": 50,
                "method": "session/requestPermission",
                "params": permission_params("cursor-tool-1", "cargo test"),
            })
            .to_string(),
        )
        .await;
        agent_write_line(&mut agent_write, &update_line("sess-cursor-1", 2)).await;
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "id": 51,
                "method": "session/requestPermission",
                "params": permission_params("cursor-tool-2", "cargo test"),
            })
            .to_string(),
        )
        .await;
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "id": prompt_id,
                "result": {
                    "stopReason": "completed",
                    "usage": { "inputTokens": 7, "outputTokens": 9 },
                },
            })
            .to_string(),
        )
        .await;

        let mut saw_cancel = false;
        while let Some(frame) = agent_read_value(&mut agent_read).await {
            if frame.get("method").and_then(Value::as_str) == Some("session/cancel") {
                saw_cancel = true;
            }
        }
        saw_cancel
    });

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    let init = driver.initialize().await.expect("handshake");
    assert_eq!(init.protocol_version, 1);
    let available: Vec<&str> = init.auth_methods.iter().map(String::as_str).collect();
    assert_eq!(
        (CursorSettings::definition().select_auth_method)(&available, false),
        Some("cursor_login")
    );
    driver
        .authenticate("cursor_login")
        .await
        .expect("authenticate");
    let session = driver.new_session("C:\\work").await.expect("session/new");
    assert_eq!(session.as_str(), "sess-cursor-1");

    // Cursor-shaped args: resolved model plus force, then `acp`.
    let shaped = settings(
        Some("composer-1"),
        Some("high"),
        None,
        Some(CursorPermissionMode::Force),
        FilesystemAccess::Workspace,
    );
    assert_eq!(
        shaped.build_args(),
        vec![
            OsString::from("--model"),
            OsString::from("composer-1-high"),
            OsString::from("--force"),
            OsString::from("acp"),
        ]
    );

    let image = PromptPart::Image(ImageBlock {
        id: "attach-1".to_owned(),
        name: "shot.png".to_owned(),
        media_type: "image/png".to_owned(),
        bytes: vec![1, 2, 3],
    });
    let content = build_prompt_content(ImageMode::Image, "hello", &[image], None).expect("content");
    let prompt_id = driver.prompt(&session, content).await.expect("prompt");

    let mut deltas = 0_u32;
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("first delta")
    {
        UpdateEvent::SessionUpdate(_) => deltas += 1,
        other => panic!("expected session update, got {other:?}"),
    }

    // Deny lands with no side effect while the turn continues.
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("first approval")
    {
        UpdateEvent::AgentRequest { method, params, .. } => {
            assert_eq!(method, "session/requestPermission");
            let pending = normalize_permission_request(&params).expect("normalize");
            assert_eq!(pending.provider_id(), "cursor-tool-1");
            assert_eq!(
                answer_permission(&pending, false),
                PermissionOutcome::Selected {
                    option_id: "reject-1".to_owned(),
                }
            );
        }
        other => panic!("expected agent request, got {other:?}"),
    }
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("second delta")
    {
        UpdateEvent::SessionUpdate(_) => deltas += 1,
        other => panic!("expected session update, got {other:?}"),
    }

    // Allow answers through the same durable path.
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("second approval")
    {
        UpdateEvent::AgentRequest { params, .. } => {
            let pending = normalize_permission_request(&params).expect("normalize");
            assert_eq!(pending.provider_id(), "cursor-tool-2");
            assert_eq!(
                answer_permission(&pending, true),
                PermissionOutcome::Selected {
                    option_id: "allow-1".to_owned(),
                }
            );
        }
        other => panic!("expected agent request, got {other:?}"),
    }
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("prompt result")
    {
        UpdateEvent::PromptResult(outcome) => {
            assert!(!outcome.cancelled);
            let usage = outcome.usage.expect("usage reported");
            assert_eq!(usage.input, 7);
            assert_eq!(usage.output, 9);
        }
        other => panic!("expected prompt result, got {other:?}"),
    }
    assert_eq!(deltas, 2);

    driver.cancel(&session).await.expect("cancel notify");
    driver.shutdown_writer().await.expect("lifeline close");
    drop(driver);
    assert!(agent.await.expect("agent joins"), "cancel must be observed");
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_cursor_plan_and_question_extensions_surface() {
    let bounds = strict_bounds();
    let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, mut agent_write) = split(agent_io);
    drop(agent_read_half);

    let agent = tokio::spawn(async move {
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "id": 60,
                "method": "cursor/ask_question",
                "params": question_fixture(),
            })
            .to_string(),
        )
        .await;
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "id": 61,
                "method": "cursor/create_plan",
                "params": plan_fixture(),
            })
            .to_string(),
        )
        .await;
    });

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    let session =
        crate::engine_owner::acp::SessionId::parse("sess-cursor-1", 256).expect("session");
    let prompt_id = crate::engine_owner::acp::AcpId::Number(9);

    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("question request")
    {
        UpdateEvent::AgentRequest { method, params, .. } => {
            assert_eq!(method, "cursor/ask_question");
            let request = parse_cursor_question_request(&params).expect("questions parse");
            let domain =
                cursor_question_to_domain(&request, &request.questions[0]).expect("domain");
            assert_eq!(domain.text, "Which?");
            assert_eq!(
                cursor_selected_option_ids(&request.questions[0], &["Second".to_owned()]),
                vec!["o2".to_owned()]
            );
        }
        other => panic!("expected agent request, got {other:?}"),
    }
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("plan request")
    {
        UpdateEvent::AgentRequest { method, params, .. } => {
            assert_eq!(method, "cursor/create_plan");
            let request = parse_cursor_plan_request(&params).expect("plan parses");
            assert_eq!(cursor_plan_entries(&request).expect("entries").len(), 3);
            let (description, approval) = cursor_plan_approval(&request).expect("approval maps");
            assert_eq!(description, "Do things");
            assert_eq!(approval.kind(), ApprovalKind::Action);
            assert_eq!(
                answer_cursor_plan(false),
                json!({ "outcome": { "outcome": "rejected" } })
            );
        }
        other => panic!("expected agent request, got {other:?}"),
    }
    agent.await.expect("agent joins");
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_cursor_resume_round_trip() {
    let bounds = strict_bounds();
    let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, mut agent_write) = split(agent_io);
    let mut agent_read = BufReader::new(agent_read_half);

    let agent = tokio::spawn(async move {
        let first = agent_read_value(&mut agent_read)
            .await
            .expect("session/load request");
        assert_eq!(
            first.get("method").and_then(Value::as_str),
            Some("session/load")
        );
        assert_eq!(
            first
                .get("params")
                .and_then(|params| params.get("sessionId"))
                .and_then(Value::as_str),
            Some("sess-cursor-9")
        );
        let first_id = first.get("id").cloned().expect("load id");
        agent_write_line(
            &mut agent_write,
            &json!({ "jsonrpc": "2.0", "id": first_id, "result": {} }).to_string(),
        )
        .await;

        let second = agent_read_value(&mut agent_read)
            .await
            .expect("second session/load request");
        let second_id = second.get("id").cloned().expect("load id");
        agent_write_line(
            &mut agent_write,
            &json!({ "jsonrpc": "2.0", "id": second_id, "error": { "code": -32_000 } }).to_string(),
        )
        .await;
    });

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    let session =
        crate::engine_owner::acp::SessionId::parse("sess-cursor-9", 256).expect("session");
    driver
        .load_session(&session, "C:\\work")
        .await
        .expect("resume");
    let error = driver
        .load_session(&session, "C:\\work")
        .await
        .expect_err("rejected resume");
    assert_eq!(error, crate::engine_owner::acp::AcpError::ChildFailed);
    agent.await.expect("agent joins");
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_cursor_malformed_frames_reject_without_shell() {
    use crate::engine_owner::acp::{AcpError, parse_envelope};

    assert_eq!(
        parse_envelope("not json", 4096).expect_err("malformed"),
        AcpError::MalformedEnvelope
    );
    assert_eq!(
        parse_envelope("{\"jsonrpc\":\"2.0\",\"id\":1}", 4096).expect_err("malformed"),
        AcpError::MalformedEnvelope
    );

    // A foreign session frame is skipped; the owned update still lands.
    let bounds = strict_bounds();
    let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, mut agent_write) = split(agent_io);
    drop(agent_read_half);
    let agent = tokio::spawn(async move {
        agent_write_line(&mut agent_write, &update_line("other-sess", 9)).await;
        agent_write_line(&mut agent_write, &update_line("sess-cursor-1", 1)).await;
    });

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    let session =
        crate::engine_owner::acp::SessionId::parse("sess-cursor-1", 256).expect("session");
    let prompt_id = crate::engine_owner::acp::AcpId::Number(4);
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("own update")
    {
        UpdateEvent::SessionUpdate(update) => {
            assert_eq!(update.session.as_str(), "sess-cursor-1");
        }
        other => panic!("expected session update, got {other:?}"),
    }
    agent.await.expect("agent joins");
}

#[test]
fn unavailable_model_startup_rejection_marks_unrunnable_claim() {
    // The dispatcher requeues the claim; the stable code travels with the
    // transcript diagnostic instead of a raw provider string.
    let failure = classify_cursor_startup_failure(
        "Cannot use this model: composer-1. Valid models: composer-1",
    )
    .expect("rejection classifies");
    assert_eq!(failure.model(), "composer-1");
    assert_eq!(failure.artisan_code(), "AE-PROVIDER-206");
}
