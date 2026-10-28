//! Core HTTP client: the Forge as a machine calling its account authority.
//!
//! Real network client against the normative Core endpoints
//! (`contracts.md` §3–§5): enrollment start/poll/confirm, machine grant
//! issue, JWKS fetch. Every authenticated call signs the Core canonical
//! request with the machine/enrollment private key. Base URL comes from
//! `AST_CORE_BASE_URL` (default production `https://api.artisan.st`).

use anyhow::{Context, Result};
use artisan_account::{
    body_hash_hex, canonical_request, enrollment_confirm_body, enrollment_request_body,
    sign_enrollment_proof, sign_proof,
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Where Core lives when nothing else is configured.
pub const DEFAULT_CORE_BASE_URL: &str = "https://api.artisan.st";

/// Base URL resolution: explicit argument, else `AST_CORE_BASE_URL`, else production.
#[must_use]
pub fn core_base_url(explicit: Option<&str>) -> String {
    explicit
        .map(str::to_owned)
        .or_else(|| std::env::var("AST_CORE_BASE_URL").ok())
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CORE_BASE_URL.to_owned())
}

pub(crate) fn now_sec() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    )
    .unwrap_or(0)
}

fn nonce_b64u() -> Result<String> {
    let mut raw = [0_u8; 16];
    getrandom::fill(&mut raw).map_err(|_rand| anyhow::anyhow!("random nonce failed"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw))
}

/// The client: machine identity + Core base URL. Clones cheaply.
#[derive(Clone, Debug)]
pub struct CoreClient {
    http: reqwest::Client,
    base_url: String,
}

impl CoreClient {
    pub fn new(base_url: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .context("build the Core HTTP client")?,
            base_url: base_url.trim_end_matches('/').to_owned(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Machine-proof headers for an authenticated machine call.
    fn proof_headers(
        subject: &str,
        secret: &[u8; 32],
        method: &str,
        path_and_query: &str,
        body: &[u8],
        now: i64,
    ) -> Result<reqwest::header::HeaderMap> {
        let nonce = nonce_b64u()?;
        let canonical =
            canonical_request(now, &nonce, method, path_and_query, &body_hash_hex(body));
        let signature = sign_proof(secret, &canonical);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "X-AST-Machine",
            subject.parse().context("machine subject header")?,
        );
        headers.insert(
            "X-AST-Timestamp",
            now.to_string().parse().context("timestamp header")?,
        );
        headers.insert("X-AST-Nonce", nonce.parse().context("nonce header")?);
        let authorization = format!(
            "AST-Machine {}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            authorization.parse().context("signature header")?,
        );
        Ok(headers)
    }

    async fn check(response: reqwest::Response) -> Result<reqwest::Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        anyhow::bail!("Core answered {status}: {}", text.trim())
    }

    /// Start an enrollment: `POST /v1/machines/enrollments`.
    pub async fn enroll_start(
        &self,
        public: &[u8; 32],
        label: &str,
        secret: &[u8; 32],
    ) -> Result<EnrollmentCreated> {
        use base64::Engine as _;
        let now = now_sec();
        let public_b64u = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public);
        let body_text = enrollment_request_body(&public_b64u, label, now);
        let proof = sign_enrollment_proof(secret, &body_text);
        let request = serde_json::json!({
            "publicKey": {"kty": "OKP", "crv": "Ed25519", "x": public_b64u},
            "label": label,
            "timestamp": now,
            "proof": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(proof),
        });
        let response = self
            .http
            .post(self.url("/v1/machines/enrollments"))
            .json(&request)
            .send()
            .await
            .context("reach Core")?;
        Self::check(response)
            .await?
            .json()
            .await
            .context("read Core enrollment answer")
    }

    /// Poll an enrollment: `GET …/enrollments/{id}/status` with the
    /// enrollment-key machine proof (`X-AST-Machine: enr_…`).
    pub async fn enrollment_status(
        &self,
        enrollment_id: &str,
        secret: &[u8; 32],
    ) -> Result<artisan_account::PairStatus> {
        let now = now_sec();
        let path = format!("/v1/machines/enrollments/{enrollment_id}/status");
        let headers = Self::proof_headers(enrollment_id, secret, "GET", &path, b"", now)?;
        let response = self
            .http
            .get(self.url(&path))
            .headers(headers)
            .send()
            .await
            .context("reach Core")?;
        Self::check(response)
            .await?
            .json()
            .await
            .context("read Core status answer")
    }

    /// Confirm an enrollment: `POST /v1/machines/enrollments/confirm` with
    /// enrollment-key request authentication and a proof over the exact
    /// account+key binding. The caller must have
    /// shown `account_id` + verified email to the local operator first.
    pub async fn confirm_enrollment(
        &self,
        enrollment_id: &str,
        account_id: &str,
        public: &[u8; 32],
        secret: &[u8; 32],
    ) -> Result<ConfirmedMachine> {
        use base64::Engine as _;
        let public_b64u = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public);
        let body_text = enrollment_confirm_body(enrollment_id, account_id, &public_b64u);
        let confirmation = sign_enrollment_proof(secret, &body_text);
        let request = serde_json::json!({
            "enrollmentId": enrollment_id,
            "accountId": account_id,
            "confirmation": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(confirmation),
        });
        let path = "/v1/machines/enrollments/confirm";
        let body = serde_json::to_vec(&request)?;
        let headers = Self::proof_headers(enrollment_id, secret, "POST", path, &body, now_sec())?;
        let response = self
            .http
            .post(self.url(path))
            .headers(headers)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .context("reach Core")?;
        Self::check(response)
            .await?
            .json()
            .await
            .context("read Core confirm answer")
    }

    /// Issue a machine grant: `POST /v1/grants/issue` (unified endpoint;
    /// machine proof auth via `X-AST-Machine`, so `cnf` defaults to the
    /// enrolled key server-side and is omitted here).
    pub async fn issue_machine_grant(
        &self,
        machine_id: &str,
        secret: &[u8; 32],
        audience: &str,
        scope: &[&str],
        ttl_seconds: u32,
        target_machine_id: Option<&str>,
    ) -> Result<GrantToken> {
        let now = now_sec();
        let path = "/v1/grants/issue";
        let mut request = serde_json::json!({
            "audience": audience,
            "scope": scope,
            "ttlSeconds": ttl_seconds,
        });
        if let Some(target) = target_machine_id {
            request["targetMachineId"] = serde_json::Value::String(target.to_owned());
        }
        let body = serde_json::to_vec(&request)?;
        let headers = Self::proof_headers(machine_id, secret, "POST", path, &body, now)?;
        let response = self
            .http
            .post(self.url(path))
            .headers(headers)
            .json(&request)
            .send()
            .await
            .context("reach Core")?;
        Self::check(response)
            .await?
            .json()
            .await
            .context("read Core grant answer")
    }

    /// Fetch the JWKS Core publishes for grant verification.
    pub async fn jwks(&self) -> Result<Jwks> {
        let response = self
            .http
            .get(self.url("/.well-known/jwks.json"))
            .send()
            .await
            .context("reach Core")?;
        Self::check(response)
            .await?
            .json()
            .await
            .context("read Core JWKS")
    }

    /// List this account's machines: `GET /v1/machines` (machine proof auth;
    /// forward-compatible entries).
    pub async fn list_machines(
        &self,
        machine_id: &str,
        secret: &[u8; 32],
    ) -> Result<Vec<MachineEntry>> {
        let now = now_sec();
        let path = "/v1/machines";
        let headers = Self::proof_headers(machine_id, secret, "GET", path, b"", now)?;
        let response = self
            .http
            .get(self.url(path))
            .headers(headers)
            .send()
            .await
            .context("reach Core")?;
        Self::check(response)
            .await?
            .json()
            .await
            .context("read Core machines answer")
    }
}

/// Build the exact headers without sending (used by tests and `ae`).
pub fn proof_headers_for(
    subject: &str,
    secret: &[u8; 32],
    method: &str,
    path_and_query: &str,
    body: &[u8],
    now: i64,
) -> Result<reqwest::header::HeaderMap> {
    CoreClient::proof_headers(subject, secret, method, path_and_query, body, now)
}

/// `POST /v1/machines/enrollments` → created (strict read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentCreated {
    pub enrollment_id: String,
    pub code: String,
    pub expires_at: String,
    pub key_fingerprint: String,
}

/// `POST …/confirm` → created machine (strict read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedMachine {
    pub machine_id: String,
    pub account_id: String,
}

/// Grant issue answer (strict read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantToken {
    pub token: String,
    pub expires_at: String,
}

/// One JWKS key (public parts only; strict read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub kid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#use: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alg: Option<String>,
}

/// `GET /.well-known/jwks.json` (strict read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Jwks {
    pub keys: Vec<Jwk>,
}

/// One machine entry (forward-compatible: ignores future fields).
/// Matches Core `MachineInfo` (`id`, not `machine_id`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineEntry {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::extract::State;
    use axum::routing::{get, post};

    #[derive(Clone)]
    struct Stub {
        expected_proof: bool,
    }

    async fn enroll(
        State(_stub): State<Stub>,
        body: axum::body::Bytes,
    ) -> impl axum::response::IntoResponse {
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(request["label"], "server-forge");
        assert!(request["proof"].as_str().is_some());
        // The proof body is over enrollment-request + JWK x + label + ts.
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
        // Machine-proof headers are present and well-formed.
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

    async fn machines(
        State(_stub): State<Stub>,
        headers: axum::http::HeaderMap,
    ) -> impl axum::response::IntoResponse {
        assert!(headers.contains_key("authorization"));
        // Forward-compatible entries may carry future fields.
        // Matches Core `MachineInfo` (`id`, not `machine_id`).
        axum::Json(serde_json::json!([
            {"id": "mch_11111111111111111111111111", "label": "srv", "future": 1},
        ]))
    }

    fn stub_router() -> Router {
        let stub = Stub {
            expected_proof: true,
        };
        Router::new()
            .route("/v1/machines/enrollments", post(enroll))
            .route("/v1/machines/enrollments/{id}/status", get(status))
            .route("/v1/machines/enrollments/confirm", post(confirm))
            .route("/v1/grants/issue", post(grants_issue))
            .route("/v1/machines", get(machines))
            .with_state(stub)
    }

    /// The full machine journey against a stub Core: enroll → poll status →
    /// confirm → grant issue → machines list. Asserts the exact proof
    /// headers and body shapes Core specifies.
    #[tokio::test]
    async fn machine_journey_against_stub_core() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, stub_router()).await.unwrap();
        });
        let client = CoreClient::new(&format!("http://{address}")).unwrap();
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
        let machines = client
            .list_machines(&confirmed.machine_id, &secret)
            .await
            .unwrap();
        assert_eq!(machines.len(), 1);
    }

    #[test]
    fn core_base_url_defaults_to_production() {
        assert_eq!(
            core_base_url(None).trim(),
            core_base_url(Some(DEFAULT_CORE_BASE_URL)).trim()
        );
    }
}
