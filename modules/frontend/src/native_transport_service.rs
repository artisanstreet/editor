//! Native Forge/session ownership for the shipping editor application.
//!
//! The service is deliberately a synchronous bounded bridge around one
//! service thread. The thread owns its Tokio runtime, authenticated session,
//! rotated capability, and owned Forge lease. Only owned domain values and
//! redacted typed diagnostics cross the bridge.

#![forbid(unsafe_code)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::module_name_repetitions)]

use std::{
    collections::HashSet,
    net::SocketAddr,
    num::NonZeroU32,
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TryRecvError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use artisan_domain::{
    AttachProject, CONVERSATION_QUERY_MAX_TURNS, Command, ConversationCursor, ConversationQuery,
    ConversationQueryBounds, ConversationRequest, ConversationSnapshot, ConversationSubscribe,
    ConversationUnsubscribe, CreateThread, DirectoryId, EngineRunConfig, ListAttachedProjects,
    ListProjectThreads, ListRegisteredEngineProfiles, PatchBatch, ProjectId, ProjectListing,
    ProjectSummary, Query, QueryTurnCount, QueueFirstMessage, ReadThreadEngineSettings, RequestId,
    SetThreadEngineConfig, ThreadId, ThreadListing, ThreadSummary, ThreadTitle, UnixMillis,
};
use artisan_editor_cli::{
    credentials::{
        NativeClientCredentials, RECONNECT_LOCK_TIMEOUT, ReconnectBinding,
        ReconnectCapabilityStore, ReconnectSessionLease, load_client_credentials,
    },
    instance::NativeInstanceConfig,
    manifest::InstallationManifest,
    paths::Layout,
    process::{ForgeLaunchSpec, ForgeProcessLease, start_owned},
};
use artisan_protocol::{
    ClientRequest, ConversationSubscriptionStarted, ConversationSubscriptionStopped, ErrorCode,
    FirstMessageReceipt, FrameId, Hello, HelloCredential, ProtocolVersion,
    RegisteredEngineProfilesResult, ResponsePayload, SetThreadEngineConfigResult,
    ThreadEngineSettingsResult, VersionOffer, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, ClientRequestError, ClientSession, ClientSessionLimits, DeadlineError,
    DeliveryReceiver, EnvelopeReceiveError, EnvelopeSendError, ExchangeError, FrameError,
    LoopbackTarget, PinnedIdentity, RequestOutcome,
};
use rustls_pki_types::CertificateDer;
use thiserror::Error;

/// Maximum number of commands waiting for the service thread.
pub const COMMAND_CAPACITY: usize = 64;

/// Maximum number of events waiting for the application thread.
pub const EVENT_CAPACITY: usize = 64;

/// Application-minted monotonic identity for one authoritative settings read.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SettingsLoadGeneration(u64);

impl SettingsLoadGeneration {
    /// Returns the first valid generation.
    #[must_use]
    pub const fn first() -> Self {
        Self(1)
    }

    /// Returns the next generation, or `None` when the counter is exhausted.
    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    /// Returns the finite generation number for test and correlation checks.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[cfg(test)]
    pub(crate) const fn from_raw_for_test(value: u64) -> Self {
        Self(value)
    }
}

/// Commands accepted by the native service.
#[derive(Clone, Eq, PartialEq)]
pub enum NativeTransportCommand {
    /// Start a fresh opaque-directory project intake.
    BeginProjectIntake,
    /// Continue the one retained retry plan for project intake.
    RetryProjectIntake,
    /// Select an existing Forge-owned project.
    SelectProject(ProjectId),
    /// Request a real snapshot for a host mounted on a known thread.
    RequestSnapshot(ThreadId),
    /// Load authoritative engine settings for one thread and generation.
    LoadThreadEngineSettings {
        /// Thread whose settings are being read.
        thread_id: ThreadId,
        /// Application-owned stale-response fence.
        generation: SettingsLoadGeneration,
    },
    /// Load the certified engine profile catalogue.
    ListRegisteredProfiles,
    /// Durably save one complete thread engine configuration.
    SetThreadEngineConfig(Box<SetThreadEngineConfig>),
    /// Durably queue the first exact message body on one known thread.
    QueueFirstMessage(Box<QueueFirstMessage>),
    /// Begin or resume authoritative conversation subscription.
    Subscribe {
        /// Thread to observe.
        thread_id: ThreadId,
        /// Cursor after which to resume, or None for fresh.
        after: Option<ConversationCursor>,
    },
    /// End authoritative conversation subscription.
    Unsubscribe {
        /// Thread no longer observed.
        thread_id: ThreadId,
    },
    /// Acknowledge that the application has applied a batch to cursor.
    AcknowledgePatch {
        /// Thread whose patch was applied.
        thread_id: ThreadId,
        /// Cursor after the applied batch.
        cursor: ConversationCursor,
    },
    /// Stop accepting work and release the session and owned Forge.
    Shutdown,
}

impl std::fmt::Debug for NativeTransportCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let variant = match self {
            Self::BeginProjectIntake => "BeginProjectIntake",
            Self::RetryProjectIntake => "RetryProjectIntake",
            Self::SelectProject(_) => "SelectProject",
            Self::RequestSnapshot(_) => "RequestSnapshot",
            Self::LoadThreadEngineSettings { .. } => "LoadThreadEngineSettings",
            Self::ListRegisteredProfiles => "ListRegisteredProfiles",
            Self::SetThreadEngineConfig(_) => "SetThreadEngineConfig",
            Self::QueueFirstMessage(_) => "QueueFirstMessage",
            Self::Subscribe { .. } => "Subscribe",
            Self::Unsubscribe { .. } => "Unsubscribe",
            Self::AcknowledgePatch { .. } => "AcknowledgePatch",
            Self::Shutdown => "Shutdown",
        };
        formatter.write_str("NativeTransportCommand::")?;
        formatter.write_str(variant)
    }
}

/// Redacted stage of a service failure.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ServiceFailureStage {
    /// Installation-root discovery.
    Layout,
    /// Installation manifest loading and ownership.
    Manifest,
    /// Active payload integrity.
    Payload,
    /// Instance configuration.
    Instance,
    /// Client credential material.
    Credentials,
    /// Owned Forge launch.
    Forge,
    /// Forge readiness receipt.
    Readiness,
    /// Authenticated transport handshake.
    Handshake,
    /// Correlated application request.
    Request,
    /// Bounded bridge delivery.
    EventBridge,
    /// Session/Forge release.
    Cleanup,
    /// Uni delivery stream.
    Delivery,
}

impl std::fmt::Display for ServiceFailureStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Layout => "layout",
            Self::Manifest => "manifest",
            Self::Payload => "payload",
            Self::Instance => "instance",
            Self::Credentials => "credentials",
            Self::Forge => "forge",
            Self::Readiness => "readiness",
            Self::Handshake => "handshake",
            Self::Request => "request",
            Self::EventBridge => "event bridge",
            Self::Cleanup => "cleanup",
            Self::Delivery => "delivery",
        };
        formatter.write_str(text)
    }
}

/// Redacted category of a service failure.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ServiceFailureCategory {
    /// The local installation or service was unavailable.
    Unavailable,
    /// A local value failed validation.
    InvalidConfiguration,
    /// Integrity or identity validation failed.
    Integrity,
    /// Authentication material or negotiation was rejected.
    Authentication,
    /// A peer response was rejected or reported failure.
    Peer,
    /// A local session operation became terminal.
    LocalSession,
    /// The bounded command bridge was full.
    Backpressure,
    /// A bounded bridge could not accept or deliver a value.
    ChannelClosed,
    /// Cleanup did not complete within its finite bounds.
    Cleanup,
}

impl std::fmt::Display for ServiceFailureCategory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Unavailable => "unavailable",
            Self::InvalidConfiguration => "invalid configuration",
            Self::Integrity => "integrity",
            Self::Authentication => "authentication",
            Self::Peer => "peer",
            Self::LocalSession => "local session",
            Self::Backpressure => "backpressure",
            Self::ChannelClosed => "channel closed",
            Self::Cleanup => "cleanup",
        };
        formatter.write_str(text)
    }
}

/// Typed diagnostic safe to show in the application.
///
/// It intentionally contains only finite enums. No filesystem path, endpoint,
/// credential, protocol detail, or peer text is retained.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ServiceFailure {
    /// Service phase that failed.
    pub stage: ServiceFailureStage,
    /// Redacted failure category.
    pub category: ServiceFailureCategory,
}

/// Redacted phase of one user-operated project intake.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeProjectIntakeStage {
    /// Waiting for the native directory chooser to settle.
    PickingDirectory,
    /// Sending the stable attach mutation.
    AttachingProject,
    /// Rediscovering the complete attached-project catalog.
    RefreshingProjects,
    /// Sending the stable thread-creation mutation.
    CreatingThread,
    /// Rediscovering the complete project-scoped thread catalog.
    RefreshingThreads,
}

/// Redacted operation classification for one project-intake failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeProjectIntakeOperation {
    /// The explicit native directory chooser request.
    PickDirectory,
    /// The durable project attachment mutation.
    AttachProject,
    /// The attached-project rediscovery query.
    RefreshProjects,
    /// The durable thread creation mutation.
    CreateThread,
    /// The project-thread rediscovery query.
    RefreshThreads,
}

impl ServiceFailure {
    const fn new(stage: ServiceFailureStage, category: ServiceFailureCategory) -> Self {
        Self { stage, category }
    }

    const fn unavailable(stage: ServiceFailureStage) -> Self {
        Self::new(stage, ServiceFailureCategory::Unavailable)
    }

    const fn invalid(stage: ServiceFailureStage) -> Self {
        Self::new(stage, ServiceFailureCategory::InvalidConfiguration)
    }

    const fn local_session() -> Self {
        Self::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::LocalSession,
        )
    }

    const fn bridge() -> Self {
        Self::new(
            ServiceFailureStage::EventBridge,
            ServiceFailureCategory::ChannelClosed,
        )
    }
}

impl std::fmt::Display for ServiceFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} ({})", self.stage, self.category)
    }
}

/// The final service custody result.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ServiceStopStatus {
    /// Session and Forge lease were released successfully.
    Clean,
    /// At least one release step reported a bounded failure.
    Failed,
}

/// Events crossing from the service thread to the GPUI application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeTransportEvent {
    /// The service thread has begun installation/session startup.
    Starting,
    /// Real attached-project rows in Forge order.
    Projects(ProjectListing),
    /// Real project-scoped thread rows in Forge order.
    Threads {
        /// Project whose rows were requested.
        project_id: ProjectId,
        /// Real thread listing.
        listing: ThreadListing,
    },
    /// Real bounded conversation state.
    Snapshot(ConversationSnapshot),
    /// Forge returned no attached projects.
    EmptyProjects,
    /// Forge returned no threads for a real project.
    EmptyThreads {
        /// Project with no threads.
        project_id: ProjectId,
    },
    /// One redacted phase of the active project intake.
    ProjectIntakeProgress(NativeProjectIntakeStage),
    /// The user dismissed the native directory chooser.
    ProjectIntakeCancelled,
    /// The intake completed with authoritative listings and identities.
    ProjectIntakeReady {
        /// Complete authoritative attached-project catalog.
        projects: ProjectListing,
        /// Project contained in `projects` and returned by attach.
        project_id: ProjectId,
        /// Complete authoritative thread catalog for `project_id`.
        threads: ThreadListing,
        /// Thread contained in `threads` and returned by create.
        thread_id: ThreadId,
    },
    /// One redacted intake failure and its single retry classification.
    ProjectIntakeFailed {
        /// Operation that failed.
        operation: NativeProjectIntakeOperation,
        /// Typed redacted failure.
        failure: ServiceFailure,
        /// Whether the one retained plan may be explicitly retried.
        retryable: bool,
    },
    /// Redacted startup, request, bridge, or cleanup failure.
    Failed(ServiceFailure),
    /// Authoritative persisted thread engine settings with its load fence.
    ThreadEngineSettings {
        /// Generation minted for this read.
        generation: SettingsLoadGeneration,
        /// Authoritative settings result.
        result: ThreadEngineSettingsResult,
    },
    /// Registered engine profile catalogue.
    RegisteredProfiles(RegisteredEngineProfilesResult),
    /// Registered engine profile catalogue read failure.
    RegisteredProfilesFailed(ServiceFailure),
    /// Durable thread engine configuration applied.
    ThreadEngineConfigSet(SetThreadEngineConfigResult, Box<EngineRunConfig>),
    /// Thread engine configuration precondition was stale.
    ThreadEngineConfigConflict {
        /// Thread whose save conflicted.
        thread_id: ThreadId,
        /// Exact save request that conflicted.
        request_id: RequestId,
    },
    /// Durable engine configuration save failed with a redacted diagnostic.
    ThreadEngineConfigFailed {
        /// Thread whose save failed.
        thread_id: ThreadId,
        /// Exact save request that failed.
        request_id: RequestId,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Durable first-message queue accepted or replayed by Forge.
    FirstMessageQueued(FirstMessageReceipt),
    /// Durable first-message queue failed with a redacted diagnostic.
    FirstMessageFailed {
        /// Thread whose queue request failed.
        thread_id: ThreadId,
        /// Exact queue request that failed.
        request_id: RequestId,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Authoritative thread-settings read failure with its load fence.
    ThreadEngineSettingsFailed {
        /// Thread whose settings were requested.
        thread_id: ThreadId,
        /// Generation minted for this read.
        generation: SettingsLoadGeneration,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Correlated subscription start acknowledgement.
    ConversationSubscriptionStarted {
        /// Thread whose subscription started.
        thread_id: ThreadId,
        /// Request that started the subscription.
        request_id: RequestId,
        /// Fresh or resumed start payload.
        started: ConversationSubscriptionStarted,
    },
    /// Correlated subscription stop acknowledgement.
    ConversationSubscriptionStopped {
        /// Thread whose subscription stopped.
        thread_id: ThreadId,
        /// Request that stopped the subscription.
        request_id: RequestId,
        /// Stop payload.
        stopped: ConversationSubscriptionStopped,
    },
    /// Uni-stream patch batch.
    PatchBatch(PatchBatch),
    /// Bounded path-free delivery loss.
    DeliveryLost(ServiceFailure),
    /// Terminal service state.
    Stopped(ServiceStopStatus),
}

/// Failure admitting a UI command without blocking.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CommandSendError {
    /// The bounded command bridge is full.
    #[error("native service command queue is busy")]
    Busy,
    /// The service thread has stopped or its receiver was released.
    #[error("native service command queue is stopped")]
    Stopped,
}

/// Failure observing the bounded event bridge.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum EventReceiveError {
    /// The service released its event sender.
    #[error("native service event queue is stopped")]
    Stopped,
}

/// Failure joining a service thread.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ServiceJoinError {
    /// Joining before the service reports completion would block the caller.
    #[error("native service has not finished")]
    NotFinished,
    /// The service thread terminated abnormally.
    #[error("native service thread terminated abnormally")]
    Panicked,
}

/// Failure creating the service thread.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ServiceSpawnError {
    /// The operating system refused the bounded service thread.
    #[error("native service thread could not be started")]
    Thread,
}

/// Delivery frame flowing from the dedicated delivery task via the private bounded Tokio channel.
#[derive(Debug)]
pub enum PrivateDelivery {
    /// Valid uni patch batch.
    Batch(PatchBatch),
    /// Bounded delivery loss.
    Lost(ServiceFailure),
}

/// One cloneable application-side handle to the native service.
#[derive(Clone)]
pub struct NativeTransportService {
    commands: tokio::sync::mpsc::Sender<NativeTransportCommand>,
    events: Arc<Mutex<Receiver<NativeTransportEvent>>>,
    finished: Arc<AtomicBool>,
    shutdown_requested: Arc<AtomicBool>,
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl NativeTransportService {
    /// Starts the one service thread and its owned Tokio runtime.
    ///
    /// No application or GPUI value is captured by the service closure.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceSpawnError::Thread`] if the service thread cannot be
    /// created.
    pub fn spawn() -> Result<Self, ServiceSpawnError> {
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(COMMAND_CAPACITY);
        let (event_tx, event_rx) = sync_channel(EVENT_CAPACITY);
        let finished = Arc::new(AtomicBool::new(false));
        let finished_for_thread = Arc::clone(&finished);
        let join = thread::Builder::new()
            .name("artisan-native-transport".to_owned())
            .spawn(move || {
                let starting_sent = event_tx.send(NativeTransportEvent::Starting).is_ok();
                if starting_sent {
                    if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        runtime.block_on(service_main(command_rx, event_tx));
                    } else {
                        let _ = event_tx.send(NativeTransportEvent::Failed(
                            ServiceFailure::unavailable(ServiceFailureStage::Handshake),
                        ));
                        let _ =
                            event_tx.send(NativeTransportEvent::Stopped(ServiceStopStatus::Failed));
                    }
                }
                finished_for_thread.store(true, Ordering::Release);
            })
            .map_err(|_| ServiceSpawnError::Thread)?;

        Ok(Self {
            commands: command_tx,
            events: Arc::new(Mutex::new(event_rx)),
            finished,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            join: Arc::new(Mutex::new(Some(join))),
        })
    }

    /// Tries to admit one command without waiting for capacity.
    ///
    /// # Errors
    ///
    /// Returns [`CommandSendError::Busy`] when the bounded command queue is
    /// full, or [`CommandSendError::Stopped`] after the service has stopped.
    pub fn submit(&self, command: NativeTransportCommand) -> Result<(), CommandSendError> {
        match command {
            NativeTransportCommand::Shutdown => self.request_shutdown(),
            command => {
                if self.shutdown_requested.load(Ordering::Acquire) {
                    return Err(CommandSendError::Stopped);
                }
                try_send_command(&self.commands, command)
            }
        }
    }

    /// Requests shutdown once, retaining nonblocking admission semantics.
    ///
    /// # Errors
    ///
    /// Returns [`CommandSendError::Busy`] when the bounded command queue is
    /// full, or [`CommandSendError::Stopped`] when its receiver is gone.
    pub fn request_shutdown(&self) -> Result<(), CommandSendError> {
        if self
            .shutdown_requested
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        match try_send_command(&self.commands, NativeTransportCommand::Shutdown) {
            Ok(()) => Ok(()),
            Err(CommandSendError::Busy) => {
                self.shutdown_requested.store(false, Ordering::Release);
                Err(CommandSendError::Busy)
            }
            Err(CommandSendError::Stopped) => Err(CommandSendError::Stopped),
        }
    }

    /// Receives one event without blocking the GPUI thread.
    ///
    /// # Errors
    ///
    /// Returns [`EventReceiveError::Stopped`] when the service has released
    /// the event sender.
    pub fn try_recv(&self) -> Result<Option<NativeTransportEvent>, EventReceiveError> {
        let receiver = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        match receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(EventReceiveError::Stopped),
        }
    }

    /// Returns whether the service thread has completed its final event path.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Joins after completion has been observed.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceJoinError::NotFinished`] if joining would block, or
    /// [`ServiceJoinError::Panicked`] if the service thread panicked.
    pub fn join(&self) -> Result<(), ServiceJoinError> {
        if !self.is_finished() {
            return Err(ServiceJoinError::NotFinished);
        }
        let mut join = self.join.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(handle) = join.take() else {
            return Ok(());
        };
        handle.join().map_err(|_| ServiceJoinError::Panicked)
    }
}

/// Tries to admit one command to the bounded Tokio queue.
///
/// # Errors
///
/// Returns [`CommandSendError::Busy`] when the queue is full, or
/// [`CommandSendError::Stopped`] when its receiver is closed.
pub fn try_send_command(
    sender: &tokio::sync::mpsc::Sender<NativeTransportCommand>,
    command: NativeTransportCommand,
) -> Result<(), CommandSendError> {
    match sender.try_send(command) {
        Ok(()) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Err(CommandSendError::Busy),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(CommandSendError::Stopped),
    }
}

/// Startup failure retained internally until it is converted into a redacted
/// service event.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StartupError {
    /// The active payload did not have exact verified health.
    #[error("native payload was not verified")]
    PayloadUnverified,
    /// A named startup stage failed without retaining its source.
    #[error("native startup stage failed")]
    Stage(ServiceFailureStage),
}

impl StartupError {
    const fn failure(self) -> ServiceFailure {
        match self {
            Self::PayloadUnverified => ServiceFailure::new(
                ServiceFailureStage::Payload,
                ServiceFailureCategory::Integrity,
            ),
            Self::Stage(stage) => {
                let category = match stage {
                    ServiceFailureStage::Payload | ServiceFailureStage::Readiness => {
                        ServiceFailureCategory::Integrity
                    }
                    ServiceFailureStage::Credentials | ServiceFailureStage::Handshake => {
                        ServiceFailureCategory::Authentication
                    }
                    ServiceFailureStage::Instance | ServiceFailureStage::Manifest => {
                        ServiceFailureCategory::InvalidConfiguration
                    }
                    _ => ServiceFailureCategory::Unavailable,
                };
                ServiceFailure::new(stage, category)
            }
        }
    }
}

/// Readiness agreement failure with no receipt text in the diagnostic.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ReadinessValidationError {
    /// The readiness endpoint was not a parseable exact loopback address.
    #[error("readiness endpoint was not exact loopback")]
    Endpoint,
    /// The readiness PID did not identify the owned lease.
    #[error("readiness PID did not identify the owned Forge")]
    Pid,
    /// The readiness certificate pin did not identify the client certificate.
    #[error("readiness certificate pin did not agree")]
    Certificate,
}

/// Validates the non-secret agreement between readiness and owned process
/// custody before a QUIC connection is attempted.
///
/// # Errors
///
/// Returns a typed readiness error when the endpoint is not exact loopback,
/// the reported PID differs from the owned lease, or the certificate pins do
/// not agree.
pub fn validate_readiness(
    endpoint: &str,
    readiness_pid: u32,
    lease_pid: u32,
    reported_certificate_pin: &str,
    expected_certificate_pin: &str,
) -> Result<LoopbackTarget, ReadinessValidationError> {
    let address = endpoint
        .parse::<SocketAddr>()
        .map_err(|_| ReadinessValidationError::Endpoint)?;
    let target = LoopbackTarget::new(address).map_err(|_| ReadinessValidationError::Endpoint)?;
    if readiness_pid != lease_pid {
        return Err(ReadinessValidationError::Pid);
    }
    if reported_certificate_pin != expected_certificate_pin
        || reported_certificate_pin != reported_certificate_pin.to_ascii_lowercase()
        || reported_certificate_pin.len() != 64
        || !reported_certificate_pin
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ReadinessValidationError::Certificate);
    }
    Ok(target)
}

#[cfg(test)]
fn payload_health_decision(
    health: &artisan_editor_cli::payload::PayloadHealth,
) -> Result<(), StartupError> {
    match health {
        artisan_editor_cli::payload::PayloadHealth::Verified => Ok(()),
        artisan_editor_cli::payload::PayloadHealth::Modified(_)
        | artisan_editor_cli::payload::PayloadHealth::Unverifiable => {
            Err(StartupError::PayloadUnverified)
        }
    }
}

/// A real service frame identity and timestamp.
struct FrameStamp {
    frame_id: FrameId,
    sent_at: UnixMillis,
}

/// One durable mutation and its exact wire identity.
///
/// This value is deliberately service-private and implements neither
/// `Debug` nor `Display`: a stable command may contain opaque routing values
/// that must never reach a log or the application bridge.
struct StableMutation {
    frame_id: FrameId,
    sent_at: UnixMillis,
    command: Command,
}

impl StableMutation {
    fn envelope(
        &self,
        protocol_version: ProtocolVersion,
    ) -> Result<(WireEnvelope, RequestId), ServiceFailure> {
        let request_id = self
            .frame_id
            .to_request_id()
            .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
        if self.command.request_id() != &request_id {
            return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
        }
        let envelope = WireEnvelope {
            protocol_version,
            frame_id: self.frame_id.clone(),
            sent_at: self.sent_at,
            body: WireEnvelopeBody::Request(ClientRequest::Command(self.command.clone())),
        };
        envelope
            .validate_correlation()
            .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
        Ok((envelope, request_id))
    }
}

/// Service-private identity minting. The checked monotonic counter makes
/// identities unique even when the clock does not advance.
struct FrameFactory {
    process_id: u32,
    counter: u64,
}

impl FrameFactory {
    fn new() -> Self {
        Self {
            process_id: std::process::id(),
            counter: 0,
        }
    }

    fn next(&mut self) -> Result<FrameStamp, StartupError> {
        let counter = self
            .counter
            .checked_add(1)
            .ok_or(StartupError::Stage(ServiceFailureStage::Request))?;
        self.counter = counter;
        let sent_at = real_unix_millis()?;
        let text = format!(
            "native-{}-{}-{}",
            self.process_id,
            sent_at.as_millis(),
            counter
        );
        let frame_id =
            FrameId::parse(text).map_err(|_| StartupError::Stage(ServiceFailureStage::Request))?;
        frame_id
            .to_request_id()
            .map_err(|_| StartupError::Stage(ServiceFailureStage::Request))?;
        Ok(FrameStamp { frame_id, sent_at })
    }
}

fn real_unix_millis() -> Result<UnixMillis, StartupError> {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let millis = i64::try_from(duration.as_millis())
                .map_err(|_| StartupError::Stage(ServiceFailureStage::Request))?;
            Ok(UnixMillis::from_millis(millis))
        }
        Err(error) => {
            let millis = i64::try_from(error.duration().as_millis())
                .map_err(|_| StartupError::Stage(ServiceFailureStage::Request))?;
            Ok(UnixMillis::from_millis(millis.saturating_neg()))
        }
    }
}

fn finite_duration(milliseconds: u64) -> Result<Duration, StartupError> {
    if milliseconds == 0 {
        return Err(StartupError::Stage(ServiceFailureStage::Instance));
    }
    Ok(Duration::from_millis(milliseconds))
}

fn build_snapshot_query(thread_id: ThreadId) -> Result<ConversationRequest, ServiceFailure> {
    let maximum_turn_count = QueryTurnCount::new(u64::from(CONVERSATION_QUERY_MAX_TURNS))
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(ConversationRequest::Query(ConversationQuery {
        thread_id,
        bounds: ConversationQueryBounds::Window { maximum_turn_count },
    }))
}

fn query_request(request: Query) -> ClientRequest {
    ClientRequest::Query(request)
}

fn project_request() -> ClientRequest {
    query_request(Query::ListAttachedProjects(ListAttachedProjects))
}

fn threads_request(project_id: ProjectId) -> ClientRequest {
    query_request(Query::ListProjectThreads(ListProjectThreads { project_id }))
}

fn snapshot_request(thread_id: ThreadId) -> Result<ClientRequest, ServiceFailure> {
    Ok(ClientRequest::Conversation(build_snapshot_query(
        thread_id,
    )?))
}

fn thread_engine_settings_request(thread_id: ThreadId) -> ClientRequest {
    query_request(Query::ReadThreadEngineSettings(
        ReadThreadEngineSettings::new(thread_id),
    ))
}

fn registered_profiles_request() -> ClientRequest {
    query_request(Query::ListRegisteredEngineProfiles(
        ListRegisteredEngineProfiles,
    ))
}

fn engine_config_stable_mutation(
    command: Box<SetThreadEngineConfig>,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = command.request_id().clone();
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if &request_id != command.request_id() {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command: Command::SetThreadEngineConfig(command),
    })
}

fn first_message_stable_mutation(
    command: QueueFirstMessage,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = command.request_id.clone();
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let frame_request_id = frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if frame_request_id != request_id || command.request_id != request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command: Command::QueueFirstMessage(command),
    })
}

fn make_request_frame(
    frames: &mut FrameFactory,
    protocol_version: ProtocolVersion,
    request: ClientRequest,
) -> Result<(WireEnvelope, RequestId), ServiceFailure> {
    let stamp = frames
        .next()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let request_id = stamp
        .frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let envelope = WireEnvelope {
        protocol_version,
        frame_id: stamp.frame_id,
        sent_at: stamp.sent_at,
        body: WireEnvelopeBody::Request(request),
    };
    envelope
        .validate_correlation()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok((envelope, request_id))
}

fn stable_mutation_from_stamp(
    stamp: FrameStamp,
    command: Command,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = stamp
        .frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if command.request_id() != &request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    Ok(StableMutation {
        frame_id: stamp.frame_id,
        sent_at: stamp.sent_at,
        command,
    })
}

fn attach_mutation(
    frames: &mut FrameFactory,
    directory_id: DirectoryId,
) -> Result<StableMutation, ServiceFailure> {
    let stamp = frames
        .next()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let request_id = stamp
        .frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    stable_mutation_from_stamp(
        stamp,
        Command::AttachProject(AttachProject {
            request_id,
            directory_id,
        }),
    )
}

fn create_mutation(
    frames: &mut FrameFactory,
    project_id: ProjectId,
    title: ThreadTitle,
) -> Result<StableMutation, ServiceFailure> {
    let stamp = frames
        .next()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let request_id = stamp
        .frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    stable_mutation_from_stamp(
        stamp,
        Command::CreateThread(CreateThread {
            request_id,
            project_id,
            title,
        }),
    )
}

#[derive(Clone)]
enum ExpectedResponse {
    Directory,
    Projects,
    AttachedProject,
    CreatedThread,
    Threads(ProjectId),
    Snapshot(ThreadId),
    ThreadEngineSettings(ThreadId),
    RegisteredProfiles,
    ThreadEngineConfigSet {
        thread_id: ThreadId,
        request_id: RequestId,
    },
    FirstMessageQueued {
        thread_id: ThreadId,
        request_id: RequestId,
    },
    ConversationSubscriptionStarted {
        thread_id: ThreadId,
    },
    ConversationSubscriptionStopped {
        thread_id: ThreadId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ThreadSelectionDecision {
    AwaitHostSnapshot(ThreadId),
    Empty,
}

fn thread_selection_decision(listing: &ThreadListing) -> ThreadSelectionDecision {
    listing
        .threads()
        .first()
        .map_or(ThreadSelectionDecision::Empty, |thread| {
            ThreadSelectionDecision::AwaitHostSnapshot(thread.thread_id.clone())
        })
}

enum RequestAttemptError {
    Retained {
        session: Box<ClientSession>,
        failure: ServiceFailure,
        peer: Option<PeerFailure>,
    },
    Terminal {
        failure: ServiceFailure,
        /// Whether the consumed session was lost during the request exchange
        /// and is eligible for the one local-session recovery owner.
        retryable_local_session_loss: bool,
    },
}

impl RequestAttemptError {
    #[cfg(test)]
    fn preserves_session(&self) -> bool {
        matches!(self, Self::Retained { .. })
    }
}

/// The only peer facts retained after correlated validation.
#[derive(Clone, Copy, Eq, PartialEq)]
struct PeerFailure {
    code: ErrorCode,
    retryable: bool,
}

/// Redacted request failure used by request and intake policies.
#[derive(Clone, Copy)]
struct RequestFailure {
    failure: ServiceFailure,
    peer: Option<PeerFailure>,
    retryable_local_session_loss: bool,
}

/// Subscription operation whose request failure is being routed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionRequestKind {
    /// A new or recovery subscription request.
    Subscribe,
    /// Retirement of the current subscription.
    Unsubscribe,
}

/// Command-loop disposition for a subscription request failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionFailureDisposition {
    /// Publish one delivery loss and let the service reconnect.
    RecoverDelivery,
    /// Propagate the failure as terminal.
    Terminal,
}

/// Classifies one subscription request failure without inferring retryability
/// from its public category alone.
#[must_use]
pub const fn subscription_failure_disposition(
    operation: SubscriptionRequestKind,
    failure: ServiceFailure,
    retryable_local_session_loss: bool,
) -> SubscriptionFailureDisposition {
    if matches!(operation, SubscriptionRequestKind::Subscribe)
        && matches!(failure.category, ServiceFailureCategory::LocalSession)
        && retryable_local_session_loss
    {
        SubscriptionFailureDisposition::RecoverDelivery
    } else {
        SubscriptionFailureDisposition::Terminal
    }
}

impl RequestFailure {
    fn terminal(failure: ServiceFailure) -> Self {
        Self {
            failure,
            peer: None,
            retryable_local_session_loss: false,
        }
    }

    fn retryable(self) -> bool {
        self.peer.is_some_and(|peer| peer.retryable)
    }

    fn code(self) -> Option<ErrorCode> {
        self.peer.map(|peer| peer.code)
    }

    fn durable_save_retry_allowed(self) -> bool {
        durable_save_retry_classification(self).is_eligible()
    }
}

impl From<RequestFailure> for ServiceFailure {
    fn from(error: RequestFailure) -> Self {
        error.failure
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DurableSaveRetryClassification {
    LocalSessionLoss,
    RetryablePeer,
    Integrity,
    Authentication,
    Conflict,
    NonRetryablePeer,
    Other,
}

impl DurableSaveRetryClassification {
    const fn is_eligible(self) -> bool {
        matches!(self, Self::LocalSessionLoss | Self::RetryablePeer)
    }
}

fn durable_save_retry_classification(error: RequestFailure) -> DurableSaveRetryClassification {
    if error.retryable_local_session_loss {
        return DurableSaveRetryClassification::LocalSessionLoss;
    }
    if let Some(peer) = error.peer {
        if peer.code == ErrorCode::EngineConfigConflict {
            return DurableSaveRetryClassification::Conflict;
        }
        if peer.retryable
            && !matches!(
                peer.code,
                ErrorCode::InvalidInput
                    | ErrorCode::IdempotencyConflict
                    | ErrorCode::UnsupportedVersion
                    | ErrorCode::UnsupportedFeature
            )
        {
            return DurableSaveRetryClassification::RetryablePeer;
        }
        return DurableSaveRetryClassification::NonRetryablePeer;
    }
    match error.failure.category {
        ServiceFailureCategory::Integrity => DurableSaveRetryClassification::Integrity,
        ServiceFailureCategory::Authentication => DurableSaveRetryClassification::Authentication,
        _ => DurableSaveRetryClassification::Other,
    }
}

fn local_session_request_loss_is_retryable(error: &ClientRequestError) -> bool {
    match error {
        ClientRequestError::Exchange(DeadlineError::Timeout { .. }) => true,
        ClientRequestError::Exchange(DeadlineError::Peer { error, .. }) => matches!(
            error,
            ExchangeError::Open(_)
                | ExchangeError::Send(EnvelopeSendError::Frame(FrameError::Write(_)))
                | ExchangeError::Receive(EnvelopeReceiveError::Frame(
                    FrameError::Read(_) | FrameError::Truncated { .. },
                ))
        ),
        _ => false,
    }
}

async fn request_payload(
    session: ClientSession,
    frames: &mut FrameFactory,
    request: ClientRequest,
    expected: ExpectedResponse,
    cancel: &CancelHandle,
) -> Result<(ClientSession, ResponsePayload), RequestAttemptError> {
    let protocol_version = session.protocol_version();
    let (envelope, expected_request_id) = make_request_frame(frames, protocol_version, request)
        .map_err(|failure| RequestAttemptError::Terminal {
            failure,
            retryable_local_session_loss: false,
        })?;
    request_envelope_payload(session, envelope, expected_request_id, expected, cancel).await
}

async fn request_envelope_payload(
    session: ClientSession,
    envelope: WireEnvelope,
    expected_request_id: RequestId,
    expected: ExpectedResponse,
    cancel: &CancelHandle,
) -> Result<(ClientSession, ResponsePayload), RequestAttemptError> {
    let (session, resolved) =
        session
            .request(envelope, cancel)
            .await
            .map_err(|error| RequestAttemptError::Terminal {
                failure: ServiceFailure::local_session(),
                retryable_local_session_loss: local_session_request_loss_is_retryable(&error),
            })?;
    let (settled_request_id, outcome) = resolved.into_parts();
    if !request_id_matches(&expected_request_id, &settled_request_id) {
        return Err(RequestAttemptError::Retained {
            session: Box::new(session),
            failure: ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ),
            peer: None,
        });
    }

    match outcome {
        RequestOutcome::Failure(failure) => {
            if !optional_request_id_matches(&expected_request_id, failure.request_id.as_ref()) {
                return Err(RequestAttemptError::Retained {
                    session: Box::new(session),
                    failure: ServiceFailure::new(
                        ServiceFailureStage::Request,
                        ServiceFailureCategory::Integrity,
                    ),
                    peer: None,
                });
            }
            let peer = PeerFailure {
                code: failure.code,
                retryable: failure.retryable,
            };
            Err(RequestAttemptError::Retained {
                session: Box::new(session),
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Peer,
                ),
                peer: Some(peer),
            })
        }
        RequestOutcome::Response(response) => {
            if !request_id_matches(&expected_request_id, &response.request_id) {
                return Err(RequestAttemptError::Retained {
                    session: Box::new(session),
                    failure: ServiceFailure::new(
                        ServiceFailureStage::Request,
                        ServiceFailureCategory::Integrity,
                    ),
                    peer: None,
                });
            }
            match validate_response_family(expected, response.payload) {
                Ok(payload) => Ok((session, payload)),
                Err(failure) => Err(RequestAttemptError::Retained {
                    session: Box::new(session),
                    failure,
                    peer: None,
                }),
            }
        }
    }
}

fn request_id_matches(expected: &RequestId, actual: &RequestId) -> bool {
    expected == actual
}

fn optional_request_id_matches(expected: &RequestId, actual: Option<&RequestId>) -> bool {
    actual == Some(expected)
}

fn validate_response_family(
    expected: ExpectedResponse,
    payload: ResponsePayload,
) -> Result<ResponsePayload, ServiceFailure> {
    match (expected, payload) {
        (ExpectedResponse::Directory, ResponsePayload::DirectoryPicked(outcome)) => {
            Ok(ResponsePayload::DirectoryPicked(outcome))
        }
        (ExpectedResponse::Projects, ResponsePayload::ProjectListing(listing)) => {
            Ok(ResponsePayload::ProjectListing(listing))
        }
        (
            ExpectedResponse::AttachedProject,
            ResponsePayload::AttachedProject {
                project,
                disposition,
            },
        ) => Ok(ResponsePayload::AttachedProject {
            project,
            disposition,
        }),
        (
            ExpectedResponse::CreatedThread,
            ResponsePayload::CreatedThread {
                thread,
                disposition,
            },
        ) => Ok(ResponsePayload::CreatedThread {
            thread,
            disposition,
        }),
        (ExpectedResponse::Threads(project_id), ResponsePayload::ThreadListing(listing)) => {
            if listing
                .threads()
                .iter()
                .any(|thread| thread.project_id != project_id)
            {
                return Err(ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ));
            }
            Ok(ResponsePayload::ThreadListing(listing))
        }
        (
            ExpectedResponse::Snapshot(thread_id),
            ResponsePayload::ConversationSnapshot(snapshot),
        ) if snapshot.thread_id() == &thread_id => {
            Ok(ResponsePayload::ConversationSnapshot(snapshot))
        }
        (
            ExpectedResponse::ThreadEngineSettings(thread_id),
            ResponsePayload::ThreadEngineSettings(result),
        ) if result.thread_id() == &thread_id => Ok(ResponsePayload::ThreadEngineSettings(result)),
        (
            ExpectedResponse::RegisteredProfiles,
            ResponsePayload::RegisteredEngineProfiles(result),
        ) => Ok(ResponsePayload::RegisteredEngineProfiles(result)),
        (
            ExpectedResponse::ThreadEngineConfigSet {
                thread_id,
                request_id,
            },
            ResponsePayload::ThreadEngineConfigSet(result),
        ) if result.thread_id == thread_id && result.request_id == request_id => {
            Ok(ResponsePayload::ThreadEngineConfigSet(result))
        }
        (
            ExpectedResponse::FirstMessageQueued {
                thread_id,
                request_id,
            },
            ResponsePayload::FirstMessageQueued(receipt),
        ) if receipt.thread_id == thread_id && receipt.request_id == request_id => {
            Ok(ResponsePayload::FirstMessageQueued(receipt))
        }
        (
            ExpectedResponse::ConversationSubscriptionStarted { thread_id },
            ResponsePayload::ConversationSubscriptionStarted(started),
        ) => {
            let actual_thread = match &started {
                ConversationSubscriptionStarted::Fresh(start) => start.snapshot().thread_id(),
                ConversationSubscriptionStarted::Resumed { thread_id, .. } => thread_id,
            };
            if actual_thread != &thread_id {
                return Err(ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ));
            }
            Ok(ResponsePayload::ConversationSubscriptionStarted(started))
        }
        (
            ExpectedResponse::ConversationSubscriptionStopped { thread_id },
            ResponsePayload::ConversationSubscriptionStopped(stopped),
        ) if stopped.thread_id == thread_id => {
            Ok(ResponsePayload::ConversationSubscriptionStopped(stopped))
        }
        _ => Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        )),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CustodyStep {
    SessionShutdown,
    ReconnectQuarantine,
    LeaseShutdown,
    ReconnectRelease,
    Stopped,
}

fn cleanup_plan(has_session: bool, has_reconnect_lease: bool, has_lease: bool) -> Vec<CustodyStep> {
    let mut plan = Vec::with_capacity(5);
    if has_session {
        plan.push(CustodyStep::SessionShutdown);
    }
    if has_reconnect_lease {
        plan.push(CustodyStep::ReconnectQuarantine);
    }
    if has_lease {
        plan.push(CustodyStep::LeaseShutdown);
    }
    if has_reconnect_lease {
        plan.push(CustodyStep::ReconnectRelease);
    }
    plan.push(CustodyStep::Stopped);
    plan
}

/// One private retry plan for a project intake. Stable mutations retain their
/// complete command identity; reads and the picker are intentionally retried
/// with fresh frames.
enum IntakeRetry {
    Pick,
    Attach(StableMutation),
    RefreshProjects {
        attached: ProjectSummary,
    },
    Create(StableMutation),
    RefreshThreads {
        project_id: ProjectId,
        created: ThreadSummary,
    },
}

struct IntakeState {
    selected_directory: Option<DirectoryId>,
    projects: Option<ProjectListing>,
    retry: Option<IntakeRetry>,
}

impl IntakeState {
    fn new() -> Self {
        Self {
            selected_directory: None,
            projects: None,
            retry: None,
        }
    }

    fn reset(&mut self) {
        self.selected_directory = None;
        self.projects = None;
        self.retry = None;
    }
}

struct SessionMaterial {
    certificate: CertificateDer<'static>,
    target: LoopbackTarget,
    pinned_identity: PinnedIdentity,
    limits: ClientSessionLimits,
    binding: ReconnectBinding,
}

/// Minimal custody for the active subscription; the `ConversationHost` is the projection authority.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubscriptionCustody {
    active_thread: Option<ThreadId>,
    pending_after: Option<ConversationCursor>,
    last_accepted_cursor: Option<ConversationCursor>,
}

impl SubscriptionCustody {
    /// Creates empty custody.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a new subscribe, tombstoning the old thread without restarting the receiver.
    pub fn on_subscribe(&mut self, thread_id: ThreadId, after: Option<ConversationCursor>) {
        if self.active_thread.as_ref() != Some(&thread_id) {
            self.active_thread = Some(thread_id);
            self.last_accepted_cursor = None;
        }
        self.pending_after = after;
    }

    /// Derives and records a Fresh or Resumed server cursor without advancing
    /// application custody.
    ///
    /// # Errors
    ///
    /// Returns an integrity failure when the response does not match the
    /// active thread, pending request, requested mode/cursor, or accepted
    /// cursor floor.
    pub fn on_started(
        &mut self,
        expected_after: Option<ConversationCursor>,
        started: &ConversationSubscriptionStarted,
    ) -> Result<(), ServiceFailure> {
        let (thread_id, cursor) = match started {
            ConversationSubscriptionStarted::Fresh(start) => {
                (start.snapshot().thread_id(), start.snapshot().cursor())
            }
            ConversationSubscriptionStarted::Resumed { thread_id, cursor } => (thread_id, *cursor),
        };
        let matches_request = match (expected_after, started) {
            (None, ConversationSubscriptionStarted::Fresh(_)) => true,
            (Some(expected), ConversationSubscriptionStarted::Resumed { cursor, .. }) => {
                *cursor == expected
            }
            _ => false,
        };
        if !matches_request
            || self.pending_after != expected_after
            || self.active_thread.as_ref() != Some(thread_id)
            || self
                .last_accepted_cursor
                .is_some_and(|last| cursor.get() < last.get())
        {
            return Err(ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ));
        }
        self.pending_after = Some(cursor);
        Ok(())
    }

    /// Advances cursor only after explicit application acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns an integrity failure for a stale thread or a cursor behind the
    /// accepted or pending cursor.
    pub fn on_acknowledge(
        &mut self,
        thread_id: &ThreadId,
        cursor: ConversationCursor,
    ) -> Result<(), ServiceFailure> {
        if self.active_thread.as_ref() != Some(thread_id) {
            return Err(ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ));
        }
        if self
            .last_accepted_cursor
            .is_some_and(|last| cursor.get() < last.get())
            || self
                .pending_after
                .is_some_and(|after| cursor.get() < after.get())
        {
            return Err(ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ));
        }
        self.last_accepted_cursor = Some(cursor);
        self.pending_after = None;
        Ok(())
    }

    /// Clears custody on unsubscribe.
    pub fn on_unsubscribe(&mut self, thread_id: &ThreadId) {
        if self.active_thread.as_ref() == Some(thread_id) {
            self.active_thread = None;
            self.pending_after = None;
            self.last_accepted_cursor = None;
        }
    }

    /// Returns the active thread.
    #[must_use]
    pub fn active_thread(&self) -> Option<&ThreadId> {
        self.active_thread.as_ref()
    }

    /// Returns the last application-accepted cursor.
    #[must_use]
    pub fn last_accepted_cursor(&self) -> Option<ConversationCursor> {
        self.last_accepted_cursor
    }

    /// Returns the pending after cursor for the in-flight subscribe.
    #[must_use]
    pub fn pending_after(&self) -> Option<ConversationCursor> {
        self.pending_after
    }
}

struct ServiceRuntime {
    session: Option<ClientSession>,
    reconnect_lease: Option<ReconnectSessionLease>,
    reconnect_binding: ReconnectBinding,
    certificate: CertificateDer<'static>,
    target: LoopbackTarget,
    pinned_identity: PinnedIdentity,
    limits: ClientSessionLimits,
    lease: Option<ForgeProcessLease>,
    cancel: CancelHandle,
    shutdown_grace: Duration,
    known_threads: HashSet<ThreadId>,
    intake: IntakeState,
    custody: SubscriptionCustody,
    delivery_cancel: Option<Arc<CancelHandle>>,
    delivery_join: Option<tokio::task::JoinHandle<()>>,
    delivery_tx: Option<tokio::sync::mpsc::Sender<PrivateDelivery>>,
}

impl ServiceRuntime {
    async fn reconnect(
        &mut self,
        frames: &mut FrameFactory,
        restore_subscription: bool,
    ) -> Result<(), ServiceFailure> {
        // Custody order: cancel delivery task, await its join exactly once,
        // drop the old session, check out the fenced capability, connect,
        // publish the rotated capability, take_delivery exactly once, and
        // start one new delivery task. Request-driven reconnects additionally
        // restore the active subscription before their request continues;
        // delivery-loss recovery leaves that one subscribe to the application.
        if let Some(cancel) = self.delivery_cancel.take() {
            cancel.cancel();
        }
        if let Some(join) = self.delivery_join.take() {
            let _ = join.await;
        }
        drop(self.session.take());
        let lease = self
            .reconnect_lease
            .take()
            .ok_or(ServiceFailure::local_session())?;
        let mut attempt = lease
            .begin_reconnect()
            .map_err(|_| reconnect_custody_failure())?;
        let capability = attempt
            .take_credential()
            .map_err(|_| reconnect_custody_failure())?;
        let hello = match reconnect_hello_with_capability(frames, capability) {
            Ok(hello) => hello,
            Err((failure, capability)) => {
                if let Ok(lease) = attempt.restore_before_handshake(capability) {
                    self.reconnect_lease = Some(lease);
                }
                return Err(failure);
            }
        };
        let connected = ClientSession::connect(
            self.target,
            self.certificate.clone(),
            self.pinned_identity,
            hello,
            self.limits,
            &self.cancel,
        )
        .await;
        let (session, welcome) = match connected {
            Ok(connected) => connected,
            Err(_) => {
                let _ = attempt.quarantine();
                return Err(ServiceFailure::new(
                    ServiceFailureStage::Handshake,
                    ServiceFailureCategory::Authentication,
                ));
            }
        };
        let reconnect_lease = attempt
            .publish_next(self.reconnect_binding, welcome.welcome.reconnect_capability)
            .map_err(|_| reconnect_custody_failure())?;
        // take_delivery exactly once
        let (session, receiver) = match session.take_delivery() {
            Ok(parts) => parts,
            Err(_) => {
                let _ = reconnect_lease.quarantine();
                return Err(ServiceFailure::local_session());
            }
        };
        let Some(tx) = self.delivery_tx.clone() else {
            drop(session);
            let _ = reconnect_lease.quarantine();
            return Err(ServiceFailure::local_session());
        };
        let version = session.protocol_version();
        self.session = Some(session);
        self.reconnect_lease = Some(reconnect_lease);
        let cancel = Arc::new(CancelHandle::new());
        let join = tokio::spawn(delivery_task_loop(
            receiver,
            tx,
            Arc::clone(&cancel),
            version,
        ));
        self.delivery_cancel = Some(cancel);
        self.delivery_join = Some(join);
        if restore_subscription && let Some(thread_id) = self.custody.active_thread().cloned() {
            let after = self.custody.last_accepted_cursor();
            if let Err(failure) = self
                .resubscribe_after_reconnect(frames, thread_id, after)
                .await
            {
                self.quarantine_reconnect_lease();
                return Err(failure);
            }
        }
        Ok(())
    }

    fn quarantine_reconnect_lease(&mut self) {
        if let Some(lease) = self.reconnect_lease.take() {
            let _ = lease.quarantine();
        }
    }

    async fn resubscribe_after_reconnect(
        &mut self,
        frames: &mut FrameFactory,
        thread_id: ThreadId,
        after: Option<ConversationCursor>,
    ) -> Result<(), ServiceFailure> {
        if self.custody.active_thread() != Some(&thread_id) {
            return Ok(());
        }
        self.custody.on_subscribe(thread_id.clone(), after);
        let subscribe = match after {
            Some(cursor) => ConversationSubscribe::resume(thread_id.clone(), cursor),
            None => ConversationSubscribe::fresh(thread_id.clone()),
        };
        let request = ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe));
        let protocol_version = self
            .session
            .as_ref()
            .map(ClientSession::protocol_version)
            .ok_or(ServiceFailure::local_session())?;
        let (envelope, request_id) = make_request_frame(frames, protocol_version, request)
            .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
        let expected = ExpectedResponse::ConversationSubscriptionStarted {
            thread_id: thread_id.clone(),
        };
        let session = self.session.take().ok_or(ServiceFailure::local_session())?;
        let attempt = request_envelope_payload(
            session,
            envelope,
            request_id.clone(),
            expected,
            &self.cancel,
        )
        .await;
        match self.finish_request_attempt(attempt) {
            Ok(ResponsePayload::ConversationSubscriptionStarted(started)) => {
                if validate_started_correlation(&thread_id, &started).is_err() {
                    return Err(ServiceFailure::new(
                        ServiceFailureStage::Request,
                        ServiceFailureCategory::Integrity,
                    ));
                }
                self.custody.on_started(after, &started)?;
                Ok(())
            }
            Ok(_) => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
            Err(error) => Err(error.into()),
        }
    }

    async fn ensure_session(
        &mut self,
        frames: &mut FrameFactory,
        force_reconnect: bool,
    ) -> Result<(), ServiceFailure> {
        let needs_reconnect = match self.session.as_ref() {
            Some(session) => session_needs_reconnect(
                session.admitted(),
                session.admission_budget(),
                force_reconnect,
            ),
            None => true,
        };
        if needs_reconnect {
            self.reconnect(frames, true).await?;
        }
        Ok(())
    }

    fn finish_request_attempt(
        &mut self,
        attempt: Result<(ClientSession, ResponsePayload), RequestAttemptError>,
    ) -> Result<ResponsePayload, RequestFailure> {
        match attempt {
            Ok((session, payload)) => {
                self.session = Some(session);
                Ok(payload)
            }
            Err(RequestAttemptError::Retained {
                session,
                failure,
                peer,
            }) => {
                self.session = Some(*session);
                Err(RequestFailure {
                    failure,
                    peer,
                    retryable_local_session_loss: false,
                })
            }
            Err(RequestAttemptError::Terminal {
                failure,
                retryable_local_session_loss,
            }) => {
                self.session = None;
                Err(RequestFailure {
                    failure,
                    peer: None,
                    retryable_local_session_loss,
                })
            }
        }
    }

    async fn request(
        &mut self,
        frames: &mut FrameFactory,
        request: ClientRequest,
        expected: ExpectedResponse,
    ) -> Result<ResponsePayload, RequestFailure> {
        self.ensure_session(frames, false)
            .await
            .map_err(RequestFailure::terminal)?;
        let session = self
            .session
            .take()
            .ok_or(RequestFailure::terminal(ServiceFailure::local_session()))?;
        let attempt = request_payload(session, frames, request, expected, &self.cancel).await;
        self.finish_request_attempt(attempt)
    }

    async fn request_stable(
        &mut self,
        frames: &mut FrameFactory,
        mutation: &StableMutation,
        expected: ExpectedResponse,
        force_reconnect: bool,
    ) -> Result<ResponsePayload, RequestFailure> {
        self.ensure_session(frames, force_reconnect)
            .await
            .map_err(RequestFailure::terminal)?;
        let protocol_version = self
            .session
            .as_ref()
            .map(ClientSession::protocol_version)
            .ok_or(RequestFailure::terminal(ServiceFailure::local_session()))?;
        let (envelope, expected_request_id) = mutation
            .envelope(protocol_version)
            .map_err(RequestFailure::terminal)?;
        let session = self
            .session
            .take()
            .ok_or(RequestFailure::terminal(ServiceFailure::local_session()))?;
        let attempt = request_envelope_payload(
            session,
            envelope,
            expected_request_id,
            expected,
            &self.cancel,
        )
        .await;
        self.finish_request_attempt(attempt)
    }

    async fn cleanup(&mut self) -> Result<(), ServiceFailure> {
        if let Some(thread_id) = self.custody.active_thread().cloned() {
            self.custody.on_unsubscribe(&thread_id);
        }
        if let Some(cancel) = self.delivery_cancel.take() {
            cancel.cancel();
        }
        if let Some(join) = self.delivery_join.take() {
            let _ = join.await;
        }
        let mut failed = false;
        for step in cleanup_plan(
            self.session.is_some(),
            self.reconnect_lease.is_some(),
            self.lease.is_some(),
        ) {
            match step {
                CustodyStep::SessionShutdown => {
                    if let Some(session) = self.session.take()
                        && session.shutdown(&self.cancel).await.is_err()
                    {
                        failed = true;
                    }
                }
                CustodyStep::ReconnectQuarantine => {
                    if let Some(lease) = self.reconnect_lease.take() {
                        match lease.quarantine_for_shutdown() {
                            Ok(lease) => self.reconnect_lease = Some(lease),
                            Err(_) => failed = true,
                        }
                    }
                }
                CustodyStep::LeaseShutdown => {
                    if let Some(lease) = self.lease.take()
                        && lease.shutdown(self.shutdown_grace).await.is_err()
                    {
                        failed = true;
                    }
                }
                CustodyStep::ReconnectRelease => drop(self.reconnect_lease.take()),
                CustodyStep::Stopped => {}
            }
        }
        if failed {
            Err(ServiceFailure::new(
                ServiceFailureStage::Cleanup,
                ServiceFailureCategory::Cleanup,
            ))
        } else {
            Ok(())
        }
    }
}

fn session_needs_reconnect(admitted: usize, admission_budget: usize, force: bool) -> bool {
    force || admitted >= admission_budget
}

fn reconnect_custody_failure() -> ServiceFailure {
    ServiceFailure::new(
        ServiceFailureStage::Handshake,
        ServiceFailureCategory::Authentication,
    )
}

struct ReconnectHelloHeader {
    frame_id: FrameId,
    sent_at: UnixMillis,
    supported_versions: VersionOffer,
}

fn reconnect_hello_header(
    frames: &mut FrameFactory,
) -> Result<ReconnectHelloHeader, ServiceFailure> {
    let stamp = frames
        .next()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Handshake))?;
    let supported_versions = VersionOffer::new(vec![1])
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Handshake))?;
    Ok(ReconnectHelloHeader {
        frame_id: stamp.frame_id,
        sent_at: stamp.sent_at,
        supported_versions,
    })
}

fn reconnect_hello_from_header(
    header: ReconnectHelloHeader,
    capability: artisan_protocol::ReconnectCapability,
) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: header.frame_id,
        sent_at: header.sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: header.supported_versions,
            credential: HelloCredential::Reconnect(capability),
            supports_lifecycle_control: false,
        }),
    }
}

fn reconnect_hello_with_capability(
    frames: &mut FrameFactory,
    capability: artisan_protocol::ReconnectCapability,
) -> Result<WireEnvelope, (ServiceFailure, artisan_protocol::ReconnectCapability)> {
    let header = match reconnect_hello_header(frames) {
        Ok(header) => header,
        Err(failure) => return Err((failure, capability)),
    };
    Ok(reconnect_hello_from_header(header, capability))
}

fn reconnect_hello(
    frames: &mut FrameFactory,
    capability: artisan_protocol::ReconnectCapability,
) -> Result<WireEnvelope, ServiceFailure> {
    reconnect_hello_with_capability(frames, capability).map_err(|(failure, _)| failure)
}

fn build_reconnect_binding(
    instance_id: [u8; 16],
    target: LoopbackTarget,
    pinned_identity: PinnedIdentity,
    forge_pid: u32,
) -> Result<ReconnectBinding, StartupError> {
    ReconnectBinding::new(
        instance_id,
        target.addr().port(),
        *pinned_identity.as_bytes(),
        NonZeroU32::new(forge_pid).ok_or(StartupError::Stage(ServiceFailureStage::Readiness))?,
    )
    .map_err(|_| StartupError::Stage(ServiceFailureStage::Instance))
}

async fn start_native_service() -> Result<(ServiceRuntime, FrameFactory), StartupError> {
    let layout =
        Layout::discover().map_err(|_| StartupError::Stage(ServiceFailureStage::Layout))?;
    let manifest = InstallationManifest::load(&layout.manifest)
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Manifest))?;
    if manifest.install_root != layout.root {
        return Err(StartupError::Stage(ServiceFailureStage::Manifest));
    }
    match artisan_editor_cli::payload::verify(&manifest.version_root()) {
        artisan_editor_cli::payload::PayloadHealth::Verified => {}
        artisan_editor_cli::payload::PayloadHealth::Modified(_)
        | artisan_editor_cli::payload::PayloadHealth::Unverifiable => {
            return Err(StartupError::PayloadUnverified);
        }
    }
    let config = NativeInstanceConfig::load_from_home(&layout.root)
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Instance))?;
    let credentials = load_client_credentials(&layout.root)
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Credentials))?;
    let launch_spec = ForgeLaunchSpec::new(&manifest, &config, credentials.paths())
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Forge))?;
    let lease = start_owned(&launch_spec)
        .await
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Forge))?;
    let mut frames = FrameFactory::new();
    let runtime =
        attach_to_owned_forge(lease, &layout.root, &config, credentials, &mut frames).await?;
    Ok((runtime, frames))
}

async fn attach_to_owned_forge(
    lease: ForgeProcessLease,
    home: &Path,
    config: &NativeInstanceConfig,
    credentials: NativeClientCredentials,
    frames: &mut FrameFactory,
) -> Result<ServiceRuntime, StartupError> {
    let result = establish_session(home, config, &lease, credentials, frames).await;
    match result {
        Ok((session, reconnect_lease, cancel, shutdown_grace, material)) => Ok(ServiceRuntime {
            session: Some(session),
            reconnect_lease: Some(reconnect_lease),
            reconnect_binding: material.binding,
            certificate: material.certificate,
            target: material.target,
            pinned_identity: material.pinned_identity,
            limits: material.limits,
            lease: Some(lease),
            cancel,
            shutdown_grace,
            known_threads: HashSet::new(),
            intake: IntakeState::new(),
            custody: SubscriptionCustody::new(),
            delivery_cancel: None,
            delivery_join: None,
            delivery_tx: None,
        }),
        Err(error) => {
            let shutdown_grace =
                finite_duration(config.listener().drain_timeout_ms()).unwrap_or(Duration::ZERO);
            let _ = lease.shutdown(shutdown_grace).await;
            Err(error)
        }
    }
}

async fn establish_session(
    home: &Path,
    config: &NativeInstanceConfig,
    lease: &ForgeProcessLease,
    credentials: NativeClientCredentials,
    frames: &mut FrameFactory,
) -> Result<
    (
        ClientSession,
        ReconnectSessionLease,
        CancelHandle,
        Duration,
        SessionMaterial,
    ),
    StartupError,
> {
    let readiness_endpoint = lease.readiness().endpoint().to_owned();
    let readiness_process_id = lease.readiness().pid();
    let readiness_certificate_pin = lease.readiness().certificate_sha256().to_owned();
    let (certificate, capability) = credentials.into_parts();
    let pinned_identity = PinnedIdentity::from_certificate(&certificate);
    let expected_pin = pinned_identity.to_hex();
    let target = validate_readiness(
        &readiness_endpoint,
        readiness_process_id,
        lease.pid(),
        &readiness_certificate_pin,
        &expected_pin,
    )
    .map_err(|_| StartupError::Stage(ServiceFailureStage::Readiness))?;
    let binding =
        build_reconnect_binding(config.instance_id(), target, pinned_identity, lease.pid())?;

    let hello_stamp = frames.next()?;
    let hello = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: hello_stamp.frame_id,
        sent_at: hello_stamp.sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])
                .map_err(|_| StartupError::Stage(ServiceFailureStage::Handshake))?,
            credential: HelloCredential::Initial(capability),
            supports_lifecycle_control: false,
        }),
    };
    let listener = config.listener();
    let limits = ClientSessionLimits {
        connect: finite_duration(listener.admission_timeout_ms())?,
        handshake: finite_duration(listener.handshake_timeout_ms())?,
        request: finite_duration(listener.request_timeout_ms())?,
        shutdown: finite_duration(listener.drain_timeout_ms())?,
        admission_budget: usize::try_from(listener.requests_per_connection().get())
            .map_err(|_| StartupError::Stage(ServiceFailureStage::Instance))?,
    };
    let trusted_certificate = certificate.clone();
    let cancel = CancelHandle::new();
    let (session, welcome) =
        ClientSession::connect(target, certificate, pinned_identity, hello, limits, &cancel)
            .await
            .map_err(|_| StartupError::Stage(ServiceFailureStage::Handshake))?;
    let reconnect_store = match ReconnectCapabilityStore::from_home(home) {
        Ok(store) => store,
        Err(_) => {
            let _ = session.shutdown(&cancel).await;
            return Err(StartupError::Stage(ServiceFailureStage::Credentials));
        }
    };
    let reconnect_lease = match reconnect_store.initialize_owner_lease(
        binding,
        welcome.welcome.reconnect_capability,
        RECONNECT_LOCK_TIMEOUT,
    ) {
        Ok(lease) => lease,
        Err(_) => {
            let _ = session.shutdown(&cancel).await;
            return Err(StartupError::Stage(ServiceFailureStage::Credentials));
        }
    };
    Ok((
        session,
        reconnect_lease,
        cancel,
        limits.shutdown,
        SessionMaterial {
            certificate: trusted_certificate,
            target,
            pinned_identity,
            limits,
            binding,
        },
    ))
}

/// Validates one uni-stream envelope and extracts a patch batch.
///
/// # Errors
///
/// Returns an integrity failure when the protocol version or envelope body
/// does not match the expected delivery family.
pub fn validate_uni_envelope(
    envelope: &WireEnvelope,
    expected_version: ProtocolVersion,
) -> Result<PatchBatch, ServiceFailure> {
    if envelope.protocol_version != expected_version {
        return Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        ));
    }
    match &envelope.body {
        WireEnvelopeBody::PatchBatch(batch) => Ok(batch.clone()),
        _ => Err(ServiceFailure::new(
            ServiceFailureStage::Delivery,
            ServiceFailureCategory::Integrity,
        )),
    }
}

/// Validates the thread identity in a subscription-start response.
///
/// # Errors
///
/// Returns an integrity failure when the response names a different thread.
pub fn validate_started_correlation(
    expected_thread_id: &ThreadId,
    started: &ConversationSubscriptionStarted,
) -> Result<(), ServiceFailure> {
    let actual_thread = match started {
        ConversationSubscriptionStarted::Fresh(start) => start.snapshot().thread_id(),
        ConversationSubscriptionStarted::Resumed { thread_id, .. } => thread_id,
    };
    if actual_thread != expected_thread_id {
        return Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        ));
    }
    Ok(())
}

/// Validates the thread identity in a subscription-stop response.
///
/// # Errors
///
/// Returns an integrity failure when the response names a different thread.
pub fn validate_stopped_correlation(
    expected_thread_id: &ThreadId,
    stopped: &ConversationSubscriptionStopped,
) -> Result<(), ServiceFailure> {
    if &stopped.thread_id != expected_thread_id {
        return Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        ));
    }
    Ok(())
}

pub async fn delivery_task_loop(
    mut receiver: DeliveryReceiver,
    tx: tokio::sync::mpsc::Sender<PrivateDelivery>,
    cancel: Arc<CancelHandle>,
    expected_version: ProtocolVersion,
) {
    loop {
        if let Ok((next_receiver, envelope)) = receiver.recv(cancel.as_ref()).await {
            receiver = next_receiver;
            let result = match validate_uni_envelope(&envelope, expected_version) {
                Ok(batch) => PrivateDelivery::Batch(batch),
                Err(failure) => PrivateDelivery::Lost(failure),
            };
            let is_lost = matches!(result, PrivateDelivery::Lost(_));
            if tx.send(result).await.is_err() {
                break;
            }
            if is_lost {
                break;
            }
        } else {
            let _ = tx
                .send(PrivateDelivery::Lost(ServiceFailure::new(
                    ServiceFailureStage::Delivery,
                    ServiceFailureCategory::LocalSession,
                )))
                .await;
            break;
        }
    }
}

async fn service_main(
    mut commands: tokio::sync::mpsc::Receiver<NativeTransportCommand>,
    events: SyncSender<NativeTransportEvent>,
) {
    let mut status = ServiceStopStatus::Clean;
    let started = start_native_service().await;
    match started {
        Ok((mut runtime, mut frames)) => {
            let (delivery_tx, mut delivery_rx) = tokio::sync::mpsc::channel::<PrivateDelivery>(64);
            runtime.delivery_tx = Some(delivery_tx.clone());
            // take_delivery exactly once for this session
            let delivery_started = {
                let session = runtime.session.take();
                match session {
                    Some(session) => match session.take_delivery() {
                        Ok((session, receiver)) => {
                            runtime.session = Some(session);
                            let cancel = Arc::new(CancelHandle::new());
                            let version = runtime
                                .session
                                .as_ref()
                                .map_or(ProtocolVersion::V1, ClientSession::protocol_version);
                            let join = tokio::spawn(delivery_task_loop(
                                receiver,
                                delivery_tx.clone(),
                                Arc::clone(&cancel),
                                version,
                            ));
                            runtime.delivery_cancel = Some(cancel);
                            runtime.delivery_join = Some(join);
                            Ok(())
                        }
                        Err(_) => Err(ServiceFailure::local_session()),
                    },
                    None => Err(ServiceFailure::local_session()),
                }
            };
            if let Err(failure) = delivery_started {
                status = ServiceStopStatus::Failed;
                let _ = publish(&events, NativeTransportEvent::Failed(failure));
            } else {
                let run_result = load_initial_catalog(&mut runtime, &mut frames, &events).await;
                if let Err(failure) = run_result {
                    status = ServiceStopStatus::Failed;
                    let _ = publish(&events, NativeTransportEvent::Failed(failure));
                } else {
                    let command_result = command_loop_with_delivery(
                        &mut commands,
                        &mut delivery_rx,
                        &mut runtime,
                        &mut frames,
                        &events,
                    )
                    .await;
                    if let Err(failure) = command_result {
                        status = ServiceStopStatus::Failed;
                        let _ = publish(&events, NativeTransportEvent::Failed(failure));
                    }
                }
            }
            if runtime.cleanup().await.is_err() {
                status = ServiceStopStatus::Failed;
                let _ = publish(
                    &events,
                    NativeTransportEvent::Failed(ServiceFailure::new(
                        ServiceFailureStage::Cleanup,
                        ServiceFailureCategory::Cleanup,
                    )),
                );
            }
        }
        Err(error) => {
            status = ServiceStopStatus::Failed;
            let _ = publish(&events, NativeTransportEvent::Failed(error.failure()));
        }
    }
    let _ = publish(&events, NativeTransportEvent::Stopped(status));
}

async fn handle_subscribe(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    after: Option<ConversationCursor>,
) -> Result<(), RequestFailure> {
    // Thread switch tombstones old thread but does not restart the session delivery receiver.
    // Do not create an empty projection authority; the ConversationHost is the authority.
    runtime.custody.on_subscribe(thread_id.clone(), after);
    let subscribe = match after {
        Some(cursor) => ConversationSubscribe::resume(thread_id.clone(), cursor),
        None => ConversationSubscribe::fresh(thread_id.clone()),
    };
    let request = ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe));
    let protocol_version = runtime
        .session
        .as_ref()
        .map(ClientSession::protocol_version)
        .ok_or(ServiceFailure::local_session())
        .map_err(RequestFailure::terminal)?;
    let (envelope, request_id) =
        make_request_frame(frames, protocol_version, request).map_err(RequestFailure::terminal)?;
    let expected = ExpectedResponse::ConversationSubscriptionStarted {
        thread_id: thread_id.clone(),
    };
    let session = runtime
        .session
        .take()
        .ok_or(ServiceFailure::local_session())
        .map_err(RequestFailure::terminal)?;
    let attempt = request_envelope_payload(
        session,
        envelope,
        request_id.clone(),
        expected,
        &runtime.cancel,
    )
    .await;
    match runtime.finish_request_attempt(attempt) {
        Ok(ResponsePayload::ConversationSubscriptionStarted(started)) => {
            if validate_started_correlation(&thread_id, &started).is_err() {
                let failure = ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                );
                return Err(RequestFailure::terminal(failure));
            }
            runtime
                .custody
                .on_started(after, &started)
                .map_err(RequestFailure::terminal)?;
            // Do not install into a second projection; emit directly.
            // Cursor will be advanced only after explicit AcknowledgePatch from application.
            publish(
                events,
                NativeTransportEvent::ConversationSubscriptionStarted {
                    thread_id,
                    request_id,
                    started,
                },
            )
            .map_err(RequestFailure::terminal)
        }
        Ok(_) => Err(RequestFailure::terminal(ServiceFailure::invalid(
            ServiceFailureStage::Request,
        ))),
        Err(error) => Err(error),
    }
}

async fn handle_unsubscribe(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
) -> Result<(), ServiceFailure> {
    let is_active = runtime.custody.active_thread() == Some(&thread_id);
    if is_active {
        runtime.custody.on_unsubscribe(&thread_id);
    }
    let request =
        ClientRequest::Conversation(ConversationRequest::Unsubscribe(ConversationUnsubscribe {
            thread_id: thread_id.clone(),
        }));
    let protocol_version = runtime
        .session
        .as_ref()
        .map(ClientSession::protocol_version)
        .ok_or(ServiceFailure::local_session())?;
    let (envelope, request_id) = make_request_frame(frames, protocol_version, request)
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let expected = ExpectedResponse::ConversationSubscriptionStopped {
        thread_id: thread_id.clone(),
    };
    let session = runtime
        .session
        .take()
        .ok_or(ServiceFailure::local_session())?;
    let attempt = request_envelope_payload(
        session,
        envelope,
        request_id.clone(),
        expected,
        &runtime.cancel,
    )
    .await;
    match runtime.finish_request_attempt(attempt) {
        Ok(ResponsePayload::ConversationSubscriptionStopped(stopped)) => {
            if validate_stopped_correlation(&thread_id, &stopped).is_err() {
                let failure = ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                );
                return Err(failure);
            }
            publish(
                events,
                NativeTransportEvent::ConversationSubscriptionStopped {
                    thread_id,
                    request_id,
                    stopped,
                },
            )
        }
        Ok(_) => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
        Err(error) => Err(error.into()),
    }
}

fn handle_acknowledge_patch(
    runtime: &mut ServiceRuntime,
    thread_id: &ThreadId,
    cursor: ConversationCursor,
) -> Result<(), ServiceFailure> {
    runtime.custody.on_acknowledge(thread_id, cursor)
}

async fn handle_delivery_lost_reconnect(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    failure: ServiceFailure,
) -> Result<(), ServiceFailure> {
    // Publish the loss first so application can observe
    publish(events, NativeTransportEvent::DeliveryLost(failure))?;
    // Perform cancel->join->lease checkout->connect->publish->take_delivery->new task.
    // The application owns the one recovery Subscribe from its mounted host cursor.
    runtime.reconnect(frames, false).await
}

async fn handle_subscribe_command(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    after: Option<ConversationCursor>,
) -> Result<(), ServiceFailure> {
    if let Err(failure) = handle_subscribe(runtime, frames, events, thread_id, after).await {
        match subscription_failure_disposition(
            SubscriptionRequestKind::Subscribe,
            failure.failure,
            failure.retryable_local_session_loss,
        ) {
            SubscriptionFailureDisposition::RecoverDelivery => {
                handle_delivery_lost_reconnect(runtime, frames, events, failure.failure).await?;
            }
            SubscriptionFailureDisposition::Terminal => return Err(failure.failure),
        }
    }
    Ok(())
}

async fn command_loop_with_delivery(
    commands: &mut tokio::sync::mpsc::Receiver<NativeTransportCommand>,
    delivery_rx: &mut tokio::sync::mpsc::Receiver<PrivateDelivery>,
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    loop {
        tokio::select! {
            cmd = commands.recv() => {
                match cmd {
                    Some(NativeTransportCommand::Shutdown) | None => return Ok(()),
                    Some(NativeTransportCommand::BeginProjectIntake) => {
                        begin_project_intake(runtime, frames, events).await?;
                    }
                    Some(NativeTransportCommand::RetryProjectIntake) => {
                        retry_project_intake(runtime, frames, events).await?;
                    }
                    Some(NativeTransportCommand::SelectProject(project_id)) => {
                        select_project(runtime, frames, events, project_id).await?;
                    }
                    Some(NativeTransportCommand::RequestSnapshot(thread_id)) => {
                        request_snapshot(runtime, frames, events, thread_id).await?;
                    }
                    Some(NativeTransportCommand::LoadThreadEngineSettings { thread_id, generation }) => {
                        load_thread_engine_settings(runtime, frames, events, thread_id, generation).await?;
                    }
                    Some(NativeTransportCommand::ListRegisteredProfiles) => {
                        list_registered_profiles(runtime, frames, events).await?;
                    }
                    Some(NativeTransportCommand::SetThreadEngineConfig(command)) => {
                        set_thread_engine_config(runtime, frames, events, command).await?;
                    }
                    Some(NativeTransportCommand::QueueFirstMessage(command)) => {
                        queue_first_message(runtime, frames, events, *command).await?;
                    }
                    Some(NativeTransportCommand::Subscribe { thread_id, after }) => {
                        handle_subscribe_command(runtime, frames, events, thread_id, after).await?;
                    }
                    Some(NativeTransportCommand::Unsubscribe { thread_id }) => {
                        // The host is retiring; never turn an unsubscribe failure into a
                        // recovery Subscribe for that same host.
                        handle_unsubscribe(runtime, frames, events, thread_id).await?;
                    }
                    Some(NativeTransportCommand::AcknowledgePatch { thread_id, cursor }) =>
                        handle_acknowledge_patch(runtime, &thread_id, cursor)?,
                }
            }
            delivery = delivery_rx.recv() => {
                match delivery {
                    Some(PrivateDelivery::Batch(batch)) => {
                        let is_stale = runtime
                            .custody
                            .active_thread()
                            .is_none_or(|tid| tid != batch.thread_id());
                        if is_stale {
                            continue;
                        }
                        // Validate cursor continuity against the pending subscribe baseline or
                        // last application-accepted cursor.
                        let Some(expected_from) = runtime
                            .custody
                            .pending_after()
                            .or_else(|| runtime.custody.last_accepted_cursor())
                        else {
                            let failure = ServiceFailure::new(
                                ServiceFailureStage::Delivery,
                                ServiceFailureCategory::Integrity,
                            );
                            handle_delivery_lost_reconnect(runtime, frames, events, failure)
                                .await?;
                            continue;
                        };
                        if batch.from_cursor() != expected_from {
                            let failure = ServiceFailure::new(
                                ServiceFailureStage::Delivery,
                                ServiceFailureCategory::Integrity,
                            );
                            handle_delivery_lost_reconnect(runtime, frames, events, failure).await?;
                            continue;
                        }
                        // Do not advance cursor here; emit to application and wait for explicit ack
                        publish(events, NativeTransportEvent::PatchBatch(batch))?;
                    }
                    Some(PrivateDelivery::Lost(failure)) =>
                        handle_delivery_lost_reconnect(runtime, frames, events, failure).await?,
                    None => {
                        let failure = ServiceFailure::new(
                            ServiceFailureStage::Delivery,
                            ServiceFailureCategory::LocalSession,
                        );
                        handle_delivery_lost_reconnect(runtime, frames, events, failure).await?;
                    }
                }
            }
        }
    }
}

async fn begin_project_intake(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    runtime.intake.reset();
    pick_directory(runtime, frames, events).await
}

async fn retry_project_intake(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    let Some(retry) = runtime.intake.retry.take() else {
        return Ok(());
    };
    match retry {
        IntakeRetry::Pick => {
            runtime.intake.selected_directory = None;
            runtime.intake.projects = None;
            pick_directory(runtime, frames, events).await
        }
        IntakeRetry::Attach(mutation) => {
            attach_project_with_mutation(runtime, frames, events, mutation, true).await
        }
        IntakeRetry::RefreshProjects { attached } => {
            refresh_projects(runtime, frames, events, attached).await
        }
        IntakeRetry::Create(mutation) => {
            let Some(projects) = runtime.intake.projects.clone() else {
                return report_intake_failure(
                    runtime,
                    events,
                    NativeProjectIntakeOperation::CreateThread,
                    RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
                    None,
                    false,
                );
            };
            let Some((project_id, title)) = create_command_values(&mutation) else {
                return report_intake_failure(
                    runtime,
                    events,
                    NativeProjectIntakeOperation::CreateThread,
                    RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
                    None,
                    false,
                );
            };
            create_thread_with_mutation(
                runtime, frames, events, projects, project_id, title, mutation, true,
            )
            .await
        }
        IntakeRetry::RefreshThreads {
            project_id,
            created,
        } => refresh_threads(runtime, frames, events, project_id, created).await,
    }
}

async fn pick_directory(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    runtime.intake.selected_directory = None;
    runtime.intake.projects = None;
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::PickingDirectory),
    )?;
    let payload = match runtime
        .request(
            frames,
            ClientRequest::PickDirectory,
            ExpectedResponse::Directory,
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::PickDirectory,
                error,
                Some(IntakeRetry::Pick),
                true,
            );
        }
    };
    match payload {
        ResponsePayload::DirectoryPicked(artisan_protocol::DirectoryPickOutcome::Selected(
            directory_id,
        )) => {
            runtime.intake.selected_directory = Some(directory_id);
            attach_project(runtime, frames, events).await
        }
        ResponsePayload::DirectoryPicked(artisan_protocol::DirectoryPickOutcome::Cancelled) => {
            runtime.intake.reset();
            publish(events, NativeTransportEvent::ProjectIntakeCancelled)
        }
        _ => report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::PickDirectory,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        ),
    }
}

async fn attach_project(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    let Some(directory_id) = runtime.intake.selected_directory.clone() else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::AttachProject,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    let mutation = match attach_mutation(frames, directory_id) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::AttachProject,
                RequestFailure::terminal(failure),
                None,
                false,
            );
        }
    };
    attach_project_with_mutation(runtime, frames, events, mutation, false).await
}

async fn attach_project_with_mutation(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    mutation: StableMutation,
    force_reconnect: bool,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::AttachingProject),
    )?;
    let payload = match runtime
        .request_stable(
            frames,
            &mutation,
            ExpectedResponse::AttachedProject,
            force_reconnect,
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            let allow_retry = attach_retry_allowed(error);
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::AttachProject,
                error,
                Some(IntakeRetry::Attach(mutation)),
                allow_retry,
            );
        }
    };
    let ResponsePayload::AttachedProject {
        project,
        disposition: _,
    } = payload
    else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::AttachProject,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    refresh_projects(runtime, frames, events, project).await
}

async fn refresh_projects(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    attached: ProjectSummary,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::RefreshingProjects),
    )?;
    let payload = match runtime
        .request(frames, project_request(), ExpectedResponse::Projects)
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::RefreshProjects,
                error,
                Some(IntakeRetry::RefreshProjects { attached }),
                true,
            );
        }
    };
    let ResponsePayload::ProjectListing(projects) = payload else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshProjects,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    if !contains_exact_project(&projects, &attached) {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshProjects,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    }
    runtime.intake.projects = Some(projects.clone());
    let Ok(title) = ThreadTitle::parse("New thread") else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::CreateThread,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    create_thread(
        runtime,
        frames,
        events,
        projects,
        attached.project_id.clone(),
        title,
    )
    .await
}

async fn create_thread(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    projects: ProjectListing,
    project_id: ProjectId,
    title: ThreadTitle,
) -> Result<(), ServiceFailure> {
    let mutation = match create_mutation(frames, project_id.clone(), title.clone()) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::CreateThread,
                RequestFailure::terminal(failure),
                None,
                false,
            );
        }
    };
    create_thread_with_mutation(
        runtime, frames, events, projects, project_id, title, mutation, false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn create_thread_with_mutation(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    projects: ProjectListing,
    project_id: ProjectId,
    title: ThreadTitle,
    mutation: StableMutation,
    force_reconnect: bool,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ProjectIntakeProgress(NativeProjectIntakeStage::CreatingThread),
    )?;
    let payload = match runtime
        .request_stable(
            frames,
            &mutation,
            ExpectedResponse::CreatedThread,
            force_reconnect,
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return report_intake_failure(
                runtime,
                events,
                NativeProjectIntakeOperation::CreateThread,
                error,
                Some(IntakeRetry::Create(mutation)),
                true,
            );
        }
    };
    let ResponsePayload::CreatedThread {
        thread,
        disposition: _,
    } = payload
    else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::CreateThread,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    if thread.project_id != project_id || thread.title != title {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::CreateThread,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    }
    runtime.intake.projects = Some(projects);
    refresh_threads(runtime, frames, events, project_id, thread).await
}

fn create_command_values(mutation: &StableMutation) -> Option<(ProjectId, ThreadTitle)> {
    match &mutation.command {
        Command::CreateThread(command) => Some((command.project_id.clone(), command.title.clone())),
        _ => None,
    }
}

async fn refresh_threads(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    project_id: ProjectId,
    created: ThreadSummary,
) -> Result<(), ServiceFailure> {
    let payload = {
        publish(
            events,
            NativeTransportEvent::ProjectIntakeProgress(
                NativeProjectIntakeStage::RefreshingThreads,
            ),
        )?;
        match runtime
            .request(
                frames,
                threads_request(project_id.clone()),
                ExpectedResponse::Threads(project_id.clone()),
            )
            .await
        {
            Ok(payload) => payload,
            Err(error) => {
                return report_intake_failure(
                    runtime,
                    events,
                    NativeProjectIntakeOperation::RefreshThreads,
                    error,
                    Some(IntakeRetry::RefreshThreads {
                        project_id,
                        created,
                    }),
                    true,
                );
            }
        }
    };
    let ResponsePayload::ThreadListing(threads) = payload else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshThreads,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    if !contains_exact_thread(&threads, &created) {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshThreads,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    }
    let Some(projects) = runtime.intake.projects.clone() else {
        return report_intake_failure(
            runtime,
            events,
            NativeProjectIntakeOperation::RefreshThreads,
            RequestFailure::terminal(ServiceFailure::invalid(ServiceFailureStage::Request)),
            None,
            false,
        );
    };
    runtime.known_threads.clear();
    runtime.known_threads.extend(
        threads
            .threads()
            .iter()
            .map(|thread| thread.thread_id.clone()),
    );
    runtime.intake.reset();
    publish(
        events,
        NativeTransportEvent::ProjectIntakeReady {
            projects,
            project_id,
            threads,
            thread_id: created.thread_id,
        },
    )
}

fn report_intake_failure(
    runtime: &mut ServiceRuntime,
    events: &SyncSender<NativeTransportEvent>,
    operation: NativeProjectIntakeOperation,
    error: RequestFailure,
    retry: Option<IntakeRetry>,
    allow_retry: bool,
) -> Result<(), ServiceFailure> {
    let retryable = retry.is_some() && report_retry_allowed(error, allow_retry);
    runtime.intake.retry = if retryable { retry } else { None };
    publish(
        events,
        NativeTransportEvent::ProjectIntakeFailed {
            operation,
            failure: error.failure,
            retryable,
        },
    )
}

fn report_retry_allowed(error: RequestFailure, allow_retry: bool) -> bool {
    allow_retry && error.retryable()
}

fn attach_retry_allowed(error: RequestFailure) -> bool {
    error.code() != Some(ErrorCode::DirectoryUnknown) && error.retryable()
}

fn contains_exact_project(projects: &ProjectListing, expected: &ProjectSummary) -> bool {
    projects.projects().contains(expected)
}

fn contains_exact_thread(threads: &ThreadListing, expected: &ThreadSummary) -> bool {
    threads.threads().contains(expected)
}

async fn load_initial_catalog(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    let payload = runtime
        .request(frames, project_request(), ExpectedResponse::Projects)
        .await?;
    let ResponsePayload::ProjectListing(listing) = payload else {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    };
    let first_project = listing
        .projects()
        .first()
        .map(|project| project.project_id.clone());
    publish(events, NativeTransportEvent::Projects(listing))?;
    let Some(project_id) = first_project else {
        publish(events, NativeTransportEvent::EmptyProjects)?;
        return Ok(());
    };
    select_project(runtime, frames, events, project_id).await
}

async fn select_project(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    project_id: ProjectId,
) -> Result<(), ServiceFailure> {
    let payload = runtime
        .request(
            frames,
            threads_request(project_id.clone()),
            ExpectedResponse::Threads(project_id.clone()),
        )
        .await?;
    let ResponsePayload::ThreadListing(listing) = payload else {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    };
    runtime.known_threads.clear();
    runtime.known_threads.extend(
        listing
            .threads()
            .iter()
            .map(|thread| thread.thread_id.clone()),
    );
    let selection = thread_selection_decision(&listing);
    publish(
        events,
        NativeTransportEvent::Threads {
            project_id: project_id.clone(),
            listing,
        },
    )?;
    match selection {
        ThreadSelectionDecision::AwaitHostSnapshot(_) => Ok(()),
        ThreadSelectionDecision::Empty => {
            publish(events, NativeTransportEvent::EmptyThreads { project_id })
        }
    }
}

async fn request_snapshot(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
) -> Result<(), ServiceFailure> {
    if !runtime.known_threads.contains(&thread_id) {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let payload = runtime
        .request(
            frames,
            snapshot_request(thread_id.clone())?,
            ExpectedResponse::Snapshot(thread_id),
        )
        .await?;
    let ResponsePayload::ConversationSnapshot(snapshot) = payload else {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    };
    publish(events, NativeTransportEvent::Snapshot(snapshot))
}

async fn load_thread_engine_settings(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    generation: SettingsLoadGeneration,
) -> Result<(), ServiceFailure> {
    if !runtime.known_threads.contains(&thread_id) {
        return publish(
            events,
            NativeTransportEvent::ThreadEngineSettingsFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let payload = match runtime
        .request(
            frames,
            thread_engine_settings_request(thread_id.clone()),
            ExpectedResponse::ThreadEngineSettings(thread_id.clone()),
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            let failure: ServiceFailure = error.into();
            return publish(
                events,
                NativeTransportEvent::ThreadEngineSettingsFailed {
                    thread_id: thread_id.clone(),
                    generation,
                    failure,
                },
            );
        }
    };
    let ResponsePayload::ThreadEngineSettings(result) = payload else {
        return publish(
            events,
            NativeTransportEvent::ThreadEngineSettingsFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    if result.thread_id() != &thread_id {
        return publish(
            events,
            NativeTransportEvent::ThreadEngineSettingsFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    publish(
        events,
        NativeTransportEvent::ThreadEngineSettings { generation, result },
    )
}

async fn list_registered_profiles(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    let payload = match runtime
        .request(
            frames,
            registered_profiles_request(),
            ExpectedResponse::RegisteredProfiles,
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            let failure: ServiceFailure = error.into();
            return publish(
                events,
                NativeTransportEvent::RegisteredProfilesFailed(failure),
            );
        }
    };
    let ResponsePayload::RegisteredEngineProfiles(result) = payload else {
        return publish(
            events,
            NativeTransportEvent::RegisteredProfilesFailed(ServiceFailure::invalid(
                ServiceFailureStage::Request,
            )),
        );
    };
    publish(events, NativeTransportEvent::RegisteredProfiles(result))
}

async fn queue_first_message(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: QueueFirstMessage,
) -> Result<(), ServiceFailure> {
    let thread_id = command.thread_id.clone();
    let request_id = command.request_id.clone();
    if known_thread_for_queue(&runtime.known_threads, &thread_id).is_err() {
        return publish(
            events,
            NativeTransportEvent::FirstMessageFailed {
                thread_id,
                request_id,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let mutation = match first_message_stable_mutation(command) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return publish(
                events,
                NativeTransportEvent::FirstMessageFailed {
                    thread_id,
                    request_id,
                    failure,
                },
            );
        }
    };
    let payload = match durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::FirstMessageQueued {
            thread_id: thread_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return publish(
                events,
                NativeTransportEvent::FirstMessageFailed {
                    thread_id,
                    request_id,
                    failure: error.into(),
                },
            );
        }
    };
    let ResponsePayload::FirstMessageQueued(receipt) = payload else {
        return publish(
            events,
            NativeTransportEvent::FirstMessageFailed {
                thread_id,
                request_id,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    if receipt.thread_id != thread_id || receipt.request_id != request_id {
        return publish(
            events,
            NativeTransportEvent::FirstMessageFailed {
                thread_id,
                request_id,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ),
            },
        );
    }
    publish(events, NativeTransportEvent::FirstMessageQueued(receipt))
}

fn known_thread_for_queue(
    known_threads: &HashSet<ThreadId>,
    thread_id: &ThreadId,
) -> Result<(), ServiceFailure> {
    if known_threads.contains(thread_id) {
        Ok(())
    } else {
        Err(ServiceFailure::invalid(ServiceFailureStage::Request))
    }
}

async fn set_thread_engine_config(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: Box<SetThreadEngineConfig>,
) -> Result<(), ServiceFailure> {
    let thread_id = command.thread_id().clone();
    let request_id = command.request_id().clone();
    let retained = command.config().clone();
    if !runtime.known_threads.contains(&thread_id) {
        return publish_engine_config_failure(
            events,
            thread_id,
            request_id,
            ServiceFailure::invalid(ServiceFailureStage::Request),
        );
    }
    let mutation = match engine_config_stable_mutation(command) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return publish_engine_config_failure(events, thread_id, request_id, failure);
        }
    };
    let payload = match durable_save_request(
        runtime,
        frames,
        &mutation,
        expected_engine_config_response(&thread_id, &request_id),
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) if error.code() == Some(ErrorCode::EngineConfigConflict) => {
            return publish_engine_config_conflict(events, thread_id, request_id);
        }
        Err(error) => {
            return publish_engine_config_failure(events, thread_id, request_id, error.into());
        }
    };
    finish_engine_config_save(events, thread_id, request_id, retained, payload)
}

fn expected_engine_config_response(
    thread_id: &ThreadId,
    request_id: &RequestId,
) -> ExpectedResponse {
    ExpectedResponse::ThreadEngineConfigSet {
        thread_id: thread_id.clone(),
        request_id: request_id.clone(),
    }
}

async fn durable_save_request(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    mutation: &StableMutation,
    expected: ExpectedResponse,
) -> Result<ResponsePayload, RequestFailure> {
    let first_attempt = runtime
        .request_stable(frames, mutation, expected.clone(), false)
        .await;
    match first_attempt {
        Ok(payload) => Ok(payload),
        Err(error) if error.durable_save_retry_allowed() => {
            runtime
                .request_stable(frames, mutation, expected, true)
                .await
        }
        Err(error) => Err(error),
    }
}

fn publish_engine_config_failure(
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    request_id: RequestId,
    failure: ServiceFailure,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ThreadEngineConfigFailed {
            thread_id,
            request_id,
            failure,
        },
    )
}

fn publish_engine_config_conflict(
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    request_id: RequestId,
) -> Result<(), ServiceFailure> {
    publish(
        events,
        NativeTransportEvent::ThreadEngineConfigConflict {
            thread_id,
            request_id,
        },
    )
}

fn finish_engine_config_save(
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    request_id: RequestId,
    retained: EngineRunConfig,
    payload: ResponsePayload,
) -> Result<(), ServiceFailure> {
    let ResponsePayload::ThreadEngineConfigSet(result) = payload else {
        return publish_engine_config_failure(
            events,
            thread_id,
            request_id,
            ServiceFailure::invalid(ServiceFailureStage::Request),
        );
    };
    if result.thread_id != thread_id || result.request_id != request_id {
        return publish_engine_config_failure(
            events,
            thread_id,
            request_id,
            ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            ),
        );
    }
    publish(
        events,
        NativeTransportEvent::ThreadEngineConfigSet(result, Box::new(retained)),
    )
}

fn publish(
    events: &SyncSender<NativeTransportEvent>,
    event: NativeTransportEvent,
) -> Result<(), ServiceFailure> {
    events.send(event).map_err(|_| ServiceFailure::bridge())
}

#[cfg(test)]
fn custody_trace() -> Vec<CustodyStep> {
    cleanup_plan(true, true, true)
}

#[cfg(test)]
mod tests {
    use super::{
        COMMAND_CAPACITY, ExpectedResponse, FrameFactory, IntakeRetry, NativeTransportCommand,
        PeerFailure, ReadinessValidationError, RequestAttemptError, RequestFailure, ServiceFailure,
        ServiceFailureCategory, StartupError, ThreadSelectionDecision, attach_mutation,
        build_reconnect_binding, contains_exact_project, contains_exact_thread,
        create_command_values, create_mutation, engine_config_stable_mutation, finite_duration,
        first_message_stable_mutation, known_thread_for_queue, make_request_frame,
        payload_health_decision, project_request, reconnect_hello, session_needs_reconnect,
        snapshot_request, thread_engine_settings_request, thread_selection_decision,
        threads_request, try_send_command, validate_readiness, validate_response_family,
    };
    use artisan_domain::{
        AttachProject, CONVERSATION_QUERY_MAX_TURNS, Command, ConversationCursor,
        ConversationQueryBounds, ConversationSnapshot, CreateThread, DirectoryId, DisplayName,
        EngineProfileId, ListProjectThreads, MessageBody, ProjectId, ProjectListing,
        ProjectSummary, Query, QueryTurnCount, QueueFirstMessage, ReceiptDisposition, RequestId,
        RootPath, SetThreadEngineConfig, ThreadId, ThreadListing, ThreadSummary, ThreadTitle,
        UnixMillis,
    };
    use artisan_editor_cli::payload::PayloadHealth;
    use artisan_protocol::{
        ClientRequest, DirectoryPickOutcome, ErrorCode, FirstMessageReceipt, HelloCredential,
        ProtocolVersion, RECONNECT_CAPABILITY_BYTES, ReconnectCapability,
        RegisteredEngineProfilesResult, ResponsePayload, SetThreadEngineConfigResult,
        WireEnvelopeBody, encode_envelope,
    };
    use artisan_transport::{
        ClientRequestError, DeadlineError, EnvelopeReceiveError, EnvelopeSendError, ExchangeError,
        FrameError, LoopbackTarget, OperationKind, PinnedIdentity,
    };
    use std::{num::NonZeroU32, time::Duration};

    fn project(value: &str, name: &str) -> ProjectSummary {
        ProjectSummary {
            project_id: ProjectId::parse(value).expect("valid project"),
            display_name: DisplayName::parse(name).expect("valid display name"),
            root_path: RootPath::parse(format!("/{value}")).expect("valid root"),
            attached_at: UnixMillis::EPOCH,
        }
    }

    fn thread(value: &str, project_id: &str) -> ThreadSummary {
        ThreadSummary {
            thread_id: ThreadId::parse(value).expect("valid thread"),
            project_id: ProjectId::parse(project_id).expect("valid project"),
            title: artisan_domain::ThreadTitle::parse(value).expect("valid title"),
            created_at: UnixMillis::EPOCH,
            updated_at: UnixMillis::EPOCH,
        }
    }

    #[test]
    fn payload_acceptance_is_exact() {
        assert!(payload_health_decision(&PayloadHealth::Verified).is_ok());
        assert_eq!(
            payload_health_decision(&PayloadHealth::Modified(Vec::new())),
            Err(StartupError::PayloadUnverified)
        );
        assert_eq!(
            payload_health_decision(&PayloadHealth::Unverifiable),
            Err(StartupError::PayloadUnverified)
        );
    }

    #[test]
    fn readiness_requires_endpoint_pid_and_pin_agreement() {
        let pin = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(validate_readiness("127.0.0.1:40123", 19, 19, pin, pin).is_ok());
        assert_eq!(
            validate_readiness("127.0.0.1:40123", 18, 19, pin, pin),
            Err(ReadinessValidationError::Pid)
        );
        assert_eq!(
            validate_readiness("127.0.0.1:40123", 19, 19, "not-a-pin", pin),
            Err(ReadinessValidationError::Certificate)
        );
        assert_eq!(
            validate_readiness("192.168.0.1:40123", 19, 19, pin, pin),
            Err(ReadinessValidationError::Endpoint)
        );
    }

    #[test]
    fn frames_are_unique_and_seed_correlations() {
        let mut frames = FrameFactory::new();
        let first = frames.next().expect("first frame");
        let second = frames.next().expect("second frame");
        assert_ne!(first.frame_id, second.frame_id);
        assert_ne!(first.sent_at, UnixMillis::MIN);
        assert_eq!(
            first
                .frame_id
                .to_request_id()
                .expect("correlation")
                .as_str(),
            first.frame_id.as_str()
        );
    }

    #[test]
    fn request_frame_stamps_a_validated_protocol_correlation() {
        let mut frames = FrameFactory::new();
        let (envelope, request_id) =
            make_request_frame(&mut frames, ProtocolVersion::V1, project_request())
                .expect("request frame");
        assert_eq!(
            envelope.frame_id.to_request_id().expect("request id"),
            request_id
        );
        assert!(envelope.validate_correlation().is_ok());
        assert!(matches!(envelope.body, WireEnvelopeBody::Request(_)));
    }

    #[test]
    fn picker_and_query_retries_are_fresh_but_mutation_retries_are_stable() {
        let mut frames = FrameFactory::new();
        let (first_picker, first_picker_id) = make_request_frame(
            &mut frames,
            ProtocolVersion::V1,
            ClientRequest::PickDirectory,
        )
        .expect("first picker frame");
        let (second_picker, second_picker_id) = make_request_frame(
            &mut frames,
            ProtocolVersion::V1,
            ClientRequest::PickDirectory,
        )
        .expect("second picker frame");
        assert_ne!(first_picker.frame_id, second_picker.frame_id);
        assert_ne!(first_picker_id, second_picker_id);

        let directory_id = DirectoryId::parse("directory-a").expect("directory");
        let attach = attach_mutation(&mut frames, directory_id.clone()).expect("attach mutation");
        let (attach_first, attach_first_id) = attach
            .envelope(ProtocolVersion::V1)
            .expect("attach envelope");
        let (attach_retry, attach_retry_id) = attach
            .envelope(ProtocolVersion::V1)
            .expect("attach retry envelope");
        assert_eq!(attach_first.frame_id, attach_retry.frame_id);
        assert_eq!(attach_first.sent_at, attach_retry.sent_at);
        assert_eq!(attach_first_id, attach_retry_id);
        assert_eq!(
            attach_first.frame_id.to_request_id().expect("attach id"),
            attach_first_id
        );
        assert!(matches!(
            attach_first.body,
            WireEnvelopeBody::Request(ClientRequest::Command(Command::AttachProject(
                AttachProject { request_id, directory_id: selected }
            ))) if request_id == attach_first_id && selected == directory_id
        ));

        let (query_retry, query_retry_id) =
            make_request_frame(&mut frames, ProtocolVersion::V1, project_request())
                .expect("query retry frame");
        assert_ne!(query_retry.frame_id, attach_retry.frame_id);
        assert_ne!(query_retry_id, attach_retry_id);

        let title = ThreadTitle::parse("New thread").expect("title");
        let project_id = ProjectId::parse("project-a").expect("project");
        let create = create_mutation(&mut frames, project_id.clone(), title.clone())
            .expect("create mutation");
        let (create_first, create_first_id) = create
            .envelope(ProtocolVersion::V1)
            .expect("create envelope");
        let (create_retry, create_retry_id) = create
            .envelope(ProtocolVersion::V1)
            .expect("create retry envelope");
        assert_eq!(create_first.frame_id, create_retry.frame_id);
        assert_eq!(create_first.sent_at, create_retry.sent_at);
        assert_eq!(create_first_id, create_retry_id);
        assert!(matches!(
            create_first.body,
            WireEnvelopeBody::Request(ClientRequest::Command(Command::CreateThread(
                CreateThread { request_id, project_id: selected, title: selected_title }
            ))) if request_id == create_first_id
                && selected == project_id
                && selected_title == title
        ));
    }

    #[test]
    fn stable_retry_plan_keeps_the_complete_mutation_without_a_second_identity() {
        let mut frames = FrameFactory::new();
        let project_id = ProjectId::parse("project-a").expect("project");
        let title = ThreadTitle::parse("New thread").expect("title");
        let mutation = create_mutation(&mut frames, project_id.clone(), title.clone())
            .expect("create mutation");
        let (original_frame, original_id) = mutation.envelope(ProtocolVersion::V1).expect("frame");
        let retry = IntakeRetry::Create(mutation);
        let IntakeRetry::Create(stable) = retry else {
            panic!("create retry plan changed variant");
        };
        let (retry_frame, retry_id) = stable.envelope(ProtocolVersion::V1).expect("retry frame");
        assert_eq!(original_frame.frame_id, retry_frame.frame_id);
        assert_eq!(original_frame.sent_at, retry_frame.sent_at);
        assert_eq!(original_id, retry_id);
        assert_eq!(create_command_values(&stable), Some((project_id, title)));
    }

    #[test]
    fn response_families_cover_intake_mutations_and_reject_cross_family_payloads() {
        let attached = project("project-a", "A");
        assert!(matches!(
            validate_response_family(
                ExpectedResponse::AttachedProject,
                ResponsePayload::AttachedProject {
                    project: attached.clone(),
                    disposition: ReceiptDisposition::Accepted,
                },
            ),
            Ok(ResponsePayload::AttachedProject { .. })
        ));

        let created = thread("thread-a", "project-a");
        assert!(matches!(
            validate_response_family(
                ExpectedResponse::CreatedThread,
                ResponsePayload::CreatedThread {
                    thread: created,
                    disposition: ReceiptDisposition::Duplicate,
                },
            ),
            Ok(ResponsePayload::CreatedThread { .. })
        ));

        let listing = ProjectListing::new(vec![attached]).expect("projects");
        assert!(
            validate_response_family(
                ExpectedResponse::AttachedProject,
                ResponsePayload::ProjectListing(listing),
            )
            .is_err()
        );
        let threads = ThreadListing::new(vec![thread("thread-a", "project-a")]).expect("threads");
        assert!(
            validate_response_family(
                ExpectedResponse::CreatedThread,
                ResponsePayload::ThreadListing(threads),
            )
            .is_err()
        );
    }

    #[test]
    fn first_message_response_family_requires_exact_request_and_thread() {
        let thread_id = ThreadId::parse("thread-a").expect("thread");
        let other_thread_id = ThreadId::parse("thread-b").expect("thread");
        let request_id = RequestId::parse("native-message-a").expect("request");
        let other_request_id = RequestId::parse("native-message-b").expect("request");
        let receipt = FirstMessageReceipt {
            request_id: request_id.clone(),
            message_id: artisan_domain::MessageId::parse("message-a").expect("message"),
            thread_id: thread_id.clone(),
            disposition: ReceiptDisposition::Accepted,
        };
        let expected = ExpectedResponse::FirstMessageQueued {
            thread_id: thread_id.clone(),
            request_id: request_id.clone(),
        };
        assert!(
            validate_response_family(
                expected.clone(),
                ResponsePayload::FirstMessageQueued(receipt.clone())
            )
            .is_ok()
        );
        assert!(
            validate_response_family(
                expected.clone(),
                ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                    disposition: ReceiptDisposition::Duplicate,
                    ..receipt.clone()
                })
            )
            .is_ok()
        );
        assert!(
            validate_response_family(
                expected.clone(),
                ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                    request_id: other_request_id,
                    ..receipt.clone()
                })
            )
            .is_err()
        );
        assert!(
            validate_response_family(
                expected.clone(),
                ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                    thread_id: other_thread_id,
                    ..receipt.clone()
                })
            )
            .is_err()
        );
        assert!(
            validate_response_family(
                expected,
                ResponsePayload::ProjectListing(
                    ProjectListing::new(vec![project("project-a", "A")]).expect("projects"),
                )
            )
            .is_err()
        );
    }

    #[test]
    fn first_message_stable_retry_keeps_every_wire_byte_and_identity() {
        let request_id = RequestId::parse("native-message-stable").expect("request");
        let thread_id = ThreadId::parse("thread-a").expect("thread");
        let body = MessageBody::parse("  hello\n世界  ").expect("body");
        let mutation = first_message_stable_mutation(QueueFirstMessage {
            request_id: request_id.clone(),
            thread_id: thread_id.clone(),
            body: body.clone(),
        })
        .expect("stable mutation");
        let (first, first_id) = mutation
            .envelope(ProtocolVersion::V1)
            .expect("first envelope");
        let (retry, retry_id) = mutation
            .envelope(ProtocolVersion::V1)
            .expect("retry envelope");
        assert_eq!(first_id, request_id);
        assert_eq!(retry_id, request_id);
        assert_eq!(first.frame_id, retry.frame_id);
        assert_eq!(first.sent_at, retry.sent_at);
        assert_eq!(
            encode_envelope(&first).expect("first bytes"),
            encode_envelope(&retry).expect("retry bytes")
        );
        assert!(matches!(
            first.body,
            WireEnvelopeBody::Request(ClientRequest::Command(Command::QueueFirstMessage(command)))
                if command.request_id == request_id
                    && command.thread_id == thread_id
                    && command.body == body
        ));
    }

    #[test]
    fn unknown_first_message_thread_is_rejected_before_forge_admission() {
        let known = ThreadId::parse("known-thread").expect("thread");
        let unknown = ThreadId::parse("unknown-thread").expect("thread");
        let mut known_threads = std::collections::HashSet::new();
        known_threads.insert(known.clone());
        assert!(known_thread_for_queue(&known_threads, &known).is_ok());
        assert_eq!(
            known_thread_for_queue(&known_threads, &unknown),
            Err(ServiceFailure::invalid(super::ServiceFailureStage::Request))
        );
    }

    #[test]
    fn command_debug_for_body_bearing_queue_is_variant_only() {
        let command = NativeTransportCommand::QueueFirstMessage(Box::new(QueueFirstMessage {
            request_id: RequestId::parse("native-message-redacted").expect("request"),
            thread_id: ThreadId::parse("thread-a").expect("thread"),
            body: MessageBody::parse("secret message text").expect("body"),
        }));
        let diagnostic = format!("{command:?}");
        assert_eq!(diagnostic, "NativeTransportCommand::QueueFirstMessage");
        assert!(!diagnostic.contains("secret message text"));
    }

    #[test]
    fn authoritative_refreshes_require_full_summary_equality() {
        let attached = project("project-a", "A");
        let same_identity_different_summary = project("project-a", "Renamed");
        let projects = ProjectListing::new(vec![attached.clone()]).expect("projects");
        assert!(contains_exact_project(&projects, &attached));
        assert!(!contains_exact_project(
            &projects,
            &same_identity_different_summary
        ));

        let created = thread("thread-a", "project-a");
        let same_identity_different_thread = thread("thread-a", "project-a");
        let threads = ThreadListing::new(vec![created.clone()]).expect("threads");
        assert!(contains_exact_thread(&threads, &created));
        assert!(!contains_exact_thread(
            &threads,
            &ThreadSummary {
                title: ThreadTitle::parse("Different title").expect("title"),
                ..same_identity_different_thread
            }
        ));
    }

    #[test]
    fn cancelled_picker_outcome_has_no_durable_command_input() {
        let payload = ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Cancelled);
        assert!(matches!(
            &payload,
            ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Cancelled)
        ));
        assert!(!matches!(
            &payload,
            ResponsePayload::AttachedProject { .. }
                | ResponsePayload::CreatedThread { .. }
                | ResponsePayload::FirstMessageQueued(_)
        ));
    }

    #[test]
    fn request_families_are_exact() {
        let project_id = ProjectId::parse("project-a").expect("project");
        let thread_id = ThreadId::parse("thread-a").expect("thread");
        assert!(matches!(
            project_request(),
            ClientRequest::Query(Query::ListAttachedProjects(_))
        ));
        assert!(matches!(
            threads_request(project_id),
            ClientRequest::Query(Query::ListProjectThreads(ListProjectThreads { .. }))
        ));
        assert!(matches!(
            snapshot_request(thread_id).expect("snapshot"),
            ClientRequest::Conversation(artisan_domain::ConversationRequest::Query(query))
                if matches!(query.bounds, ConversationQueryBounds::Window { .. })
        ));
        let _ = WireEnvelopeBody::Request(project_request());
    }

    #[test]
    fn response_families_are_correlated_and_identity_scoped() {
        let project_id = ProjectId::parse("project-a").expect("project");
        let other_project_id = ProjectId::parse("project-b").expect("project");
        let thread_id = ThreadId::parse("thread-a").expect("thread");
        let other_thread_id = ThreadId::parse("thread-b").expect("thread");
        assert!(matches!(
            validate_response_family(
                ExpectedResponse::Directory,
                ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Selected(
                    DirectoryId::parse("directory-a").expect("directory")
                )),
            ),
            Ok(ResponsePayload::DirectoryPicked(_))
        ));
        let projects = ProjectListing::new(vec![project("project-a", "A")]).expect("projects");
        assert!(
            validate_response_family(
                ExpectedResponse::Projects,
                ResponsePayload::ProjectListing(projects),
            )
            .is_ok()
        );
        let threads = ThreadListing::new(vec![thread("thread-a", "project-a")]).expect("threads");
        assert!(
            validate_response_family(
                ExpectedResponse::Threads(project_id.clone()),
                ResponsePayload::ThreadListing(threads.clone()),
            )
            .is_ok()
        );
        assert!(
            validate_response_family(
                ExpectedResponse::Threads(other_project_id),
                ResponsePayload::ThreadListing(threads),
            )
            .is_err()
        );
        let snapshot = ConversationSnapshot::new(
            thread_id.clone(),
            ConversationCursor::new(0),
            Vec::new(),
            Vec::new(),
            UnixMillis::EPOCH,
        )
        .expect("snapshot");
        assert!(
            validate_response_family(
                ExpectedResponse::Snapshot(thread_id),
                ResponsePayload::ConversationSnapshot(snapshot.clone()),
            )
            .is_ok()
        );
        assert!(
            validate_response_family(
                ExpectedResponse::Snapshot(other_thread_id),
                ResponsePayload::ConversationSnapshot(snapshot),
            )
            .is_err()
        );
        assert!(
            validate_response_family(
                ExpectedResponse::Projects,
                ResponsePayload::ThreadListing(threads_from_project("project-a")),
            )
            .is_err()
        );
    }

    #[test]
    fn response_correlation_requires_the_exact_outer_request_id() {
        let expected = RequestId::parse("request-a").expect("request");
        let other = RequestId::parse("request-b").expect("request");
        assert!(super::request_id_matches(&expected, &expected));
        assert!(!super::request_id_matches(&expected, &other));
        assert!(super::optional_request_id_matches(
            &expected,
            Some(&expected)
        ));
        assert!(!super::optional_request_id_matches(&expected, Some(&other)));
        assert!(!super::optional_request_id_matches(&expected, None));
    }

    #[test]
    fn empty_and_first_real_rows_are_selected_in_forge_order() {
        let empty = ProjectListing::new(Vec::new()).expect("empty projects");
        assert!(empty.projects().is_empty());
        let listing = ProjectListing::new(vec![project("p1", "First"), project("p2", "Second")])
            .expect("projects");
        assert_eq!(
            listing
                .projects()
                .first()
                .expect("first")
                .project_id
                .as_str(),
            "p1"
        );
        let empty_threads = ThreadListing::new(Vec::new()).expect("empty threads");
        assert!(empty_threads.threads().is_empty());
        let threads =
            ThreadListing::new(vec![thread("t1", "p1"), thread("t2", "p1")]).expect("threads");
        assert_eq!(
            threads.threads().first().expect("first").thread_id.as_str(),
            "t1"
        );
    }

    #[test]
    fn thread_selection_waits_for_host_snapshot_request() {
        let empty = ThreadListing::new(Vec::new()).expect("empty threads");
        assert_eq!(
            thread_selection_decision(&empty),
            ThreadSelectionDecision::Empty
        );

        let listing = ThreadListing::new(vec![thread("t1", "p1")]).expect("threads");
        assert_eq!(
            thread_selection_decision(&listing),
            ThreadSelectionDecision::AwaitHostSnapshot(ThreadId::parse("t1").expect("thread"))
        );
    }

    #[test]
    fn query_bounds_cover_zero_one_maximum_and_overflow() {
        assert!(QueryTurnCount::new(0).is_err());
        assert_eq!(QueryTurnCount::new(1).expect("one").get(), 1);
        assert_eq!(
            QueryTurnCount::new(u64::from(CONVERSATION_QUERY_MAX_TURNS))
                .expect("maximum")
                .get(),
            CONVERSATION_QUERY_MAX_TURNS
        );
        assert!(QueryTurnCount::new(u64::from(CONVERSATION_QUERY_MAX_TURNS) + 1).is_err());
    }

    #[test]
    fn bounded_command_admission_reports_full_and_closed() {
        let (sender, receiver) = tokio::sync::mpsc::channel(COMMAND_CAPACITY.min(1));
        try_send_command(&sender, NativeTransportCommand::Shutdown).expect("first admission");
        assert_eq!(
            try_send_command(&sender, NativeTransportCommand::Shutdown),
            Err(super::CommandSendError::Busy)
        );
        drop(receiver);
        assert_eq!(
            try_send_command(&sender, NativeTransportCommand::Shutdown),
            Err(super::CommandSendError::Stopped)
        );
    }

    #[test]
    fn local_session_request_errors_are_terminal() {
        let error = RequestAttemptError::Terminal {
            failure: ServiceFailure::local_session(),
            retryable_local_session_loss: true,
        };
        assert!(!error.preserves_session());
        assert_eq!(
            ServiceFailure::local_session().category,
            ServiceFailureCategory::LocalSession
        );
    }

    #[test]
    fn settings_load_generation_is_checked_and_monotonic() {
        let first = super::SettingsLoadGeneration::first();
        assert_eq!(first.get(), 1);
        assert_eq!(first.checked_next().expect("next").get(), 2);
        let exhausted = super::SettingsLoadGeneration(u64::MAX);
        assert!(exhausted.checked_next().is_none());
    }

    #[test]
    fn durable_save_retry_allows_local_loss_or_retryable_peer() {
        let local_loss = RequestFailure {
            failure: ServiceFailure::local_session(),
            peer: None,
            retryable_local_session_loss: true,
        };
        assert_eq!(
            super::durable_save_retry_classification(local_loss),
            super::DurableSaveRetryClassification::LocalSessionLoss
        );
        assert!(local_loss.durable_save_retry_allowed());

        let retryable_peer = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code: ErrorCode::Internal,
                retryable: true,
            }),
            retryable_local_session_loss: false,
        };
        assert_eq!(
            super::durable_save_retry_classification(retryable_peer),
            super::DurableSaveRetryClassification::RetryablePeer
        );
        assert!(retryable_peer.durable_save_retry_allowed());
    }

    #[test]
    fn durable_save_retry_rejects_invalid_conflicting_or_unsupported_peer() {
        let invalid_input = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code: ErrorCode::InvalidInput,
                retryable: true,
            }),
            retryable_local_session_loss: false,
        };
        assert!(!invalid_input.durable_save_retry_allowed());

        for code in [
            ErrorCode::IdempotencyConflict,
            ErrorCode::UnsupportedVersion,
            ErrorCode::UnsupportedFeature,
        ] {
            let excluded = RequestFailure {
                failure: ServiceFailure::new(
                    super::ServiceFailureStage::Request,
                    ServiceFailureCategory::Peer,
                ),
                peer: Some(PeerFailure {
                    code,
                    retryable: true,
                }),
                retryable_local_session_loss: false,
            };
            assert!(!excluded.durable_save_retry_allowed());
        }

        let conflict = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code: ErrorCode::EngineConfigConflict,
                retryable: true,
            }),
            retryable_local_session_loss: false,
        };
        assert!(!conflict.durable_save_retry_allowed());
    }

    #[test]
    fn durable_save_retry_rejects_nonretryable_and_terminal_failures() {
        let nonretryable_peer = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code: ErrorCode::Internal,
                retryable: false,
            }),
            retryable_local_session_loss: false,
        };
        assert_eq!(
            super::durable_save_retry_classification(nonretryable_peer),
            super::DurableSaveRetryClassification::NonRetryablePeer
        );
        assert!(!nonretryable_peer.durable_save_retry_allowed());

        let integrity = RequestFailure::terminal(ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        ));
        assert_eq!(
            super::durable_save_retry_classification(integrity),
            super::DurableSaveRetryClassification::Integrity
        );
        assert!(!integrity.durable_save_retry_allowed());

        let authentication = RequestFailure::terminal(ServiceFailure::new(
            super::ServiceFailureStage::Handshake,
            ServiceFailureCategory::Authentication,
        ));
        assert_eq!(
            super::durable_save_retry_classification(authentication),
            super::DurableSaveRetryClassification::Authentication
        );
        assert!(!authentication.durable_save_retry_allowed());
    }

    #[test]
    fn local_session_loss_retry_excludes_integrity_and_cancellation() {
        let timeout = ClientRequestError::Exchange(DeadlineError::Timeout {
            operation: OperationKind::Receive,
            limit: Duration::from_secs(1),
        });
        assert!(super::local_session_request_loss_is_retryable(&timeout));

        let stream_loss = ClientRequestError::Exchange(DeadlineError::Peer {
            operation: OperationKind::Receive,
            error: ExchangeError::Receive(EnvelopeReceiveError::Frame(FrameError::Truncated {
                expected: 4,
                received: 0,
            })),
        });
        assert!(super::local_session_request_loss_is_retryable(&stream_loss));

        let integrity = ClientRequestError::Exchange(DeadlineError::Peer {
            operation: OperationKind::Receive,
            error: ExchangeError::Send(EnvelopeSendError::Frame(FrameError::Empty)),
        });
        assert!(!super::local_session_request_loss_is_retryable(&integrity));

        let cancelled = ClientRequestError::Exchange(DeadlineError::Cancelled {
            operation: OperationKind::Receive,
        });
        assert!(!super::local_session_request_loss_is_retryable(&cancelled));
    }

    #[test]
    fn only_correlated_retryable_peer_classification_can_retain_a_retry_plan() {
        let retryable = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code: ErrorCode::Internal,
                retryable: true,
            }),
            retryable_local_session_loss: false,
        };
        assert!(retryable.retryable());
        assert_eq!(retryable.code(), Some(ErrorCode::Internal));

        let nonretryable = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code: ErrorCode::IdempotencyConflict,
                retryable: false,
            }),
            retryable_local_session_loss: false,
        };
        assert!(!nonretryable.retryable());
        assert_eq!(nonretryable.code(), Some(ErrorCode::IdempotencyConflict));

        let terminal = RequestFailure::terminal(ServiceFailure::local_session());
        assert!(!terminal.retryable());
        assert_eq!(terminal.code(), None);
    }

    #[test]
    fn admission_budget_one_rolls_before_the_next_request_and_stable_retry_forces_it() {
        assert!(!session_needs_reconnect(0, 1, false));
        assert!(session_needs_reconnect(1, 1, false));
        assert!(session_needs_reconnect(1, 1, true));
        assert!(session_needs_reconnect(0, 1, true));
    }

    #[test]
    fn reconnect_hello_consumes_a_capability_without_formatting_or_copying_it() {
        let capability = ReconnectCapability::from_bytes([0xA5; RECONNECT_CAPABILITY_BYTES]);
        let mut frames = FrameFactory::new();
        let hello = reconnect_hello(&mut frames, capability).expect("reconnect hello");
        let WireEnvelopeBody::Hello(message) = hello.body else {
            panic!("reconnect hello body");
        };
        assert!(matches!(message.credential, HelloCredential::Reconnect(_)));
        assert!(!message.supports_lifecycle_control);
    }

    #[test]
    fn reconnect_binding_uses_validated_target_pin_and_owned_pid() {
        let target = LoopbackTarget::new("127.0.0.1:40_123".parse().expect("socket address"))
            .expect("loopback target");
        let pinned_identity = PinnedIdentity::from_digest([0xB6; 32]);
        let binding = build_reconnect_binding([0xC7; 16], target, pinned_identity, 4_242)
            .expect("reconnect binding");
        assert_eq!(binding.instance_id, [0xC7; 16]);
        assert_eq!(binding.endpoint_port, 40_123);
        assert_eq!(binding.certificate_sha256, [0xB6; 32]);
        assert_eq!(binding.pid, NonZeroU32::new(4_242).expect("pid"));
        assert!(build_reconnect_binding([0xC7; 16], target, pinned_identity, 0).is_err());
    }

    #[test]
    fn directory_unknown_is_a_terminal_attach_classification() {
        let failure = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code: ErrorCode::DirectoryUnknown,
                retryable: true,
            }),
            retryable_local_session_loss: false,
        };
        assert!(failure.retryable());
        assert_eq!(failure.code(), Some(ErrorCode::DirectoryUnknown));
        // The attach path treats this exact code as terminal; the retained
        // failure classification itself remains redacted.
        assert!(!super::attach_retry_allowed(failure));
    }

    #[test]
    fn custody_trace_is_session_then_quarantine_then_lease_then_release() {
        assert_eq!(
            super::custody_trace(),
            vec![
                super::CustodyStep::SessionShutdown,
                super::CustodyStep::ReconnectQuarantine,
                super::CustodyStep::LeaseShutdown,
                super::CustodyStep::ReconnectRelease,
                super::CustodyStep::Stopped,
            ]
        );
    }

    fn threads_from_project(project_id: &str) -> ThreadListing {
        ThreadListing::new(vec![thread("thread-a", project_id)]).expect("threads")
    }

    #[test]
    fn listener_duration_rejects_unbounded_zero() {
        assert!(finite_duration(0).is_err());
        assert_eq!(finite_duration(1_250).expect("finite").as_millis(), 1_250);
    }

    #[test]
    fn thread_engine_settings_responses_are_thread_scoped() {
        let thread_a = ThreadId::parse("thread-a").expect("thread");
        let thread_b = ThreadId::parse("thread-b").expect("thread");
        let unconfigured = artisan_protocol::ThreadEngineSettingsResult::Unconfigured {
            thread_id: thread_a.clone(),
        };
        assert!(
            validate_response_family(
                ExpectedResponse::ThreadEngineSettings(thread_a.clone()),
                ResponsePayload::ThreadEngineSettings(unconfigured.clone())
            )
            .is_ok()
        );
        assert!(
            validate_response_family(
                ExpectedResponse::ThreadEngineSettings(thread_b),
                ResponsePayload::ThreadEngineSettings(unconfigured)
            )
            .is_err()
        );
        let snapshot = ConversationSnapshot::new(
            thread_a.clone(),
            ConversationCursor::new(0),
            Vec::new(),
            Vec::new(),
            UnixMillis::EPOCH,
        )
        .expect("snapshot");
        assert!(
            validate_response_family(
                ExpectedResponse::ThreadEngineSettings(thread_a),
                ResponsePayload::ConversationSnapshot(snapshot)
            )
            .is_err()
        );
    }

    #[test]
    fn registered_profiles_response_family_is_exact() {
        let missing = RegisteredEngineProfilesResult::RegistryMissing;
        assert!(
            validate_response_family(
                ExpectedResponse::RegisteredProfiles,
                ResponsePayload::RegisteredEngineProfiles(missing)
            )
            .is_ok()
        );
        let present_empty = RegisteredEngineProfilesResult::RegistryPresent {
            profile_ids: Vec::new(),
        };
        assert!(
            validate_response_family(
                ExpectedResponse::RegisteredProfiles,
                ResponsePayload::RegisteredEngineProfiles(present_empty)
            )
            .is_ok()
        );
        let present = RegisteredEngineProfilesResult::RegistryPresent {
            profile_ids: vec![EngineProfileId::parse("default").expect("profile")],
        };
        assert!(
            validate_response_family(
                ExpectedResponse::RegisteredProfiles,
                ResponsePayload::RegisteredEngineProfiles(present)
            )
            .is_ok()
        );
        let listing = ProjectListing::new(vec![project("project-a", "A")]).expect("projects");
        assert!(
            validate_response_family(
                ExpectedResponse::RegisteredProfiles,
                ResponsePayload::ProjectListing(listing)
            )
            .is_err()
        );
    }

    #[test]
    fn thread_engine_config_set_response_requires_exact_thread_and_request() {
        let thread_id = ThreadId::parse("thread-a").expect("thread");
        let request_id = RequestId::parse("request-a").expect("request");
        let other_thread = ThreadId::parse("thread-b").expect("thread");
        let other_request = RequestId::parse("request-b").expect("request");
        let result = SetThreadEngineConfigResult {
            request_id: request_id.clone(),
            thread_id: thread_id.clone(),
            revision: artisan_domain::EngineConfigRevision::new(1).expect("rev"),
            disposition: artisan_domain::ReceiptDisposition::Accepted,
        };
        assert!(
            validate_response_family(
                ExpectedResponse::ThreadEngineConfigSet {
                    thread_id: thread_id.clone(),
                    request_id: request_id.clone()
                },
                ResponsePayload::ThreadEngineConfigSet(result.clone())
            )
            .is_ok()
        );
        assert!(
            validate_response_family(
                ExpectedResponse::ThreadEngineConfigSet {
                    thread_id: other_thread,
                    request_id: request_id.clone()
                },
                ResponsePayload::ThreadEngineConfigSet(result.clone())
            )
            .is_err()
        );
        assert!(
            validate_response_family(
                ExpectedResponse::ThreadEngineConfigSet {
                    thread_id,
                    request_id: other_request
                },
                ResponsePayload::ThreadEngineConfigSet(result)
            )
            .is_err()
        );
    }

    #[test]
    fn engine_config_stable_retry_retains_request_id_and_payload() {
        let thread_id = ThreadId::parse("thread-a").expect("thread");
        let profile = EngineProfileId::parse("default").expect("profile");
        let model = artisan_domain::EngineModelId::parse("model-test").expect("model");
        let route = artisan_domain::EngineRouteId::parse("route-test").expect("route");
        let permission = artisan_domain::PermissionId::parse("perm-a").expect("perm");
        let agent = artisan_domain::EngineAgentId::parse("agent-a").expect("agent");
        let permission_policy = artisan_domain::EnginePermissionPolicy::new(
            permission,
            agent,
            artisan_domain::ApprovalMode::Never,
            artisan_domain::FilesystemAccess::None,
            artisan_domain::NetworkAccess::Disabled,
            artisan_domain::WebSearchAccess::Disabled,
        );
        let selection = artisan_domain::EngineSelection::OpenCode2(
            artisan_domain::OpenCode2Selection::new(profile, model, route, None, permission_policy),
        );
        let runtime = artisan_domain::EngineRuntimeControls::new(
            artisan_domain::EngineRuntimeControlsInput {
                attempt_budget: artisan_domain::FiniteMillis::new(5).expect("budget"),
                readiness_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
                health_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
                prompt_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
                stream_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
                close_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
                max_json_body_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
                max_sse_line_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
                max_sse_event_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
                max_readiness_line_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
                max_header_count: artisan_domain::CountLimit::new(1).expect("limit"),
                max_http_buffer_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
                max_stderr_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
                observation_capacity: artisan_domain::CountLimit::new(1).expect("limit"),
            },
        )
        .expect("runtime");
        let config = artisan_domain::EngineRunConfig::new(selection, runtime);
        let request_id = RequestId::parse("engine-save-1").expect("request");
        let command = Box::new(SetThreadEngineConfig::new(
            request_id.clone(),
            thread_id.clone(),
            artisan_domain::EngineConfigUpdatePrecondition::Unconfigured,
            config.clone(),
        ));
        let mutation = engine_config_stable_mutation(command).expect("mutation");
        let (first_envelope, first_request_id) =
            mutation.envelope(ProtocolVersion::V1).expect("envelope");
        let (second_envelope, second_request_id) = mutation
            .envelope(ProtocolVersion::V1)
            .expect("retry envelope");
        assert_eq!(
            first_envelope.protocol_version,
            second_envelope.protocol_version
        );
        assert!(first_envelope.body == second_envelope.body);
        assert_eq!(first_request_id, second_request_id);
        assert_eq!(first_envelope.frame_id, second_envelope.frame_id);
        assert_eq!(first_envelope.sent_at, second_envelope.sent_at);
        assert_eq!(
            first_envelope.frame_id.to_request_id().expect("id"),
            first_request_id
        );
        assert_eq!(first_request_id, request_id);
        // Fresh read uses fresh frame.
        let mut frames = FrameFactory::new();
        let (fresh_first, _) = make_request_frame(
            &mut frames,
            ProtocolVersion::V1,
            thread_engine_settings_request(thread_id.clone()),
        )
        .expect("fresh");
        let (fresh_second, _) = make_request_frame(
            &mut frames,
            ProtocolVersion::V1,
            thread_engine_settings_request(thread_id),
        )
        .expect("fresh second");
        assert_ne!(fresh_first.frame_id, fresh_second.frame_id);
    }

    #[test]
    fn engine_config_conflict_is_identifiable_and_retryable_flag_is_preserved() {
        let conflict = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code: ErrorCode::EngineConfigConflict,
                retryable: false,
            }),
            retryable_local_session_loss: false,
        };
        assert_eq!(conflict.code(), Some(ErrorCode::EngineConfigConflict));
        assert!(!conflict.retryable());
        let retryable = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code: ErrorCode::Internal,
                retryable: true,
            }),
            retryable_local_session_loss: false,
        };
        assert!(retryable.retryable());
    }

    #[test]
    fn redacted_service_failures_do_not_reveal_engine_values() {
        let failure = ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        );
        let text = failure.to_string();
        assert!(!text.contains("model-test"));
        assert!(!text.contains("route-test"));
        assert!(!text.contains("default"));
        assert!(!text.contains("engine-save"));
    }
}
