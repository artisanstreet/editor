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
    let rejected = parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params());
    assert!(
        matches!(rejected, Err(GrantError::Signature(_))),
        "{rejected:?}"
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
    let rejected = parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params());
    assert!(
        matches!(rejected, Err(GrantError::Ttl { max_ttl_s: 300 })),
        "{rejected:?}"
    );
}

#[test]
fn wrong_audience_and_binding_are_rejected() {
    let key = signing_key();
    let mut wrong_aud = claims();
    wrong_aud.aud = Aud::One("ast-core".into());
    let token = compact(&wrong_aud, &key);
    let rejected = parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params());
    assert!(
        matches!(rejected, Err(GrantError::Audience)),
        "{rejected:?}"
    );
    let mut wrong_mid = claims();
    wrong_mid.mid = "mch_22222222222222222222222222".into();
    let token = compact(&wrong_mid, &key);
    let rejected = parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params());
    assert!(matches!(rejected, Err(GrantError::Binding)), "{rejected:?}");
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

fn signed(header: &serde_json::Value, payload: &[u8], key: &ed25519_dalek::SigningKey) -> String {
    use ed25519_dalek::Signer as _;
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(payload)
    );
    let signature = key.sign(input.as_bytes());
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

fn header() -> serde_json::Value {
    serde_json::json!({"alg": "EdDSA", "kid": "test-key-1", "typ": "JWT"})
}

#[test]
fn signed_claims_that_no_longer_match_the_grant_say_so_with_their_cause() {
    let key = signing_key();
    let mut drifted = serde_json::to_value(claims()).unwrap();
    drifted["sub"] = serde_json::json!(42);
    let token = signed(&header(), drifted.to_string().as_bytes(), &key);

    let rejected = parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params());

    let Err(failure @ GrantError::Claims(UnverifiedGrantError::Json { index: 1, .. })) = rejected
    else {
        panic!("drifted claims were classified as {rejected:?}");
    };
    assert!(
        std::error::Error::source(&failure)
            .and_then(std::error::Error::source)
            .is_some_and(<dyn std::error::Error>::is::<serde_json::Error>),
        "the JSON cause was dropped"
    );
}

#[test]
fn a_signed_payload_that_isnt_base64_is_a_claims_decoding_failure_not_a_bad_id() {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let key = signing_key();
    let input = format!(
        "{}.not*base64",
        URL_SAFE_NO_PAD.encode(header().to_string())
    );
    let signature = URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes()).to_bytes());
    let token = format!("{input}.{signature}");

    let rejected = parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params());

    assert!(
        matches!(
            rejected,
            Err(GrantError::Claims(UnverifiedGrantError::Base64 {
                index: 1,
                ..
            }))
        ),
        "{rejected:?}"
    );
}

#[test]
fn a_header_that_isnt_eddsa_and_a_short_signature_are_told_apart() {
    let key = signing_key();
    let not_eddsa = signed(
        &serde_json::json!({"alg": "HS256", "kid": "test-key-1"}),
        serde_json::to_string(&claims()).unwrap().as_bytes(),
        &key,
    );
    let token = compact(&claims(), &key);
    let (unsigned, _signature) = token.rsplit_once('.').unwrap();
    let short = format!("{unsigned}.{}", URL_SAFE_NO_PAD.encode([1_u8; 10]));

    let not_eddsa = parse_and_verify_jwt(&not_eddsa, key.verifying_key().as_bytes(), &params());
    let short = parse_and_verify_jwt(&short, key.verifying_key().as_bytes(), &params());

    assert!(
        matches!(
            not_eddsa,
            Err(GrantError::Header(UnverifiedGrantError::NotEdDsa { .. }))
        ),
        "{not_eddsa:?}"
    );
    assert!(
        matches!(short, Err(GrantError::SignatureLength { length: 10 })),
        "{short:?}"
    );
}

#[test]
fn a_rejected_grant_never_repeats_its_token_in_what_is_logged() {
    let key = signing_key();
    let mut drifted = serde_json::to_value(claims()).unwrap();
    drifted["mid"] = serde_json::json!("mch_SENTINEL");
    let token = signed(&header(), drifted.to_string().as_bytes(), &key);

    let rejected = parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params())
        .expect_err("another machine's grant is refused");
    let logged = format!("{rejected} {rejected:?}");

    assert!(!logged.contains("SENTINEL"), "{logged}");
    assert!(!logged.contains(&token), "{logged}");
}
