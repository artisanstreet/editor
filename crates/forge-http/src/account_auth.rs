//! Independent Core-grant verification for tunneled execution.
//!
//! Relay admission checks a grant envelope; Forge re-verifies the in-confirm
//! grant itself before executing anything (Core §4, independent of Core
//! availability: JWKS is cached). The initiator grant travels INSIDE the
//! encrypted confirm; the Forge takes ONLY the verified `cnf` key for the
//! binding check. Stored verified claims must also carry `forge:execute`
//! before anything runs.

use anyhow::{Context, Result};
use artisan_account::{GrantClaims, Scope, VerifyParams, is_account_id, is_machine_id};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a fetched JWKS is trusted (Core §4: 10 min).
pub const JWKS_TTL: Duration = Duration::from_mins(10);

#[derive(Clone, Debug)]
struct CachedJwks {
    at: Instant,
    keys: HashMap<String, [u8; 32]>,
}

/// Verifies inner tunnel grants against Core's JWKS, independent of Relay.
pub struct GrantVerifier {
    http: reqwest::Client,
    jwks_url: String,
    expected_iss: String,
    cache: tokio::sync::Mutex<Option<CachedJwks>>,
}

impl GrantVerifier {
    pub fn new(jwks_url: &str, expected_iss: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .context("build the JWKS HTTP client")?,
            jwks_url: jwks_url.to_owned(),
            expected_iss: expected_iss.to_owned(),
            cache: tokio::sync::Mutex::new(None),
        })
    }

    async fn jwks(&self, force: bool) -> Result<HashMap<String, [u8; 32]>> {
        {
            let cache = self.cache.lock().await;
            if !force
                && let Some(cached) = cache.as_ref()
                && cached.at.elapsed() < JWKS_TTL
            {
                return Ok(cached.keys.clone());
            }
        }
        let keys = Self::fetch(&self.http, &self.jwks_url).await?;
        *self.cache.lock().await = Some(CachedJwks {
            at: Instant::now(),
            keys: keys.clone(),
        });
        Ok(keys)
    }

    async fn fetch(http: &reqwest::Client, url: &str) -> Result<HashMap<String, [u8; 32]>> {
        use base64::Engine as _;
        let jwks: crate::core::Jwks = http
            .get(url)
            .send()
            .await
            .context("fetch Core JWKS")?
            .error_for_status()
            .context("Core JWKS answered an error")?
            .json()
            .await
            .context("read Core JWKS")?;
        let mut keys = HashMap::new();
        for key in jwks.keys {
            if key.kty != "OKP" || key.crv != "Ed25519" {
                continue;
            }
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(key.x.as_bytes())
                .context("read a JWKS key")?;
            if raw.len() != 32 {
                continue;
            }
            let mut public = [0_u8; 32];
            public.copy_from_slice(&raw);
            keys.insert(key.kid, public);
        }
        Ok(keys)
    }

    /// Verify an in-confirm channel grant: issuer JWKS (`kid` selection,
    /// one refetch on unknown `kid`), Core §4 claims, then route binding.
    /// Requires role `client` and `relay:connect`; `forge:execute` is
    /// enforced separately at dispatch time unless `require_execute` is
    /// set. Returns the claims; the caller checks the binding signature
    /// with the verified `cnf` key.
    pub async fn verify_channel_grant(
        &self,
        token: &str,
        account: &str,
        machine: &str,
        now_sec: i64,
        require_execute: bool,
    ) -> Result<GrantClaims> {
        if !is_account_id(account) || !is_machine_id(machine) {
            anyhow::bail!("grant is not bound to a Core account/machine route");
        }
        let kid = artisan_account::unverified_kid(token)
            .map_err(|_header| anyhow::anyhow!("grant rejected: bad header"))?;
        let mut keys = self.jwks(false).await?;
        if !keys.contains_key(&kid) {
            keys = self.jwks(true).await?;
        }
        let issuer = keys
            .get(&kid)
            .copied()
            .with_context(|| "unknown grant key id".to_owned())?;
        let params = VerifyParams {
            expected_iss: self.expected_iss.clone(),
            expected_aud: artisan_account::AUD_RELAY.to_owned(),
            max_ttl_s: artisan_account::GRANT_MAX_TTL_S,
            expected_sub: account.to_owned(),
            expected_mid: machine.to_owned(),
            now_sec,
        };
        let claims = artisan_account::parse_and_verify_jwt(token, &issuer, &params)
            .map_err(|error| anyhow::anyhow!("grant rejected: {:?}", error.kind))?;
        if claims.role != artisan_account::Role::Client {
            anyhow::bail!("grant is not a client execution grant");
        }
        if !claims.scope.contains(&Scope::RelayConnect) {
            anyhow::bail!("grant lacks the relay:connect scope");
        }
        if require_execute && !claims.scope.contains(&Scope::ForgeExecute) {
            anyhow::bail!("grant lacks the forge:execute scope");
        }
        Ok(claims)
    }
}

/// The error kind of a rejected grant (no token bytes in messages).
pub use artisan_account::GrantErrorKind as RejectionKind;

#[allow(
    clippy::missing_errors_doc,
    reason = "test-only helpers; failures fail the test"
)]
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use axum::routing::get;

    async fn jwks_stub(
        axum::extract::State(keys): axum::extract::State<serde_json::Value>,
    ) -> impl axum::response::IntoResponse {
        axum::Json(keys)
    }

    pub async fn verifier_with_stub(jwks: serde_json::Value) -> (GrantVerifier, String) {
        let router = axum::Router::new()
            .route("/.well-known/jwks.json", get(jwks_stub))
            .with_state(jwks);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let base = format!("http://{address}");
        let verifier = GrantVerifier::new(
            &format!("{base}/.well-known/jwks.json"),
            "https://core.example.invalid",
        )
        .unwrap();
        (verifier, base)
    }

    fn issuer() -> (ed25519_dalek::SigningKey, String) {
        use base64::Engine as _;
        let key = ed25519_dalek::SigningKey::from_bytes(&[0x77_u8; 32]);
        let x = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(key.verifying_key().to_bytes());
        (key, x)
    }

    pub fn grant_token(
        key: &ed25519_dalek::SigningKey,
        sub: &str,
        mid: &str,
        scope: &[Scope],
    ) -> String {
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

    /// Independent verification: unknown kid refetches, wrong scope and
    /// wrong binding are refused without trusting Relay.
    #[tokio::test]
    async fn tunnel_grants_verify_against_core_jwks() {
        let (issuer_key, x) = issuer();
        let (verifier, _base) = verifier_with_stub(serde_json::json!({
            "keys": [{"kty": "OKP", "crv": "Ed25519", "x": x, "kid": "k1",
                      "use": "sig", "alg": "EdDSA"}],
        }))
        .await;
        let sub = "acct_00000000000000000000000000";
        let mid = "mch_11111111111111111111111111";
        let token = grant_token(&issuer_key, sub, mid, &[Scope::RelayConnect, Scope::ForgeExecute]);
        let claims = verifier
            .verify_channel_grant(&token, sub, mid, 1_700_000_100, true)
            .await
            .unwrap();
        assert_eq!(claims.sub, sub);
        // Missing forge:execute is refused.
        let routed = grant_token(&issuer_key, sub, mid, &[Scope::RelayConnect]);
        assert!(verifier.verify_channel_grant(&routed, sub, mid, 1_700_000_100, true).await.is_err());
        // Cross-machine binding is refused.
        assert!(
            verifier
                .verify_channel_grant(&token, sub, "mch_22222222222222222222222222", 1_700_000_100, true)
                .await
                .is_err()
        );
    }

    #[test]
    fn rejection_kind_is_exported() {
        let _ = RejectionKind::Signature;
    }
}
