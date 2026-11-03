use super::decoder::Decoder;
use crate::EngineEvent;
use artisan_api::{ItemKind, Status};
use serde_json::{Value, json};

fn root() -> Decoder {
    let mut decoder = Decoder::default();
    let id = decoder.request("thread/start", json!({}));
    decoder
        .feed(&json!({"id": id, "result": {"thread": {"id": "root"}}}).to_string())
        .unwrap();
    decoder
}

fn activity(decoder: &mut Decoder, kind: &str, id: &str) -> Vec<EngineEvent> {
    feed(
        decoder,
        &json!({"method": "item/completed", "params": {"threadId": "root", "item": {
            "type": "subAgentActivity", "id": id, "agentThreadId": "child", "agentPath": "/root/worker", "kind": kind, "model": "worker-model"
        }}}),
    )
}

fn feed(decoder: &mut Decoder, value: &Value) -> Vec<EngineEvent> {
    decoder.feed(&value.to_string()).unwrap()
}

#[test]
fn activity_tracks_one_worker_across_completion_and_followup() {
    let mut decoder = root();
    let started = activity(&mut decoder, "started", "spawn");
    assert!(
        matches!(&started[..], [EngineEvent::Item { key, parent: None, status: Status::Running, kind: ItemKind::Agent { agent_id: Some(agent), model: Some(model), .. } }] if key == "child" && agent == "child" && model == "worker-model")
    );
    let reply = feed(
        &mut decoder,
        &json!({"method": "item/completed", "params": {"threadId": "child", "item": {"type": "agentMessage", "id": "reply", "text": "report", "phase": "final_answer"}}}),
    );
    assert!(reply.iter().any(|event| matches!(event, EngineEvent::Item { parent: Some(parent), kind: ItemKind::AssistantMessage { .. }, .. } if parent == "child")));
    let done = activity(&mut decoder, "completed", "done");
    assert!(done.iter().any(|event| matches!(event, EngineEvent::Item { key, status: Status::Done, kind: ItemKind::Agent { result: Some(_), .. }, .. } if key == "child")));
    let resumed = activity(&mut decoder, "interacted", "followup");
    assert!(
        matches!(&resumed[..], [EngineEvent::Item { key, status: Status::Running, kind: ItemKind::Agent { result: None, .. }, .. }] if key == "child")
    );
    let stopped = activity(&mut decoder, "interrupted", "stop");
    assert!(
        matches!(&stopped[..], [EngineEvent::Item { key, status: Status::Cancelled, .. }] if key == "child")
    );
}

#[test]
fn activity_recovers_a_worker_in_a_later_root_run() {
    let mut decoder = root();
    let resumed = activity(&mut decoder, "interacted", "followup");
    assert!(
        matches!(&resumed[..], [EngineEvent::Item { key, status: Status::Running, .. }] if key == "child")
    );
    let done = activity(&mut decoder, "completed", "done");
    assert!(
        matches!(&done[..], [EngineEvent::Item { key, status: Status::Done, .. }] if key == "child")
    );
}

#[test]
fn activity_reuses_a_worker_already_discovered_from_thread_started() {
    let mut decoder = root();
    feed(
        &mut decoder,
        &json!({"method": "thread/started", "params": {"thread": {"id": "child", "parentThreadId": "root", "agentNickname": "Helper"}}}),
    );
    let started = activity(&mut decoder, "started", "spawn");
    assert!(matches!(&started[..], [EngineEvent::Item { key, .. }] if key == "child"));
}
