//! Owned application-protocol values.
//!
//! Generated Cap'n Proto readers borrow message storage and cannot safely
//! cross service boundaries. These values own their data, preserve the domain
//! vocabulary, and validate protocol-only metadata before the transport sees
//! it.

use std::fmt;

use artisan_domain::{
    Command, DirectoryListing, Event, IdentifierError, MessageId, ProjectListing, ProjectSummary,
    Query, RequestId, ThreadId, ThreadListing, ThreadSummary, UnixMillis,
};
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
#[derive(Eq, PartialEq)]
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

    /// Borrows the secret solely for serialization or constant-time
    /// authentication at a restricted boundary. Callers must never format it.
    #[must_use]
    pub(crate) const fn expose_for_wire(&self) -> &[u8; LOCAL_CAPABILITY_BYTES] {
        &self.0
    }
}

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
#[derive(Eq, PartialEq)]
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

    /// Borrows the secret solely for serialization or constant-time
    /// authentication at a restricted boundary. Callers must never format it.
    #[must_use]
    pub(crate) const fn expose_for_wire(&self) -> &[u8; RECONNECT_CAPABILITY_BYTES] {
        &self.0
    }
}

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
}

/// First-workflow request payload after frame correlation is separated out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientRequest {
    /// Pure domain read.
    Query(Query),
    /// Idempotent domain mutation.
    Command(Command),
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
}

/// Successful response correlated to a client request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerResponse {
    /// Triggering client request identity.
    pub request_id: RequestId,
    /// Successful result.
    pub payload: ResponsePayload,
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
    /// Durable Forge-originated event.
    Event(Event),
    /// Typed rejection or failure.
    ProtocolError(ProtocolFailure),
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
            _ => Ok(()),
        }
    }
}
