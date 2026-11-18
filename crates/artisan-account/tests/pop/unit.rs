use super::*;

#[test]
fn pop_signature_binds_nonce_and_jti() {
    let secret = [5_u8; 32];
    let public = ed25519_dalek::SigningKey::from_bytes(&secret).verifying_key();
    let nonce = [6_u8; 32];
    let signature = ed25519_dalek::Signature::from_bytes(&pop_sign(&secret, &nonce, "jti-1"));
    assert!(
        public
            .verify_strict(&pop_message(&nonce, "jti-1"), &signature)
            .is_ok(),
        "own proof"
    );
    assert!(
        public
            .verify_strict(&pop_message(&nonce, "jti-2"), &signature)
            .is_err(),
        "other jti"
    );
    assert!(
        public
            .verify_strict(&pop_message(&[7_u8; 32], "jti-1"), &signature)
            .is_err(),
        "other nonce"
    );
}
