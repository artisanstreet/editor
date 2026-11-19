use super::*;

const FIXTURE: &str = include_str!("../../fixtures/core-issued.json");

fn fixture() -> serde_json::Value {
    serde_json::from_str(FIXTURE).unwrap()
}

#[test]
fn core_issued_grant_verifies_byte_exact() {
    let fixture = fixture();
    let token = fixture["grant"].as_str().unwrap();
    let jwks_key = fixture["jwks"]["keys"][0].clone();
    let fixture_kid = jwks_key["kid"].as_str().unwrap().to_owned();
    assert_eq!(unverified_kid(token).unwrap(), fixture_kid);
    let issuer = {
        use base64::Engine as _;
        let raw = URL_SAFE_NO_PAD
            .decode(jwks_key["x"].as_str().unwrap().as_bytes())
            .unwrap();
        <[u8; 32]>::try_from(raw).unwrap()
    };
    let payload_bytes = {
        use base64::Engine as _;
        let part = token.split('.').nth(1).unwrap();
        URL_SAFE_NO_PAD.decode(part.as_bytes()).unwrap()
    };
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).unwrap();
    let fixture_iat = payload["iat"].as_i64().unwrap();
    let fixture_jti = payload["jti"].as_str().unwrap().to_owned();
    let expected = &fixture["expected"];
    let params = VerifyParams {
        expected_iss: expected["iss"].as_str().unwrap().into(),
        expected_aud: AUD_RELAY.into(),
        max_ttl_s: crate::GRANT_MAX_TTL_S,
        expected_sub: expected["sub"].as_str().unwrap().into(),
        expected_mid: expected["mid"].as_str().unwrap().into(),
        now_sec: fixture_iat,
    };
    let claims = parse_and_verify_jwt(token, &issuer, &params).unwrap();
    assert_eq!(claims.v, 1);
    assert_eq!(claims.sub, expected["sub"].as_str().unwrap());
    assert_eq!(claims.mid, expected["mid"].as_str().unwrap());
    assert_eq!(claims.role, Role::Forge);
    assert_eq!(claims.scope, vec![Scope::RelayConnect, Scope::ForgeExecute]);
    assert_eq!(claims.iat, fixture_iat);
    assert_eq!(claims.nbf, Some(fixture_iat));
    assert_eq!(claims.exp - claims.iat, 300);
    assert_eq!(claims.jti, fixture_jti);
    assert_eq!(claims.rev, None);
    let cnf = cnf_public_key(&claims).unwrap();
    assert_eq!(
        URL_SAFE_NO_PAD.encode(cnf),
        expected["cnf"]["jwk"]["jwk"]["x"].as_str().unwrap()
    );
}

#[test]
fn fixture_machine_proof_verifies_with_the_enrolled_key() {
    use base64::Engine as _;
    let fixture = fixture();
    let proof = &fixture["machineProof"];
    let headers = &proof["headers"];
    let body = proof["raw_body"].as_str().unwrap().as_bytes();
    assert_eq!(
        crate::machine_proof::body_hash_hex(body),
        crate::machine_proof::EMPTY_BODY_HASH
    );
    let token = fixture["grant"].as_str().unwrap();
    let payload = token.split('.').nth(1).unwrap();
    let payload = URL_SAFE_NO_PAD.decode(payload.as_bytes()).unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    let cnf_x = payload["cnf"]["jwk"]["x"].as_str().unwrap();
    let public = URL_SAFE_NO_PAD.decode(cnf_x.as_bytes()).unwrap();
    let public = <[u8; 32]>::try_from(public).unwrap();
    let request = crate::machine_proof::MachineProof {
        public: &public,
        body,
        timestamp_sec: headers["x-ast-timestamp"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        nonce_b64u: headers["x-ast-nonce"].as_str().unwrap(),
        method: proof["method"].as_str().unwrap(),
        path_and_query: proof["pathAndQuery"].as_str().unwrap(),
        signature_b64u: headers["authorization"]
            .as_str()
            .unwrap()
            .strip_prefix("AST-Machine ")
            .unwrap(),
        now_sec: headers["x-ast-timestamp"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
    };
    crate::machine_proof::verify_machine_proof(&request).unwrap();
}
