//! Finite G1 Grok lifecycle proofs without the real CLI.
//!
//! Pure coverage (settings-to-row definition matrix, version/auth/image
//! classifiers, steer policy, continuation rejection, startup-classifier
//! absence, usage honesty, binding round trip) plus fixture ACP script turns
//! through the shared core: a duplex fixture agent speaks canned JSON-RPC
//! while the test drives initialize, auth, session/new, one prompt with
//! deltas, approval deny-then-allow, a question round trip, resume via
//! session/load, cancel/close, and malformed-frame rejection with
//! grok-shaped args from [`super::grok::GrokSettings`]. No real `grok`
//! binary, no catalog flag, no frontend selection, no usage surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use artisan_domain::{
    ApprovalKind, ApprovalMode, EngineAgentId, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, FilesystemAccess, GrokPermissionMode, GrokReasoningEffort, GrokSelection,
    NetworkAccess, PermissionId, WebSearchAccess,
};
use serde_json::Value;
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt as _, BufReader, DuplexStream, ReadHalf, WriteHalf, duplex,
    split,
};

use super::acp::{
    AcpBounds, AcpError, AcpFramer, AcpId, AcpResponsePayload, AcpTransport, ImageBlock, ImageMode,
    METHOD_AUTHENTICATE, METHOD_INITIALIZE, METHOD_SESSION_CANCEL, METHOD_SESSION_LOAD,
    METHOD_SESSION_NEW, METHOD_SESSION_PROMPT, PromptPart, SessionId, UpdateEvent,
    build_prompt_content, parse_envelope, parse_prompt_result,
};
use super::acp_bridges::{
    PermissionOutcome, answer_elicitation, answer_permission, normalize_elicitation_request,
    normalize_permission_request,
};
use super::grok::{
    GROK_ELICITATION_METHOD, GROK_NATIVE_CONTINUATION_REASON, GROK_PERMISSION_METHOD,
    GROK_USAGE_UNAVAILABLE_REASON, GrokCommandError, GrokContinuationError, GrokFollowUp,
    GrokLaunch, GrokSettings, check_native_continuation, classify_startup_failure, follow_up,
    usage_surface,
};
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
        PermissionId::parse("permission-grok").expect("permission id"),
        EngineAgentId::parse("agent-grok").expect("agent id"),
        approval,
        filesystem,
        network,
        WebSearchAccess::Disabled,
    )
}

fn grok_selection() -> GrokSelection {
    GrokSelection::new(
        EngineProfileId::parse("grok-fixture").expect("profile id"),
        Some(EngineModelId::parse("grok-model").expect("model id")),
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        Some(GrokReasoningEffort::parse("high").expect("effort")),
        Some(GrokPermissionMode::parse("auto").expect("permission mode")),
    )
}

fn argv_strings(settings: &GrokSettings) -> Vec<String> {
    let definition = settings.definition();
    (definition.build_args)(&settings.launch_args())
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn grok_settings_map_selection_onto_grok_row() {
    let settings = GrokSettings::from_selection(&grok_selection());
    assert_eq!(settings.profile_id(), "grok-fixture");
    let args = settings.launch_args();
    assert_eq!(args.model.as_deref(), Some("grok-model"));
    assert_eq!(args.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(args.permission.as_deref(), Some("auto"));
    assert!(!args.speed_fast);
    assert!(args.write_access);
    let definition = settings.definition();
    assert_eq!(definition.engine_id, "grok");
    assert_eq!(definition.executable, "grok");
    assert_eq!(definition.image_mode, ImageMode::Embedded);
    assert_eq!(
        argv_strings(&settings),
        vec![
            "--no-auto-update",
            "--model",
            "grok-model",
            "--reasoning-effort",
            "high",
            "--permission-mode",
            "auto",
            "agent",
            "stdio",
        ]
    );
}

#[test]
fn grok_args_force_plan_without_write_access() {
    let selection = GrokSelection::new(
        EngineProfileId::parse("grok-readonly").expect("profile id"),
        Some(EngineModelId::parse("grok-model").expect("model id")),
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::None,
            NetworkAccess::Enabled,
        ),
        None,
        Some(GrokPermissionMode::parse("auto").expect("permission mode")),
    );
    let settings = GrokSettings::from_selection(&selection);
    // Plan mode wins over the stored `auto` spelling when writes are denied.
    assert_eq!(
        argv_strings(&settings),
        vec![
            "--no-auto-update",
            "--model",
            "grok-model",
            "--permission-mode",
            "plan",
            "agent",
            "stdio",
        ]
    );
}

#[test]
fn grok_args_permission_modes_and_minimal_shape() {
    let always = GrokSelection::new(
        EngineProfileId::parse("grok-approve").expect("profile id"),
        None,
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        None,
        Some(GrokPermissionMode::parse("always-approve").expect("permission mode")),
    );
    assert_eq!(
        argv_strings(&GrokSettings::from_selection(&always)),
        vec!["--no-auto-update", "--always-approve", "agent", "stdio",]
    );
    let minimal = GrokSelection::new(
        EngineProfileId::parse("grok-minimal").expect("profile id"),
        None,
        permission(
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
        ),
        None,
        None,
    );
    assert_eq!(
        argv_strings(&GrokSettings::from_selection(&minimal)),
        vec!["--no-auto-update", "agent", "stdio",]
    );
}

#[test]
fn grok_version_parser_matrix() {
    let definition = GrokSettings::from_selection(&grok_selection()).definition();
    assert_eq!(
        (definition.parse_version)("grok 1.2.3"),
        Some("1.2.3".to_owned())
    );
    assert_eq!(
        (definition.parse_version)("something grok 0.9.0-beta.1 done"),
        Some("0.9.0-beta.1".to_owned())
    );
    assert_eq!((definition.parse_version)("codex 1.0.0"), None);
    assert_eq!((definition.parse_version)("grok-notaversion"), None);
}

#[test]
fn grok_auth_classifier_matrix() {
    let definition = GrokSettings::from_selection(&grok_selection()).definition();
    let available = ["xai.api_key", "cached_token"];
    assert_eq!(
        (definition.select_auth_method)(&available, true),
        Some("xai.api_key")
    );
    assert_eq!(
        (definition.select_auth_method)(&available, false),
        Some("cached_token")
    );
    assert_eq!((definition.select_auth_method)(&["other"], false), None);
    assert!((definition.is_authenticated_output)(
        "grok --no-auto-update models\nok"
    ));
    assert!(!(definition.is_authenticated_output)(
        "Error: not authenticated. Run grok login first."
    ));
}

#[test]
fn grok_image_mode_embeds_attachments() {
    let definition = GrokSettings::from_selection(&grok_selection()).definition();
    let image = PromptPart::Image(ImageBlock {
        id: "a1".to_owned(),
        name: "shot.png".to_owned(),
        media_type: "image/png".to_owned(),
        bytes: vec![1, 2, 3],
    });
    let content = build_prompt_content(
        definition.image_mode,
        "hi",
        std::slice::from_ref(&image),
        None,
    )
    .expect("embedded content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "resource");
    assert_eq!(
        content[0]["resource"]["uri"],
        "artisan://attachment/a1/shot.png"
    );
    let bare = PromptPart::Image(ImageBlock {
        id: String::new(),
        name: "shot.png".to_owned(),
        media_type: "image/png".to_owned(),
        bytes: vec![1],
    });
    assert!(matches!(
        build_prompt_content(
            definition.image_mode,
            "hi",
            std::slice::from_ref(&bare),
            None
        ),
        Err(AcpError::InvalidContent)
    ));
}

#[test]
fn waiting_follow_up_is_new_prompt_active_steer_rejected() {
    assert_eq!(follow_up(false), Ok(GrokFollowUp::NewPrompt));
    assert!(matches!(
        follow_up(true),
        Err(GrokCommandError::UnsupportedCommand { command: "steer" })
    ));
}

#[test]
fn native_continuation_always_unsupported() {
    match check_native_continuation() {
        Err(GrokContinuationError::Unsupported { reason }) => {
            assert_eq!(reason, GROK_NATIVE_CONTINUATION_REASON);
        }
        Ok(()) => panic!("native continuation must stay unsupported"),
    }
}

#[test]
fn startup_classifier_absent_and_usage_unsupported() {
    assert_eq!(classify_startup_failure("initialize", "boom"), None);
    assert_eq!(usage_surface().reason(), GROK_USAGE_UNAVAILABLE_REASON);
}

#[test]
fn grok_launch_carries_probe_identity() {
    let launch = GrokLaunch::new(
        PathBuf::from("/usr/bin/grok"),
        EngineProfileId::parse("grok-fixture").expect("profile id"),
        "1.2.3".to_owned(),
    );
    assert_eq!(launch.profile_id().as_str(), "grok-fixture");
    assert_eq!(launch.executable_path(), Path::new("/usr/bin/grok"));
    assert_eq!(launch.version(), "1.2.3");
}

#[test]
fn binding_tag_grok_format_one_round_trip() {
    let raw = binding_bytes_vec("grok", "grok-fixture", "sess-grok-1").expect("binding bytes");
    assert!(binding_matches_bytes(
        &raw,
        "grok",
        "grok-fixture",
        "sess-grok-1"
    ));
    assert!(!binding_matches_bytes(
        &raw,
        "codex",
        "grok-fixture",
        "sess-grok-1"
    ));
    let foreign = binding_bytes_vec("codex", "grok-fixture", "sess-grok-1").expect("foreign bytes");
    assert!(!binding_matches_bytes(
        &foreign,
        "grok",
        "grok-fixture",
        "sess-grok-1"
    ));
}

#[test]
fn prompt_result_cancelled_maps() {
    let payload = AcpResponsePayload::Result(serde_json::json!({ "stopReason": "cancelled" }));
    let outcome = parse_prompt_result(&payload).expect("cancelled parses");
    assert!(outcome.cancelled);
    let failed = AcpResponsePayload::Error { code: 7 };
    assert!(matches!(
        parse_prompt_result(&failed),
        Err(AcpError::ChildFailed)
    ));
}

// ---------------------------------------------------------------------------
// Fixture ACP scripts through the shared core
// ---------------------------------------------------------------------------

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

async fn agent_read_value(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Option<Value> {
    let mut line = String::new();
    let count = reader.read_line(&mut line).await.expect("agent reads");
    if count == 0 {
        return None;
    }
    Some(serde_json::from_str(line.trim_end()).expect("driver frames stay valid json"))
}

async fn agent_write_line(writer: &mut WriteHalf<DuplexStream>, line: &str) {
    writer
        .write_all(line.as_bytes())
        .await
        .expect("agent writes");
    writer.write_all(b"\n").await.expect("agent writes");
    writer.flush().await.expect("agent flushes");
}

fn update_line(session: &str, index: u32) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": { "sessionId": session, "update": { "kind": "delta", "index": index } },
    })
    .to_string()
}

fn provider_id_of(id: &AcpId) -> String {
    match id {
        AcpId::Number(number) => number.to_string(),
        AcpId::Text(text) => text.clone(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_grok_start_deltas_approval_question_cancel_close() {
    let settings = GrokSettings::from_selection(&grok_selection());
    assert_eq!(
        argv_strings(&settings),
        [
            "--no-auto-update",
            "--model",
            "grok-model",
            "--reasoning-effort",
            "high",
            "--permission-mode",
            "auto",
            "agent",
            "stdio",
        ]
    );
    let bounds = strict_bounds();
    let (driver_io, agent_io) = duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, agent_write_half) = split(agent_io);
    let mut agent_read = BufReader::new(agent_read_half);
    let mut agent_write = agent_write_half;
    let (cancel_seen_tx, cancel_seen_rx) = tokio::sync::oneshot::channel::<bool>();
    let (eof_seen_tx, eof_seen_rx) = tokio::sync::oneshot::channel::<bool>();

    let agent = tokio::spawn(async move {
        let init = agent_read_value(&mut agent_read)
            .await
            .expect("initialize request");
        assert_eq!(
            init.get("method").and_then(Value::as_str),
            Some(METHOD_INITIALIZE)
        );
        let init_id = init.get("id").cloned().expect("initialize id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": {
                    "protocolVersion": 1,
                    "authMethods": [{ "id": "xai.api_key" }, { "id": "cached_token" }],
                },
            })
            .to_string(),
        )
        .await;

        let auth = agent_read_value(&mut agent_read)
            .await
            .expect("authenticate request");
        assert_eq!(
            auth.get("method").and_then(Value::as_str),
            Some(METHOD_AUTHENTICATE)
        );
        assert_eq!(
            auth.pointer("/params/methodId").and_then(Value::as_str),
            Some("xai.api_key")
        );
        let auth_id = auth.get("id").cloned().expect("authenticate id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({ "jsonrpc": "2.0", "id": auth_id, "result": {} }).to_string(),
        )
        .await;

        let new = agent_read_value(&mut agent_read)
            .await
            .expect("session/new request");
        assert_eq!(
            new.get("method").and_then(Value::as_str),
            Some(METHOD_SESSION_NEW)
        );
        let new_id = new.get("id").cloned().expect("session/new id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": new_id,
                "result": { "sessionId": "sess-grok-1" },
            })
            .to_string(),
        )
        .await;

        let prompt = agent_read_value(&mut agent_read)
            .await
            .expect("session/prompt request");
        assert_eq!(
            prompt.get("method").and_then(Value::as_str),
            Some(METHOD_SESSION_PROMPT)
        );
        assert_eq!(
            prompt.pointer("/params/sessionId").and_then(Value::as_str),
            Some("sess-grok-1")
        );
        assert_eq!(
            prompt
                .pointer("/params/prompt/0/text")
                .and_then(Value::as_str),
            Some("hello grok")
        );
        let prompt_id = prompt.get("id").cloned().expect("prompt id");
        agent_write_line(&mut agent_write, &update_line("sess-grok-1", 1)).await;
        agent_write_line(&mut agent_write, &update_line("sess-other", 9)).await;
        agent_write_line(&mut agent_write, &update_line("sess-grok-1", 2)).await;
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "perm-1",
                "method": "session/requestPermission",
                "params": {
                    "toolCall": {
                        "toolCallId": "tool-grok-1",
                        "kind": "execute",
                        "title": "Run tests",
                        "rawInput": { "command": "cargo test", "cwd": "C:\\work" },
                    },
                    "options": [
                        { "kind": "allow_once", "optionId": "allow-grok-1", "label": "Allow" },
                        { "kind": "reject_once", "optionId": "reject-grok-1", "label": "Deny" },
                    ],
                },
            })
            .to_string(),
        )
        .await;
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "elicit-1",
                "method": "elicitation/create",
                "params": {
                    "mode": "form",
                    "message": "Pick",
                    "requestedSchema": {
                        "properties": {
                            "color": {
                                "type": "string",
                                "title": "Color",
                                "oneOf": ["red", "green"],
                            },
                        },
                    },
                },
            })
            .to_string(),
        )
        .await;
        // The fixture answers nothing live (G1 never auto-answers); the
        // script settles the round so the driver proves terminal mapping.
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": prompt_id,
                "result": {
                    "stopReason": "completed",
                    "usage": { "inputTokens": 7, "outputTokens": 11 },
                },
            })
            .to_string(),
        )
        .await;

        let mut saw_cancel = false;
        while let Some(frame) = agent_read_value(&mut agent_read).await {
            if frame.get("method").and_then(Value::as_str) == Some(METHOD_SESSION_CANCEL) {
                saw_cancel = true;
            }
        }
        let _ignored = cancel_seen_tx.send(saw_cancel);
        let _ignored = eof_seen_tx.send(true);
    });

    let definition = settings.definition();
    let mut driver = AcpTransport::new(driver_read, driver_write, bounds);
    let init = driver.initialize().await.expect("handshake");
    assert_eq!(init.protocol_version, 1);
    let available: Vec<&str> = init.auth_methods.iter().map(String::as_str).collect();
    assert_eq!(
        (definition.select_auth_method)(&available, true),
        Some("xai.api_key")
    );
    driver
        .authenticate("xai.api_key")
        .await
        .expect("authenticate");
    let session = driver.new_session("C:\\work").await.expect("session/new");
    assert_eq!(session.as_str(), "sess-grok-1");
    let content = build_prompt_content(definition.image_mode, "hello grok", &[], None)
        .expect("prompt content");
    let prompt_id = driver.prompt(&session, content).await.expect("prompt");
    for expected in [1, 2] {
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("session update")
        {
            UpdateEvent::SessionUpdate(update) => {
                assert_eq!(update.value()["index"], serde_json::json!(expected));
            }
            other => panic!("expected session update, got {other:?}"),
        }
    }
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("permission request")
    {
        UpdateEvent::AgentRequest { id, method, params } => {
            assert_eq!(method, GROK_PERMISSION_METHOD);
            assert_eq!(provider_id_of(&id), "perm-1");
            let pending = normalize_permission_request(&params).expect("permission normalizes");
            assert_eq!(pending.provider_id(), "tool-grok-1");
            assert_eq!(pending.description(), "Run tests");
            assert_eq!(pending.request().kind(), ApprovalKind::Command);
            assert_eq!(pending.request().command_text(), Some("cargo test"));
            assert_eq!(
                answer_permission(&pending, false),
                PermissionOutcome::Selected {
                    option_id: "reject-grok-1".to_owned(),
                }
            );
            assert_eq!(
                answer_permission(&pending, true),
                PermissionOutcome::Selected {
                    option_id: "allow-grok-1".to_owned(),
                }
            );
        }
        other => panic!("expected permission request, got {other:?}"),
    }
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("elicitation request")
    {
        UpdateEvent::AgentRequest { id, method, params } => {
            assert_eq!(method, GROK_ELICITATION_METHOD);
            let pending = normalize_elicitation_request(provider_id_of(&id).as_str(), &params)
                .expect("elicitation normalizes");
            assert_eq!(pending.provider_id(), "elicit-1");
            assert_eq!(pending.questions().len(), 1);
            let answers = BTreeMap::from([("color".to_owned(), vec!["green".to_owned()])]);
            assert_eq!(
                answer_elicitation(&pending, &answers).expect("encode"),
                serde_json::json!({ "color": "green" })
            );
        }
        other => panic!("expected elicitation request, got {other:?}"),
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
            assert_eq!(usage.output, 11);
        }
        other => panic!("expected prompt result, got {other:?}"),
    }
    driver.cancel(&session).await.expect("cancel notify");
    driver.shutdown_writer().await.expect("lifeline close");
    drop(driver);
    assert!(cancel_seen_rx.await.expect("cancel report"));
    assert!(eof_seen_rx.await.expect("eof report"));
    agent.await.expect("agent joins");
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_grok_resume_via_session_load() {
    let bounds = strict_bounds();
    let (driver_io, agent_io) = duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, agent_write_half) = split(agent_io);
    let mut agent_read = BufReader::new(agent_read_half);
    let mut agent_write = agent_write_half;

    let agent = tokio::spawn(async move {
        let init = agent_read_value(&mut agent_read)
            .await
            .expect("initialize request");
        let init_id = init.get("id").cloned().expect("initialize id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "result": { "protocolVersion": 1, "authMethods": [{ "id": "cached_token" }] },
            })
            .to_string(),
        )
        .await;
        let auth = agent_read_value(&mut agent_read)
            .await
            .expect("authenticate request");
        let auth_id = auth.get("id").cloned().expect("authenticate id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({ "jsonrpc": "2.0", "id": auth_id, "result": {} }).to_string(),
        )
        .await;
        let new = agent_read_value(&mut agent_read)
            .await
            .expect("session/new request");
        let new_id = new.get("id").cloned().expect("session/new id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": new_id,
                "result": { "sessionId": "sess-resume-1" },
            })
            .to_string(),
        )
        .await;
        let load = agent_read_value(&mut agent_read)
            .await
            .expect("session/load request");
        assert_eq!(
            load.get("method").and_then(Value::as_str),
            Some(METHOD_SESSION_LOAD)
        );
        assert_eq!(
            load.pointer("/params/sessionId").and_then(Value::as_str),
            Some("sess-resume-1")
        );
        let load_id = load.get("id").cloned().expect("session/load id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({ "jsonrpc": "2.0", "id": load_id, "result": {} }).to_string(),
        )
        .await;
        let prompt = agent_read_value(&mut agent_read)
            .await
            .expect("session/prompt request");
        let prompt_id = prompt.get("id").cloned().expect("prompt id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": prompt_id,
                "result": { "stopReason": "completed" },
            })
            .to_string(),
        )
        .await;
        while agent_read_value(&mut agent_read).await.is_some() {}
    });

    let mut driver = AcpTransport::new(driver_read, driver_write, bounds);
    driver.initialize().await.expect("handshake");
    driver.authenticate("cached_token").await.expect("auth");
    let session = driver.new_session("C:\\work").await.expect("new");
    assert_eq!(session.as_str(), "sess-resume-1");
    let resumed = SessionId::parse("sess-resume-1", 256).expect("resume identity");
    driver
        .load_session(&resumed, "C:\\work")
        .await
        .expect("session/load");
    let content =
        build_prompt_content(ImageMode::Embedded, "again", &[], None).expect("prompt content");
    let prompt_id = driver.prompt(&resumed, content).await.expect("prompt");
    match driver
        .next_update(&resumed, &prompt_id)
        .await
        .expect("prompt result")
    {
        UpdateEvent::PromptResult(outcome) => {
            assert!(!outcome.cancelled);
            assert_eq!(outcome.usage, None);
        }
        other => panic!("expected prompt result, got {other:?}"),
    }
    driver.shutdown_writer().await.expect("lifeline close");
    drop(driver);
    agent.await.expect("agent joins");
}

#[test]
fn fixture_grok_malformed_frames_rejected() {
    assert!(matches!(
        parse_envelope("{nope", 1_024),
        Err(AcpError::MalformedEnvelope)
    ));
    assert!(matches!(
        parse_envelope(&"x".repeat(32), 16),
        Err(AcpError::EnvelopeTooLarge)
    ));
    let mut framer = AcpFramer::new(8);
    assert!(matches!(
        framer.push(b"123456789"),
        Err(AcpError::LineTooLong)
    ));
    assert!(matches!(framer.push(b"more"), Err(AcpError::Poisoned)));
    let mut closed = AcpFramer::new(64);
    closed.finish().expect("finish");
    assert!(matches!(closed.push(b"x\n"), Err(AcpError::Poisoned)));
    assert!(matches!(
        SessionId::parse("", 256),
        Err(AcpError::MalformedEnvelope)
    ));
    assert_eq!(
        SessionId::parse("ok-id_1", 256)
            .expect("valid session")
            .as_str(),
        "ok-id_1"
    );
}
