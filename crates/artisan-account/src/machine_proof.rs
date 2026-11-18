use base64::Engine as _;
use sha2::Digest as _;

pub const PROOF_TIMESTAMP_WINDOW_S: i64 = 300;

pub const MIN_NONCE_BYTES: usize = 12;

#[cfg(test)]
pub const EMPTY_BODY_HASH: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[must_use]
pub fn body_hash_hex(body: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(body))
}

#[must_use]
pub fn canonical_request(
    timestamp_sec: i64,
    nonce_b64u: &str,
    method: &str,
    path_and_query: &str,
    body_hash_hex_value: &str,
) -> String {
    format!(
        "{timestamp_sec}\n{nonce_b64u}\n{}\n{path_and_query}\n{body_hash_hex_value}",
        method.to_ascii_uppercase()
    )
}

pub struct MachineProof<'a> {
    pub public: &'a [u8; 32],
    pub body: &'a [u8],
    pub timestamp_sec: i64,
    pub nonce_b64u: &'a str,
    pub method: &'a str,
    pub path_and_query: &'a str,
    pub signature_b64u: &'a str,
    pub now_sec: i64,
}

#[must_use]
pub fn sign_proof(secret: &[u8; 32], canonical: &str) -> [u8; 64] {
    use ed25519_dalek::Signer as _;
    let key = ed25519_dalek::SigningKey::from_bytes(secret);
    key.sign(canonical.as_bytes()).to_bytes()
}

#[derive(Debug, thiserror::Error)]
pub enum MachineProofError {
    #[error(
        "the proof timestamp {timestamp_sec} is outside the acceptance window around {now_sec}"
    )]
    Stale { timestamp_sec: i64, now_sec: i64 },
    #[error("decode the proof nonce")]
    Nonce(#[source] base64::DecodeError),
    #[error("the proof nonce has {bytes} bytes, fewer than {MIN_NONCE_BYTES}")]
    ShortNonce { bytes: usize },
    #[error("read the machine key")]
    Key(#[source] ed25519_dalek::SignatureError),
    #[error("decode the proof signature")]
    SignatureEncoding(#[source] base64::DecodeError),
    #[error("read the proof signature")]
    SignatureShape(#[source] ed25519_dalek::SignatureError),
    #[error("the machine proof signature does not verify")]
    Signature(#[source] ed25519_dalek::SignatureError),
}

pub fn verify_machine_proof(proof: &MachineProof<'_>) -> Result<(), MachineProofError> {
    if proof.timestamp_sec.saturating_sub(proof.now_sec).abs() > PROOF_TIMESTAMP_WINDOW_S {
        return Err(MachineProofError::Stale {
            timestamp_sec: proof.timestamp_sec,
            now_sec: proof.now_sec,
        });
    }
    let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(proof.nonce_b64u.as_bytes())
        .map_err(MachineProofError::Nonce)?;
    if nonce.len() < MIN_NONCE_BYTES {
        return Err(MachineProofError::ShortNonce { bytes: nonce.len() });
    }
    let canonical = canonical_request(
        proof.timestamp_sec,
        proof.nonce_b64u,
        proof.method,
        proof.path_and_query,
        &body_hash_hex(proof.body),
    );
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(proof.public).map_err(MachineProofError::Key)?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(proof.signature_b64u.as_bytes())
        .map_err(MachineProofError::SignatureEncoding)?;
    let signature = ed25519_dalek::Signature::try_from(signature.as_slice())
        .map_err(MachineProofError::SignatureShape)?;
    key.verify_strict(canonical.as_bytes(), &signature)
        .map_err(MachineProofError::Signature)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test fixtures; failures fail the test")]
#[path = "../tests/machine_proof/unit.rs"]
mod tests;
