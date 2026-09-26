//! The dev Forge's configuration, applied through the installed `ae setup`.
//!
//! The runner never writes Forge configuration itself: it hands these
//! values to the installation's own `ae setup`, the one creator of instance
//! configuration, which also provisions credentials (keeping existing ones),
//! records host access, and installs the Forge service.
//!
//! Listener budgets are lifetime budgets, not concurrency limits: the
//! backend closes a connection after `requests_per_connection` completed
//! requests (`BudgetReached`), and lifetime admissions are capped by
//! `admission_capacity`. Development values are therefore large and
//! bounded — a dev session with usage polling and catalog refreshes burns
//! hundreds of requests per hour, and 32 would disconnect normal use.
//! `prompt_delivery` accepts any nonempty string up to 256 bytes without
//! control characters or line breaks; the Forge enforces the identical
//! rule, and `queue` is the value the CLI fixtures use.

use std::ffi::OsString;

use crate::paths::DevPaths;

/// Listener admission budget for a dev Forge, in milliseconds.
pub const DEV_ADMISSION_TIMEOUT_MS: u64 = 5_000;
/// Handshake budget for a dev Forge, in milliseconds.
pub const DEV_HANDSHAKE_TIMEOUT_MS: u64 = 5_000;
/// Per-request budget for a dev Forge, in milliseconds.
pub const DEV_REQUEST_TIMEOUT_MS: u64 = 30_000;
/// Drain budget for a dev Forge shutdown, in milliseconds.
pub const DEV_DRAIN_TIMEOUT_MS: u64 = 2_000;
/// Lifetime admission capacity for a dev Forge.
pub const DEV_ADMISSION_CAPACITY: u32 = 1_024;
/// Lifetime per-connection request budget for a dev Forge.
pub const DEV_REQUESTS_PER_CONNECTION: u32 = 65_536;

/// Native-run claim lease for a dev Forge, in milliseconds.
pub const DEV_RUN_CLAIM_LEASE_MS: u64 = 30_000;
/// Native-run poll interval for a dev Forge, in milliseconds.
pub const DEV_RUN_POLL_INTERVAL_MS: u64 = 500;
/// Native-run retry backoff for a dev Forge, in milliseconds.
pub const DEV_RUN_RETRY_BACKOFF_MS: u64 = 1_000;
/// Native-run shutdown budget for a dev Forge, in milliseconds.
pub const DEV_RUN_SHUTDOWN_BUDGET_MS: u64 = 2_000;
/// Native-run queue capacity for a dev Forge.
pub const DEV_RUN_QUEUE_CAPACITY: u32 = 64;
/// Native-run command retry bound for a dev Forge.
pub const DEV_RUN_MAX_COMMAND_RETRIES: u32 = 3;
/// Native-run prompt delivery mode for a dev Forge.
pub const DEV_RUN_PROMPT_DELIVERY: &str = "queue";
/// Native-run stream threshold for a dev Forge.
pub const DEV_RUN_STREAM_AFTER: u64 = 0;

/// Where the dev Forge listens for Editors, and the name its invitation
/// carries (`ae setup --listen --host-name`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostAccess {
    /// `IP:PORT` or `auto:PORT`.
    pub listen: String,
    /// Machine name shown in the Editor.
    pub name: String,
}

/// The `ae setup` arguments that configure the dev Forge of `paths` as an
/// autostarted service serving `access`.
#[must_use]
pub fn setup_arguments(paths: &DevPaths, access: &HostAccess) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = vec!["setup".into()];
    for (option, path) in [
        ("--database-path", paths.database_path()),
        ("--custody-path", paths.custody_path()),
        ("--readiness-path", paths.readiness_path()),
    ] {
        arguments.push(option.into());
        arguments.push(path.into_os_string());
    }
    for (option, value) in [
        (
            "--admission-timeout-ms",
            DEV_ADMISSION_TIMEOUT_MS.to_string(),
        ),
        (
            "--handshake-timeout-ms",
            DEV_HANDSHAKE_TIMEOUT_MS.to_string(),
        ),
        ("--request-timeout-ms", DEV_REQUEST_TIMEOUT_MS.to_string()),
        ("--drain-timeout-ms", DEV_DRAIN_TIMEOUT_MS.to_string()),
        ("--admission-capacity", DEV_ADMISSION_CAPACITY.to_string()),
        (
            "--requests-per-connection",
            DEV_REQUESTS_PER_CONNECTION.to_string(),
        ),
        (
            "--native-run-claim-lease-ms",
            DEV_RUN_CLAIM_LEASE_MS.to_string(),
        ),
        (
            "--native-run-poll-interval-ms",
            DEV_RUN_POLL_INTERVAL_MS.to_string(),
        ),
        (
            "--native-run-retry-backoff-ms",
            DEV_RUN_RETRY_BACKOFF_MS.to_string(),
        ),
        (
            "--native-run-shutdown-budget-ms",
            DEV_RUN_SHUTDOWN_BUDGET_MS.to_string(),
        ),
        (
            "--native-run-queue-capacity",
            DEV_RUN_QUEUE_CAPACITY.to_string(),
        ),
        (
            "--native-run-max-command-retries",
            DEV_RUN_MAX_COMMAND_RETRIES.to_string(),
        ),
        (
            "--native-run-prompt-delivery",
            DEV_RUN_PROMPT_DELIVERY.to_owned(),
        ),
        (
            "--native-run-stream-after",
            DEV_RUN_STREAM_AFTER.to_string(),
        ),
        ("--listen", access.listen.clone()),
        ("--host-name", access.name.clone()),
    ] {
        arguments.push(option.into());
        arguments.push(value.into());
    }
    arguments.push("--autostart".into());
    arguments
}
