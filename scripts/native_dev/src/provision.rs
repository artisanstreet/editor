//! Forge home provisioning: credentials and native instance configuration.
//!
//! Existing credentials, instance identity, and dev data are preserved: a
//! repeat invocation never re-mints them. This runs before binary
//! activation, so a failed provision never strands a `complete` manifest
//! over a broken home.
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

use std::fs;

use artisan_editor_cli::{
    credentials::{self, ForgeCredentialPaths},
    instance::{NativeInstanceConfig, NativeListenerConfig, NativeRunConfig, NativeRunConfigInput},
};

use crate::{error::DevError, paths::DevPaths};

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

/// Outcome of provisioning the native instance configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstanceOutcome {
    /// A fresh instance identity was minted for a new dev home.
    Created,
    /// The existing instance identity and dev data were preserved.
    Preserved,
}

/// Builds the dev listener configuration.
#[must_use]
pub fn dev_listener_config() -> NativeListenerConfig {
    NativeListenerConfig::new(
        DEV_ADMISSION_TIMEOUT_MS,
        DEV_HANDSHAKE_TIMEOUT_MS,
        DEV_REQUEST_TIMEOUT_MS,
        DEV_DRAIN_TIMEOUT_MS,
        std::num::NonZeroU32::new(DEV_ADMISSION_CAPACITY).unwrap_or(std::num::NonZeroU32::MIN),
        std::num::NonZeroU32::new(DEV_REQUESTS_PER_CONNECTION).unwrap_or(std::num::NonZeroU32::MIN),
    )
}

/// Builds the dev native-run configuration.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the dev values violate the shared
/// validation rule (a programming error, surfaced instead of panicking).
pub fn dev_run_config() -> Result<NativeRunConfig, DevError> {
    NativeRunConfig::new(NativeRunConfigInput {
        claim_lease_ms: DEV_RUN_CLAIM_LEASE_MS,
        poll_interval_ms: DEV_RUN_POLL_INTERVAL_MS,
        retry_backoff_ms: DEV_RUN_RETRY_BACKOFF_MS,
        shutdown_budget_ms: DEV_RUN_SHUTDOWN_BUDGET_MS,
        queue_capacity: DEV_RUN_QUEUE_CAPACITY,
        max_command_retries: DEV_RUN_MAX_COMMAND_RETRIES,
        prompt_delivery: DEV_RUN_PROMPT_DELIVERY.to_owned(),
        stream_after: DEV_RUN_STREAM_AFTER,
    })
    .map_err(|error| DevError::Stage {
        stage: "provision",
        reason: format!("dev instance configuration is invalid: {error}"),
    })
}

/// Provisions Forge credentials and the native instance configuration.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when provisioning or validation fails.
pub fn provision_forge_home(paths: &DevPaths) -> Result<InstanceOutcome, DevError> {
    fs::create_dir_all(&paths.home).map_err(|_| DevError::Stage {
        stage: "provision",
        reason: format!("cannot create {}", paths.home.display()),
    })?;
    credentials::provision_or_load(&paths.home).map_err(|error| DevError::Stage {
        stage: "provision",
        reason: format!("cannot provision dev credentials: {error}"),
    })?;
    let credential_paths =
        ForgeCredentialPaths::from_home(&paths.home).map_err(|error| DevError::Stage {
            stage: "provision",
            reason: format!("cannot resolve dev credentials: {error}"),
        })?;
    let instance_path = NativeInstanceConfig::native_path(&paths.home);
    let (instance_id, outcome) = match fs::symlink_metadata(&instance_path) {
        Ok(_) => (
            NativeInstanceConfig::load(&instance_path)
                .map_err(|error| DevError::Stage {
                    stage: "provision",
                    reason: format!("existing dev instance is invalid: {error}"),
                })?
                .instance_id(),
            InstanceOutcome::Preserved,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            artisan_editor_cli::instance::mint_instance_id().map_err(|error| DevError::Stage {
                stage: "provision",
                reason: format!("secure random source failed: {error}"),
            })?,
            InstanceOutcome::Created,
        ),
        Err(_) => {
            return Err(DevError::Stage {
                stage: "provision",
                reason: format!("cannot inspect {}", instance_path.display()),
            });
        }
    };
    let config = NativeInstanceConfig::new_with_instance_id(
        instance_id,
        paths.database_path(),
        paths.custody_path(),
        paths.readiness_path(),
        credential_paths.manifest_path().to_path_buf(),
        dev_listener_config(),
        dev_run_config()?,
    )
    .map_err(|error| DevError::Stage {
        stage: "provision",
        reason: format!("dev instance configuration is invalid: {error}"),
    })?;
    config
        .write_to_home(&paths.home)
        .map_err(|error| DevError::Stage {
            stage: "provision",
            reason: format!("cannot write dev instance: {error}"),
        })?;
    Ok(outcome)
}
