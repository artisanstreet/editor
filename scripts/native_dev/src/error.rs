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
    /// Another `dev` run holds the staging lock.
    #[error(
        "dev staging is locked by another run; wait for it or remove {lock} if no dev run is active"
    )]
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
    /// The payload gate rejected the staged version.
    #[error("staged payload is not verified: {issues}")]
    PayloadUnverified {
        /// Integrity findings from the existing verifier.
        issues: String,
    },
    /// A previous dev Forge still owns the dev home.
    #[error(
        "previous dev Forge still running with pid {pid}; close the previous dev session first"
    )]
    PreviousForgeRunning {
        /// Live Forge process identity from the readiness receipt.
        pid: u32,
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
