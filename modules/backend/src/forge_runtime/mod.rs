//! Explicit long-lived Forge process assembly.
//!
//! This module is the only boundary that turns the independent application,
//! storage, custody, and transport owners into one process. Every path comes
//! from the caller, credentials are loaded before custody is acquired, and
//! readiness is published only after migrated storage and the real loopback
//! endpoint exist.

#![forbid(unsafe_code)]

mod config;
#[cfg(test)]
#[path = "../../../../tests/backend/forge_configured_run.rs"]
mod forge_configured_run;
mod readiness;
mod startup;

pub use self::config::{
    CredentialMaterialError, EXIT_CODE_APPLICATION_STARTUP, EXIT_CODE_CONFIGURATION,
    EXIT_CODE_CUSTODY, EXIT_CODE_SERVER_STARTUP, EXIT_CODE_SERVICE, EXIT_CODE_SHUTDOWN,
    ForgeConfigError, ForgeLaunchConfig, ForgeLaunchConfigInput, READY_SCHEMA, parse_args,
};
pub use self::readiness::ReadinessError;
pub use self::startup::{
    ForgeCleanupError, ForgePrimaryCleanupError, ForgeRuntimeError, ForgeServiceError, run,
};

#[cfg(test)]
use crate::ListenerLimits;

use std::fs::{Metadata, OpenOptions};
use std::net::{IpAddr, SocketAddr};

fn is_required_loopback(address: SocketAddr) -> bool {
    address.port() != 0 && address.ip() == IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

#[cfg(windows)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
const fn is_reparse_point(_: &Metadata) -> bool {
    false
}

#[cfg(windows)]
fn configure_no_reparse_open(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;

    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(windows))]
const fn configure_no_reparse_open(_: &mut OpenOptions) {}
