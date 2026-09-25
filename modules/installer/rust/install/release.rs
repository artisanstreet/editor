//! Installing a release: acquire and verify it from its source, check its
//! channel and versions, stage its payload, and hand it to activation.
//!
//! Split from `workflow.rs`, which keeps activation and the maintenance
//! commands (repair, diagnose, uninstall, prepare-update). Sources live in
//! the sibling `source` module.

use std::path::{Path, PathBuf};

use crate::{
    error::{InstallerError, Result},
    manifest::ReleaseRecord,
    processes::Retirement,
};

use super::{
    authority::{
        EntryKind, InstallerLock, RootMode, StageLease, complete_install_locked,
        ensure_owned_directory, ordinary_metadata, ordinary_path_identity,
    },
    source::{Acquired, Payload, acquire},
    state::recover_activation_pointer_swap,
    workflow::{
        InstallOptions, activate_release, invoke_ae, restore_retired_forge, run_setup_sequence,
    },
};

/// Installs or updates the release `options.source` names into
/// `options.install_root` and activates it.
///
/// # Errors
///
/// Returns [`InstallerError`] when the root is busy or unsafe, the release
/// fails verification or belongs to another channel, an existing release does
/// not match it, a running instance cannot be retired, or activation fails. A
/// failed install leaves the previous release active.
pub async fn install(options: InstallOptions) -> Result<()> {
    let root_lock = InstallerLock::acquire(&options.install_root, RootMode::Create)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let acquired = acquire(&options.source, &options.trust, &options.platform).await?;
    check_channel(&options, &acquired.record)?;
    check_versions(&acquired.record)?;
    let version = acquired.record.product_version.clone();
    let versions = options.install_root.join("versions");
    ensure_owned_directory(&versions)?;
    let release = versions.join(&version);
    let existing = match std::fs::symlink_metadata(&release) {
        Ok(metadata) if ordinary_metadata(&metadata, EntryKind::Directory) => {
            ordinary_path_identity(&release, EntryKind::Directory)
                .map_err(|()| InstallerError::UnsafeOwnedPath)?;
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    };
    if existing {
        return verify_and_activate_existing(&root_lock, &options, &acquired, release).await;
    }
    let stage = options
        .install_root
        .join(format!(".stage-{version}-{}", std::process::id()));
    root_lock.fence()?;
    let mut stage_lease = StageLease::acquire(stage.clone(), &version)?;
    let reuse = active_version_root(&options.install_root);

    let result = async {
        acquired
            .payload
            .materialize(&stage, reuse.as_deref())
            .await?;
        // The tree is final: record per-file digests so `ae doctor` can
        // detect payload drift after activation.
        crate::payload::write_manifest(&stage)?;

        root_lock.fence()?;
        match std::fs::symlink_metadata(&release) {
            Ok(_) => return Err(InstallerError::ExistingRelease(version.clone())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(InstallerError::UnsafeOwnedPath),
        }
        stage_lease.transfer_to(&release)?;
        let retirement = activate_release(&root_lock, &options, &acquired.record, &release)?;
        root_lock.fence()?;
        finish_activation(&options, &release, retirement)
    }
    .await;
    complete_install_locked(&root_lock, &mut stage_lease, result)
}

/// Adopts an existing `versions/<v>` tree only after proving it is the tree
/// the verified release produces. An archive release is re-materialized into
/// a verification stage and compared byte for byte; a tree release is
/// compared against its signed per-file digests. The directory has no
/// signed provenance of its own, so an unprovable tree is refused.
async fn verify_and_activate_existing(
    root_lock: &InstallerLock,
    options: &InstallOptions,
    acquired: &Acquired,
    existing: PathBuf,
) -> Result<()> {
    let version = &acquired.record.product_version;
    if let Payload::Tree { files, .. } = &acquired.payload {
        root_lock.fence()?;
        crate::payload::verify_existing_against_files(&existing, files, version)?;
        let retirement = activate_release(root_lock, options, &acquired.record, &existing)?;
        root_lock.fence()?;
        return finish_activation(options, &existing, retirement);
    }
    let Some(artifact) = acquired.payload.artifact() else {
        return Err(InstallerError::UnverifiedRelease {
            version: version.clone(),
            reason: "the release declares no artifact".to_owned(),
        });
    };
    let stage = options
        .install_root
        .join(format!(".stage-verify-{version}-{}", std::process::id()));
    root_lock.fence()?;
    let mut stage_lease = StageLease::acquire(stage.clone(), version)?;
    let result = async {
        acquired.payload.materialize(&stage, None).await?;
        root_lock.fence()?;
        crate::payload::verify_existing_against_stage(&existing, &stage, artifact, version)?;
        stage_lease.cleanup()?;
        let retirement = activate_release(root_lock, options, &acquired.record, &existing)?;
        root_lock.fence()?;
        restore_retired_forge(options, &existing, retirement)?;
        root_lock.fence()?;
        invoke_ae(&existing, &["--version"])
    }
    .await;
    complete_install_locked(root_lock, &mut stage_lease, result)
}

/// A first install configures Forge; any other install optionally restores
/// the Forge that retirement stopped.
fn finish_activation(
    options: &InstallOptions,
    release: &Path,
    retirement: Retirement,
) -> Result<()> {
    if options.run_setup {
        run_setup_sequence(release)
    } else {
        restore_retired_forge(options, release, retirement)
    }
}

/// A release lands only in an installation of its own channel, and local
/// trust verifies `dev`-channel releases and nothing else, so a locally
/// signed build can never enter a stable or nightly installation.
fn check_channel(options: &InstallOptions, record: &ReleaseRecord) -> Result<()> {
    let mismatch = |expected: &str| {
        InstallerError::InvalidRelease(format!(
            "release channel {} does not match the {expected} channel of this installation",
            record.channel
        ))
    };
    if let Some(expected) = &options.expected_channel
        && *expected != record.channel
    {
        return Err(mismatch(expected));
    }
    if options.trust.is_local() != (record.channel == crate::local::LOCAL_CHANNEL) {
        return Err(InstallerError::InvalidRelease(format!(
            "{} releases must {}be signed with the installation's local key",
            record.channel,
            if options.trust.is_local() { "not " } else { "" }
        )));
    }
    if let Some(installed) = installed_channel(&options.install_root)?
        && installed != record.channel
    {
        return Err(mismatch(&installed));
    }
    Ok(())
}

fn check_versions(record: &ReleaseRecord) -> Result<()> {
    let current_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    let minimum_version = semver::Version::parse(&record.minimum_installer_version)
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    if current_version < minimum_version {
        return Err(InstallerError::InstallerTooOld {
            current: current_version.to_string(),
            minimum: minimum_version.to_string(),
        });
    }
    let parse = |value: &str| {
        semver::Version::parse(value)
            .map_err(|error| InstallerError::InvalidRelease(error.to_string()))
    };
    let product_version = parse(&record.product_version)?;
    let compatibility_version = parse(&record.editor_forge_compatibility_version)?;
    let minimum_cli_version = parse(&record.minimum_cli_version)?;
    if product_version != compatibility_version || product_version < minimum_cli_version {
        return Err(InstallerError::InvalidRelease(
            "product, Editor/Forge compatibility, and minimum CLI versions disagree".to_owned(),
        ));
    }
    Ok(())
}

/// The channel an existing installation at `root` was installed from.
fn installed_channel(root: &Path) -> Result<Option<String>> {
    let path = root.join("installation.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(InstallerError::FileSystem { path, source }),
    };
    let document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)?;
    Ok(document
        .get("channel")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned))
}

/// The active version's directory, whose files a tree install may reuse.
fn active_version_root(root: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(root.join("installation.json")).ok()?;
    let document: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let version = document.get("active_version")?.as_str()?;
    let mut components = Path::new(version).components();
    let single = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    single.then(|| root.join("versions").join(version))
}
