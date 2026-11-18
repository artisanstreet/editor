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
    assert_eq!(
        state
            .respond(&json!(first), None, Some(&error))
            .unwrap()
            .result,
        Err(RpcError {
            code: None,
            message: "rejected: details".into()
        })
    );
    assert!(state.requests.is_empty());
}
