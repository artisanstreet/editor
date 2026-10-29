use super::decoder::{Decoder, Goal};
use crate::{EngineEvent, Input, RunIo, UserInput};
use artisan_api::{ItemKind, NoticeKind};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;

fn goal(status: &str, updated_at: u64) -> Value {
    json!({ "id": "goal-id", "objective": "Ship the parser", "status": status, "updatedAt": updated_at })
}

fn feed(decoder: &mut Decoder, kind: &str, session: &str, data: Value) -> Vec<EngineEvent> {
    let mut data = data;
    data["sessionID"] = json!(session);
    decoder
        .feed(&json!({ "id": "event", "type": kind, "data": data }).to_string())
        .unwrap()
}

fn changed(decoder: &mut Decoder, session: &str, goal: Value) -> Vec<EngineEvent> {
    feed(
        decoder,
        "rpc.opencode.goal.changed",
        session,
        Value::Object(serde_json::Map::from_iter([("goal".to_owned(), goal)])),
    )
}

fn notice(
    events: &[EngineEvent],
    expected_key: &str,
    expected_parent: Option<&str>,
    expected_state: &str,
) {
    assert!(
        matches!(events, [EngineEvent::Item { key, parent, kind: ItemKind::Notice { kind: NoticeKind::Goal { state }, text }, .. }]
        if key == expected_key && parent.as_deref() == expected_parent && state == expected_state && text.inline() == Some("Ship the parser"))
    );
}

#[test]
fn goals_follow_native_transitions_and_keep_background_work_alive() {
    let mut decoder = Decoder::new(Some("root".to_owned()));
    assert!(changed(&mut decoder, "other", goal("active", 1)).is_empty());
    assert!(changed(&mut decoder, "root", Value::Null).is_empty());
    assert!(decoder.idle());
    notice(
        &changed(&mut decoder, "root", goal("active", 1)),
        "goal",
        None,
        "active",
    );
    feed(&mut decoder, "session.execution.started", "root", json!({}));
    feed(
        &mut decoder,
        "session.execution.succeeded",
        "root",
        json!({}),
    );
    assert!(!decoder.idle());
    notice(
        &changed(&mut decoder, "root", goal("paused", 2)),
        "goal",
        None,
        "paused",
    );
    assert!(decoder.idle());
    changed(&mut decoder, "root", goal("active", 3));
    assert!(!decoder.idle());
    notice(
        &changed(&mut decoder, "root", goal("complete", 4)),
        "goal",
        None,
        "complete",
    );
    assert!(decoder.idle());
    notice(
        &changed(&mut decoder, "root", Value::Null),
        "goal",
        None,
        "cleared",
    );
    assert!(changed(&mut decoder, "root", Value::Null).is_empty());
}

#[test]
fn child_goals_stay_under_their_agent_and_do_not_hold_the_root_open() {
    let mut decoder = Decoder::new(Some("root".to_owned()));
    feed(
        &mut decoder,
        "session.created",
        "child",
        json!({ "parentID": "root" }),
    );
    let events = changed(&mut decoder, "child", goal("active", 1));
    notice(
        &events[1..],
        "goal:agent:child",
        Some("agent:child"),
        "active",
    );
    assert!(decoder.idle());
    assert!(changed(&mut decoder, "root", Value::Null).is_empty());
}

#[test]
fn a_resumed_goal_waits_for_its_change_past_queued_older_events() {
    let mut decoder = Decoder::new(Some("root".to_owned()));
    let resumed = goal("active", 3);
    decoder.goal_changed(
        "root",
        Some(serde_json::from_value::<Goal>(resumed.clone()).unwrap()),
    );
    decoder.await_goal_change(Some(serde_json::from_value(resumed.clone()).unwrap()));
    changed(&mut decoder, "root", goal("paused", 2));
    assert!(!decoder.idle());
    changed(&mut decoder, "root", resumed);
    assert!(!decoder.idle());
    changed(&mut decoder, "root", goal("complete", 4));
    assert!(decoder.idle());
}

#[test]
fn malformed_goals_do_not_replace_valid_state_and_failures_end_the_run() {
    let mut decoder = Decoder::new(Some("root".to_owned()));
    changed(&mut decoder, "root", goal("active", 1));
    for value in [
        json!({}),
        json!({ "objective": "bad", "status": "other" }),
        json!({ "id": "bad", "objective": " ", "status": "paused", "updatedAt": 2 }),
    ] {
        assert!(changed(&mut decoder, "root", value).is_empty());
        assert!(!decoder.idle());
    }
    feed(
        &mut decoder,
        "session.execution.failed",
        "root",
        json!({ "error": { "message": "failed" } }),
    );
    assert!(decoder.idle());
}

#[test]
fn only_a_leading_goal_command_is_intercepted() {
    for (text, expected) in [
        ("/goal ship it", Some("ship it")),
        ("  /goal\nresume ", Some("resume")),
        ("/goal", Some("")),
        ("/goals ship it", None),
        ("explain /goal", None),
    ] {
        assert_eq!(super::goal::argument(text), expected);
    }
}

#[tokio::test]
async fn goal_commands_use_the_native_endpoint_and_clearing_reaches_the_server() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = super::Api::new(
        format!("http://{}", listener.local_addr().unwrap()),
        "secret".to_owned(),
    )
    .unwrap();
    let expected_goal = goal("active", 1);
    let response_goal = expected_goal.clone();
    let server = tokio::spawn(async move {
        let requests = [
            (
                "/api/session/root/command",
                json!({ "name": "goal", "text": "Ship the parser", "files": [], "delivery": "steer" }),
                Value::Null,
            ),
            (
                "/api/rpc/opencode.goal/get",
                json!({ "input": { "sessionID": "root" } }),
                json!({ "output": response_goal }),
            ),
            (
                "/api/session/root/command",
                json!({ "name": "goal", "text": "clear", "files": [], "delivery": "steer" }),
                Value::Null,
            ),
        ];
        for (path, expected, response) in requests {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let (head, body) = loop {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
                    let length: usize = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .unwrap()
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        break (head, bytes[end + 4..].to_vec());
                    }
                }
            };
            assert!(
                head.starts_with(&format!("POST {path} HTTP/1.1\r\n")),
                "{head}"
            );
            assert!(head.contains("authorization: Basic b3BlbmNvZGU6c2VjcmV0"));
            assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), expected);
            let body = response.to_string();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let (_, inbox) = mpsc::channel(8);
    let (events, mut output) = mpsc::channel(8);
    let io = RunIo {
        inbox,
        events,
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    let mut decoder = Decoder::new(Some("root".to_owned()));
    let input = UserInput {
        text: "/goal Ship the parser".to_owned(),
        ..Default::default()
    };
    super::send_input(&api, "root", &input, &mut decoder, &io)
        .await
        .unwrap();
    assert!(
        matches!(output.recv().await.unwrap(), EngineEvent::Item { kind: ItemKind::Notice { kind: NoticeKind::Goal { state }, .. }, .. } if state == "active")
    );
    assert!(
        matches!(output.recv().await.unwrap(), EngineEvent::InputAccepted { id } if id == input.id)
    );
    changed(&mut decoder, "root", expected_goal);
    super::handle_input(&api, "root", &mut decoder, &io, Input::ClearGoal)
        .await
        .unwrap();
    assert!(
        matches!(output.recv().await.unwrap(), EngineEvent::Item { kind: ItemKind::Notice { kind: NoticeKind::Goal { state }, .. }, .. } if state == "cleared")
    );
    assert!(decoder.idle());
    server.await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn an_idle_goal_is_cleared_without_starting_another_prompt() {
    use crate::{Harness as _, Launch, Turn};
    use artisan_api::{EngineConfig, EngineId, ThreadId};
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("opencode");
    std::fs::write(
        &binary,
        r"#!/usr/bin/env python3
import http.server,json,pathlib
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args): pass
    def do_POST(self):
        assert self.path=='/api/session/idle-session/command',self.path
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert body=={'name':'goal','text':'clear','files':[],'delivery':'steer'},body
        pathlib.Path('cleared').write_text('idle-session')
        self.send_response(204)
        self.end_headers()
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
print('opencode server listening on http://127.0.0.1:'+str(server.server_port),flush=True)
server.serve_forever()
",
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let turn = Turn {
        thread_id: ThreadId::new(),
        engine: EngineId::OpenCode,
        binary,
        home: dir.path().join("home"),
        cwd: dir.path().to_owned(),
        config: EngineConfig::new(EngineId::OpenCode),
        session_id: Some("idle-session".to_owned()),
        input: UserInput::default(),
        launch: Launch::default(),
        trace_dir: None,
    };
    super::OpenCodeHarness.clear_goal(turn).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("cleared")).unwrap(),
        "idle-session"
    );
}
