use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const AUD_RELAY: &str = "ast-relay";

/// Allowed clock skew in seconds (Core §4).
const SKEW_S: i64 = 60;

/// Whether an id matches Relay's channel charset
/// (`^[A-Za-z0-9][A-Za-z0-9._~-]{0,127}$`, also used for route ids).
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
    rest.iter().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'~' | b'-')
    })
}

/// Whether `value` is a Core id with the given prefix
/// (`{prefix}_{26 Crockford ULID chars, no I/L/O/U}`).
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

/// Whether `value` is an `acct_…` id.
#[must_use]
pub fn is_account_id(value: &str) -> bool {
    is_core_id("acct", value)
}

/// Whether `value` is an `mch_…` id.
#[must_use]
pub fn is_machine_id(value: &str) -> bool {
    is_core_id("mch", value)
}

/// Grant scope (reconciled set).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Open a Relay route (replaces Relay v1 `relay:route`).
    #[serde(rename = "relay:connect")]
    RelayConnect,
    /// Execute on the paired Forge (replaces `forge:exec`; full Forge trust,
    /// no per-project tiers).
    #[serde(rename = "forge:execute")]
    ForgeExecute,
    /// Machine config/vault sync (replaces `forge:sync`).
    #[serde(rename = "machine:sync")]
    MachineSync,
}

/// `client` (Editor) or `forge` (machine daemon).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Client,
    Forge,
}

/// `aud`: one value or several. Accepts both on read; writes back as sent.
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

/// Confirmation key: Ed25519 public key in JWK shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CnfJwk {
    pub kty: String,
    pub crv: String,
    /// 32-byte Ed25519 public key, base64url (43 chars, no padding).
    pub x: String,
}

/// Reconciled grant claims. Unknown top-level fields are rejected.
///
/// Core v1: `aud` is a singular exact string (`ast-relay` or `ast-core`).
/// Array-formed `aud` is tolerated on read only (narrow membership check);
/// Core never emits arrays. There is NO `rev` claim in v1 (short TTL +
/// revocation lists instead); `rev` is accepted when present for
/// forward-compatibility but never required.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantClaims {
    /// Protocol version, currently 1.
    pub v: u32,
    /// Core issuer (equals `CORE_JWT_ISSUER`).
    pub iss: String,
    /// Audience: `ast-relay` for Relay admission, `ast-core` for sync.
    pub aud: Aud,
    /// Account id `acct_…` (reconciled: `sub`).
    pub sub: String,
    /// Explicit target machine `mch_…` for ALL relay grants.
    pub mid: String,
    /// `client` or `forge`.
    pub role: Role,
    /// At least one reconciled scope.
    pub scope: Vec<Scope>,
    /// Proof-of-possession key holder.
    pub cnf: CnfHolder,
    /// Accepted when present (pre-Core-update tokens); never required in v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<i64>,
    /// Issued-at / not-before / expiry (seconds since Unix epoch).
    pub iat: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbf: Option<i64>,
    pub exp: i64,
    /// Single-use id.
    pub jti: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CnfHolder {
    pub jwk: CnfJwk,
}

/// What `verify_claims` checked.
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

/// Parameters the verifier is configured with (Core agreement pending on
/// `iss`/JWKS URL; `expected_aud` is `ast-relay` for Relay admission).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyParams {
    pub expected_iss: String,
    pub expected_aud: String,
    pub max_ttl_s: i64,
    /// Route binding: the `{account}/{machine}` being opened.
    pub expected_sub: String,
    pub expected_mid: String,
    /// Forge role needs `forge:execute` when it serves; clients need
    /// `relay:connect`. Checked by the caller via [`required_scope`].
    pub now_sec: i64,
}

/// Decode the 32-byte `cnf` public key.
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

/// Check every reconciled claim rule except the EdDSA signature itself.
///
/// Clock skew: ±60 s (Core §4). TTL: `0 < exp - iat <= max_ttl_s`.
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
    // Touch the key shape now so malformed `cnf` fails before admission.
    cnf_public_key(claims)?;
    Ok(())
}

/// Parse a compact JWT and verify its EdDSA signature with `issuer_pubkey`.
///
/// Then check reconciled claims. `kid` handling (key selection/rotation) is
/// the caller's job; this only verifies bytes with the given key.
pub fn parse_and_verify_jwt(
    compact: &str,
    issuer_pubkey: &[u8; 32],
    params: &VerifyParams,
) -> Result<GrantClaims, GrantError> {
    let (signing_input, signature) = split_compact(compact)?;
    let claims: GrantClaims = unverified_part(compact, 1).map_err(|_decode| GrantError::of(GrantErrorKind::Id))?;
    // Standard JWT EdDSA verification: Ed25519 over ASCII `header.payload`.
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
    let (Some(header), Some(payload), Some(signature), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
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
    unverified_kid(compact).map(drop).map_err(|_header| GrantError::of(GrantErrorKind::Signature))
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

/// Decodes one part (0 header, 1 payload) of a compact JWT without checking
/// its signature.
pub fn unverified_part<T: serde::de::DeserializeOwned>(
    compact: &str,
    index: usize,
) -> anyhow::Result<T> {
    use anyhow::Context as _;
    let part = compact.split('.').nth(index).context("the grant is not a compact JWT")?;
    let raw = URL_SAFE_NO_PAD.decode(part.as_bytes()).context("decode the grant")?;
    serde_json::from_slice(&raw).context("read the grant")
}

pub fn unverified_jti(compact: &str) -> anyhow::Result<String> {
    let UnverifiedJti { jti } = unverified_part(compact, 1)?;
    anyhow::ensure!(!jti.is_empty(), "the grant has no jti");
    Ok(jti)
}

pub fn unverified_kid(compact: &str) -> anyhow::Result<String> {
    let UnverifiedHeader { alg, kid } = unverified_part(compact, 0)?;
    anyhow::ensure!(alg == "EdDSA", "the grant is not EdDSA-signed");
    anyhow::ensure!(!kid.is_empty(), "the grant has no kid");
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
mod tests {
    use super::*;

    fn signing_key() -> ed25519_dalek::SigningKey {
        let mut secret = [7_u8; 32];
        // Fixed test key (not a fixture vector, just a deterministic signer).
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
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({"alg": "EdDSA", "kid": "test-key-1", "typ": "JWT"}).to_string(),
        );
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
        assert!(serde_json::from_value::<GrantClaims>(value).is_err());
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
        // `relay:route` / `forge:exec` / `forge:sync` / `relay:admin` /
        // `relay:pair` are NOT in the reconciled set.
        for legacy in [
            "relay:route",
            "forge:exec",
            "forge:sync",
            "relay:admin",
            "relay:pair",
        ] {
            let mut value = serde_json::to_value(claims()).unwrap();
            value["scope"] = serde_json::json!([legacy]);
            assert!(
                serde_json::from_value::<GrantClaims>(value).is_err(),
                "{legacy} must not parse"
            );
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
        // I, L, O, U are not Crockford base32.
        assert!(!is_account_id("acct_IIIIIIIIIIIIIIIIIIIIIIIIII"));
        assert!(!is_account_id("acct_0000000000000000000000000"));
        assert!(!is_account_id("acct_000000000000000000000000000"));
    }

    #[test]
    fn kid_selects_the_verifying_key() {
        let key = signing_key();
        let token = compact(&claims(), &key);
        assert_eq!(unverified_kid(&token).unwrap(), "test-key-1");
        assert!(unverified_kid("bad.token").is_err());
    }

    #[test]
    fn array_aud_is_tolerated_but_core_emits_singular() {
        let key = signing_key();
        let mut tolerated = claims();
        tolerated.aud = Aud::Many(vec!["other".into(), AUD_RELAY.into()]);
        let token = compact(&tolerated, &key);
        parse_and_verify_jwt(&token, key.verifying_key().as_bytes(), &params()).unwrap();
    }
}

/// Core-issued fixture validation (`fixtures/core-issued.json`, copied
/// verbatim from `core/fixtures/v1/` — TEST-ONLY synthetic credentials).
/// Catches exact field drift against Core's actual `issueGrant` output.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures; failures fail the test"
)]
mod core_fixture_tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/core-issued.json");

    fn fixture() -> serde_json::Value {
        serde_json::from_str(FIXTURE).unwrap()
    }

    #[test]
    fn core_issued_grant_verifies_byte_exact() {
        let fixture = fixture();
        let token = fixture["grant"].as_str().unwrap();
        // Issuer key comes from the fixture JWKS (kid-selected, as prod does).
        let jwks_key = fixture["jwks"]["keys"][0].clone();
        let fixture_kid = jwks_key["kid"].as_str().unwrap().to_owned();
        assert_eq!(crate::grant::unverified_kid(token).unwrap(), fixture_kid);
        let issuer = {
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(jwks_key["x"].as_str().unwrap().as_bytes())
                .unwrap();
            <[u8; 32]>::try_from(raw).unwrap()
        };
        // now_sec comes from the token itself (iat == nbf in the fixture):
        // keeps the test byte-exact without hardcoding Core's emission time.
        let payload_bytes = {
            use base64::Engine as _;
            let part = token.split('.').nth(1).unwrap();
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part.as_bytes())
                .unwrap()
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
            // nowSec inside the grant window (iat == nbf here).
            now_sec: fixture_iat,
        };
        let claims = parse_and_verify_jwt(token, &issuer, &params).unwrap();
        assert_eq!(claims.v, 1);
        assert_eq!(claims.sub, expected["sub"].as_str().unwrap());
        assert_eq!(claims.mid, expected["mid"].as_str().unwrap());
        assert_eq!(claims.role, Role::Forge);
        assert_eq!(
            claims.scope,
            vec![Scope::RelayConnect, Scope::ForgeExecute]
        );
        assert_eq!(claims.iat, fixture_iat);
        assert_eq!(claims.nbf, Some(fixture_iat));
        assert_eq!(claims.exp - claims.iat, 300);
        assert_eq!(claims.jti, fixture_jti);
        // v1 has NO rev claim: present-in-fixture would fail strict reads,
        // and our parser must accept its absence.
        assert_eq!(claims.rev, None);
        let cnf = cnf_public_key(&claims).unwrap();
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cnf),
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
        // Body hash matches the signed canonical string.
        assert_eq!(
            crate::machine_proof::body_hash_hex(body),
            crate::machine_proof::EMPTY_BODY_HASH
        );
        let canonical = crate::machine_proof::canonical_request(
            headers["x-ast-timestamp"].as_str().unwrap().parse().unwrap(),
            headers["x-ast-nonce"].as_str().unwrap(),
            "GET",
            proof["pathAndQuery"].as_str().unwrap(),
            &crate::machine_proof::body_hash_hex(body),
        );
        // Machine grants bind cnf == enrolled key: the proof verifies with
        // the grant's cnf public key.
        let token = fixture["grant"].as_str().unwrap();
        let payload = token.split('.').nth(1).unwrap();
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.as_bytes())
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        let cnf_x = payload["cnf"]["jwk"]["x"].as_str().unwrap();
        let public = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(cnf_x.as_bytes())
            .unwrap();
        let public = <[u8; 32]>::try_from(public).unwrap();
        let request = crate::machine_proof::MachineProof {
            public: &public,
            body,
            timestamp_sec: headers["x-ast-timestamp"].as_str().unwrap().parse().unwrap(),
            nonce_b64u: headers["x-ast-nonce"].as_str().unwrap(),
            method: proof["method"].as_str().unwrap(),
            path_and_query: proof["pathAndQuery"].as_str().unwrap(),
            signature_b64u: headers["authorization"]
                .as_str()
                .unwrap()
                .strip_prefix("AST-Machine ")
                .unwrap(),
            now_sec: headers["x-ast-timestamp"].as_str().unwrap().parse().unwrap(),
        };
        crate::machine_proof::verify_machine_proof(&request).unwrap();
        let _ = canonical;
    }
}
