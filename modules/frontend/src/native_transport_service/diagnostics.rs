//! Redacted diagnostics, retry classification, and readiness validation for
//! the native transport service.
//!
//! The parent `native_transport_service` remains the owner of the service
//! thread, session custody, and public transport enums. Root mounts this file
//! as a child module so one reviewable home holds the finite failure
//! vocabulary that crosses the bridge.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::*;

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
    pub(super) const fn new(stage: ServiceFailureStage, category: ServiceFailureCategory) -> Self {
        Self { stage, category }
    }

    pub(super) const fn unavailable(stage: ServiceFailureStage) -> Self {
        Self::new(stage, ServiceFailureCategory::Unavailable)
    }

    pub(super) const fn invalid(stage: ServiceFailureStage) -> Self {
        Self::new(stage, ServiceFailureCategory::InvalidConfiguration)
    }

    pub(super) const fn local_session() -> Self {
        Self::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::LocalSession,
        )
    }

    pub(super) const fn bridge() -> Self {
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
    /// Valid uni engine observation event.
    Observation(ServerEvent),
    /// Bounded delivery loss.
    Lost(ServiceFailure),
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
    pub(crate) const fn failure(self) -> ServiceFailure {
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
pub(super) fn payload_health_decision(
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

pub(super) enum RequestAttemptError {
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
    pub(super) fn preserves_session(&self) -> bool {
        matches!(self, Self::Retained { .. })
    }
}

/// The only peer facts retained after correlated validation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct PeerFailure {
    pub(super) code: ErrorCode,
    pub(super) retryable: bool,
}

/// Redacted request failure used by request and intake policies.
#[derive(Clone, Copy)]
pub(super) struct RequestFailure {
    pub(super) failure: ServiceFailure,
    pub(super) peer: Option<PeerFailure>,
    pub(super) retryable_local_session_loss: bool,
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
    pub(super) fn terminal(failure: ServiceFailure) -> Self {
        Self {
            failure,
            peer: None,
            retryable_local_session_loss: false,
        }
    }

    pub(super) fn retryable(self) -> bool {
        self.peer.is_some_and(|peer| peer.retryable)
    }

    pub(super) fn code(self) -> Option<ErrorCode> {
        self.peer.map(|peer| peer.code)
    }

    pub(super) fn durable_save_retry_allowed(self) -> bool {
        durable_save_retry_classification(self).is_eligible()
    }
}

impl From<RequestFailure> for ServiceFailure {
    fn from(error: RequestFailure) -> Self {
        error.failure
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DurableSaveRetryClassification {
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

pub(super) fn durable_save_retry_classification(
    error: RequestFailure,
) -> DurableSaveRetryClassification {
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

pub(super) fn local_session_request_loss_is_retryable(error: &ClientRequestError) -> bool {
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
