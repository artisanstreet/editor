use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const AUD_RELAY: &str = "ast-relay";

const SKEW_S: i64 = 60;

#[must_use]
pub fn is_valid_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return false;
    }
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    rest.iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-'))
}

#[must_use]
fn is_core_id(prefix: &str, value: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    let Some(suffix) = suffix.strip_prefix('_') else {
        return false;
    };
    if suffix.len() != 26 {
        return false;
    }
    suffix.bytes().all(|byte| {
        matches!(
            byte,
            b'0'..=b'9' | b'A'..=b'H' | b'J' | b'K' | b'M' | b'N' | b'P'..=b'T' | b'V'..=b'Z'
        )
    })
}

#[must_use]
pub fn is_account_id(value: &str) -> bool {
    is_core_id("acct", value)
}

#[must_use]
pub fn is_machine_id(value: &str) -> bool {
    is_core_id("mch", value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[serde(rename = "relay:connect")]
    RelayConnect,
    #[serde(rename = "forge:execute")]
    ForgeExecute,
    #[serde(rename = "machine:sync")]
    MachineSync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Client,
    Forge,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Aud {
    One(String),
    Many(Vec<String>),
}

impl Aud {
    #[must_use]
    pub fn values(&self) -> Vec<&str> {
        match self {
            Self::One(value) => vec![value.as_str()],
            Self::Many(values) => values.iter().map(String::as_str).collect(),
        }
    }

    #[must_use]
    pub fn contains(&self, aud: &str) -> bool {
        self.values().contains(&aud)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CnfJwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantClaims {
    pub v: u32,
    pub iss: String,
    pub aud: Aud,
    pub sub: String,
    pub mid: String,
    pub role: Role,
    pub scope: Vec<Scope>,
    pub cnf: CnfHolder,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<i64>,
    pub iat: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbf: Option<i64>,
    pub exp: i64,
    pub jti: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CnfHolder {
    pub jwk: CnfJwk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantErrorKind {
    Version,
    Issuer,
    Audience,
    Time,
    Ttl,
    Scope,
    Binding,
    Key,
    Id,
    Jti,
    Signature,
    Encoding,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("grant rejected: {kind:?}")]
pub struct GrantError {
    pub kind: GrantErrorKind,
}

impl GrantError {
    const fn of(kind: GrantErrorKind) -> Self {
        Self { kind }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyParams {
    pub expected_iss: String,
    pub expected_aud: String,
    pub max_ttl_s: i64,
    pub expected_sub: String,
    pub expected_mid: String,
    pub now_sec: i64,
}

pub fn cnf_public_key(claims: &GrantClaims) -> Result<[u8; 32], GrantError> {
    if claims.cnf.jwk.kty != "OKP" || claims.cnf.jwk.crv != "Ed25519" {
        return Err(GrantError::of(GrantErrorKind::Key));
    }
    let raw = URL_SAFE_NO_PAD
        .decode(claims.cnf.jwk.x.as_bytes())
        .map_err(|_cause| GrantError::of(GrantErrorKind::Key))?;
    if raw.len() != 32 {
        return Err(GrantError::of(GrantErrorKind::Key));
    }
    let mut key = [0_u8; 32];
    key.copy_from_slice(&raw);
    Ok(key)
}

pub fn verify_claims(claims: &GrantClaims, params: &VerifyParams) -> Result<(), GrantError> {
    if claims.v != crate::PROTOCOL_VERSION {
        return Err(GrantError::of(GrantErrorKind::Version));
    }
    if claims.iss != params.expected_iss {
        return Err(GrantError::of(GrantErrorKind::Issuer));
    }
    if !claims.aud.contains(params.expected_aud.as_str()) {
        return Err(GrantError::of(GrantErrorKind::Audience));
    }
    if claims.sub != params.expected_sub || claims.mid != params.expected_mid {
        return Err(GrantError::of(GrantErrorKind::Binding));
    }
    if !is_account_id(&claims.sub) || !is_machine_id(&claims.mid) {
        return Err(GrantError::of(GrantErrorKind::Id));
    }
    if claims.jti.is_empty() || claims.jti.len() > 256 {
        return Err(GrantError::of(GrantErrorKind::Jti));
    }
    if claims.iat > params.now_sec.saturating_add(SKEW_S)
        || claims.exp <= params.now_sec.saturating_sub(SKEW_S)
    {
        return Err(GrantError::of(GrantErrorKind::Time));
    }
    if let Some(nbf) = claims.nbf
        && params.now_sec < nbf
    {
        return Err(GrantError::of(GrantErrorKind::Time));
    }
    let ttl = claims.exp.saturating_sub(claims.iat);
    if ttl <= 0 || ttl > params.max_ttl_s {
        return Err(GrantError::of(GrantErrorKind::Ttl));
    }
    if claims.scope.is_empty() {
        return Err(GrantError::of(GrantErrorKind::Scope));
    }
    let unique: BTreeSet<Scope> = claims.scope.iter().copied().collect();
    if unique.len() != claims.scope.len() {
        return Err(GrantError::of(GrantErrorKind::Scope));
    }
    if !unique.contains(&Scope::RelayConnect) {
        return Err(GrantError::of(GrantErrorKind::Scope));
    }
    cnf_public_key(claims)?;
    Ok(())
}

pub fn parse_and_verify_jwt(
    compact: &str,
    issuer_pubkey: &[u8; 32],
    params: &VerifyParams,
) -> Result<GrantClaims, GrantError> {
    let (signing_input, signature) = split_compact(compact)?;
    let claims: GrantClaims =
        unverified_part(compact, 1).map_err(|_decode| GrantError::of(GrantErrorKind::Id))?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(issuer_pubkey)
        .map_err(|_cause| GrantError::of(GrantErrorKind::Signature))?;
    let signature = ed25519_dalek::Signature::from_slice(&signature)
        .map_err(|_cause| GrantError::of(GrantErrorKind::Signature))?;
    key.verify_strict(signing_input.as_bytes(), &signature)
        .map_err(|_cause| GrantError::of(GrantErrorKind::Signature))?;
    verify_claims(&claims, params)?;
    Ok(claims)
}

fn split_compact(compact: &str) -> Result<(String, Vec<u8>), GrantError> {
    let mut parts = compact.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(GrantError::of(GrantErrorKind::Encoding));
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return Err(GrantError::of(GrantErrorKind::Encoding));
    }
    check_header(compact)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature.as_bytes())
        .map_err(|_cause| GrantError::of(GrantErrorKind::Encoding))?;
    if signature.len() != 64 {
        return Err(GrantError::of(GrantErrorKind::Signature));
    }
    Ok((format!("{header}.{payload}"), signature))
}

fn check_header(compact: &str) -> Result<(), GrantError> {
    unverified_kid(compact)
        .map(drop)
        .map_err(|_header| GrantError::of(GrantErrorKind::Signature))
}

#[derive(Deserialize)]
struct UnverifiedHeader {
    alg: String,
    kid: String,
}

#[derive(Deserialize)]
struct UnverifiedJti {
    jti: String,
}

#[derive(Debug, thiserror::Error)]
pub enum UnverifiedGrantError {
    #[error("the grant is not a compact JWT with part {index}")]
    MissingPart { index: usize },
    #[error("decode part {index} of the grant")]
    Base64 {
        index: usize,
        #[source]
        source: base64::DecodeError,
    },
    #[error("read part {index} of the grant")]
    Json {
        index: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("the grant has no jti")]
    NoJti,
    #[error("the grant is signed with {alg}, not EdDSA")]
    NotEdDsa { alg: String },
    #[error("the grant has no kid")]
    NoKid,
}

pub fn unverified_part<T: serde::de::DeserializeOwned>(
    compact: &str,
    index: usize,
) -> Result<T, UnverifiedGrantError> {
    let part = compact
        .split('.')
        .nth(index)
        .ok_or(UnverifiedGrantError::MissingPart { index })?;
    let raw = URL_SAFE_NO_PAD
        .decode(part.as_bytes())
        .map_err(|source| UnverifiedGrantError::Base64 { index, source })?;
    serde_json::from_slice(&raw).map_err(|source| UnverifiedGrantError::Json { index, source })
}

pub fn unverified_jti(compact: &str) -> Result<String, UnverifiedGrantError> {
    let UnverifiedJti { jti } = unverified_part(compact, 1)?;
    if jti.is_empty() {
        return Err(UnverifiedGrantError::NoJti);
    }
    Ok(jti)
}

pub fn unverified_kid(compact: &str) -> Result<String, UnverifiedGrantError> {
    let UnverifiedHeader { alg, kid } = unverified_part(compact, 0)?;
    if alg != "EdDSA" {
        return Err(UnverifiedGrantError::NotEdDsa { alg });
    }
    if kid.is_empty() {
        return Err(UnverifiedGrantError::NoKid);
    }
    Ok(kid)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test fixtures; failures fail the test"
)]
#[path = "../tests/grant/unit.rs"]
mod tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test fixtures; failures fail the test")]
#[path = "../tests/grant/core_fixture.rs"]
mod core_fixture_tests;
