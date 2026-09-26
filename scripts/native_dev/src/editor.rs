//! The Editor half of `nix run .#dev`, run on the Editor's platform.
//!
//! When the Editor has an installation of its own (the Windows Editor of a
//! WSL Forge), its payload is signed and installed through the shipping
//! installer first, closing a running dev Editor the way an update does.
//! The Forge's host invitation is then registered exactly as the Editor's
//! own import does, and the installed Editor is launched on that host and
//! confirmed through its startup receipt.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

use artisan_editor_cli::credentials::hosts;
use artisan_install::LocalSigner;

use crate::{
    Progress,
    binaries::locate_binaries,
    error::DevError,
    launch::{
        DEV_STARTUP_TIMEOUT_MS, EditorOutput, EditorProcess, StartupWait, clear_stale_receipt,
        editor_log_path, editor_output, fresh_receipt_path, spawn_editor, staged_editor,
        streams_are_terminals, wait_for_startup,
    },
    lock::DevLock,
    paths::DevPaths,
    payload::{install_payload, payload_identity, sign_payload},
};

/// What the Editor half needs.
#[derive(Clone, Debug)]
pub struct EditorOptions {
    /// The Editor's installation.
    pub paths: DevPaths,
    /// A payload to install, when the Editor has its own installation.
    pub payload: Option<PathBuf>,
    /// The Forge's host invitation.
    pub invitation: PathBuf,
    /// Whether to launch the Editor.
    pub launch: bool,
    /// Follow the launched Editor until it exits.
    pub attach: bool,
    /// Inactive versions kept.
    pub keep: usize,
}

/// How the Editor half ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorOutcome {
    /// Installed and registered, not launched.
    Staged,
    /// Launched and confirmed; it keeps running on its own.
    Running {
        /// The Editor's process id.
        pid: u32,
    },
    /// Followed until it exited with this code.
    Exited {
        /// Exit code.
        code: i32,
    },
}

/// Installs (when given a payload), registers the host, and launches.
///
/// # Errors
///
/// Returns [`DevError`] when installing, registering, or launching fails,
/// or the Editor does not confirm startup.
pub fn deploy(options: &EditorOptions, progress: &mut Progress) -> Result<EditorOutcome, DevError> {
    let paths = &options.paths;
    let lock = DevLock::acquire(paths)?;
    if let Some(payload) = &options.payload {
        let identity = payload_identity(payload)?;
        locate_binaries(Some(&payload.join("bin")))?;
        let signer = LocalSigner::load_or_create(&paths.home).map_err(DevError::Install)?;
        let manifests = paths.runner_dir().join("manifest");
        sign_payload(payload, &manifests, &identity, &signer)?;
        progress.stage("sign", signer.key_id());
        install_payload(paths, payload, &manifests, &signer, false)?;
        progress.stage("install", &crate::describe(&identity));
        crate::prune(paths, options.keep);
    }
    let host = hosts::import_file(&options.invitation).map_err(|error| DevError::Stage {
        stage: "host",
        reason: format!(
            "cannot register the invitation {}: {error}",
            options.invitation.display()
        ),
    })?;
    progress.stage("host", &host.display().to_string());

    let version_root = paths.active_version_root()?;
    let editor = staged_editor(&version_root);
    if !options.launch {
        println!(
            "dev: installed without launching; run {} --host-home {}",
            editor.display(),
            host.display()
        );
        return Ok(EditorOutcome::Staged);
    }
    let receipt = fresh_receipt_path(paths);
    clear_stale_receipt(&receipt)?;
    let output = editor_output(
        options.attach,
        streams_are_terminals(),
        editor_log_path(paths),
    );
    let arguments: [OsString; 2] = ["--host-home".into(), host.into_os_string()];
    let mut process = spawn_editor(&editor, &paths.home, &receipt, &output, &arguments)?;
    progress.stage("launch", &editor.display().to_string());
    // Installs are serialized only up to here: the lock is never held for
    // the Editor's lifetime, so the next run can retire this Editor.
    drop(lock);
    let startup = wait_for_startup(
        process.child_mut(),
        &receipt,
        Duration::from_millis(DEV_STARTUP_TIMEOUT_MS),
    );
    let _ = std::fs::remove_file(&receipt);
    let process = confirm(startup, process)?;
    progress.stage("startup", "the Editor opened the host");
    if !options.attach {
        let pid = process.pid();
        process.release();
        if let EditorOutput::Detached { log } = &output
            && !cfg!(windows)
        {
            println!("dev: editor output goes to {}", log.display());
        }
        return Ok(EditorOutcome::Running { pid });
    }
    follow(process, &version_root)
}

/// The confirmed Editor, or why startup failed; an Editor that is still
/// running without confirming is stopped.
fn confirm(startup: StartupWait, process: EditorProcess) -> Result<EditorProcess, DevError> {
    let reason = match startup {
        StartupWait::Ready { .. } => return Ok(process),
        StartupWait::Failed { stage, reason } => format!("{stage}: {reason}"),
        StartupWait::Timeout => format!(
            "no startup receipt within {}s: the Editor did not complete its first host connection",
            DEV_STARTUP_TIMEOUT_MS / 1_000
        ),
        StartupWait::EditorExited { code } => {
            return Err(DevError::StartupUnconfirmed {
                reason: format!(
                    "the Editor exited before confirming startup{}",
                    code.map_or(String::new(), |code| format!(" with code {code}"))
                ),
            });
        }
    };
    let _ = process.stop();
    Err(DevError::StartupUnconfirmed {
        reason: format!("{reason}; it was stopped"),
    })
}

fn follow(process: EditorProcess, version_root: &Path) -> Result<EditorOutcome, DevError> {
    let status = process.wait().map_err(|error| DevError::EditorStatus {
        status: error.to_string(),
    })?;
    match status.code() {
        Some(code) => {
            if code == 0 {
                println!("dev: editor from {} exited", version_root.display());
            }
            Ok(EditorOutcome::Exited { code })
        }
        None => Err(DevError::EditorStatus {
            status: "terminated by a signal".to_owned(),
        }),
    }
}
