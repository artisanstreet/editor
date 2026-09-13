//! Explicit Forge launch contract: the exact long-form argument grammar,
//! typed launch configuration, and credential material loading.

mod arguments;
use arguments::{parse_option, recognized_option};

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, Metadata};
use std::io;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use artisan_native_engine::NativeOpenCode2Authority;
use artisan_protocol::{LocalCapability, LocalCapabilityError};
use artisan_transport::CancelHandle;
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use thiserror::Error;
use zeroize::Zeroize;

use super::is_reparse_point;
use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::listener::ListenerLimits;
use crate::native_run_dispatch::{
    NativeRunDispatcherConfig, NativeRunDispatcherConfigError, NativeRunDispatcherConfigInput,
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
pub(super) const ADMISSION_CAPACITY_OPTION: &str = "--admission-capacity";
const REQUESTS_PER_CONNECTION_OPTION: &str = "--requests-per-connection";
const NATIVE_CLAIM_LEASE_OPTION: &str = "--native-run-claim-lease-ms";
const NATIVE_POLL_INTERVAL_OPTION: &str = "--native-run-poll-interval-ms";
const NATIVE_RETRY_BACKOFF_OPTION: &str = "--native-run-retry-backoff-ms";
const NATIVE_SHUTDOWN_BUDGET_OPTION: &str = "--native-run-shutdown-budget-ms";
const NATIVE_QUEUE_CAPACITY_OPTION: &str = "--native-run-queue-capacity";
const NATIVE_MAX_COMMAND_RETRIES_OPTION: &str = "--native-run-max-command-retries";
const NATIVE_PROMPT_DELIVERY_OPTION: &str = "--native-run-prompt-delivery";
const NATIVE_STREAM_AFTER_OPTION: &str = "--native-run-stream-after";

/// Typed failures raised while parsing the exact Forge command-line contract.
///
/// No rejected argument value is retained. In particular, malformed input
/// cannot be reflected into diagnostics merely by asking for `Debug`.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ForgeConfigError {
    /// The explicitly selected interface is malformed or cannot receive unicast traffic.
    #[error("Forge --listen requires a unicast or wildcard IP:port")]
    InvalidListen,
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

    /// A text option was not valid UTF-8 at the operating-system boundary.
    #[error("Forge option {option} contains invalid text")]
    InvalidText { option: &'static str },

    /// The complete native-run scheduler could not be represented safely.
    #[error("Forge native-run configuration is invalid")]
    NativeRunConfiguration {
        #[source]
        source: NativeRunDispatcherConfigError,
    },
}

/// Complete inputs for one Forge launch.
///
/// The public input has no defaults: callers must provide the native-run
/// scheduler together with every storage, listener, credential, and custody
/// input. Path and scheduler validation is performed atomically by
/// [`ForgeLaunchConfig::new`].
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct ForgeLaunchConfigInput {
    /// SQLite database path.
    pub database: PathBuf,
    /// Process-custody path.
    pub custody: PathBuf,
    /// Ordered certificate DER paths, leaf first.
    pub certificate_der: Vec<PathBuf>,
    /// PKCS#8 private-key DER path.
    pub private_key_der: PathBuf,
    /// Bootstrap capability path.
    pub bootstrap_capability: PathBuf,
    /// Readiness receipt path.
    pub ready_file: PathBuf,
    /// Listener timeouts.
    pub limits: ListenerLimits,
    /// Listener admission capacity.
    pub admission_capacity: NonZeroU32,
    /// Per-connection request capacity.
    pub requests_per_connection: NonZeroU32,
    /// Complete configured native-run scheduler.
    pub native_run: NativeRunDispatcherConfig,
    /// Caller-owned process cancellation handle.
    pub cancel: Arc<CancelHandle>,
}

/// Explicit process configuration. No implicit storage location, filename, or
/// environment lookup is represented by this type.
///
/// The type deliberately has no `Default`. A caller must provide every
/// process path, limit, capacity, and cancellation owner explicitly.
#[allow(clippy::module_name_repetitions)]
pub struct ForgeLaunchConfig {
    pub(super) listen: std::net::SocketAddr,
    pub(super) database: PathBuf,
    pub(super) custody: PathBuf,
    pub(super) certificate_der: Vec<PathBuf>,
    pub(super) private_key_der: PathBuf,
    pub(super) bootstrap_capability: PathBuf,
    pub(super) ready_file: PathBuf,
    pub(super) limits: ListenerLimits,
    pub(super) admission_capacity: NonZeroU32,
    pub(super) requests_per_connection: NonZeroU32,
    pub(super) cancel: Arc<CancelHandle>,
    pub(super) native_run: NativeRunDispatcherConfig,
}

impl fmt::Debug for ForgeLaunchConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForgeLaunchConfig")
            .field("listen", &self.listen)
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
            .field("native_run", &"configured")
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
    pub fn new(input: ForgeLaunchConfigInput) -> Result<Self, ForgeConfigError> {
        let ForgeLaunchConfigInput {
            database,
            custody,
            certificate_der,
            private_key_der,
            bootstrap_capability,
            ready_file,
            limits,
            admission_capacity,
            requests_per_connection,
            native_run,
            cancel,
        } = input;
        let database = explicit_path(DATABASE_OPTION, database)?;
        let custody = explicit_path(CUSTODY_OPTION, custody)?;
        if certificate_der.is_empty() {
            return Err(ForgeConfigError::MissingOption {
                option: CERTIFICATE_OPTION,
            });
        }
        let certificate_der = certificate_der
            .into_iter()
            .map(|path| explicit_path(CERTIFICATE_OPTION, path))
            .collect::<Result<Vec<_>, _>>()?;
        let private_key_der = explicit_path(PRIVATE_KEY_OPTION, private_key_der)?;
        let bootstrap_capability = explicit_path(BOOTSTRAP_OPTION, bootstrap_capability)?;
        let ready_file = explicit_path(READY_FILE_OPTION, ready_file)?;
        Ok(Self {
            listen: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
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
            native_run,
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

struct ParsedNativeRunArguments {
    claim_lease_ms: Option<u64>,
    poll_interval_ms: Option<u64>,
    retry_backoff_ms: Option<u64>,
    shutdown_budget_ms: Option<u64>,
    queue_capacity: Option<NonZeroUsize>,
    max_command_retries: Option<NonZeroUsize>,
    prompt_delivery: Option<String>,
    stream_after: Option<u64>,
}

impl ParsedNativeRunArguments {
    fn empty() -> Self {
        Self {
            claim_lease_ms: None,
            poll_interval_ms: None,
            retry_backoff_ms: None,
            shutdown_budget_ms: None,
            queue_capacity: None,
            max_command_retries: None,
            prompt_delivery: None,
            stream_after: None,
        }
    }
}

struct ParsedForgeArguments {
    listen: Option<std::net::SocketAddr>,
    database: Option<PathBuf>,
    custody: Option<PathBuf>,
    certificate_der: Vec<PathBuf>,
    private_key_der: Option<PathBuf>,
    bootstrap_capability: Option<PathBuf>,
    ready_file: Option<PathBuf>,
    admission_timeout_ms: Option<u64>,
    handshake_timeout_ms: Option<u64>,
    request_timeout_ms: Option<u64>,
    drain_timeout_ms: Option<u64>,
    admission_capacity: Option<NonZeroU32>,
    requests_per_connection: Option<NonZeroU32>,
    native_run: ParsedNativeRunArguments,
}

impl ParsedForgeArguments {
    fn empty() -> Self {
        Self {
            listen: None,
            database: None,
            custody: None,
            certificate_der: Vec::new(),
            private_key_der: None,
            bootstrap_capability: None,
            ready_file: None,
            admission_timeout_ms: None,
            handshake_timeout_ms: None,
            request_timeout_ms: None,
            drain_timeout_ms: None,
            admission_capacity: None,
            requests_per_connection: None,
            native_run: ParsedNativeRunArguments::empty(),
        }
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
    let parsed = parse_argument_values(args)?;
    build_launch_config(parsed, cancel)
}

fn parse_argument_values<I, S>(args: I) -> Result<ParsedForgeArguments, ForgeConfigError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut arguments = args.into_iter().map(Into::into);
    let mut parsed = ParsedForgeArguments::empty();

    while let Some(raw_option) = arguments.next() {
        let Some(option) = recognized_option(&raw_option) else {
            return Err(ForgeConfigError::UnknownOption);
        };
        let raw_value = arguments
            .next()
            .ok_or(ForgeConfigError::MissingValue { option })?;
        parse_option(&mut parsed, option, raw_value)?;
    }

    Ok(parsed)
}

fn parse_native_option(
    parsed: &mut ParsedNativeRunArguments,
    option: &'static str,
    raw_value: OsString,
) -> Result<(), ForgeConfigError> {
    match option {
        NATIVE_CLAIM_LEASE_OPTION => {
            set_duration(&mut parsed.claim_lease_ms, option, raw_value.as_os_str())
        }
        NATIVE_POLL_INTERVAL_OPTION => {
            set_duration(&mut parsed.poll_interval_ms, option, raw_value.as_os_str())
        }
        NATIVE_RETRY_BACKOFF_OPTION => {
            set_duration(&mut parsed.retry_backoff_ms, option, raw_value.as_os_str())
        }
        NATIVE_SHUTDOWN_BUDGET_OPTION => set_duration(
            &mut parsed.shutdown_budget_ms,
            option,
            raw_value.as_os_str(),
        ),
        NATIVE_QUEUE_CAPACITY_OPTION => {
            set_usize_capacity(&mut parsed.queue_capacity, option, raw_value.as_os_str())
        }
        NATIVE_MAX_COMMAND_RETRIES_OPTION => set_usize_capacity(
            &mut parsed.max_command_retries,
            option,
            raw_value.as_os_str(),
        ),
        NATIVE_PROMPT_DELIVERY_OPTION => set_text(&mut parsed.prompt_delivery, option, raw_value),
        NATIVE_STREAM_AFTER_OPTION => {
            set_number(&mut parsed.stream_after, option, raw_value.as_os_str())
        }
        _ => Err(ForgeConfigError::UnknownOption),
    }
}

fn build_launch_config(
    parsed: ParsedForgeArguments,
    cancel: Arc<CancelHandle>,
) -> Result<ForgeLaunchConfig, ForgeConfigError> {
    let ParsedForgeArguments {
        listen,
        database,
        custody,
        certificate_der,
        private_key_der,
        bootstrap_capability,
        ready_file,
        admission_timeout_ms,
        handshake_timeout_ms,
        request_timeout_ms,
        drain_timeout_ms,
        admission_capacity,
        requests_per_connection,
        native_run,
    } = parsed;
    let native_run = NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::from_millis(required(
                native_run.claim_lease_ms,
                NATIVE_CLAIM_LEASE_OPTION,
            )?),
            poll_interval: Duration::from_millis(required(
                native_run.poll_interval_ms,
                NATIVE_POLL_INTERVAL_OPTION,
            )?),
            retry_backoff: Duration::from_millis(required(
                native_run.retry_backoff_ms,
                NATIVE_RETRY_BACKOFF_OPTION,
            )?),
            shutdown_budget: Duration::from_millis(required(
                native_run.shutdown_budget_ms,
                NATIVE_SHUTDOWN_BUDGET_OPTION,
            )?),
            queue_capacity: required(native_run.queue_capacity, NATIVE_QUEUE_CAPACITY_OPTION)?,
            max_command_retries: required(
                native_run.max_command_retries,
                NATIVE_MAX_COMMAND_RETRIES_OPTION,
            )?,
            prompt_delivery: required(native_run.prompt_delivery, NATIVE_PROMPT_DELIVERY_OPTION)?,
            stream_after: required(native_run.stream_after, NATIVE_STREAM_AFTER_OPTION)?,
        },
    )
    .map_err(|source| ForgeConfigError::NativeRunConfiguration { source })?;
    let mut config = ForgeLaunchConfig::new(ForgeLaunchConfigInput {
        database: required(database, DATABASE_OPTION)?,
        custody: required(custody, CUSTODY_OPTION)?,
        certificate_der: if certificate_der.is_empty() {
            return Err(ForgeConfigError::MissingOption {
                option: CERTIFICATE_OPTION,
            });
        } else {
            certificate_der
        },
        private_key_der: required(private_key_der, PRIVATE_KEY_OPTION)?,
        bootstrap_capability: required(bootstrap_capability, BOOTSTRAP_OPTION)?,
        ready_file: required(ready_file, READY_FILE_OPTION)?,
        limits: ListenerLimits {
            admission: Duration::from_millis(required(
                admission_timeout_ms,
                ADMISSION_TIMEOUT_OPTION,
            )?),
            handshake: Duration::from_millis(required(
                handshake_timeout_ms,
                HANDSHAKE_TIMEOUT_OPTION,
            )?),
            next_request: Duration::from_millis(required(
                request_timeout_ms,
                REQUEST_TIMEOUT_OPTION,
            )?),
            drain: Duration::from_millis(required(drain_timeout_ms, DRAIN_TIMEOUT_OPTION)?),
        },
        admission_capacity: required(admission_capacity, ADMISSION_CAPACITY_OPTION)?,
        requests_per_connection: required(requests_per_connection, REQUESTS_PER_CONNECTION_OPTION)?,
        native_run,
        cancel,
    })?;
    if let Some(listen) = listen {
        config.listen = listen;
    }
    Ok(config)
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

/// Loads every explicit certificate, key, and capability before custody.
pub(super) fn load_material(
    config: &ForgeLaunchConfig,
) -> Result<LoadedMaterial, CredentialMaterialError> {
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

pub(super) struct LoadedMaterial {
    pub(super) certificate_chain: Vec<CertificateDer<'static>>,
    pub(super) private_key: PrivatePkcs8KeyDer<'static>,
    pub(super) bootstrap: LocalCapability,
    pub(super) leaf: CertificateDer<'static>,
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

fn set_usize_capacity(
    slot: &mut Option<NonZeroUsize>,
    option: &'static str,
    raw_value: &OsStr,
) -> Result<(), ForgeConfigError> {
    if slot.is_some() {
        return Err(ForgeConfigError::Duplicate { option });
    }
    let value = parse_unsigned(raw_value, option)?;
    let value = usize::try_from(value).map_err(|_| ForgeConfigError::NumberOverflow { option })?;
    *slot = Some(NonZeroUsize::new(value).ok_or(ForgeConfigError::ZeroCapacity { option })?);
    Ok(())
}

fn set_number(
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

fn set_text(
    slot: &mut Option<String>,
    option: &'static str,
    raw_value: OsString,
) -> Result<(), ForgeConfigError> {
    if slot.is_some() {
        return Err(ForgeConfigError::Duplicate { option });
    }
    *slot = Some(
        raw_value
            .into_string()
            .map_err(|_| ForgeConfigError::InvalidText { option })?,
    );
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
