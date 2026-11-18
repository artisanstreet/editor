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
