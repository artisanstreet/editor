//! One-command native development installation.
//!
//! `nix run .#dev` builds a payload with Nix; this runner signs it with the
//! development root's local key and installs it as a `dev`-channel release
//! into the per-user `Artisan Street Dev` installation through
//! `artisan-install`, the same code that installs published releases. It then
//! provisions that root as the dev Forge home through the existing CLI
//! custody APIs and launches the **installed** Editor, which performs its
//! unchanged shipping startup: it discovers the root through `ARTISAN_HOME`,
//! verifies the installed payload, starts its owned Forge, connects over
//! authenticated QUIC, and writes the opt-in startup receipt the runner
//! waits for.
//!
//! The real installation is never touched: the dev root is a separate
//! installation whose channel pins it to locally signed releases, and
//! repeat runs preserve its database, credentials, and instance identity.
//! Running a new build over an open dev Editor retires it the way an update
//! does, so every iteration exercises the real update path.

#![forbid(unsafe_code)]

pub mod args;
pub mod binaries;
pub mod error;
pub mod launch;
pub mod lock;
pub mod paths;
pub mod payload;
pub mod provision;

pub use args::{Action, Command, DEFAULT_KEEP, DevArgs, usage};
pub use binaries::{BinarySet, locate_binaries, locate_in_dir};
pub use error::DevError;
pub use launch::{
    DEV_LAUNCH_REPORT_TIMEOUT_MS, DEV_STARTUP_POLL_MS, DEV_STARTUP_TIMEOUT_MS, EditorOutput,
    EditorProcess, MAX_RECEIPT_TEXT, ReadinessReconcile, STARTUP_RECEIPT_ENV,
    STARTUP_RECEIPT_SCHEMA, StartupWait, clear_stale_receipt, editor_log_path, editor_output,
    fresh_receipt_path, read_receipt, reconcile_stale_readiness, refuse_live_forge, spawn_editor,
    staged_editor, staged_forge, streams_are_terminals, wait_for_startup,
};
pub use lock::DevLock;
pub use paths::{
    DEV_HOME_ENV, DEV_ROOT_ENV, DEV_ROOT_NAME, DevPaths, OWNED_DEV_FORGE_ENV,
    STRIPPED_DEV_HOME_ENV, STRIPPED_DEV_READY_ENV, default_dev_root, exe_name, is_network_share,
    resolve_dev_root,
};
pub use payload::{install_payload, payload_identity, sign_payload};
pub use provision::{
    DEV_ADMISSION_CAPACITY, DEV_ADMISSION_TIMEOUT_MS, DEV_DRAIN_TIMEOUT_MS,
    DEV_HANDSHAKE_TIMEOUT_MS, DEV_REQUEST_TIMEOUT_MS, DEV_REQUESTS_PER_CONNECTION,
    DEV_RUN_CLAIM_LEASE_MS, DEV_RUN_MAX_COMMAND_RETRIES, DEV_RUN_POLL_INTERVAL_MS,
    DEV_RUN_PROMPT_DELIVERY, DEV_RUN_QUEUE_CAPACITY, DEV_RUN_RETRY_BACKOFF_MS,
    DEV_RUN_SHUTDOWN_BUDGET_MS, DEV_RUN_STREAM_AFTER, InstanceOutcome, dev_listener_config,
    dev_run_config, provision_forge_home,
};

/// Formats one completed stage line (plain text, no TTY codes).
#[must_use]
pub fn stage_line(index: u32, total: u32, stage: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("dev: stage {index}/{total} {stage} ... ok")
    } else {
        format!("dev: stage {index}/{total} {stage} ... ok ({detail})")
    }
}
