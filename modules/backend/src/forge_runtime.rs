//! Explicit long-lived Forge process assembly.
//!
//! This module is the only boundary that turns the independent application,
//! storage, custody, and transport owners into one process. Every path comes
//! from the caller, credentials are loaded before custody is acquired, and
//! readiness is published only after migrated storage and the real loopback
//! endpoint exist.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{
    ffi::{OsStr, OsString},
    fmt,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr},
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use artisan_database::SqliteConfig;
use artisan_protocol::{LocalCapability, LocalCapabilityError};
use artisan_transport::{CancelHandle, PinnedIdentity, TransportError, server_config};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use thiserror::Error;
use zeroize::Zeroize;

use crate::{
    ForgeApp, ForgeConfig, ForgeListener, ForgeProcessCustody, ForgeProcessCustodyError,
    ForgeShutdownError, ForgeStartupError, ListenerError, ListenerLimits, RequestHandler,
    SystemCommandOrigin,
    directory_controller::{
        ControllerStartError, DirectoryController, DirectoryControllerConfig, ShutdownReport,
    },
    file_identity_policy::{FileIdentity, read_file_identity, same_file_identity},
    listener::ServeUntilCancelError,
};

/// Stable process exit code for invalid configuration or credential material.
pub const EXIT_CODE_CONFIGURATION: u8 = 64;
/// Stable process exit code for application or storage startup failure.
pub const EXIT_CODE_APPLICATION_STARTUP: u8 = 70;
/// Stable process exit code for server configuration, binding, address, or
/// readiness failure.
pub const EXIT_CODE_SERVER_STARTUP: u8 = 71;
/// Stable process exit code for a non-cancellation service-loop failure.
pub const EXIT_CODE_SERVICE: u8 = 72;
/// Stable process exit code for runtime or shutdown failure.
pub const EXIT_CODE_SHUTDOWN: u8 = 73;
/// Stable process exit code for process-custody contention or unavailability.
pub const EXIT_CODE_CUSTODY: u8 = 75;

/// Exact readiness schema identifier for a running Forge process.
pub const READY_SCHEMA: &str = "artisan-forge-ready-v1";

const DATABASE_OPTION: &str = "--database";
const CUSTODY_OPTION: &str = "--custody";
const CERTIFICATE_OPTION: &str = "--certificate-der";
const PRIVATE_KEY_OPTION: &str = "--private-key-der";
const BOOTSTRAP_OPTION: &str = "--bootstrap-capability";
const READY_FILE_OPTION: &str = "--ready-file";
const ADMISSION_TIMEOUT_OPTION: &str = "--admission-timeout-ms";
const HANDSHAKE_TIMEOUT_OPTION: &str = "--handshake-timeout-ms";
const REQUEST_TIMEOUT_OPTION: &str = "--request-timeout-ms";
const DRAIN_TIMEOUT_OPTION: &str = "--drain-timeout-ms";
const ADMISSION_CAPACITY_OPTION: &str = "--admission-capacity";
const REQUESTS_PER_CONNECTION_OPTION: &str = "--requests-per-connection";

/// Typed failures raised while parsing the exact Forge command-line contract.
///
/// No rejected argument value is retained. In particular, malformed input
/// cannot be reflected into diagnostics merely by asking for `Debug`.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ForgeConfigError {
    /// An option was supplied without its required value.
    #[error("Forge option {option} is missing its value")]
    MissingValue { option: &'static str },

    /// An option was supplied more than once.
    #[error("Forge option {option} was supplied more than once")]
    Duplicate { option: &'static str },

    /// An option name was not part of the exact long-form contract.
    #[error("unknown Forge option")]
    UnknownOption,

    /// A path option carried an empty value.
    #[error("Forge option {option} has an empty path")]
    EmptyPath { option: &'static str },

    /// A path option carried a relative path.
    #[error("Forge option {option} requires an absolute path")]
    RelativePath { option: &'static str },

    /// A path option did not name a file path.
    #[error("Forge option {option} must name a file")]
    NotAFilePath { option: &'static str },

    /// A path option contained a byte that cannot be represented by the
    /// operating-system path boundary.
    #[error("Forge option {option} contains an invalid path")]
    InvalidPath { option: &'static str },

    /// A numeric option was not an unsigned decimal integer.
    #[error("Forge option {option} requires an unsigned decimal integer")]
    InvalidNumber { option: &'static str },

    /// A numeric option did not fit its declared integer width.
    #[error("Forge option {option} is outside its permitted integer range")]
    NumberOverflow { option: &'static str },

    /// A nonzero capacity received zero.
    #[error("Forge option {option} must be nonzero")]
    ZeroCapacity { option: &'static str },

    /// A required option was not supplied.
    #[error("Forge option {option} is required")]
    MissingOption { option: &'static str },
}

/// Explicit process configuration. No implicit storage location, filename, or
/// environment lookup is represented by this type.
///
/// The type deliberately has no `Default`. A caller must provide every
/// process path, limit, capacity, and cancellation owner explicitly.
pub struct ForgeLaunchConfig {
    database: PathBuf,
    custody: PathBuf,
    certificate_der: Vec<PathBuf>,
    private_key_der: PathBuf,
    bootstrap_capability: PathBuf,
    ready_file: PathBuf,
    limits: ListenerLimits,
    admission_capacity: NonZeroU32,
    requests_per_connection: NonZeroU32,
    cancel: Arc<CancelHandle>,
}

impl fmt::Debug for ForgeLaunchConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForgeLaunchConfig")
            .field("database", &self.database)
            .field("custody", &self.custody)
            .field("certificate_der", &self.certificate_der)
            .field("private_key_der", &self.private_key_der)
            .field("bootstrap_capability", &self.bootstrap_capability)
            .field("ready_file", &self.ready_file)
            .field("limits", &self.limits)
            .field("admission_capacity", &self.admission_capacity)
            .field("requests_per_connection", &self.requests_per_connection)
            .field("cancel", &"caller-owned")
            .finish()
    }
}

impl ForgeLaunchConfig {
    /// Creates a fully explicit launch configuration.
    ///
    /// Every path is checked lexically for an absolute file path. Credential
    /// filesystem safety and credential contents are checked later, before
    /// process custody is acquired.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeConfigError`] when a path is empty, relative, or lacks
    /// a file component, or when no certificate path is supplied.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: impl Into<PathBuf>,
        custody: impl Into<PathBuf>,
        certificate_der: Vec<PathBuf>,
        private_key_der: impl Into<PathBuf>,
        bootstrap_capability: impl Into<PathBuf>,
        ready_file: impl Into<PathBuf>,
        limits: ListenerLimits,
        admission_capacity: NonZeroU32,
        requests_per_connection: NonZeroU32,
        cancel: Arc<CancelHandle>,
    ) -> Result<Self, ForgeConfigError> {
        let database = explicit_path(DATABASE_OPTION, database.into())?;
        let custody = explicit_path(CUSTODY_OPTION, custody.into())?;
        if certificate_der.is_empty() {
            return Err(ForgeConfigError::MissingOption {
                option: CERTIFICATE_OPTION,
            });
        }
        let certificate_der = certificate_der
            .into_iter()
            .map(|path| explicit_path(CERTIFICATE_OPTION, path))
            .collect::<Result<Vec<_>, _>>()?;
        let private_key_der = explicit_path(PRIVATE_KEY_OPTION, private_key_der.into())?;
        let bootstrap_capability = explicit_path(BOOTSTRAP_OPTION, bootstrap_capability.into())?;
        let ready_file = explicit_path(READY_FILE_OPTION, ready_file.into())?;
        Ok(Self {
            database,
            custody,
            certificate_der,
            private_key_der,
            bootstrap_capability,
            ready_file,
            limits,
            admission_capacity,
            requests_per_connection,
            cancel,
        })
    }

    /// Parses the exact long-form Forge options and attaches the caller-owned
    /// cancellation handle.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeConfigError`] for an unknown, duplicate, missing,
    /// malformed, empty, relative, overflowing, or zero-valued option.
    pub fn from_args<I, S>(args: I, cancel: Arc<CancelHandle>) -> Result<Self, ForgeConfigError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        parse_args(args, cancel)
    }

    /// Returns the explicitly selected SQLite database file path.
    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.database
    }

    /// Returns the explicitly selected process-custody file path.
    #[must_use]
    pub fn custody_path(&self) -> &Path {
        &self.custody
    }

    /// Returns the ordered certificate DER paths, with the leaf first.
    #[must_use]
    pub fn certificate_der_paths(&self) -> &[PathBuf] {
        &self.certificate_der
    }

    /// Returns the explicitly selected PKCS#8 private-key DER path.
    #[must_use]
    pub fn private_key_der_path(&self) -> &Path {
        &self.private_key_der
    }

    /// Returns the explicitly selected bootstrap-capability path.
    #[must_use]
    pub fn bootstrap_capability_path(&self) -> &Path {
        &self.bootstrap_capability
    }

    /// Returns the explicitly selected readiness receipt path.
    #[must_use]
    pub fn ready_file_path(&self) -> &Path {
        &self.ready_file
    }

    /// Returns the complete listener limits.
    #[must_use]
    pub const fn listener_limits(&self) -> ListenerLimits {
        self.limits
    }

    /// Returns the finite lifetime admission capacity.
    #[must_use]
    pub const fn admission_capacity(&self) -> NonZeroU32 {
        self.admission_capacity
    }

    /// Returns the finite per-connection request capacity.
    #[must_use]
    pub const fn requests_per_connection(&self) -> NonZeroU32 {
        self.requests_per_connection
    }

    /// Returns the caller-owned cancellation handle.
    #[must_use]
    pub fn cancel_handle(&self) -> &Arc<CancelHandle> {
        &self.cancel
    }
}

/// Parses the exact Forge command-line contract after the executable name.
///
/// # Errors
///
/// Returns [`ForgeConfigError`] for an unknown, duplicate, missing,
/// malformed, empty, relative, overflowing, or zero-valued option.
pub fn parse_args<I, S>(
    args: I,
    cancel: Arc<CancelHandle>,
) -> Result<ForgeLaunchConfig, ForgeConfigError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut arguments = args.into_iter().map(Into::into);
    let mut database = None;
    let mut custody = None;
    let mut certificate_der = Vec::new();
    let mut private_key_der = None;
    let mut bootstrap_capability = None;
    let mut ready_file = None;
    let mut admission_timeout_ms = None;
    let mut handshake_timeout_ms = None;
    let mut request_timeout_ms = None;
    let mut drain_timeout_ms = None;
    let mut admission_capacity = None;
    let mut requests_per_connection = None;

    while let Some(raw_option) = arguments.next() {
        let Some(option) = recognized_option(&raw_option) else {
            return Err(ForgeConfigError::UnknownOption);
        };
        let raw_value = arguments
            .next()
            .ok_or(ForgeConfigError::MissingValue { option })?;
        match option {
            DATABASE_OPTION => set_path(&mut database, option, raw_value)?,
            CUSTODY_OPTION => set_path(&mut custody, option, raw_value)?,
            CERTIFICATE_OPTION => certificate_der.push(explicit_path(option, raw_value.into())?),
            PRIVATE_KEY_OPTION => set_path(&mut private_key_der, option, raw_value)?,
            BOOTSTRAP_OPTION => set_path(&mut bootstrap_capability, option, raw_value)?,
            READY_FILE_OPTION => set_path(&mut ready_file, option, raw_value)?,
            ADMISSION_TIMEOUT_OPTION => {
                set_duration(&mut admission_timeout_ms, option, raw_value.as_os_str())?;
            }
            HANDSHAKE_TIMEOUT_OPTION => {
                set_duration(&mut handshake_timeout_ms, option, raw_value.as_os_str())?;
            }
            REQUEST_TIMEOUT_OPTION => {
                set_duration(&mut request_timeout_ms, option, raw_value.as_os_str())?;
            }
            DRAIN_TIMEOUT_OPTION => {
                set_duration(&mut drain_timeout_ms, option, raw_value.as_os_str())?;
            }
            ADMISSION_CAPACITY_OPTION => {
                set_capacity(&mut admission_capacity, option, raw_value.as_os_str())?;
            }
            REQUESTS_PER_CONNECTION_OPTION => {
                set_capacity(&mut requests_per_connection, option, raw_value.as_os_str())?;
            }
            _ => return Err(ForgeConfigError::UnknownOption),
        }
    }

    let admission_timeout_ms = required(admission_timeout_ms, ADMISSION_TIMEOUT_OPTION)?;
    let handshake_timeout_ms = required(handshake_timeout_ms, HANDSHAKE_TIMEOUT_OPTION)?;
    let request_timeout_ms = required(request_timeout_ms, REQUEST_TIMEOUT_OPTION)?;
    let drain_timeout_ms = required(drain_timeout_ms, DRAIN_TIMEOUT_OPTION)?;
    ForgeLaunchConfig::new(
        required(database, DATABASE_OPTION)?,
        required(custody, CUSTODY_OPTION)?,
        if certificate_der.is_empty() {
            return Err(ForgeConfigError::MissingOption {
                option: CERTIFICATE_OPTION,
            });
        } else {
            certificate_der
        },
        required(private_key_der, PRIVATE_KEY_OPTION)?,
        required(bootstrap_capability, BOOTSTRAP_OPTION)?,
        required(ready_file, READY_FILE_OPTION)?,
        ListenerLimits {
            admission: Duration::from_millis(admission_timeout_ms),
            handshake: Duration::from_millis(handshake_timeout_ms),
            next_request: Duration::from_millis(request_timeout_ms),
            drain: Duration::from_millis(drain_timeout_ms),
        },
        required(admission_capacity, ADMISSION_CAPACITY_OPTION)?,
        required(requests_per_connection, REQUESTS_PER_CONNECTION_OPTION)?,
        cancel,
    )
}

/// Typed failures raised while reading explicit credential material.
///
/// No byte buffer is retained by an error. The only diagnostic data are the
/// safe stage label, explicit path, and typed operating-system or length
/// failure.
#[derive(Debug, Error)]
pub enum CredentialMaterialError {
    /// A credential parent could not be inspected.
    #[error("failed to inspect {kind} credential parent at {path}")]
    InspectParent {
        kind: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A credential parent was absent.
    #[error("{kind} credential parent is missing at {path}")]
    ParentMissing { kind: &'static str, path: PathBuf },

    /// A credential parent was not a directory.
    #[error("{kind} credential parent is not a directory at {path}")]
    ParentNotDirectory { kind: &'static str, path: PathBuf },

    /// A credential parent was a symbolic link.
    #[error("{kind} credential parent is a symbolic link at {path}")]
    ParentSymlink { kind: &'static str, path: PathBuf },

    /// A credential parent was a Windows reparse point.
    #[error("{kind} credential parent is a reparse point at {path}")]
    ParentReparsePoint { kind: &'static str, path: PathBuf },

    /// The final credential path did not exist.
    #[error("{kind} credential is missing at {path}")]
    Missing { kind: &'static str, path: PathBuf },

    /// The final credential path could not be inspected.
    #[error("failed to inspect {kind} credential at {path}")]
    Inspect {
        kind: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The final credential path was a symbolic link.
    #[error("{kind} credential is a symbolic link at {path}")]
    Symlink { kind: &'static str, path: PathBuf },

    /// The final credential path was a Windows reparse point.
    #[error("{kind} credential is a reparse point at {path}")]
    ReparsePoint { kind: &'static str, path: PathBuf },

    /// The final credential path was not a regular file.
    #[error("{kind} credential is not a regular file at {path}")]
    NotRegular { kind: &'static str, path: PathBuf },

    /// Reading a regular credential file failed.
    #[error("failed to read {kind} credential at {path}")]
    Read {
        kind: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The bootstrap file did not contain exactly the protocol capability
    /// length.
    #[error("bootstrap capability has invalid length")]
    CapabilityLength(#[source] LocalCapabilityError),

    /// The configured certificate list could not produce a leaf.
    #[error("certificate chain did not contain a leaf")]
    EmptyCertificateChain,
}

/// The accepted listener's complete consuming-loop failure.
///
/// This alias deliberately keeps [`ServeUntilCancelError`] as the stored
/// value. Its private representation retains the service cause and any
/// secondary drain error without introducing a second classification surface.
pub type ForgeServiceError = ServeUntilCancelError;

/// Aggregated failures observed while stopping a partially or fully started
/// Forge process.
///
/// Keeping every typed failure in this private-field value means cleanup
/// errors are not silently replaced by the first startup or service error.
#[derive(Debug)]
pub struct ForgeCleanupError {
    failures: Vec<ForgeRuntimeError>,
}

impl ForgeCleanupError {
    /// Returns all typed failures observed during this shutdown attempt.
    #[must_use]
    pub fn failures(&self) -> &[ForgeRuntimeError] {
        &self.failures
    }
}

impl fmt::Display for ForgeCleanupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Forge shutdown failed at {} stage(s)",
            self.failures.len()
        )
    }
}

impl std::error::Error for ForgeCleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.failures
            .first()
            .map(|failure| failure as &(dyn std::error::Error + 'static))
    }
}

/// A primary process failure together with typed failures found while
/// cleaning up its owners.
///
/// The private fields keep the primary/cleanup relationship correlated. The
/// primary remains the process outcome, while every cleanup failure remains
/// available through typed accessors for diagnostics and tests.
#[derive(Debug)]
pub struct ForgePrimaryCleanupError {
    primary: Box<ForgeRuntimeError>,
    cleanup: ForgeCleanupError,
}

impl ForgePrimaryCleanupError {
    fn new(primary: ForgeRuntimeError, failures: Vec<ForgeRuntimeError>) -> Self {
        Self {
            primary: Box::new(primary),
            cleanup: ForgeCleanupError { failures },
        }
    }

    /// Returns the failure that determined the process exit code.
    #[must_use]
    pub fn primary(&self) -> &ForgeRuntimeError {
        &self.primary
    }

    /// Returns the aggregate of typed cleanup failures.
    #[must_use]
    pub fn cleanup(&self) -> &ForgeCleanupError {
        &self.cleanup
    }

    /// Returns every typed cleanup failure without exposing a parallel status
    /// flag or a duplicated exit-code value.
    #[must_use]
    pub fn cleanup_failures(&self) -> &[ForgeRuntimeError] {
        self.cleanup.failures()
    }
}

impl fmt::Display for ForgePrimaryCleanupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Forge primary failure ({}) retained with {} cleanup failure(s)",
            self.primary,
            self.cleanup.failures.len()
        )
    }
}

impl std::error::Error for ForgePrimaryCleanupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.primary.as_ref())
    }
}

/// Complete typed Forge process outcome.
#[derive(Debug, Error)]
pub enum ForgeRuntimeError {
    /// Explicit configuration was absent or malformed.
    #[error("invalid Forge configuration")]
    Configuration(#[source] ForgeConfigError),

    /// Explicit credential material could not be safely read or validated.
    #[error("invalid Forge credential material")]
    Credentials(#[source] CredentialMaterialError),

    /// The process-custody file could not be exclusively acquired.
    #[error("Forge process custody is unavailable")]
    Custody(#[source] ForgeProcessCustodyError),

    /// The migrated Forge application could not be started.
    #[error("Forge application startup failed")]
    ApplicationStartup(#[source] ForgeStartupError),

    /// The running Forge executable could not be resolved for the native
    /// directory controller.
    #[error("Forge directory controller executable could not be resolved")]
    DirectoryControllerExecutable,

    /// The native directory controller could not be started.
    #[error("Forge directory controller startup failed")]
    DirectoryControllerStartup(#[source] ControllerStartError),

    /// The loaded TLS material could not produce a server configuration.
    #[error("Forge server configuration failed")]
    ServerConfiguration(#[source] TransportError),

    /// The loopback listener could not be bound.
    #[error("Forge listener bind failed")]
    ListenerBind(#[source] ListenerError),

    /// The bound listener address could not be observed or was not the exact
    /// loopback address required by the contract.
    #[error("Forge listener address failed")]
    Address(#[source] io::Error),

    /// Readiness could not be safely published.
    #[error("Forge readiness failed")]
    Readiness(#[source] ReadinessError),

    /// A non-cancellation service operation failed.
    #[error("Forge service failed")]
    Service(#[source] ForgeServiceError),

    /// The accepted listener loop ended with a cancellation-only drain
    /// failure. The complete accepted error remains available to callers.
    #[error("Forge listener drain failed")]
    ListenerDrain(#[source] ForgeServiceError),

    /// A runtime could not be constructed.
    #[error("Forge runtime construction failed")]
    Runtime(#[source] io::Error),

    /// Ctrl-C signal registration or observation failed.
    #[error("Forge signal handling failed")]
    Signal(#[source] io::Error),

    /// A listener drain failed during cleanup.
    #[error("Forge listener shutdown failed")]
    ListenerShutdown(#[source] TransportError),

    /// The application storage pool failed to close during cleanup.
    #[error("Forge application shutdown failed")]
    ApplicationShutdown(#[source] ForgeShutdownError),

    /// A readiness receipt failed to be removed during cleanup.
    #[error("Forge readiness cleanup failed")]
    ReadinessCleanup(#[source] ReadinessError),

    /// The native directory controller did not report a joined owner task
    /// during cleanup.
    #[error("Forge directory controller shutdown failed")]
    DirectoryControllerShutdown(ShutdownReport),

    /// More than one failure was observed while stopping Forge.
    #[error("Forge shutdown encountered multiple failures")]
    Shutdown(#[source] ForgeCleanupError),

    /// A primary failure remained primary while cleanup failures were also
    /// retained for typed inspection.
    #[error("Forge primary failure retained with cleanup failures")]
    PrimaryWithCleanup(#[source] ForgePrimaryCleanupError),
}

impl ForgeRuntimeError {
    pub(crate) fn with_cleanup(primary: Self, cleanup_failures: Vec<Self>) -> Self {
        if cleanup_failures.is_empty() {
            primary
        } else {
            Self::PrimaryWithCleanup(ForgePrimaryCleanupError::new(primary, cleanup_failures))
        }
    }

    /// Returns the stable process exit code for this typed failure.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Configuration(_) | Self::Credentials(_) => EXIT_CODE_CONFIGURATION,
            Self::Custody(_) => EXIT_CODE_CUSTODY,
            Self::ApplicationStartup(_) => EXIT_CODE_APPLICATION_STARTUP,
            Self::DirectoryControllerExecutable | Self::DirectoryControllerStartup(_) => {
                EXIT_CODE_SERVER_STARTUP
            }
            Self::ServerConfiguration(_)
            | Self::ListenerBind(_)
            | Self::Address(_)
            | Self::Readiness(_) => EXIT_CODE_SERVER_STARTUP,
            Self::Service(_) => EXIT_CODE_SERVICE,
            Self::Runtime(_)
            | Self::Signal(_)
            | Self::ListenerShutdown(_)
            | Self::ListenerDrain(_)
            | Self::ApplicationShutdown(_)
            | Self::ReadinessCleanup(_)
            | Self::DirectoryControllerShutdown(_)
            | Self::Shutdown(_) => EXIT_CODE_SHUTDOWN,
            Self::PrimaryWithCleanup(composite) => composite.primary().exit_code(),
        }
    }

    /// Returns the primary failure when cleanup was also unsuccessful.
    #[must_use]
    pub fn primary_failure(&self) -> Option<&ForgeRuntimeError> {
        match self {
            Self::PrimaryWithCleanup(composite) => Some(composite.primary()),
            _ => None,
        }
    }

    /// Returns all typed cleanup failures retained by this outcome.
    #[must_use]
    pub fn cleanup_failures(&self) -> &[ForgeRuntimeError] {
        match self {
            Self::PrimaryWithCleanup(composite) => composite.cleanup_failures(),
            Self::Shutdown(cleanup) => cleanup.failures(),
            _ => &[],
        }
    }

    /// Returns the correlated primary/cleanup composite, when present.
    #[must_use]
    pub fn as_primary_with_cleanup(&self) -> Option<&ForgePrimaryCleanupError> {
        match self {
            Self::PrimaryWithCleanup(composite) => Some(composite),
            _ => None,
        }
    }
}

/// Typed readiness receipt failures. All variants are payload-free with
/// respect to credential bytes; only the explicit readiness path and safe
/// endpoint metadata can appear in diagnostics.
#[derive(Debug, Error)]
pub enum ReadinessError {
    /// The listener address was not the required IPv4 loopback endpoint.
    #[error("Forge listener did not bind the required IPv4 loopback endpoint")]
    InvalidEndpoint { address: SocketAddr },

    /// The readiness parent could not be inspected.
    #[error("failed to inspect readiness parent at {path}")]
    InspectParent {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The readiness parent was absent.
    #[error("readiness parent is missing at {path}")]
    ParentMissing { path: PathBuf },

    /// The readiness parent was not a directory.
    #[error("readiness parent is not a directory at {path}")]
    ParentNotDirectory { path: PathBuf },

    /// The readiness parent was a symbolic link.
    #[error("readiness parent is a symbolic link at {path}")]
    ParentSymlink { path: PathBuf },

    /// The readiness parent was a Windows reparse point.
    #[error("readiness parent is a reparse point at {path}")]
    ParentReparsePoint { path: PathBuf },

    /// The readiness target already existed.
    #[error("readiness target already exists at {path}")]
    TargetExists { path: PathBuf },

    /// The readiness target was a symbolic link.
    #[error("readiness target is a symbolic link at {path}")]
    TargetSymlink { path: PathBuf },

    /// The readiness target was a Windows reparse point.
    #[error("readiness target is a reparse point at {path}")]
    TargetReparsePoint { path: PathBuf },

    /// The readiness target could not be inspected.
    #[error("failed to inspect readiness target at {path}")]
    InspectTarget {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Creation of a same-directory private temporary file failed.
    #[error("failed to create private readiness temporary file at {path}")]
    CreateTemporary {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Writing or synchronizing the private temporary file failed.
    #[error("failed to write readiness temporary file at {path}")]
    WriteTemporary {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Atomic no-overwrite installation failed.
    #[error("failed to install readiness receipt at {path}")]
    Install {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A cleanup operation on a private temporary file failed.
    #[error("failed to remove readiness temporary file at {path}")]
    TemporaryCleanup {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The primary readiness operation and its private-temporary cleanup
    /// both failed. Neither typed failure is discarded.
    #[error("readiness operation failed ({primary}); cleanup also failed ({cleanup})")]
    Cleanup {
        #[source]
        primary: Box<Self>,
        cleanup: Box<Self>,
    },

    /// The target changed after this runtime installed its receipt.
    #[error("readiness target changed before cleanup at {path}")]
    TargetReplaced { path: PathBuf },

    /// Removing this runtime's receipt failed.
    #[error("failed to remove readiness receipt at {path}")]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Runs one explicit Forge process until cancellation or a typed service
/// failure. This is the deterministic, signal-free facade used by focused
/// real-crate tests; the binary wraps it with `ctrl_c` in [`crate::run`].
///
/// # Errors
///
/// Returns a typed failure mapped to the stable process exit codes exposed by
/// [`ForgeRuntimeError::exit_code`].
#[allow(clippy::too_many_lines)]
pub async fn run(config: ForgeLaunchConfig) -> Result<(), ForgeRuntimeError> {
    let material = load_material(&config).map_err(ForgeRuntimeError::Credentials)?;
    let custody =
        ForgeProcessCustody::acquire(config.custody_path()).map_err(ForgeRuntimeError::Custody)?;

    let app = match ForgeApp::start(ForgeConfig::new(SqliteConfig::file(
        config.database_path().to_path_buf(),
    )))
    .await
    {
        Ok(app) => app,
        Err(error) => {
            drop(custody);
            return Err(ForgeRuntimeError::ApplicationStartup(error));
        }
    };

    let Ok(forge_executable) = std::env::current_exe() else {
        let handler = RequestHandler::with_subscriptions(app.repository().clone());
        return finish(
            app,
            handler,
            custody,
            None,
            None,
            Some(ForgeRuntimeError::DirectoryControllerExecutable),
        )
        .await;
    };
    let directory_controller = match DirectoryController::start(
        DirectoryControllerConfig::new(forge_executable),
        &tokio::runtime::Handle::current(),
    ) {
        Ok(controller) => controller,
        Err(error) => {
            let handler = RequestHandler::with_subscriptions(app.repository().clone());
            return finish(
                app,
                handler,
                custody,
                None,
                None,
                Some(ForgeRuntimeError::DirectoryControllerStartup(error)),
            )
            .await;
        }
    };
    let handler = RequestHandler::with_directory_picker(
        app.repository().clone(),
        directory_controller,
        config.listener_limits().next_request,
    )
    .with_registered_engine_profiles_reader(
        crate::request_handler::NativeRegisteredEngineProfilesReader::new(
            config.database_path().to_path_buf(),
        ),
    );
    let LoadedMaterial {
        certificate_chain,
        private_key,
        bootstrap,
        leaf,
    } = material;

    let server = match server_config(certificate_chain, private_key) {
        Ok(server) => server,
        Err(error) => {
            return finish(
                app,
                handler,
                custody,
                None,
                None,
                Some(ForgeRuntimeError::ServerConfiguration(error)),
            )
            .await;
        }
    };

    let listener = match ForgeListener::bind(
        server,
        bootstrap,
        Box::new(SystemCommandOrigin),
        config.listener_limits(),
        config.admission_capacity(),
        config.requests_per_connection(),
    ) {
        Ok(listener) => listener,
        Err(error) => {
            return finish(
                app,
                handler,
                custody,
                None,
                None,
                Some(ForgeRuntimeError::ListenerBind(error)),
            )
            .await;
        }
    };

    let address = match listener.local_addr() {
        Ok(address) if is_required_loopback(address) => address,
        Ok(address) => {
            return finish(
                app,
                handler,
                custody,
                Some(listener),
                None,
                Some(ForgeRuntimeError::Address(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("Forge listener address is not required loopback: {address}"),
                ))),
            )
            .await;
        }
        Err(error) => {
            return finish(
                app,
                handler,
                custody,
                Some(listener),
                None,
                Some(ForgeRuntimeError::Address(error)),
            )
            .await;
        }
    };

    if config.cancel_handle().is_cancelled() {
        return finish(app, handler, custody, Some(listener), None, None).await;
    }

    // The leaf is retained separately so this identity computation happens
    // only after the actual listener address has been observed.
    let identity = PinnedIdentity::from_certificate(&leaf);
    let receipt = match ReadinessReceipt::publish(config.ready_file_path(), address, identity) {
        Ok(receipt) => receipt,
        Err(error) => {
            return finish(
                app,
                handler,
                custody,
                Some(listener),
                None,
                Some(ForgeRuntimeError::Readiness(error)),
            )
            .await;
        }
    };

    let primary = match listener
        .serve_until_cancel(&handler, config.cancel_handle())
        .await
    {
        Ok(()) => None,
        Err(error) if error.is_service_failure() => Some(ForgeRuntimeError::Service(error)),
        Err(error) if error.is_drain_failure() => Some(ForgeRuntimeError::ListenerDrain(error)),
        // The accepted error has exactly the two classifications above. If
        // that contract ever grows, preserve the complete typed value and
        // keep the conservative shutdown exit rather than flattening it.
        Err(error) => Some(ForgeRuntimeError::ListenerDrain(error)),
    };

    // `serve_until_cancel` consumes the listener on every path. No listener
    // owner or endpoint custody remains to pass into the cleanup tail.
    finish(app, handler, custody, None, Some(receipt), primary).await
}

/// Finishes every owner in the required order and preserves every failure.
async fn finish(
    app: ForgeApp,
    mut handler: RequestHandler,
    custody: ForgeProcessCustody,
    listener: Option<ForgeListener>,
    receipt: Option<ReadinessReceipt>,
    primary: Option<ForgeRuntimeError>,
) -> Result<(), ForgeRuntimeError> {
    let mut cleanup_failures = Vec::new();
    if let Some(listener) = listener
        && let Err(error) = listener.drain().await
    {
        cleanup_failures.push(ForgeRuntimeError::ListenerShutdown(error));
    }
    if let Some(receipt) = receipt
        && let Err(error) = receipt.remove()
    {
        cleanup_failures.push(ForgeRuntimeError::ReadinessCleanup(error));
    }
    if let Some(report) = handler.shutdown_directory_controller().await
        && report != ShutdownReport::Joined
    {
        cleanup_failures.push(ForgeRuntimeError::DirectoryControllerShutdown(report));
    }
    drop(handler);
    if let Err(error) = app.shutdown().await {
        cleanup_failures.push(ForgeRuntimeError::ApplicationShutdown(error));
    }
    // Custody is deliberately the last owning resource released. Its Drop
    // closes the exact lock carrier; no explicit unlock or unlink occurs.
    drop(custody);

    if let Some(primary) = primary {
        return Err(ForgeRuntimeError::with_cleanup(primary, cleanup_failures));
    }

    match cleanup_failures.len() {
        0 => Ok(()),
        1 => Err(cleanup_failures
            .into_iter()
            .next()
            .expect("one cleanup failure exists when length is one")),
        _ => Err(ForgeRuntimeError::Shutdown(ForgeCleanupError {
            failures: cleanup_failures,
        })),
    }
}

/// Loads every explicit certificate, key, and capability before custody.
fn load_material(config: &ForgeLaunchConfig) -> Result<LoadedMaterial, CredentialMaterialError> {
    let mut certificate_chain = Vec::with_capacity(config.certificate_der_paths().len());
    for path in config.certificate_der_paths() {
        let bytes = read_regular_credential(path, "certificate DER")?;
        certificate_chain.push(CertificateDer::from(bytes));
    }
    let leaf = certificate_chain
        .first()
        .cloned()
        .ok_or(CredentialMaterialError::EmptyCertificateChain)?;

    let private_key_bytes =
        read_regular_credential(config.private_key_der_path(), "private key DER")?;
    let private_key: PrivatePkcs8KeyDer<'static> = PrivatePkcs8KeyDer::from(private_key_bytes);

    let mut capability_bytes =
        read_regular_credential(config.bootstrap_capability_path(), "bootstrap capability")?;
    let capability = LocalCapability::try_from_slice(&capability_bytes);
    capability_bytes.zeroize();
    let bootstrap = match capability {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            let mut private_key = private_key;
            private_key.zeroize();
            return Err(CredentialMaterialError::CapabilityLength(error));
        }
    };

    Ok(LoadedMaterial {
        certificate_chain,
        private_key,
        bootstrap,
        leaf,
    })
}

struct LoadedMaterial {
    certificate_chain: Vec<CertificateDer<'static>>,
    private_key: PrivatePkcs8KeyDer<'static>,
    bootstrap: LocalCapability,
    leaf: CertificateDer<'static>,
}

/// Reads one credential only after rejecting every observable parent and the
/// final entry's symbolic-link/reparse/non-regular shape.
fn read_regular_credential(
    path: &Path,
    kind: &'static str,
) -> Result<Vec<u8>, CredentialMaterialError> {
    let parent = path
        .parent()
        .ok_or_else(|| CredentialMaterialError::ParentMissing {
            kind,
            path: path.to_path_buf(),
        })?;
    validate_credential_parent_chain(parent, kind)?;

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CredentialMaterialError::Missing {
                kind,
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(CredentialMaterialError::Inspect {
                kind,
                path: path.to_path_buf(),
                source,
            });
        }
    };
    validate_credential_metadata(path, kind, &metadata)?;
    fs::read(path).map_err(|source| CredentialMaterialError::Read {
        kind,
        path: path.to_path_buf(),
        source,
    })
}

fn validate_credential_parent_chain(
    parent: &Path,
    kind: &'static str,
) -> Result<(), CredentialMaterialError> {
    let mut current = parent;
    loop {
        let metadata = match fs::symlink_metadata(current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(CredentialMaterialError::ParentMissing {
                    kind,
                    path: current.to_path_buf(),
                });
            }
            Err(source) => {
                return Err(CredentialMaterialError::InspectParent {
                    kind,
                    path: current.to_path_buf(),
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(CredentialMaterialError::ParentSymlink {
                kind,
                path: current.to_path_buf(),
            });
        }
        if is_reparse_point(&metadata) {
            return Err(CredentialMaterialError::ParentReparsePoint {
                kind,
                path: current.to_path_buf(),
            });
        }
        if !metadata.is_dir() {
            return Err(CredentialMaterialError::ParentNotDirectory {
                kind,
                path: current.to_path_buf(),
            });
        }
        let Some(next) = current.parent() else {
            break;
        };
        if next == current || next.as_os_str().is_empty() {
            break;
        }
        current = next;
    }
    Ok(())
}

fn validate_credential_metadata(
    path: &Path,
    kind: &'static str,
    metadata: &Metadata,
) -> Result<(), CredentialMaterialError> {
    if metadata.file_type().is_symlink() {
        return Err(CredentialMaterialError::Symlink {
            kind,
            path: path.to_path_buf(),
        });
    }
    if is_reparse_point(metadata) {
        return Err(CredentialMaterialError::ReparsePoint {
            kind,
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(CredentialMaterialError::NotRegular {
            kind,
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Publishes one compact JSON readiness receipt through a same-directory
/// private file and an atomic hard-link installation. Hard-link installation
/// fails if the target appeared after the initial inspection, so it cannot
/// overwrite an existing target.
struct ReadinessReceipt {
    path: PathBuf,
    identity: FileIdentity,
    contents: Vec<u8>,
    temporary: PathBuf,
}

impl ReadinessReceipt {
    #[allow(clippy::too_many_lines)]
    fn publish(
        path: &Path,
        address: SocketAddr,
        identity: PinnedIdentity,
    ) -> Result<Self, ReadinessError> {
        if !is_required_loopback(address) {
            return Err(ReadinessError::InvalidEndpoint { address });
        }
        let parent = path.parent().ok_or_else(|| ReadinessError::ParentMissing {
            path: path.to_path_buf(),
        })?;
        validate_readiness_parent_chain(parent)?;
        inspect_readiness_target(path)?;

        let body = readiness_json(address, identity);
        let mut temporary = None;
        let mut file = None;
        for _ in 0..32 {
            let candidate = temporary_path(parent);
            match create_private_temporary(&candidate) {
                Ok(created) => {
                    temporary = Some(candidate);
                    file = Some(created);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(ReadinessError::CreateTemporary {
                        path: candidate,
                        source,
                    });
                }
            }
        }
        let temporary = temporary.ok_or_else(|| ReadinessError::CreateTemporary {
            path: parent.to_path_buf(),
            source: io::Error::new(
                io::ErrorKind::AlreadyExists,
                "readiness temporary-name space is exhausted",
            ),
        })?;
        let mut file = file.expect("temporary file exists whenever its path exists");
        if let Err(source) = file.write_all(body.as_bytes()) {
            let temporary_path = temporary.clone();
            return Err(with_open_temporary_cleanup(
                temporary,
                file,
                ReadinessError::WriteTemporary {
                    path: temporary_path,
                    source,
                },
            ));
        }
        if let Err(source) = file.flush().and_then(|()| file.sync_all()) {
            let temporary_path = temporary.clone();
            return Err(with_open_temporary_cleanup(
                temporary,
                file,
                ReadinessError::WriteTemporary {
                    path: temporary_path,
                    source,
                },
            ));
        }
        let temporary_identity = match read_file_identity(&file) {
            Ok(identity) => identity,
            Err(source) => {
                let temporary_path = temporary.clone();
                return Err(with_open_temporary_cleanup(
                    temporary,
                    file,
                    ReadinessError::WriteTemporary {
                        path: temporary_path,
                        source,
                    },
                ));
            }
        };
        drop(file);

        if let Err(source) = fs::hard_link(&temporary, path) {
            let error = if source.kind() == io::ErrorKind::AlreadyExists {
                ReadinessError::TargetExists {
                    path: path.to_path_buf(),
                }
            } else {
                ReadinessError::Install {
                    path: path.to_path_buf(),
                    source,
                }
            };
            return Err(with_temporary_identity_cleanup(
                &temporary,
                temporary_identity,
                error,
            ));
        }
        // The hard link is now installed. Construct the cleanup owner before
        // opening or inspecting the target so every later failure has a
        // recorded identity-bound owner to clean up.
        let receipt = Self {
            path: path.to_path_buf(),
            identity: temporary_identity,
            contents: body.into_bytes(),
            temporary,
        };
        match receipt.validate_published_target() {
            Ok(()) => Ok(receipt),
            Err(primary) => Err(receipt.cleanup_after_error(primary)),
        }
    }

    fn remove(self) -> Result<(), ReadinessError> {
        // Both owners are attempted independently. A target replacement must
        // never prevent removal of this run's private temporary hard link.
        let target_failure = remove_owned_target(&self.path, self.identity, &self.contents).err();
        let temporary_failure = remove_owned_temporary(&self.temporary, self.identity).err();

        match (target_failure, temporary_failure) {
            (None, None) => Ok(()),
            (Some(failure), None) | (None, Some(failure)) => Err(failure),
            (Some(primary), Some(cleanup)) => Err(ReadinessError::Cleanup {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            }),
        }
    }

    fn validate_published_target(&self) -> Result<(), ReadinessError> {
        let Some(mut target) = open_readiness_target(&self.path)? else {
            return Err(ReadinessError::TargetReplaced {
                path: self.path.clone(),
            });
        };
        let target_identity =
            read_file_identity(&target).map_err(|source| ReadinessError::InspectTarget {
                path: self.path.clone(),
                source,
            })?;
        if !same_file_identity(target_identity, self.identity)
            || !readiness_contents_match(&mut target, &self.contents).map_err(|source| {
                ReadinessError::InspectTarget {
                    path: self.path.clone(),
                    source,
                }
            })?
        {
            return Err(ReadinessError::TargetReplaced {
                path: self.path.clone(),
            });
        }
        Ok(())
    }

    fn cleanup_after_error(self, primary: ReadinessError) -> ReadinessError {
        match self.remove() {
            Ok(()) => primary,
            Err(cleanup) => ReadinessError::Cleanup {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            },
        }
    }
}

/// Opens an existing readiness entry only after checking its complete parent
/// chain and final path shape. The path is checked again after opening so the
/// returned handle is paired with the exact entry that was inspected.
fn open_readiness_target(path: &Path) -> Result<Option<File>, ReadinessError> {
    let parent = path.parent().ok_or_else(|| ReadinessError::ParentMissing {
        path: path.to_path_buf(),
    })?;
    validate_readiness_parent_chain(parent)?;

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ReadinessError::InspectTarget {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if readiness_target_is_replaced(&metadata) {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }

    let file = match open_readiness_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ReadinessError::InspectTarget {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let handle_metadata = file
        .metadata()
        .map_err(|source| ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        })?;
    if readiness_target_is_replaced(&handle_metadata) {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) if readiness_target_is_replaced(&metadata) => {
            Err(ReadinessError::TargetReplaced {
                path: path.to_path_buf(),
            })
        }
        Ok(_) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_owned_target(
    path: &Path,
    expected_identity: FileIdentity,
    expected_contents: &[u8],
) -> Result<(), ReadinessError> {
    let Some(mut target) = open_readiness_target(path)? else {
        return Ok(());
    };
    let target_identity =
        read_file_identity(&target).map_err(|source| ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        })?;
    if !same_file_identity(target_identity, expected_identity)
        || !readiness_contents_match(&mut target, expected_contents).map_err(|source| {
            ReadinessError::InspectTarget {
                path: path.to_path_buf(),
                source,
            }
        })?
    {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }

    let Some(mut final_target) = open_readiness_target(path)? else {
        return Ok(());
    };
    let final_identity =
        read_file_identity(&final_target).map_err(|source| ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        })?;
    if !same_file_identity(final_identity, expected_identity)
        || !readiness_contents_match(&mut final_target, expected_contents).map_err(|source| {
            ReadinessError::InspectTarget {
                path: path.to_path_buf(),
                source,
            }
        })?
    {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }
    drop(final_target);
    drop(target);

    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ReadinessError::Remove {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_owned_temporary(
    path: &Path,
    expected_identity: FileIdentity,
) -> Result<(), ReadinessError> {
    let Some(file) = open_readiness_target(path)? else {
        return Ok(());
    };
    let identity =
        read_file_identity(&file).map_err(|source| ReadinessError::TemporaryCleanup {
            path: path.to_path_buf(),
            source,
        })?;
    if !same_file_identity(identity, expected_identity) {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }

    let Some(final_file) = open_readiness_target(path)? else {
        return Ok(());
    };
    let final_identity =
        read_file_identity(&final_file).map_err(|source| ReadinessError::TemporaryCleanup {
            path: path.to_path_buf(),
            source,
        })?;
    if !same_file_identity(final_identity, expected_identity) {
        return Err(ReadinessError::TargetReplaced {
            path: path.to_path_buf(),
        });
    }
    drop(final_file);
    drop(file);

    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ReadinessError::TemporaryCleanup {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn open_readiness_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    configure_no_reparse_open(&mut options);
    options.open(path)
}

fn readiness_contents_match(file: &mut File, expected: &[u8]) -> io::Result<bool> {
    let expected_length = u64::try_from(expected.len()).unwrap_or(u64::MAX);
    let limit = expected_length.saturating_add(1);
    let mut contents = Vec::with_capacity(expected.len().saturating_add(1));
    let mut limited = (&mut *file).take(limit);
    limited.read_to_end(&mut contents)?;
    Ok(contents == expected)
}

fn readiness_target_is_replaced(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink() || is_reparse_point(metadata) || !metadata.is_file()
}

fn with_open_temporary_cleanup(
    path: PathBuf,
    file: File,
    primary: ReadinessError,
) -> ReadinessError {
    let identity = match read_file_identity(&file) {
        Ok(identity) => identity,
        Err(source) => {
            drop(file);
            return ReadinessError::Cleanup {
                primary: Box::new(primary),
                cleanup: Box::new(ReadinessError::TemporaryCleanup { path, source }),
            };
        }
    };
    drop(file);
    with_temporary_identity_cleanup(&path, identity, primary)
}

fn with_temporary_identity_cleanup(
    path: &Path,
    identity: FileIdentity,
    primary: ReadinessError,
) -> ReadinessError {
    match remove_owned_temporary(path, identity) {
        Ok(()) => primary,
        Err(cleanup) => ReadinessError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        },
    }
}

fn readiness_json(address: SocketAddr, identity: PinnedIdentity) -> String {
    format!(
        "{{\"schema\":\"{READY_SCHEMA}\",\"endpoint\":\"{address}\",\"certificate_sha256\":\"{}\",\"pid\":{}}}\n",
        identity.to_hex(),
        std::process::id(),
    )
}

fn validate_readiness_parent_chain(parent: &Path) -> Result<(), ReadinessError> {
    let mut current = parent;
    loop {
        let metadata = match fs::symlink_metadata(current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ReadinessError::ParentMissing {
                    path: current.to_path_buf(),
                });
            }
            Err(source) => {
                return Err(ReadinessError::InspectParent {
                    path: current.to_path_buf(),
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(ReadinessError::ParentSymlink {
                path: current.to_path_buf(),
            });
        }
        if is_reparse_point(&metadata) {
            return Err(ReadinessError::ParentReparsePoint {
                path: current.to_path_buf(),
            });
        }
        if !metadata.is_dir() {
            return Err(ReadinessError::ParentNotDirectory {
                path: current.to_path_buf(),
            });
        }
        let Some(next) = current.parent() else {
            break;
        };
        if next == current || next.as_os_str().is_empty() {
            break;
        }
        current = next;
    }
    Ok(())
}

fn inspect_readiness_target(path: &Path) -> Result<(), ReadinessError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ReadinessError::TargetSymlink {
            path: path.to_path_buf(),
        }),
        Ok(metadata) if is_reparse_point(&metadata) => Err(ReadinessError::TargetReparsePoint {
            path: path.to_path_buf(),
        }),
        Ok(_) => Err(ReadinessError::TargetExists {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ReadinessError::InspectTarget {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn create_private_temporary(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    options.open(path)
}

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temporary_path(parent: &Path) -> PathBuf {
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".artisan-forge-ready-{}-{sequence}.tmp",
        std::process::id()
    ))
}

fn is_required_loopback(address: SocketAddr) -> bool {
    address.port() != 0 && address.ip() == IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

fn explicit_path(option: &'static str, path: PathBuf) -> Result<PathBuf, ForgeConfigError> {
    if path.as_os_str().is_empty() {
        return Err(ForgeConfigError::EmptyPath { option });
    }
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(ForgeConfigError::InvalidPath { option });
    }
    if !path.is_absolute() {
        return Err(ForgeConfigError::RelativePath { option });
    }
    if path.file_name().is_none() {
        return Err(ForgeConfigError::NotAFilePath { option });
    }
    Ok(path)
}

fn recognized_option(option: &OsStr) -> Option<&'static str> {
    match option.to_str()? {
        DATABASE_OPTION => Some(DATABASE_OPTION),
        CUSTODY_OPTION => Some(CUSTODY_OPTION),
        CERTIFICATE_OPTION => Some(CERTIFICATE_OPTION),
        PRIVATE_KEY_OPTION => Some(PRIVATE_KEY_OPTION),
        BOOTSTRAP_OPTION => Some(BOOTSTRAP_OPTION),
        READY_FILE_OPTION => Some(READY_FILE_OPTION),
        ADMISSION_TIMEOUT_OPTION => Some(ADMISSION_TIMEOUT_OPTION),
        HANDSHAKE_TIMEOUT_OPTION => Some(HANDSHAKE_TIMEOUT_OPTION),
        REQUEST_TIMEOUT_OPTION => Some(REQUEST_TIMEOUT_OPTION),
        DRAIN_TIMEOUT_OPTION => Some(DRAIN_TIMEOUT_OPTION),
        ADMISSION_CAPACITY_OPTION => Some(ADMISSION_CAPACITY_OPTION),
        REQUESTS_PER_CONNECTION_OPTION => Some(REQUESTS_PER_CONNECTION_OPTION),
        _ => None,
    }
}

fn set_path(
    slot: &mut Option<PathBuf>,
    option: &'static str,
    raw_value: OsString,
) -> Result<(), ForgeConfigError> {
    if slot.is_some() {
        return Err(ForgeConfigError::Duplicate { option });
    }
    *slot = Some(explicit_path(option, raw_value.into())?);
    Ok(())
}

fn set_duration(
    slot: &mut Option<u64>,
    option: &'static str,
    raw_value: &OsStr,
) -> Result<(), ForgeConfigError> {
    if slot.is_some() {
        return Err(ForgeConfigError::Duplicate { option });
    }
    *slot = Some(parse_unsigned(raw_value, option)?);
    Ok(())
}

fn set_capacity(
    slot: &mut Option<NonZeroU32>,
    option: &'static str,
    raw_value: &OsStr,
) -> Result<(), ForgeConfigError> {
    if slot.is_some() {
        return Err(ForgeConfigError::Duplicate { option });
    }
    let value = parse_unsigned(raw_value, option)?;
    let value = u32::try_from(value).map_err(|_| ForgeConfigError::NumberOverflow { option })?;
    *slot = Some(NonZeroU32::new(value).ok_or(ForgeConfigError::ZeroCapacity { option })?);
    Ok(())
}

fn parse_unsigned(value: &OsStr, option: &'static str) -> Result<u64, ForgeConfigError> {
    let Some(value) = value.to_str() else {
        return Err(ForgeConfigError::InvalidNumber { option });
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ForgeConfigError::InvalidNumber { option });
    }
    value
        .parse::<u64>()
        .map_err(|_| ForgeConfigError::NumberOverflow { option })
}

fn required<T>(value: Option<T>, option: &'static str) -> Result<T, ForgeConfigError> {
    value.ok_or(ForgeConfigError::MissingOption { option })
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
