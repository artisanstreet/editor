//! Forge application assembly boundary.

use std::process::ExitCode;

pub mod credential_authority;
pub mod storage;

pub use credential_authority::{
    AuthenticatedCredential, CredentialAuthenticationError, CredentialAuthority,
    CredentialEntropyError, CredentialKind, PendingReconnect, ReconnectRotationError,
};
pub use storage::{ForgeStorage, ForgeStorageCloseError, ForgeStorageOpenError};

/// Runs the currently implemented Forge process boundary.
#[must_use]
pub const fn run() -> ExitCode {
    ExitCode::SUCCESS
}
