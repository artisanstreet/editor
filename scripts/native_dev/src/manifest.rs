//! Installation and payload manifests for the dev home.
//!
//! Both documents are validated through the shipping authorities — the
//! installation manifest through the CLI loader and the payload through
//! the existing verifier — so a home the Editor would refuse fails here
//! first. The document shapes mirror exactly what the loader reads; the
//! release installer's writer covers a different (signed-release) schema
//! and stays untouched (see the runbook).

use std::collections::BTreeMap;

use artisan_editor_cli::{
    manifest::InstallationManifest,
    payload::{self, PAYLOAD_MANIFEST_NAME},
};

use crate::{
    error::DevError,
    paths::{DEV_VERSION, DevPaths},
    stage::{hash_file, staged_relative_names, write_atomic},
};

/// Builds the `installation.json` document for a dev home.
#[must_use]
pub fn installation_document(
    home: &std::path::Path,
    permanent_ae: &std::path::Path,
) -> serde_json::Value {
    serde_json::json!({
        "activation_state": "active",
        "finalization_state": "complete",
        "active_version": DEV_VERSION,
        "install_root": home,
        "permanent_ae_path": permanent_ae,
    })
}

/// Writes the payload manifest into one version root.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when a binary cannot be hashed or the
/// manifest cannot be written.
pub fn write_payload_manifest(version_root: &std::path::Path) -> Result<(), DevError> {
    let mut files = BTreeMap::new();
    for relative in staged_relative_names() {
        let digest = hash_file(&version_root.join(&relative))?;
        files.insert(relative, digest);
    }
    let identity = version_root.join(artisan_build_info::RESOURCE_PATH);
    if identity.is_file() {
        files.insert(
            artisan_build_info::RESOURCE_PATH.to_owned(),
            hash_file(&identity)?,
        );
    }
    let document = serde_json::json!({
        "format_version": 1,
        "files": files,
    });
    let bytes = serde_json::to_vec(&document).map_err(|_| DevError::Stage {
        stage: "payload",
        reason: "cannot serialize payload manifest".to_owned(),
    })?;
    write_atomic(&version_root.join(PAYLOAD_MANIFEST_NAME), &bytes, "payload")
}

/// Gates one version root on the shipping verifier.
///
/// # Errors
///
/// Returns [`DevError::PayloadUnverified`] when the existing verifier
/// reports drift.
pub fn verify_payload_dir(version_root: &std::path::Path) -> Result<(), DevError> {
    match payload::verify(version_root) {
        payload::PayloadHealth::Verified => Ok(()),
        payload::PayloadHealth::Modified(issues) => Err(DevError::PayloadUnverified {
            issues: issues.join(", "),
        }),
        payload::PayloadHealth::Unverifiable => Err(DevError::PayloadUnverified {
            issues: "payload manifest missing or unreadable".to_owned(),
        }),
    }
}

/// Writes and validates the dev installation manifest.
///
/// This runs after binary activation and provisioning, so a failed update
/// never leaves a `complete` marker over a broken home. On validation
/// failure the previous manifest is restored (or the new file removed when
/// there was none).
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the manifest cannot be written or the
/// shipping loader rejects it.
pub fn provision_manifest(paths: &DevPaths) -> Result<(), DevError> {
    let previous = std::fs::read(&paths.manifest_path).ok();
    let document = installation_document(&paths.home, &paths.permanent_ae);
    let bytes = serde_json::to_vec_pretty(&document).map_err(|_| DevError::Stage {
        stage: "manifest",
        reason: "cannot serialize installation manifest".to_owned(),
    })?;
    write_atomic(&paths.manifest_path, &bytes, "manifest")?;
    if let Err(error) = InstallationManifest::load(&paths.manifest_path) {
        match previous {
            Some(bytes) => {
                let _ = std::fs::write(&paths.manifest_path, &bytes);
            }
            None => {
                let _ = std::fs::remove_file(&paths.manifest_path);
            }
        }
        return Err(DevError::Stage {
            stage: "manifest",
            reason: format!("shipping manifest loader rejected the dev home: {error}"),
        });
    }
    Ok(())
}
