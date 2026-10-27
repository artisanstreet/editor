//! Proof-of-possession: binds the JWT bearer to the `cnf` key holder.
//!
//! `sig = Ed25519(cnf_sk, "ast-relay-auth-v1" || nonce(32) || jti_utf8)`.
//! Standard Ed25519 only; the KEM/data channel it admits is NOT specified
//! here (awaits the HPKE-or-Noise decision).

/// Domain separator for the admission proof.
pub const POP_DOMAIN: &str = "ast-relay-auth-v1";

/// The signed bytes: domain || nonce || jti.
#[must_use]
pub fn pop_message(nonce: &[u8; 32], jti: &str) -> Vec<u8> {
    let domain = POP_DOMAIN.as_bytes();
    let jti = jti.as_bytes();
    let mut out = Vec::with_capacity(domain.len() + 32 + jti.len());
    out.extend_from_slice(domain);
    out.extend_from_slice(nonce);
    out.extend_from_slice(jti);
    out
}

/// Sign an admission challenge with a `cnf` secret key.
#[must_use]
pub fn pop_sign(secret: &[u8; 32], nonce: &[u8; 32], jti: &str) -> [u8; 64] {
    use ed25519_dalek::Signer as _;
    let key = ed25519_dalek::SigningKey::from_bytes(secret);
    key.sign(&pop_message(nonce, jti)).to_bytes()
}

/// Verify an admission proof against a `cnf` public key.
#[must_use]
pub fn pop_verify(public: &[u8; 32], nonce: &[u8; 32], jti: &str, signature: &[u8]) -> bool {
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(public) else {
        return false;
    };
    let Ok(signature) = ed25519_dalek::Signature::try_from(signature) else {
        return false;
    };
    key.verify_strict(&pop_message(nonce, jti), &signature)
        .is_ok()
}

/// A fresh ephemeral Ed25519 keypair (Editor PoP keys are ephemeral and
/// automatic; Forge `cnf` matches its enrolled key).
#[expect(
    clippy::map_err_ignore,
    reason = "key failures are coarse KeyError so messages never carry key material"
)]
pub fn generate_ephemeral() -> Result<[u8; 32], crate::enrollment::KeyError> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|_| crate::enrollment::KeyError)?;
    Ok(secret)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;

    #[test]
    fn pop_roundtrip_binds_nonce_and_jti() {
        let secret = [5_u8; 32];
        let public = {
            let key = ed25519_dalek::SigningKey::from_bytes(&secret);
            key.verifying_key().to_bytes()
        };
        let nonce = [6_u8; 32];
        let signature = pop_sign(&secret, &nonce, "jti-1");
        assert!(pop_verify(&public, &nonce, "jti-1", &signature));
        assert!(!pop_verify(&public, &nonce, "jti-2", &signature));
        assert!(!pop_verify(&public, &[7_u8; 32], "jti-1", &signature));
    }
}
