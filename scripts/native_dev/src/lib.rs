//! One-command native development installation.
//!
//! `bazel run //:dev` builds the Editor and Forge binaries through the
//! authoritative Bazel graph, stages them into an isolated development
//! installation under `<workspace>/.dist/dev`, provisions that home through
//! the existing CLI custody APIs (installation manifest, payload integrity,
//! Forge credentials, native instance configuration), and launches the
//! **staged** Editor. The Editor then performs its unchanged shipping
//! startup: it discovers the dev home through `ARTISAN_HOME`, verifies the
//! staged payload, starts its newly owned Forge through
//! [`artisan_editor_cli::process::start_owned`], connects over
//! authenticated QUIC, and writes the opt-in startup receipt the launcher
//! waits for. This crate invents no transport, handshake, or credential
//! flow; it only stages files, spawns the Editor, and confirms startup.
//!
//! The real installed application is never touched: every path lives under
//! the dev directory, and repeat invocations preserve the dev database,
//! credentials, and instance identity. Failed updates never touch the
//! active version: binaries stage into a scratch directory, verify there,
//! and swap in atomically.

#![forbid(unsafe_code)]

pub mod args;
pub mod error;
pub mod launch;
pub mod manifest;
pub mod paths;
pub mod provision;
pub mod runfiles;
pub mod stage;

pub use args::{Action, DevArgs, usage};
pub use error::DevError;
pub use launch::{
    DEV_STARTUP_POLL_MS, DEV_STARTUP_TIMEOUT_MS, MAX_RECEIPT_TEXT, STARTUP_RECEIPT_ENV,
    STARTUP_RECEIPT_SCHEMA, StartupWait, read_receipt, refuse_live_forge, spawn_editor,
    staged_editor, staged_forge, stop_editor, wait_for_startup,
};
pub use manifest::{
    installation_document, provision_manifest, verify_payload_dir, write_payload_manifest,
};
pub use paths::{
    DEV_HOME_ENV, DEV_HOME_NAME, DEV_VERSION, DIST_DEV_LEAF, DevPaths, STRIPPED_DEV_HOME_ENV,
    STRIPPED_DEV_READY_ENV, WORKSPACE_ENV, default_base_dir, exe_name, resolve_dev_dir,
};
pub use provision::{
    DEV_ADMISSION_CAPACITY, DEV_ADMISSION_TIMEOUT_MS, DEV_DRAIN_TIMEOUT_MS,
    DEV_HANDSHAKE_TIMEOUT_MS, DEV_REQUEST_TIMEOUT_MS, DEV_REQUESTS_PER_CONNECTION,
    DEV_RUN_CLAIM_LEASE_MS, DEV_RUN_MAX_COMMAND_RETRIES, DEV_RUN_POLL_INTERVAL_MS,
    DEV_RUN_PROMPT_DELIVERY, DEV_RUN_QUEUE_CAPACITY, DEV_RUN_RETRY_BACKOFF_MS,
    DEV_RUN_SHUTDOWN_BUDGET_MS, DEV_RUN_STREAM_AFTER, InstanceOutcome, dev_listener_config,
    dev_run_config, provision_forge_home,
};
pub use runfiles::{
    BinarySet, RUNFILES_DIR_ENV, RUNFILES_MANIFEST_ENV, find_in_manifest,
    find_prefixed_in_manifest, locate_binaries, locate_in_dir, runfiles_candidates,
};
pub use stage::{
    DevLock, StageCounts, hash_file, stage_binaries, stage_one_binary, staged_relative_names,
    write_atomic,
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
