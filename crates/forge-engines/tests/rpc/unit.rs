use super::*;
#[test]
fn replies_correlate_out_of_order_and_unknown_ids_do_not_consume_requests() {
    let mut state = State::default();
    let first = state.request("first", json!({}));
    let second = state.request("second", json!({}));
    assert!(state.respond(&json!(999), Some(json!({})), None).is_none());
    assert_eq!(
        state
            .respond(&json!(second), Some(json!({ "ok": true })), None)
            .unwrap()
            .method,
        "second"
    );
    let error = json!({ "message": "rejected", "data": { "message": "details" } });
    let refused = state
        .respond(&json!(first), None, Some(&error))
        .unwrap()
        .result
        .unwrap_err();
    assert_eq!(refused.said(), Some("rejected: details"));
    assert!(state.requests.is_empty());
}

#[test]
fn an_engine_refusal_logs_its_code_but_never_what_the_engine_wrote() {
    let mut state = State::default();
    let id = state.request("turn/start", json!({}));
    let reflected = "sk-ant-api03-SENTINEL and the user's prompt";
    let error =
        json!({ "code": -32000, "message": "bad request", "data": { "message": reflected } });

    let refused = state
        .respond(&json!(id), None, Some(&error))
        .unwrap()
        .result
        .unwrap_err();

    let logged = format!("{refused} {refused:?}");
    assert!(!logged.contains("SENTINEL"), "{logged}");
    assert!(logged.contains("-32000"), "{logged}");
    assert!(
        told(&refused).contains(reflected),
        "the run's failure no longer tells the user why Codex refused"
    );
}

#[test]
fn a_refusal_without_a_message_still_reads_as_a_refusal_not_a_missing_result() {
    let mut state = State::default();
    let refused_id = state.request("thread/start", json!({}));
    let empty_id = state.request("thread/read", json!({}));

    let refused = state
        .respond(&json!(refused_id), None, Some(&json!({ "code": -32001 })))
        .unwrap()
        .result
        .unwrap_err();
    let empty = state
        .respond(&json!(empty_id), None, None)
        .unwrap()
        .result
        .unwrap_err();

    assert_eq!(refused.said(), None);
    assert_eq!(refused.to_string(), "the engine refused with code -32001");
    assert_eq!(empty.to_string(), "the engine answered without a result");
}
