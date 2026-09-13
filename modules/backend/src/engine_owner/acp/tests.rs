#![allow(clippy::module_name_repetitions)]
#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::io;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::time::Instant;

use super::*;
use tokio::io::{AsyncBufReadExt, DuplexStream, ReadHalf, WriteHalf, duplex, split};

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

fn launch_args() -> LaunchArgs {
    LaunchArgs {
        model: None,
        reasoning_effort: None,
        speed_fast: false,
        permission: None,
        write_access: true,
    }
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

#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end fixture lifecycle proof; splitting would duplicate the duplex wiring"
)]
#[tokio::test(flavor = "current_thread")]
async fn fixture_agent_lifecycle_initialize_session_updates_close() {
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
                "result": { "protocolVersion": 1, "authMethods": [{ "id": "cached_token" }] },
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
                "result": { "sessionId": "sess-1" },
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
        let prompt_id = prompt.get("id").cloned().expect("prompt id");
        for index in [1, 2] {
            agent_write_line(&mut agent_write, &update_line("sess-1", index)).await;
        }
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": prompt_id,
                "result": {
                    "stopReason": "completed",
                    "usage": { "inputTokens": 3, "outputTokens": 5 },
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

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    let init = driver.initialize().await.expect("handshake");
    assert_eq!(init.protocol_version, ACP_PROTOCOL_VERSION);
    assert_eq!(init.auth_methods, vec!["cached_token".to_owned()]);
    let available: Vec<&str> = init.auth_methods.iter().map(String::as_str).collect();
    assert_eq!(
        (GROK_ACP.select_auth_method)(&available, false),
        Some("cached_token")
    );
    driver
        .authenticate("cached_token")
        .await
        .expect("authenticate");
    let session = driver.new_session("C:\\work").await.expect("session/new");
    assert_eq!(session.as_str(), "sess-1");
    let content =
        build_prompt_content(ImageMode::Embedded, "hello", &[], None).expect("prompt content");
    let prompt_id = driver.prompt(&session, content).await.expect("prompt");
    for _ in [1, 2] {
        let event = driver
            .next_update(&session, &prompt_id)
            .await
            .expect("session update");
        assert!(
            matches!(event, UpdateEvent::SessionUpdate(_)),
            "expected update, got {event:?}"
        );
    }
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("prompt result")
    {
        UpdateEvent::PromptResult(outcome) => {
            assert!(!outcome.cancelled);
            let usage = outcome.usage.expect("usage reported");
            assert_eq!(usage.input, 3);
            assert_eq!(usage.output, 5);
            assert_eq!(usage.cached_input, None);
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
async fn oversize_request_rejected_before_any_write() {
    let bounds = AcpBounds::new(
        4096,
        128,
        256,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(2),
    )
    .expect("test bounds hold");
    let (driver_io, agent_io) = duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, agent_write_half) = split(agent_io);
    let mut agent_read = BufReader::new(agent_read_half);

    let agent = tokio::spawn(async move {
        drop(agent_write_half);
        let mut line = String::new();
        agent_read
            .read_line(&mut line)
            .await
            .expect("agent reads one frame");
        assert!(
            line.contains(METHOD_SESSION_CANCEL),
            "first agent frame must be the cancel, got: {line}"
        );
    });

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    let big = Value::String("x".repeat(256));
    let error = driver
        .send_request(METHOD_SESSION_PROMPT, big)
        .await
        .expect_err("oversize envelope");
    assert_eq!(error, AcpError::EnvelopeTooLarge);
    let session = SessionId::parse("sess-1", 256).expect("session");
    driver.cancel(&session).await.expect("stream stays usable");
    driver.shutdown_writer().await.expect("close");
    drop(driver);
    agent.await.expect("agent joins");
}

#[tokio::test(flavor = "current_thread")]
async fn inactivity_stall_after_silence_and_rearm_on_activity() {
    let bounds = AcpBounds::new(
        4096,
        16_384,
        256,
        Duration::from_secs(5),
        Duration::from_millis(400),
        Duration::from_secs(2),
    )
    .expect("test bounds hold");
    let (driver_io, agent_io) = duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, mut agent_write) = split(agent_io);
    drop(agent_read_half);

    let agent = tokio::spawn(async move {
        agent_write_line(&mut agent_write, &update_line("sess-stall", 1)).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        agent_write_line(&mut agent_write, &update_line("sess-stall", 2)).await;
        // Hold the pipe open past the stall window: the third read must
        // observe silence on a live pipe, not EOF from a dropped writer.
        tokio::time::sleep(Duration::from_millis(1_000)).await;
    });

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    assert!(
        driver.activity_deadline() > Instant::now(),
        "rolling deadline sits one window ahead"
    );
    let session = SessionId::parse("sess-stall", 256).expect("session");
    let prompt_id = AcpId::Number(7);
    let first = driver
        .next_update(&session, &prompt_id)
        .await
        .expect("first update inside the window");
    assert!(matches!(first, UpdateEvent::SessionUpdate(_)));
    let second = driver
        .next_update(&session, &prompt_id)
        .await
        .expect("activity re-arms the deadline");
    assert!(matches!(second, UpdateEvent::SessionUpdate(_)));
    let stall = driver
        .next_update(&session, &prompt_id)
        .await
        .expect_err("silence must stall");
    assert_eq!(stall, AcpError::InactivityStall);
    agent.await.expect("agent joins");
}

#[tokio::test(flavor = "current_thread")]
async fn load_session_resume_round_trip() {
    let bounds = strict_bounds();
    let (driver_io, agent_io) = duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, mut agent_write) = split(agent_io);
    let mut agent_read = BufReader::new(agent_read_half);

    let agent = tokio::spawn(async move {
        let first = agent_read_value(&mut agent_read)
            .await
            .expect("session/load request");
        assert_eq!(
            first.get("method").and_then(Value::as_str),
            Some(METHOD_SESSION_LOAD)
        );
        assert_eq!(
            first
                .get("params")
                .and_then(|params| params.get("sessionId"))
                .and_then(Value::as_str),
            Some("sess-9")
        );
        let first_id = first.get("id").cloned().expect("load id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": first_id,
                "result": { "sessionId": "sess-9" },
            })
            .to_string(),
        )
        .await;

        let second = agent_read_value(&mut agent_read)
            .await
            .expect("second session/load request");
        let second_id = second.get("id").cloned().expect("load id");
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": second_id,
                "error": { "code": -32_000 },
            })
            .to_string(),
        )
        .await;
    });

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    let session = SessionId::parse("sess-9", 256).expect("session");
    driver
        .load_session(&session, "C:\\work")
        .await
        .expect("resume");
    let error = driver
        .load_session(&session, "C:\\work")
        .await
        .expect_err("rejected resume");
    assert_eq!(error, AcpError::ChildFailed);
    agent.await.expect("agent joins");
}

#[tokio::test(flavor = "current_thread")]
async fn update_loop_skips_foreign_frames_and_surfaces_agent_requests() {
    let bounds = strict_bounds();
    let (driver_io, agent_io) = duplex(64 * 1024);
    let (driver_read, driver_write) = split(driver_io);
    let (agent_read_half, mut agent_write) = split(agent_io);
    drop(agent_read_half);

    let agent = tokio::spawn(async move {
        agent_write_line(&mut agent_write, &update_line("other-sess", 9)).await;
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 99,
                "method": "requestPermission",
                "params": { "toolCall": { "toolCallId": "t1" } },
            })
            .to_string(),
        )
        .await;
        agent_write_line(&mut agent_write, &update_line("sess-1", 1)).await;
        agent_write_line(
            &mut agent_write,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 5,
                "result": { "stopReason": "cancelled" },
            })
            .to_string(),
        )
        .await;
    });

    let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
    let session = SessionId::parse("sess-1", 256).expect("session");
    let prompt_id = AcpId::Number(5);
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("agent request")
    {
        UpdateEvent::AgentRequest { id, method, .. } => {
            assert_eq!(id, AcpId::Number(99));
            assert_eq!(method, "requestPermission");
        }
        other => panic!("expected agent request, got {other:?}"),
    }
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("own update")
    {
        UpdateEvent::SessionUpdate(update) => {
            assert_eq!(update.session.as_str(), "sess-1");
        }
        other => panic!("expected session update, got {other:?}"),
    }
    match driver
        .next_update(&session, &prompt_id)
        .await
        .expect("prompt result")
    {
        UpdateEvent::PromptResult(outcome) => {
            assert!(outcome.cancelled);
            assert_eq!(outcome.usage, None);
        }
        other => panic!("expected prompt result, got {other:?}"),
    }
    agent.await.expect("agent joins");
}

#[cfg(windows)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the cfg(not(windows)) variant returns None when no sleep binary exists; both variants share one Option-returning signature"
)]
fn silent_idle_child_command() -> Option<(OsString, Vec<OsString>)> {
    Some((
        OsString::from("powershell"),
        vec![
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-Command"),
            OsString::from("Start-Sleep -Seconds 60"),
        ],
    ))
}

#[cfg(not(windows))]
fn silent_idle_child_command() -> Option<(OsString, Vec<OsString>)> {
    for candidate in ["/bin/sleep", "/usr/bin/sleep"] {
        if std::path::Path::new(candidate).is_file() {
            return Some((OsString::from(candidate), vec![OsString::from("60")]));
        }
    }
    None
}

#[tokio::test(flavor = "current_thread")]
async fn slow_agent_handshake_deadline_kill_with_custody() {
    let Some((program, args)) = silent_idle_child_command() else {
        eprintln!("SKIP: no silent idle child for the kill-custody proof");
        return;
    };
    let bounds = AcpBounds::new(
        4096,
        16_384,
        256,
        Duration::from_millis(300),
        Duration::from_millis(300),
        Duration::from_secs(5),
    )
    .expect("test bounds hold");
    let Ok(mut child) = spawn_acp_child(program.as_os_str(), &args, None) else {
        eprintln!("SKIP: idle child spawn unavailable");
        return;
    };
    assert!(child.id().is_some(), "spawned child reports a pid");
    let pipes = child.take_pipes().expect("piped stdio");
    drop(pipes.stderr);
    let mut driver = AcpTransport::new(BufReader::new(pipes.stdout), pipes.stdin, bounds);
    let error = driver
        .initialize()
        .await
        .expect_err("silent agent must miss the handshake deadline");
    assert_eq!(error, AcpError::HandshakeTimeout);
    driver.shutdown_writer().await.expect("lifeline close");
    drop(driver);
    match shutdown_acp_child(child, Duration::from_secs(5)).await {
        AcpShutdown::ReapedAfterKill(status) => {
            let observed = ObservedExit::from(status);
            assert!(
                matches!(
                    classify_exit(observed, false),
                    ClassifiedExit::Interrupted | ClassifiedExit::Failed { .. }
                ),
                "killed child classifies as interrupted or failed"
            );
        }
        AcpShutdown::ReapedWithoutKill(_) => {}
        AcpShutdown::Retained(_) => panic!("idle child must be reaped after kill"),
    }
}

#[test]
fn spawn_missing_executable_fails_without_shell() {
    let missing = if cfg!(windows) {
        "C:\\nonexistent\\artisan-acp-test-agent.exe"
    } else {
        "/nonexistent/artisan-acp-test-agent"
    };
    assert!(
        matches!(
            spawn_acp_child(OsStr::new(missing), &[], None),
            Err(error) if error.kind() == io::ErrorKind::NotFound
        ),
        "missing executable"
    );
}

#[test]
fn framer_splits_fragmented_lines_and_trims_cr() {
    let mut framer = AcpFramer::new(64);
    let first = framer.push(b"{\"a\":1}\n{\"b\":").expect("frames");
    assert_eq!(first, vec!["{\"a\":1}".to_owned()]);
    let second = framer.push(b"2}\r\n").expect("frames");
    assert_eq!(second, vec!["{\"b\":2}".to_owned()]);
}

#[test]
fn framer_rejects_oversize_line_and_poisons() {
    let mut framer = AcpFramer::new(8);
    assert_eq!(
        framer.push(b"123456789").expect_err("oversize"),
        AcpError::LineTooLong
    );
    assert_eq!(
        framer.push(b"\n").expect_err("poisoned"),
        AcpError::Poisoned
    );
}

#[test]
fn framer_rejects_invalid_utf8_and_discards_partial_on_finish() {
    let mut framer = AcpFramer::new(64);
    assert_eq!(
        framer.push(&[0x7b, 0xff, 0x7d, b'\n']).expect_err("utf8"),
        AcpError::InvalidUtf8
    );
    assert_eq!(
        framer.push(b"{}\n").expect_err("poisoned"),
        AcpError::Poisoned
    );
    assert_eq!(
        framer.finish().expect_err("poisoned finish"),
        AcpError::Poisoned
    );

    let mut clean = AcpFramer::new(64);
    assert!(clean.push(b"partial").expect("buffered").is_empty());
    clean.finish().expect("finish discards partial");
    assert_eq!(
        clean.push(b"x\n").expect_err("finished"),
        AcpError::Poisoned
    );
}

#[test]
fn malformed_envelopes_rejected() {
    let bound = 4096;
    let cases = [
        "",
        "not json",
        "[]",
        "null",
        "42",
        "\"x\"",
        "{\"jsonrpc\":\"2.0\"}",
        "{\"method\":\"m\"}",
        "{\"jsonrpc\":\"1.0\",\"id\":1,\"method\":\"m\"}",
        "{\"jsonrpc\":\"2.0\",\"id\":true,\"method\":\"m\"}",
        "{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"m\"}",
        "{\"jsonrpc\":\"2.0\",\"id\":1}",
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":1,\"error\":{\"code\":1}}",
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":42}",
        "{\"jsonrpc\":\"2.0\",\"id\":\"\",\"method\":\"m\"}",
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"m\",\"result\":1}",
        "{\"jsonrpc\":\"2.0\",\"id\":1.5,\"method\":\"m\"}",
        "{\"jsonrpc\":\"2.0\",\"method\":\"m\",\"params\":1,\"id\":2,\"result\":null}",
    ];
    for line in cases {
        assert_eq!(
            parse_envelope(line, bound).expect_err("malformed"),
            AcpError::MalformedEnvelope,
            "line: {line}"
        );
    }
    assert_eq!(
        parse_envelope("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"m\"}", 8).expect_err("oversize"),
        AcpError::EnvelopeTooLarge
    );
}

#[test]
fn valid_envelopes_round_trip() {
    let bound = 4096;
    match parse_envelope(
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/prompt\",\"params\":{\"a\":1}}",
        bound,
    )
    .expect("request")
    {
        AcpEnvelope::Request { id, method, params } => {
            assert_eq!(id, AcpId::Number(3));
            assert_eq!(method, "session/prompt");
            assert_eq!(params, serde_json::json!({"a": 1}));
        }
        other => panic!("expected request, got {other:?}"),
    }
    match parse_envelope("{\"jsonrpc\":\"2.0\",\"method\":\"session/update\"}", bound)
        .expect("notification")
    {
        AcpEnvelope::Notification { method, params } => {
            assert_eq!(method, "session/update");
            assert_eq!(params, Value::Null);
        }
        other => panic!("expected notification, got {other:?}"),
    }
    match parse_envelope(
        "{\"jsonrpc\":\"2.0\",\"id\":\"a\",\"result\":{\"sessionId\":\"s\"}}",
        bound,
    )
    .expect("response")
    {
        AcpEnvelope::Response { id, payload } => {
            assert_eq!(id, AcpId::Text("a".to_owned()));
            assert_eq!(
                payload,
                AcpResponsePayload::Result(serde_json::json!({"sessionId": "s"}))
            );
        }
        other => panic!("expected response, got {other:?}"),
    }
    match parse_envelope(
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"error\":{\"code\":-32600,\"message\":\"hidden\"}}",
        bound,
    )
    .expect("error response")
    {
        AcpEnvelope::Response { id, payload } => {
            assert_eq!(id, AcpId::Number(4));
            assert_eq!(payload, AcpResponsePayload::Error { code: -32_600 });
        }
        other => panic!("expected error response, got {other:?}"),
    }
}

#[test]
fn session_id_validation() {
    assert_eq!(
        SessionId::parse("sess-1", 256).expect("valid").as_str(),
        "sess-1"
    );
    assert_eq!(SessionId::parse("x", 1).expect("boundary").as_str(), "x");
    for bad in ["", "a/b", "a?b", "a#b", "a%b", "a b", "a\nb", "xy"] {
        let max = if bad == "xy" { 1 } else { 256 };
        assert!(
            SessionId::parse(bad, max).is_err(),
            "session id must reject {bad:?}"
        );
    }
}

#[test]
fn bounds_reject_zero_sizes() {
    let ok = || {
        AcpBounds::new(
            1024,
            1024,
            256,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
    };
    assert!(ok().is_ok());
    assert_eq!(
        AcpBounds::new(
            0,
            1024,
            256,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect_err("line"),
        AcpBoundsError::ZeroBound {
            field: "max_line_bytes"
        }
    );
    assert_eq!(
        AcpBounds::new(
            1024,
            0,
            256,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect_err("envelope"),
        AcpBoundsError::ZeroBound {
            field: "max_envelope_bytes"
        }
    );
    assert_eq!(
        AcpBounds::new(
            1024,
            1024,
            0,
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect_err("session"),
        AcpBoundsError::ZeroBound {
            field: "max_session_id_bytes"
        }
    );
}

#[test]
fn exit_classification_matrix() {
    assert_eq!(
        classify_exit(
            ObservedExit {
                code: None,
                signaled: false
            },
            false
        ),
        ClassifiedExit::Interrupted
    );
    assert_eq!(
        classify_exit(
            ObservedExit {
                code: None,
                signaled: true
            },
            false
        ),
        ClassifiedExit::Interrupted
    );
    assert_eq!(
        classify_exit(
            ObservedExit {
                code: Some(0),
                signaled: false
            },
            false
        ),
        ClassifiedExit::Failed { code: Some(0) }
    );
    assert_eq!(
        classify_exit(
            ObservedExit {
                code: Some(1),
                signaled: true
            },
            false
        ),
        ClassifiedExit::Interrupted
    );
    assert_eq!(
        classify_exit(
            ObservedExit {
                code: Some(1),
                signaled: false
            },
            false
        ),
        ClassifiedExit::Failed { code: Some(1) }
    );
    assert_eq!(
        classify_exit(
            ObservedExit {
                code: Some(0),
                signaled: false
            },
            true
        ),
        ClassifiedExit::Cancelled
    );
    assert_eq!(
        classify_exit(
            ObservedExit {
                code: None,
                signaled: true
            },
            true
        ),
        ClassifiedExit::Cancelled
    );
}

#[cfg(unix)]
#[test]
fn real_exit_status_converts() {
    let candidate = if std::path::Path::new("/bin/true").is_file() {
        "/bin/true"
    } else if std::path::Path::new("/usr/bin/true").is_file() {
        "/usr/bin/true"
    } else {
        eprintln!("SKIP: no true(1) for exit conversion");
        return;
    };
    let status = std::process::Command::new(candidate)
        .status()
        .expect("true runs");
    let observed = ObservedExit::from(status);
    assert!(!observed.signaled);
    assert_eq!(observed.code, Some(0));
    assert_eq!(
        classify_exit(observed, false),
        ClassifiedExit::Failed { code: Some(0) }
    );
}

#[test]
fn grok_version_parser_accepts_and_rejects() {
    assert_eq!(parse_grok_version("grok 0.7.3"), Some("0.7.3".to_owned()));
    assert_eq!(
        parse_grok_version("GROK 1.2.3-beta.1"),
        Some("1.2.3-beta.1".to_owned())
    );
    assert_eq!(
        parse_grok_version("my grok 10.20.30 done"),
        Some("10.20.30".to_owned())
    );
    for bad in [
        "0.7.3",
        "grokk 1.2.3",
        "grok-cli 2.0.0",
        "grok",
        "grok x.y.z",
        "grok 1.2",
        "grok 1.2.3-",
    ] {
        assert_eq!(parse_grok_version(bad), None, "grok rejects {bad:?}");
    }
    assert_eq!(
        (GROK_ACP.parse_version)("grok 0.7.3"),
        Some("0.7.3".to_owned())
    );
}

#[test]
fn cursor_version_parser_accepts_and_rejects() {
    assert_eq!(
        parse_cursor_version("agent 2026.9.6-stable.1 (abc)"),
        Some("2026.9.6-stable.1".to_owned())
    );
    assert_eq!(
        parse_cursor_version("2025.12.31-nightly"),
        Some("2025.12.31-nightly".to_owned())
    );
    for bad in [
        "1.2.3",
        "2026.9.6",
        "26.9.6-x",
        "2026.9-x",
        "no version here",
    ] {
        assert_eq!(parse_cursor_version(bad), None, "cursor rejects {bad:?}");
    }
    assert_eq!(
        (CURSOR_ACP.parse_version)("agent 2026.9.6-stable.1"),
        Some("2026.9.6-stable.1".to_owned())
    );
}

#[test]
fn auth_classifiers_per_row() {
    assert_eq!(
        grok_select_auth_method(&["xai.api_key", "cached_token"], true),
        Some("xai.api_key")
    );
    assert_eq!(
        grok_select_auth_method(&["xai.api_key", "cached_token"], false),
        Some("cached_token")
    );
    assert_eq!(
        grok_select_auth_method(&["cached_token"], false),
        Some("cached_token")
    );
    assert_eq!(grok_select_auth_method(&[], true), None);
    assert_eq!(grok_select_auth_method(&["other"], false), None);

    assert_eq!(
        cursor_select_auth_method(&["cursor_login"], false),
        Some("cursor_login")
    );
    assert_eq!(
        cursor_select_auth_method(&["cursor_login"], true),
        Some("cursor_login")
    );
    assert_eq!(cursor_select_auth_method(&[], false), None);
    assert_eq!(cursor_select_auth_method(&["cached_token"], true), None);

    assert!((GROK_ACP.is_authenticated_output)("models:\n  foo"));
    assert!((CURSOR_ACP.is_authenticated_output)("signed in as s"));
    assert!(!default_is_authenticated_output(
        "Error: not authenticated, run login"
    ));
    assert!(!default_is_authenticated_output("Not logged in"));
}

#[test]
fn grok_args_mirror_ts_matrix() {
    assert_eq!(
        grok_build_args(&launch_args()),
        vec![
            OsString::from("--no-auto-update"),
            OsString::from("agent"),
            OsString::from("stdio"),
        ]
    );
    let full = LaunchArgs {
        model: Some("grok-4".to_owned()),
        reasoning_effort: Some("high".to_owned()),
        speed_fast: false,
        permission: None,
        write_access: true,
    };
    assert_eq!(
        grok_build_args(&full),
        vec![
            OsString::from("--no-auto-update"),
            OsString::from("--model"),
            OsString::from("grok-4"),
            OsString::from("--reasoning-effort"),
            OsString::from("high"),
            OsString::from("agent"),
            OsString::from("stdio"),
        ]
    );
    let plan = LaunchArgs {
        write_access: false,
        ..launch_args()
    };
    assert_eq!(
        grok_build_args(&plan),
        vec![
            OsString::from("--no-auto-update"),
            OsString::from("--permission-mode"),
            OsString::from("plan"),
            OsString::from("agent"),
            OsString::from("stdio"),
        ]
    );
    let auto = LaunchArgs {
        permission: Some("auto".to_owned()),
        ..launch_args()
    };
    assert!(grok_build_args(&auto).contains(&OsString::from("auto")));
    let approve = LaunchArgs {
        permission: Some("always-approve".to_owned()),
        ..launch_args()
    };
    assert!(grok_build_args(&approve).contains(&OsString::from("--always-approve")));
    assert_eq!((GROK_ACP.build_args)(&launch_args()).len(), 3);
}

#[test]
fn cursor_args_mirror_ts_matrix() {
    assert_eq!(
        cursor_build_args(&launch_args()),
        vec![OsString::from("acp")]
    );
    let effort = LaunchArgs {
        model: Some("composer-1".to_owned()),
        reasoning_effort: Some("high".to_owned()),
        ..launch_args()
    };
    let built = cursor_build_args(&effort);
    assert_eq!(built[0], OsString::from("--model"));
    assert_eq!(built[1], OsString::from("composer-1-high"));
    assert_eq!(built[2], OsString::from("acp"));

    let suffixed = LaunchArgs {
        model: Some("composer-1-high".to_owned()),
        reasoning_effort: Some("low".to_owned()),
        ..launch_args()
    };
    assert_eq!(
        cursor_build_args(&suffixed)[1],
        OsString::from("composer-1-high")
    );

    let fast = LaunchArgs {
        model: Some("composer-1".to_owned()),
        reasoning_effort: Some("high".to_owned()),
        speed_fast: true,
        ..launch_args()
    };
    assert_eq!(
        cursor_build_args(&fast)[1],
        OsString::from("composer-1-high-fast")
    );

    let bracket = LaunchArgs {
        model: Some("cursor[fast]".to_owned()),
        reasoning_effort: Some("high".to_owned()),
        ..launch_args()
    };
    assert_eq!(
        cursor_build_args(&bracket)[1],
        OsString::from("cursor[fast]")
    );

    let ask = LaunchArgs {
        write_access: false,
        ..launch_args()
    };
    assert_eq!(
        cursor_build_args(&ask),
        vec![
            OsString::from("--mode"),
            OsString::from("ask"),
            OsString::from("acp"),
        ]
    );
    let force = LaunchArgs {
        permission: Some("force".to_owned()),
        ..launch_args()
    };
    assert!(cursor_build_args(&force).contains(&OsString::from("--force")));
}

#[test]
fn engine_rows_carry_plain_data() {
    assert_eq!(GROK_ACP.engine_id, "grok");
    assert_eq!(GROK_ACP.executable, "grok");
    assert_eq!(GROK_ACP.version_args, &["--version"][..]);
    assert_eq!(
        GROK_ACP.auth_probe_args,
        &["--no-auto-update", "models"][..]
    );
    assert_eq!(GROK_ACP.image_mode, ImageMode::Embedded);

    assert_eq!(CURSOR_ACP.engine_id, "cursor");
    assert!(CURSOR_ACP.executable.contains("agent"));
    assert_eq!(CURSOR_ACP.version_args, &["--version"][..]);
    assert_eq!(CURSOR_ACP.auth_probe_args, &["status"][..]);
    assert_eq!(CURSOR_ACP.image_mode, ImageMode::Image);
    if cfg!(windows) {
        assert_eq!(CURSOR_ACP.executable, "agent.cmd");
    } else {
        assert_eq!(CURSOR_ACP.executable, "agent");
    }
}

#[test]
fn prompt_content_image_modes() {
    let image = PromptPart::Image(ImageBlock {
        id: "a/b".to_owned(),
        name: "x y.png".to_owned(),
        media_type: "image/png".to_owned(),
        bytes: vec![1, 2, 3],
    });
    let embedded = build_prompt_content(
        ImageMode::Embedded,
        "hi",
        std::slice::from_ref(&image),
        Some("rules"),
    )
    .expect("embedded content");
    assert_eq!(embedded.len(), 2);
    assert_eq!(
        embedded[0],
        serde_json::json!({
            "type": "text",
            "text": "<artisan-product-instructions>\nrules\n</artisan-product-instructions>",
        })
    );
    assert_eq!(
        embedded[1],
        serde_json::json!({
            "type": "resource",
            "resource": {
                "blob": "AQID",
                "mimeType": "image/png",
                "uri": "artisan://attachment/a%2Fb/x%20y.png",
            },
        })
    );

    let native =
        build_prompt_content(ImageMode::Image, "hi", &[image], None).expect("image content");
    assert_eq!(native.len(), 1);
    assert_eq!(
        native[0],
        serde_json::json!({ "type": "image", "data": "AQID", "mimeType": "image/png" })
    );

    let plain = build_prompt_content(ImageMode::Image, "hi", &[], None).expect("text");
    assert_eq!(
        plain,
        vec![serde_json::json!({ "type": "text", "text": "hi" })]
    );

    let bad = PromptPart::Image(ImageBlock {
        id: String::new(),
        name: "n".to_owned(),
        media_type: "image/png".to_owned(),
        bytes: vec![1],
    });
    assert_eq!(
        build_prompt_content(ImageMode::Image, "hi", &[bad], None).expect_err("metadata"),
        AcpError::InvalidContent
    );
}

#[test]
fn generated_title_metadata_is_scoped_and_optional() {
    let session = SessionId::parse("root", 128).unwrap();
    let params = serde_json::json!({"sessionId":"root","update":{"sessionUpdate":"session_info_update","title":"List project files"}});
    let update = parse_session_update(&params, &session, 128)
        .unwrap()
        .unwrap();
    assert_eq!(
        update.update.summary_title().unwrap().as_str(),
        "List project files"
    );
    let foreign = serde_json::json!({"sessionId":"child","update":{"sessionUpdate":"session_info_update","title":"Wrong thread"}});
    assert!(
        parse_session_update(&foreign, &session, 128)
            .unwrap()
            .is_none()
    );
    let other = serde_json::json!({"sessionId":"root","update":{"sessionUpdate":"tool_call","title":"Not a session title"}});
    assert!(
        parse_session_update(&other, &session, 128)
            .unwrap()
            .unwrap()
            .update
            .summary_title()
            .is_none()
    );
}
