//! Machine request proof: Forge → Core (Core `contracts.md` §3, normative).
//!
//! Every machine call that has no grant JWT proves with Ed25519 over a
//! canonical string (UTF-8, `\n` separators, no trailing newline):
//! `{timestamp}\n{nonce}\n{METHOD}\n{pathAndQuery}\n{bodyHashHex}`.
//! Headers: `X-AST-Machine`, `X-AST-Timestamp`, `X-AST-Nonce`,
//! `Authorization: AST-Machine <base64url signature>`. Server checks id,
//! `|now - timestamp| <= 300`s, durable nonce dedup (unique
//! `(subject, nonce)` insert), body-hash compare, Ed25519 verify.

use base64::Engine as _;
use sha2::Digest as _;

/// Allowed clock skew for machine proofs (seconds, Core §3).
pub const PROOF_TIMESTAMP_WINDOW_S: i64 = 300;

/// Minimum raw nonce bytes (Core §3: ≥12 random bytes).
pub const MIN_NONCE_BYTES: usize = 12;

/// Hex SHA-256 of the empty body.
pub const EMPTY_BODY_HASH: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MachineProofError {
    #[error("proof timestamp outside the acceptance window")]
    Time,
    #[error("nonce too short")]
    Nonce,
    #[error("bad signature")]
    Signature,
    #[error("bad encoding")]
    Encoding,
}

/// Hex lowercase SHA-256 of raw request body bytes.
#[must_use]
pub fn body_hash_hex(body: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(body))
}

/// The canonical string over UTF-8 `\n` separators, no trailing newline.
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

/// A machine-proof verification request (Core §3).
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

/// Sign the canonical request with the machine/enrollment private key.
#[must_use]
pub fn sign_proof(secret: &[u8; 32], canonical: &str) -> [u8; 64] {
    use ed25519_dalek::Signer as _;
    let key = ed25519_dalek::SigningKey::from_bytes(secret);
    key.sign(canonical.as_bytes()).to_bytes()
}

/// Verify a machine proof: timestamp window, nonce length, body hash
/// (constant-time), Ed25519 signature. Nonce dedup is server-side state.
///
/// Takes a [`MachineProof`] request (8 fields would exceed the argument
/// limit).
#[expect(
    clippy::map_err_ignore,
    reason = "proof errors are coarse so messages never carry key material"
)]
pub fn verify_machine_proof(proof: &MachineProof<'_>) -> Result<(), MachineProofError> {
    if proof
        .timestamp_sec
        .saturating_sub(proof.now_sec)
        .abs()
        > PROOF_TIMESTAMP_WINDOW_S
    {
        return Err(MachineProofError::Time);
    }
    let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(proof.nonce_b64u.as_bytes())
        .map_err(|_| MachineProofError::Encoding)?;
    if nonce.len() < MIN_NONCE_BYTES {
        return Err(MachineProofError::Nonce);
    }
    // The body hash is recomputed from the received body and covered by the
    // signature: a tampered body fails verification. No separate transmitted
    // hash exists to compare against server-side.
    let canonical = canonical_request(
        proof.timestamp_sec,
        proof.nonce_b64u,
        proof.method,
        proof.path_and_query,
        &body_hash_hex(proof.body),
    );
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(proof.public).map_err(|_| MachineProofError::Signature)?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(proof.signature_b64u.as_bytes())
        .map_err(|_| MachineProofError::Encoding)?;
    let signature =
        ed25519_dalek::Signature::try_from(signature.as_slice()).map_err(|_| MachineProofError::Signature)?;
    key.verify_strict(canonical.as_bytes(), &signature)
        .map_err(|_| MachineProofError::Signature)?;
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;

    fn keys() -> ([u8; 32], [u8; 32]) {
        let secret = [0x41_u8; 32];
        let public = ed25519_dalek::SigningKey::from_bytes(&secret)
            .verifying_key()
            .to_bytes();
        (secret, public)
    }

    #[test]
    fn canonical_string_matches_core_shape() {
        assert_eq!(
            canonical_request(1_700_000_000, "bm9uY2U", "post", "/v1/x?a=1", "abc"),
            "1700000000\nbm9uY2U\nPOST\n/v1/x?a=1\nabc"
        );
        assert_eq!(body_hash_hex(b""), EMPTY_BODY_HASH);
    }

    #[test]
    fn sign_verify_roundtrip_and_tamper_fails() {
        use base64::Engine as _;
        let (secret, public) = keys();
        let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 16]);
        let body = br#"{"a":1}"#;
        let canonical = canonical_request(1_700_000_100, &nonce, "POST", "/v1/m", &body_hash_hex(body));
        let signature = sign_proof(&secret, &canonical);
        let signature_b64u = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature);
        let proof = MachineProof {
            public: &public,
            body,
            timestamp_sec: 1_700_000_100,
            nonce_b64u: &nonce,
            method: "post",
            path_and_query: "/v1/m",
            signature_b64u: &signature_b64u,
            now_sec: 1_700_000_100,
        };
        verify_machine_proof(&proof).unwrap();
        // Tampered body fails.
        let tampered = MachineProof { body: br#"{"a":2}"#, ..proof };
        assert_eq!(
            verify_machine_proof(&tampered).unwrap_err(),
            MachineProofError::Signature
        );
        // Expired timestamp fails.
        let expired = MachineProof { now_sec: 1_700_000_100 + PROOF_TIMESTAMP_WINDOW_S + 1, ..proof };
        assert_eq!(
            verify_machine_proof(&expired).unwrap_err(),
            MachineProofError::Time
        );
        // Short nonce fails.
        let short = MachineProof {
            nonce_b64u: &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([1_u8; 4]),
            ..proof
        };
        // Re-sign would be needed for a new nonce; reuse shows nonce checked first.
        assert_eq!(
            verify_machine_proof(&short).unwrap_err(),
            MachineProofError::Nonce
        );
    }
}
