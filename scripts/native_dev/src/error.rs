//! Bounded failures for the native dev launcher.
//!
//! Diagnostics name stages and paths but never credential material: paths
//! are operational context, secrets never cross this boundary.

use std::path::PathBuf;

use thiserror::Error;

/// Bounded failure for one dev stage.
#[derive(Debug, Error)]
pub enum DevError {
    /// Command-line usage was invalid.
    #[error("invalid arguments: {reason}")]
    Usage {
        /// What was wrong with the invocation.
        reason: String,
    },
    /// A required path was not absolute.
    #[error("path must be absolute: {path}")]
    NotAbsolute {
        /// The offending path.
        path: PathBuf,
    },
    /// A staged binary could not be located.
    #[error("dev binary missing: {name} ({hint})")]
    BinaryMissing {
        /// Binary file name that was not found.
        name: String,
        /// Where the search looked.
        hint: String,
    },
    /// The dev root is on a network share, where locks and hardlinks are
    /// unreliable.
    #[error(
        "dev root {path} is on a network share; install on local disk instead (pass --root or set ARTISAN_DEV_ROOT)"
    )]
    NetworkShare {
        /// The shared dev root.
        path: PathBuf,
    },
    /// Another `dev` run holds the runner lock.
    #[error("another dev run is installing on this root; wait for it to finish ({lock})")]
    StagingLocked {
        /// Lock file that is held.
        lock: PathBuf,
    },
    /// A dev stage failed with a bounded reason.
    #[error("stage {stage} failed: {reason}")]
    Stage {
        /// Stage that failed.
        stage: &'static str,
        /// Bounded human-readable reason.
        reason: String,
    },
    /// The installer refused or failed to install the dev payload.
    #[error("install failed: {0}")]
    Install(#[source] artisan_install::InstallerError),
    /// A previous dev Forge still owns the dev home.
    #[error(
        "previous dev Forge still running with pid {pid}; close the previous dev session first"
    )]
    PreviousForgeRunning {
        /// Live Forge process identity from the readiness receipt.
        pid: u32,
    },
    /// Forge custody is held on this home; a live Forge may own it.
    #[error("forge custody is held at {path}; a live Forge may own this home, receipt preserved")]
    CustodyHeld {
        /// Custody file another owner holds.
        path: PathBuf,
    },
    /// Startup confirmation failed after launch.
    #[error("editor startup not confirmed: {reason}")]
    StartupUnconfirmed {
        /// Bounded reason: failed stage, timeout, or early exit.
        reason: String,
    },
    /// The staged Editor exited abnormally.
    #[error("staged editor exited with {status}")]
    EditorStatus {
        /// Exit status text.
        status: String,
    },
}
