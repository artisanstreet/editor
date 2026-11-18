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
    let tampered = MachineProof {
        body: br#"{"a":2}"#,
        ..proof
    };
    assert!(matches!(
        verify_machine_proof(&tampered),
        Err(MachineProofError::Signature(_))
    ));
    let expired = MachineProof {
        now_sec: 1_700_000_100 + PROOF_TIMESTAMP_WINDOW_S + 1,
        ..proof
    };
    assert!(matches!(
        verify_machine_proof(&expired),
        Err(MachineProofError::Stale { .. })
    ));
    let short = MachineProof {
        nonce_b64u: &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([1_u8; 4]),
        ..proof
    };
    assert!(matches!(
        verify_machine_proof(&short),
        Err(MachineProofError::ShortNonce { bytes: 4 })
    ));
}
