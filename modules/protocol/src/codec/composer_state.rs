//! Owned conversions for the standalone composer-state Cap'n Proto leaf.
//!
//! The parent protocol codec owns envelope and union dispatch. This module
//! owns only the imported request/response structs, so the parent arms remain
//! small and the bounded payload cannot fall back to opaque JSON.
//!
//! Request codecs live in [`requests`], listings in [`listings`], drafts and
//! stored attachments in [`drafts`], payloads in
//! [`payload`], run-usage values in [`usage`], summary attachments in
//! [`attachments`], and shared helpers in [`helpers`].

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::wildcard_imports
)]

#[path = "composer_state/attachments.rs"]
mod attachments;
#[path = "composer_state/drafts.rs"]
mod drafts;
#[path = "composer_state/helpers.rs"]
mod helpers;
#[path = "composer_state/listings.rs"]
mod listings;
#[path = "composer_state/payload.rs"]
mod payload;
#[path = "composer_state/requests.rs"]
mod requests;
#[path = "composer_state/submissions.rs"]
mod submissions;
#[path = "composer_state/usage.rs"]
mod usage;

pub use self::drafts::*;
pub use self::helpers::validate_withdrawal_response_correlation;
pub use self::listings::*;
pub use self::payload::*;
pub use self::requests::*;
pub use self::submissions::*;
pub use self::usage::*;
use artisan_domain::composer_state::{
    COMPOSER_STATE_IMAGE_MAX_BYTES, COMPOSER_STATE_IMAGE_MAX_COUNT,
    COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES, QueuedMessageWithdrawalResult, ReadRecalledMessage,
    ReadRunUsage, RecalledMessageResult, RunUsageResult, WithdrawQueuedMessageCommand,
    validate_payload_bounds,
};
use artisan_domain::{
    AuthoredText, AuthoredTextError, CommandReceipt, DispatchError, EngineId, EngineModelId,
    EngineRouteId, EngineVariantId, FAILED_MESSAGE_LIST_MAX, FailedMessageListing,
    FailedMessageSummary, IdentifierError, ImageAttachment, ImageAttachmentRef, ListFailedMessages,
    ListQueuedMessages, MessageId, QUEUED_MESSAGE_LIST_MAX, QueueMessagePayload,
    QueueMessagePayloadError, QueuedMessageListOrder, QueuedMessageListing, QueuedMessageState,
    QueuedMessageSummary, ReceiptDisposition, RequestId, RunId, RunUsageBasis, RunUsageReport,
    RunUsageReportInput, ThreadId, UnixMillis,
};
use thiserror::Error;

use crate::composer_state_capnp;

/// Failure while encoding or decoding one imported composer-state value.
#[derive(Debug, Error)]
pub enum ComposerStateCodecError {
    /// Cap'n Proto rejected the pointer graph or a generated accessor.
    #[error("invalid composer-state Cap'n Proto value: {source}")]
    Capnp {
        /// Runtime validation failure.
        #[source]
        source: capnp::Error,
    },
    /// A text pointer contained invalid UTF-8.
    #[error("{field} is not valid UTF-8: {source}")]
    InvalidUtf8 {
        /// Logical field path.
        field: &'static str,
        /// UTF-8 failure.
        #[source]
        source: std::str::Utf8Error,
    },
    /// An identifier failed the domain grammar.
    #[error("invalid {field}: {source}")]
    Identifier {
        /// Logical field path.
        field: &'static str,
        /// Domain identifier failure.
        #[source]
        source: IdentifierError,
    },
    /// Authored text failed its bounded domain validation.
    #[error("invalid {field}: {source}")]
    AuthoredText {
        /// Logical field path.
        field: &'static str,
        /// Domain text failure.
        #[source]
        source: AuthoredTextError,
    },
    /// A payload failed its domain-level invariant.
    #[error("invalid {field}: {source}")]
    Payload {
        /// Logical field path.
        field: &'static str,
        /// Domain payload failure.
        #[source]
        source: QueueMessagePayloadError,
    },
    /// Image metadata or bytes failed exact domain validation.
    #[error("invalid {field}: image metadata or bytes failed validation")]
    Image {
        /// Logical field path.
        field: &'static str,
    },
    /// A byte-free image reference failed exact domain validation.
    #[error("invalid {field}: image reference failed validation")]
    ImageReference {
        /// Logical field path.
        field: &'static str,
    },
    /// A bounded listing violated its count, scope, or ordering invariant.
    #[error("invalid {field}: queued-message listing invariant failed")]
    Listing {
        /// Logical field path.
        field: &'static str,
    },
    /// A usage report failed its bounded domain validation.
    #[error("invalid {field}: run-usage report failed validation")]
    Usage {
        /// Logical field path.
        field: &'static str,
    },
    /// A value failed one of the composer-state result invariants.
    #[error("invalid {field}: composer-state value failed validation")]
    StateValue {
        /// Logical field path.
        field: &'static str,
    },
    /// A parent response and its nested durable receipt disagreed.
    #[error("{field} does not match the enclosing response request id")]
    ResponseCorrelationMismatch {
        /// Nested field whose correlation failed.
        field: &'static str,
    },
    /// A result did not remain in the exact query scope supplied by its
    /// authenticated caller.
    #[error("{field} does not match the exact query scope")]
    ScopeMismatch {
        /// Scope field that failed.
        field: &'static str,
    },
    /// An enum ordinal is unknown to this revision.
    #[error("unknown composer-state enum value {value} for {field}")]
    UnknownEnum {
        /// Logical enum path.
        field: &'static str,
        /// Unknown ordinal.
        value: u16,
    },
    /// A collection length could not be represented by a Cap'n Proto list.
    #[error("{field} contains {length} entries and cannot be represented on the wire")]
    CollectionTooLarge {
        /// Logical collection path.
        field: &'static str,
        /// Offending native length.
        length: usize,
    },
    /// A noncanonical optional wrapper carried a value while marked absent.
    #[error("{field} carries a value while marked absent")]
    NonCanonicalOptional {
        /// Logical optional field path.
        field: &'static str,
    },
}

impl From<capnp::Error> for ComposerStateCodecError {
    fn from(source: capnp::Error) -> Self {
        Self::Capnp { source }
    }
}
