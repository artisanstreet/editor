const POP_DOMAIN: &str = "ast-relay-auth-v1";

fn pop_message(nonce: &[u8; 32], jti: &str) -> Vec<u8> {
    [POP_DOMAIN.as_bytes(), nonce, jti.as_bytes()].concat()
}

#[must_use]
pub fn pop_sign(secret: &[u8; 32], nonce: &[u8; 32], jti: &str) -> [u8; 64] {
    use ed25519_dalek::Signer as _;
    ed25519_dalek::SigningKey::from_bytes(secret)
        .sign(&pop_message(nonce, jti))
        .to_bytes()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test fixtures; failures fail the test")]
mod tests {
    use super::*;

    #[test]
    fn pop_signature_binds_nonce_and_jti() {
        let secret = [5_u8; 32];
        let public = ed25519_dalek::SigningKey::from_bytes(&secret).verifying_key();
        let nonce = [6_u8; 32];
        let signature = ed25519_dalek::Signature::from_bytes(&pop_sign(&secret, &nonce, "jti-1"));
        assert!(public.verify_strict(&pop_message(&nonce, "jti-1"), &signature).is_ok(), "own proof");
        assert!(public.verify_strict(&pop_message(&nonce, "jti-2"), &signature).is_err(), "other jti");
        assert!(public.verify_strict(&pop_message(&[7_u8; 32], "jti-1"), &signature).is_err(), "other nonce");
    }
}
