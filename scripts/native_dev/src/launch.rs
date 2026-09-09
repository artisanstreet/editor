//! Launching the staged Editor with bounded startup confirmation.
//!
//! The launcher spawns the **staged** Editor (`<home>/versions/dev`), never
//! a build-output binary, and refuses to stage while the readiness receipt
//! identifies a Forge running the staged binary. After spawning, it waits
//! for the opt-in startup receipt the Editor's own transport service
//! writes once authenticated initial queries complete — the existing
//! startup signal, not a separate probe, so no bootstrap credential is
//! ever consumed outside the owned session. On failure or timeout the
//! launcher stops the Editor it owns (which releases the owned Forge
//! through the Editor's lease and Job Object containment) and reports an
//! honest stage result instead of an exit-code wait.

use std::{
    path::{Path, PathBuf},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use artisan_editor_cli::process::{self, ForgeReadinessStatus};

use crate::{
    error::DevError,
    paths::{DEV_HOME_ENV, DevPaths, STRIPPED_DEV_HOME_ENV, STRIPPED_DEV_READY_ENV, exe_name},
};

/// Environment variable selecting the Editor's startup receipt file.
///
/// Must match `dev_startup_receipt::STARTUP_RECEIPT_ENV` in
/// `modules/frontend`; the contract test pins the literal on both sides.
pub const STARTUP_RECEIPT_ENV: &str = "ARTISAN_DEV_STARTUP_RECEIPT";

/// Schema marker the Editor writes into every receipt.
pub const STARTUP_RECEIPT_SCHEMA: &str = "artisan-dev-startup-v1";

/// Bounded wait for the Editor's startup receipt, in milliseconds.
///
/// A cold Forge can take well over 30 seconds to initialize durable state
/// before the first authenticated queries complete.
pub const DEV_STARTUP_TIMEOUT_MS: u64 = 90_000;

/// Poll interval while waiting for the startup receipt, in milliseconds.
pub const DEV_STARTUP_POLL_MS: u64 = 100;

/// Maximum receipts-stage text retained for diagnostics.
pub const MAX_RECEIPT_TEXT: usize = 256;

/// Per-launch receipt path inside the dev directory.
///
/// The process identity makes each launch's receipt unique, so a stale
/// receipt from a crashed run can never confirm a new launch.
#[must_use]
pub fn fresh_receipt_path(paths: &DevPaths) -> PathBuf {
    paths
        .dev_dir
        .join(format!("startup-receipt-{}.json", std::process::id()))
}

/// Removes a stale receipt before spawning the Editor.
///
/// A missing file is the expected first-launch state. Any other removal
/// failure aborts the launch: silently waiting on an unreadable path
/// would read a stale `ready` first.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when an existing receipt cannot be removed.
pub fn clear_stale_receipt(path: &Path) -> Result<(), DevError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DevError::Stage {
            stage: "launch",
            reason: format!(
                "cannot clear stale startup receipt {}: {error}",
                path.display()
            ),
        }),
    }
}

/// Staged Editor binary launched by every run.
#[must_use]
pub fn staged_editor(paths: &DevPaths) -> PathBuf {
    paths.version_bin.join(exe_name("editor"))
}

/// Staged Forge binary the production startup launches.
#[must_use]
pub fn staged_forge(paths: &DevPaths) -> PathBuf {
    paths.version_bin.join(exe_name("forge"))
}

/// Outcome of waiting for the startup receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartupWait {
    /// The Editor confirmed authenticated initial-query completion.
    Ready {
        /// Receipt stage text.
        stage: String,
    },
    /// The Editor reported a startup failure.
    Failed {
        /// Secret-free failure stage.
        stage: String,
        /// Secret-free failure detail.
        reason: String,
    },
    /// No receipt arrived before the deadline.
    Timeout,
    /// The Editor exited before confirming startup.
    EditorExited {
        /// Exit code when available.
        code: Option<i32>,
    },
}

/// Refuses staging when a previous dev Forge still owns the dev home.
///
/// A stale readiness receipt is fine: the staged Editor's owned startup
/// replaces it the way production does.
///
/// # Errors
///
/// Returns [`DevError::PreviousForgeRunning`] when the readiness receipt
/// identifies a live Forge running the staged binary.
pub fn refuse_live_forge(paths: &DevPaths, forge_exe: &Path) -> Result<(), DevError> {
    match process::readiness_status(&paths.readiness_path(), forge_exe) {
        ForgeReadinessStatus::Ready(readiness) => Err(DevError::PreviousForgeRunning {
            pid: readiness.pid(),
        }),
        ForgeReadinessStatus::Missing | ForgeReadinessStatus::Invalid => Ok(()),
    }
}

/// Spawns the staged Editor on the dev home.
///
/// The child inherits the environment with `ARTISAN_HOME` pointed at the
/// dev home, the manual-forge escape hatches removed, and the startup
/// receipt path set, so the Editor always exercises its owned Forge
/// custody path. Standard streams are inherited so Editor output stays
/// visible.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the Editor cannot be spawned.
pub fn spawn_editor(
    editor_exe: &Path,
    home: &Path,
    receipt_path: &Path,
) -> Result<Child, DevError> {
    let mut command = std::process::Command::new(editor_exe);
    command
        .env(DEV_HOME_ENV, home)
        .env(STARTUP_RECEIPT_ENV, receipt_path)
        .env_remove(STRIPPED_DEV_HOME_ENV)
        .env_remove(STRIPPED_DEV_READY_ENV)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command.spawn().map_err(|_| DevError::Stage {
        stage: "launch",
        reason: format!("cannot launch {}", editor_exe.display()),
    })
}

/// Reads one receipt file without blocking.
///
/// Returns `None` when the file is absent or not yet a valid receipt
/// document; the caller keeps polling until the deadline.
#[must_use]
pub fn read_receipt(path: &Path) -> Option<StartupWait> {
    let bytes = std::fs::read(path).ok()?;
    let document: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if document.get("schema")?.as_str()? != STARTUP_RECEIPT_SCHEMA {
        return None;
    }
    let stage = document
        .get("stage")?
        .as_str()?
        .chars()
        .take(MAX_RECEIPT_TEXT)
        .collect::<String>();
    match document.get("status")?.as_str()? {
        "ready" => Some(StartupWait::Ready { stage }),
        "failed" => {
            let reason = document
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("startup failed")
                .chars()
                .take(MAX_RECEIPT_TEXT)
                .collect::<String>();
            Some(StartupWait::Failed { stage, reason })
        }
        _ => None,
    }
}

/// Waits for the startup receipt while the Editor is alive.
///
/// Polls the receipt file until it parses, the Editor exits, or the
/// deadline passes. Never consumes Forge credentials: the receipt is the
/// Editor's own startup signal.
pub fn wait_for_startup(child: &mut Child, receipt_path: &Path, timeout: Duration) -> StartupWait {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(outcome) = read_receipt(receipt_path) {
            return outcome;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return StartupWait::EditorExited {
                    code: status.code(),
                };
            }
            Ok(None) => {}
            Err(_) => {
                return StartupWait::EditorExited { code: None };
            }
        }
        if Instant::now() >= deadline {
            return StartupWait::Timeout;
        }
        std::thread::sleep(
            Duration::from_millis(DEV_STARTUP_POLL_MS).min(
                deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or(Duration::ZERO),
            ),
        );
    }
}

/// Stops an owned Editor after unconfirmed startup.
///
/// Best-effort: killing the Editor releases its owned Forge through the
/// Editor's lease and Job Object containment. Returns the Editor's exit
/// code when the wait completes.
pub fn stop_editor(mut child: Child) -> Option<i32> {
    let _ = child.kill();
    child.wait().ok().and_then(|status| status.code())
}
