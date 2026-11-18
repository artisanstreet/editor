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
#[path = "../tests/pop/unit.rs"]
mod tests;
