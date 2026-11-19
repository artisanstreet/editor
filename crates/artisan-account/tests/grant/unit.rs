use super::*;

fn signing_key() -> ed25519_dalek::SigningKey {
    let mut secret = [7_u8; 32];
    secret[0] = 0x42;
    ed25519_dalek::SigningKey::from_bytes(&secret)
}

fn claims() -> GrantClaims {
    GrantClaims {
        v: 1,
        iss: "https://core.example.invalid".into(),
        aud: Aud::One(AUD_RELAY.into()),
        sub: "acct_00000000000000000000000000".into(),
        mid: "mch_11111111111111111111111111".into(),
        role: Role::Forge,
        scope: vec![Scope::RelayConnect, Scope::ForgeExecute],
        cnf: CnfHolder {
            jwk: CnfJwk {
                kty: "OKP".into(),
                crv: "Ed25519".into(),
                x: URL_SAFE_NO_PAD.encode([9_u8; 32]),
            },
        },
        rev: None,
        iat: 1_700_000_000,
        nbf: None,
        exp: 1_700_000_200,
        jti: "01jti-test-0001".into(),
    }
}

fn params() -> VerifyParams {
    VerifyParams {
        expected_iss: "https://core.example.invalid".into(),
        expected_aud: AUD_RELAY.into(),
        max_ttl_s: crate::GRANT_MAX_TTL_S,
        expected_sub: "acct_00000000000000000000000000".into(),
        expected_mid: "mch_11111111111111111111111111".into(),
        now_sec: 1_700_000_100,
    }
}

fn compact(claims: &GrantClaims, key: &ed25519_dalek::SigningKey) -> String {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let header = URL_SAFE_NO_PAD
        .encode(serde_json::json!({"alg": "EdDSA", "kid": "test-key-1", "typ": "JWT"}).to_string());
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_string(claims).unwrap());
    let input = format!("{header}.{payload}");
    let signature = key.sign(input.as_bytes());
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

#[test]
fn a_valid_grant_verifies() {
    let key = signing_key();
    let token = compact(&claims(), &key);
    let out = parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params()).unwrap();
    assert_eq!(out.sub, "acct_00000000000000000000000000");
    assert_eq!(out.mid, "mch_11111111111111111111111111");
    assert_eq!(out.rev, None);
}

#[test]
fn tampered_payload_fails_the_signature() {
    let key = signing_key();
    let token = compact(&claims(), &key);
    let mut token = token;
    token.pop();
    token.push('A');
    assert_eq!(
        parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params())
            .unwrap_err()
            .kind,
        GrantErrorKind::Signature
    );
}

#[test]
fn strict_reads_reject_unknown_top_level_claims() {
    let mut value = serde_json::to_value(claims()).unwrap();
    value["unknown_future"] = serde_json::json!("nope");
    serde_json::from_value::<GrantClaims>(value)
        .expect_err("an unknown top-level claim is refused");
}

#[test]
fn ttl_over_300s_is_rejected() {
    let key = signing_key();
    let mut claims = claims();
    claims.exp = claims.iat + 301;
    let token = compact(&claims, &key);
    assert_eq!(
        parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params())
            .unwrap_err()
            .kind,
        GrantErrorKind::Ttl
    );
}

#[test]
fn wrong_audience_and_binding_are_rejected() {
    let key = signing_key();
    let mut wrong_aud = claims();
    wrong_aud.aud = Aud::One("ast-core".into());
    let token = compact(&wrong_aud, &key);
    assert_eq!(
        parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params())
            .unwrap_err()
            .kind,
        GrantErrorKind::Audience
    );
    let mut wrong_mid = claims();
    wrong_mid.mid = "mch_22222222222222222222222222".into();
    let token = compact(&wrong_mid, &key);
    assert_eq!(
        parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params())
            .unwrap_err()
            .kind,
        GrantErrorKind::Binding
    );
}

#[test]
fn legacy_relay_scopes_do_not_parse() {
    for legacy in [
        "relay:route",
        "forge:exec",
        "forge:sync",
        "relay:admin",
        "relay:pair",
    ] {
        let mut value = serde_json::to_value(claims()).unwrap();
        value["scope"] = serde_json::json!([legacy]);
        let Err(_) = serde_json::from_value::<GrantClaims>(value) else {
            panic!("{legacy} must not parse");
        };
    }
}

#[test]
fn id_charset_matches_relay_rules() {
    assert!(is_valid_id("a"));
    assert!(is_valid_id("forge-01.x_y~z"));
    assert!(!is_valid_id(""));
    assert!(!is_valid_id("-bad"));
    assert!(!is_valid_id("has space"));
    assert!(!is_valid_id(&"a".repeat(129)));
}

#[test]
fn core_ids_validate_prefix_and_crockford() {
    assert!(is_account_id("acct_00000000000000000000000000"));
    assert!(is_machine_id("mch_00000000000000000000000000"));
    assert!(!is_account_id("mch_00000000000000000000000000"));
    assert!(!is_machine_id("alice"));
    assert!(!is_account_id("acct_IIIIIIIIIIIIIIIIIIIIIIIIII"));
    assert!(!is_account_id("acct_0000000000000000000000000"));
    assert!(!is_account_id("acct_000000000000000000000000000"));
}

#[test]
fn kid_selects_the_verifying_key() {
    let key = signing_key();
    let token = compact(&claims(), &key);
    assert_eq!(unverified_kid(&token).unwrap(), "test-key-1");
    unverified_kid("bad.token").expect_err("a token without a readable header has no kid");
}

#[test]
fn array_aud_is_tolerated_but_core_emits_singular() {
    let key = signing_key();
    let mut tolerated = claims();
    tolerated.aud = Aud::Many(vec!["other".into(), AUD_RELAY.into()]);
    let token = compact(&tolerated, &key);
    parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params()).unwrap();
}
