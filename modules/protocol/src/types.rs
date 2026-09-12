//! Owned application-protocol values.
//!
//! Generated Cap'n Proto readers borrow message storage and cannot safely
//! cross service boundaries. These values own their data, preserve the domain
//! vocabulary, and validate protocol-only metadata before the transport sees
//! it.
//!
//! The values are grouped by wire family: [`handshake`] (hello, welcome,
//! identity, capabilities, lifecycle), [`catalog`] (catalog, settings,
//! favorites, rich links), [`dispatch`] (client requests and receipts), and
//! [`envelope`] (responses, failures, and the frame body).

#![forbid(unsafe_code)]

mod catalog;
mod dispatch;
mod envelope;
mod handshake;

pub use self::catalog::*;
pub use self::dispatch::*;
pub use self::envelope::*;
pub use self::handshake::{LifecycleState, *};

use artisan_domain::IdentifierError;
use thiserror::Error;

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
/// Maximum encoded shared catalog snapshot carried by one response payload.
///
/// The application frame retains at least one additional MiB for Cap'n Proto
/// metadata and envelope overhead.
pub const CATALOG_SNAPSHOT_MAX_BYTES: usize = 15 * 1024 * 1024;
/// Maximum UTF-8 byte length of one wire-supplied rich-link URL.
pub const RICH_LINK_URL_MAX_BYTES: usize = 2_048;
/// Maximum UTF-8 byte length of one resolved rich-link page name.
pub const RICH_LINK_PAGE_NAME_MAX_BYTES: usize = 2_048;

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
    /// The shared catalog snapshot could not be decoded after construction.
    #[error("catalog snapshot failed shared validation")]
    InvalidCatalogSnapshot,
    /// A catalog result omitted the runtime scope required for profile routing.
    #[error("catalog result is missing its runtime profile scope")]
    CatalogScopeMissing,
    /// The embedded catalog scope belongs to a different engine profile.
    #[error("catalog result profile scope does not match its response profile")]
    CatalogScopeMismatch,
    /// A rich-link URL or resolved page name violated its bounded shape.
    #[error("invalid rich link value: {reason}")]
    RichLink {
        /// Stable validation reason.
        reason: &'static str,
    },
    /// A project-repository field violated its bounded shape or invariant.
    #[error("invalid project repository value: {reason}")]
    Repository {
        /// Stable validation reason.
        reason: &'static str,
    },
}
