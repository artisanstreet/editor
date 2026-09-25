//! Turning a local build into an installed dev release.
//!
//! The runner never copies binaries into an installation itself. It
//! assembles the build outputs and their identity into an unpacked payload
//! tree, signs a tree manifest with the dev root's local key, and hands the
//! tree to `artisan-install`, the same code that installs published
//! releases. Retiring a running dev Editor and Forge, activation, and
//! rollback therefore behave exactly as they do for a real update.

use std::{fs, io::Read, path::Path};

use artisan_build_info::{BuildInfo, Channel, FORMAT_VERSION};
use artisan_install::{
    InstallIntegrationOptions, InstallOptions, LOCAL_CHANNEL, LocalRelease, LocalSigner, Platform,
    ReleaseSource, RetirementPolicy,
};
use sha2::{Digest, Sha256};

use crate::{
    binaries::BinarySet,
    error::DevError,
    identity::{GitState, dev_version, runner_target},
    paths::DevPaths,
};

/// Directory inside the Cargo target directory where payloads are assembled.
pub const PAYLOAD_DIRECTORY: &str = "artisan-dev-payload";

/// Lowercase hex SHA-256 of one file.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the file cannot be read.
pub fn hash_file(path: &Path) -> Result<String, DevError> {
    let unreadable = || DevError::Stage {
        stage: "assemble",
        reason: format!("cannot read {}", path.display()),
    };
    let mut file = fs::File::open(path).map_err(|_| unreadable())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| unreadable())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Version of a payload: the checkout's dev version plus a digest of the
/// binaries, so equal bytes always share a version (re-installing them is a
/// no-op re-activation) and different bytes never do, even for rebuilds of
/// the same commit.
#[must_use]
pub fn payload_version(git: &GitState, binaries_digest: &str) -> String {
    let base = dev_version(env!("CARGO_PKG_VERSION"), git);
    let digest = binaries_digest.get(..10).unwrap_or(binaries_digest);
    if base.contains('+') {
        format!("{base}.b{digest}")
    } else {
        format!("{base}+b{digest}")
    }
}

/// Digest over the binaries' own digests, in payload order.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when a binary cannot be read.
pub fn binaries_digest(binaries: &BinarySet) -> Result<String, DevError> {
    let mut hasher = Sha256::new();
    for (relative, source) in binaries.entries() {
        hasher.update(relative.as_bytes());
        hasher.update(b"\0");
        hasher.update(hash_file(&source)?.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Assembles `binaries` and their identity into a signed payload tree at
/// `tree`, replacing any previous assembly there.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the tree cannot be written or signed.
pub fn assemble(
    binaries: &BinarySet,
    git: &GitState,
    profile: &str,
    tree: &Path,
    signer: &LocalSigner,
) -> Result<BuildInfo, DevError> {
    let failed = |reason: String| DevError::Stage {
        stage: "assemble",
        reason,
    };
    if tree.exists() {
        fs::remove_dir_all(tree)
            .map_err(|error| failed(format!("cannot clear {}: {error}", tree.display())))?;
    }
    for (relative, source) in binaries.entries() {
        let destination = tree.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| failed(format!("cannot create {}: {error}", parent.display())))?;
        }
        // The tree is read, never executed, so linking build outputs is safe
        // here; the installer copies them into the installation.
        if fs::hard_link(&source, &destination).is_err() {
            fs::copy(&source, &destination)
                .map_err(|error| failed(format!("cannot copy {}: {error}", source.display())))?;
        }
    }
    let info = BuildInfo {
        format_version: FORMAT_VERSION,
        version: payload_version(git, &binaries_digest(binaries)?),
        channel: Channel::Dev,
        commit: git.commit.clone(),
        dirty: git.dirty,
        profile: profile.to_owned(),
        target: runner_target().to_owned(),
        built_at: None,
    };
    let identity = tree.join(artisan_build_info::RESOURCE_PATH);
    if let Some(parent) = identity.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| failed(format!("cannot create {}: {error}", parent.display())))?;
    }
    fs::write(&identity, info.to_json())
        .map_err(|error| failed(format!("cannot write {}: {error}", identity.display())))?;
    let platform = Platform::detect().map_err(|error| failed(error.to_string()))?;
    signer
        .write_tree_manifest(
            tree,
            &LocalRelease {
                product_version: info.version.clone(),
                platform,
            },
        )
        .map_err(|error| failed(format!("cannot sign the payload: {error}")))?;
    Ok(info)
}

/// Installs the signed tree into the dev root as a `dev`-channel release.
///
/// A running dev Editor is closed and its Forge stopped first, exactly as a
/// release update retires superseded instances; PATH, shortcuts, and the
/// `artisan://` handler stay with the real installation.
///
/// # Errors
///
/// Returns [`DevError::Install`] when the installer refuses or fails.
pub fn install_tree(paths: &DevPaths, tree: &Path, signer: &LocalSigner) -> Result<(), DevError> {
    let platform = Platform::detect().map_err(DevError::Install)?;
    let options = InstallOptions {
        source: ReleaseSource::Tree {
            path: tree.to_path_buf(),
            manifest_directory: None,
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
