use super::*;

#[test]
fn only_the_forks_compaction_is_accepted_and_usage_includes_cache() {
    let data = json!({ "type": "session.compaction.ended", "data": {
        "sessionID": "fork", "text": "## Objective\n- Continue",
        "tokens": { "input": 10, "output": 20, "reasoning": 5, "cache": { "read": 100, "write": 30 } },
    }}).to_string();
    assert!(compact_event(&data, "source").unwrap().is_none());
    let result = compact_event(&data, "fork").unwrap().unwrap();
    assert_eq!(
        result.usage,
        SideUsage {
            input: 140,
            cached_input: 100,
            output: 25
        }
    );
    assert!(crate::is_compaction_summary(&result.text));
    compact_event(&json!({ "type": "session.compaction.failed", "data": { "sessionID": "fork", "error": "overflow" }}).to_string(), "fork").expect_err("a failed compaction is reported");
}

fn ended_with(error: &Value) -> Option<TurnFailure> {
    let mut decoder = Decoder::new(Some("fork".to_owned()));
    decoder
        .feed(
            &json!({
                "id": "evt_1", "created": 1, "type": "session.execution.failed",
                "data": { "sessionID": "fork", "error": error },
            })
            .to_string(),
        )
        .unwrap();
    decoder.error
}

#[test]
fn only_a_provider_context_rejection_reports_a_context_limit() {
    let overflows = [
        json!({
            "type": "provider.invalid-request", "status": 400,
            "message": "prompt is too long: 213519 tokens > 200000 maximum",
            "response": { "body": json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "prompt is too long: 213519 tokens > 200000 maximum" } }).to_string() },
        }),
        json!({
            "type": "provider.invalid-request", "status": 400,
            "message": "Your input exceeds the context window of this model. Please adjust your input and try again.",
            "response": { "body": json!({ "error": { "message": "Your input exceeds the context window of this model. Please adjust your input and try again.", "type": "invalid_request_error", "param": "input", "code": "context_length_exceeded" } }).to_string() },
        }),
        json!({
            "type": "provider.invalid-request", "status": 413,
            "message": "Request Entity Too Large",
            "response": { "body": "<html><body>413 Request Entity Too Large</body></html>" },
        }),
    ];
    for error in &overflows {
        assert!(
            matches!(ended_with(error), Some(TurnFailure::ContextLimit(_))),
            "{error}"
        );
        let event = json!({ "type": "session.compaction.failed", "data": { "sessionID": "fork", "error": error }}).to_string();
        assert!(compact_event(&event, "fork").unwrap_err().exceeds_context());
    }
    let unrelated = [
        json!({
            "type": "provider.invalid-request", "status": 400,
            "message": "context_management: Extra inputs are not permitted",
            "response": { "body": json!({ "type": "error", "error": { "type": "invalid_request_error", "message": "context_management: Extra inputs are not permitted" } }).to_string() },
        }),
        json!({ "type": "provider.rate-limit", "status": 429, "message": "Too many tokens, please wait before trying again." }),
    ];
    for error in &unrelated {
        assert!(
            matches!(ended_with(error), Some(TurnFailure::Failed(_))),
            "{error}"
        );
        let event = json!({ "type": "session.compaction.failed", "data": { "sessionID": "fork", "error": error }}).to_string();
        assert!(!compact_event(&event, "fork").unwrap_err().exceeds_context());
    }
}
