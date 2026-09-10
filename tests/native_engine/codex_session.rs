//! Codex app-server session probe fixtures.
//!
//! Registration (controller-owned, not part of this packet):
//! `tests/native_engine/BUILD.bazel` gains a `rust_test` target for
//! `codex_session.rs` depending on `//modules/native_engine:native_engine`.
//!
//! The live exchange needs a real `codex` binary, so these tests drive the
//! pure protocol pieces (request builders, envelope routing, handshake
//! validation, response awaiting) over canned transcripts. Live verification
//! runs at the controller gate.

use artisan_native_engine::codex::{
    CodexClientIdentity, CodexProbeError, ServerEnvelope, await_response_id,
    decode_server_envelope, make_account_read_request, make_initialize_request,
    validate_initialize_result,
};

fn client() -> CodexClientIdentity {
    CodexClientIdentity::new("artisan-editor", "0.3.0").unwrap()
}

#[test]
fn client_identity_rejects_empty_fields() {
    assert!(CodexClientIdentity::new("", "0.3.0").is_none());
    assert!(CodexClientIdentity::new("artisan-editor", "").is_none());
    assert!(CodexClientIdentity::new("artisan-editor", "0.3.0").is_some());
}

#[test]
fn request_builders_match_typescript_protocol() {
    let initialize = make_initialize_request(1, &client());
    assert_eq!(
        initialize,
        serde_json::json!({
            "id": 1,
            "method": "initialize",
            "params": {
                "capabilities": {
                    "experimentalApi": false,
                    "optOutNotificationMethods": [
                        "account/rateLimits/updated",
                        "mcpServer/startupStatus/updated",
                        "remoteControl/status/changed",
                    ],
                    "requestAttestation": false,
                },
                "clientInfo": { "name": "artisan-editor", "version": "0.3.0" },
            },
        })
    );
    assert_eq!(
        make_account_read_request(7),
        serde_json::json!({ "id": 7, "method": "account/read", "params": {} })
    );
}

#[test]
fn envelope_routing_matches_response_request_and_notification() {
    let response = decode_server_envelope(br#"{"id":1,"result":{"codexHome":"C:\\x"}}"#).unwrap();
    assert!(matches!(response, ServerEnvelope::Response { .. }));

    let string_id = decode_server_envelope(br#"{"jsonrpc":"2.0","id":"a","result":{}}"#).unwrap();
    assert!(matches!(string_id, ServerEnvelope::Response { .. }));

    let error =
        decode_server_envelope(br#"{"id":2,"error":{"code":-32600,"message":"bad"}}"#).unwrap();
    assert!(matches!(error, ServerEnvelope::ErrorResponse { .. }));

    let request = decode_server_envelope(br#"{"id":9,"method":"approval/request"}"#).unwrap();
    assert!(matches!(request, ServerEnvelope::ServerRequest { .. }));

    let notification =
        decode_server_envelope(br#"{"method":"account/rateLimits/updated"}"#).unwrap();
    assert!(matches!(notification, ServerEnvelope::Notification { .. }));
}

#[test]
fn ambiguous_and_malformed_envelopes_are_protocol_errors() {
    for line in [
        br#"{"id":1,"method":"m","result":{}}"#.as_slice(),
        br#"{"id":1,"result":{},"error":{"code":1,"message":"x"}}"#.as_slice(),
        br#"{"id":1}"#.as_slice(),
        br#"{"result":{}}"#.as_slice(),
        br#"{"id":null,"result":{}}"#.as_slice(),
        br#"{"id":1.5,"result":{}}"#.as_slice(),
        br#"{"id":true,"result":{}}"#.as_slice(),
        br#"{"id":1,"method":""}"#.as_slice(),
        br#"{"id":1,"method":42}"#.as_slice(),
        br#"[]"#.as_slice(),
        br#"not json"#.as_slice(),
    ] {
        assert_eq!(
            decode_server_envelope(line),
            Err(CodexProbeError::Protocol),
            "unexpected acceptance"
        );
    }
}

#[test]
fn initialize_result_requires_non_empty_fields_and_ignores_extras() {
    let info = validate_initialize_result(&serde_json::json!({
        "codexHome": "C:\\Users\\runner\\.codex",
        "platformFamily": "windows",
        "platformOs": "windows",
        "userAgent": "codex/0.145.0",
        "futureField": true,
    }))
    .unwrap();
    assert_eq!(info.codex_home(), "C:\\Users\\runner\\.codex");
    assert_eq!(info.platform_family(), "windows");

    for result in [
        serde_json::json!({}),
        serde_json::json!({
            "codexHome": "",
            "platformFamily": "windows",
            "platformOs": "windows",
            "userAgent": "codex",
        }),
        serde_json::json!({
            "codexHome": "C:\\x",
            "platformFamily": "windows",
            "platformOs": "windows",
        }),
        serde_json::json!([]),
    ] {
        assert_eq!(
            validate_initialize_result(&result),
            Err(CodexProbeError::Protocol)
        );
    }
}

fn transcript(lines: &[&str]) -> Vec<Result<ServerEnvelope, CodexProbeError>> {
    lines
        .iter()
        .map(|line| decode_server_envelope(line.as_bytes()))
        .collect()
}

#[test]
fn await_skips_stray_envelopes_and_matches_identifier() {
    let mut envelopes = transcript(&[
        r#"{"method":"account/rateLimits/updated"}"#,
        r#"{"id":9,"method":"approval/request"}"#,
        r#"{"id":8,"result":{"stale":true}}"#,
        r#"{"id":1,"result":{"ok":true}}"#,
    ])
    .into_iter();
    let mut budget = 16;
    assert_eq!(
        await_response_id(&mut envelopes, 1, &mut budget),
        Ok(serde_json::json!({"ok": true}))
    );
    assert_eq!(budget, 12);
}

#[test]
fn await_rejects_error_responses_budget_exhaustion_and_closed_streams() {
    let mut errored =
        transcript(&[r#"{"id":2,"error":{"code":-32600,"message":"bad"}}"#]).into_iter();
    let mut budget = 16;
    assert_eq!(
        await_response_id(&mut errored, 2, &mut budget),
        Err(CodexProbeError::Protocol)
    );

    let mut noisy = transcript(&[
        r#"{"method":"a"}"#,
        r#"{"method":"b"}"#,
        r#"{"id":1,"result":{}}"#,
    ])
    .into_iter();
    let mut budget = 2;
    assert_eq!(
        await_response_id(&mut noisy, 1, &mut budget),
        Err(CodexProbeError::Protocol)
    );

    let mut closed = transcript(&[]).into_iter();
    let mut budget = 16;
    assert_eq!(
        await_response_id(&mut closed, 1, &mut budget),
        Err(CodexProbeError::Unavailable)
    );

    let mut failed = vec![Err(CodexProbeError::Timeout)].into_iter();
    let mut budget = 16;
    assert_eq!(
        await_response_id(&mut failed, 1, &mut budget),
        Err(CodexProbeError::Timeout)
    );
}
