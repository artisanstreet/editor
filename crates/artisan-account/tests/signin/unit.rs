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
    serde_json::from_value::<MagicFinalizeRequest>(raw)
        .expect_err("a finalize request without its secret is refused");
}

#[test]
fn a_logged_redeem_request_never_shows_the_code_or_verifier() {
    let request = DesktopRedeemRequest {
        code: "auth-code-7f3a".to_owned(),
        verifier: "pkce-verifier-9c1d".to_owned(),
    };
    let logged = format!("{request:?}");
    assert!(!logged.contains("auth-code-7f3a"), "{logged}");
    assert!(!logged.contains("pkce-verifier-9c1d"), "{logged}");
}

#[test]
fn logged_sign_in_requests_show_none_of_their_secrets() {
    let logged = format!(
        "{:?} {:?} {:?}",
        MagicRequestResponse {
            request_id: "logreq_1".to_owned(),
            request_secret: "request-secret".to_owned(),
            expires_at: "t".to_owned(),
        },
        MagicStatusRequest {
            request_id: "logreq_1".to_owned(),
            request_secret: "request-secret".to_owned(),
        },
        MagicFinalizeRequest {
            request_id: "logreq_1".to_owned(),
            request_secret: "request-secret".to_owned(),
            totp_code: Some("totp-secret".to_owned()),
            recovery_code: Some("recovery-secret".to_owned()),
        },
    );
    for secret in ["request-secret", "totp-secret", "recovery-secret"] {
        assert!(!logged.contains(secret), "{secret} in {logged}");
    }
    assert!(logged.contains("logreq_1"), "{logged}");
}
