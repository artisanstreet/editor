//! Installing a Nix-built payload as a dev release.
//!
//! Nix builds the payload (`bin/` plus `resources/build-info.json`) and owns
//! its identity. The runner never copies binaries into an installation
//! itself: it signs a tree manifest for the payload with the dev root's local
//! key, next to (not inside) the read-only payload, and hands both to
//! `artisan-install`, the same code that installs published releases.
//! Retiring a running dev Editor and Forge, activation, and rollback
//! therefore behave exactly as they do for a real update.

use std::path::Path;

use artisan_build_info::BuildInfo;
use artisan_install::{
    InstallIntegrationOptions, InstallOptions, LOCAL_CHANNEL, LocalRelease, LocalSigner, Platform,
    ReleaseSource, RetirementPolicy,
};

use crate::{error::DevError, paths::DevPaths};

/// Reads the identity Nix recorded in `payload`.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the payload carries no valid identity.
pub fn payload_identity(payload: &Path) -> Result<BuildInfo, DevError> {
    BuildInfo::read(payload).map_err(|error| DevError::Stage {
        stage: "payload",
        reason: format!(
            "{} is not a Nix-built payload ({error}); build one with `nix build .#<platform>-<stage>`",
            payload.display()
        ),
    })
}

/// Signs a tree manifest for `payload` into `manifests` with the dev root's
/// local key, leaving the payload itself untouched.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the payload layout is invalid or the
/// manifest cannot be written.
pub fn sign_payload(
    payload: &Path,
    manifests: &Path,
    identity: &BuildInfo,
    signer: &LocalSigner,
) -> Result<(), DevError> {
    let failed = |reason: String| DevError::Stage {
        stage: "sign",
        reason,
    };
    let platform = Platform::detect().map_err(|error| failed(error.to_string()))?;
    signer
        .write_tree_manifest_to(
            payload,
            manifests,
            &LocalRelease {
                product_version: identity.version.clone(),
                platform,
            },
        )
        .map_err(|error| failed(format!("cannot sign {}: {error}", payload.display())))
}

/// Installs the signed payload into the dev root as a `dev`-channel release.
///
/// A running dev Editor is closed and its Forge stopped first, exactly as a
/// release update retires superseded instances; PATH, shortcuts, and the
/// `artisan://` handler stay with the real installation.
///
/// # Errors
///
/// Returns [`DevError::Install`] when the installer refuses or fails.
pub fn install_payload(
    paths: &DevPaths,
    payload: &Path,
    manifests: &Path,
    signer: &LocalSigner,
) -> Result<(), DevError> {
    let platform = Platform::detect().map_err(DevError::Install)?;
    let options = InstallOptions {
        source: ReleaseSource::Tree {
            path: payload.to_path_buf(),
            manifest_directory: Some(manifests.to_path_buf()),
        },
        platform,
        install_root: paths.home.clone(),
        trust: signer.trust(),
        expected_channel: Some(LOCAL_CHANNEL.to_owned()),
        run_setup: false,
        restore_forge: false,
        integrations: InstallIntegrationOptions {
            register_protocol: false,
            register_shortcuts: false,
            register_path: false,
        },
        retirement: Some(RetirementPolicy {
            force: false,
            // The point of a dev run is to replace the running dev Editor;
            // its owned Forge stops with it.
            close_editors_first: true,
        }),
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| DevError::Stage {
            stage: "install",
            reason: format!("cannot start the install runtime: {error}"),
        })?
        .block_on(artisan_install::install(options))
        .map_err(DevError::Install)
}
