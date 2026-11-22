use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MagicRequest {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "deviceName")]
    pub device_name: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MagicRequestResponse {
    pub request_id: String,
    pub request_secret: String,
    pub expires_at: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MagicStatusRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "requestSecret")]
    pub request_secret: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagicStatusResponse {
    pub request_id: String,
    pub status: String,
    pub totp_required: bool,
    pub email: String,
    pub expires_at: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MagicFinalizeRequest {
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DesktopRedeemRequest {
    pub code: String,
    pub verifier: String,
}

impl std::fmt::Debug for MagicRequestResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MagicRequestResponse")
            .field("request_id", &self.request_id)
            .field("request_secret", &"redacted")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl std::fmt::Debug for MagicStatusRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MagicStatusRequest")
            .field("request_id", &self.request_id)
            .field("request_secret", &"redacted")
            .finish()
    }
}

impl std::fmt::Debug for MagicFinalizeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MagicFinalizeRequest")
            .field("request_id", &self.request_id)
            .field("request_secret", &"redacted")
            .field(
                "totp_code",
                &self.totp_code.as_ref().map(|_code| "redacted"),
            )
            .field(
                "recovery_code",
                &self.recovery_code.as_ref().map(|_code| "redacted"),
            )
            .finish()
    }
}

impl std::fmt::Debug for DesktopRedeemRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DesktopRedeemRequest(redacted)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagicFinalizeResponse {
    pub session_id: String,
    pub account_id: String,
    pub expires_at: String,
    pub totp_verified: bool,
}

#[cfg(test)]
#[path = "../tests/signin/unit.rs"]
mod tests;
