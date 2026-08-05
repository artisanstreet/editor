use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use url::Url;

use crate::error::{InstallerError, Result};

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
// Release builds must replace this development key through --public-key or
// ARTISAN_INSTALLER_PUBLIC_KEY until the official key is finalized.
const EMBEDDED_PUBLIC_KEY: Option<&str> = option_env!("ARTISAN_RELEASE_PUBLIC_KEY_HEX");

#[derive(Clone)]
pub struct TrustKey(VerifyingKey);

impl TrustKey {
    pub fn resolve(configured: Option<&str>) -> Result<Self> {
        let bytes = match configured {
            Some(value) => hex::decode(value)
                .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?,
            None => hex::decode(EMBEDDED_PUBLIC_KEY.ok_or_else(|| {
                InstallerError::InvalidTrustKey(
                    "release build has no embedded key; provide --public-key".to_owned(),
                )
            })?)
            .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?,
        };
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
            .map(Self)
            .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))
    }

    #[cfg(test)]
    pub fn from_verifying_key(key: VerifyingKey) -> Self {
        Self(key)
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

#[derive(Clone, Debug, Deserialize)]
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

fn decode(bytes: &[u8], signature_bytes: &[u8], trust: &TrustKey) -> Result<ReleaseManifest> {
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(InstallerError::ManifestTooLarge(MAX_MANIFEST_BYTES));
    }
    let envelope: ManifestSignature =
        serde_json::from_slice(signature_bytes).map_err(InstallerError::InvalidManifest)?;
    if envelope.algorithm != "ed25519" {
        return Err(InstallerError::InvalidSignature);
    }
    let signature_bytes = STANDARD
        .decode(envelope.signature)
        .map_err(|_| InstallerError::InvalidSignature)?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| InstallerError::InvalidSignature)?;
    trust
        .0
        .verify(bytes, &signature)
        .map_err(|_| InstallerError::InvalidSignature)?;
    let manifest: ReleaseManifest =
        serde_json::from_slice(bytes).map_err(InstallerError::InvalidPayload)?;
    if manifest.format_version != 1
        || manifest.signing_identity.algorithm != envelope.algorithm
        || manifest.signing_identity.key_id != envelope.key_id
    {
        return Err(InstallerError::InvalidSignature);
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    use super::{TrustKey, decode};

    #[test]
    fn accepts_an_authentic_payload() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let payload = br#"{"format_version":1,"product_version":"1.0.0","editor_forge_compatibility_version":"1.0.0","channel":"stable","signing_identity":{"key_id":"test","algorithm":"ed25519"},"minimum_installer_version":"0.1.0","minimum_cli_version":"0.1.0","artifacts":[]}"#;
        let envelope = json!({
            "algorithm": "ed25519",
            "key_id": "test",
            "signature": STANDARD.encode(signing.sign(payload).to_bytes())
        });
        let manifest = decode(
            payload,
            &serde_json::to_vec(&envelope).expect("serialize"),
            &TrustKey::from_verifying_key(signing.verifying_key()),
        )
        .expect("valid manifest");
        assert_eq!(manifest.product_version, "1.0.0");
    }

    #[test]
    fn rejects_payload_tampering() {
        let signing = SigningKey::from_bytes(&[8; 32]);
        let payload = br#"{"format_version":1}"#;
        let envelope = json!({
            "algorithm": "ed25519",
            "key_id": "test",
            "signature": STANDARD.encode(signing.sign(payload).to_bytes())
        });
        assert!(
            decode(
                br#"{"format_version":2}"#,
                &serde_json::to_vec(&envelope).expect("serialize"),
                &TrustKey::from_verifying_key(signing.verifying_key())
            )
            .is_err()
        );
    }
}
