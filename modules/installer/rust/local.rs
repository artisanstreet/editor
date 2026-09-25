//! Locally built releases: the per-root signing key and the tree manifest.
//!
//! A `dev`-channel installation trusts exactly one key: the one generated for
//! that installation root the first time a local release is produced for it.
//! The private seed stays in `<root>/trust/` and never enters a repository,
//! CI, or a build output. Release and nightly roots never consult it: only
//! [`TrustKey::is_local`] trust may verify `dev`-channel releases, and local
//! trust may verify nothing else.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    error::{InstallerError, Result, io},
    manifest::{SigningIdentity, TREE_MANIFEST_NAME, TREE_SIGNATURE_NAME, TreeManifest, TrustKey},
    platform::Platform,
};

/// Channel every local release belongs to.
pub const LOCAL_CHANNEL: &str = "dev";

/// Directory inside an installation root holding its local signing key.
pub const LOCAL_TRUST_DIRECTORY: &str = "trust";

const SEED_FILE: &str = "local-signing.key";
const PUBLIC_FILE: &str = "local-signing.json";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicRecord {
    key_id: String,
    public_key_hex: String,
}

/// The signing key of one installation root's local releases.
pub struct LocalSigner {
    key: SigningKey,
    key_id: String,
}

/// Versions and target of one local release.
#[derive(Clone, Debug)]
pub struct LocalRelease {
    /// Product version, which also names `versions/<v>`.
    pub product_version: String,
    /// Platform the payload's binaries run on.
    pub platform: Platform,
}

impl LocalSigner {
    /// Loads the local signing key of `root`, generating and recording it the
    /// first time.
    ///
    /// # Errors
    ///
    /// Returns [`InstallerError`] when the key files cannot be read, written,
    /// or disagree with each other, or randomness is unavailable.
    pub fn load_or_create(root: &Path) -> Result<Self> {
        let directory = root.join(LOCAL_TRUST_DIRECTORY);
        let seed_path = directory.join(SEED_FILE);
        if seed_path.is_file() {
            let seed = std::fs::read_to_string(&seed_path).map_err(io(&seed_path))?;
            let seed: [u8; 32] = hex::decode(seed.trim())
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| {
                    InstallerError::InvalidTrustKey("local signing key is malformed".to_owned())
                })?;
            let signer = Self::from_seed(&seed);
            let recorded = read_public_record(root)?;
            if recorded.key_id != signer.key_id
                || recorded.public_key_hex != hex::encode(signer.key.verifying_key().to_bytes())
            {
                return Err(InstallerError::InvalidTrustKey(
                    "local signing key does not match its recorded public key".to_owned(),
                ));
            }
            return Ok(signer);
        }
        let mut seed = [0_u8; 32];
        getrandom::fill(&mut seed).map_err(|error| {
            InstallerError::InvalidTrustKey(format!("no randomness for a local key: {error}"))
        })?;
        let signer = Self::from_seed(&seed);
        std::fs::create_dir_all(&directory).map_err(io(&directory))?;
        write_private(&seed_path, hex::encode(seed).as_bytes())?;
        let public = PublicRecord {
            key_id: signer.key_id.clone(),
            public_key_hex: hex::encode(signer.key.verifying_key().to_bytes()),
        };
        let public_path = directory.join(PUBLIC_FILE);
        let bytes = serde_json::to_vec_pretty(&public).map_err(InstallerError::InvalidPayload)?;
        std::fs::write(&public_path, bytes).map_err(io(&public_path))?;
        Ok(signer)
    }

    fn from_seed(seed: &[u8; 32]) -> Self {
        let key = SigningKey::from_bytes(seed);
        let key_id = key_id_for(&key.verifying_key());
        Self { key, key_id }
    }

    /// Key id recorded in manifests this key signs.
    #[must_use]
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Trust that verifies exactly this key's `dev`-channel releases.
    #[must_use]
    pub fn trust(&self) -> TrustKey {
        TrustKey::local(self.key.verifying_key(), self.key_id.clone())
    }

    /// Declares every payload file of the unpacked tree at `tree` in a signed
    /// tree manifest written beside them.
    ///
    /// # Errors
    ///
    /// Returns [`InstallerError`] when the tree layout is not a valid payload
    /// or the manifest cannot be written.
    pub fn write_tree_manifest(&self, tree: &Path, release: &LocalRelease) -> Result<()> {
        self.write_tree_manifest_to(tree, tree, release)
    }

    /// Like [`Self::write_tree_manifest`], but writes the manifest and
    /// signature into `destination`, leaving a read-only `tree` untouched.
    ///
    /// # Errors
    ///
    /// Returns [`InstallerError`] when the tree layout is not a valid payload
    /// or the manifest cannot be written.
    pub fn write_tree_manifest_to(
        &self,
        tree: &Path,
        destination: &Path,
        release: &LocalRelease,
    ) -> Result<()> {
        let manifest = TreeManifest {
            format_version: 1,
            product_version: release.product_version.clone(),
            editor_forge_compatibility_version: release.product_version.clone(),
            channel: LOCAL_CHANNEL.to_owned(),
            signing_identity: SigningIdentity {
                key_id: self.key_id.clone(),
                algorithm: "ed25519".to_owned(),
            },
            minimum_installer_version: env!("CARGO_PKG_VERSION").to_owned(),
            minimum_cli_version: release.product_version.clone(),
            platform: release.platform.os.to_owned(),
            architecture: release.platform.arch.to_owned(),
            files: crate::payload::tree_files(tree)?,
        };
        let bytes = serde_json::to_vec_pretty(&manifest).map_err(InstallerError::InvalidPayload)?;
        let signature = serde_json::to_vec(&serde_json::json!({
            "algorithm": "ed25519",
            "key_id": self.key_id,
            "signature": STANDARD.encode(self.key.sign(&bytes).to_bytes()),
        }))
        .map_err(InstallerError::InvalidPayload)?;
        std::fs::create_dir_all(destination).map_err(io(destination))?;
        let manifest_path = destination.join(TREE_MANIFEST_NAME);
        std::fs::write(&manifest_path, bytes).map_err(io(&manifest_path))?;
        let signature_path = destination.join(TREE_SIGNATURE_NAME);
        std::fs::write(&signature_path, signature).map_err(io(&signature_path))?;
        Ok(())
    }
}

/// Trust in the local signing key recorded for `root`, for verifying local
/// releases without the private key.
///
/// # Errors
///
/// Returns [`InstallerError`] when `root` has no recorded local key or the
/// record is malformed.
pub fn local_trust(root: &Path) -> Result<TrustKey> {
    let record = read_public_record(root)?;
    let bytes: [u8; 32] = hex::decode(&record.public_key_hex)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| {
            InstallerError::InvalidTrustKey("local public key is malformed".to_owned())
        })?;
    let key = VerifyingKey::from_bytes(&bytes)
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    if key_id_for(&key) != record.key_id {
        return Err(InstallerError::InvalidTrustKey(
            "local key id does not match its public key".to_owned(),
        ));
    }
    Ok(TrustKey::local(key, record.key_id))
}

fn read_public_record(root: &Path) -> Result<PublicRecord> {
    let path: PathBuf = root.join(LOCAL_TRUST_DIRECTORY).join(PUBLIC_FILE);
    let bytes = std::fs::read(&path).map_err(io(&path))?;
    serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)
}

/// `local-` plus the first 16 hex digits of the public key's SHA-256.
fn key_id_for(key: &VerifyingKey) -> String {
    let digest = hex::encode(Sha256::digest(key.to_bytes()));
    format!("local-{}", &digest[..16])
}

/// Writes secret material readable only by the owner where the platform
/// supports it (per-user profile directories already are on Windows).
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(io(path))?;
    file.write_all(bytes).map_err(io(path))?;
    file.sync_all().map_err(io(path))
}
