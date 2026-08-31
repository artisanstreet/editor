//! Owned application-protocol values.
//!
//! Generated Cap'n Proto readers borrow message storage and cannot safely
//! cross service boundaries. These values own their data, preserve the domain
//! vocabulary, and validate protocol-only metadata before the transport sees
//! it.

use std::fmt;

use artisan_domain::{
    Command, ConversationCursor, ConversationRequest, ConversationSnapshot,
    ConversationSubscriptionStart, DirectoryId, DirectoryListing, EngineConfigRevision,
    EngineRunConfig, Event, IdentifierError, MessageId, PatchBatch, ProjectListing, ProjectSummary,
    Query, ReceiptDisposition, RequestId, ThreadId, ThreadListing, ThreadSummary, UnixMillis,
};
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::Zeroize;

/// Current application protocol revision.
pub const APPLICATION_PROTOCOL_VERSION: u32 = 1;
/// Maximum number of application revisions offered during hello.
pub const HELLO_VERSION_MAX_ENTRIES: usize = 8;
/// Required byte length of the one-time local capability.
pub const LOCAL_CAPABILITY_BYTES: usize = 32;
/// Required byte length of a rotated reconnect capability.
pub const RECONNECT_CAPABILITY_BYTES: usize = 32;
/// Maximum UTF-8 byte length of a protocol error detail.
pub const ERROR_DETAIL_MAX_BYTES: usize = 1_024;

/// Validation failure for protocol-owned metadata.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProtocolValueError {
    /// This implementation does not speak the supplied revision.
    #[error("unsupported application protocol version {version}")]
    UnsupportedVersion {
        /// Unsupported integer revision.
        version: u32,
    },
    /// Server event zero is reserved and cannot identify a delivered event.
    #[error("server event cursor must be greater than zero")]
    ZeroEventCursor,
    /// A frame identity failed the shared identifier rule.
    #[error("invalid frame id: {source}")]
    FrameId {
        /// Underlying identifier failure.
        #[source]
        source: IdentifierError,
    },
    /// A connection identity failed the shared identifier rule.
    #[error("invalid connection id: {source}")]
    ConnectionId {
        /// Underlying identifier failure.
        #[source]
        source: IdentifierError,
    },
    /// Error detail exceeded its protocol-owned byte ceiling.
    #[error("protocol error detail is {length} UTF-8 bytes; the maximum is {maximum}")]
    ErrorDetailTooLong {
        /// Offending UTF-8 byte length.
        length: usize,
        /// Documented ceiling.
        maximum: usize,
    },
    /// A mutation command carried a different request id than its frame.
    #[error("request frame id and command request id must match")]
    RequestCorrelationMismatch,
    /// A nested receipt carried a different request id than its response.
    #[error("response request id and nested receipt request id must match")]
    ResponseCorrelationMismatch,
    /// A lifecycle status carried an impossible state and active-work count.
    #[error("lifecycle status {state:?} cannot report active work count {active_work_count}")]
    InvalidLifecycleStatus {
        /// Reported lifecycle state.
        state: LifecycleState,
        /// Reported active-work count.
        active_work_count: u32,
    },
}

/// Negotiated application protocol version.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProtocolVersion(u32);

impl ProtocolVersion {
    /// The sole revision supported by this packet.
    pub const V1: Self = Self(APPLICATION_PROTOCOL_VERSION);

    /// Validates an integer revision.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::UnsupportedVersion`] unless `value` is 1.
    pub const fn new(value: u32) -> Result<Self, ProtocolValueError> {
        if value == APPLICATION_PROTOCOL_VERSION {
            Ok(Self(value))
        } else {
            Err(ProtocolValueError::UnsupportedVersion { version: value })
        }
    }

    /// Returns the wire integer.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One-based sequence assigned to a Forge-originated server event.
///
/// Unlike a conversation replay cursor, zero is not a meaningful starting
/// sentinel: every delivered event has an explicit positive sequence.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EventCursor(u64);

impl EventCursor {
    /// Creates a one-based server event cursor.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::ZeroEventCursor`] for zero.
    pub const fn new(value: u64) -> Result<Self, ProtocolValueError> {
        if value == 0 {
            Err(ProtocolValueError::ZeroEventCursor)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the wire integer.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Protocol-owned identity of one sender-minted frame.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FrameId(String);

impl FrameId {
    /// Validates a frame identity using the shared domain identifier rule.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::FrameId`] for invalid external text.
    pub fn parse(value: impl Into<String>) -> Result<Self, ProtocolValueError> {
        let value = value.into();
        RequestId::parse(value.clone()).map_err(|source| ProtocolValueError::FrameId { source })?;
        Ok(Self(value))
    }

    /// Returns the validated identity text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Converts a client request frame identity to its domain correlation id.
    ///
    /// # Errors
    ///
    /// This can fail only if invariants were violated internally; callers still
    /// receive the typed identifier failure rather than a panic.
    pub fn to_request_id(&self) -> Result<RequestId, IdentifierError> {
        RequestId::parse(self.0.clone())
    }
}

impl fmt::Display for FrameId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Protocol-owned connection diagnostic identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ConnectionId(String);

impl ConnectionId {
    /// Validates a connection identity using the shared identifier rule.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::ConnectionId`] for invalid external text.
    pub fn parse(value: impl Into<String>) -> Result<Self, ProtocolValueError> {
        let value = value.into();
        RequestId::parse(value.clone())
            .map_err(|source| ProtocolValueError::ConnectionId { source })?;
        Ok(Self(value))
    }

    /// Returns the validated identity text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validation failure for one-time local capability material.
///
/// Errors report only lengths and never include secret bytes.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum LocalCapabilityError {
    /// Capability data had the wrong length.
    #[error("local capability is {length} bytes; exactly {expected} bytes are required")]
    InvalidLength {
        /// Received length.
        length: usize,
        /// Required length.
        expected: usize,
    },
}

/// High-entropy one-time local client capability.
///
/// Deliberately implements neither [`fmt::Debug`] nor [`fmt::Display`], so
/// ordinary tracing and error formatting cannot expose its bytes.
pub struct LocalCapability([u8; LOCAL_CAPABILITY_BYTES]);

impl LocalCapability {
    /// Owns an already length-safe capability.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; LOCAL_CAPABILITY_BYTES]) -> Self {
        Self(bytes)
    }

    /// Copies capability bytes after checking the exact length.
    ///
    /// # Errors
    ///
    /// Returns [`LocalCapabilityError::InvalidLength`] without including any
    /// byte content in the error.
    pub fn try_from_slice(bytes: &[u8]) -> Result<Self, LocalCapabilityError> {
        let length = bytes.len();
        let value = <[u8; LOCAL_CAPABILITY_BYTES]>::try_from(bytes).map_err(|_| {
            LocalCapabilityError::InvalidLength {
                length,
                expected: LOCAL_CAPABILITY_BYTES,
            }
        })?;
        Ok(Self(value))
    }

    /// Compares capability bytes without data-dependent early exit.
    ///
    /// Both operands have the same fixed length by construction, so the
    /// comparison time does not disclose a matching prefix.
    #[must_use]
    pub fn constant_time_eq(&self, candidate: &Self) -> bool {
        bool::from(self.0.ct_eq(&candidate.0))
    }

    /// Borrows the secret solely for serialization or constant-time
    /// authentication at a restricted boundary. Callers must never format it.
    #[must_use]
    pub(crate) const fn expose_for_wire(&self) -> &[u8; LOCAL_CAPABILITY_BYTES] {
        &self.0
    }
}

impl PartialEq for LocalCapability {
    fn eq(&self, other: &Self) -> bool {
        self.constant_time_eq(other)
    }
}

impl Eq for LocalCapability {}

impl Drop for LocalCapability {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Validation failure for rotated reconnect capability material.
///
/// Errors report only lengths and never include secret bytes.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ReconnectCapabilityError {
    /// Capability data had the wrong length.
    #[error("reconnect capability is {length} bytes; exactly {expected} bytes are required")]
    InvalidLength {
        /// Received length.
        length: usize,
        /// Required length.
        expected: usize,
    },
}

/// High-entropy rotated single-use reconnect capability.
///
/// Deliberately distinct from [`LocalCapability`] so the two credential
/// vocabularies can never be confused. Deliberately implements neither
/// [`fmt::Debug`] nor [`fmt::Display`] nor [`Clone`], so ordinary tracing,
/// error formatting, and accidental duplication cannot expose or copy its
/// bytes. Single-use/session enforcement is Phase 3 work and intentionally
/// absent here.
pub struct ReconnectCapability([u8; RECONNECT_CAPABILITY_BYTES]);

impl ReconnectCapability {
    /// Owns an already length-safe reconnect capability.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; RECONNECT_CAPABILITY_BYTES]) -> Self {
        Self(bytes)
    }

    /// Copies reconnect capability bytes after checking the exact length.
    ///
    /// # Errors
    ///
    /// Returns [`ReconnectCapabilityError::InvalidLength`] without including
    /// any byte content in the error.
    pub fn try_from_slice(bytes: &[u8]) -> Result<Self, ReconnectCapabilityError> {
        let length = bytes.len();
        let value = <[u8; RECONNECT_CAPABILITY_BYTES]>::try_from(bytes).map_err(|_| {
            ReconnectCapabilityError::InvalidLength {
                length,
                expected: RECONNECT_CAPABILITY_BYTES,
            }
        })?;
        Ok(Self(value))
    }

    /// Compares capability bytes without data-dependent early exit.
    ///
    /// Both operands have the same fixed length by construction, so the
    /// comparison time does not disclose a matching prefix.
    #[must_use]
    pub fn constant_time_eq(&self, candidate: &Self) -> bool {
        bool::from(self.0.ct_eq(&candidate.0))
    }

    /// Borrows the secret solely for serialization or constant-time
    /// authentication at a restricted boundary. Callers must never format it.
    #[must_use]
    pub(crate) const fn expose_for_wire(&self) -> &[u8; RECONNECT_CAPABILITY_BYTES] {
        &self.0
    }
}

impl PartialEq for ReconnectCapability {
    fn eq(&self, other: &Self) -> bool {
        self.constant_time_eq(other)
    }
}

impl Eq for ReconnectCapability {}

impl Drop for ReconnectCapability {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Validation failure for a hello version offer.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum VersionOfferError {
    /// At least one revision must be offered.
    #[error("hello version offer must not be empty")]
    Empty,
    /// The offer exceeded its protocol-owned collection ceiling.
    #[error("hello version offer holds {count} entries; the maximum is {maximum}")]
    TooMany {
        /// Offending list size.
        count: usize,
        /// Documented ceiling.
        maximum: usize,
    },
    /// Revisions must be listed in ascending order.
    #[error("hello versions must be strictly ascending; {actual} follows {previous}")]
    OutOfOrder {
        /// Previous offered revision.
        previous: u32,
        /// Non-ascending revision.
        actual: u32,
    },
    /// One revision appeared more than once.
    #[error("hello version offer contains duplicate version {version}")]
    Duplicate {
        /// Repeated revision.
        version: u32,
    },
    /// An offered revision is unsupported by this implementation.
    #[error("unsupported application protocol version {version}")]
    Unsupported {
        /// Unsupported revision.
        version: u32,
    },
}

/// Bounded, strictly ascending, unique hello revision offer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionOffer(Vec<ProtocolVersion>);

impl VersionOffer {
    /// Validates an external integer revision list.
    ///
    /// # Errors
    ///
    /// Returns [`VersionOfferError`] for empty, oversized, duplicate,
    /// non-ascending, or unsupported offers.
    pub fn new(versions: Vec<u32>) -> Result<Self, VersionOfferError> {
        if versions.is_empty() {
            return Err(VersionOfferError::Empty);
        }
        if versions.len() > HELLO_VERSION_MAX_ENTRIES {
            return Err(VersionOfferError::TooMany {
                count: versions.len(),
                maximum: HELLO_VERSION_MAX_ENTRIES,
            });
        }

        for pair in versions.windows(2) {
            let previous = pair[0];
            let actual = pair[1];
            if actual == previous {
                return Err(VersionOfferError::Duplicate { version: actual });
            }
            if actual < previous {
                return Err(VersionOfferError::OutOfOrder { previous, actual });
            }
        }

        versions
            .into_iter()
            .map(|version| {
                ProtocolVersion::new(version)
                    .map_err(|_| VersionOfferError::Unsupported { version })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    /// Returns the validated offered revisions.
    #[must_use]
    pub fn versions(&self) -> &[ProtocolVersion] {
        &self.0
    }
}

/// Bounded human-readable protocol error detail.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ErrorDetail(String);

impl ErrorDetail {
    /// Validates an error detail. Empty text is allowed.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::ErrorDetailTooLong`] above 1,024 UTF-8
    /// bytes.
    pub fn parse(value: impl Into<String>) -> Result<Self, ProtocolValueError> {
        let value = value.into();
        let length = value.len();
        if length > ERROR_DETAIL_MAX_BYTES {
            return Err(ProtocolValueError::ErrorDetailTooLong {
                length,
                maximum: ERROR_DETAIL_MAX_BYTES,
            });
        }
        Ok(Self(value))
    }

    /// Returns the validated detail.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Owned credential presented inside a hello.
///
/// Deliberately implements neither [`fmt::Debug`] nor [`fmt::Display`] and is
/// not [`Clone`], so its secret bytes can never be formatted, logged, or
/// duplicated past the handshake boundary.
#[derive(Eq, PartialEq)]
pub enum HelloCredential {
    /// First-contact one-time launcher capability.
    Initial(LocalCapability),
    /// Rotated reconnect credential from the previous successful welcome.
    Reconnect(ReconnectCapability),
}

/// Authenticated client hello.
#[derive(Eq, PartialEq)]
pub struct Hello {
    /// Bounded supported revision offer.
    pub supported_versions: VersionOffer,
    /// Owned single-use credential proving this session's right to connect.
    pub credential: HelloCredential,
    /// Whether this client offers native lifecycle control support.
    pub supports_lifecycle_control: bool,
}

/// Successful application protocol negotiation.
#[derive(Eq, PartialEq)]
pub struct Welcome {
    /// Selected revision, which must have appeared in the hello offer.
    pub negotiated_version: ProtocolVersion,
    /// Opaque connection-scoped diagnostic identity.
    pub connection_id: ConnectionId,
    /// Rotated single-use reconnect credential for resuming a later session.
    pub reconnect_capability: ReconnectCapability,
    /// Whether this connection negotiated native lifecycle control support.
    pub lifecycle_control_supported: bool,
}

/// Native Forge lifecycle state reported by status and stop receipts.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleState {
    /// No lifecycle work is currently active.
    Ready,
    /// One or more lifecycle operations are active.
    Busy,
    /// Shutdown is draining in-flight lifecycle work.
    Draining,
}

/// Native lifecycle status with a state/count consistency invariant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleStatus {
    /// Coarse lifecycle state.
    pub state: LifecycleState,
    /// Number of active units of lifecycle work.
    pub active_work_count: u32,
}

impl LifecycleStatus {
    /// Creates a lifecycle status after checking its state/count invariant.
    ///
    /// `Ready` requires a zero count, `Busy` requires a positive count, and
    /// `Draining` permits any in-flight count while cancellation completes.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::InvalidLifecycleStatus`] for an
    /// inconsistent state/count pair.
    pub const fn new(
        state: LifecycleState,
        active_work_count: u32,
    ) -> Result<Self, ProtocolValueError> {
        let status = Self {
            state,
            active_work_count,
        };
        match status.validate() {
            Ok(()) => Ok(status),
            Err(error) => Err(error),
        }
    }

    /// Validates a status assembled at the public field boundary.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::InvalidLifecycleStatus`] for an
    /// inconsistent state/count pair.
    pub const fn validate(&self) -> Result<(), ProtocolValueError> {
        match self.state {
            LifecycleState::Ready => {
                if self.active_work_count == 0 {
                    Ok(())
                } else {
                    Err(ProtocolValueError::InvalidLifecycleStatus {
                        state: self.state,
                        active_work_count: self.active_work_count,
                    })
                }
            }
            LifecycleState::Busy => {
                if self.active_work_count == 0 {
                    Err(ProtocolValueError::InvalidLifecycleStatus {
                        state: self.state,
                        active_work_count: self.active_work_count,
                    })
                } else {
                    Ok(())
                }
            }
            LifecycleState::Draining => Ok(()),
        }
    }
}

/// Result classification for a native lifecycle stop request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleStopDisposition {
    /// The stop transition was newly accepted.
    Accepted,
    /// The same stop request was already accepted.
    Duplicate,
    /// A stop transition is already in progress.
    AlreadyStopping,
}

/// Native lifecycle stop receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleStopReceipt {
    /// Whether this stop request was accepted, replayed, or already stopping.
    pub disposition: LifecycleStopDisposition,
    /// Lifecycle state observed with the disposition.
    pub state: LifecycleState,
}

/// Native lifecycle control request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleRequest {
    /// Read the current lifecycle status.
    Status,
    /// Ask Forge to stop, optionally requiring an idle lifecycle first.
    Stop {
        /// Whether the stop may be accepted only after the lifecycle is idle.
        require_idle: bool,
    },
}

/// Native lifecycle control response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleResponse {
    /// Current lifecycle status.
    Status(LifecycleStatus),
    /// Result of a stop request.
    Stop(LifecycleStopReceipt),
}

/// First-workflow request payload after frame correlation is separated out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientRequest {
    /// Pure domain read.
    Query(Query),
    /// Idempotent domain mutation.
    Command(Command),
    /// Bounded conversation read or subscription control.
    Conversation(ConversationRequest),
    /// Explicit host interaction: ask the local Forge process to show its
    /// native directory picker once. Deliberately outside the pure domain
    /// [`Query`] and durable [`Command`] vocabularies: nothing durable is
    /// created or mutated, the request must not be automatically replayed,
    /// and every deliberate new attempt uses a fresh frame identity -- a
    /// fresh [`FrameId`] as its wire `Envelope.messageId`, unlike the stable
    /// verbatim-retry identity of durable commands. This schema slice
    /// implements neither duplicate-request suppression nor cancellation
    /// propagation.
    PickDirectory,
    /// Negotiated native lifecycle status or stop control.
    Lifecycle(LifecycleRequest),
}

/// Successful durable thread engine-configuration mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetThreadEngineConfigResult {
    /// Stable client request identity echoed by the nested result.
    pub request_id: RequestId,
    /// Thread whose configuration was changed.
    pub thread_id: ThreadId,
    /// Resulting one-based configuration revision.
    pub revision: EngineConfigRevision,
    /// Newly accepted or exact duplicate replay.
    pub disposition: ReceiptDisposition,
}

/// Receipt returned when a first message is durably queued.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirstMessageReceipt {
    /// Stable client correlation identity.
    pub request_id: RequestId,
    /// Forge-minted durable message identity.
    pub message_id: MessageId,
    /// Owning thread.
    pub thread_id: ThreadId,
    /// Accepted or exact duplicate replay.
    pub disposition: artisan_domain::ReceiptDisposition,
}

/// Successful start of authoritative conversation delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationSubscriptionStarted {
    /// Fresh delivery begins with a complete validated snapshot.
    Fresh(ConversationSubscriptionStart),
    /// Resumed delivery continues strictly after an already-applied cursor.
    Resumed {
        /// Thread whose delivery is resuming.
        thread_id: ThreadId,
        /// Last patch already applied by the subscriber.
        cursor: ConversationCursor,
    },
}

/// Successful stop of authoritative conversation delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSubscriptionStopped {
    /// Thread no longer being delivered.
    pub thread_id: ThreadId,
}

/// Validated outcome of one explicit directory-picker interaction.
///
/// [`DirectoryPickOutcome::Selected`] carries only the opaque, validated
/// [`DirectoryId`] of the chosen directory: never a filesystem path, label,
/// enumeration, or child flag. [`DirectoryPickOutcome::Cancelled`] reports
/// an actual user dismissal of the picker rather than a request cancellation
/// or a dropped frame; cancellation propagation and late-response handling
/// stay explicitly outside this slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryPickOutcome {
    /// The user chose a directory.
    Selected(DirectoryId),
    /// The user dismissed the picker.
    Cancelled,
}

/// Authoritative persisted thread engine settings read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThreadEngineSettingsResult {
    /// No engine configuration has been stored for this thread.
    Unconfigured { thread_id: ThreadId },
    /// Complete persisted configuration with its one-based revision.
    Configured {
        thread_id: ThreadId,
        revision: EngineConfigRevision,
        config: EngineRunConfig,
    },
}

impl ThreadEngineSettingsResult {
    /// Returns the thread owning these settings.
    #[must_use]
    pub fn thread_id(&self) -> &ThreadId {
        match self {
            Self::Unconfigured { thread_id } | Self::Configured { thread_id, .. } => thread_id,
        }
    }

    /// Returns the stored revision when configured.
    #[must_use]
    pub fn revision(&self) -> Option<EngineConfigRevision> {
        match self {
            Self::Unconfigured { .. } => None,
            Self::Configured { revision, .. } => Some(*revision),
        }
    }

    /// Returns the stored configuration when configured.
    #[must_use]
    pub fn config(&self) -> Option<&EngineRunConfig> {
        match self {
            Self::Unconfigured { .. } => None,
            Self::Configured { config, .. } => Some(config),
        }
    }
}

/// Successful first-workflow response payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponsePayload {
    /// Forge-visible directory listing.
    DirectoryListing(DirectoryListing),
    /// Complete bounded catalog of attached projects.
    ProjectListing(ProjectListing),
    /// Idempotent project attachment result.
    AttachedProject {
        /// Original durable project summary.
        project: ProjectSummary,
        /// Accepted now or replayed.
        disposition: artisan_domain::ReceiptDisposition,
    },
    /// Project-scoped thread listing.
    ThreadListing(ThreadListing),
    /// Idempotent thread creation result.
    CreatedThread {
        /// Original durable thread summary.
        thread: ThreadSummary,
        /// Accepted now or replayed.
        disposition: artisan_domain::ReceiptDisposition,
    },
    /// Durable first-message receipt.
    FirstMessageQueued(FirstMessageReceipt),
    /// Complete bounded conversation projection.
    ConversationSnapshot(ConversationSnapshot),
    /// Fresh snapshot-first or resumed subscription acknowledgement.
    ConversationSubscriptionStarted(ConversationSubscriptionStarted),
    /// Clean conversation subscription stop acknowledgement.
    ConversationSubscriptionStopped(ConversationSubscriptionStopped),
    /// Outcome of one explicit native directory-picker interaction.
    DirectoryPicked(DirectoryPickOutcome),
    /// Negotiated native lifecycle status or stop result.
    Lifecycle(LifecycleResponse),
    /// Durable thread engine-configuration result.
    ThreadEngineConfigSet(SetThreadEngineConfigResult),
    /// Authoritative persisted thread engine settings.
    ThreadEngineSettings(ThreadEngineSettingsResult),
}

/// Successful response correlated to a client request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerResponse {
    /// Triggering client request identity.
    pub request_id: RequestId,
    /// Successful result.
    pub payload: ResponsePayload,
}

/// Durable Forge-originated event with its connection replay sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerEvent {
    /// One-based sequence used to detect duplicate, missing, or regressed events.
    pub cursor: EventCursor,
    /// Domain event delivered at this sequence.
    pub event: Event,
}

/// Stable protocol rejection classification.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ErrorCode {
    /// No mutually supported protocol revision exists.
    UnsupportedVersion,
    /// One field violated its documented validation rule.
    InvalidInput,
    /// Opaque directory identity is unknown or stale.
    DirectoryUnknown,
    /// Attached project does not exist.
    ProjectUnknown,
    /// Thread does not exist.
    ThreadUnknown,
    /// Forge failed internally; retry may later succeed.
    Internal,
    /// The same stable request identity was previously accepted for a
    /// different command kind or immutable payload. The originally accepted
    /// outcome stands, and repeating the conflicting request is never
    /// retryable.
    IdempotencyConflict,
    /// The peer requested lifecycle control without a negotiated capability.
    UnsupportedFeature,
    /// Lifecycle control cannot be accepted while lifecycle work is busy.
    LifecycleBusy,
    /// Thread engine configuration revision was stale.
    EngineConfigConflict,
}

/// Typed application-protocol rejection or failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolFailure {
    /// Stable classification.
    pub code: ErrorCode,
    /// Bounded human-readable evidence.
    pub detail: ErrorDetail,
    /// Whether repeating the identical request later may succeed.
    pub retryable: bool,
    /// Triggering request, or `None` for connection/hello failures.
    pub request_id: Option<RequestId>,
}

/// Failure settling exactly one dispatched client request.
///
/// The wire keeps the general [`ProtocolFailure`] shape because hello-time
/// version rejections legitimately implicate no request. A received client
/// request, however, must always be settled by a failure that names it: this
/// owned value makes that correlation mandatory. The triggering
/// [`RequestId`] is derived from the settled frame itself rather than passed
/// separately, so a dispatch failure can never disagree with the request it
/// answers, the fields stay private so the correlation cannot be mutated
/// afterwards, and conversion into [`ProtocolFailure`] always selects the
/// correlated arm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchFailure {
    code: ErrorCode,
    detail: ErrorDetail,
    retryable: bool,
    request_id: RequestId,
}

impl DispatchFailure {
    /// Builds the rejection that settles the client request carried by
    /// `frame`.
    ///
    /// The correlation identity comes from the frame's own client-minted id
    /// -- the same id every conforming response echoes -- never from a
    /// second argument that could drift from the settled request. Returns
    /// [`None`] when the frame carries no client request body (hello,
    /// welcome, response, event, protocol error, patch batch): those
    /// failures stay uncorrelated on the wire.
    #[must_use]
    pub fn settling(
        frame: &WireEnvelope,
        code: ErrorCode,
        detail: ErrorDetail,
        retryable: bool,
    ) -> Option<Self> {
        if !matches!(frame.body, WireEnvelopeBody::Request(_)) {
            return None;
        }
        let request_id = frame.frame_id.to_request_id().ok()?;
        Some(Self {
            code,
            detail,
            retryable,
            request_id,
        })
    }

    /// Stable classification of the failure.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Bounded human-readable evidence.
    #[must_use]
    pub const fn detail(&self) -> &ErrorDetail {
        &self.detail
    }

    /// Whether repeating the identical request later may succeed.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    /// Mandatory triggering request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }
}

impl From<DispatchFailure> for ProtocolFailure {
    fn from(value: DispatchFailure) -> Self {
        Self {
            code: value.code,
            detail: value.detail,
            retryable: value.retryable,
            request_id: Some(value.request_id),
        }
    }
}

/// Owned application frame body.
#[derive(Eq, PartialEq)]
pub enum WireEnvelopeBody {
    /// Authenticated client negotiation offer.
    Hello(Hello),
    /// Successful server negotiation answer.
    Welcome(Welcome),
    /// First-workflow client request.
    Request(ClientRequest),
    /// Successful correlated server response.
    Response(ServerResponse),
    /// Durable Forge-originated event with its replay sequence.
    Event(ServerEvent),
    /// Typed rejection or failure.
    ProtocolError(ProtocolFailure),
    /// Contiguous conversation replay after a known cursor.
    PatchBatch(PatchBatch),
}

/// One fully owned application-protocol frame.
#[derive(Eq, PartialEq)]
pub struct WireEnvelope {
    /// Revision stamped on this frame.
    pub protocol_version: ProtocolVersion,
    /// Sender-minted frame identity.
    pub frame_id: FrameId,
    /// Sender timestamp as signed Unix epoch milliseconds.
    pub sent_at: UnixMillis,
    /// Exactly one message family.
    pub body: WireEnvelopeBody,
}

impl WireEnvelope {
    /// Validates command/frame request-correlation invariants.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::RequestCorrelationMismatch`] when an
    /// idempotent mutation's domain request id differs from its frame id.
    pub fn validate_correlation(&self) -> Result<(), ProtocolValueError> {
        match &self.body {
            WireEnvelopeBody::Request(ClientRequest::Command(command))
                if command.request_id().as_str() != self.frame_id.as_str() =>
            {
                Err(ProtocolValueError::RequestCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::FirstMessageQueued(receipt),
            }) if request_id != &receipt.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::ThreadEngineConfigSet(result),
            }) if request_id != &result.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            _ => Ok(()),
        }
    }
}
