//! `nix run .#dev`: build the whole product with Nix and deploy it through
//! the shipping installer.
//!
//! The Linux runner (the `dev` app of the flake) builds the stage outputs of
//! the checkout with Nix, then deploys two halves, each a `dev`-channel
//! release signed with its installation's local key and installed by
//! `artisan-install`, the code that installs published releases:
//!
//! - **Forge** ([`host`]): the Linux dev installation
//!   (`$XDG_DATA_HOME/Artisan Street Dev`), configured by its own `ae setup`
//!   to run the Forge as the systemd user service
//!   `artisan-forge-dev.service`, listening where Editors can reach it and
//!   publishing a host invitation. The default installation also puts `ae`
//!   on the PATH and adopts a hand-deployed Forge ([`legacy`]).
//! - **Editor** ([`editor`]): inside WSL, the cross-built Windows runner,
//!   run through interop ([`wsl`]), installs the Windows payload into
//!   `%LOCALAPPDATA%\Artisan Street Dev`; elsewhere the Linux installation
//!   already holds the Editor. The Forge's invitation is registered like any
//!   host, and the installed Editor is launched on it.
//!
//! Rerunning retires the running Forge and Editor the way an update does, so
//! every iteration exercises the real update path; superseded versions are
//! kept for rollback and pruned.

#![forbid(unsafe_code)]

pub mod args;
pub mod binaries;
pub mod editor;
pub mod error;
pub mod host;
pub mod launch;
pub mod legacy;
pub mod lock;
pub mod nix;
pub mod paths;
pub mod payload;
pub mod provision;
pub mod wsl;

use artisan_build_info::BuildInfo;

pub use args::{Action, Command, DEFAULT_KEEP, DevArgs, EditorArgs, EditorPlatform, parse, usage};
pub use binaries::{BinarySet, locate_binaries, locate_in_dir};
pub use error::DevError;
pub use launch::{
    DEV_LAUNCH_REPORT_TIMEOUT_MS, DEV_STARTUP_POLL_MS, DEV_STARTUP_TIMEOUT_MS, EditorOutput,
    EditorProcess, MAX_RECEIPT_TEXT, STARTUP_RECEIPT_ENV, STARTUP_RECEIPT_SCHEMA, StartupWait,
    clear_stale_receipt, editor_library_path, editor_log_path, editor_output, fresh_receipt_path,
    read_receipt, spawn_editor, staged_editor, staged_forge, streams_are_terminals,
    wait_for_startup, windows_command_line,
};
pub use lock::DevLock;
pub use paths::{
    DEV_HOME_ENV, DEV_ROOT_ENV, DEV_ROOT_NAME, DevPaths, STRIPPED_DEV_HOME_ENV,
    STRIPPED_DEV_READY_ENV, default_dev_root, exe_name, is_network_share, resolve_dev_root,
};
pub use payload::{install_payload, payload_identity, sign_payload};
pub use provision::{
    DEV_ADMISSION_CAPACITY, DEV_ADMISSION_TIMEOUT_MS, DEV_DRAIN_TIMEOUT_MS,
    DEV_HANDSHAKE_TIMEOUT_MS, DEV_REQUEST_TIMEOUT_MS, DEV_REQUESTS_PER_CONNECTION,
    DEV_RUN_CLAIM_LEASE_MS, DEV_RUN_MAX_COMMAND_RETRIES, DEV_RUN_POLL_INTERVAL_MS,
    DEV_RUN_PROMPT_DELIVERY, DEV_RUN_QUEUE_CAPACITY, DEV_RUN_RETRY_BACKOFF_MS,
    DEV_RUN_SHUTDOWN_BUDGET_MS, DEV_RUN_STREAM_AFTER, HostAccess, setup_arguments,
};

/// Numbered stage lines of one half (`forge` or `editor`).
#[derive(Debug)]
pub struct Progress {
    half: &'static str,
    index: u32,
}

impl Progress {
    /// Progress of the half named `half`.
    #[must_use]
    pub const fn new(half: &'static str) -> Self {
        Self { half, index: 0 }
    }

    /// Reports one completed stage.
    pub fn stage(&mut self, stage: &str, detail: &str) {
        self.index += 1;
        println!("{}", stage_line(self.half, self.index, stage, detail));
    }
}

/// Formats one completed stage line (plain text, no TTY codes).
#[must_use]
pub fn stage_line(half: &str, index: u32, stage: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("dev: {half} {index} {stage} ... ok")
    } else {
        format!("dev: {half} {index} {stage} ... ok ({detail})")
    }
}

/// One line describing an installed build.
#[must_use]
pub fn describe(info: &BuildInfo) -> String {
    let commit = info.short_commit().map_or_else(String::new, |commit| {
        format!(
            ", commit {commit}{}",
            if info.dirty {
                " with local changes"
            } else {
                ""
            }
        )
    });
    format!(
        "{} ({} channel{commit})",
        info.version,
        info.channel.as_str()
    )
}

/// Removes superseded versions, reporting what was kept in use.
pub fn prune(paths: &DevPaths, keep: usize) {
    match artisan_install::prune(&paths.home, keep) {
        Ok(report) => {
            for version in &report.removed {
                println!("dev: pruned {version} from {}", paths.home.display());
            }
            for version in &report.in_use {
                println!("dev: kept {version} (in use)");
            }
        }
        Err(error) => eprintln!("dev: warning: prune skipped: {error}"),
    }
}
