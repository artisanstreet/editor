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
    let message = activity(&mut decoder, "interacted", "followup");
    assert!(matches!(
        &message[..],
        [EngineEvent::Item {
            kind: ItemKind::Tool {
                tool: artisan_api::ToolKind::AgentMessage { .. },
                ..
            },
            ..
        }]
    ));
    let resumed = feed(
        &mut decoder,
        &json!({"method": "turn/started", "params": {"threadId": "child", "turn": {"id": "followup-turn"}}}),
    );
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
    assert!(resumed.iter().any(|event| matches!(event, EngineEvent::Item { key, status: Status::Running, kind: ItemKind::Agent { .. }, .. } if key == "child")));
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

#[test]
fn native_paths_remain_descriptions_while_tasks_keep_the_actual_prompt() {
    let mut decoder = root();
    let started = activity(&mut decoder, "started", "spawn");
    assert!(
        matches!(&started[..], [EngineEvent::Item { kind: ItemKind::Agent { description, prompt: None, .. }, .. }] if description.inline() == Some("/root/worker"))
    );
    let prompt = "Check the vegetation transitions.\nKeep the triangle budget unchanged.";
    let events = feed(
        &mut decoder,
        &json!({"method": "item/completed", "params": {"threadId": "child", "item": {"type": "userMessage", "id": "task", "content": [{"type": "text", "text": prompt}]}}}),
    );
    assert!(
        matches!(&events[..], [EngineEvent::Item { kind: ItemKind::Agent { description, prompt: Some(task), .. }, .. }] if description.inline() == Some("/root/worker") && task.inline() == Some(prompt))
    );
}

#[test]
fn messages_to_parent_and_peers_do_not_create_workers() {
    let mut decoder = root();
    activity(&mut decoder, "started", "spawn");
    for recipient in ["root", "peer"] {
        let events = feed(
            &mut decoder,
            &json!({"method": "item/completed", "params": {"threadId": "child", "item": {"type": "subAgentActivity", "id": format!("message-{recipient}"), "kind": "interacted", "agentThreadId": recipient, "agentPath": format!("/root/{recipient}")}}}),
        );
        assert!(
            matches!(&events[..], [EngineEvent::Item { parent: Some(parent), kind: ItemKind::Tool { tool: artisan_api::ToolKind::AgentMessage { recipient: to, text, summary: Some(_), .. }, .. }, .. }] if parent == "child" && to == recipient && text.byte_len() == 0)
        );
    }
    let root_end = feed(
        &mut decoder,
        &json!({"method": "turn/completed", "params": {"threadId": "root", "turn": {"status": "completed"}}}),
    );
    assert!(root_end.is_empty());
    assert!(decoder.take_turn_end().is_some());
}

#[test]
fn legacy_collaboration_messages_keep_the_actual_message_body() {
    for tool in ["sendMessage", "followupTask", "sendInput"] {
        let mut decoder = root();
        let events = feed(
            &mut decoder,
            &json!({"method": "item/completed", "params": {"threadId": "root", "item": {"type": "collabAgentToolCall", "id": "message", "tool": tool, "status": "completed", "receiverThreadIds": ["child"], "prompt": "Use the existing terrain material."}}}),
        );
        assert!(
            matches!(&events[..], [EngineEvent::Item { kind: ItemKind::Tool { tool: artisan_api::ToolKind::AgentMessage { text, .. }, .. }, .. }] if text.inline() == Some("Use the existing terrain material."))
        );
    }
}
