//! Finite C3 cursor continuation, usage, and cleanup fixture proofs.
//!
//! Covers the C3 gate matrix (same engine, explicit target model,
//! certified CLI floor), the `session/load` resume that reopens the same
//! session id, cumulative usage projection with a replacing context gauge,
//! dashboard quota mapping with clamping and kind classification, whole-group
//! teardown with a pipe-holding tail, and durable-prefix replay after kill —
//! all without the real CLI. Still not runnable: no catalog flag flip, no
//! frontend selection, no model inventory.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use artisan_domain::{EngineModelId, RunId, RunUsageBasis, ThreadId, UnixMillis};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::acp::{
    AcpBounds, AcpShutdown, AcpTransport, SessionId, SessionUpdate, UpdateEvent,
    shutdown_acp_child, spawn_acp_child,
};
use super::cursor::{
    CURSOR_C1_UNPROBED_VERSION, CURSOR_CONTINUATION_MINIMUM_CLI_VERSION,
    CURSOR_MAX_SESSION_ID_BYTES, CursorContinuationDecision, CursorContinuationGateInput,
    CursorQuotaWindowKind, CursorUsageAttribution, CursorUsageContext, CursorUsageSample,
    CursorUsageScope, check_cursor_native_continuation, clamp_cursor_percent_used,
    classify_cursor_quota_window_kind, cursor_cli_meets_minimum, cursor_requires_group_termination,
    cursor_reset_at_iso, cursor_resume_session_id, cursor_usage_report, map_cursor_quota_windows,
    parse_cursor_prompt_usage, parse_cursor_usage_update, project_cursor_usage_sample,
};
use super::observation::{EngineObservation, TerminalState};

fn run_id() -> RunId {
    RunId::parse("run-cursor-1").expect("run id")
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

// ---------------------------------------------------------------------------
// C3: continuation gate matrix (same engine, explicit model, certified CLI)
// ---------------------------------------------------------------------------

fn gate_decision(
    cli_version: &str,
    target_model: Option<&str>,
    advertised_models: Option<&[&str]>,
    same_engine: bool,
) -> CursorContinuationDecision {
    check_cursor_native_continuation(&CursorContinuationGateInput {
        cli_version,
        target_model,
        advertised_models,
        same_engine,
    })
}

#[test]
fn cursor_continuation_gate_matrix() {
    // The recorded certified floor authorizes continuation.
    assert_eq!(
        CURSOR_CONTINUATION_MINIMUM_CLI_VERSION,
        "2026.08.11-e8db854"
    );
    assert_eq!(
        gate_decision("2026.08.11-e8db854", Some("composer-1"), None, true),
        CursorContinuationDecision::Compatible
    );
    // Newer dated releases stay compatible, with or without the agent prefix.
    assert_eq!(
        gate_decision("2026.9.6-stable.1", Some("composer-1"), None, true),
        CursorContinuationDecision::Compatible
    );
    assert_eq!(
        gate_decision(
            "agent 2026.9.6-stable.1 (abc)",
            Some("composer-1"),
            None,
            true
        ),
        CursorContinuationDecision::Compatible
    );
    assert_eq!(
        gate_decision(
            "2026.08.11-e8db854",
            Some("composer-1"),
            Some(&["composer-1", "other-model"]),
            true
        ),
        CursorContinuationDecision::Compatible
    );
    // Explicit target model is required before resume.
    assert!(matches!(
        gate_decision("2026.08.11-e8db854", None, None, true),
        CursorContinuationDecision::Incompatible { .. }
    ));
    assert!(matches!(
        gate_decision("2026.08.11-e8db854", Some(""), None, true),
        CursorContinuationDecision::Incompatible { .. }
    ));
    // Older dated releases never authorize continuation.
    for old in [
        "2026.08.10-nightly",
        "2026.9.5-stable.1",
        "2025.12.31-nightly",
        "not a version",
        "",
    ] {
        assert!(
            matches!(
                gate_decision(old, Some("composer-1"), None, true),
                CursorContinuationDecision::Incompatible { .. }
            ),
            "CLI {old} must not authorize continuation"
        );
    }
    // The unprobed sentinel fails the floor so an unprobed launch can never
    // resume provider history.
    assert!(matches!(
        gate_decision(CURSOR_C1_UNPROBED_VERSION, Some("composer-1"), None, true),
        CursorContinuationDecision::Incompatible { .. }
    ));
    // Cross-engine resume never proceeds, even with a fresh CLI and model.
    assert!(matches!(
        gate_decision("2026.08.11-e8db854", Some("composer-1"), None, false),
        CursorContinuationDecision::Incompatible { .. }
    ));
    // Advertisement is enforced only when an inventory is supplied.
    assert!(matches!(
        gate_decision(
            "2026.08.11-e8db854",
            Some("composer-1"),
            Some(&["other-model"]),
            true
        ),
        CursorContinuationDecision::Incompatible { .. }
    ));
}

#[test]
fn cursor_cli_version_floor_parses_dated_versions() {
    assert!(cursor_cli_meets_minimum(
        "2026.08.11-e8db854",
        CURSOR_CONTINUATION_MINIMUM_CLI_VERSION
    ));
    assert!(cursor_cli_meets_minimum(
        "agent 2026.9.6-stable.1",
        CURSOR_CONTINUATION_MINIMUM_CLI_VERSION
    ));
    assert!(cursor_cli_meets_minimum(
        "2027.1.1-alpha",
        CURSOR_CONTINUATION_MINIMUM_CLI_VERSION
    ));
    assert!(!cursor_cli_meets_minimum(
        "2026.08.10-nightly",
        CURSOR_CONTINUATION_MINIMUM_CLI_VERSION
    ));
    assert!(!cursor_cli_meets_minimum(
        CURSOR_C1_UNPROBED_VERSION,
        CURSOR_CONTINUATION_MINIMUM_CLI_VERSION
    ));
    assert!(!cursor_cli_meets_minimum(
        "no version here",
        CURSOR_CONTINUATION_MINIMUM_CLI_VERSION
    ));
    assert!(!cursor_cli_meets_minimum(
        "",
        CURSOR_CONTINUATION_MINIMUM_CLI_VERSION
    ));
}

// ---------------------------------------------------------------------------
// C3: resume reopens the same session id over start options
// ---------------------------------------------------------------------------

#[test]
fn cursor_resume_session_id_is_bounded() {
    assert_eq!(
        cursor_resume_session_id("sess-cursor-1").as_deref(),
        Some("sess-cursor-1")
    );
    assert_eq!(cursor_resume_session_id(""), None);
    assert_eq!(cursor_resume_session_id(&"s".repeat(257)), None);
    assert_eq!(
        cursor_resume_session_id(&"s".repeat(CURSOR_MAX_SESSION_ID_BYTES)).as_deref(),
        Some("s".repeat(CURSOR_MAX_SESSION_ID_BYTES).as_str())
    );
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
    writer
        .write_all(line.as_bytes())
        .await
        .expect("agent writes");
    writer.write_all(b"\n").await.expect("agent writes");
    writer.flush().await.expect("agent flushes");
}

fn update_text(update: &SessionUpdate) -> &str {
    update
        .update
        .value()
        .get("content")
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
        .expect("text content")
}

#[tokio::test(flavor = "current_thread")]
async fn cursor_resume_reopens_the_same_session_id() {
    let bounds = strict_bounds();
    let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (driver_read, driver_write) = tokio::io::split(driver_io);
    let (agent_read_half, agent_write_half) = tokio::io::split(agent_io);
    let mut agent_read = BufReader::new(agent_read_half);
    let mut agent_write = agent_write_half;

    let stored = cursor_resume_session_id("sess-cursor-7").expect("stored session validates");
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

        // Resume must arrive as `session/load`, never as a second
        // `session/new`: provider effects are never duplicated.
        let load = agent_read_value(&mut agent_read)
            .await
            .expect("session/load request");
        assert_eq!(
            load.get("method").and_then(Value::as_str),
            Some("session/load")
        );
        assert_eq!(
            load.pointer("/params/sessionId").and_then(Value::as_str),
            Some("sess-cursor-7")
        );
        let load_id = load.get("id").cloned().expect("session/load id");
        agent_write_line(
            &mut agent_write,
            &json!({ "jsonrpc": "2.0", "id": load_id, "result": {} }).to_string(),
        )
        .await;

        let prompt = agent_read_value(&mut agent_read)
            .await
            .expect("session/prompt request");
        assert_eq!(
            prompt.get("method").and_then(Value::as_str),
            Some("session/prompt")
        );
        assert_eq!(
            prompt.pointer("/params/sessionId").and_then(Value::as_str),
            Some("sess-cursor-7")
        );
        let prompt_id = prompt.get("id").cloned().expect("prompt id");
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "sess-cursor-7",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": { "type": "text", "text": "replayed" },
                    },
                },
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
    });

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    let init = driver.initialize().await.expect("handshake");
    assert_eq!(init.protocol_version, 1);
    driver
        .authenticate("cursor_login")
        .await
        .expect("authenticate");
    let session = SessionId::parse(&stored, CURSOR_MAX_SESSION_ID_BYTES).expect("session");
    driver
        .load_session(&session, "C:\\work")
        .await
        .expect("session/load reopens the stored session");
    assert_eq!(session.as_str(), "sess-cursor-7");
    let prompt_id = driver
        .prompt(
            &session,
            vec![json!({ "type": "text", "text": "continue" })],
        )
        .await
        .expect("prompt");
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("replayed delta")
    {
        UpdateEvent::SessionUpdate(update) => {
            assert_eq!(update.session.as_str(), "sess-cursor-7");
            assert_eq!(update_text(&update), "replayed");
        }
        other => panic!("expected session update, got {other:?}"),
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
    agent.await.expect("agent joins");
}

// ---------------------------------------------------------------------------
// C3: usage basis rules (cumulative per the ACP disclosure, gauge replaces)
// ---------------------------------------------------------------------------

#[test]
fn cursor_prompt_usage_decodes_to_a_cumulative_sample() {
    let sample: CursorUsageSample = parse_cursor_prompt_usage(&json!({
        "inputTokens": 40,
        "outputTokens": 9,
        "cachedReadTokens": 12,
    }))
    .expect("usage decodes");
    assert_eq!(sample.input, Some(40));
    assert_eq!(sample.output, Some(9));
    assert_eq!(sample.cached_input, Some(12));
    assert_eq!(sample.context, None);
    assert_eq!(sample.context_window, None);

    // An empty measurement stays observable without a report.
    assert_eq!(parse_cursor_prompt_usage(&json!({})), None);
    assert_eq!(parse_cursor_prompt_usage(&Value::Null), None);

    // Non-u64 numerics fail closed to absent instead of poisoning the turn.
    assert_eq!(
        parse_cursor_prompt_usage(&json!({ "inputTokens": -1 })),
        None
    );
    assert_eq!(
        parse_cursor_prompt_usage(&json!({ "outputTokens": 9.5 })),
        None
    );

    // Zero is preserved and distinct from absent.
    let zero = parse_cursor_prompt_usage(&json!({
        "inputTokens": 0,
        "outputTokens": 0,
    }))
    .expect("zero sample");
    assert_eq!(zero.input, Some(0));
    assert_eq!(zero.output, Some(0));
    assert_eq!(zero.cached_input, None);
}

#[test]
fn cursor_usage_update_decodes_to_a_replacing_gauge() {
    let sample =
        parse_cursor_usage_update(&json!({ "used": 41, "size": 200_000 })).expect("gauge decodes");
    assert_eq!(sample.context, Some(41));
    assert_eq!(sample.context_window, Some(200_000));
    assert_eq!(sample.input, None);

    // Zero gauges are preserved and distinct from absent.
    let zero = parse_cursor_usage_update(&json!({ "used": 0, "size": 0 })).expect("zero gauge");
    assert_eq!(zero.context, Some(0));
    assert_eq!(zero.context_window, Some(0));

    // An empty measurement stays observable without a report.
    assert_eq!(parse_cursor_usage_update(&json!({})), None);
    assert_eq!(
        parse_cursor_usage_update(&json!({ "sessionUpdate": "agent_message_chunk" })),
        None
    );
}

#[test]
fn cursor_usage_report_is_cumulative_with_a_replacing_gauge() {
    let run = run_id();
    let thread = ThreadId::parse("thread-usage-1").expect("thread id");
    let model = EngineModelId::parse("model-usage-1").expect("model id");
    let context = CursorUsageContext {
        run_id: &run,
        thread_id: &thread,
        provider_session_id: "sess-cursor-7",
        model_id: &model,
        observed_at: UnixMillis::from_millis(7),
    };
    let sample = parse_cursor_prompt_usage(&json!({
        "inputTokens": 40,
        "outputTokens": 9,
        "cachedReadTokens": 12,
    }))
    .expect("sample");
    let report = cursor_usage_report(&context, Some("turn-1".to_owned()), 3, &sample)
        .expect("report builds");
    assert_eq!(report.basis(), RunUsageBasis::Cumulative);
    assert_eq!(report.provider_session_id(), "sess-cursor-7");
    assert_eq!(report.provider_turn_id(), Some("turn-1"));
    assert_eq!(report.source_sequence(), 3);
    assert_eq!(report.input_tokens(), Some(40));
    assert_eq!(report.output_tokens(), Some(9));
    assert_eq!(report.cached_input_tokens(), Some(12));
    // Prompt totals carry no window gauge: absent stays absent.
    assert_eq!(report.context_tokens(), None);
    assert_eq!(report.context_window_tokens(), None);

    // The window gauge replaces: the codec performs no arithmetic.
    let gauge = parse_cursor_usage_update(&json!({ "used": 41, "size": 200_000 })).expect("gauge");
    let report = cursor_usage_report(&context, None, 4, &gauge).expect("gauge report");
    assert_eq!(report.basis(), RunUsageBasis::Cumulative);
    assert_eq!(report.context_tokens(), Some(41));
    assert_eq!(report.context_window_tokens(), Some(200_000));
    assert_eq!(report.input_tokens(), None);
    assert_eq!(report.provider_turn_id(), None);

    // Empty measurements are never reports.
    assert_eq!(parse_cursor_prompt_usage(&json!({})), None);
    assert_eq!(parse_cursor_usage_update(&json!({})), None);
}

#[tokio::test]
async fn cursor_usage_projects_an_observation_without_blocking_the_turn() {
    let run = run_id();
    let attribution = CursorUsageAttribution {
        thread_id: ThreadId::parse("thread-usage-1").expect("thread id"),
        model_id: EngineModelId::parse("model-usage-1").expect("model id"),
    };
    let scope = CursorUsageScope {
        thread_id: &attribution.thread_id,
        model_id: &attribution.model_id,
        provider_session_id: "sess-cursor-7",
    };
    let sample = parse_cursor_prompt_usage(&json!({
        "inputTokens": 40,
        "outputTokens": 9,
    }))
    .expect("sample");
    let (sender, mut receiver) = mpsc::channel(8);
    let terminal = project_cursor_usage_sample(
        &sender,
        &run,
        Some(&scope),
        Some("turn-9".to_owned()),
        9,
        &sample,
    )
    .await;
    assert_eq!(terminal, None, "usage never settles the turn");
    let EngineObservation::Usage(observation) = receiver.try_recv().expect("usage observed") else {
        panic!("expected a usage observation");
    };
    assert_eq!(observation.report().basis(), RunUsageBasis::Cumulative);
    assert_eq!(observation.report().input_tokens(), Some(40));
    assert_eq!(observation.report().provider_turn_id(), Some("turn-9"));
    assert_eq!(observation.report().source_sequence(), 9);

    // Without attribution the same sample is a diagnostic: no observation,
    // no terminal, and the turn continues.
    let (sender, mut receiver) = mpsc::channel(8);
    let terminal = project_cursor_usage_sample(&sender, &run, None, None, 10, &sample).await;
    assert_eq!(terminal, None);
    assert!(receiver.try_recv().is_err(), "no usage without scope");

    // A closed sink is the only terminal usage outcome.
    let (sender, receiver) = mpsc::channel(8);
    drop(receiver);
    let terminal =
        project_cursor_usage_sample(&sender, &run, Some(&scope), None, 11, &sample).await;
    assert_eq!(terminal, Some(TerminalState::Interrupted));
}

// ---------------------------------------------------------------------------
// C3: dashboard quota mapping (split pools, clamp, kinds, resets)
// ---------------------------------------------------------------------------

fn dashboard_fixture() -> Value {
    json!({
        "planUsage": {
            "totalSpend": 80,
            "limit": 100,
            "autoPercentUsed": 120,
            "apiPercentUsed": -5,
        },
        "billingCycleStart": 1_758_000_000_000_u64,
        "billingCycleEnd": 1_760_592_000_000_u64,
        "autoBucketModels": [],
        "displayMessage": "80% used",
        "spendLimitUsage": {
            "overallLimit": 50,
            "overallUsed": 60,
            "overallRemaining": 0,
        },
    })
}

#[test]
fn cursor_quota_windows_map_split_pools_with_clamp_and_kinds() {
    let windows = map_cursor_quota_windows(&dashboard_fixture());
    assert_eq!(windows.len(), 3);

    assert_eq!(windows[0].id, "cursor:cursor-models");
    assert_eq!(windows[0].kind, CursorQuotaWindowKind::Monthly);
    assert_eq!(windows[0].label.as_deref(), Some("Cursor models"));
    assert!(
        (windows[0].percent_used - 100.0).abs() < 1e-9,
        "over-full gauge clamps to 100"
    );
    assert_eq!(windows[0].scope, "shared");
    assert_eq!(windows[0].window_minutes, Some(43_200));

    assert_eq!(windows[1].id, "cursor:other-models");
    assert_eq!(windows[1].kind, CursorQuotaWindowKind::Monthly);
    assert_eq!(windows[1].label.as_deref(), Some("Other models"));
    assert!(
        (windows[1].percent_used - 0.0).abs() < 1e-9,
        "negative gauge clamps to 0"
    );
    assert_eq!(windows[1].scope, "shared");

    assert_eq!(windows[2].id, "cursor:on-demand");
    assert_eq!(windows[2].kind, CursorQuotaWindowKind::Monthly);
    assert_eq!(windows[2].label.as_deref(), Some("On-demand"));
    assert!(
        (windows[2].percent_used - 100.0).abs() < 1e-9,
        "over-full on-demand gauge clamps to 100"
    );
    assert_eq!(windows[2].scope, "shared");

    // All windows share the billing-cycle reset and period.
    let resets_at = windows[0].resets_at.clone().expect("reset instant");
    assert!(
        windows
            .iter()
            .all(|window| window.resets_at.as_deref() == Some(resets_at.as_str()))
    );
    assert_eq!(
        windows[0].resets_at.as_deref(),
        cursor_reset_at_iso(1_760_592_000_000)
    );

    // Foreign ids are never monthly quota.
    assert_eq!(
        classify_cursor_quota_window_kind("codex:primary"),
        CursorQuotaWindowKind::Unknown
    );
    assert_eq!(
        classify_cursor_quota_window_kind(""),
        CursorQuotaWindowKind::Unknown
    );
    assert!(
        clamp_cursor_percent_used(None).abs() < 1e-9,
        "absent gauge becomes 0"
    );
    assert!(
        (clamp_cursor_percent_used(Some(33.5)) - 33.5).abs() < 1e-9,
        "in-range gauge passes through"
    );
}

#[test]
fn cursor_quota_windows_fall_back_to_included_usage_and_message_percent() {
    // No split disclosure: one included-usage pool from spend over limit.
    let single = map_cursor_quota_windows(&json!({
        "planUsage": { "totalSpend": 30, "limit": 100 },
    }));
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].id, "cursor:included-usage");
    assert_eq!(single[0].kind, CursorQuotaWindowKind::Monthly);
    assert!((single[0].percent_used - 30.0).abs() < 1e-9);
    assert_eq!(single[0].resets_at, None);
    assert_eq!(single[0].window_minutes, None);

    // Zero limit falls back to the provider percent, then the message gauge.
    let provider = map_cursor_quota_windows(&json!({
        "planUsage": { "totalSpend": 30, "limit": 0, "totalPercentUsed": 42 },
    }));
    assert_eq!(provider.len(), 1);
    assert!((provider[0].percent_used - 42.0).abs() < 1e-9);

    let messaged = map_cursor_quota_windows(&json!({
        "planUsage": { "totalSpend": 30, "limit": 0 },
        "displayMessage": "Used 7.5% of quota",
    }));
    assert_eq!(messaged.len(), 1);
    assert!((messaged[0].percent_used - 7.5).abs() < 1e-9);

    // The bucket discriminator also fires on the repeated field alone.
    let bucketed = map_cursor_quota_windows(&json!({
        "planUsage": {},
        "autoBucketModels": ["composer-1"],
    }));
    assert_eq!(bucketed.len(), 2);
    assert_eq!(bucketed[0].id, "cursor:cursor-models");
    assert!(bucketed[0].percent_used.abs() < 1e-9);
}

#[test]
fn cursor_quota_windows_cover_individual_pooled_and_remaining_math() {
    // Individual is used when overall carries no limit.
    let individual = map_cursor_quota_windows(&json!({
        "planUsage": { "totalSpend": 1, "limit": 10 },
        "spendLimitUsage": {
            "overallLimit": 0,
            "individualLimit": 40,
            "individualUsed": 10,
        },
    }));
    assert_eq!(individual.len(), 2);
    assert_eq!(individual[1].id, "cursor:on-demand");
    assert!((individual[1].percent_used - 25.0).abs() < 1e-9);

    // Remaining derives used when the provider omits it.
    let derived = map_cursor_quota_windows(&json!({
        "planUsage": { "totalSpend": 1, "limit": 10 },
        "spendLimitUsage": { "pooledLimit": 100, "pooledRemaining": 30 },
    }));
    assert_eq!(derived.len(), 2);
    assert!((derived[1].percent_used - 70.0).abs() < 1e-9);

    // Malformed input yields no windows: diagnostics never block a turn.
    for malformed in [
        json!(null),
        json!({}),
        json!({ "planUsage": null }),
        json!({ "planUsage": { "totalSpend": "lots" } }),
        json!({ "planUsage": { "totalSpend": 1e999 } }),
    ] {
        assert!(
            map_cursor_quota_windows(&malformed).is_empty(),
            "malformed dashboard input maps to no windows: {malformed}"
        );
    }
}

#[test]
fn cursor_reset_instants_resolve_without_a_date_dependency() {
    assert_eq!(
        cursor_reset_at_iso(0).as_deref(),
        Some("1970-01-01T00:00:00Z")
    );
    assert_eq!(
        cursor_reset_at_iso(1_767_225_600_000).as_deref(),
        Some("2026-01-01T00:00:00Z")
    );
    assert_eq!(cursor_reset_at_iso(-1), None);
}

// ---------------------------------------------------------------------------
// C3: teardown kills the whole group; quarantine stays on unobserved reaps
// ---------------------------------------------------------------------------

#[test]
fn cursor_teardown_requires_group_termination() {
    assert!(
        cursor_requires_group_termination(),
        "teardown must kill the whole process group so no cursor grandchild \
         holding a pipe is orphaned; unobserved reaps quarantine through \
         shutdown_acp_child and finish_turn_result"
    );
}

struct CursorFixtureScript {
    directory: PathBuf,
}

impl CursorFixtureScript {
    fn new(responses: &str, tail: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("artisan-cursor-{}-{nonce}", std::process::id()));
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

    fn program(&self) -> OsString {
        #[cfg(windows)]
        {
            OsString::from("cmd")
        }
        #[cfg(not(windows))]
        {
            self.directory.join("fixture.sh").into_os_string()
        }
    }

    fn args(&self) -> Vec<OsString> {
        #[cfg(windows)]
        {
            vec![
                OsString::from("/C"),
                self.directory.join("fixture.cmd").into_os_string(),
            ]
        }
        #[cfg(not(windows))]
        {
            Vec::new()
        }
    }
}

impl Drop for CursorFixtureScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_kill_reports_interruption_with_durable_prefix() {
    let responses = [
        r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"authMethods":[{"id":"cursor_login"}]}}"#,
        r#"{"jsonrpc":"2.0","id":2,"result":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"result":{"sessionId":"sess-cursor-1"}}"#,
        r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-cursor-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"durable-"}}}}"#,
    ]
    .join("\n")
        + "\n";
    // Pipe-holding tail: the leader is killed below while this holder still
    // inherits stdout, so EOF (and the interruption) must arrive bounded
    // without an orphan wedging the read. The tail outlives the first
    // teardown wait so the group kill is always the path that reaps it.
    #[cfg(windows)]
    let tail = "ping -n 13 127.0.0.1 >nul";
    #[cfg(not(windows))]
    let tail = "sleep 12";
    let script = CursorFixtureScript::new(&responses, tail);
    let program = script.program();
    let args = script.args();
    let mut child = match spawn_acp_child(program.as_os_str(), &args, None) {
        Ok(child) => child,
        Err(_) => {
            eprintln!("SKIP: fixture child spawn unavailable");
            return;
        }
    };
    assert!(child.id().is_some(), "spawned child reports a pid");
    let pipes = child.take_pipes().expect("piped stdio");
    drop(pipes.stderr);
    let mut reader = BufReader::new(pipes.stdout);
    let stdin = pipes.stdin;

    // The durable prefix lands before the kill: handshake, auth, session,
    // then one text delta.
    let mut prefix = String::new();
    for _ in 0..4 {
        let mut line = String::new();
        let count = tokio::time::timeout(Duration::from_secs(15), reader.read_line(&mut line))
            .await
            .expect("prefix lines arrive bounded")
            .expect("prefix lines readable");
        assert!(count > 0, "no EOF before the durable prefix");
        prefix.push_str(&line);
    }
    assert!(
        prefix.contains("durable-"),
        "durable prefix lands before the kill"
    );

    // Close the lifeline first, then run the fixed cursor teardown: bounded
    // wait, group kill only when no exit was observed, one more wait on the
    // remaining budget. The tail outlives the first wait, so the kill path
    // is deterministic and the second wait observes the reap.
    drop(stdin);
    let shutdown = shutdown_acp_child(child, Duration::from_secs(5)).await;
    assert!(
        matches!(shutdown, AcpShutdown::ReapedAfterKill(_)),
        "the pipe-holding tail must be killed, not left orphaned"
    );

    // EOF arrives bounded once the group is gone: no orphan wedges the pipe,
    // and EOF before a terminal maps to interruption.
    let mut rest = String::new();
    let eof = tokio::time::timeout(Duration::from_secs(30), reader.read_line(&mut rest)).await;
    let terminal = match eof {
        Ok(Ok(0)) => Some(TerminalState::Interrupted),
        Ok(Ok(_)) => None,
        _ => None,
    };
    assert_eq!(terminal, Some(TerminalState::Interrupted));
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_restart_after_kill_replays_prefix_on_the_same_session() {
    let interrupted = tokio::time::timeout(Duration::from_secs(60), fixture_kill_prefix_run())
        .await
        .expect("first attempt finishes");
    assert_eq!(interrupted, "durable-");

    // The restart resumes provider-owned state: the same session id reopens
    // through session/load instead of duplicating provider effects with a
    // second session/new.
    let stored = cursor_resume_session_id("sess-cursor-1").expect("stored session validates");
    assert_eq!(
        cursor_resume_session_id(&stored).as_deref(),
        Some("sess-cursor-1")
    );

    let bounds = strict_bounds();
    let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
    let (driver_read, driver_write) = tokio::io::split(driver_io);
    let (agent_read_half, agent_write_half) = tokio::io::split(agent_io);
    let mut agent_read = BufReader::new(agent_read_half);
    let mut agent_write = agent_write_half;
    let agent = tokio::spawn(async move {
        // Greet the resumed run: initialize, authenticate, then exactly one
        // session/load. A second session/new here would duplicate provider
        // effects, so any other method fails the test.
        loop {
            let frame = agent_read_value(&mut agent_read)
                .await
                .expect("resume handshake frame");
            let method = frame.get("method").and_then(Value::as_str).expect("method");
            let id = frame.get("id").cloned().expect("frame id");
            match method {
                "initialize" => {
                    agent_write_line(
                        &mut agent_write,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "protocolVersion": 1,
                                "authMethods": [{ "id": "cursor_login" }],
                            },
                        })
                        .to_string(),
                    )
                    .await;
                }
                "authenticate" => {
                    agent_write_line(
                        &mut agent_write,
                        &json!({ "jsonrpc": "2.0", "id": id, "result": {} }).to_string(),
                    )
                    .await;
                }
                "session/load" => {
                    assert_eq!(
                        frame.pointer("/params/sessionId").and_then(Value::as_str),
                        Some("sess-cursor-1")
                    );
                    agent_write_line(
                        &mut agent_write,
                        &json!({ "jsonrpc": "2.0", "id": id, "result": {} }).to_string(),
                    )
                    .await;
                    break;
                }
                other => panic!("resume must not start a second session, got {other}"),
            }
        }
        let prompt = agent_read_value(&mut agent_read)
            .await
            .expect("session/prompt request");
        assert_eq!(
            prompt.pointer("/params/sessionId").and_then(Value::as_str),
            Some("sess-cursor-1")
        );
        let prompt_id = prompt.get("id").cloned().expect("prompt id");
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "sess-cursor-1",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": { "type": "text", "text": "replayed" },
                    },
                },
            })
            .to_string(),
        )
        .await;
        agent_write_line(
            &mut agent_write,
            &json!({
                "jsonrpc": "2.0",
                "id": prompt_id,
                "result": { "stopReason": "completed" },
            })
            .to_string(),
        )
        .await;
    });

    let completed = tokio::time::timeout(Duration::from_secs(30), async {
        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        driver.initialize().await.expect("handshake");
        driver
            .authenticate("cursor_login")
            .await
            .expect("authenticate");
        let session = SessionId::parse(&stored, CURSOR_MAX_SESSION_ID_BYTES).expect("session");
        driver
            .load_session(&session, "C:\\work")
            .await
            .expect("session/load");
        let prompt_id = driver
            .prompt(&session, vec![json!({ "type": "text", "text": "go" })])
            .await
            .expect("prompt");
        let mut tail = String::new();
        loop {
            match driver
                .next_update(&session, &prompt_id)
                .await
                .expect("update")
            {
                UpdateEvent::SessionUpdate(update) => tail.push_str(update_text(&update)),
                UpdateEvent::PromptResult(_) => break,
                UpdateEvent::AgentRequest { .. } => {}
            }
        }
        agent.await.expect("agent joins");
        tail
    })
    .await
    .expect("restart finishes");
    assert_eq!(format!("{interrupted}{completed}"), "durable-replayed");
}

async fn fixture_kill_prefix_run() -> String {
    let responses = [
        r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"authMethods":[{"id":"cursor_login"}]}}"#,
        r#"{"jsonrpc":"2.0","id":2,"result":{}}"#,
        r#"{"jsonrpc":"2.0","id":3,"result":{"sessionId":"sess-cursor-1"}}"#,
        r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-cursor-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"durable-"}}}}"#,
    ]
    .join("\n")
        + "\n";
    #[cfg(windows)]
    let tail = "ping -n 13 127.0.0.1 >nul";
    #[cfg(not(windows))]
    let tail = "sleep 12";
    let script = CursorFixtureScript::new(&responses, tail);
    let program = script.program();
    let args = script.args();
    let mut child = spawn_acp_child(program.as_os_str(), &args, None).expect("fixture spawns");
    let script = CursorFixtureScript::new(&responses, tail);
    let program = script.program();
    let args = script.args();
    let mut child = spawn_acp_child(program.as_os_str(), &args, None).expect("fixture spawns");
    let pipes = child.take_pipes().expect("piped stdio");
    drop(pipes.stderr);
    let mut reader = BufReader::new(pipes.stdout);
    drop(pipes.stdin);
    let mut prefix = String::new();
    for _ in 0..4 {
        let mut line = String::new();
        let count = reader
            .read_line(&mut line)
            .await
            .expect("prefix lines readable");
        assert!(count > 0, "no EOF before the durable prefix");
        if line.contains("durable-") {
            prefix.push_str("durable-");
        }
    }
    let _ = shutdown_acp_child(child, Duration::from_secs(5)).await;
    prefix
}
