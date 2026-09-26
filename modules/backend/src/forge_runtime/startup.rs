//! The finite Forge process runtime: startup reconciliation, listener and
//! dispatcher bring-up, the consuming service loop, and ordered shutdown.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

use artisan_database::{SqliteConfig, StartupReconciliationCandidate};
use artisan_domain::PatchId;
use artisan_transport::{CancelHandle, PinnedIdentity, TransportError, server_config};
use rustls_pki_types::CertificateDer;
use thiserror::Error;

use super::config::{
    ADMISSION_CAPACITY_OPTION, CredentialMaterialError, EXIT_CODE_APPLICATION_STARTUP,
    EXIT_CODE_CONFIGURATION, EXIT_CODE_CUSTODY, EXIT_CODE_SERVER_STARTUP, EXIT_CODE_SERVICE,
    EXIT_CODE_SHUTDOWN, ForgeConfigError, ForgeLaunchConfig, LoadedMaterial, load_material,
};

use super::readiness::{ReadinessError, ReadinessReceipt};
use crate::{
    CommandOrigin, CommandOriginClockError, ForgeApp, ForgeConfig, ForgeListener,
    ForgeProcessCustody, ForgeProcessCustodyError, ForgeShutdownError, ForgeStartupError,
    ListenerError, ListenerLimits, RequestHandler, SystemCommandOrigin,
    directory_controller::{
        ControllerStartError, DirectoryController, DirectoryControllerConfig, ShutdownReport,
    },
    lifecycle_control::{ActivityGateImpl, LifecycleController},
    listener::ServeUntilCancelError,
    native_run_dispatch::{
        NativeRunDispatcher, NativeRunDispatcherConfig, NativeRunDispatcherShutdown,
    },
    run_cancellation::RunCancellationRegistry,
    startup_reconciliation_sweep::{
        PatchSourceError, StartupReconciliationPatchSource, StartupReconciliationPatches,
        StartupReconciliationSweepError, StartupReconciliationSweepInput,
        sweep_startup_reconciliation,
    },
};

/// The accepted listener's complete consuming-loop failure.
///
/// This alias deliberately keeps [`ServeUntilCancelError`] as the stored
/// value. Its private representation retains the service cause and any
/// secondary drain error without introducing a second classification surface.
#[allow(clippy::module_name_repetitions)]
pub type ForgeServiceError = ServeUntilCancelError;

/// Aggregated failures observed while stopping a partially or fully started
/// Forge process.
///
/// Keeping every typed failure in this private-field value means cleanup
/// errors are not silently replaced by the first startup or service error.
#[allow(clippy::module_name_repetitions)]
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
#[allow(clippy::module_name_repetitions)]
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
#[allow(clippy::module_name_repetitions)]
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

    /// The startup reconciliation clock could not provide a representable
    /// operation instant.
    #[error("Forge startup reconciliation clock failed")]
    StartupReconciliationClock(#[source] CommandOriginClockError),

    /// The bounded startup reconciliation pass failed.
    #[error("Forge startup reconciliation failed")]
    StartupReconciliation(#[source] Box<StartupReconciliationSweepError>),

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

    /// The configured native-run dispatcher did not join cleanly during
    /// cleanup. Its owner is awaited before this error is reported so child
    /// custody is never detached from Forge shutdown.
    #[error("Forge native-run dispatcher shutdown failed")]
    NativeRunDispatcherShutdown,

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
            Self::ApplicationStartup(_)
            | Self::StartupReconciliationClock(_)
            | Self::StartupReconciliation(_) => EXIT_CODE_APPLICATION_STARTUP,
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
            | Self::NativeRunDispatcherShutdown
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

/// Runs one explicit Forge process until cancellation or a typed service
/// failure. This is the deterministic, signal-free facade used by focused
/// real-crate tests; the binary wraps it with `ctrl_c` in [`crate::run`].
///
/// # Errors
///
/// Returns a typed failure mapped to the stable process exit codes exposed by
/// [`ForgeRuntimeError::exit_code`].
pub async fn run(config: ForgeLaunchConfig) -> Result<(), ForgeRuntimeError> {
    let material = load_material(&config).map_err(ForgeRuntimeError::Credentials)?;
    let ForgeLaunchConfig {
        database,
        custody: custody_path,
        certificate_der: _,
        private_key_der: _,
        bootstrap_capability: _,
        ready_file,
        listen,
        limits,
        admission_capacity,
        requests_per_connection,
        cancel,
        native_run,
    } = config;
    let custody =
        ForgeProcessCustody::acquire(&custody_path).map_err(ForgeRuntimeError::Custody)?;

    let app = match ForgeApp::start(ForgeConfig::new(SqliteConfig::file(database.clone()))).await {
        Ok(app) => app,
        Err(error) => {
            drop(custody);
            return Err(ForgeRuntimeError::ApplicationStartup(error));
        }
    };

    Box::pin(run_with_context(ForgeRunContext {
        app,
        custody,
        material,
        database,
        ready_file,
        listen,
        limits,
        admission_capacity,
        requests_per_connection,
        cancel,
        native_run,
    }))
    .await
}

struct ForgeRunContext {
    app: ForgeApp,
    custody: ForgeProcessCustody,
    material: LoadedMaterial,
    database: PathBuf,
    ready_file: PathBuf,
    listen: SocketAddr,
    limits: ListenerLimits,
    admission_capacity: NonZeroU32,
    requests_per_connection: NonZeroU32,
    cancel: Arc<CancelHandle>,
    native_run: NativeRunDispatcherConfig,
}

/// Deterministic patch identities used only by startup reconciliation.
///
/// The discovered run and assistant-item identities are already Forge-minted
/// opaque text. Retyping that text keeps recovery synchronous and
/// restart-stable without a second entropy or I/O boundary.
struct ForgeStartupReconciliationPatchSource;

impl StartupReconciliationPatchSource for ForgeStartupReconciliationPatchSource {
    fn patch_ids_for(
        &mut self,
        candidate: &StartupReconciliationCandidate,
    ) -> Result<StartupReconciliationPatches, PatchSourceError> {
        let turn_patch_id =
            PatchId::parse(candidate.run_id.as_str()).map_err(|_| PatchSourceError)?;
        let item_patch_id = candidate
            .assistant_item_id
            .as_ref()
            .map(|item_id| PatchId::parse(item_id.as_str()).map_err(|_| PatchSourceError))
            .transpose()?;
        Ok(StartupReconciliationPatches::new(
            turn_patch_id,
            item_patch_id,
        ))
    }
}

async fn reconcile_startup(app: &ForgeApp) -> Result<(), ForgeRuntimeError> {
    let operated_at = SystemCommandOrigin
        .acceptance_instant()
        .map_err(ForgeRuntimeError::StartupReconciliationClock)?;
    let input = StartupReconciliationSweepInput::new(operated_at, 64)
        .map_err(|error| ForgeRuntimeError::StartupReconciliation(Box::new(error)))?;
    let mut patch_source = ForgeStartupReconciliationPatchSource;
    sweep_startup_reconciliation(app.repository(), input, &mut patch_source)
        .await
        .map(|_| ())
        .map_err(|error| ForgeRuntimeError::StartupReconciliation(Box::new(error)))
}

async fn run_with_context(context: ForgeRunContext) -> Result<(), ForgeRuntimeError> {
    let ForgeRunContext {
        app,
        custody,
        material,
        database,
        ready_file,
        listen,
        limits,
        admission_capacity,
        requests_per_connection,
        cancel,
        native_run,
    } = context;

    if let Err(error) = reconcile_startup(&app).await {
        let handler = RequestHandler::with_subscriptions(app.repository().clone());
        return finish(app, handler, custody, None, None, None, Some(error)).await;
    }

    let Ok(forge_executable) = std::env::current_exe() else {
        let handler = RequestHandler::with_subscriptions(app.repository().clone());
        return finish(
            app,
            handler,
            custody,
            None,
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
                None,
                Some(ForgeRuntimeError::DirectoryControllerStartup(error)),
            )
            .await;
        }
    };
    let handler = RequestHandler::with_directory_picker(
        app.repository().clone(),
        directory_controller,
        limits.next_request,
    )
    .with_registered_engine_profiles_reader(
        crate::request_handler::NativeRegisteredEngineProfilesReader::new(database.clone()),
    )
    .with_conversation_commit_notifier(native_run.conversation_commit_notifier());
    Box::pin(run_with_handler(
        ForgeRunContext {
            app,
            custody,
            material,
            database,
            ready_file,
            listen,
            limits,
            admission_capacity,
            requests_per_connection,
            cancel,
            native_run,
        },
        handler,
    ))
    .await
}

struct ForgeListenerStartup {
    listener: ForgeListener,
    address: SocketAddr,
    leaf: CertificateDer<'static>,
}

struct ForgeListenerStartupError {
    listener: Option<ForgeListener>,
    error: Box<ForgeRuntimeError>,
}

fn prepare_forge_listener(
    listen: SocketAddr,
    material: LoadedMaterial,
    limits: ListenerLimits,
    admission_capacity: NonZeroU32,
    requests_per_connection: NonZeroU32,
    lifecycle: LifecycleController,
) -> Result<ForgeListenerStartup, Box<ForgeListenerStartupError>> {
    let LoadedMaterial {
        certificate_chain,
        private_key,
        bootstrap,
        leaf,
    } = material;
    let server = match server_config(certificate_chain, private_key) {
        Ok(server) => server,
        Err(error) => {
            return Err(Box::new(ForgeListenerStartupError {
                listener: None,
                error: Box::new(ForgeRuntimeError::ServerConfiguration(error)),
            }));
        }
    };
    let listener = match ForgeListener::bind_at_with_lifecycle(
        server,
        bootstrap,
        Box::new(SystemCommandOrigin),
        limits,
        admission_capacity,
        requests_per_connection,
        lifecycle,
        listen,
    ) {
        Ok(listener) => listener,
        Err(error) => {
            return Err(Box::new(ForgeListenerStartupError {
                listener: None,
                error: Box::new(ForgeRuntimeError::ListenerBind(error)),
            }));
        }
    };
    let address = match listener.local_addr() {
        Ok(address) if address.port() != 0 && address.ip() == listen.ip() => address,
        Ok(address) => {
            return Err(Box::new(ForgeListenerStartupError {
                listener: Some(listener),
                error: Box::new(ForgeRuntimeError::Address(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!(
                        "Forge listener address does not match the configured interface: {address}"
                    ),
                ))),
            }));
        }
        Err(error) => {
            return Err(Box::new(ForgeListenerStartupError {
                listener: Some(listener),
                error: Box::new(ForgeRuntimeError::Address(error)),
            }));
        }
    };
    Ok(ForgeListenerStartup {
        listener,
        address,
        leaf,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "one linear runtime loop bring-up sharing listener, dispatcher, and handler custody; extraction would thread that state"
)]
async fn run_with_handler(
    context: ForgeRunContext,
    handler: RequestHandler,
) -> Result<(), ForgeRuntimeError> {
    let ForgeRunContext {
        app,
        custody,
        material,
        database,
        ready_file,
        listen,
        limits,
        admission_capacity,
        requests_per_connection,
        cancel,
        native_run,
    } = context;
    // `u32` fits `usize` on every supported target; the admission capacity is
    // a `NonZeroU32`, so the registry's only constructor failure cannot occur.
    let admission_capacity_usize =
        usize::try_from(admission_capacity.get()).expect("u32 admission capacity fits usize");
    let run_cancellation =
        RunCancellationRegistry::new(admission_capacity_usize).map_err(|_| {
            ForgeRuntimeError::Configuration(ForgeConfigError::ZeroCapacity {
                option: ADMISSION_CAPACITY_OPTION,
            })
        })?;
    let handler = handler.with_run_cancellation_registry(run_cancellation.clone());
    let handler = handler
        .with_rich_link_resolver(crate::rich_link_service::RichLinkResolver::with_defaults());
    let handler = handler.with_project_repository_service(
        crate::project_repository_service::ProjectRepositoryService::new(app.repository().clone()),
    );
    let activity = ActivityGateImpl::new();
    let lifecycle = LifecycleController::with_activity_gate(Arc::new(activity.clone()));
    let ForgeListenerStartup {
        listener,
        address,
        leaf,
    } = match prepare_forge_listener(
        listen,
        material,
        limits,
        admission_capacity,
        requests_per_connection,
        lifecycle,
    ) {
        Ok(startup) => startup,
        Err(startup_error) => {
            let ForgeListenerStartupError { listener, error } = *startup_error;
            return finish(app, handler, custody, listener, None, None, Some(*error)).await;
        }
    };
    if cancel.is_cancelled() {
        return finish(app, handler, custody, Some(listener), None, None, None).await;
    }
    // The leaf is retained separately so this identity computation happens
    // only after the actual listener address has been observed.
    let identity = PinnedIdentity::from_certificate(&leaf);
    let receipt = match ReadinessReceipt::publish(&ready_file, address, identity) {
        Ok(receipt) => receipt,
        Err(error) => {
            return finish(
                app,
                handler,
                custody,
                Some(listener),
                None,
                None,
                Some(ForgeRuntimeError::Readiness(error)),
            )
            .await;
        }
    };
    let notifier = native_run.conversation_commit_notifier();
    let native_dispatcher = NativeRunDispatcher::start_with_registry(
        app.repository().clone(),
        database.clone(),
        native_run,
        Arc::clone(&cancel),
        run_cancellation,
        activity,
        &tokio::runtime::Handle::current(),
    );
    let handler = handler.with_run_interaction_registry(native_dispatcher.interaction_registry());
    let handler = handler.with_composer_catalog(
        crate::composer_catalog_service::ComposerCatalogService::new(
            native_dispatcher.catalog_client(),
            database,
        ),
    );
    // The Forge keeps usage fresh while an Editor is connected and pushes
    // every change; Editors never schedule usage reads.
    let usage = Arc::new(
        crate::account_usage_service::AccountUsageService::with_defaults(
            &artisan_native_engine::resolve_codex_cli(),
            &artisan_native_engine::resolve_claude_cli(),
            crate::account_usage_cursor::CursorUsageConfig::new(),
        )
        .with_host_state_notifier(notifier.clone()),
    );
    let usage_refresher = tokio::spawn(crate::account_usage_service::refresh_while_observed(
        Arc::clone(&usage),
        Arc::clone(&cancel),
    ));
    let handler = handler.with_shared_account_usage_service(usage);
    let primary = match listener.serve_until_cancel(&handler, &cancel).await {
        Ok(()) => None,
        Err(error) if error.is_service_failure() => Some(ForgeRuntimeError::Service(error)),
        Err(error) if error.is_drain_failure() => Some(ForgeRuntimeError::ListenerDrain(error)),
        // The accepted error has exactly the two classifications above. If
        // that contract ever grows, preserve the complete typed value and
        // keep the conservative shutdown exit rather than flattening it.
        Err(error) => Some(ForgeRuntimeError::ListenerDrain(error)),
    };
    // Serving ended; no Editor remains for the refresher to keep current.
    usage_refresher.abort();
    // `serve_until_cancel` consumes the listener on every path. No listener
    // owner or endpoint custody remains to pass into the cleanup tail.
    finish(
        app,
        handler,
        custody,
        None,
        Some(receipt),
        Some(native_dispatcher),
        primary,
    )
    .await
}

/// Finishes every owner in the required order and preserves every failure.
async fn finish(
    app: ForgeApp,
    mut handler: RequestHandler,
    custody: ForgeProcessCustody,
    listener: Option<ForgeListener>,
    receipt: Option<ReadinessReceipt>,
    mut native_dispatcher: Option<NativeRunDispatcher>,
    primary: Option<ForgeRuntimeError>,
) -> Result<(), ForgeRuntimeError> {
    let mut cleanup_failures = Vec::new();
    if let Some(listener) = listener
        && let Err(error) = listener.drain().await
    {
        cleanup_failures.push(ForgeRuntimeError::ListenerShutdown(error));
    }
    if let Some(dispatcher) = native_dispatcher.as_mut()
        && !matches!(
            dispatcher.shutdown().await,
            NativeRunDispatcherShutdown::Joined
        )
    {
        cleanup_failures.push(ForgeRuntimeError::NativeRunDispatcherShutdown);
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

    let mut failures = cleanup_failures.into_iter();
    match (failures.next(), failures.next()) {
        (None, _) => Ok(()),
        (Some(only), None) => Err(only),
        (Some(first), Some(second)) => Err(ForgeRuntimeError::Shutdown(ForgeCleanupError {
            failures: std::iter::once(first)
                .chain(std::iter::once(second))
                .chain(failures)
                .collect(),
        })),
    }
}
