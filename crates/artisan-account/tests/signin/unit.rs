use super::*;

#[test]
fn status_and_finalize_reads_tolerate_future_fields() {
    let raw = serde_json::json!({
        "request_id": "logreq_1",
        "status": "pending",
        "totp_required": false,
        "email": "a@example.invalid",
        "expires_at": "2030-01-01T00:00:00Z",
        "future": [1],
    });
    let status: MagicStatusResponse = serde_json::from_value(raw).unwrap();
    assert_eq!(status.status, "pending");
    let raw = serde_json::json!({
        "session_id": "sess_1",
        "account_id": "acct_1",
        "expires_at": "t",
        "totp_verified": false,
        "future": 1,
    });
    let finalize: MagicFinalizeResponse = serde_json::from_value(raw).unwrap();
    assert_eq!(finalize.session_id, "sess_1");
}

#[test]
fn secret_is_required_to_finalize() {
    let raw = serde_json::json!({"requestId": "logreq_1"});
    assert!(serde_json::from_value::<MagicFinalizeRequest>(raw).is_err());
}
