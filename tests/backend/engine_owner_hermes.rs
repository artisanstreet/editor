//! Finite H2 Hermes lifecycle proofs without the real gateway.
//!
//! Pure coverage (settings, guidance seed, readiness, SHA-1 vector, framing,
//! envelopes, inventory, resume enforcement, usage basis, image rejection,
//! approval/question idempotency, stall predicate, binding round trip) plus
//! fixture WebSocket gateway turns: an in-test loopback TCP server speaks the
//! Hermes JSON-RPC dialect (upgrade, `gateway.ready`, `model.options`,
//! `session.create`/`resume`, `config.set`, `prompt.submit`, scripted
//! streaming events, steer/respond/interrupt/close) while the test drives the
//! shared [`super::hermes`] client through connect, inventory validation,
//! session open, one authorized prompt, and the streaming pump. No real
//! `hermes` binary, no real gateway, no catalog flag, no frontend selection.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use artisan_domain::{
    AuthoredText, EngineModelId, EngineProfileId, EngineRouteId, HermesPermissionMode,
    HermesReasoningEffort, HermesSelection, ImageAttachment, QueueMessagePayload, RootPath, RunId,
    RunUsageBasis, ThreadId, UnixMillis,
};
use artisan_transport::CancelHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;

use super::hermes::{
    ApplyContext, DecodedEnvelope, GatewayClient, GatewayError, HERMES_MAX_ANSWERS,
    HERMES_MAX_FRAME_BYTES, HermesEvent, HermesNormalizer, HermesPendingTracker, HermesSettings,
    HermesTurnError, InventoryError, OpenSessionInput, RequestScope, SessionError,
    VerifiedHermesLaunch, answer_approval, answer_questions, apply_observations, decode_approval,
    decode_envelope, decode_questions, guidance_seed_messages, has_stalled, interrupt_live_turn,
    inventory_supports, new_session_token, open_session, parse_ready_port_line, read_ready_port,
    read_ws_frame, reject_image_attachments, resolve_service_executable, resume_selection_matches,
    steer_live_turn, usage_report, usage_sample, validate_model_options_inventory,
    websocket_accept_key, write_client_text, write_server_text,
};
use super::observation::{EngineObservation, TerminalState};
use super::process::spawn_hermes_engine;
use crate::native_run_dispatch::{binding_bytes_vec, binding_matches_bytes};

// ---------------------------------------------------------------------------
// Selection and settings
// ---------------------------------------------------------------------------

fn hermes_selection() -> HermesSelection {
    HermesSelection::new(
        EngineProfileId::parse("hermes-fixture").expect("profile id"),
        EngineModelId::parse("nous/model-x").expect("model id"),
        EngineRouteId::parse("provider-a").expect("route id"),
        HermesPermissionMode::Profile,
        Some(HermesReasoningEffort::parse("high").expect("effort")),
        true,
    )
}

#[test]
fn hermes_settings_map_selection_without_coercion() {
    let settings = HermesSettings::from_selection(&hermes_selection());
    assert_eq!(settings.profile_id(), "hermes-fixture");
    assert_eq!(settings.model_id(), "nous/model-x");
    assert_eq!(settings.route_id(), "provider-a");
    let params = settings.create_params("C:\\work", &[]);
    assert_eq!(params["cwd"], "C:\\work");
    assert_eq!(params["model"], "nous/model-x");
    assert_eq!(params["provider"], "provider-a");
    assert_eq!(params["profile"], "hermes-fixture");
    assert_eq!(params["fast"], true);
    assert_eq!(params["reasoning_effort"], "high");
    assert_eq!(params["source"], "artisan");
    assert_eq!(params["close_on_disconnect"], false);
}

#[test]
fn hermes_default_profile_omitted_and_effort_defaults_to_medium() {
    let selection = HermesSelection::new(
        EngineProfileId::parse("default").expect("profile id"),
        EngineModelId::parse("m").expect("model id"),
        EngineRouteId::parse("r").expect("route id"),
        HermesPermissionMode::Yolo,
        None,
        false,
    );
    let settings = HermesSettings::from_selection(&selection);
    let params = settings.create_params("C:\\work", &[]);
    assert!(params.get("profile").is_none());
    assert_eq!(params["reasoning_effort"], "medium");
}

#[test]
fn guidance_seed_joins_sections_into_one_system_message() {
    assert!(guidance_seed_messages(&[]).is_empty());
    let seed = guidance_seed_messages(&["alpha".to_owned(), "beta".to_owned()]);
    assert_eq!(seed.len(), 1);
    assert_eq!(seed[0]["role"], "system");
    assert_eq!(seed[0]["content"], "alpha\n\nbeta");
}

#[test]
fn session_token_is_url_safe_base64_of_32_bytes() {
    let token = new_session_token().expect("token");
    assert_eq!(token.len(), 43);
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "token must be base64url without padding"
    );
}

// ---------------------------------------------------------------------------
// Service resolution and spawn
// ---------------------------------------------------------------------------

#[test]
fn launch_rejects_empty_identities() {
    let exe = std::env::current_exe().expect("current exe");
    assert!(VerifiedHermesLaunch::new(exe.clone(), String::new(), "0.20.0".to_owned()).is_none());
    assert!(VerifiedHermesLaunch::new(exe, "p".to_owned(), String::new()).is_none());
}

#[test]
fn launch_revalidate_tracks_executable_presence() {
    let exe = std::env::current_exe().expect("current exe");
    let launch =
        VerifiedHermesLaunch::new(exe, "p".to_owned(), "0.20.0".to_owned()).expect("launch");
    assert!(launch.revalidate().is_ok());
    let missing = VerifiedHermesLaunch::new(
        std::path::PathBuf::from("definitely-not-a-hermes-binary-xyz"),
        "p".to_owned(),
        "0.20.0".to_owned(),
    )
    .expect("launch");
    assert!(missing.revalidate().is_err());
}

#[test]
fn spawn_missing_executable_fails_without_child() {
    let launch = VerifiedHermesLaunch::new(
        std::path::PathBuf::from("definitely-not-a-hermes-binary-xyz"),
        "p".to_owned(),
        "0.20.0".to_owned(),
    )
    .expect("launch");
    let root =
        RootPath::parse(std::env::temp_dir().to_str().expect("temp path utf8")).expect("root");
    assert!(spawn_hermes_engine(&launch, &root, "token").is_err());
}

#[tokio::test]
async fn spawn_installed_executable_reaches_readiness_eof() {
    let exe = std::env::current_exe().expect("current exe");
    let launch =
        VerifiedHermesLaunch::new(exe, "default".to_owned(), "0.20.0".to_owned()).expect("launch");
    let root =
        RootPath::parse(std::env::temp_dir().to_str().expect("temp path utf8")).expect("root");
    let mut child = spawn_hermes_engine(&launch, &root, "token").expect("spawns installed binary");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let error = read_ready_port(
        &mut stdout,
        1024,
        256 * 1024,
        Instant::now() + Duration::from_secs(30),
        &cancel,
        &shutdown,
    )
    .await
    .expect_err("re-executed test binary never reports readiness");
    assert_eq!(error, super::readiness::ReadinessError::EofBeforeNewline);
    assert!(child.wait().await.is_ok(), "child must reap");
}

#[test]
fn service_resolution_reports_something_or_nothing_without_panic() {
    let _ = resolve_service_executable();
}

// ---------------------------------------------------------------------------
// Readiness grammar
// ---------------------------------------------------------------------------

#[test]
fn ready_port_line_parses_exactly() {
    assert_eq!(
        parse_ready_port_line("HERMES_BACKEND_READY port=1234\n"),
        Some(1234)
    );
    assert_eq!(
        parse_ready_port_line("HERMES_BACKEND_READY port=65535\r\n"),
        Some(65535)
    );
    assert_eq!(parse_ready_port_line("HERMES_BACKEND_READY port=0\n"), None);
    assert_eq!(
        parse_ready_port_line("HERMES_BACKEND_READY port=65536\n"),
        None
    );
    assert_eq!(
        parse_ready_port_line("HERMES_BACKEND_READY port=abc\n"),
        None
    );
    assert_eq!(parse_ready_port_line("HERMES_BACKEND_READY\n"), None);
    assert_eq!(parse_ready_port_line("noise\n"), None);
    assert_eq!(parse_ready_port_line(""), None);
}

#[tokio::test]
async fn ready_port_skips_noise_then_reports() {
    let mut cursor =
        std::io::Cursor::new(b"starting up\nHERMES_BACKEND_READY port=4321\n".as_slice());
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let port = read_ready_port(
        &mut cursor,
        1024,
        256 * 1024,
        Instant::now() + Duration::from_secs(5),
        &cancel,
        &shutdown,
    )
    .await
    .expect("ready line parses");
    assert_eq!(port, 4321);
}

#[tokio::test]
async fn ready_port_rejects_eof_without_record() {
    let mut cursor = std::io::Cursor::new(b"no record here\n".as_slice());
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let error = read_ready_port(
        &mut cursor,
        1024,
        256 * 1024,
        Instant::now() + Duration::from_secs(5),
        &cancel,
        &shutdown,
    )
    .await
    .expect_err("missing record must fail");
    assert_eq!(error, super::readiness::ReadinessError::EofBeforeNewline);
}

#[tokio::test]
async fn ready_port_rejects_overlong_lines() {
    let overlong = vec![b'x'; 64];
    let mut cursor = std::io::Cursor::new(overlong.as_slice());
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let error = read_ready_port(
        &mut cursor,
        16,
        256 * 1024,
        Instant::now() + Duration::from_secs(5),
        &cancel,
        &shutdown,
    )
    .await
    .expect_err("overlong line must fail");
    assert_eq!(error, super::readiness::ReadinessError::Io);
}

#[tokio::test]
async fn ready_port_times_out_on_silence() {
    let mut silent = tokio::io::pending();
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let error = read_ready_port(
        &mut silent,
        1024,
        256 * 1024,
        Instant::now(),
        &cancel,
        &shutdown,
    )
    .await
    .expect_err("expired deadline must fail");
    assert_eq!(error, super::readiness::ReadinessError::Deadline);
}

// ---------------------------------------------------------------------------
// Handshake vector, framing, envelopes
// ---------------------------------------------------------------------------

#[test]
fn accept_key_matches_rfc6455_vector() {
    assert_eq!(
        websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
}

#[test]
fn envelopes_decode_to_typed_shapes() {
    let event = decode_envelope(
        r#"{"method":"event","params":{"type":"message.delta","session_id":"s-1","payload":{"text":"hi"}}}"#,
    )
    .expect("event decodes");
    match event {
        DecodedEnvelope::Event(event) => {
            assert_eq!(event.event_type(), "message.delta");
            assert_eq!(event.session_id(), Some("s-1"));
        }
        other => panic!("expected event, got {other:?}"),
    }
    let response = decode_envelope(r#"{"id":3,"result":{"ok":true}}"#).expect("response decodes");
    assert!(matches!(response, DecodedEnvelope::Response { id: 3, .. }));
    let remote = decode_envelope(r#"{"id":4,"error":{"code":404,"message":"nope"}}"#)
        .expect("remote decodes");
    match remote {
        DecodedEnvelope::Response { error_code, .. } => assert_eq!(error_code, Some(404)),
        other => panic!("expected response, got {other:?}"),
    }
    assert!(matches!(
        decode_envelope(r#"{"method":"something-else","params":{}}"#).expect("ignored"),
        DecodedEnvelope::Ignored
    ));
    assert!(matches!(
        decode_envelope(r#"{"id":"nope","result":{}}"#).expect("foreign id ignored"),
        DecodedEnvelope::Ignored
    ));
    assert!(decode_envelope("not json").is_err());
    assert!(decode_envelope("[1,2]").is_err());
    assert!(decode_envelope(r#"{"method":"event","params":{"type":""}}"#).is_err());
}

#[tokio::test]
async fn masked_client_text_round_trips_through_frame_reader() {
    let (mut client_side, server_side) = tokio::io::duplex(65536);
    write_client_text(&mut client_side, b"hello-hermes", HERMES_MAX_FRAME_BYTES)
        .await
        .expect("masked write");
    drop(client_side);
    let mut reader = tokio::io::BufReader::new(server_side);
    let frame = read_ws_frame(&mut reader, HERMES_MAX_FRAME_BYTES)
        .await
        .expect("frame reads");
    assert!(matches!(frame, super::hermes::WsFrame::Text(bytes, true) if bytes == b"hello-hermes"));
}

#[tokio::test]
async fn unmasked_server_text_round_trips() {
    let (mut server_side, client_side) = tokio::io::duplex(65536);
    write_server_text(&mut server_side, b"{}", HERMES_MAX_FRAME_BYTES)
        .await
        .expect("server write");
    drop(server_side);
    let mut reader = tokio::io::BufReader::new(client_side);
    let frame = read_ws_frame(&mut reader, HERMES_MAX_FRAME_BYTES)
        .await
        .expect("frame reads");
    assert!(matches!(frame, super::hermes::WsFrame::Text(bytes, true) if bytes == b"{}"));
}

#[tokio::test]
async fn framing_bounds_and_malformed_reject() {
    let too_large = [
        0x82_u8, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ];
    let mut reader = tokio::io::BufReader::new(&too_large[..]);
    assert_eq!(
        read_ws_frame(&mut reader, HERMES_MAX_FRAME_BYTES).await,
        Err(super::hermes::WsFrameError::TooLarge)
    );
    let bad_opcode = [0x83_u8, 0x00];
    let mut reader = tokio::io::BufReader::new(&bad_opcode[..]);
    assert_eq!(
        read_ws_frame(&mut reader, HERMES_MAX_FRAME_BYTES).await,
        Err(super::hermes::WsFrameError::InvalidFrame)
    );
    let empty: &[u8] = &[];
    let mut reader = tokio::io::BufReader::new(empty);
    assert_eq!(
        read_ws_frame(&mut reader, HERMES_MAX_FRAME_BYTES).await,
        Err(super::hermes::WsFrameError::Closed)
    );
    let oversize = vec![0_u8; 8];
    let mut writer = tokio::io::BufWriter::new(Vec::new());
    assert_eq!(
        write_client_text(&mut writer, &oversize, 4).await,
        Err(super::hermes::WsFrameError::TooLarge)
    );
}

// ---------------------------------------------------------------------------
// Inventory validation
// ---------------------------------------------------------------------------

fn valid_inventory() -> serde_json::Value {
    serde_json::json!({
        "providers": [
            {
                "slug": "provider-a",
                "name": "Provider A",
                "authenticated": true,
                "models": ["nous/model-x", "nous/model-y"],
                "capabilities": { "nous/model-x": { "fast": true } },
                "pricing": { "nous/model-x": { "input": "$1.00", "output": "$2.00" } },
                "unavailable_models": ["nous/model-y"],
                "future_field": "ignored, never inferred",
            },
            {
                "slug": "provider-b",
                "name": "Provider B",
                "authenticated": false,
                "models": ["other/model-z"],
            },
        ],
    })
}

#[test]
fn inventory_validates_selection_without_inference() {
    let inventory = validate_model_options_inventory(&valid_inventory()).expect("valid inventory");
    assert!(inventory_supports(&inventory, "provider-a", "nous/model-x"));
    assert!(!inventory_supports(
        &inventory,
        "provider-a",
        "nous/model-y"
    ));
    assert!(!inventory_supports(
        &inventory,
        "provider-b",
        "other/model-z"
    ));
    assert!(!inventory_supports(
        &inventory,
        "provider-a",
        "nous/model-missing"
    ));
}

#[test]
fn inventory_rejects_duplicates_and_tampering() {
    let mut duplicated = valid_inventory();
    duplicated["providers"][0]["models"] = serde_json::json!(["nous/model-x", "nous/model-x"]);
    assert_eq!(
        validate_model_options_inventory(&duplicated),
        Err(InventoryError::DuplicateModel)
    );
    assert_eq!(
        validate_model_options_inventory(&serde_json::json!({})),
        Err(InventoryError::InvalidShape)
    );
    assert_eq!(
        validate_model_options_inventory(&serde_json::json!({"providers": "nope"})),
        Err(InventoryError::InvalidShape)
    );
    assert_eq!(
        validate_model_options_inventory(&serde_json::json!({
            "providers": [{ "slug": "a", "name": "A", "models": [42] }],
        })),
        Err(InventoryError::InvalidShape)
    );
    assert_eq!(
        validate_model_options_inventory(&serde_json::json!({
            "providers": [{ "slug": "a", "name": "A", "models": ["m"], "authenticated": "yes" }],
        })),
        Err(InventoryError::InvalidShape)
    );
}

// ---------------------------------------------------------------------------
// Resume enforcement, usage basis, images, stall
// ---------------------------------------------------------------------------

#[test]
fn resume_requires_identical_model_selection() {
    assert!(resume_selection_matches("m", "r", Some("m"), Some("r")));
    assert!(resume_selection_matches("m", "r", None, None));
    assert!(!resume_selection_matches(
        "m",
        "r",
        Some("other"),
        Some("r")
    ));
    assert!(!resume_selection_matches(
        "m",
        "r",
        Some("m"),
        Some("other")
    ));
}

#[test]
fn usage_samples_report_cumulative_basis() {
    let run_id = RunId::parse("run-1").expect("run id");
    let thread_id = ThreadId::parse("thread-1").expect("thread id");
    let settings = HermesSettings::from_selection(&hermes_selection());
    let sample = usage_sample(&serde_json::json!({
        "usage": { "input": 10, "output": 5, "context_used": 100, "context_max": 1000 },
    }))
    .expect("sample");
    let report = usage_report(
        &run_id,
        Some(&thread_id),
        "runtime-1",
        Some("run-1:turn:0".to_owned()),
        7,
        &settings,
        &sample,
        UnixMillis::from_millis(1),
    )
    .expect("report");
    assert_eq!(report.basis(), RunUsageBasis::Cumulative);
    assert_eq!(report.input_tokens(), Some(10));
    assert_eq!(report.output_tokens(), Some(5));
    assert_eq!(report.context_tokens(), Some(100));
    assert_eq!(report.context_window_tokens(), Some(1000));

    let long_form = usage_sample(&serde_json::json!({ "input_tokens": 3 })).expect("long spelling");
    let long_report = usage_report(
        &run_id,
        Some(&thread_id),
        "runtime-1",
        None,
        8,
        &settings,
        &long_form,
        UnixMillis::from_millis(1),
    )
    .expect("long-form report");
    assert_eq!(long_report.input_tokens(), Some(3));
    assert!(usage_sample(&serde_json::json!({ "cost_usd": 1.5 })).is_none());
    assert!(usage_sample(&serde_json::json!({})).is_none());
    assert!(
        usage_report(
            &run_id,
            None,
            "runtime-1",
            None,
            7,
            &settings,
            &sample,
            UnixMillis::from_millis(1),
        )
        .is_none(),
        "usage without a thread scope must not synthesize identities"
    );
}

#[test]
fn images_reject_with_typed_error_never_silently_dropped() {
    let text_only = QueueMessagePayload::text_only("hello").expect("text payload");
    assert!(reject_image_attachments(&text_only).is_ok());
    let image = ImageAttachment::new("image/png", vec![1, 2, 3], "a.png").expect("image");
    let with_image = QueueMessagePayload::new(
        Some(AuthoredText::parse("see this").expect("text")),
        vec![image],
    )
    .expect("image payload");
    assert_eq!(
        reject_image_attachments(&with_image),
        Err(HermesTurnError::ImagesUnsupported)
    );
}

#[test]
fn stall_predicate_matches_sibling_semantics() {
    let now = Instant::now();
    assert!(!has_stalled(false, now, Duration::from_secs(1), now));
    assert!(!has_stalled(true, now, Duration::from_secs(60), now));
    assert!(has_stalled(
        true,
        now - Duration::from_secs(61),
        Duration::from_secs(60),
        now
    ));
}

#[test]
fn hermes_binding_round_trips_tag_format_and_session() {
    let bytes = binding_bytes_vec("hermes", "hermes-fixture", "stored-1").expect("binding");
    assert!(binding_matches_bytes(
        &bytes,
        "hermes",
        "hermes-fixture",
        "stored-1"
    ));
    assert!(!binding_matches_bytes(
        &bytes,
        "codex",
        "hermes-fixture",
        "stored-1"
    ));
    assert!(!binding_matches_bytes(
        &bytes,
        "hermes",
        "hermes-fixture",
        "stored-2"
    ));
}

// ---------------------------------------------------------------------------
// Approval/question decoding and idempotency
// ---------------------------------------------------------------------------

fn event_fixture(payload: &str) -> HermesEvent {
    match decode_envelope(payload).expect("envelope decodes") {
        DecodedEnvelope::Event(event) => event,
        other => panic!("expected event, got {other:?}"),
    }
}

#[test]
fn approval_decodes_and_idempotent_tracker_holds_one() {
    let event = event_fixture(
        r#"{"method":"event","params":{"type":"approval.request","session_id":"s","payload":{"request_id":"apr-1","description":"Run tests?","command":"npm test"}}}"#,
    );
    let request = decode_approval(&event).expect("approval decodes");
    let domain = request.to_domain_request().expect("domain request");
    assert_eq!(domain.kind(), artisan_domain::ApprovalKind::Command);
    let mut tracker = HermesPendingTracker::new();
    assert!(tracker.note_approval(request.clone()));
    assert!(!tracker.note_approval(request));
    assert_eq!(tracker.pending_approvals(), 1);
    assert!(!tracker.resolve_approval("missing"));
    assert!(tracker.resolve_approval("apr-1"));
    assert_eq!(tracker.pending_approvals(), 0);
}

#[test]
fn approval_without_command_maps_to_action_and_bad_frames_drop() {
    let event = event_fixture(
        r#"{"method":"event","params":{"type":"approval.request","session_id":"s","payload":{"request_id":"apr-2"}}}"#,
    );
    let request = decode_approval(&event).expect("approval decodes");
    assert_eq!(
        request.to_domain_request().expect("domain").kind(),
        artisan_domain::ApprovalKind::Action
    );
    assert!(
        decode_approval(&event_fixture(
            r#"{"method":"event","params":{"type":"approval.request","session_id":"s","payload":{}}}"#
        ))
        .is_none()
    );
    assert!(
        decode_approval(&event_fixture(
            r#"{"method":"event","params":{"type":"message.delta","session_id":"s","payload":{}}}"#
        ))
        .is_none()
    );
    let oversized = event_fixture(&format!(
        r#"{{"method":"event","params":{{"type":"approval.request","session_id":"s","payload":{{"request_id":"apr-3","command":"{}"}}}}}}"#,
        "x".repeat(9 * 1024)
    ));
    let bad = decode_approval(&oversized).expect("decodes before domain");
    let mut tracker = HermesPendingTracker::new();
    assert!(
        !tracker.note_approval(bad),
        "out-of-bound command must not reach durable rows"
    );
}

#[test]
fn questions_decode_list_and_single_forms_idempotently() {
    let event = event_fixture(
        r#"{"method":"event","params":{"type":"clarify.request","session_id":"s","payload":{"request_id":"clr-1","questions":[{"qid":"q1","question":"Pick one?","choices":["a","b"],"multi_select":true}]}}}"#,
    );
    let groups = decode_questions(&event);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].questions().len(), 1);
    assert_eq!(groups[0].questions()[0].question_id(), "clr-1:q1");
    let mut tracker = HermesPendingTracker::new();
    assert_eq!(tracker.note_questions(&groups[0]), 1);
    assert_eq!(tracker.note_questions(&groups[0]), 0);
    assert_eq!(tracker.pending_questions(), 1);

    let single = event_fixture(
        r#"{"method":"event","params":{"type":"clarify.request","session_id":"s","payload":{"request_id":"clr-2","question":"Free text?"}}}"#,
    );
    let groups = decode_questions(&single);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].questions()[0].question_id(), "clr-2");
    assert_eq!(tracker.note_questions(&groups[0]), 1);
    assert!(!tracker.resolve_question("missing"));
    assert!(tracker.resolve_question("clr-2"));
}

#[test]
fn answer_batching_caps_at_domain_ceiling() {
    assert_eq!(HERMES_MAX_ANSWERS, 16);
}

// ---------------------------------------------------------------------------
// Fixture WebSocket gateway (test-only loopback Hermes dialect)
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum FixtureHandshake {
    Ok,
    BadAccept,
    Reject,
}

#[derive(Clone)]
enum PrefixFrame {
    Binary,
    MalformedText,
    Ping,
    Close,
    Fragments(Vec<Vec<u8>>),
}

#[derive(Clone)]
struct FixtureMode {
    handshake: FixtureHandshake,
    blackhole: bool,
    prefix_frames: Vec<PrefixFrame>,
}

impl FixtureMode {
    fn scripted() -> Self {
        Self {
            handshake: FixtureHandshake::Ok,
            blackhole: false,
            prefix_frames: Vec::new(),
        }
    }
}

struct FixtureSession {
    runtime: String,
    model: String,
    provider: String,
}

struct FixtureScript {
    inventory: serde_json::Value,
    events_after_prompt: Vec<serde_json::Value>,
}

struct FixtureState {
    script: FixtureScript,
    sessions: HashMap<String, FixtureSession>,
    received: Vec<(String, serde_json::Value)>,
    pong_seen: bool,
    runtime_counter: u64,
    stored_counter: u64,
}

fn fixture_inventory() -> serde_json::Value {
    serde_json::json!({
        "providers": [
            {
                "slug": "provider-a",
                "name": "Provider A",
                "authenticated": true,
                "models": ["nous/model-x"],
            },
        ],
    })
}

fn fixture_script() -> FixtureScript {
    let event = |event_type: &str, payload: serde_json::Value| {
        serde_json::json!({
            "method": "event",
            "params": {
                "type": event_type,
                "session_id": "{runtime}",
                "payload": payload,
            },
        })
    };
    FixtureScript {
        inventory: fixture_inventory(),
        events_after_prompt: vec![
            event("message.start", serde_json::json!({})),
            event("message.delta", serde_json::json!({"text": "hello "})),
            event(
                "approval.request",
                serde_json::json!({"request_id": "apr-1", "description": "Run tests?", "command": "npm test"}),
            ),
            event(
                "clarify.request",
                serde_json::json!({"request_id": "clr-1", "question": "Pick one?", "choices": ["a", "b"]}),
            ),
            event(
                "tool.start",
                serde_json::json!({"tool_id": "tool-1", "name": "shell"}),
            ),
            event(
                "tool.complete",
                serde_json::json!({"tool_id": "tool-1", "name": "shell", "result_text": "ok"}),
            ),
            event(
                "session.usage",
                serde_json::json!({"usage": {"input": 10, "output": 5, "context_used": 100, "context_max": 1000}}),
            ),
            event(
                "message.complete",
                serde_json::json!({"text": "done", "usage": {"input": 12, "output": 6}}),
            ),
        ],
    }
}

async fn read_http_head(
    reader: &mut tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> Option<String> {
    let mut head = Vec::new();
    loop {
        if head.len() > 8192 {
            return None;
        }
        let mut chunk = [0_u8; 512];
        let count = reader.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        head.extend_from_slice(&chunk[..count]);
        if head.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(head).ok()
}

async fn run_fixture_connection(
    stream: TcpStream,
    state: Arc<Mutex<FixtureState>>,
    mode: FixtureMode,
) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    let Some(head) = read_http_head(&mut reader).await else {
        return;
    };
    if !head.starts_with("GET /api/ws?token=") {
        let _ = writer
            .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n")
            .await;
        return;
    }
    if matches!(mode.handshake, FixtureHandshake::Reject) {
        let _ = writer
            .write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
            .await;
        return;
    }
    let key = head
        .split("\r\n")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("sec-websocket-key")
                .then(|| value.trim().to_owned())
        })
        .unwrap_or_default();
    let accept = match mode.handshake {
        FixtureHandshake::Ok => websocket_accept_key(&key),
        FixtureHandshake::BadAccept => "tampered-accept".to_owned(),
        FixtureHandshake::Reject => unreachable!("rejected above"),
    };
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if writer.write_all(response.as_bytes()).await.is_err() {
        return;
    }
    let ready = serde_json::json!({"method": "event", "params": {"type": "gateway.ready"}});
    if write_server_text(
        &mut writer,
        ready.to_string().as_bytes(),
        HERMES_MAX_FRAME_BYTES,
    )
    .await
    .is_err()
    {
        return;
    }
    for prefix in &mode.prefix_frames {
        let sent = match prefix {
            PrefixFrame::Binary => {
                let header = [0x82_u8, 0x03];
                writer.write_all(&header).await.is_ok() && writer.write_all(b"bin").await.is_ok()
            }
            PrefixFrame::MalformedText => {
                write_server_text(&mut writer, b"{oops", HERMES_MAX_FRAME_BYTES)
                    .await
                    .is_ok()
            }
            PrefixFrame::Ping => {
                let header = [0x89_u8, 0x04];
                writer.write_all(&header).await.is_ok() && writer.write_all(b"ping").await.is_ok()
            }
            PrefixFrame::Close => writer.write_all(&[0x88_u8, 0x00]).await.is_ok(),
            PrefixFrame::Fragments(parts) => {
                let mut ok = true;
                for (index, part) in parts.iter().enumerate() {
                    let last = index + 1 == parts.len();
                    let base: u8 = if index == 0 { 0x01 } else { 0x00 };
                    let fin: u8 = if last { 0x80 } else { 0 };
                    let first = base | fin;
                    let length = u8::try_from(part.len()).unwrap_or(u8::MAX);
                    ok = ok
                        && writer.write_all(&[first, length]).await.is_ok()
                        && writer.write_all(part).await.is_ok();
                }
                ok
            }
        };
        if !sent {
            return;
        }
    }
    if mode.blackhole {
        loop {
            match read_ws_frame(&mut reader, HERMES_MAX_FRAME_BYTES).await {
                Ok(super::hermes::WsFrame::Close) => return,
                Ok(_) => {}
                Err(_) => return,
            }
        }
    }
    loop {
        let frame = match read_ws_frame(&mut reader, HERMES_MAX_FRAME_BYTES).await {
            Ok(frame) => frame,
            Err(_) => return,
        };
        match frame {
            super::hermes::WsFrame::Text(bytes, finished) => {
                if !finished {
                    return;
                }
                let value: serde_json::Value = match serde_json::from_slice(&bytes) {
                    Ok(value) => value,
                    Err(_) => return,
                };
                let id = value.get("id").and_then(|id| id.as_u64()).unwrap_or(0);
                let method = value
                    .get("method")
                    .and_then(|method| method.as_str())
                    .unwrap_or("")
                    .to_owned();
                let params = value
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let reply = {
                    let mut guarded = state.lock().await;
                    guarded.received.push((method.clone(), params.clone()));
                    fixture_reply(&mut guarded, id, &method, &params)
                };
                if let Some(reply) = reply {
                    if write_server_text(&mut writer, reply.as_bytes(), HERMES_MAX_FRAME_BYTES)
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                if method == "prompt.submit" {
                    let events = {
                        let guarded = state.lock().await;
                        let runtime = params
                            .get("session_id")
                            .and_then(|session| session.as_str())
                            .unwrap_or("")
                            .to_owned();
                        guarded
                            .script
                            .events_after_prompt
                            .iter()
                            .map(|event| event.to_string().replace("{runtime}", &runtime))
                            .collect::<Vec<_>>()
                    };
                    for event in events {
                        if write_server_text(&mut writer, event.as_bytes(), HERMES_MAX_FRAME_BYTES)
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            super::hermes::WsFrame::Ping(_) => {}
            super::hermes::WsFrame::Pong(_) => {
                state.lock().await.pong_seen = true;
            }
            super::hermes::WsFrame::Close
            | super::hermes::WsFrame::Binary(_, _)
            | super::hermes::WsFrame::Continuation(_, _) => return,
        }
    }
}

fn fixture_reply(
    state: &mut FixtureState,
    id: u64,
    method: &str,
    params: &serde_json::Value,
) -> Option<String> {
    let result = match method {
        "model.options" => state.script.inventory.clone(),
        "session.create" => {
            state.runtime_counter += 1;
            state.stored_counter += 1;
            let runtime = format!("runtime-{}", state.runtime_counter);
            let stored = format!("stored-{}", state.stored_counter);
            let model = params
                .get("model")
                .and_then(|model| model.as_str())
                .unwrap_or("")
                .to_owned();
            let provider = params
                .get("provider")
                .and_then(|provider| provider.as_str())
                .unwrap_or("")
                .to_owned();
            state.sessions.insert(
                stored.clone(),
                FixtureSession {
                    runtime: runtime.clone(),
                    model,
                    provider,
                },
            );
            let session = state.sessions.get(&stored).expect("just stored");
            serde_json::json!({
                "session_id": runtime,
                "stored_session_id": stored,
                "info": {
                    "desktop_contract": 6,
                    "model": session.model,
                    "provider": session.provider,
                },
            })
        }
        "session.resume" => {
            let stored = params
                .get("session_id")
                .and_then(|session| session.as_str())
                .unwrap_or("");
            let Some(session) = state.sessions.get(stored) else {
                return Some(
                    serde_json::json!({"id": id, "error": {"code": 404, "message": "unknown"}})
                        .to_string(),
                );
            };
            serde_json::json!({
                "session_id": session.runtime,
                "info": {
                    "desktop_contract": 6,
                    "model": session.model,
                    "provider": session.provider,
                },
            })
        }
        "config.set" | "prompt.submit" | "session.steer" | "approval.respond"
        | "clarify.respond" | "session.interrupt" | "session.close" => serde_json::json!({}),
        _ => {
            return Some(
                serde_json::json!({"id": id, "error": {"code": 400, "message": "unknown"}})
                    .to_string(),
            );
        }
    };
    Some(serde_json::json!({"id": id, "result": result}).to_string())
}

async fn spawn_fixture(
    mode: FixtureMode,
    script: FixtureScript,
) -> (
    SocketAddr,
    Arc<Mutex<FixtureState>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("fixture listener binds");
    let address = listener.local_addr().expect("fixture address");
    let state = Arc::new(Mutex::new(FixtureState {
        script,
        sessions: HashMap::new(),
        received: Vec::new(),
        pong_seen: false,
        runtime_counter: 0,
        stored_counter: 0,
    }));
    let handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let state = Arc::clone(&state);
                let mode = mode.clone();
                tokio::spawn(async move {
                    run_fixture_connection(stream, state, mode).await;
                });
            }
        })
    };
    (address, state, handle)
}

fn test_scope(cancel: &CancelHandle, shutdown: &CancelHandle, millis: u64) -> RequestScope<'_> {
    RequestScope {
        deadline: Instant::now() + Duration::from_millis(millis),
        cancel,
        shutdown,
    }
}

async fn connect_fixture(
    address: SocketAddr,
    cancel: &CancelHandle,
    shutdown: &CancelHandle,
) -> GatewayClient {
    let scope = test_scope(cancel, shutdown, 10_000);
    GatewayClient::connect(address, "fixture-token", &scope)
        .await
        .expect("fixture connects")
}

// ---------------------------------------------------------------------------
// Gateway lifecycle over the fixture
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hermes_turn_flows_through_fixture_gateway() {
    let (address, state, server) = spawn_fixture(FixtureMode::scripted(), fixture_script()).await;
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let mut client = connect_fixture(address, &cancel, &shutdown).await;

    let scope = test_scope(&cancel, &shutdown, 10_000);
    let (inventory_value, _) = client
        .request(
            "model.options",
            serde_json::json!({
                "explicit_only": true,
                "include_unconfigured": false,
                "refresh": false,
            }),
            &scope,
        )
        .await
        .expect("inventory answers");
    let inventory =
        validate_model_options_inventory(&inventory_value).expect("inventory validates");
    let settings = HermesSettings::from_selection(&hermes_selection());
    assert!(inventory_supports(
        &inventory,
        settings.route_id(),
        settings.model_id()
    ));

    let open_input = OpenSessionInput {
        settings: &settings,
        project_root: "C:\\work",
        guidance_sections: &[],
        resume_stored_session_id: None,
    };
    let opened = open_session(&mut client, &open_input, &scope)
        .await
        .expect("session opens");
    assert_eq!(opened.runtime_session_id, "runtime-1");
    assert_eq!(opened.durable_session_id, "stored-1");

    let submit_scope = test_scope(&cancel, &shutdown, 10_000);
    let (_, _) = client
        .request(
            "prompt.submit",
            serde_json::json!({ "session_id": opened.runtime_session_id, "text": "hi" }),
            &submit_scope,
        )
        .await
        .expect("prompt submits");

    let run_id = RunId::parse("run-1").expect("run id");
    let thread_id = ThreadId::parse("thread-1").expect("thread id");
    let (observations, mut receiver) = mpsc::channel(64);
    let mut normalizer = HermesNormalizer::new();
    let mut tracker = HermesPendingTracker::new();
    let mut active_turn: Option<String> = None;
    let mut frame_sequence: u64 = 0;
    let pump = async {
        let mut terminal = None;
        for _ in 0..16 {
            let event = client.next_event(&cancel, &shutdown).await?;
            frame_sequence += 1;
            terminal = apply_observations(
                &mut normalizer,
                &event,
                ApplyContext {
                    run_id: &run_id,
                    thread_id: Some(&thread_id),
                    settings: &settings,
                    runtime_session_id: &opened.runtime_session_id,
                    tracker: &mut tracker,
                    active_turn: &mut active_turn,
                    observations: &observations,
                    frame_sequence,
                },
            )
            .await;
            if terminal.is_some() {
                break;
            }
        }
        Ok::<_, GatewayError>(terminal)
    };
    let terminal = tokio::time::timeout(Duration::from_secs(30), pump)
        .await
        .expect("pump settles")
        .expect("pump streams");
    assert_eq!(terminal, Some(TerminalState::Completed));
    assert_eq!(tracker.pending_approvals(), 1);
    assert_eq!(tracker.pending_questions(), 1);
    drop(observations);

    let mut deltas = Vec::new();
    let mut usages = Vec::new();
    while let Some(observation) = receiver.recv().await {
        match observation {
            EngineObservation::TextDelta(delta) => deltas.push(delta.delta().to_owned()),
            EngineObservation::Usage(usage) => usages.push(usage),
            other => panic!("unexpected observation shape: {other:?}"),
        }
    }
    assert_eq!(deltas.join(""), "hello done");
    assert_eq!(usages.len(), 2);
    for usage in &usages {
        assert_eq!(usage.report().basis(), RunUsageBasis::Cumulative);
    }

    let answer_scope = test_scope(&cancel, &shutdown, 10_000);
    answer_approval(
        &mut client,
        &mut tracker,
        &opened.runtime_session_id,
        "apr-1",
        false,
        &answer_scope,
    )
    .await
    .expect("deny answers");
    assert_eq!(tracker.pending_approvals(), 0);
    let questions = decode_questions(&event_fixture(
        r#"{"method":"event","params":{"type":"clarify.request","session_id":"runtime-1","payload":{"request_id":"clr-1","question":"Pick one?","choices":["a","b"]}}}"#,
    ));
    assert_eq!(
        tracker.note_questions(&questions[0]),
        0,
        "already pending stays idempotent"
    );
    answer_questions(
        &mut client,
        &mut tracker,
        &opened.runtime_session_id,
        &questions[0],
        &[("clr-1".to_owned(), vec!["a".to_owned()])],
        &answer_scope,
    )
    .await
    .expect("question answers");
    steer_live_turn(
        &mut client,
        &opened.runtime_session_id,
        "keep going",
        &answer_scope,
    )
    .await
    .expect("steer sends");
    interrupt_live_turn(&mut client, &opened.runtime_session_id, &answer_scope)
        .await
        .expect("interrupt sends");

    let received = &state.lock().await.received;
    let methods = received
        .iter()
        .map(|(method, _)| method.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "model.options",
        "session.create",
        "config.set",
        "prompt.submit",
        "approval.respond",
        "clarify.respond",
        "session.steer",
        "session.interrupt",
    ] {
        assert!(methods.contains(&expected), "gateway saw {expected}");
    }
    let deny = received
        .iter()
        .find(|(method, _)| method == "approval.respond")
        .expect("deny recorded");
    assert_eq!(deny.1["choice"], "deny");

    let close_scope = test_scope(&cancel, &shutdown, 10_000);
    let _ = client
        .request(
            "session.close",
            serde_json::json!({ "session_id": opened.runtime_session_id }),
            &close_scope,
        )
        .await;
    client.close().await;
    server.abort();
}

#[tokio::test]
async fn hermes_resume_recovers_across_restart_with_model_enforcement() {
    let (address, _state, server) = spawn_fixture(FixtureMode::scripted(), fixture_script()).await;
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let settings = HermesSettings::from_selection(&hermes_selection());

    let mut first = connect_fixture(address, &cancel, &shutdown).await;
    let scope = test_scope(&cancel, &shutdown, 10_000);
    let opened = open_session(
        &mut first,
        &OpenSessionInput {
            settings: &settings,
            project_root: "C:\\work",
            guidance_sections: &[],
            resume_stored_session_id: None,
        },
        &scope,
    )
    .await
    .expect("session creates");
    first.close().await;

    let mut second = connect_fixture(address, &cancel, &shutdown).await;
    let resumed = open_session(
        &mut second,
        &OpenSessionInput {
            settings: &settings,
            project_root: "C:\\work",
            guidance_sections: &[],
            resume_stored_session_id: Some(&opened.durable_session_id),
        },
        &scope,
    )
    .await
    .expect("durable session resumes after restart");
    assert_eq!(resumed.durable_session_id, opened.durable_session_id);

    let other_model = HermesSelection::new(
        EngineProfileId::parse("hermes-fixture").expect("profile id"),
        EngineModelId::parse("nous/other-model").expect("model id"),
        EngineRouteId::parse("provider-a").expect("route id"),
        HermesPermissionMode::Profile,
        None,
        false,
    );
    let other_settings = HermesSettings::from_selection(&other_model);
    let error = open_session(
        &mut second,
        &OpenSessionInput {
            settings: &other_settings,
            project_root: "C:\\work",
            guidance_sections: &[],
            resume_stored_session_id: Some(&opened.durable_session_id),
        },
        &scope,
    )
    .await
    .expect_err("changed model must fail closed");
    assert_eq!(error, SessionError::Configuration);

    let missing = open_session(
        &mut second,
        &OpenSessionInput {
            settings: &settings,
            project_root: "C:\\work",
            guidance_sections: &[],
            resume_stored_session_id: Some("stored-missing"),
        },
        &scope,
    )
    .await
    .expect_err("unknown stored session must fail");
    assert_eq!(missing, SessionError::ProviderRequestFailed);
    second.close().await;
    server.abort();
}

#[tokio::test]
async fn hermes_gateway_rejects_bad_handshakes_and_frames() {
    let (bad_address, _, bad_server) = spawn_fixture(
        FixtureMode {
            handshake: FixtureHandshake::BadAccept,
            blackhole: false,
            prefix_frames: Vec::new(),
        },
        fixture_script(),
    )
    .await;
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let scope = test_scope(&cancel, &shutdown, 10_000);
    let bad = GatewayClient::connect(bad_address, "token", &scope)
        .await
        .expect_err("tampered accept must fail");
    assert_eq!(bad, GatewayError::Handshake);
    bad_server.abort();

    let (reject_address, _, reject_server) = spawn_fixture(
        FixtureMode {
            handshake: FixtureHandshake::Reject,
            blackhole: false,
            prefix_frames: Vec::new(),
        },
        fixture_script(),
    )
    .await;
    let rejected = GatewayClient::connect(reject_address, "token", &scope)
        .await
        .expect_err("rejected upgrade must fail");
    assert_eq!(rejected, GatewayError::Handshake);
    reject_server.abort();

    let empty = GatewayClient::connect(reject_address, "", &scope)
        .await
        .expect_err("empty token must fail closed");
    assert_eq!(empty, GatewayError::Handshake);
}

#[tokio::test]
async fn hermes_request_times_out_without_gateway_answer() {
    let (address, _, server) = spawn_fixture(
        FixtureMode {
            handshake: FixtureHandshake::Ok,
            blackhole: true,
            prefix_frames: Vec::new(),
        },
        fixture_script(),
    )
    .await;
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let mut client = connect_fixture(address, &cancel, &shutdown).await;
    let scope = test_scope(&cancel, &shutdown, 200);
    assert_eq!(
        client
            .request("model.options", serde_json::json!({}), &scope)
            .await,
        Err(GatewayError::Timeout)
    );
    client.close().await;
    server.abort();
}

#[tokio::test]
async fn hermes_binary_and_malformed_frames_reject() {
    for prefix in [PrefixFrame::Binary, PrefixFrame::MalformedText] {
        let (address, _, server) = spawn_fixture(
            FixtureMode {
                handshake: FixtureHandshake::Ok,
                blackhole: false,
                prefix_frames: vec![prefix],
            },
            fixture_script(),
        )
        .await;
        let cancel = CancelHandle::new();
        let shutdown = CancelHandle::new();
        let mut client = connect_fixture(address, &cancel, &shutdown).await;
        assert!(client.next_event(&cancel, &shutdown).await.is_err());
        client.close().await;
        server.abort();
    }
}

#[tokio::test]
async fn hermes_pong_answers_ping_and_close_ends_stream() {
    let (address, state, server) = spawn_fixture(
        FixtureMode {
            handshake: FixtureHandshake::Ok,
            blackhole: false,
            prefix_frames: vec![PrefixFrame::Ping],
        },
        fixture_script(),
    )
    .await;
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let mut client = connect_fixture(address, &cancel, &shutdown).await;
    let scope = test_scope(&cancel, &shutdown, 10_000);
    let (inventory_value, _) = client
        .request("model.options", serde_json::json!({}), &scope)
        .await
        .expect("request flows past ping");
    assert!(inventory_value.get("providers").is_some());
    client.close().await;
    assert!(state.lock().await.pong_seen, "client must answer ping");
    server.abort();

    let (close_address, _, close_server) = spawn_fixture(
        FixtureMode {
            handshake: FixtureHandshake::Ok,
            blackhole: false,
            prefix_frames: vec![PrefixFrame::Close],
        },
        fixture_script(),
    )
    .await;
    let mut closer = connect_fixture(close_address, &cancel, &shutdown).await;
    assert_eq!(
        closer.next_event(&cancel, &shutdown).await,
        Err(GatewayError::Closed)
    );
    closer.close().await;
    close_server.abort();
}

#[tokio::test]
async fn hermes_fragmented_text_reassembles() {
    let (address, _, server) = spawn_fixture(
        FixtureMode {
            handshake: FixtureHandshake::Ok,
            blackhole: false,
            prefix_frames: vec![PrefixFrame::Fragments(vec![
                br#"{"method":"event","params":{"type":"message"#.to_vec(),
                br#".delta","session_id":"s","payload":{"text":"frag"}}}"#.to_vec(),
            ])],
        },
        fixture_script(),
    )
    .await;
    let cancel = CancelHandle::new();
    let shutdown = CancelHandle::new();
    let mut client = connect_fixture(address, &cancel, &shutdown).await;
    let event = client
        .next_event(&cancel, &shutdown)
        .await
        .expect("fragments reassemble");
    assert_eq!(event.event_type(), "message.delta");
    client.close().await;
    server.abort();
}
