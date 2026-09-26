//! The Forge half of `nix run .#dev`: the Linux dev installation and its
//! Forge service.
//!
//! The Linux stage payload is signed and installed through the shipping
//! installer, retiring the running Forge the way an update does. The
//! installed `ae` then configures the Forge (`ae setup ... --autostart`,
//! which writes the systemd user unit) and starts it (`ae start`, through
//! the user manager). The runner waits until the new Forge has published
//! its host invitation, which the Editor half registers.
//!
//! The per-user environment belongs to the default installation only: for
//! it, `ae` is linked onto the PATH and a hand-deployed Forge (and a
//! hand-added `ae` in the Nix profile) is adopted. A scratch `--root`
//! leaves the user's environment alone.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use artisan_build_info::BuildInfo;
use artisan_editor_cli::{
    credentials::hosts::HostInvitation,
    host_access::INVITATION_FILE,
    process::{self, ForgeReadinessStatus},
    service::{ForgeService, UserSystemctl},
};
use artisan_install::LocalSigner;

use crate::{
    Progress,
    binaries::locate_binaries,
    error::DevError,
    legacy::{self, LegacyForge},
    lock::DevLock,
    paths::DevPaths,
    payload::{install_payload, payload_identity, sign_payload},
    provision::{HostAccess, setup_arguments},
};

/// How long a started Forge may take to publish its invitation.
const INVITATION_TIMEOUT: Duration = Duration::from_secs(90);

/// What the Forge half needs.
#[derive(Clone, Debug)]
pub struct HostOptions {
    /// The Linux dev installation.
    pub paths: DevPaths,
    /// The Linux stage payload.
    pub payload: PathBuf,
    /// Where the Forge listens and what its invitation is called.
    pub access: HostAccess,
    /// Inactive versions kept.
    pub keep: usize,
    /// Whether this is the user's default dev installation, which owns the
    /// PATH link and adopts a hand-deployed Forge.
    pub default_root: bool,
}

/// The deployed Forge.
#[derive(Clone, Debug)]
pub struct HostDeployment {
    /// The installed build.
    pub identity: BuildInfo,
    /// The published host invitation.
    pub invitation: PathBuf,
}

/// Installs the Linux payload, configures and starts its Forge service, and
/// waits for the Forge's host invitation.
///
/// # Errors
///
/// Returns [`DevError`] when any step fails; the previously active version
/// stays active when installation fails.
pub fn deploy(options: &HostOptions, progress: &mut Progress) -> Result<HostDeployment, DevError> {
    let paths = &options.paths;
    let identity = payload_identity(&options.payload)?;
    locate_binaries(Some(&options.payload.join("bin")))?;
    let lock = DevLock::acquire(paths)?;
    let signer = LocalSigner::load_or_create(&paths.home).map_err(DevError::Install)?;
    let manifests = paths.runner_dir().join("manifest");
    sign_payload(&options.payload, &manifests, &identity, &signer)?;
    progress.stage("sign", signer.key_id());

    let register_path = options.default_root && path_link_is_free(paths);
    install_payload(paths, &options.payload, &manifests, &signer, register_path)?;
    progress.stage("install", &crate::describe(&identity));

    if options.default_root {
        adopt_legacy(paths, progress)?;
    }
    let ae = &paths.permanent_ae;
    run_ae(ae, &setup_arguments(paths, &options.access))?;
    let service = ForgeService::for_current_user(&paths.home)
        .map_err(|error| stage_error("configure", error.to_string()))?;
    progress.stage(
        "configure",
        &format!(
            "{}, listening on {} as {}",
            service.unit_name(),
            options.access.listen,
            options.access.name
        ),
    );

    run_ae(ae, &["start".into()])?;
    let version_root = paths.active_version_root()?;
    let invitation = wait_for_invitation(
        &paths.home,
        &paths.readiness_path(),
        &version_root.join("bin").join("forge"),
        INVITATION_TIMEOUT,
    )
    .map_err(|error| {
        stage_error(
            "start",
            format!(
                "{error}; see `journalctl --user -u {}`",
                service.unit_name()
            ),
        )
    })?;
    progress.stage("start", &invitation.display().to_string());
    drop(lock);

    crate::prune(paths, options.keep);
    if options.default_root {
        remove_profile_ae(progress);
    }
    Ok(HostDeployment {
        identity,
        invitation,
    })
}

/// `~/.local/bin/ae` is free, or already this installation's.
fn path_link_is_free(paths: &DevPaths) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let link = home.join(".local").join("bin").join("ae");
    match std::fs::read_link(&link) {
        Ok(target) => target == paths.permanent_ae,
        Err(_) if link.symlink_metadata().is_ok() => false,
        Err(_) => true,
    }
}

fn adopt_legacy(paths: &DevPaths, progress: &mut Progress) -> Result<(), DevError> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Ok(());
    };
    let variable = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let legacy = LegacyForge::for_user(
        &home,
        variable("XDG_STATE_HOME").as_deref(),
        variable("XDG_CONFIG_HOME").as_deref(),
    );
    if let Some(adoption) =
        legacy::adopt(&legacy, paths, &UserSystemctl, &legacy::checkpoint_database)?
    {
        progress.stage(
            "adopt",
            &format!(
                "{} moved, {} GC roots removed{}; backup in {}",
                adoption.moved.len(),
                adoption.gc_roots_removed,
                if adoption.unit_retired {
                    ", artisan-forge.service retired"
                } else {
                    ""
                },
                adoption.backup.display()
            ),
        );
    }
    Ok(())
}

/// Removes a hand-added Artisan `ae` from the user's Nix profile, which
/// would otherwise shadow the installation's.
fn remove_profile_ae(progress: &mut Progress) {
    match legacy::remove_profile_ae(&legacy::UserNixProfile) {
        Ok(removed) if !removed.is_empty() => {
            progress.stage("profile", &format!("removed {}", removed.join(", ")));
        }
        Ok(_) => {}
        Err(error) => eprintln!("dev: warning: the Nix profile was left alone: {error}"),
    }
}

/// Runs the installation's permanent `ae`, which discovers the installation
/// from its own location.
fn run_ae(ae: &Path, arguments: &[OsString]) -> Result<(), DevError> {
    let status = Command::new(ae)
        .args(arguments)
        .env_remove("ARTISAN_HOME")
        .env_remove("ARTISAN_INSTALL_ROOT")
        .stdin(Stdio::null())
        .status()
        .map_err(|error| {
            stage_error("configure", format!("cannot run {}: {error}", ae.display()))
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(stage_error(
            "configure",
            format!(
                "`ae {}` failed ({status})",
                arguments
                    .first()
                    .map_or_else(String::new, |word| word.to_string_lossy().into_owned())
            ),
        ))
    }
}

/// Waits until the Forge running `forge` is ready and its invitation in
/// `home` names that exact process, so the Editor never registers a stale
/// incarnation.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when no current invitation appears in time.
pub fn wait_for_invitation(
    home: &Path,
    readiness: &Path,
    forge: &Path,
    timeout: Duration,
) -> Result<PathBuf, DevError> {
    let invitation = home.join(INVITATION_FILE);
    let deadline = Instant::now() + timeout;
    loop {
        if let ForgeReadinessStatus::Ready(ready) = process::readiness_status(readiness, forge)
            && invitation_names(&invitation, ready.pid())
        {
            return Ok(invitation);
        }
        if Instant::now() >= deadline {
            return Err(stage_error(
                "start",
                format!(
                    "the Forge published no current invitation at {} within {}s",
                    invitation.display(),
                    timeout.as_secs()
                ),
            ));
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// Whether the invitation at `path` describes Forge process `pid`.
#[must_use]
pub fn invitation_names(path: &Path, pid: u32) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| HostInvitation::decode(&bytes).ok())
        .is_some_and(|invitation| invitation.pid == pid)
}

fn stage_error(stage: &'static str, reason: String) -> DevError {
    DevError::Stage { stage, reason }
}
