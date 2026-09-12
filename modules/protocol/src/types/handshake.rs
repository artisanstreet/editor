//! Handshake values, identity wrappers, capabilities, and lifecycle control.

#![forbid(unsafe_code)]

use std::fmt;

use artisan_domain::{IdentifierError, RequestId};
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::Zeroize;

use super::{
    APPLICATION_PROTOCOL_VERSION, ERROR_DETAIL_MAX_BYTES, HELLO_VERSION_MAX_ENTRIES,
    LOCAL_CAPABILITY_BYTES, ProtocolValueError, RECONNECT_CAPABILITY_BYTES,
};
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

    /// Consumes the capability into a zeroizing fixed-size byte buffer.
    ///
    /// The private array is moved into the returned buffer. The consumed
    /// value is replaced with zeroes before its destructor runs, so the
    /// destructor never has to expose or retain the transferred capability.
    #[must_use]
    pub fn into_zeroizing_bytes(mut self) -> zeroize::Zeroizing<[u8; RECONNECT_CAPABILITY_BYTES]> {
        let bytes = std::mem::replace(&mut self.0, [0_u8; RECONNECT_CAPABILITY_BYTES]);
        zeroize::Zeroizing::new(bytes)
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
