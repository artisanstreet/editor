//! Machine identity and account enrollment: client side only.
//!
//! The Forge keypair is local and persisted by Forge (`secrets`, same as
//! the existing Forge identity). Core owns the enrollment lifecycle
//! (codes, TTL, rate limits, claims); Rust only builds the exact proof
//! bodies Core specifies, signs them, and reads the strict DTOs back.
//! No parallel enrollment business logic lives here.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot make a key right now")]
pub struct KeyError;

/// A local Ed25519 machine keypair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineKeypair {
    secret: [u8; 32],
    public: [u8; 32],
}

impl MachineKeypair {
    #[expect(
        clippy::map_err_ignore,
        reason = "key failures are coarse KeyError so messages never carry key material"
    )]
    pub fn generate() -> Result<Self, KeyError> {
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret).map_err(|_| KeyError)?;
        Self::from_secret(secret)
    }

    pub fn from_secret(secret: [u8; 32]) -> Result<Self, KeyError> {
        if secret == [0_u8; 32] {
            return Err(KeyError);
        }
        let key = ed25519_dalek::SigningKey::from_bytes(&secret);
        Ok(Self {
            secret,
            public: key.verifying_key().to_bytes(),
        })
    }

    #[must_use]
    pub const fn public(&self) -> &[u8; 32] {
        &self.public
    }

    #[must_use]
    pub fn public_b64u(&self) -> String {
        use base64::Engine as _;
        URL_SAFE_NO_PAD.encode(self.public)
    }

    /// Base64url-no-pad SHA-256 of the public key: how operators compare
    /// machines (`keyFingerprint`, Core §2). Never log private material.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        use base64::Engine as _;
        Fingerprint(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(self.public)),
        )
    }

    #[must_use]
    pub const fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

/// The status Core returns to the authenticated holder of the enrollment
/// key (`GET …/status`): named account for explicit local approval, no
/// pasted-id friction. Strict read: unknown fields are rejected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairStatus {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Enrollment operator key fingerprint for display.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fingerprint(pub String);

/// Normalize a user-typed code the same way everywhere: keep ASCII
/// alphanumerics, uppercase (client sends the raw code; Core hashes).
#[must_use]
pub fn normalise_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Enrollment-request proof body (Core §5):
/// `enrollment-request\n<x>\n<label>\n<timestamp>` where `x` is the
/// base64url Ed25519 public key (JWK `x`), `timestamp` is unix seconds.
#[must_use]
pub fn enrollment_request_body(public_b64u: &str, label: &str, timestamp_sec: i64) -> String {
    format!("enrollment-request\n{public_b64u}\n{label}\n{timestamp_sec}")
}

/// Enrollment-confirm proof body (Core §5):
/// `enrollment-confirm\n<enrollmentId>\n<accountId>\n<x>`.
#[must_use]
pub fn enrollment_confirm_body(enrollment_id: &str, account_id: &str, public_b64u: &str) -> String {
    format!("enrollment-confirm\n{enrollment_id}\n{account_id}\n{public_b64u}")
}

/// Sign an enrollment proof body with the enrollment private key.
#[must_use]
pub fn sign_enrollment_proof(secret: &[u8; 32], body: &str) -> [u8; 64] {
    use ed25519_dalek::Signer as _;
    let key = ed25519_dalek::SigningKey::from_bytes(secret);
    key.sign(body.as_bytes()).to_bytes()
}

/// Verify an enrollment proof body against the enrollment public key.
#[must_use]
pub fn verify_enrollment_proof(
    public: &[u8; 32],
    body: &str,
    signature: &[u8],
) -> bool {
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(public) else {
        return false;
    };
    let Ok(signature) = ed25519_dalek::Signature::try_from(signature) else {
        return false;
    };
    key.verify_strict(body.as_bytes(), &signature).is_ok()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;

    #[test]
    fn proof_bodies_match_core_shape() {
        assert_eq!(
            enrollment_request_body("eA", "server-forge", 1_700_000_000),
            "enrollment-request\neA\nserver-forge\n1700000000"
        );
        assert_eq!(
            enrollment_confirm_body("enr_1", "acct_2", "eA"),
            "enrollment-confirm\nenr_1\nacct_2\neA"
        );
    }

    #[test]
    fn proofs_sign_and_verify_with_the_enrollment_key() {
        let secret = [0x21_u8; 32];
        let public = ed25519_dalek::SigningKey::from_bytes(&secret)
            .verifying_key()
            .to_bytes();
        let body = enrollment_request_body("eA", "label", 1);
        let signature = sign_enrollment_proof(&secret, &body);
        assert!(verify_enrollment_proof(&public, &body, &signature));
        assert!(!verify_enrollment_proof(&public, "other", &signature));
    }

    #[test]
    fn pair_status_reads_are_strict() {
        let raw = serde_json::json!({
            "status": "claimed",
            "account_id": "acct_1",
            "email": "a@example.invalid",
        });
        let status: PairStatus = serde_json::from_value(raw).unwrap();
        assert_eq!(status.account_id.as_deref(), Some("acct_1"));
        let raw = serde_json::json!({"status": "pending", "future": 1});
        assert!(serde_json::from_value::<PairStatus>(raw).is_err());
    }

    #[test]
    fn keypairs_reject_zero_and_fingerprint_is_b64u() {
        assert!(MachineKeypair::from_secret([0_u8; 32]).is_err());
        let keypair = MachineKeypair::from_secret([0x21_u8; 32]).unwrap();
        assert_eq!(keypair.fingerprint().0.len(), 43);
        assert_eq!(normalise_code("ab-12 cd"), "AB12CD");
    }
}
