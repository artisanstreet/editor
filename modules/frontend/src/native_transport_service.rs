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
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvError, SyncSender, TryRecvError, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use artisan_domain::{
    AttachProject, CONVERSATION_QUERY_MAX_TURNS, Command, ConversationQuery,
    ConversationQueryBounds, ConversationRequest, ConversationSnapshot, CreateThread, DirectoryId,
    ListAttachedProjects, ListProjectThreads, ProjectId, ProjectListing, ProjectSummary, Query,
    QueryTurnCount, RequestId, ThreadId, ThreadListing, ThreadSummary, ThreadTitle, UnixMillis,
};
use artisan_editor_cli::{
    credentials::{NativeClientCredentials, load_client_credentials},
    instance::NativeInstanceConfig,
    manifest::InstallationManifest,
    paths::Layout,
    process::{ForgeLaunchSpec, ForgeProcessLease, start_owned},
};
use artisan_protocol::{
    ClientRequest, ErrorCode, FrameId, Hello, HelloCredential, ProtocolVersion, ResponsePayload,
    VersionOffer, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, ClientSession, ClientSessionLimits, LoopbackTarget, PinnedIdentity,
    RequestOutcome,
};
use rustls_pki_types::CertificateDer;
use thiserror::Error;

/// Maximum number of commands waiting for the service thread.
pub const COMMAND_CAPACITY: usize = 64;

/// Maximum number of events waiting for the application thread.
pub const EVENT_CAPACITY: usize = 64;

/// Commands accepted by the native service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeTransportCommand {
    /// Start a fresh opaque-directory project intake.
    BeginProjectIntake,
    /// Continue the one retained retry plan for project intake.
    RetryProjectIntake,
    /// Select an existing Forge-owned project.
    SelectProject(ProjectId),
    /// Request a real snapshot for a host mounted on a known thread.
    RequestSnapshot(ThreadId),
    /// Stop accepting work and release the session and owned Forge.
    Shutdown,
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

/// One cloneable application-side handle to the native service.
#[derive(Clone)]
pub struct NativeTransportService {
    commands: SyncSender<NativeTransportCommand>,
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
        let (command_tx, command_rx) = sync_channel(COMMAND_CAPACITY);
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

fn try_send_command(
    sender: &SyncSender<NativeTransportCommand>,
    command: NativeTransportCommand,
) -> Result<(), CommandSendError> {
    match sender.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(CommandSendError::Busy),
        Err(TrySendError::Disconnected(_)) => Err(CommandSendError::Stopped),
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

enum ExpectedResponse {
    Directory,
    Projects,
    AttachedProject,
    CreatedThread,
    Threads(ProjectId),
    Snapshot(ThreadId),
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

/// Redacted request failure used by the intake policy.
#[derive(Clone, Copy)]
struct RequestFailure {
    failure: ServiceFailure,
    peer: Option<PeerFailure>,
}

impl RequestFailure {
    fn terminal(failure: ServiceFailure) -> Self {
        Self {
            failure,
            peer: None,
        }
    }

    fn retryable(self) -> bool {
        self.peer.is_some_and(|peer| peer.retryable)
    }

    fn code(self) -> Option<ErrorCode> {
        self.peer.map(|peer| peer.code)
    }
}

impl From<RequestFailure> for ServiceFailure {
    fn from(error: RequestFailure) -> Self {
        error.failure
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
        .map_err(|failure| RequestAttemptError::Terminal { failure })?;
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
            .map_err(|_| RequestAttemptError::Terminal {
                failure: ServiceFailure::local_session(),
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
        _ => Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        )),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CustodyStep {
    SessionShutdown,
    LeaseShutdown,
    ReconnectDrop,
    Stopped,
}

fn cleanup_plan(
    has_session: bool,
    has_lease: bool,
    has_reconnect_capability: bool,
) -> Vec<CustodyStep> {
    let mut plan = Vec::with_capacity(4);
    if has_session {
        plan.push(CustodyStep::SessionShutdown);
    }
    if has_lease {
        plan.push(CustodyStep::LeaseShutdown);
    }
    if has_reconnect_capability {
        plan.push(CustodyStep::ReconnectDrop);
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
}

struct ServiceRuntime {
    session: Option<ClientSession>,
    reconnect_capability: Option<artisan_protocol::ReconnectCapability>,
    certificate: CertificateDer<'static>,
    target: LoopbackTarget,
    pinned_identity: PinnedIdentity,
    limits: ClientSessionLimits,
    lease: Option<ForgeProcessLease>,
    cancel: CancelHandle,
    shutdown_grace: Duration,
    known_threads: HashSet<ThreadId>,
    intake: IntakeState,
}

impl ServiceRuntime {
    async fn reconnect(&mut self, frames: &mut FrameFactory) -> Result<(), ServiceFailure> {
        // A session is never reused after its lifetime budget or a stable
        // mutation settlement. Dropping it before moving the one current
        // capability makes custody explicit and prevents two live owners.
        drop(self.session.take());
        let Some(capability) = self.reconnect_capability.take() else {
            return Err(ServiceFailure::local_session());
        };
        let hello = match reconnect_hello(frames, capability) {
            Ok(hello) => hello,
            Err(failure) => return Err(failure),
        };
        if let Ok((session, welcome)) = ClientSession::connect(
            self.target,
            self.certificate.clone(),
            self.pinned_identity,
            hello,
            self.limits,
            &self.cancel,
        )
        .await
        {
            self.session = Some(session);
            self.reconnect_capability = Some(welcome.welcome.reconnect_capability);
            Ok(())
        } else {
            // The old session and the only current capability are gone;
            // an uncertain handshake cannot safely be retried here.
            self.session = None;
            self.reconnect_capability = None;
            Err(ServiceFailure::new(
                ServiceFailureStage::Handshake,
                ServiceFailureCategory::Authentication,
            ))
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
            self.reconnect(frames).await?;
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
                Err(RequestFailure { failure, peer })
            }
            Err(RequestAttemptError::Terminal { failure }) => {
                self.session = None;
                Err(RequestFailure::terminal(failure))
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
        let mut failed = false;
        for step in cleanup_plan(
            self.session.is_some(),
            self.lease.is_some(),
            self.reconnect_capability.is_some(),
        ) {
            match step {
                CustodyStep::SessionShutdown => {
                    if let Some(session) = self.session.take()
                        && session.shutdown(&self.cancel).await.is_err()
                    {
                        failed = true;
                    }
                }
                CustodyStep::LeaseShutdown => {
                    if let Some(lease) = self.lease.take()
                        && lease.shutdown(self.shutdown_grace).await.is_err()
                    {
                        failed = true;
                    }
                }
                CustodyStep::ReconnectDrop => {
                    drop(self.reconnect_capability.take());
                }
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

fn reconnect_hello(
    frames: &mut FrameFactory,
    capability: artisan_protocol::ReconnectCapability,
) -> Result<WireEnvelope, ServiceFailure> {
    let stamp = frames
        .next()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Handshake))?;
    let supported_versions = VersionOffer::new(vec![1])
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Handshake))?;
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: stamp.frame_id,
        sent_at: stamp.sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions,
            credential: HelloCredential::Reconnect(capability),
            supports_lifecycle_control: false,
        }),
    })
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
    let runtime = attach_to_owned_forge(lease, &config, credentials, &mut frames).await?;
    Ok((runtime, frames))
}

async fn attach_to_owned_forge(
    lease: ForgeProcessLease,
    config: &NativeInstanceConfig,
    credentials: NativeClientCredentials,
    frames: &mut FrameFactory,
) -> Result<ServiceRuntime, StartupError> {
    let result = establish_session(config, &lease, credentials, frames).await;
    match result {
        Ok((session, reconnect_capability, cancel, shutdown_grace, material)) => {
            Ok(ServiceRuntime {
                session: Some(session),
                reconnect_capability: Some(reconnect_capability),
                certificate: material.certificate,
                target: material.target,
                pinned_identity: material.pinned_identity,
                limits: material.limits,
                lease: Some(lease),
                cancel,
                shutdown_grace,
                known_threads: HashSet::new(),
                intake: IntakeState::new(),
            })
        }
        Err(error) => {
            let shutdown_grace =
                finite_duration(config.listener().drain_timeout_ms()).unwrap_or(Duration::ZERO);
            let _ = lease.shutdown(shutdown_grace).await;
            Err(error)
        }
    }
}

async fn establish_session(
    config: &NativeInstanceConfig,
    lease: &ForgeProcessLease,
    credentials: NativeClientCredentials,
    frames: &mut FrameFactory,
) -> Result<
    (
        ClientSession,
        artisan_protocol::ReconnectCapability,
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
    Ok((
        session,
        welcome.welcome.reconnect_capability,
        cancel,
        limits.shutdown,
        SessionMaterial {
            certificate: trusted_certificate,
            target,
            pinned_identity,
            limits,
        },
    ))
}

async fn service_main(
    commands: Receiver<NativeTransportCommand>,
    events: SyncSender<NativeTransportEvent>,
) {
    let mut status = ServiceStopStatus::Clean;
    let started = start_native_service().await;
    match started {
        Ok((mut runtime, mut frames)) => {
            let run_result = load_initial_catalog(&mut runtime, &mut frames, &events).await;
            if let Err(failure) = run_result {
                status = ServiceStopStatus::Failed;
                let _ = publish(&events, NativeTransportEvent::Failed(failure));
            } else {
                let command_result =
                    command_loop(commands, &mut runtime, &mut frames, &events).await;
                if let Err(failure) = command_result {
                    status = ServiceStopStatus::Failed;
                    let _ = publish(&events, NativeTransportEvent::Failed(failure));
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

async fn command_loop(
    commands: Receiver<NativeTransportCommand>,
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
) -> Result<(), ServiceFailure> {
    loop {
        match commands.recv() {
            Ok(NativeTransportCommand::Shutdown) | Err(RecvError) => return Ok(()),
            Ok(NativeTransportCommand::BeginProjectIntake) => {
                begin_project_intake(runtime, frames, events).await?;
            }
            Ok(NativeTransportCommand::RetryProjectIntake) => {
                retry_project_intake(runtime, frames, events).await?;
            }
            Ok(NativeTransportCommand::SelectProject(project_id)) => {
                select_project(runtime, frames, events, project_id).await?;
            }
            Ok(NativeTransportCommand::RequestSnapshot(thread_id)) => {
                request_snapshot(runtime, frames, events, thread_id).await?;
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
        contains_exact_project, contains_exact_thread, create_command_values, create_mutation,
        finite_duration, make_request_frame, payload_health_decision, project_request,
        reconnect_hello, session_needs_reconnect, snapshot_request, thread_selection_decision,
        threads_request, try_send_command, validate_readiness, validate_response_family,
    };
    use artisan_domain::{
        AttachProject, CONVERSATION_QUERY_MAX_TURNS, Command, ConversationCursor,
        ConversationQueryBounds, ConversationSnapshot, CreateThread, DirectoryId, DisplayName,
        ListProjectThreads, ProjectId, ProjectListing, ProjectSummary, Query, QueryTurnCount,
        ReceiptDisposition, RequestId, RootPath, ThreadId, ThreadListing, ThreadSummary,
        ThreadTitle, UnixMillis,
    };
    use artisan_editor_cli::payload::PayloadHealth;
    use artisan_protocol::{
        ClientRequest, DirectoryPickOutcome, ErrorCode, HelloCredential, ProtocolVersion,
        RECONNECT_CAPABILITY_BYTES, ReconnectCapability, ResponsePayload, WireEnvelopeBody,
    };
    use std::sync::mpsc::sync_channel;

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
        let (sender, receiver) = sync_channel(COMMAND_CAPACITY.min(1));
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
        };
        assert!(!error.preserves_session());
        assert_eq!(
            ServiceFailure::local_session().category,
            ServiceFailureCategory::LocalSession
        );
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
        assert!(matches!(
            hello.body,
            WireEnvelopeBody::Hello(artisan_protocol::Hello {
                credential: HelloCredential::Reconnect(_),
                ..
            })
        ));
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
        };
        assert!(failure.retryable());
        assert_eq!(failure.code(), Some(ErrorCode::DirectoryUnknown));
        // The attach path treats this exact code as terminal; the retained
        // failure classification itself remains redacted.
        assert!(!super::attach_retry_allowed(failure));
    }

    #[test]
    fn custody_trace_is_session_then_lease_then_capability() {
        assert_eq!(
            super::custody_trace(),
            vec![
                super::CustodyStep::SessionShutdown,
                super::CustodyStep::LeaseShutdown,
                super::CustodyStep::ReconnectDrop,
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
}
