use artisan_account::{GrantClaims, Scope, VerifyParams, is_account_id, is_machine_id};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const JWKS_TTL: Duration = Duration::from_mins(10);

#[derive(Clone, Debug)]
struct CachedJwks {
    at: Instant,
    keys: HashMap<String, [u8; 32]>,
}

pub struct GrantVerifier {
    http: reqwest::Client,
    jwks_url: String,
    expected_iss: String,
    cache: tokio::sync::Mutex<Option<CachedJwks>>,
}

#[derive(Debug, thiserror::Error)]
#[error("build the JWKS HTTP client")]
pub struct BuildVerifierError(#[source] reqwest::Error);

#[derive(Debug, thiserror::Error)]
pub enum FetchJwksError {
    #[error("fetch Core's JWKS from {url}")]
    Reach {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("Core's JWKS at {url} answered an error")]
    Status {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("read Core's JWKS from {url}")]
    Decode {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("decode JWKS key {kid} from {url}")]
    Key {
        url: String,
        kid: String,
        #[source]
        source: base64::DecodeError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyGrantError {
    #[error("grant is not bound to a Core account/machine route ({account}/{machine})")]
    Route { account: String, machine: String },
    #[error("grant rejected: bad header")]
    Header(#[source] artisan_account::UnverifiedGrantError),
    #[error(transparent)]
    Jwks(#[from] FetchJwksError),
    #[error("grant key id {kid} is unknown to Core")]
    UnknownKey { kid: String },
    #[error("verify the grant for {account}/{machine}")]
    Rejected {
        account: String,
        machine: String,
        #[source]
        source: artisan_account::GrantError,
    },
    #[error("grant {jti} is not a client execution grant")]
    NotClient { jti: String },
    #[error("grant {jti} lacks the relay:connect scope")]
    NoRelayConnect { jti: String },
    #[error("grant {jti} lacks the forge:execute scope")]
    NoForgeExecute { jti: String },
}

impl GrantVerifier {
    pub fn new(jwks_url: &str, expected_iss: &str) -> Result<Self, BuildVerifierError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(BuildVerifierError)?,
            jwks_url: jwks_url.to_owned(),
            expected_iss: expected_iss.to_owned(),
            cache: tokio::sync::Mutex::new(None),
        })
    }

    async fn jwks(&self, force: bool) -> Result<HashMap<String, [u8; 32]>, FetchJwksError> {
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

    async fn fetch(
        http: &reqwest::Client,
        url: &str,
    ) -> Result<HashMap<String, [u8; 32]>, FetchJwksError> {
        use base64::Engine as _;
        let jwks: crate::core::Jwks = http
            .get(url)
            .send()
            .await
            .map_err(|source| FetchJwksError::Reach {
                url: url.to_owned(),
                source,
            })?
            .error_for_status()
            .map_err(|source| FetchJwksError::Status {
                url: url.to_owned(),
                source,
            })?
            .json()
            .await
            .map_err(|source| FetchJwksError::Decode {
                url: url.to_owned(),
                source,
            })?;
        let mut keys = HashMap::new();
        for key in jwks.keys {
            if key.kty != "OKP" || key.crv != "Ed25519" {
                continue;
            }
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(key.x.as_bytes())
                .map_err(|source| FetchJwksError::Key {
                    url: url.to_owned(),
                    kid: key.kid.clone(),
                    source,
                })?;
            if let Ok(public) = <[u8; 32]>::try_from(raw.as_slice()) {
                keys.insert(key.kid, public);
            }
        }
        Ok(keys)
    }

    pub async fn verify_channel_grant(
        &self,
        token: &str,
        account: &str,
        machine: &str,
        now_sec: i64,
        require_execute: bool,
    ) -> Result<GrantClaims, VerifyGrantError> {
        if !is_account_id(account) || !is_machine_id(machine) {
            return Err(VerifyGrantError::Route {
                account: account.to_owned(),
                machine: machine.to_owned(),
            });
        }
        let kid = artisan_account::unverified_kid(token).map_err(VerifyGrantError::Header)?;
        let mut keys = self.jwks(false).await?;
        if !keys.contains_key(&kid) {
            keys = self.jwks(true).await?;
        }
        let Some(issuer) = keys.get(&kid).copied() else {
            return Err(VerifyGrantError::UnknownKey { kid });
        };
        let params = VerifyParams {
            expected_iss: self.expected_iss.clone(),
            expected_aud: artisan_account::AUD_RELAY.to_owned(),
            max_ttl_s: artisan_account::GRANT_MAX_TTL_S,
            expected_sub: account.to_owned(),
            expected_mid: machine.to_owned(),
            now_sec,
        };
        let claims =
            artisan_account::parse_and_verify_jwt(token, &issuer, &params).map_err(|source| {
                VerifyGrantError::Rejected {
                    account: account.to_owned(),
                    machine: machine.to_owned(),
                    source,
                }
            })?;
        if claims.role != artisan_account::Role::Client {
            return Err(VerifyGrantError::NotClient { jti: claims.jti });
        }
        if !claims.scope.contains(&Scope::RelayConnect) {
            return Err(VerifyGrantError::NoRelayConnect { jti: claims.jti });
        }
        if require_execute && !claims.scope.contains(&Scope::ForgeExecute) {
            return Err(VerifyGrantError::NoForgeExecute { jti: claims.jti });
        }
        Ok(claims)
    }
}

#[cfg(test)]
#[path = "../tests/account_auth/unit.rs"]
pub(crate) mod tests;
