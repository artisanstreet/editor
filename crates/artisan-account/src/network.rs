use serde::{Deserialize, Serialize};

use crate::signin::{
    DesktopRedeemRequest, MagicFinalizeRequest, MagicFinalizeResponse, MagicRequest,
    MagicRequestResponse, MagicStatusRequest, MagicStatusResponse,
};

#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(artisan_api::Secret);

impl SessionToken {
    #[must_use]
    pub fn new(token: String) -> Self {
        Self(artisan_api::Secret::new(token))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.expose()
    }
}

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionToken(redacted)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    #[error("build the Core HTTP client")]
    Client(#[source] reqwest::Error),
    #[error("{action}: could not reach Core")]
    Transport {
        action: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("{action}: Core answered {status} without its error body")]
    UnexpectedStatus { action: &'static str, status: u16 },
    #[error("{action}: read Core's answer")]
    Decode {
        action: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("{action}: Core opened no session")]
    NoSession { action: &'static str },
    #[error("{action}: the request stopped before Core answered")]
    Stopped {
        action: &'static str,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("{action}: Core refused the call: {code} (request {request_id})")]
    Core {
        action: &'static str,
        code: String,
        message: String,
        request_id: String,
    },
}

impl NetworkError {
    #[must_use]
    pub const fn is_unreachable(&self) -> bool {
        !matches!(self, Self::Core { .. })
    }

    #[must_use]
    pub fn core_code(&self) -> Option<&str> {
        match self {
            Self::Core { code, .. } => Some(code),
            Self::Client(_)
            | Self::Transport { .. }
            | Self::UnexpectedStatus { .. }
            | Self::Decode { .. }
            | Self::NoSession { .. }
            | Self::Stopped { .. } => None,
        }
    }
}

/// Core error envelope (strict read).
#[derive(Clone, Debug, Deserialize)]
struct CoreErrorBody {
    error: CoreErrorDetail,
}

#[derive(Clone, Debug, Deserialize)]
struct CoreErrorDetail {
    code: String,
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

/// The verified profile behind `GET /v1/me`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountProfile {
    pub account_id: String,
    pub email: String,
    pub display_name: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub totp_enrolled: bool,
    #[serde(default)]
    pub machines: Vec<MachineEntry>,
    #[serde(default)]
    pub latest_config_version: Option<u64>,
}

#[derive(Deserialize)]
struct AccountDetails {
    username: Option<String>,
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

#[derive(Clone, Debug, Deserialize)]
pub struct GrantToken {
    pub token: String,
    pub expires_at: String,
}

/// Session-authed Core HTTP. One client per base URL; cheap to clone.
#[derive(Clone, Debug)]
pub struct CoreSessionClient {
    http: reqwest::Client,
    base_url: String,
}

impl CoreSessionClient {
    pub fn new(base_url: &str) -> Result<Self, NetworkError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(NetworkError::Client)?;
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
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|source| NetworkError::Decode { action, source })?;
        match serde_json::from_slice::<CoreErrorBody>(&body) {
            Ok(parsed) => Err(NetworkError::Core {
                action,
                code: parsed.error.code,
                message: parsed.error.message,
                request_id: parsed.error.request_id,
            }),
            Err(_not_core) => Err(NetworkError::UnexpectedStatus { action, status }),
        }
    }

    async fn send(
        request: reqwest::RequestBuilder,
        action: &'static str,
    ) -> Result<reqwest::Response, NetworkError> {
        let response = request
            .send()
            .await
            .map_err(|source| NetworkError::Transport { action, source })?;
        Self::check(response, action).await
    }

    async fn read<R: for<'de> Deserialize<'de>>(
        response: reqwest::Response,
        action: &'static str,
    ) -> Result<R, NetworkError> {
        response
            .json()
            .await
            .map_err(|source| NetworkError::Decode { action, source })
    }

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
        Self::read(Self::send(request, action).await?, action).await
    }

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
        Self::read(Self::send(request, action).await?, action).await
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

    pub async fn magic_finalize(
        &self,
        request_id: &str,
        request_secret: &str,
        totp_code: Option<&str>,
        recovery_code: Option<&str>,
    ) -> Result<(MagicFinalizeResponse, SessionToken), NetworkError> {
        let action = "finish signing in";
        let request =
            self.http
                .post(self.url("/v1/auth/magic/finalize"))
                .json(&MagicFinalizeRequest {
                    request_id: request_id.to_owned(),
                    request_secret: request_secret.to_owned(),
                    totp_code: totp_code.map(str::to_owned),
                    recovery_code: recovery_code.map(str::to_owned),
                });
        Self::opened_session(Self::send(request, action).await?, action).await
    }

    pub async fn desktop_redeem(
        &self,
        code: &str,
        verifier: &str,
    ) -> Result<(MagicFinalizeResponse, SessionToken), NetworkError> {
        let action = "redeem the desktop sign-in";
        let request =
            self.http
                .post(self.url("/v1/auth/desktop/redeem"))
                .json(&DesktopRedeemRequest {
                    code: code.to_owned(),
                    verifier: verifier.to_owned(),
                });
        Self::opened_session(Self::send(request, action).await?, action).await
    }

    async fn opened_session(
        response: reqwest::Response,
        action: &'static str,
    ) -> Result<(MagicFinalizeResponse, SessionToken), NetworkError> {
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
            .ok_or(NetworkError::NoSession { action })?;
        let finalized: MagicFinalizeResponse = Self::read(response, action).await?;
        Ok((finalized, SessionToken::new(token)))
    }

    pub async fn me(&self, session: &SessionToken) -> Result<AccountProfile, NetworkError> {
        let mut profile: AccountProfile = self
            .get("/v1/me", Some(session), "load the account")
            .await?;
        if let Ok(details) = self
            .get::<AccountDetails>("/v1/account/profile", Some(session), "load the profile")
            .await
        {
            profile.username = details.username;
        }
        Ok(profile)
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
}

/// `POST /v1/machines/enrollments/claim` answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedEnrollment {
    pub enrollment_id: String,
}
