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
    io::{self, Write},
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
use artisan_transport::{
    CancelHandle, DeadlineError, PinnedIdentity, TransportError, server_config,
};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use thiserror::Error;
use zeroize::Zeroize;

use crate::{
    ForgeApp, ForgeConfig, ForgeListener, ForgeProcessCustody, ForgeProcessCustodyError,
    ForgeShutdownError, ForgeStartupError, ListenerError, ListenerLimits, RequestHandler,
    RequestTermination, SystemCommandOrigin, connection::RequestStageError,
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
                set_duration(&mut admission_timeout_ms, option, raw_value)?;
            }
            HANDSHAKE_TIMEOUT_OPTION => {
                set_duration(&mut handshake_timeout_ms, option, raw_value)?;
            }
            REQUEST_TIMEOUT_OPTION => {
                set_duration(&mut request_timeout_ms, option, raw_value)?;
            }
            DRAIN_TIMEOUT_OPTION => {
                set_duration(&mut drain_timeout_ms, option, raw_value)?;
            }
            ADMISSION_CAPACITY_OPTION => {
                set_capacity(&mut admission_capacity, option, raw_value)?;
            }
            REQUESTS_PER_CONNECTION_OPTION => {
                set_capacity(&mut requests_per_connection, option, raw_value)?;
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

/// Typed service-loop failure after startup.
#[derive(Debug, Error)]
pub enum ForgeServiceError {
    /// The listener's consuming service boundary failed.
    #[error("Forge listener service failed")]
    Listener(#[source] ListenerError),

    /// A request-stage failure ended a reusable authenticated connection.
    #[error("Forge request service failed")]
    Request(#[source] DeadlineError<RequestStageError>),
}

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

    /// More than one failure was observed while stopping Forge.
    #[error("Forge shutdown encountered multiple failures")]
    Shutdown(#[source] ForgeCleanupError),
}

impl ForgeRuntimeError {
    /// Returns the stable process exit code for this typed failure.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Configuration(_) | Self::Credentials(_) => EXIT_CODE_CONFIGURATION,
            Self::Custody(_) => EXIT_CODE_CUSTODY,
            Self::ApplicationStartup(_) => EXIT_CODE_APPLICATION_STARTUP,
            Self::ServerConfiguration(_)
            | Self::ListenerBind(_)
            | Self::Address(_)
            | Self::Readiness(_) => EXIT_CODE_SERVER_STARTUP,
            Self::Service(_) => EXIT_CODE_SERVICE,
            Self::Runtime(_)
            | Self::Signal(_)
            | Self::ListenerShutdown(_)
            | Self::ApplicationShutdown(_)
            | Self::ReadinessCleanup(_)
            | Self::Shutdown(_) => EXIT_CODE_SHUTDOWN,
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

    let handler = RequestHandler::with_subscriptions(app.repository().clone());
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

    let service = serve_until_cancel(listener, &handler, config.cancel_handle()).await;
    let (listener, primary) = match service {
        ServiceEnd::Cancelled { listener } => (listener, None),
        ServiceEnd::Failure { listener, error } => (listener, Some(error)),
        ServiceEnd::ConsumedOnCancellation => (None, None),
    };

    finish(app, handler, custody, listener, Some(receipt), primary).await
}

/// A result of the linear listener loop. The enum keeps lifecycle state
/// correlated instead of exposing public boolean combinations.
enum ServiceEnd {
    /// Cancellation was observed with reusable listener custody and the
    /// caller must finish the awaited drain.
    Cancelled { listener: Option<ForgeListener> },
    /// A typed non-cancellation service failure ended the loop.
    Failure {
        listener: Option<ForgeListener>,
        error: ForgeRuntimeError,
    },
    /// A consuming listener operation returned cancellation after it had
    /// synchronously closed and dropped its endpoint.
    ConsumedOnCancellation,
}

/// Serves one listener connection at a time until cancellation or failure.
async fn serve_until_cancel(
    mut listener: ForgeListener,
    handler: &RequestHandler,
    cancel: &CancelHandle,
) -> ServiceEnd {
    loop {
        if cancel.is_cancelled() {
            return match listener.drain().await {
                Ok(()) => ServiceEnd::Cancelled { listener: None },
                Err(error) => ServiceEnd::Failure {
                    listener: None,
                    error: ForgeRuntimeError::Shutdown(ForgeCleanupError {
                        failures: vec![ForgeRuntimeError::ListenerShutdown(error)],
                    }),
                },
            };
        }

        match listener.serve_one(handler, cancel).await {
            Ok((next, report)) => {
                listener = next;
                match report.termination {
                    RequestTermination::BudgetReached => {}
                    RequestTermination::Failed { source } if deadline_was_cancelled(&source) => {
                        return ServiceEnd::Cancelled {
                            listener: Some(listener),
                        };
                    }
                    RequestTermination::Failed { source } => {
                        return ServiceEnd::Failure {
                            listener: Some(listener),
                            error: ForgeRuntimeError::Service(ForgeServiceError::Request(source)),
                        };
                    }
                }
            }
            Err(error) if listener_error_was_cancelled(&error) => {
                return ServiceEnd::ConsumedOnCancellation;
            }
            Err(error) => {
                return ServiceEnd::Failure {
                    listener: None,
                    error: ForgeRuntimeError::Service(ForgeServiceError::Listener(error)),
                };
            }
        }
    }
}

/// Finishes every owner in the required order and preserves every failure.
async fn finish(
    app: ForgeApp,
    handler: RequestHandler,
    custody: ForgeProcessCustody,
    listener: Option<ForgeListener>,
    receipt: Option<ReadinessReceipt>,
    primary: Option<ForgeRuntimeError>,
) -> Result<(), ForgeRuntimeError> {
    let mut failures = Vec::new();
    if let Some(primary) = primary {
        failures.push(primary);
    }
    if let Some(listener) = listener {
        if let Err(error) = listener.drain().await {
            failures.push(ForgeRuntimeError::ListenerShutdown(error));
        }
    }
    if let Some(receipt) = receipt {
        if let Err(error) = receipt.remove() {
            failures.push(ForgeRuntimeError::ReadinessCleanup(error));
        }
    }
    drop(handler);
    if let Err(error) = app.shutdown().await {
        failures.push(ForgeRuntimeError::ApplicationShutdown(error));
    }
    // Custody is deliberately the last owning resource released. Its Drop
    // closes the exact lock carrier; no explicit unlock or unlink occurs.
    drop(custody);

    match failures.len() {
        0 => Ok(()),
        1 => Err(failures
            .pop()
            .expect("one cleanup failure exists when length is one")),
        _ => Err(ForgeRuntimeError::Shutdown(ForgeCleanupError { failures })),
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
            drop(file);
            let temporary_path = temporary.clone();
            return Err(with_temporary_cleanup(
                temporary,
                ReadinessError::WriteTemporary {
                    path: temporary_path,
                    source,
                },
            ));
        }
        if let Err(source) = file.flush().and_then(|()| file.sync_all()) {
            drop(file);
            let temporary_path = temporary.clone();
            return Err(with_temporary_cleanup(
                temporary,
                ReadinessError::WriteTemporary {
                    path: temporary_path,
                    source,
                },
            ));
        }
        let temporary_identity = match file.metadata() {
            Ok(metadata) => FileIdentity::from_metadata(&metadata),
            Err(source) => {
                drop(file);
                let temporary_path = temporary.clone();
                return Err(with_temporary_cleanup(
                    temporary,
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
            return Err(with_temporary_cleanup(temporary, error));
        }
        let target_metadata = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(with_temporary_cleanup(
                    temporary,
                    ReadinessError::TargetReplaced {
                        path: path.to_path_buf(),
                    },
                ));
            }
            Ok(metadata) if is_reparse_point(&metadata) || !metadata.is_file() => {
                return Err(with_temporary_cleanup(
                    temporary,
                    ReadinessError::TargetReplaced {
                        path: path.to_path_buf(),
                    },
                ));
            }
            Ok(metadata) => metadata,
            Err(source) => {
                return Err(with_temporary_cleanup(
                    temporary,
                    ReadinessError::Install {
                        path: path.to_path_buf(),
                        source,
                    },
                ));
            }
        };
        let identity = FileIdentity::from_metadata(&target_metadata);
        if identity != temporary_identity {
            return Err(with_temporary_cleanup(
                temporary,
                ReadinessError::TargetReplaced {
                    path: path.to_path_buf(),
                },
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            identity,
            contents: body.into_bytes(),
            temporary,
        })
    }

    fn remove(self) -> Result<(), ReadinessError> {
        let mut failure = None;
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                failure = Some(ReadinessError::TargetReplaced {
                    path: self.path.clone(),
                });
            }
            Ok(metadata) if is_reparse_point(&metadata) => {
                failure = Some(ReadinessError::TargetReplaced {
                    path: self.path.clone(),
                });
            }
            Ok(_) => match fs::metadata(&self.path) {
                Ok(metadata) if FileIdentity::from_metadata(&metadata) == self.identity => {
                    match fs::read(&self.path) {
                        Ok(contents) if contents.as_slice() == self.contents.as_slice() => {
                            if let Err(source) = fs::remove_file(&self.path) {
                                failure = Some(ReadinessError::Remove {
                                    path: self.path.clone(),
                                    source,
                                });
                            }
                        }
                        Ok(_) => {
                            failure = Some(ReadinessError::TargetReplaced {
                                path: self.path.clone(),
                            });
                        }
                        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                        Err(source) => {
                            failure = Some(ReadinessError::Remove {
                                path: self.path.clone(),
                                source,
                            });
                        }
                    }
                }
                Ok(_) => {
                    failure = Some(ReadinessError::TargetReplaced {
                        path: self.path.clone(),
                    });
                }
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    failure = Some(ReadinessError::Remove {
                        path: self.path.clone(),
                        source,
                    });
                }
            },
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                failure = Some(ReadinessError::Remove {
                    path: self.path.clone(),
                    source,
                });
            }
        }
        if let Err(source) = remove_private_temporary(&self.temporary) {
            let cleanup = ReadinessError::TemporaryCleanup {
                path: self.temporary,
                source,
            };
            failure = Some(match failure {
                Some(primary) => ReadinessError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                },
                None => cleanup,
            });
        }
        failure.map_or(Ok(()), Err)
    }
}

fn with_temporary_cleanup(path: PathBuf, primary: ReadinessError) -> ReadinessError {
    match remove_private_temporary(&path) {
        Ok(()) => primary,
        Err(source) => ReadinessError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(ReadinessError::TemporaryCleanup { path, source }),
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

fn remove_private_temporary(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temporary_path(parent: &Path) -> PathBuf {
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".artisan-forge-ready-{}-{sequence}.tmp",
        std::process::id()
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: Option<u32>,
    #[cfg(windows)]
    index: Option<u64>,
    #[cfg(not(any(unix, windows)))]
    length: u64,
}

impl FileIdentity {
    #[cfg(unix)]
    fn from_metadata(metadata: &Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;

        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    #[cfg(windows)]
    fn from_metadata(metadata: &Metadata) -> Self {
        use std::os::windows::fs::MetadataExt;

        Self {
            volume: metadata.volume_serial_number(),
            index: metadata.file_index(),
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            length: metadata.len(),
        }
    }
}

fn is_required_loopback(address: SocketAddr) -> bool {
    address.port() != 0 && address.ip() == IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

fn deadline_was_cancelled<E>(error: &DeadlineError<E>) -> bool {
    matches!(error, DeadlineError::Cancelled { .. })
}

fn listener_error_was_cancelled(error: &ListenerError) -> bool {
    match error {
        ListenerError::Admission { source } => deadline_was_cancelled(source),
        ListenerError::Authentication { source } => deadline_was_cancelled(source),
        ListenerError::UnrepresentableLimits
        | ListenerError::Bind(_)
        | ListenerError::AdmissionCapacityExhausted
        | ListenerError::Metadata { .. } => false,
    }
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
    raw_value: OsString,
) -> Result<(), ForgeConfigError> {
    if slot.is_some() {
        return Err(ForgeConfigError::Duplicate { option });
    }
    *slot = Some(parse_unsigned(raw_value.as_os_str(), option)?);
    Ok(())
}

fn set_capacity(
    slot: &mut Option<NonZeroU32>,
    option: &'static str,
    raw_value: OsString,
) -> Result<(), ForgeConfigError> {
    if slot.is_some() {
        return Err(ForgeConfigError::Duplicate { option });
    }
    let value = parse_unsigned(raw_value.as_os_str(), option)?;
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
fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
const fn is_reparse_point(_: &Metadata) -> bool {
    false
}
