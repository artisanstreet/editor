//! Core magic-link + session DTOs: strict client shapes mirroring
//! `core/src/contracts/v1.ts` (requests deny unknown fields; responses
//! read forward-compatibly).
//!
//! The lifecycle lives in Core (server authority). Rust never replicates
//! it: this module holds the request/response shapes the Editor/CLI sends
//! and reads. Behavior rules (link GET never consumes; the emailed token
//! alone can never mint a session — the `requestSecret` returned once to
//! the originating session is required; cross-browser flows need explicit
//! original-session approval; email alone never bypasses TOTP) are
//! enforced by Core; the client only surfaces them.

use serde::{Deserialize, Serialize};

/// `POST /v1/auth/magic/request`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MagicRequest {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "deviceName")]
    pub device_name: Option<String>,
}

/// `POST /v1/auth/magic/request` → link sent; the token is emailed, the
/// secret returns here once (the email token alone can never mint).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MagicRequestResponse {
    pub request_id: String,
    pub request_secret: String,
    pub expires_at: String,
}

/// `POST /v1/auth/magic/status` — poll the login request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MagicStatusRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "requestSecret")]
    pub request_secret: String,
}

/// `200` status read (forward-compatible: unknown fields ignored).
/// NOTE (Vilde owns this file): Core also returns required `email` and
/// `expires_at` here — added to match the Core OpenAPI exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagicStatusResponse {
    pub request_id: String,
    pub status: String,
    pub totp_required: bool,
    pub email: String,
    pub expires_at: String,
}

/// `POST /v1/auth/magic/consume` — approve from the emailed link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MagicConsumeRequest {
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "requestSecret")]
    pub request_secret: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagicConsumeResponse {
    pub request_id: String,
    pub status: String,
    pub totp_required: bool,
}

/// `POST /v1/auth/magic/finalize` — complete sign-in (originating session).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MagicFinalizeRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "requestSecret")]
    pub request_secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "totpCode")]
    pub totp_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "recoveryCode")]
    pub recovery_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagicFinalizeResponse {
    pub session_id: String,
    pub account_id: String,
    pub expires_at: String,
    pub totp_verified: bool,
}

/// `GET /v1/sessions` entry (forward-compatible read).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(default)]
    pub device_name: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    #[serde(default)]
    pub revoked_at: Option<String>,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;

    #[test]
    fn requests_reject_unknown_fields() {
        let raw = serde_json::json!({"email": "a@example.invalid", "future": 1});
        assert!(serde_json::from_value::<MagicRequest>(raw).is_err());
        let raw = serde_json::json!({"requestId": "logreq_1", "requestSecret": "s"});
        assert!(serde_json::from_value::<MagicStatusRequest>(raw).is_ok());
        let raw = serde_json::json!({"requestId": "logreq_1", "requestSecret": "s", "extra": 1});
        assert!(serde_json::from_value::<MagicStatusRequest>(raw).is_err());
    }

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
}
