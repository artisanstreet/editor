//! Forge application assembly boundary.

use std::process::ExitCode;

pub mod credential_authority;

pub use credential_authority::{
    AuthenticatedCredential, CredentialAuthenticationError, CredentialAuthority, CredentialKind,
    PendingReconnect, ReconnectRotationError,
};

/// Runs the currently implemented Forge process boundary.
#[must_use]
pub const fn run() -> ExitCode {
    ExitCode::SUCCESS
}
