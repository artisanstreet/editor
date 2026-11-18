use artisan_account::{
    body_hash_hex, canonical_request, enrollment_confirm_body, enrollment_request_body,
    sign_enrollment_proof, sign_proof,
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_CORE_BASE_URL: &str = "https://api.artisan.st";

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

#[derive(Debug, thiserror::Error)]
#[error("build the Core HTTP client")]
pub struct BuildCoreClientError(#[source] reqwest::Error);

#[derive(Debug, thiserror::Error)]
pub enum ProofHeadersError {
    #[error("draw the nonce of a machine proof")]
    Nonce(#[source] getrandom::Error),
    #[error("put the machine proof's {header} header on a request")]
    Header {
        header: &'static str,
        #[source]
        source: reqwest::header::InvalidHeaderValue,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CoreCallError {
    #[error("sign the Core {call} request")]
    Proof {
        call: &'static str,
        #[source]
        source: ProofHeadersError,
    },
    #[error("encode the Core {call} request")]
    Encode {
        call: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("reach Core for {call}")]
    Reach {
        call: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("Core answered {call} with {status}: {body}")]
    Refused {
        call: &'static str,
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("read Core's {status} refusal of {call}")]
    ReadRefusal {
        call: &'static str,
        status: reqwest::StatusCode,
        #[source]
        source: reqwest::Error,
    },
    #[error("read Core's answer to {call}")]
    Decode {
        call: &'static str,
        #[source]
        source: reqwest::Error,
    },
}

fn nonce_b64u() -> Result<String, ProofHeadersError> {
    let mut raw = [0_u8; 16];
    getrandom::fill(&mut raw).map_err(ProofHeadersError::Nonce)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw))
}

fn header(
    header: &'static str,
    value: &str,
) -> Result<reqwest::header::HeaderValue, ProofHeadersError> {
    value
        .parse()
        .map_err(|source| ProofHeadersError::Header { header, source })
}

#[derive(Clone, Debug)]
pub struct CoreClient {
    http: reqwest::Client,
    base_url: String,
}

impl CoreClient {
    pub fn new(base_url: &str) -> Result<Self, BuildCoreClientError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(BuildCoreClientError)?,
            base_url: base_url.trim_end_matches('/').to_owned(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    pub(crate) fn proof_headers(
        subject: &str,
        secret: &[u8; 32],
        method: &str,
        path_and_query: &str,
        body: &[u8],
        now: i64,
    ) -> Result<reqwest::header::HeaderMap, ProofHeadersError> {
        let nonce = nonce_b64u()?;
        let canonical =
            canonical_request(now, &nonce, method, path_and_query, &body_hash_hex(body));
        let signature = sign_proof(secret, &canonical);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("X-AST-Machine", header("X-AST-Machine", subject)?);
        headers.insert(
            "X-AST-Timestamp",
            header("X-AST-Timestamp", &now.to_string())?,
        );
        headers.insert("X-AST-Nonce", header("X-AST-Nonce", &nonce)?);
        let authorization = format!(
            "AST-Machine {}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            header("Authorization", &authorization)?,
        );
        Ok(headers)
    }

    async fn answer<T: serde::de::DeserializeOwned>(
        call: &'static str,
        response: Result<reqwest::Response, reqwest::Error>,
    ) -> Result<T, CoreCallError> {
        let response = response.map_err(|source| CoreCallError::Reach { call, source })?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|source| CoreCallError::ReadRefusal {
                    call,
                    status,
                    source,
                })?;
            return Err(CoreCallError::Refused {
                call,
                status,
                body: body.trim().to_owned(),
            });
        }
        response
            .json()
            .await
            .map_err(|source| CoreCallError::Decode { call, source })
    }

    pub async fn enroll_start(
        &self,
        public: &[u8; 32],
        label: &str,
        secret: &[u8; 32],
    ) -> Result<EnrollmentCreated, CoreCallError> {
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
            .await;
        Self::answer("enrollment start", response).await
    }

    pub async fn enrollment_status(
        &self,
        enrollment_id: &str,
        secret: &[u8; 32],
    ) -> Result<artisan_account::PairStatus, CoreCallError> {
        let call = "enrollment status";
        let now = now_sec();
        let path = format!("/v1/machines/enrollments/{enrollment_id}/status");
        let headers = Self::proof_headers(enrollment_id, secret, "GET", &path, b"", now)
            .map_err(|source| CoreCallError::Proof { call, source })?;
        let response = self.http.get(self.url(&path)).headers(headers).send().await;
        Self::answer(call, response).await
    }

    pub async fn confirm_enrollment(
        &self,
        enrollment_id: &str,
        account_id: &str,
        public: &[u8; 32],
        secret: &[u8; 32],
    ) -> Result<ConfirmedMachine, CoreCallError> {
        use base64::Engine as _;
        let call = "enrollment confirm";
        let public_b64u = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public);
        let body_text = enrollment_confirm_body(enrollment_id, account_id, &public_b64u);
        let confirmation = sign_enrollment_proof(secret, &body_text);
        let request = serde_json::json!({
            "enrollmentId": enrollment_id,
            "accountId": account_id,
            "confirmation": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(confirmation),
        });
        let path = "/v1/machines/enrollments/confirm";
        let body = serde_json::to_vec(&request)
            .map_err(|source| CoreCallError::Encode { call, source })?;
        let headers = Self::proof_headers(enrollment_id, secret, "POST", path, &body, now_sec())
            .map_err(|source| CoreCallError::Proof { call, source })?;
        let response = self
            .http
            .post(self.url(path))
            .headers(headers)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await;
        Self::answer(call, response).await
    }

    pub(crate) async fn issue_machine_grant(
        &self,
        machine_id: &str,
        secret: &[u8; 32],
        audience: &str,
        scope: &[&str],
        ttl_seconds: u32,
        target_machine_id: Option<&str>,
    ) -> Result<GrantToken, CoreCallError> {
        let call = "grant issue";
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
        let body = serde_json::to_vec(&request)
            .map_err(|source| CoreCallError::Encode { call, source })?;
        let headers = Self::proof_headers(machine_id, secret, "POST", path, &body, now)
            .map_err(|source| CoreCallError::Proof { call, source })?;
        let response = self
            .http
            .post(self.url(path))
            .headers(headers)
            .json(&request)
            .send()
            .await;
        Self::answer(call, response).await
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentCreated {
    pub enrollment_id: String,
    pub code: String,
    pub expires_at: String,
    pub key_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedMachine {
    pub machine_id: String,
    pub account_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantToken {
    pub(crate) token: String,
    pub(crate) expires_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Jwk {
    pub(crate) kty: String,
    pub(crate) crv: String,
    pub(crate) x: String,
    pub(crate) kid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) r#use: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) alg: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Jwks {
    pub(crate) keys: Vec<Jwk>,
}

#[cfg(test)]
#[path = "../tests/core/unit.rs"]
mod tests;
