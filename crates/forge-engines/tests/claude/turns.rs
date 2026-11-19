use super::*;

fn feed(decoder: &mut ClaudeDecoder, frame: &Value) -> Result<Vec<EngineEvent>, DecodeError> {
    decoder.feed(&frame.to_string())
}

fn refused_turn(decoder: &mut ClaudeDecoder) {
    feed(
        decoder,
        &json!({ "type": "stream_event", "event": { "type": "message_start", "message": { "id": "m1" } } }),
    )
    .unwrap();
    feed(
        decoder,
        &json!({ "type": "stream_event", "event": { "type": "message_delta", "delta": { "stop_reason": "refusal" }, "usage": {} } }),
    )
    .unwrap();
}

fn result(decoder: &mut ClaudeDecoder) -> Outcome {
    feed(
        decoder,
        &json!({ "type": "result", "subtype": "success", "result": "ok", "usage": {} }),
    )
    .unwrap();
    decoder.take_outcome().expect("a result ends the turn")
}

#[test]
fn a_refusal_ends_only_its_own_turn() {
    let mut decoder = ClaudeDecoder::default();
    refused_turn(&mut decoder);
    assert_eq!(result(&mut decoder), Outcome::Refused);
    feed(
        &mut decoder,
        &json!({ "type": "user", "isReplay": true, "message": { "content": "next" } }),
    )
    .unwrap();
    assert_eq!(result(&mut decoder), Outcome::Done);
}

#[test]
fn a_malformed_background_snapshot_keeps_the_background_work() {
    let mut decoder = ClaudeDecoder::default();
    feed(
        &mut decoder,
        &json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": [{ "task_id": "a1", "parent_task_id": "agent-1" }] }),
    )
    .unwrap();
    assert!(decoder.background_running());
    for malformed in [
        json!({ "type": "system", "subtype": "background_tasks_changed" }),
        json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": "none" }),
        json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": [{ "ambient": "yes" }] }),
    ] {
        assert!(matches!(
            feed(&mut decoder, &malformed),
            Err(DecodeError::Background(_))
        ));
        assert!(decoder.background_running(), "{malformed}");
    }
    feed(
        &mut decoder,
        &json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": [] }),
    )
    .unwrap();
    assert!(!decoder.background_running());
}

fn permission_request(id: &str, tool: &str, input: &Value) -> Value {
    json!({ "type": "control_request", "request_id": id, "request": {
        "subtype": "can_use_tool", "tool_name": tool, "tool_use_id": format!("toolu-{id}"), "input": input,
    } })
}

#[test]
fn a_malformed_permission_request_is_refused_and_shown() {
    let mut decoder = ClaudeDecoder::default();
    let events = feed(
        &mut decoder,
        &permission_request("r1", "Bash", &json!({ "description": "no command" })),
    )
    .unwrap();
    assert!(
        events.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                kind: ItemKind::Error { .. },
                ..
            }
        )),
        "{events:?}"
    );
    let replies = decoder.take_replies();
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(replies[0]["response"]["request_id"], "r1");
    assert_eq!(replies[0]["response"]["subtype"], "error");
}

#[test]
fn starting_an_agent_waits_on_an_approval_the_user_can_answer() {
    let mut decoder = ClaudeDecoder::default();
    let events = feed(
        &mut decoder,
        &permission_request(
            "r2",
            "Agent",
            &json!({ "description": "Audit the parser", "prompt": "Audit it", "subagent_type": "general-purpose" }),
        ),
    )
    .unwrap();
    let Some(EngineEvent::Item {
        key,
        status: Status::AwaitingUser,
        kind: ItemKind::Tool {
            approval: Some(_), ..
        },
        ..
    }) = events.last()
    else {
        panic!("an approval the user can answer: {events:?}");
    };
    let key = key.clone();
    let answered = decoder.answer(
        &key,
        &Answer::Approval {
            decision: Decision::AllowOnce,
        },
    );
    assert!(
        answered.iter().any(|event| matches!(
            event,
            EngineEvent::Item { key: answered, status: Status::Done, .. } if *answered == key
        )),
        "{answered:?}"
    );
    let replies = decoder.take_replies();
    assert_eq!(replies[0]["response"]["request_id"], "r2");
    assert_eq!(replies[0]["response"]["response"]["behavior"], "allow");
}

#[test]
fn peer_messages_without_ids_keep_separate_rows() {
    let mut decoder = ClaudeDecoder::default();
    let mut keys = Vec::new();
    for body in ["first note", "second note"] {
        let events = feed(
            &mut decoder,
            &json!({ "type": "user", "isReplay": true, "message": { "content": body }, "origin": { "kind": "peer", "name": "reviewer", "body": body } }),
        )
        .unwrap();
        keys.extend(events.into_iter().filter_map(|event| {
            if let EngineEvent::Item {
                key,
                kind: ItemKind::Tool { .. },
                ..
            } = event
            {
                Some(key)
            } else {
                None
            }
        }));
    }
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert_ne!(keys[0], keys[1]);
}

fn shell_call(n: usize) -> Value {
    json!({ "type": "assistant", "message": { "id": format!("m{n}"), "content": [
        { "type": "tool_use", "id": format!("t{n}"), "name": "Bash", "input": { "command": "ls" } }
    ] } })
}

#[test]
fn a_long_run_keeps_only_a_bounded_record_of_settled_tools() {
    let mut decoder = ClaudeDecoder::default();
    for n in 0..10_000 {
        feed(&mut decoder, &shell_call(n)).unwrap();
        feed(
            &mut decoder,
            &json!({ "type": "user", "message": { "content": [
                { "type": "tool_result", "tool_use_id": format!("t{n}"), "content": "ok" }
            ] } }),
        )
        .unwrap();
    }
    assert!(
        decoder.tools.len() < 16,
        "{} tools kept",
        decoder.tools.len()
    );
    let replayed = feed(&mut decoder, &shell_call(9_999)).unwrap();
    assert!(
        !replayed
            .iter()
            .any(|event| matches!(event, EngineEvent::Item { key, .. } if key == "t9999")),
        "a replayed call to a settled tool must not reopen it: {replayed:?}"
    );
}
