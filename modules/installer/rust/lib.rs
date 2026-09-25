//! `artisan-install`: the one implementation of verifying, staging,
//! activating, repairing, and removing Artisan installations.
//!
//! The standalone `installer` binary, the permanent `ae` launcher, and the
//! `dev` runner all drive installations through this library, so a local
//! development build is activated by exactly the code path a signed release
//! update takes.

mod archive;
mod background_process;
mod error;
mod install;
mod integrations;
mod local;
mod manifest;
mod payload;
mod platform;
mod processes;
mod shortcuts;

pub use error::{InstallerError, Result};
pub use install::{
    InstallIntegrationOptions, InstallOptions, PruneReport, RELEASE_MANIFEST_NAME,
    RELEASE_SIGNATURE_NAME, ReleaseSource, diagnose, install, prepare_update, prune, repair,
    uninstall,
};
pub use local::{LOCAL_CHANNEL, LOCAL_TRUST_DIRECTORY, LocalRelease, LocalSigner, local_trust};
pub use manifest::{TREE_MANIFEST_NAME, TREE_SIGNATURE_NAME, TrustKey};
pub use platform::{Platform, resolve_install_root};
pub use processes::RetirementPolicy;

#[cfg(debug_assertions)]
pub use platform::forbid_default_install_root;

/// Schedules removal of the running installer executable after it exits.
///
/// Only an executable inside the system temporary directory (a downloaded
/// bootstrap) may be removed this way.
///
/// # Errors
///
/// Returns [`InstallerError`] when the executable or temporary directory
/// cannot be resolved, the executable lies outside the temporary directory,
/// or the cleanup helper cannot be started.
pub fn schedule_self_cleanup() -> Result<()> {
    let executable = std::env::current_exe().map_err(InstallerError::CurrentExecutable)?;
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .map_err(InstallerError::TemporaryDirectory)?;
    let executable = executable
        .canonicalize()
        .map_err(InstallerError::CurrentExecutable)?;
    if !executable.starts_with(&temporary_root) {
        return Err(InstallerError::UnsafeSelfCleanup(executable));
    }

    #[cfg(windows)]
    {
        background_process::detached_background_command("cmd.exe")
            .args([
                "/d",
                "/s",
                "/c",
                "ping 127.0.0.1 -n 3 > nul & del /f /q \"%ARTISAN_BOOTSTRAP_DELETE%\"",
            ])
            .env("ARTISAN_BOOTSTRAP_DELETE", &executable)
            .spawn()
            .map_err(InstallerError::CleanupHelper)?;
    }
    #[cfg(unix)]
    {
        std::process::Command::new("sh")
            .args([
                "-c",
                "sleep 1; rm -f -- \"$1\"",
                "ae-installer-cleanup",
                executable
                    .to_str()
                    .ok_or_else(|| InstallerError::NonUtf8Path(executable.clone()))?,
            ])
            .spawn()
            .map_err(InstallerError::CleanupHelper)?;
    }
    Ok(())
}
