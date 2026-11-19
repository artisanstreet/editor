use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum MachineKeyError {
    #[error("read random bytes for a machine key")]
    Random(#[source] getrandom::Error),
    #[error("the machine key is all zeros")]
    AllZeros,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MachineKeypair {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl std::fmt::Debug for MachineKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineKeypair")
            .field("secret", &"redacted")
            .field("public", &self.public_b64u())
            .finish()
    }
}

impl MachineKeypair {
    pub fn generate() -> Result<Self, MachineKeyError> {
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret).map_err(MachineKeyError::Random)?;
        Self::from_secret(secret)
    }

    pub fn from_secret(secret: [u8; 32]) -> Result<Self, MachineKeyError> {
        if secret == [0_u8; 32] {
            return Err(MachineKeyError::AllZeros);
        }
        let key = ed25519_dalek::SigningKey::from_bytes(&secret);
        Ok(Self {
            secret: Zeroizing::new(secret),
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

    #[must_use]
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrollment_id: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[must_use]
pub fn normalise_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

#[must_use]
pub fn enrollment_request_body(public_b64u: &str, label: &str, timestamp_sec: i64) -> String {
    format!("enrollment-request\n{public_b64u}\n{label}\n{timestamp_sec}")
}

#[must_use]
pub fn enrollment_confirm_body(enrollment_id: &str, account_id: &str, public_b64u: &str) -> String {
    format!("enrollment-confirm\n{enrollment_id}\n{account_id}\n{public_b64u}")
}

#[must_use]
pub fn sign_enrollment_proof(secret: &[u8; 32], body: &str) -> [u8; 64] {
    use ed25519_dalek::Signer as _;
    let key = ed25519_dalek::SigningKey::from_bytes(secret);
    key.sign(body.as_bytes()).to_bytes()
}

#[must_use]
pub fn verify_enrollment_proof(public: &[u8; 32], body: &str, signature: &[u8]) -> bool {
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(public) else {
        return false;
    };
    let Ok(signature) = ed25519_dalek::Signature::try_from(signature) else {
        return false;
    };
    key.verify_strict(body.as_bytes(), &signature).is_ok()
}
