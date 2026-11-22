use super::*;

async fn streamed(events: Vec<Value>) -> (Outcome, Arc<Script>, tempfile::TempDir) {
    let work = tempfile::tempdir().expect("a dir");
    let mut all = vec![start("m1")];
    all.extend(events);
    let backend = script(vec![Reply::Waits(all), answer("m2", "Done.")]);
    let (outcome, _, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        nobody(),
    )
    .await;
    (outcome, backend, work)
}

fn write_call(index: Value, id: Option<&str>, partial_json: &str) -> Vec<Value> {
    let mut block =
        json!({ "type": "tool_use", "name": "Write", "input": {}, "tool_origin": null });
    if let Some(id) = id {
        block["id"] = id.into();
    }
    let mut start = json!({ "type": "content_block_start", "content_block": block });
    let mut delta = json!({ "type": "content_block_delta", "delta": { "type": "input_json_delta", "partial_json": partial_json } });
    let mut stop = json!({ "type": "content_block_stop" });
    if !index.is_null() {
        start["index"] = index.clone();
        delta["index"] = index.clone();
        stop["index"] = index;
    }
    vec![start, delta, stop]
}

fn assert_protocol_failure(outcome: &Outcome, backend: &Script, work: &Path) {
    assert_eq!(outcome.status(), RunStatus::Failed, "{outcome:?}");
    assert!(
        outcome
            .error()
            .is_some_and(|error| error.contains("could not read")),
        "{outcome:?}"
    );
    assert!(!work.join("ran").exists(), "the malformed call never runs");
    assert_eq!(
        backend.sent.lock().expect("lock").len(),
        1,
        "no results are sent back for a malformed call"
    );
}

#[tokio::test]
async fn a_tool_call_without_an_id_fails_the_turn_instead_of_running() {
    let (outcome, backend, work) = streamed(write_call(
        json!(0),
        None,
        r#"{"file_path":"ran","content":"ran"}"#,
    ))
    .await;
    assert_protocol_failure(&outcome, &backend, work.path());
}

#[tokio::test]
async fn tool_input_that_is_not_json_fails_the_turn_instead_of_running_with_null() {
    let (outcome, backend, work) = streamed(write_call(
        json!(0),
        Some("toolu_1"),
        r#"{"file_path":"ran","content":"#,
    ))
    .await;
    assert_protocol_failure(&outcome, &backend, work.path());
}

#[tokio::test]
async fn a_block_without_an_index_does_not_overwrite_block_zero() {
    let mut events = vec![
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text" } }),
        json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Let me write." } }),
    ];
    events.extend(write_call(
        Value::Null,
        Some("toolu_1"),
        r#"{"file_path":"ran","content":"ran"}"#,
    ));
    let (outcome, backend, work) = streamed(events).await;
    assert_protocol_failure(&outcome, &backend, work.path());
}

#[tokio::test]
async fn a_block_opened_twice_at_one_index_fails_the_turn() {
    let mut events = vec![
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text" } }),
    ];
    events.extend(write_call(
        json!(0),
        Some("toolu_1"),
        r#"{"file_path":"ran","content":"ran"}"#,
    ));
    let (outcome, backend, work) = streamed(events).await;
    assert_protocol_failure(&outcome, &backend, work.path());
}
