//! Finite G3 Grok lifecycle proofs without the real CLI.
//!
//! Pure coverage (settings-to-row definition matrix, version/auth/image
//! classifiers, steer policy, continuation gate, startup-classifier absence,
//! binding round trip) plus fixture ACP script turns through the shared
//! core: a duplex fixture agent speaks canned JSON-RPC while the test drives
//! initialize, auth, session/new (or `session/load` for a gated resume of
//! the same conversation id), one prompt with deltas, approval
//! deny-then-allow, a question round trip, per-round usage projection,
//! cancel/close, and malformed-frame rejection with grok-shaped args from
//! [`super::grok::GrokSettings`]. Real-child tests prove the Windows
//! kill-tree (no orphaned pipe-holding grandchild, bounded return) and the
//! retained quarantine path. No real `grok` binary, no catalog flag, no
//! frontend selection, no model inventory.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use artisan_domain::{
    ApprovalKind, ApprovalMode, EngineAgentId, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, FilesystemAccess, GrokPermissionMode, GrokReasoningEffort, GrokSelection,
    NetworkAccess, PermissionId, RunId, RunUsageBasis, ThreadId, UnixMillis, WebSearchAccess,
};
use serde_json::Value;
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt as _, BufReader, DuplexStream, ReadHalf, WriteHalf, duplex,
    split,
};
use tokio::sync::mpsc;

use super::acp::{
    AcpBounds, AcpError, AcpFramer, AcpId, AcpResponsePayload, AcpShutdown, AcpTransport,
    ImageBlock, ImageMode, METHOD_AUTHENTICATE, METHOD_INITIALIZE, METHOD_SESSION_CANCEL,
    METHOD_SESSION_LOAD, METHOD_SESSION_NEW, METHOD_SESSION_PROMPT, PromptPart, SessionId,
    TokenUsage, UpdateEvent, build_prompt_content, parse_envelope, parse_grok_version,
    parse_prompt_result, shutdown_acp_child, spawn_acp_child,
};
use super::acp_bridges::{
    PermissionOutcome, answer_elicitation, answer_permission, normalize_elicitation_request,
    normalize_permission_request,
};
use super::grok::{
    GROK_ELICITATION_METHOD, GROK_PERMISSION_METHOD, GrokCommandError, GrokContinuationDecision,
    GrokContinuationGateInput, GrokFollowUp, GrokLaunch, GrokQuotaWindowKind, GrokSettings,
    GrokUsageContext, GrokUsageSample, GrokUsageScope, check_grok_native_continuation,
    clamp_grok_percent_used, classify_grok_quota_window_kind, classify_startup_failure,
    compare_grok_cli_versions, follow_up, grok_cli_version_recorded, grok_loaded_session_is_stored,
    grok_requires_group_termination, grok_reset_at_iso, grok_resume_session_id,
    grok_sample_from_token_usage, grok_usage_report, map_grok_quota_windows,
    project_grok_usage_sample,
};
use super::observation::{EngineObservation, TerminalState};
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
    let definition = GrokSettings::definition();
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
    let definition = GrokSettings::definition();
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
    let definition = GrokSettings::definition();
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
    let definition = GrokSettings::definition();
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
    let definition = GrokSettings::definition();
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
fn grok_continuation_gate_matrix() {
    let gate = |cli_version: &str,
                target_model: Option<&str>,
                advertised_models: Option<&[&str]>,
                same_engine: bool| {
        check_grok_native_continuation(&GrokContinuationGateInput {
            cli_version,
            target_model,
            advertised_models,
            same_engine,
        })
    };
    assert_eq!(
        gate("grok 1.2.3", Some("grok-model"), None, true),
        GrokContinuationDecision::Compatible
    );
    assert_eq!(
        gate("1.2.4", Some("grok-model"), None, true),
        GrokContinuationDecision::Compatible
    );
    assert_eq!(
        gate(
            "grok 1.2.3",
            Some("grok-model"),
            Some(&["grok-model", "other-model"]),
            true
        ),
        GrokContinuationDecision::Compatible
    );
    // Explicit target model is pre-validated before resume.
    assert!(matches!(
        gate("grok 1.2.3", None, None, true),
        GrokContinuationDecision::Incompatible { .. }
    ));
    assert!(matches!(
        gate("grok 1.2.3", Some(""), None, true),
        GrokContinuationDecision::Incompatible { .. }
    ));
    // The recorded CLI version must parse through the shared ACP row.
    for unrecorded in ["", "not a version", "codex 1.0.0"] {
        assert!(
            matches!(
                gate(unrecorded, Some("grok-model"), None, true),
                GrokContinuationDecision::Incompatible { .. }
            ),
            "CLI {unrecorded} must not authorize continuation"
        );
    }
    // Cross-engine resume never proceeds, even with a recorded CLI and model.
    assert!(matches!(
        gate("grok 1.2.3", Some("grok-model"), None, false),
        GrokContinuationDecision::Incompatible { .. }
    ));
    // Advertisement is enforced only when an inventory is supplied.
    assert!(matches!(
        gate(
            "grok 1.2.3",
            Some("grok-model"),
            Some(&["other-model"]),
            true
        ),
        GrokContinuationDecision::Incompatible { .. }
    ));
}

#[test]
fn grok_cli_version_gate_parses_recorded_spellings() {
    // The dispatcher seats the bare triple the row extracts; the gate must
    // accept exactly that recorded form.
    let recorded = parse_grok_version("grok 1.2.3").expect("probe output parses");
    assert_eq!(recorded, "1.2.3");
    assert!(grok_cli_version_recorded(&recorded));
    assert!(grok_cli_version_recorded("grok 1.2.3"));
    assert!(grok_cli_version_recorded("GROK 0.9.0-beta.1"));
    assert!(!grok_cli_version_recorded(""));
    assert!(!grok_cli_version_recorded("codex 1.0.0"));
    assert!(!grok_cli_version_recorded("not a version"));
    assert!(compare_grok_cli_versions("grok 1.2.3", "1.2.3").is_some());
    assert!(compare_grok_cli_versions("no version", "1.2.3").is_none());
}

#[test]
fn grok_resume_reopens_the_same_conversation_id() {
    assert_eq!(
        grok_resume_session_id("sess-grok-1").as_deref(),
        Some("sess-grok-1")
    );
    // Corrupt stored identities fail closed instead of resuming.
    assert_eq!(grok_resume_session_id(""), None);
    assert_eq!(grok_resume_session_id(&"s".repeat(257)), None);
    // The prepared identity must equal the stored one: resume reopens the
    // same conversation id and never adopts a foreign session.
    assert!(grok_loaded_session_is_stored("sess-grok-1", "sess-grok-1"));
    assert!(!grok_loaded_session_is_stored("sess-other", "sess-grok-1"));
    assert!(!grok_loaded_session_is_stored("", "sess-grok-1"));
}

#[test]
fn startup_classifier_absent() {
    assert_eq!(classify_startup_failure("initialize", "boom"), None);
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

fn run_id() -> RunId {
    RunId::parse("grok-run-1").expect("run id")
}

#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
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

    let definition = GrokSettings::definition();
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
                assert_eq!(update.update.value()["index"], serde_json::json!(expected));
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

#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
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

// ---------------------------------------------------------------------------
// G3: per-round usage is a delta (basis honesty per provider disclosure)
// ---------------------------------------------------------------------------

#[test]
fn grok_usage_sample_is_a_per_round_delta() {
    let sample = grok_sample_from_token_usage(&TokenUsage {
        input: 7,
        output: 11,
        cached_input: None,
    })
    .expect("nonzero round reports");
    assert_eq!(sample.input, Some(7));
    assert_eq!(sample.output, Some(11));
    assert_eq!(sample.cached_input, None);
    // A zero round is a diagnostic, never a report.
    assert_eq!(
        grok_sample_from_token_usage(&TokenUsage {
            input: 0,
            output: 0,
            cached_input: None,
        }),
        None
    );

    let run = run_id();
    let thread = ThreadId::parse("thread-grok-usage").expect("thread id");
    let model = EngineModelId::parse("grok-model").expect("model id");
    let context = GrokUsageContext {
        run_id: &run,
        thread_id: &thread,
        provider_session_id: "sess-grok-1",
        model_id: &model,
        observed_at: UnixMillis::from_millis(7),
    };
    let report = grok_usage_report(&context, 3, &sample).expect("report builds");
    // ACP discloses per-round counters: delta basis, no turn identity, and
    // no context gauge is synthesized.
    assert_eq!(report.basis(), RunUsageBasis::Delta);
    assert_eq!(report.provider_session_id(), "sess-grok-1");
    assert_eq!(report.provider_turn_id(), None);
    assert_eq!(report.source_sequence(), 3);
    assert_eq!(report.input_tokens(), Some(7));
    assert_eq!(report.output_tokens(), Some(11));
    assert_eq!(report.context_tokens(), None);
    assert_eq!(report.context_window_tokens(), None);
}

#[tokio::test(flavor = "current_thread")]
async fn project_grok_usage_sample_emits_best_effort() {
    let run = run_id();
    let thread = ThreadId::parse("thread-grok-usage").expect("thread id");
    let model = EngineModelId::parse("grok-model").expect("model id");
    let scope = GrokUsageScope {
        thread_id: &thread,
        model_id: &model,
        provider_session_id: "sess-grok-1",
    };
    let sample = GrokUsageSample {
        input: Some(3),
        cached_input: None,
        output: Some(5),
    };
    let (sender, mut receiver) = mpsc::channel(8);
    let terminal = project_grok_usage_sample(&sender, &run, Some(&scope), 1, &sample).await;
    assert_eq!(terminal, None);
    let EngineObservation::Usage(usage) = receiver.try_recv().expect("usage observed") else {
        panic!("expected usage observation");
    };
    assert_eq!(usage.report().basis(), RunUsageBasis::Delta);
    assert_eq!(usage.report().input_tokens(), Some(3));
    assert_eq!(usage.report().output_tokens(), Some(5));
    // No scope: skipped without disturbing the turn.
    assert_eq!(
        project_grok_usage_sample(&sender, &run, None, 2, &sample).await,
        None
    );
    assert!(receiver.try_recv().is_err());
    // Closed sink: interruption is terminal.
    drop(receiver);
    assert_eq!(
        project_grok_usage_sample(&sender, &run, Some(&scope), 3, &sample).await,
        Some(TerminalState::Interrupted)
    );
}

// ---------------------------------------------------------------------------
// G3: quota-budget windows classify with clamping, kinds stay diagnostics
// ---------------------------------------------------------------------------

#[test]
fn grok_quota_windows_classify_clamp_and_stay_diagnostics() {
    assert_eq!(
        classify_grok_quota_window_kind(Some(300)),
        GrokQuotaWindowKind::Session
    );
    assert_eq!(
        classify_grok_quota_window_kind(Some(10_080)),
        GrokQuotaWindowKind::Weekly
    );
    assert_eq!(
        classify_grok_quota_window_kind(Some(43_200)),
        GrokQuotaWindowKind::Monthly
    );
    assert_eq!(
        classify_grok_quota_window_kind(None),
        GrokQuotaWindowKind::Unknown
    );
    assert_eq!(
        classify_grok_quota_window_kind(Some(60)),
        GrokQuotaWindowKind::Unknown
    );
    assert!(
        (clamp_grok_percent_used(Some(150.0)) - 100.0).abs() < 1e-9,
        "over-range gauge clamps to 100"
    );
    assert!(
        clamp_grok_percent_used(Some(-5.0)).abs() < 1e-9,
        "negative gauge clamps to 0"
    );
    assert!(
        clamp_grok_percent_used(None).abs() < 1e-9,
        "absent gauge becomes 0"
    );
    assert!(
        clamp_grok_percent_used(Some(f64::NAN)).abs() < 1e-9,
        "non-finite gauge becomes 0"
    );
    assert!(
        clamp_grok_percent_used(Some(f64::INFINITY)).abs() < 1e-9,
        "infinite gauge becomes 0"
    );
    assert_eq!(grok_reset_at_iso(0), "1970-01-01T00:00:00Z");

    let result = serde_json::json!({
        "rateLimitsByLimitId": {
            "grok-plan": {
                "limitName": "Grok Plan",
                "primary": {
                    "windowDurationMins": 300,
                    "usedPercent": 12.5,
                    "resetsAt": 1_700_000_000,
                },
                "secondary": null,
            },
            "grok": {
                "primary": { "windowDurationMins": 43_200, "usedPercent": 250.0 },
            },
        },
    });
    let windows = map_grok_quota_windows(&result);
    assert_eq!(windows.len(), 2);
    let modeled = windows
        .iter()
        .find(|window| window.id == "grok-plan:primary")
        .expect("modeled window");
    assert_eq!(modeled.kind, GrokQuotaWindowKind::Session);
    assert_eq!(modeled.label.as_deref(), Some("Grok Plan"));
    assert!(
        (modeled.percent_used - 12.5).abs() < 1e-9,
        "in-range gauge passes through"
    );
    assert_eq!(modeled.resets_at.as_deref(), Some("2023-11-14T22:13:20Z"));
    assert_eq!(modeled.scope, "model");
    let clamped = windows
        .iter()
        .find(|window| window.id == "grok:primary")
        .expect("clamped window");
    assert_eq!(clamped.kind, GrokQuotaWindowKind::Monthly);
    assert!(
        (clamped.percent_used - 100.0).abs() < 1e-9,
        "over-range gauge clamps to 100"
    );
    assert_eq!(clamped.scope, "unknown");
    // Malformed input yields no windows: diagnostics never block a turn.
    assert!(map_grok_quota_windows(&serde_json::json!({})).is_empty());
    assert!(map_grok_quota_windows(&serde_json::json!({ "rateLimits": null })).is_empty());
}

// ---------------------------------------------------------------------------
// G3: teardown kills the whole group; quarantine path settles bounded
// ---------------------------------------------------------------------------

#[test]
fn grok_teardown_requires_group_termination() {
    assert!(
        grok_requires_group_termination(),
        "Windows teardown must kill the whole Job Object so no grok grandchild \
         holding a pipe is orphaned; unobserved reaps surface as \
         UnresolvedReapDuring through finish_grok_turn"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_kill_tree_reaps_bounded_without_orphaned_pipes() {
    // Pipe-holding grandchild: the leader is terminated below while this
    // holder still inherits stdout, so EOF must arrive bounded without an
    // orphan wedging the reader.
    #[cfg(windows)]
    let (program, args): (&OsStr, Vec<OsString>) = (
        OsStr::new("cmd"),
        vec![
            OsString::from("/C"),
            OsString::from("echo READY& start /B ping -n 4 127.0.0.1 & ping -n 30 127.0.0.1 >nul"),
        ],
    );
    #[cfg(not(windows))]
    let (program, args): (&OsStr, Vec<OsString>) = (
        OsStr::new("sh"),
        vec![
            OsString::from("-c"),
            OsString::from("echo READY; sleep 3 & sleep 30"),
        ],
    );
    let mut child = spawn_acp_child(program, &args, None).expect("acp child spawns");
    let pipes = child.take_pipes().expect("acp pipes");
    let mut reader = BufReader::new(pipes.stdout);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(15), reader.read_line(&mut line))
        .await
        .expect("ready line arrives bounded")
        .expect("ready line reads");
    assert_eq!(line.trim_end(), "READY");
    drop(pipes.stdin);
    drop(pipes.stderr);
    let shutdown = tokio::time::timeout(
        Duration::from_secs(15),
        shutdown_acp_child(child, Duration::from_secs(3)),
    )
    .await
    .expect("teardown returns bounded");
    assert!(
        matches!(shutdown, AcpShutdown::ReapedAfterKill(_)),
        "live leader must be reaped after kill, got {shutdown:?}"
    );
    // The pipe-holding grandchild is gone with the group (Windows) or exits
    // on its own (elsewhere): EOF arrives bounded either way.
    let eof = tokio::time::timeout(Duration::from_secs(30), async {
        let mut tail = String::new();
        loop {
            let count = reader.read_line(&mut tail).await.expect("tail reads");
            if count == 0 {
                break;
            }
            tail.clear();
        }
    })
    .await;
    assert!(eof.is_ok(), "stdout EOF must arrive bounded");
}

#[tokio::test(flavor = "current_thread")]
async fn acp_zero_budget_settles_the_quarantine_path_bounded() {
    #[cfg(windows)]
    let (program, args): (&OsStr, Vec<OsString>) = (
        OsStr::new("cmd"),
        vec![
            OsString::from("/C"),
            OsString::from("ping -n 30 127.0.0.1 >nul"),
        ],
    );
    #[cfg(not(windows))]
    let (program, args): (&OsStr, Vec<OsString>) = (
        OsStr::new("sh"),
        vec![OsString::from("-c"), OsString::from("sleep 30")],
    );
    let mut child = spawn_acp_child(program, &args, None).expect("acp child spawns");
    // Lifeline closed up front, exactly like the executor teardown.
    drop(child.take_pipes());
    let settled = tokio::time::timeout(
        Duration::from_secs(15),
        shutdown_acp_child(child, Duration::ZERO),
    )
    .await
    .expect("zero-budget teardown returns bounded");
    // The defined post-kill grace must observe the terminated sleeper: a
    // zero budget never collapses the kill into a bare poll/quarantine.
    assert!(
        matches!(settled, AcpShutdown::ReapedAfterKill(_)),
        "zero-budget acp teardown must reap after kill, got {settled:?}"
    );
}

// ---------------------------------------------------------------------------
// G3: sweep replay of the durable prefix after kill, same conversation id
// ---------------------------------------------------------------------------

#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
#[tokio::test(flavor = "current_thread")]
async fn fixture_restart_after_kill_replays_prefix_on_the_same_conversation() {
    // Attempt 1: the durable prefix lands, then the peer dies before the
    // prompt result (kill): the update loop reports interruption.
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
                "result": { "sessionId": "sess-grok-replay" },
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
        // One durable update, then the leader dies: no prompt result, the
        // pipe just closes.
        agent_write_line(&mut agent_write, &update_line("sess-grok-replay", 1)).await;
    });

    let mut driver = AcpTransport::new(driver_read, driver_write, bounds);
    driver.initialize().await.expect("handshake");
    driver.authenticate("cached_token").await.expect("auth");
    let session = driver.new_session("C:\\work").await.expect("new");
    assert_eq!(session.as_str(), "sess-grok-replay");
    let content =
        build_prompt_content(ImageMode::Embedded, "again", &[], None).expect("prompt content");
    let prompt_id = driver.prompt(&session, content).await.expect("prompt");
    let mut durable_prefix = String::new();
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("session update")
    {
        UpdateEvent::SessionUpdate(update) => {
            assert_eq!(update.update.value()["index"], serde_json::json!(1));
            durable_prefix.push_str("durable-");
        }
        other => panic!("expected session update, got {other:?}"),
    }
    match driver.next_update(&session, &prompt_id).await {
        Err(AcpError::PeerClosed) => {}
        other => panic!("expected peer-closed interruption, got {other:?}"),
    }
    drop(driver);
    agent.await.expect("agent joins");
    assert_eq!(durable_prefix, "durable-");

    // Attempt 2 (startup-sweep replay): resume reopens the SAME conversation
    // id through session/load — never a second conversation duplicating
    // provider effects — and the turn completes over the durable prefix.
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
        let load = agent_read_value(&mut agent_read)
            .await
            .expect("session/load request");
        assert_eq!(
            load.get("method").and_then(Value::as_str),
            Some(METHOD_SESSION_LOAD)
        );
        assert_eq!(
            load.pointer("/params/sessionId").and_then(Value::as_str),
            Some("sess-grok-replay")
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
                "result": {
                    "stopReason": "completed",
                    "usage": { "inputTokens": 7, "outputTokens": 11 },
                },
            })
            .to_string(),
        )
        .await;
        while agent_read_value(&mut agent_read).await.is_some() {}
    });

    let stored = grok_resume_session_id("sess-grok-replay").expect("stored validates");
    let resumed = SessionId::parse(stored.as_str(), 256).expect("resume identity");
    let mut driver = AcpTransport::new(driver_read, driver_write, bounds);
    driver.initialize().await.expect("handshake");
    driver.authenticate("cached_token").await.expect("auth");
    driver
        .load_session(&resumed, "C:\\work")
        .await
        .expect("session/load");
    assert!(grok_loaded_session_is_stored(
        resumed.as_str(),
        "sess-grok-replay"
    ));
    let content =
        build_prompt_content(ImageMode::Embedded, "again", &[], None).expect("prompt content");
    let prompt_id = driver.prompt(&resumed, content).await.expect("prompt");
    let mut suffix = String::new();
    match driver
        .next_update(&resumed, &prompt_id)
        .await
        .expect("prompt result")
    {
        UpdateEvent::PromptResult(outcome) => {
            assert!(!outcome.cancelled);
            let sample = grok_sample_from_token_usage(&outcome.usage.expect("usage reported"))
                .expect("usage sample");
            assert_eq!(sample.input, Some(7));
            assert_eq!(sample.output, Some(11));
            suffix.push_str("replayed");
        }
        other => panic!("expected prompt result, got {other:?}"),
    }
    driver.shutdown_writer().await.expect("lifeline close");
    drop(driver);
    agent.await.expect("agent joins");
    assert_eq!(format!("{durable_prefix}{suffix}"), "durable-replayed");
}
