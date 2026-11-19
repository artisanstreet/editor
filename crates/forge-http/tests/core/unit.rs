use super::*;
use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};

#[derive(Clone)]
struct Stub;

async fn enroll(
    State(_stub): State<Stub>,
    body: axum::body::Bytes,
) -> impl axum::response::IntoResponse {
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["label"], "server-forge");
    assert!(request["proof"].as_str().is_some());
    axum::Json(serde_json::json!({
        "enrollment_id": "enr_00000000000000000000000001",
        "code": "ABCDEFGH",
        "expires_at": "2026-10-26T20:10:00.000Z",
        "key_fingerprint": "fp",
    }))
}

async fn status(
    State(_stub): State<Stub>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    assert_eq!(id, "enr_00000000000000000000000001");
    for name in [
        "x-ast-machine",
        "x-ast-timestamp",
        "x-ast-nonce",
        "authorization",
    ] {
        assert!(
            headers.contains_key(name),
            "missing machine proof header {name}"
        );
    }
    let authorization = headers["authorization"].to_str().unwrap();
    assert!(authorization.starts_with("AST-Machine "));
    axum::Json(serde_json::json!({
        "enrollment_id": "enr_00000000000000000000000001",
        "status": "claimed",
        "account_id": "acct_00000000000000000000000000",
        "email": "op@example.invalid",
        "display_name": "Op",
        "expires_at": "2026-10-26T20:10:00.000Z",
    }))
}

async fn confirm(
    State(_stub): State<Stub>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl axum::response::IntoResponse {
    let enrollment_id = "enr_00000000000000000000000001";
    assert_eq!(headers["x-ast-machine"], enrollment_id);
    let timestamp = headers["x-ast-timestamp"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let nonce = headers["x-ast-nonce"].to_str().unwrap();
    let signature = headers["authorization"]
        .to_str()
        .unwrap()
        .strip_prefix("AST-Machine ")
        .unwrap();
    let keypair = artisan_account::MachineKeypair::from_secret([0x21_u8; 32]).unwrap();
    artisan_account::verify_machine_proof(&artisan_account::MachineProof {
        public: keypair.public(),
        body: &body,
        timestamp_sec: timestamp,
        nonce_b64u: nonce,
        method: "POST",
        path_and_query: "/v1/machines/enrollments/confirm",
        signature_b64u: signature,
        now_sec: now_sec(),
    })
    .unwrap();
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["enrollmentId"], enrollment_id);
    assert_eq!(request["accountId"], "acct_00000000000000000000000000");
    let confirmation = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(request["confirmation"].as_str().unwrap())
        .unwrap();
    let binding = enrollment_confirm_body(
        enrollment_id,
        "acct_00000000000000000000000000",
        &keypair.public_b64u(),
    );
    assert!(artisan_account::verify_enrollment_proof(
        keypair.public(),
        &binding,
        &confirmation,
    ));
    axum::Json(serde_json::json!({
        "machine_id": "mch_11111111111111111111111111",
        "account_id": "acct_00000000000000000000000000",
    }))
}

async fn grants_issue(
    State(_stub): State<Stub>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl axum::response::IntoResponse {
    assert!(headers.contains_key("authorization"));
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["audience"], "ast-relay");
    axum::Json(serde_json::json!({
        "token": "grant.jwt.token",
        "expires_at": "2026-10-26T20:05:00.000Z",
    }))
}

fn stub_router() -> Router {
    let stub = Stub;
    Router::new()
        .route("/v1/machines/enrollments", post(enroll))
        .route("/v1/machines/enrollments/{id}/status", get(status))
        .route("/v1/machines/enrollments/confirm", post(confirm))
        .route("/v1/grants/issue", post(grants_issue))
        .with_state(stub)
}

#[tokio::test]
async fn machine_journey_against_stub_core() {
    let core = test_support::http::StubServer::serve(stub_router()).await;
    let client = CoreClient::new(core.url()).unwrap();
    let keypair = artisan_account::MachineKeypair::from_secret([0x21_u8; 32]).unwrap();
    let secret = *keypair.secret();
    let public = *keypair.public();
    let created = client
        .enroll_start(&public, "server-forge", &secret)
        .await
        .unwrap();
    assert_eq!(created.code, "ABCDEFGH");
    let status = client
        .enrollment_status(&created.enrollment_id, &secret)
        .await
        .unwrap();
    assert_eq!(
        status.account_id.as_deref(),
        Some("acct_00000000000000000000000000")
    );
    let confirmed = client
        .confirm_enrollment(
            &created.enrollment_id,
            "acct_00000000000000000000000000",
            &public,
            &secret,
        )
        .await
        .unwrap();
    assert_eq!(confirmed.machine_id, "mch_11111111111111111111111111");
    let grant = client
        .issue_machine_grant(
            &confirmed.machine_id,
            &secret,
            "ast-relay",
            &["relay:connect"],
            300,
            Some(&confirmed.machine_id),
        )
        .await
        .unwrap();
    assert_eq!(grant.token, "grant.jwt.token");
}

#[test]
fn a_logged_grant_never_shows_its_token() {
    let grant: GrantToken = serde_json::from_value(serde_json::json!({
        "token": "grant-secret-7f3a",
        "expires_at": "2030-01-01T00:00:00Z",
    }))
    .unwrap();
    let logged = format!("{grant:?}");
    assert!(!logged.contains("grant-secret-7f3a"), "{logged}");
    assert!(logged.contains("2030-01-01T00:00:00Z"), "{logged}");
}
