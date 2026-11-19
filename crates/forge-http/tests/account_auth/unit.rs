use super::*;

pub(crate) async fn verifier_with_stub(
    jwks: serde_json::Value,
) -> (GrantVerifier, test_support::http::StubServer) {
    let server = test_support::http::StubServer::json("/.well-known/jwks.json", jwks).await;
    let verifier = GrantVerifier::new(
        &format!("{}/.well-known/jwks.json", server.url()),
        "https://core.example.invalid",
    )
    .unwrap();
    (verifier, server)
}

fn issuer() -> (ed25519_dalek::SigningKey, String) {
    use base64::Engine as _;
    let key = ed25519_dalek::SigningKey::from_bytes(&[0x77_u8; 32]);
    let x = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes());
    (key, x)
}

fn grant_token(key: &ed25519_dalek::SigningKey, sub: &str, mid: &str, scope: &[Scope]) -> String {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let cnf = ed25519_dalek::SigningKey::from_bytes(&[0x33_u8; 32]);
    let claims = GrantClaims {
        v: 1,
        iss: "https://core.example.invalid".into(),
        aud: artisan_account::Aud::One(artisan_account::AUD_RELAY.into()),
        sub: sub.into(),
        mid: mid.into(),
        role: artisan_account::Role::Client,
        scope: scope.to_vec(),
        cnf: artisan_account::CnfHolder {
            jwk: artisan_account::CnfJwk {
                kty: "OKP".into(),
                crv: "Ed25519".into(),
                x: base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(cnf.verifying_key().to_bytes()),
            },
        },
        rev: None,
        iat: 1_700_000_000,
        nbf: None,
        exp: 1_700_000_200,
        jti: "tkn-test-0001".into(),
    };
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::json!({"alg": "EdDSA", "kid": "k1", "typ": "JWT"}).to_string());
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_string(&claims).unwrap());
    let input = format!("{header}.{payload}");
    let signature = key.sign(input.as_bytes());
    format!(
        "{input}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

#[tokio::test]
async fn tunnel_grants_verify_against_core_jwks() {
    let (issuer_key, x) = issuer();
    let (verifier, _jwks) = verifier_with_stub(serde_json::json!({
        "keys": [{"kty": "OKP", "crv": "Ed25519", "x": x, "kid": "k1",
                  "use": "sig", "alg": "EdDSA"}],
    }))
    .await;
    let sub = "acct_00000000000000000000000000";
    let mid = "mch_11111111111111111111111111";
    let token = grant_token(
        &issuer_key,
        sub,
        mid,
        &[Scope::RelayConnect, Scope::ForgeExecute],
    );
    let claims = verifier
        .verify_channel_grant(&token, sub, mid, 1_700_000_100, true)
        .await
        .unwrap();
    assert_eq!(claims.sub, sub);
    let routed = grant_token(&issuer_key, sub, mid, &[Scope::RelayConnect]);
    assert!(
        verifier
            .verify_channel_grant(&routed, sub, mid, 1_700_000_100, true)
            .await
            .is_err()
    );
    assert!(
        verifier
            .verify_channel_grant(
                &token,
                sub,
                "mch_22222222222222222222222222",
                1_700_000_100,
                true
            )
            .await
            .is_err()
    );
}
