//! Forge application assembly boundary.

use std::process::ExitCode;

pub mod app;
pub mod command_admission;
pub mod connection;
pub mod credential_authority;
pub mod directory_helper;
pub(crate) mod directory_helper_codec;
pub mod request_handler;
pub mod storage;

pub use app::{ForgeApp, ForgeConfig, ForgeShutdownError, ForgeStartupError};
pub use command_admission::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, SystemCommandOrigin,
};
pub use connection::{
    AuthenticationStageError, ConnectionLimits, ForgeConnection, RequestStageError,
    ServerFrameStamp, WelcomeMetadata,
};
pub use credential_authority::{
    AuthenticatedCredential, CredentialAuthenticationError, CredentialAuthority,
    CredentialEntropyError, CredentialKind, PendingReconnect, ReconnectRotationError,
};
// The directory helper keeps its surface private to this crate: only
// `directory_helper::run_if_requested` is public, for `main` composition.
pub use request_handler::RequestHandler;
pub use storage::{ForgeStorage, ForgeStorageCloseError, ForgeStorageOpenError};

/// Runs the currently implemented Forge process boundary.
///
/// Product assembly has not yet selected Forge's data-directory policy, so
/// this process is launched without an injected [`ForgeConfig`] value and
/// cannot construct an owning [`ForgeApp`]. The boundary therefore performs
/// no storage work and claims no readiness: it must never open-and-close a
/// database to appear busy or fabricate an application value. Once assembly
/// injects a configuration, startup must go through [`ForgeApp::start`] and
/// end in [`ForgeApp::shutdown`].
#[must_use]
pub const fn run() -> ExitCode {
    ExitCode::SUCCESS
}
