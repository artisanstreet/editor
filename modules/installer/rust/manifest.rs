//! Signed release manifest verification and the build-time trust policy.
//!
//! The installer has exactly one trust root per build:
//!
//! - **Development builds** carry no embedded anchor. The caller must supply
//!   `--public-key` (or `ARTISAN_INSTALLER_PUBLIC_KEY`) and the resulting trust
//!   is explicitly unpinned: it checks signatures but does not claim a release
//!   identity.
//! - **Release builds** embed the pinned release key id and public key at
//!   compile time (`ARTISAN_RELEASE_KEY_ID`, `ARTISAN_RELEASE_PUBLIC_KEY_HEX`).
//!   They refuse runtime overrides and reject any manifest whose signing key id
//!   does not match the pinned identity, even when the signature verifies.
//!
//! A release build compiled without an anchor is a compile-time error; see
//! `modules/installer/RELEASE_TRUST.md` for the release procedure.

use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::{InstallerError, Result};

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Pinned release identity embedded at compile time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseAnchor {
    pub key_id: &'static str,
    pub public_key_hex: &'static str,
}

/// The trust posture this binary was built with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildTrust {
    /// Development build: no embedded anchor; an explicit key is required at
    /// invocation and its signatures are not bound to a key id.
    Development,
    /// Release build: manifests must verify against the embedded pinned
    /// anchor. `None` is a build defect that fails closed.
    Release { anchor: Option<ReleaseAnchor> },
}

const EMBEDDED_RELEASE_PUBLIC_KEY_HEX: Option<&str> = option_env!("ARTISAN_RELEASE_PUBLIC_KEY_HEX");
const EMBEDDED_RELEASE_KEY_ID: Option<&str> = option_env!("ARTISAN_RELEASE_KEY_ID");
const EXPLICIT_RELEASE_BUILD: bool = option_env!("ARTISAN_INSTALLER_RELEASE").is_some();
const EXPLICIT_DEVELOPMENT_BUILD: bool = option_env!("ARTISAN_INSTALLER_DEVELOPMENT").is_some();

const fn embedded_anchor() -> Option<ReleaseAnchor> {
    match (EMBEDDED_RELEASE_KEY_ID, EMBEDDED_RELEASE_PUBLIC_KEY_HEX) {
        (Some(key_id), Some(public_key_hex)) => Some(ReleaseAnchor {
            key_id,
            public_key_hex,
        }),
        _ => None,
    }
}

/// The trust posture of the running binary, derived from compile-time inputs:
/// Explicit mode markers take precedence over the Cargo profile.
pub const fn build_trust() -> BuildTrust {
    if EXPLICIT_RELEASE_BUILD {
        BuildTrust::Release {
            anchor: embedded_anchor(),
        }
    } else if EXPLICIT_DEVELOPMENT_BUILD || cfg!(debug_assertions) {
        BuildTrust::Development
    } else {
        BuildTrust::Release {
            anchor: embedded_anchor(),
        }
    }
}

/// Shipping a release build without the pinned anchor would reintroduce
/// caller-chosen trust, so it is a build error rather than a runtime surprise.
#[cfg(not(test))]
const _: () = {
    if let BuildTrust::Release { anchor: None } = build_trust() {
        panic!(
            "release installer builds must embed the pinned release key (see modules/installer/RELEASE_TRUST.md)"
        );
    }
};

/// Whether the verification key originates from the pinned release anchor,
/// from an explicit development override, or from an installation root's
/// own local signing key.
#[derive(Clone, Debug, Eq, PartialEq)]
enum TrustIdentity {
    /// Development trust: the caller supplied the key; no key id is pinned.
    Unpinned,
    /// Release trust: the manifest must name exactly this key id.
    Pinned(String),
    /// Local trust: the key an installation root generated for locally built
    /// releases. Pinned to its key id and valid only for the `dev` channel.
    Local(String),
}

#[derive(Clone)]
pub struct TrustKey {
    key: VerifyingKey,
    identity: TrustIdentity,
}

impl TrustKey {
    /// Resolves the trust key for this process. Release builds use only their
    /// embedded anchor; development builds require an explicit override.
    ///
    /// # Errors
    ///
    /// Returns [`InstallerError`] when a release build lacks its anchor or is
    /// given an override, a development build lacks a key, or the key is
    /// malformed.
    pub fn resolve(configured: Option<&str>) -> Result<Self> {
        Self::resolve_for(build_trust(), configured)
    }

    fn resolve_for(build: BuildTrust, configured: Option<&str>) -> Result<Self> {
        match build {
            BuildTrust::Release { anchor } => {
                if configured.is_some() {
                    return Err(InstallerError::ReleaseTrustOverride);
                }
                let anchor = anchor.ok_or(InstallerError::MissingReleaseTrustAnchor)?;
                Self::from_hex(
                    anchor.public_key_hex,
                    TrustIdentity::Pinned(anchor.key_id.to_owned()),
                )
            }
            BuildTrust::Development => {
                let configured = configured.ok_or(InstallerError::MissingDevelopmentTrustKey)?;
                Self::from_hex(configured, TrustIdentity::Unpinned)
            }
        }
    }

    fn from_hex(value: &str, identity: TrustIdentity) -> Result<Self> {
        let bytes = hex::decode(value)
            .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
        let bytes = match bytes.as_slice() {
            // RFC 8410 SubjectPublicKeyInfo prefix used by the existing
            // TypeScript release trust contract.
            [
                0x30,
                0x2a,
                0x30,
                0x05,
                0x06,
                0x03,
                0x2b,
                0x65,
                0x70,
                0x03,
                0x21,
                0x00,
                rest @ ..,
            ] if rest.len() == 32 => rest.to_vec(),
            _ => bytes,
        };
        let bytes: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            InstallerError::InvalidTrustKey(format!("expected 32 bytes, got {}", bytes.len()))
        })?;
        VerifyingKey::from_bytes(&bytes)
            .map(|key| Self { key, identity })
            .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))
    }

    /// Trust in one installation root's local signing key.
    pub(crate) const fn local(key: VerifyingKey, key_id: String) -> Self {
        Self {
            key,
            identity: TrustIdentity::Local(key_id),
        }
    }

    /// Whether this is an installation root's local signing key, which may
    /// only ever verify `dev`-channel releases.
    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self.identity, TrustIdentity::Local(_))
    }

    const fn pinned_key_id(&self) -> Option<&String> {
        match &self.identity {
            TrustIdentity::Unpinned => None,
            TrustIdentity::Pinned(key_id) | TrustIdentity::Local(key_id) => Some(key_id),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn from_verifying_key(key: VerifyingKey) -> Self {
        Self {
            key,
            identity: TrustIdentity::Unpinned,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ManifestSignature {
    algorithm: String,
    key_id: String,
    signature: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub format_version: u8,
    pub product_version: String,
    pub editor_forge_compatibility_version: String,
    pub channel: String,
    pub signing_identity: SigningIdentity,
    pub minimum_installer_version: String,
    pub minimum_cli_version: String,
    pub artifacts: Vec<Artifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SigningIdentity {
    pub key_id: String,
    pub algorithm: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    #[serde(rename = "artifact_id")]
    pub id: String,
    pub platform: String,
    pub architecture: String,
    pub libc: Option<String>,
    #[serde(rename = "archive_format")]
    pub format: ArchiveFormat,
    pub file_name: String,
    #[serde(rename = "byte_size")]
    pub size: u64,
    pub sha256: String,
    pub archive_entries: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveFormat {
    Zip,
    #[serde(rename = "tar.zst")]
    TarZstd,
}

pub async fn fetch(
    client: &reqwest::Client,
    manifest_url: Url,
    signature_url: Url,
    trust: &TrustKey,
) -> Result<ReleaseManifest> {
    let response = client
        .get(manifest_url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(InstallerError::ManifestRequest)?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_MANIFEST_BYTES)
    {
        return Err(InstallerError::ManifestTooLarge(MAX_MANIFEST_BYTES));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(InstallerError::ManifestRequest)?;
    let signature = client
        .get(signature_url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(InstallerError::ManifestRequest)?
        .bytes()
        .await
        .map_err(InstallerError::ManifestRequest)?;
    if signature.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(InstallerError::ManifestTooLarge(MAX_MANIFEST_BYTES));
    }
    decode(&bytes, &signature, trust)
}

pub(crate) fn decode(
    bytes: &[u8],
    signature_bytes: &[u8],
    trust: &TrustKey,
) -> Result<ReleaseManifest> {
    let envelope = verify_signature(bytes, signature_bytes, trust)?;
    let manifest: ReleaseManifest =
        serde_json::from_slice(bytes).map_err(InstallerError::InvalidPayload)?;
    if manifest.format_version != 1 || !manifest.signing_identity.matches(&envelope) {
        return Err(InstallerError::InvalidSignature);
    }
    require_pinned_key_id(trust, envelope.key_id)?;
    Ok(manifest)
}

/// Verifies a signed tree manifest exactly like a release manifest.
pub(crate) fn decode_tree(
    bytes: &[u8],
    signature_bytes: &[u8],
    trust: &TrustKey,
) -> Result<TreeManifest> {
    let envelope = verify_signature(bytes, signature_bytes, trust)?;
    let manifest: TreeManifest =
        serde_json::from_slice(bytes).map_err(InstallerError::InvalidPayload)?;
    if manifest.format_version != 1 || !manifest.signing_identity.matches(&envelope) {
        return Err(InstallerError::InvalidSignature);
    }
    require_pinned_key_id(trust, envelope.key_id)?;
    Ok(manifest)
}

/// Checks the detached Ed25519 envelope over `bytes` against `trust`.
fn verify_signature(
    bytes: &[u8],
    signature_bytes: &[u8],
    trust: &TrustKey,
) -> Result<ManifestSignature> {
    if bytes.len() as u64 > MAX_MANIFEST_BYTES || signature_bytes.len() as u64 > MAX_MANIFEST_BYTES
    {
        return Err(InstallerError::ManifestTooLarge(MAX_MANIFEST_BYTES));
    }
    let envelope: ManifestSignature =
        serde_json::from_slice(signature_bytes).map_err(InstallerError::InvalidManifest)?;
    if envelope.algorithm != "ed25519" {
        return Err(InstallerError::InvalidSignature);
    }
    let raw = STANDARD
        .decode(&envelope.signature)
        .map_err(|_| InstallerError::InvalidSignature)?;
    let signature = Signature::from_slice(&raw).map_err(|_| InstallerError::InvalidSignature)?;
    trust
        .key
        .verify(bytes, &signature)
        .map_err(|_| InstallerError::InvalidSignature)?;
    Ok(envelope)
}

/// A verifying signature is not enough for pinned trust: the manifest must
/// also name the pinned key id.
fn require_pinned_key_id(trust: &TrustKey, actual: String) -> Result<()> {
    match trust.pinned_key_id() {
        Some(expected) if *expected != actual => Err(InstallerError::UntrustedSigningKey {
            expected: expected.clone(),
            actual,
        }),
        _ => Ok(()),
    }
}

impl SigningIdentity {
    fn matches(&self, envelope: &ManifestSignature) -> bool {
        self.algorithm == envelope.algorithm && self.key_id == envelope.key_id
    }
}

/// Signed description of an unpacked payload directory (a [`TreeManifest`]
/// file beside `bin/` and `resources/`), produced for locally built releases.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TreeManifest {
    pub format_version: u8,
    pub product_version: String,
    pub editor_forge_compatibility_version: String,
    pub channel: String,
    pub signing_identity: SigningIdentity,
    pub minimum_installer_version: String,
    pub minimum_cli_version: String,
    pub platform: String,
    pub architecture: String,
    /// Payload-relative path to lowercase hex SHA-256 for every payload file.
    pub files: BTreeMap<String, String>,
}

/// File name of a tree manifest inside its payload directory.
pub const TREE_MANIFEST_NAME: &str = "tree-manifest.json";

/// File name of a tree manifest's detached signature.
pub const TREE_SIGNATURE_NAME: &str = "tree-manifest.sig";

/// What activation records about a verified release, whatever its source.
#[derive(Clone, Debug)]
pub(crate) struct ReleaseRecord {
    pub product_version: String,
    pub editor_forge_compatibility_version: String,
    pub minimum_installer_version: String,
    pub minimum_cli_version: String,
    pub channel: String,
    pub signing_key_id: String,
    /// Artifact id, or `tree` for an unpacked payload.
    pub artifact_id: String,
    /// Archive digest, or the tree manifest digest for an unpacked payload.
    pub artifact_sha256: String,
}

impl ReleaseManifest {
    pub(crate) fn record(&self, artifact: &Artifact) -> ReleaseRecord {
        ReleaseRecord {
            product_version: self.product_version.clone(),
            editor_forge_compatibility_version: self.editor_forge_compatibility_version.clone(),
            minimum_installer_version: self.minimum_installer_version.clone(),
            minimum_cli_version: self.minimum_cli_version.clone(),
            channel: self.channel.clone(),
            signing_key_id: self.signing_identity.key_id.clone(),
            artifact_id: artifact.id.clone(),
            artifact_sha256: artifact.sha256.clone(),
        }
    }
}

impl TreeManifest {
    pub(crate) fn record(&self, manifest_sha256: String) -> ReleaseRecord {
        ReleaseRecord {
            product_version: self.product_version.clone(),
            editor_forge_compatibility_version: self.editor_forge_compatibility_version.clone(),
            minimum_installer_version: self.minimum_installer_version.clone(),
            minimum_cli_version: self.minimum_cli_version.clone(),
            channel: self.channel.clone(),
            signing_key_id: self.signing_identity.key_id.clone(),
            artifact_id: "tree".to_owned(),
            artifact_sha256: manifest_sha256,
        }
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
    use serde_json::json;

    use super::{BuildTrust, ReleaseAnchor, TrustKey, decode};
    use crate::error::InstallerError;

    fn static_hex(key: &VerifyingKey) -> &'static str {
        Box::leak(hex::encode(key.to_bytes()).into_boxed_str())
    }

    fn signed_envelope(signing: &SigningKey, key_id: &str, payload: &[u8]) -> Vec<u8> {
        json!({
            "algorithm": "ed25519",
            "key_id": key_id,
            "signature": STANDARD.encode(signing.sign(payload).to_bytes())
        })
        .to_string()
        .into_bytes()
    }

    fn manifest_payload(key_id: &str) -> Vec<u8> {
        format!(
            r#"{{"format_version":1,"product_version":"1.0.0","editor_forge_compatibility_version":"1.0.0","channel":"stable","signing_identity":{{"key_id":"{key_id}","algorithm":"ed25519"}},"minimum_installer_version":"0.1.0","minimum_cli_version":"0.1.0","artifacts":[]}}"#
        )
        .into_bytes()
    }

    fn pinned_trust(signing: &SigningKey, key_id: &'static str) -> TrustKey {
        TrustKey::resolve_for(
            BuildTrust::Release {
                anchor: Some(ReleaseAnchor {
                    key_id,
                    public_key_hex: static_hex(&signing.verifying_key()),
                }),
            },
            None,
        )
        .expect("pinned release anchor")
    }

    #[test]
    fn accepts_an_authentic_payload() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let payload = manifest_payload("test");
        let envelope = signed_envelope(&signing, "test", &payload);
        let manifest = decode(
            &payload,
            &envelope,
            &TrustKey::from_verifying_key(signing.verifying_key()),
        )
        .expect("valid manifest");
        assert_eq!(manifest.product_version, "1.0.0");
    }

    #[test]
    fn rejects_payload_tampering() {
        let signing = SigningKey::from_bytes(&[8; 32]);
        let payload = manifest_payload("test");
        let envelope = signed_envelope(&signing, "test", &payload);
        assert!(
            decode(
                br#"{"format_version":2}"#,
                &envelope,
                &TrustKey::from_verifying_key(signing.verifying_key())
            )
            .is_err()
        );
    }

    #[test]
    fn release_build_without_an_anchor_fails_closed() {
        let error = TrustKey::resolve_for(BuildTrust::Release { anchor: None }, None)
            .err()
            .expect("anchorless release trust");
        assert!(matches!(error, InstallerError::MissingReleaseTrustAnchor));
    }

    #[test]
    fn release_build_rejects_a_runtime_override() {
        let signing = SigningKey::from_bytes(&[9; 32]);
        let hex = static_hex(&signing.verifying_key());
        let error = TrustKey::resolve_for(
            BuildTrust::Release {
                anchor: Some(ReleaseAnchor {
                    key_id: "pinned",
                    public_key_hex: hex,
                }),
            },
            Some(hex),
        )
        .err()
        .expect("release override");
        assert!(matches!(error, InstallerError::ReleaseTrustOverride));
    }

    #[test]
    fn release_build_accepts_a_manifest_with_the_pinned_key_id() {
        let signing = SigningKey::from_bytes(&[10; 32]);
        let payload = manifest_payload("pinned");
        let envelope = signed_envelope(&signing, "pinned", &payload);
        let manifest = decode(&payload, &envelope, &pinned_trust(&signing, "pinned"))
            .expect("pinned manifest");
        assert_eq!(manifest.signing_identity.key_id, "pinned");
    }

    #[test]
    fn release_build_rejects_a_verifying_signature_with_the_wrong_key_id() {
        let signing = SigningKey::from_bytes(&[11; 32]);
        let payload = manifest_payload("other");
        let envelope = signed_envelope(&signing, "other", &payload);
        let error = decode(&payload, &envelope, &pinned_trust(&signing, "pinned"))
            .expect_err("mismatched key id");
        assert!(matches!(
            error,
            InstallerError::UntrustedSigningKey { expected, actual }
                if expected == "pinned" && actual == "other"
        ));
    }

    #[test]
    fn development_build_requires_an_explicit_key() {
        let error = TrustKey::resolve_for(BuildTrust::Development, None)
            .err()
            .expect("missing development key");
        assert!(matches!(error, InstallerError::MissingDevelopmentTrustKey));
    }

    #[test]
    fn development_build_accepts_an_explicit_key_without_pinning_an_identity() {
        let signing = SigningKey::from_bytes(&[12; 32]);
        let trust = TrustKey::resolve_for(
            BuildTrust::Development,
            Some(static_hex(&signing.verifying_key())),
        )
        .expect("development trust key");
        let payload = manifest_payload("whatever");
        let envelope = signed_envelope(&signing, "whatever", &payload);
        decode(&payload, &envelope, &trust).expect("development manifest");
    }
}
