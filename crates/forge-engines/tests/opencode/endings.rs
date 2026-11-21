use super::decoder::Decoder;
use crate::EngineEvent;
use artisan_api::{ItemKind, Status};
use serde_json::{Value, json};

const ROOT: &str = "ses_root";

fn feed(decoder: &mut Decoder, id: u32, kind: &str, data: &Value) -> Vec<EngineEvent> {
    decoder
        .feed(&json!({ "id": format!("evt_{id}"), "type": kind, "data": data }).to_string())
        .unwrap()
}

fn status_of(events: &[EngineEvent], wanted: &str) -> Option<Status> {
    events.iter().rev().find_map(|event| match event {
        EngineEvent::Item { key, status, .. } | EngineEvent::Status { key, status }
            if key == wanted =>
        {
            Some(*status)
        }
        EngineEvent::Item { .. }
        | EngineEvent::Status { .. }
        | EngineEvent::Session { .. }
        | EngineEvent::SessionGone
        | EngineEvent::Model { .. }
        | EngineEvent::Title { .. }
        | EngineEvent::Image { .. }
        | EngineEvent::Live { .. }
        | EngineEvent::Usage(_)
        | EngineEvent::Commands(_)
        | EngineEvent::InputAccepted { .. } => None,
    })
}

#[test]
fn text_finished_before_its_step_failed_is_kept() {
    let mut decoder = Decoder::new(Some(ROOT.to_owned()));
    let text = json!({ "sessionID": ROOT, "assistantMessageID": "msg_1", "ordinal": 0 });
    feed(
        &mut decoder,
        0,
        "session.step.started",
        &json!({ "sessionID": ROOT }),
    );
    feed(&mut decoder, 1, "session.text.started", &text);
    let mut ended = text.clone();
    ended["text"] = json!("Half of the answer");
    feed(&mut decoder, 2, "session.text.ended", &ended);
    let events = feed(
        &mut decoder,
        3,
        "session.step.failed",
        &json!({ "sessionID": ROOT, "error": { "type": "api", "message": "overloaded" } }),
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            EngineEvent::Item { key, status: Status::Done, kind: ItemKind::AssistantMessage { text, .. }, .. }
                if key == "msg_1/0" && text.inline() == Some("Half of the answer")
        )),
        "{events:?}"
    );
}

#[test]
fn an_unclaimed_child_agent_finishes_with_its_session() {
    let mut decoder = Decoder::new(Some(ROOT.to_owned()));
    let child = json!({ "sessionID": "ses_child" });
    feed(
        &mut decoder,
        0,
        "session.created",
        &json!({ "sessionID": "ses_child", "parentID": ROOT, "title": "Audit", "agent": "general" }),
    );
    let started = feed(&mut decoder, 1, "session.step.started", &child);
    assert_eq!(
        status_of(&started, "agent:ses_child"),
        Some(Status::Running)
    );
    let finished = feed(&mut decoder, 2, "session.execution.succeeded", &child);
    assert_eq!(status_of(&finished, "agent:ses_child"), Some(Status::Done));
}

#[test]
fn a_shell_exit_without_a_code_waits_for_its_result() {
    let mut decoder = Decoder::new(Some(ROOT.to_owned()));
    let call = "call_1";
    feed(
        &mut decoder,
        0,
        "session.tool.input.started",
        &json!({ "sessionID": ROOT, "id": call, "name": "shell" }),
    );
    feed(
        &mut decoder,
        1,
        "session.tool.called",
        &json!({ "sessionID": ROOT, "id": call, "input": { "command": "make", "background": true } }),
    );
    feed(
        &mut decoder,
        2,
        "session.tool.success",
        &json!({ "sessionID": ROOT, "id": call,
            "metadata": { "status": "running", "shellID": "sh_1", "truncated": false },
            "content": [{ "type": "text", "text": "Command moved to the background (shell ID: sh_1)" }] }),
    );
    let exited = feed(
        &mut decoder,
        3,
        "shell.exited",
        &json!({ "id": "sh_1", "status": "exited" }),
    );
    assert_eq!(status_of(&exited, call), None, "{exited:?}");
    let reported = feed(
        &mut decoder,
        4,
        "session.inbox.enqueued",
        &json!({ "sessionID": ROOT, "inboxID": "in_1", "item": { "type": "synthetic", "payload": {
            "text": "make: *** [all] Error 2",
            "metadata": { "source": "shell", "shellID": "sh_1", "state": "completed", "exit": 2 },
        } } }),
    );
    assert_eq!(status_of(&reported, call), Some(Status::Failed));
}
