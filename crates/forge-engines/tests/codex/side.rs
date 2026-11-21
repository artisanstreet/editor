use super::*;

fn ended(error: &Value) -> SideQueryError {
    let params = json!({
        "threadId": "fork",
        "turn": { "id": "turn", "items": [], "status": "failed", "error": error },
    });
    let Ok(Notification::TurnCompleted(notice)) = Notification::parse("turn/completed", &params)
    else {
        panic!("a failed turn/completed parses");
    };
    failed(notice.thread_id, notice.turn.error)
}

#[test]
fn only_an_exceeded_context_window_reports_a_context_limit() {
    let overflow = ended(&json!({
        "message": "Codex ran out of room in the model's context window. Start a new thread or clear earlier history before retrying.",
        "codexErrorInfo": "contextWindowExceeded",
        "additionalDetails": null,
    }));
    assert!(overflow.exceeds_context(), "{overflow}");
    for error in [
        json!({
            "message": "unexpected status 400 Bad Request: Unsupported parameter: 'context_management' is not supported with this model.",
            "codexErrorInfo": "badRequest",
        }),
        json!({
            "message": "stream disconnected before completion: response too large",
            "codexErrorInfo": { "responseStreamDisconnected": { "httpStatusCode": null } },
        }),
        json!({ "message": "the context window overflowed?", "codexErrorInfo": null }),
    ] {
        let failure = ended(&error);
        assert!(!failure.exceeds_context(), "{failure}");
        assert!(
            matches!(failure, SideQueryError::Failed { .. }),
            "{failure}"
        );
    }
}
