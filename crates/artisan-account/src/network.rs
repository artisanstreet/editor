//! Session-authed Core HTTP client shared by the native Editor (and CLI).
//!
//! Mirrors the Core OpenAPI (`core/contracts/v1.openapi.json`) exactly:
//! requests deny unknown fields (Core is strict); responses ignore unknown
//! fields (forward-compatible reads). No business logic beyond transport:
//! no stores, no crypto (proofs/keys stay in `enrollment`/`pop`), and the
//! raw session token is captured from `Set-Cookie` and never logged.
//!
//! Owned by the Editor side until root routes otherwise; the Relay client
//! stays Vilde's.

use serde::{Deserialize, Serialize};

use crate::signin::{
    MagicConsumeRequest, MagicConsumeResponse, MagicFinalizeRequest, MagicFinalizeResponse,
    MagicRequest, MagicRequestResponse, MagicStatusRequest, MagicStatusResponse,
};

/// Opaque session token. Never logged or displayed; redacted `Debug`,
/// zeroed on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(Vec<u8>);

impl SessionToken {
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "String::into_bytes is not const; token owns its bytes"
    )]
    pub fn new(token: String) -> Self {
        Self(token.into_bytes())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        // Only ever constructed from `&str`; the fallback is unreachable.
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionToken(redacted)")
    }
}

impl Drop for SessionToken {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// Transport + Core errors. User text is composed by the caller from `code`
/// (plain language, no secret/ID pasting); the request id is for logs only.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NetworkError {
    #[error("could not reach Core")]
    Transport,
    #[error("Core refused the call: {code}")]
    Core {
        code: String,
        #[allow(dead_code, reason = "Core message kept for exact shape; user text uses code only")]
        message: String,
        request_id: String,
    },
}

/// Core error envelope (strict read).
#[derive(Clone, Debug, Deserialize)]
struct CoreErrorBody {
    error: CoreErrorDetail,
}

#[derive(Clone, Debug, Deserialize)]
struct CoreErrorDetail {
    code: String,
    #[allow(dead_code, reason = "Core message kept for exact shape; user text uses code only")]
    message: String,
    request_id: String,
}

/// Ed25519 JWK (`kty: OKP, crv: Ed25519`), as Core emits it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
}

/// One enrolled Forge, as `/v1/me` and `/v1/machines` report it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineEntry {
    pub id: String,
    pub label: String,
    pub public_key: Jwk,
    pub created_at: String,
    #[serde(default)]
    pub revoked_at: Option<String>,
    #[serde(default)]
    pub last_seen_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct MachinesResponse {
    machines: Vec<MachineEntry>,
}

/// The verified profile behind `GET /v1/me`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountProfile {
    pub account_id: String,
    pub email: String,
    pub display_name: String,
    pub totp_enrolled: bool,
    #[serde(default)]
    pub machines: Vec<MachineEntry>,
    #[serde(default)]
    pub latest_config_version: Option<u64>,
}

/// `GET /v1/sessions` entry (forward-compatible read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: String,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct SessionsResponse {
    #[serde(default)]
    sessions: Vec<SessionEntry>,
}

/// `POST /v1/machines/enrollments/claim`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRequest {
    pub code: String,
}

/// `POST /v1/grants/issue` from an authenticated session (Editor path:
/// explicit target machine + ephemeral `cnf`).
///
/// Wire shape: `cnf` is the PLAIN JWK `{kty, crv, x}`. Only the ISSUED
/// JWT nests it (`cnf.jwk` claim); sending `{cnf: {jwk: {...}}}` fails
/// Core validation with 400.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantIssueRequest {
    pub audience: String,
    pub scope: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "ttlSeconds"
    )]
    pub ttl_seconds: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "targetMachineId"
    )]
    pub target_machine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cnf: Option<Jwk>,
}

/// The `cnf` of a grant request: exactly the [`Jwk`]. Kept as a name so
/// existing imports keep working; there is no extra nesting on the wire.
pub type GrantCnf = Jwk;

#[derive(Clone, Debug, Deserialize)]
pub struct GrantToken {
    pub token: String,
    pub expires_at: String,
}

/// Latest config bundle head (version + hash only; manifest sync stays with
/// the skills sync owner). ETag revalidation is the caller's job.
#[derive(Clone, Debug, Deserialize)]
pub struct BundleHead {
    pub version: u64,
    pub content_hash: String,
}

#[derive(Clone, Debug, Deserialize)]
struct LatestBundleResponse {
    bundle: Option<BundleHead>,
}

/// Session-authed Core HTTP. One client per base URL; cheap to clone.
#[derive(Clone, Debug)]
pub struct CoreSessionClient {
    http: reqwest::Client,
    base_url: String,
}

impl CoreSessionClient {
    #[expect(
        clippy::map_err_ignore,
        reason = "transport errors stay coarse Transport so tokens and Core internals never leak"
    )]
    pub fn new(base_url: &str) -> Result<Self, NetworkError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|_| NetworkError::Transport)?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn check(
        response: reqwest::Response,
        action: &'static str,
    ) -> Result<reqwest::Response, NetworkError> {
        let _ = action;
        if response.status().is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        if let Ok(parsed) = serde_json::from_str::<CoreErrorBody>(&body) {
            return Err(NetworkError::Core {
                code: parsed.error.code,
                message: parsed.error.message,
                request_id: parsed.error.request_id,
            });
        }
        Err(NetworkError::Transport)
    }

    #[expect(
        clippy::map_err_ignore,
        reason = "transport errors stay coarse Transport so tokens and Core internals never leak"
    )]
    async fn post<B: Serialize + Sync, R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &B,
        session: Option<&SessionToken>,
        action: &'static str,
    ) -> Result<R, NetworkError> {
        let mut request = self.http.post(self.url(path)).json(body);
        if let Some(session) = session {
            request = request.bearer_auth(session.as_str());
        }
        let response = request.send().await.map_err(|_| NetworkError::Transport)?;
        let response = Self::check(response, action).await?;
        response.json().await.map_err(|_| NetworkError::Transport)
    }

    #[expect(
        clippy::map_err_ignore,
        reason = "transport errors stay coarse Transport so tokens and Core internals never leak"
    )]
    async fn get<R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        session: Option<&SessionToken>,
        action: &'static str,
    ) -> Result<R, NetworkError> {
        let mut request = self.http.get(self.url(path));
        if let Some(session) = session {
            request = request.bearer_auth(session.as_str());
        }
        let response = request.send().await.map_err(|_| NetworkError::Transport)?;
        let response = Self::check(response, action).await?;
        response.json().await.map_err(|_| NetworkError::Transport)
    }

    /// Start a magic-link request. The secret returns here once; the emailed
    /// token alone can never mint a session.
    pub async fn magic_request(
        &self,
        email: &str,
        device_name: &str,
    ) -> Result<MagicRequestResponse, NetworkError> {
        self.post::<_, _>(
            "/v1/auth/magic/request",
            &MagicRequest {
                email: email.to_owned(),
                display_name: None,
                device_name: Some(device_name.to_owned()),
            },
            None,
            "request a sign-in link",
        )
        .await
    }

    pub async fn magic_status(
        &self,
        request_id: &str,
        request_secret: &str,
    ) -> Result<MagicStatusResponse, NetworkError> {
        self.post::<_, _>(
            "/v1/auth/magic/status",
            &MagicStatusRequest {
                request_id: request_id.to_owned(),
                request_secret: request_secret.to_owned(),
            },
            None,
            "check the sign-in request",
        )
        .await
    }

    pub async fn magic_consume(&self, token: &str) -> Result<MagicConsumeResponse, NetworkError> {
        self.post::<_, _>(
            "/v1/auth/magic/consume",
            &MagicConsumeRequest {
                token: token.to_owned(),
                request_secret: None,
            },
            None,
            "approve the sign-in link",
        )
        .await
    }

    /// Finalize and capture the raw session token from `Set-Cookie`
    /// (`ast_session=`). The token never appears in a JSON body.
    #[expect(
        clippy::map_err_ignore,
        reason = "transport errors stay coarse Transport so tokens and Core internals never leak"
    )]
    pub async fn magic_finalize(
        &self,
        request_id: &str,
        request_secret: &str,
        totp_code: Option<&str>,
        recovery_code: Option<&str>,
    ) -> Result<(MagicFinalizeResponse, SessionToken), NetworkError> {
        let response = self
            .http
            .post(self.url("/v1/auth/magic/finalize"))
            .json(&MagicFinalizeRequest {
                request_id: request_id.to_owned(),
                request_secret: request_secret.to_owned(),
                totp_code: totp_code.map(str::to_owned),
                recovery_code: recovery_code.map(str::to_owned),
            })
            .send()
            .await
            .map_err(|_| NetworkError::Transport)?;
        let response = Self::check(response, "finish signing in").await?;
        let token = response
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|cookie| {
                cookie
                    .split(';')
                    .next()
                    .and_then(|pair| pair.trim().strip_prefix("ast_session="))
                    .map(str::to_owned)
            })
            .ok_or(NetworkError::Transport)?;
        let finalized: MagicFinalizeResponse =
            response.json().await.map_err(|_| NetworkError::Transport)?;
        Ok((finalized, SessionToken::new(token)))
    }

    pub async fn me(&self, session: &SessionToken) -> Result<AccountProfile, NetworkError> {
        self.get("/v1/me", Some(session), "load the account").await
    }

    pub async fn list_sessions(
        &self,
        session: &SessionToken,
    ) -> Result<Vec<SessionEntry>, NetworkError> {
        Ok(self
            .get::<SessionsResponse>("/v1/sessions", Some(session), "list sessions")
            .await?
            .sessions)
    }

    pub async fn revoke_current(&self, session: &SessionToken) -> Result<(), NetworkError> {
        let _: serde_json::Value = self
            .post(
                "/v1/sessions/current/revoke",
                &serde_json::json!({}),
                Some(session),
                "sign out",
            )
            .await?;
        Ok(())
    }

    pub async fn revoke_all_sessions(&self, session: &SessionToken) -> Result<(), NetworkError> {
        let _: serde_json::Value = self
            .post(
                "/v1/sessions/revoke-all",
                &serde_json::json!({}),
                Some(session),
                "sign out everywhere",
            )
            .await?;
        Ok(())
    }

    pub async fn totp_enroll(
        &self,
        session: &SessionToken,
    ) -> Result<TotpEnrollResponse, NetworkError> {
        self.post::<_, _>(
            "/v1/auth/totp/enroll",
            &serde_json::json!({}),
            Some(session),
            "start second-factor enrollment",
        )
        .await
    }

    pub async fn totp_verify(
        &self,
        session: &SessionToken,
        code: &str,
    ) -> Result<(), NetworkError> {
        let _: serde_json::Value = self
            .post(
                "/v1/auth/totp/verify",
                &TotpVerifyRequest {
                    code: code.to_owned(),
                },
                Some(session),
                "verify the second factor",
            )
            .await?;
        Ok(())
    }

    pub async fn totp_disable(
        &self,
        session: &SessionToken,
        code: &str,
    ) -> Result<(), NetworkError> {
        let _: serde_json::Value = self
            .post(
                "/v1/auth/totp/disable",
                &TotpDisableRequest {
                    code: code.to_owned(),
                },
                Some(session),
                "disable the second factor",
            )
            .await?;
        Ok(())
    }

    pub async fn claim_enrollment(
        &self,
        session: &SessionToken,
        code: &str,
    ) -> Result<ClaimedEnrollment, NetworkError> {
        self.post(
            "/v1/machines/enrollments/claim",
            &ClaimRequest {
                code: code.to_owned(),
            },
            Some(session),
            "claim the pairing code",
        )
        .await
    }

    pub async fn list_machines(
        &self,
        session: &SessionToken,
    ) -> Result<Vec<MachineEntry>, NetworkError> {
        Ok(self
            .get::<MachinesResponse>("/v1/machines", Some(session), "list Forges")
            .await?
            .machines)
    }

    pub async fn revoke_machine(
        &self,
        session: &SessionToken,
        machine_id: &str,
    ) -> Result<(), NetworkError> {
        let _: serde_json::Value = self
            .post(
                &format!("/v1/machines/{machine_id}/revoke"),
                &serde_json::json!({}),
                Some(session),
                "remove the Forge",
            )
            .await?;
        Ok(())
    }

    /// Session-authed Editor grant for a Relay target. `cnf_x_b64u` is the
    /// ephemeral key's public half (base64url); the secret never leaves the
    /// device except inside the PoP proof the Relay transport makes.
    pub async fn issue_editor_grant(
        &self,
        session: &SessionToken,
        target_machine_id: &str,
        cnf_x_b64u: &str,
    ) -> Result<GrantToken, NetworkError> {
        self.post(
            "/v1/grants/issue",
            &GrantIssueRequest {
                audience: "ast-relay".to_owned(),
                scope: vec!["relay:connect".to_owned(), "forge:execute".to_owned()],
                ttl_seconds: Some(300),
                target_machine_id: Some(target_machine_id.to_owned()),
                cnf: Some(Jwk {
                    kty: "OKP".to_owned(),
                    crv: "Ed25519".to_owned(),
                    x: cnf_x_b64u.to_owned(),
                }),
            },
            Some(session),
            "prepare the Relay connection",
        )
        .await
    }

    /// Latest config bundle head (version + hash). Manifest sync stays with
    /// the skills-sync owner; the controller only caches the head.
    pub async fn latest_bundle_head(
        &self,
        session: &SessionToken,
    ) -> Result<Option<BundleHead>, NetworkError> {
        Ok(self
            .get::<LatestBundleResponse>(
                "/v1/config/bundles/latest",
                Some(session),
                "check the config revision",
            )
            .await?
            .bundle)
    }

    /// The JWKS Core publishes for grant verification.
    pub async fn jwks(&self) -> Result<serde_json::Value, NetworkError> {
        self.get("/.well-known/jwks.json", None, "load Core keys")
            .await
    }
}

/// TOTP enrollment answer: `POST /v1/auth/totp/enroll`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TotpEnrollResponse {
    pub otpauth_url: String,
    pub recovery_codes: Vec<String>,
}

/// `POST /v1/auth/totp/verify`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TotpVerifyRequest {
    pub code: String,
}

/// `POST /v1/auth/totp/disable`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TotpDisableRequest {
    pub code: String,
}

/// `POST /v1/machines/enrollments/claim` answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedEnrollment {
    pub enrollment_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_reject_unknown_fields() {
        let raw = serde_json::json!({"email": "a@example.invalid", "future": 1});
        assert!(serde_json::from_value::<MagicRequest>(raw).is_err());
        let raw = serde_json::json!({
            "requestId": "logreq_1",
            "requestSecret": "s",
        });
        assert!(serde_json::from_value::<MagicStatusRequest>(raw).is_ok());
        let raw = serde_json::json!({
            "requestId": "logreq_1",
            "requestSecret": "s",
            "future": 1,
        });
        assert!(serde_json::from_value::<MagicStatusRequest>(raw).is_err());
    }

    #[test]
    fn responses_tolerate_future_fields() {
        let raw = serde_json::json!({
            "request_id": "logreq_1",
            "status": "pending",
            "totp_required": false,
            "email": "a@example.invalid",
            "expires_at": "t",
            "future": [1],
        });
        let status: MagicStatusResponse = serde_json::from_value(raw).unwrap();
        assert!(!status.totp_required);
    }

    #[test]
    fn session_token_never_prints() {
        let token = SessionToken::new("STUB-SESSION-SECRET".to_owned());
        let rendered = format!("{token:?}");
        assert!(!rendered.contains("STUB-SESSION-SECRET"), "{rendered}");
        assert!(rendered.contains("redacted"));
        assert_eq!(token.as_str(), "STUB-SESSION-SECRET");
    }

    #[test]
    fn grant_issue_rejects_unknown_fields() {
        let raw = serde_json::json!({
            "audience": "ast-relay",
            "scope": ["relay:connect"],
            "future": true,
        });
        assert!(serde_json::from_value::<GrantIssueRequest>(raw).is_err());
    }

    /// The grant request body must be Core's exact shape: `cnf` is the
    /// plain JWK, never nested under `jwk` (that nesting lives only in
    /// the issued JWT's claims). A nested body fails Core with 400, which
    /// broke both the initial Editor grant and the stream grant.
    #[test]
    fn grant_issue_body_matches_core_shape() {
        let request = GrantIssueRequest {
            audience: "ast-relay".to_owned(),
            scope: vec!["relay:connect".to_owned(), "forge:execute".to_owned()],
            ttl_seconds: Some(300),
            target_machine_id: Some("mch_11111111111111111111111111".to_owned()),
            cnf: Some(Jwk {
                kty: "OKP".to_owned(),
                crv: "Ed25519".to_owned(),
                x: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            }),
        };
        let body = serde_json::to_value(&request).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "audience": "ast-relay",
                "scope": ["relay:connect", "forge:execute"],
                "ttlSeconds": 300,
                "targetMachineId": "mch_11111111111111111111111111",
                "cnf": {
                    "kty": "OKP",
                    "crv": "Ed25519",
                    "x": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                },
            })
        );
        assert!(body["cnf"].get("jwk").is_none(), "{body}");
    }
}
