use artisan_account::{
    MachineKeyError, MachineKeypair, PairStatus, enrollment_confirm_body, enrollment_request_body,
    normalise_code, sign_enrollment_proof, verify_enrollment_proof,
};

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
    let status: PairStatus = serde_json::from_value(raw).expect("a claimed status");
    assert_eq!(status.account_id.as_deref(), Some("acct_1"));
    let raw = serde_json::json!({"status": "pending", "future": 1});
    serde_json::from_value::<PairStatus>(raw).expect_err("unknown fields are refused");
}

#[test]
fn keypairs_reject_zero_and_codes_normalise() {
    assert!(matches!(
        MachineKeypair::from_secret([0_u8; 32]),
        Err(MachineKeyError::AllZeros)
    ));
    assert_eq!(normalise_code("ab-12 cd"), "AB12CD");
}
