//! Acquiring a signed release: manifest fetch and verification, artifact
//! download and extraction, stage transfer, and hand-off to activation.
//!
//! Split from `workflow.rs`, which keeps activation and the maintenance
//! commands (repair, diagnose, uninstall, prepare-update).

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    archive,
    error::{InstallerError, Result, io},
    manifest::{Artifact, fetch},
    platform::Platform,
};

use super::{
    authority::{
        EntryKind, InstallerLock, RootMode, StageLease, complete_install_locked, create_owned_file,
        ensure_owned_directory, ordinary_metadata, ordinary_path_identity, remove_owned_file,
    },
    state::recover_activation_pointer_swap,
    workflow::{
        InstallOptions, activate_release, invoke_ae, restore_retired_forge, run_setup_sequence,
    },
};

const ABSOLUTE_ARTIFACT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const NATIVE_PAYLOAD_LABEL: &str = "native payload";

/// Installs or updates the release named by the signed manifest at
/// `options.manifest_url` into `options.install_root` and activates it.
///
/// # Errors
///
/// Returns [`InstallerError`] when the root is busy or unsafe, the manifest
/// or artifact fails verification, an existing release does not match the
/// signed artifact, a running instance cannot be retired, or activation
/// fails. A failed install leaves the previous release active.
#[allow(clippy::too_many_lines)]
pub async fn install(options: InstallOptions) -> Result<()> {
    let root_lock = InstallerLock::acquire(&options.install_root, RootMode::Create)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    // Plain HTTP is permitted only from this machine's own loopback, which
    // cannot be intercepted off-host. A locally built, locally signed release
    // is installed by serving its output directory on 127.0.0.1; every remote
    // manifest still requires TLS, and the signature check applies to both.
    let loopback_manifest = options.manifest_url.host().is_some_and(|host| match host {
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
        url::Host::Domain(domain) => domain == "localhost",
    });
    let client = reqwest::Client::builder()
        .https_only(!loopback_manifest)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(InstallerError::ManifestRequest)?;
    let artifact_base_url = options
        .manifest_url
        .join("./")
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    let manifest = fetch(
        &client,
        options.manifest_url.clone(),
        options.signature_url.clone(),
        &options.trust,
    )
    .await?;
    let current_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    let minimum_version = semver::Version::parse(&manifest.minimum_installer_version)
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    if current_version < minimum_version {
        return Err(InstallerError::InstallerTooOld {
            current: current_version.to_string(),
            minimum: minimum_version.to_string(),
        });
    }
    let product_version = semver::Version::parse(&manifest.product_version)
        .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
    let compatibility_version =
        semver::Version::parse(&manifest.editor_forge_compatibility_version)
            .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
    let minimum_cli_version = semver::Version::parse(&manifest.minimum_cli_version)
        .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
    if product_version != compatibility_version || product_version < minimum_cli_version {
        return Err(InstallerError::InvalidRelease(
            "product, Editor/Forge compatibility, and minimum CLI versions disagree".to_owned(),
        ));
    }
    let versions = options.install_root.join("versions");
    ensure_owned_directory(&versions)?;
    let existing_release = versions.join(&manifest.product_version);
    let existing_release = match std::fs::symlink_metadata(&existing_release) {
        Ok(metadata) if ordinary_metadata(&metadata, EntryKind::Directory) => {
            ordinary_path_identity(&existing_release, EntryKind::Directory)
                .map_err(|()| InstallerError::UnsafeOwnedPath)?;
            Some(existing_release)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    };
    if let Some(existing_release) = existing_release {
        return verify_and_activate_existing(
            &root_lock,
            &client,
            &options,
            &manifest,
            &artifact_base_url,
            existing_release,
        )
        .await;
    }
    let stage = options.install_root.join(format!(
        ".stage-{}-{}",
        manifest.product_version,
        std::process::id()
    ));
    root_lock.fence()?;
    let mut stage_lease = StageLease::acquire(stage.clone(), &manifest.product_version)?;

    let result = async {
        let artifact = native_artifact(&manifest, &options.platform)?;
        let artifact_url = artifact_url(&artifact_base_url, artifact)?;
        install_artifact(&client, artifact, artifact_url, &stage).await?;
        // The tree is final: record per-file digests so `ae doctor` can
        // detect payload drift after activation.
        crate::payload::write_manifest(&stage)?;

        root_lock.fence()?;
        let release = versions.join(&manifest.product_version);
        match std::fs::symlink_metadata(&release) {
            Ok(_) => {
                return Err(InstallerError::ExistingRelease(
                    manifest.product_version.clone(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(InstallerError::UnsafeOwnedPath),
        }
        stage_lease.transfer_to(&release)?;
        let retirement = activate_release(&root_lock, &options, &manifest, &release)?;
        if options.run_setup {
            root_lock.fence()?;
            run_setup_sequence(&release)?;
        } else {
            root_lock.fence()?;
            restore_retired_forge(&options, &release, retirement)?;
        }
        Ok(())
    }
    .await;
    complete_install_locked(&root_lock, &mut stage_lease, result)
}

/// Adopts an existing `versions/<v>` tree only after the signed release
/// artifact has been re-downloaded, its checksum verified, and its extracted
/// tree compared byte for byte with the tree on disk. The tree has no signed
/// provenance of its own, so re-deriving it is the only sound way to activate
/// it; an unverifiable or dev-staged tree is refused instead.
async fn verify_and_activate_existing(
    root_lock: &InstallerLock,
    client: &reqwest::Client,
    options: &InstallOptions,
    manifest: &crate::manifest::ReleaseManifest,
    artifact_base_url: &Url,
    existing_release: PathBuf,
) -> Result<()> {
    let stage = options.install_root.join(format!(
        ".stage-verify-{}-{}",
        manifest.product_version,
        std::process::id()
    ));
    root_lock.fence()?;
    let mut stage_lease = StageLease::acquire(stage.clone(), &manifest.product_version)?;
    let result = async {
        let artifact = native_artifact(manifest, &options.platform)?;
        let artifact_url = artifact_url(artifact_base_url, artifact)?;
        install_artifact(client, artifact, artifact_url, &stage).await?;
        root_lock.fence()?;
        crate::payload::verify_existing_against_stage(
            &existing_release,
            &stage,
            artifact,
            &manifest.product_version,
        )?;
        stage_lease.cleanup()?;
        let retirement = activate_release(root_lock, options, manifest, &existing_release)?;
        root_lock.fence()?;
        restore_retired_forge(options, &existing_release, retirement)?;
        root_lock.fence()?;
        invoke_ae(&existing_release, &["--version"])
    }
    .await;
    complete_install_locked(root_lock, &mut stage_lease, result)
}

fn native_artifact<'a>(
    manifest: &'a crate::manifest::ReleaseManifest,
    platform: &Platform,
) -> Result<&'a Artifact> {
    manifest
        .artifacts
        .iter()
        .find(|artifact| {
            artifact.platform == platform.os
                && artifact.architecture == platform.arch
                && (platform.os != "linux" || artifact.libc.as_deref() == Some(platform_libc()))
        })
        .ok_or_else(|| InstallerError::MissingArtifact {
            component: NATIVE_PAYLOAD_LABEL.to_owned(),
            target: platform.target(),
        })
}

fn artifact_url(base: &Url, artifact: &Artifact) -> Result<Url> {
    base.join(&artifact.file_name)
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))
}

async fn install_artifact(
    client: &reqwest::Client,
    artifact: &Artifact,
    artifact_url: Url,
    stage: &Path,
) -> Result<()> {
    if artifact.size == 0 || artifact.size > ABSOLUTE_ARTIFACT_LIMIT {
        return Err(InstallerError::ArtifactTooLarge {
            url: artifact_url.clone(),
        });
    }
    let response = client
        .get(artifact_url.clone())
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| InstallerError::ArtifactRequest {
            url: artifact_url.clone(),
            source,
        })?;
    if response
        .content_length()
        .is_some_and(|size| size > artifact.size)
    {
        return Err(InstallerError::ArtifactTooLarge {
            url: artifact_url.clone(),
        });
    }
    let download = stage.join(format!(".{}.download", artifact.id));
    let mut file = create_owned_file(&download)?;
    let mut response = response;
    let mut downloaded = 0_u64;
    let mut hasher = Sha256::new();
    while let Some(chunk) =
        response
            .chunk()
            .await
            .map_err(|source| InstallerError::ArtifactRequest {
                url: artifact_url.clone(),
                source,
            })?
    {
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > artifact.size {
            return Err(InstallerError::ArtifactTooLarge {
                url: artifact_url.clone(),
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(io(&download))?;
    }
    if downloaded != artifact.size {
        return Err(InstallerError::ArtifactSizeMismatch {
            expected: artifact.size,
            actual: downloaded,
        });
    }
    file.sync_all().map_err(io(&download))?;
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(InstallerError::ChecksumMismatch(artifact_url));
    }
    archive::extract(&download, artifact.format, stage, &artifact.archive_entries)?;
    remove_owned_file(&download)?;
    Ok(())
}

pub(crate) fn platform_libc() -> &'static str {
    if cfg!(target_env = "musl") {
        "musl"
    } else {
        "glibc"
    }
}
