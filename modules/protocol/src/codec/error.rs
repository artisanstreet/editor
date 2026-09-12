//! Protocol codec error surfaces.
//!
//! Owns the public encode/decode failure enums and their typed `From`
//! bridges to shared protocol and domain errors.

use thiserror::Error;

#[allow(clippy::wildcard_imports)]
use super::*;

/// Failure while serializing one already-owned protocol frame.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProtocolEncodeError {
    #[error("invalid composer state")]
    ComposerState,

    /// Protocol metadata or request correlation was invalid.
    #[error(transparent)]
    Value(#[from] ProtocolValueError),
    /// A collection could not be represented by Cap'n Proto's list length.
    #[error("{field} holds {length} entries and cannot be represented on the wire")]
    CollectionTooLarge {
        /// Name of the collection.
        field: &'static str,
        /// Offending native length.
        length: usize,
    },
    /// A collection contained a duplicate entry.
    #[error("{field} contains duplicate entry {value:?}")]
    Duplicate {
        /// Name of the collection.
        field: &'static str,
        /// Duplicate value.
        value: String,
    },
    /// A public protocol snapshot failed the existing domain validation.
    #[error("invalid model favorites snapshot: {source}")]
    ModelFavoritesSnapshot {
        /// Domain-owned snapshot validation failure.
        #[source]
        source: ModelFavoritesSnapshotError,
    },
    /// A run-interaction answer list failed its domain bound.
    #[error("invalid run interaction answers: {source}")]
    RunInteraction {
        /// Domain-owned interaction validation failure.
        #[source]
        source: RunInteractionError,
    },
}

/// Failure while reading and validating one external protocol frame.
#[derive(Debug, Error)]
pub enum ProtocolDecodeError {
    #[error("invalid composer state: {0}")]
    ComposerState(#[from] crate::composer_state_codec::ComposerStateCodecError),

    /// Cap'n Proto framing, pointer, nesting, or allocation validation failed.
    #[error("invalid Cap'n Proto message: {source}")]
    Capnp {
        /// Underlying generated/runtime failure.
        #[source]
        source: capnp::Error,
    },
    /// A transport frame contained more than one serialized message or
    /// otherwise appended bytes after its single envelope.
    #[error("application frame has {length} trailing bytes after its envelope")]
    TrailingBytes {
        /// Number of bytes not consumed by the first Cap'n Proto message.
        length: usize,
    },
    /// A union or enum carried an ordinal unknown to this revision.
    #[error("unknown Cap'n Proto discriminant {value}")]
    UnknownDiscriminant {
        /// Unknown ordinal.
        value: u16,
    },
    /// A text field was not valid UTF-8.
    #[error("{field} is not valid UTF-8: {source}")]
    InvalidUtf8 {
        /// Field being decoded.
        field: &'static str,
        /// UTF-8 validation failure.
        #[source]
        source: std::str::Utf8Error,
    },
    /// Protocol-owned metadata failed validation.
    #[error("invalid protocol metadata: {source}")]
    ProtocolValue {
        /// Validation failure.
        #[source]
        source: ProtocolValueError,
    },
    /// Capability length failed without exposing capability bytes.
    #[error("invalid local capability: {source}")]
    LocalCapability {
        /// Length-only validation failure.
        #[source]
        source: LocalCapabilityError,
    },
    /// Rotated reconnect capability length failed without exposing its bytes.
    #[error("invalid reconnect capability: {source}")]
    ReconnectCapability {
        /// Length-only validation failure.
        #[source]
        source: ReconnectCapabilityError,
    },
    /// Hello version list failed bounded negotiation validation.
    #[error("invalid hello version offer: {source}")]
    VersionOffer {
        /// Offer validation failure.
        #[source]
        source: VersionOfferError,
    },
    /// A domain identifier field failed validation.
    #[error("invalid {field}: {source}")]
    Identifier {
        /// Field being decoded.
        field: &'static str,
        /// Shared identifier failure.
        #[source]
        source: IdentifierError,
    },
    /// A display label failed its domain bound.
    #[error("invalid {field}: {source}")]
    DisplayName {
        /// Field being decoded.
        field: &'static str,
        /// Domain text failure.
        #[source]
        source: DisplayNameError,
    },
    /// A project root description failed its domain bound.
    #[error("invalid project root path: {source}")]
    RootPath {
        /// Domain text failure.
        #[source]
        source: RootPathError,
    },
    /// A thread title failed its domain bound.
    #[error("invalid thread title: {source}")]
    ThreadTitle {
        /// Domain text failure.
        #[source]
        source: ThreadTitleError,
    },
    /// A message body failed its domain bound.
    #[error("invalid message body: {source}")]
    MessageBody {
        /// Domain text failure.
        #[source]
        source: MessageBodyError,
    },
    /// Optional authored text or image payload failed its domain bound.
    #[error("invalid queued message payload: {source}")]
    MessagePayload {
        /// Domain payload validation failure.
        #[source]
        source: QueueMessagePayloadError,
    },
    /// One image attachment failed its domain bound.
    #[error("invalid {field}: {source}")]
    ImageAttachment {
        /// Attachment field being decoded.
        field: &'static str,
        /// Domain attachment validation failure.
        #[source]
        source: ImageAttachmentError,
    },
    /// A renderer-visible image reference failed its bounded validation.
    #[error("invalid image attachment reference in {field}: {reason}")]
    ImageAttachmentReference {
        /// Reference field being decoded.
        field: &'static str,
        /// Stable validation reason.
        reason: &'static str,
    },
    /// An assistant body failed its domain bound.
    #[error("invalid assistant body: {source}")]
    AssistantBody {
        /// Domain text failure.
        #[source]
        source: AssistantBodyError,
    },
    /// Directory collection bounds were exceeded.
    #[error("invalid directory listing: {source}")]
    DirectoryListing {
        /// Domain collection failure.
        #[source]
        source: DirectoryListingError,
    },
    /// Attached-project collection bounds were exceeded.
    #[error("invalid project listing: {source}")]
    ProjectListing {
        /// Domain collection failure.
        #[source]
        source: ProjectListingError,
    },
    /// Thread collection bounds were exceeded.
    #[error("invalid thread listing: {source}")]
    ThreadListing {
        /// Domain collection failure.
        #[source]
        source: ThreadListingError,
    },
    /// A conversation counter violated its zero/one-based convention.
    #[error("invalid {field}: {source}")]
    Counter {
        /// Counter-bearing field being decoded.
        field: &'static str,
        /// Domain counter validation failure.
        #[source]
        source: CounterError,
    },
    /// A streamed conversation text fragment exceeded its byte ceiling.
    #[error("invalid conversation text fragment: {source}")]
    IncrementalText {
        /// Domain text validation failure.
        #[source]
        source: IncrementalTextError,
    },
    /// A conversation query requested an invalid turn count.
    #[error("invalid conversation query turn count: {source}")]
    QueryTurnCount {
        /// Domain query-bound validation failure.
        #[source]
        source: QueryTurnCountError,
    },
    /// A decoded conversation snapshot violated structural invariants.
    #[error("invalid conversation snapshot: {source}")]
    ConversationSnapshot {
        /// Domain snapshot validation failure.
        #[source]
        source: ConversationSnapshotError,
    },
    /// A decoded conversation patch batch violated replay invariants.
    #[error("invalid conversation patch batch: {source}")]
    PatchBatch {
        /// Domain replay validation failure.
        #[source]
        source: PatchBatchError,
    },
    /// The placeholder conversation item union arm is never conforming input.
    #[error("conversation item uses the reserved unmodeled union arm")]
    UnmodeledConversationItem,
    /// Two wire correlation fields disagreed.
    #[error("{field} does not match its enclosing request correlation")]
    CorrelationMismatch {
        /// Nested correlation field.
        field: &'static str,
    },
    /// An engine configuration field failed its bounded domain validation.
    #[error("invalid engine configuration: {source}")]
    EngineConfig {
        /// Domain-owned bounded configuration failure.
        #[source]
        source: EngineConfigError,
    },
    /// A catalog snapshot failed the shared wire validation or byte bound.
    #[error("invalid catalog snapshot: {source}")]
    CatalogSnapshot {
        /// Owned catalog snapshot validation failure.
        #[source]
        source: CatalogSnapshotWireError,
    },
    /// A catalog revision failed its domain bound or character policy.
    #[error("invalid catalog revision: {source}")]
    CatalogRevision {
        /// Domain-owned revision validation failure.
        #[source]
        source: CatalogRevisionError,
    },
    /// A favorite model identity failed its domain bound.
    #[error("invalid model favorite id: {source}")]
    ModelFavoriteId {
        /// Domain-owned model-id validation failure.
        #[source]
        source: ModelFavoriteIdError,
    },
    /// A favorite revision failed its domain storage bound.
    #[error("invalid model favorites revision: {source}")]
    ModelFavoritesRevision {
        /// Domain-owned revision validation failure.
        #[source]
        source: ModelFavoritesRevisionError,
    },
    /// A decoded favorites snapshot failed domain cardinality/uniqueness.
    #[error("invalid model favorites snapshot: {source}")]
    ModelFavoritesSnapshot {
        /// Domain-owned snapshot validation failure.
        #[source]
        source: ModelFavoritesSnapshotError,
    },
    /// An account-usage field failed its domain bound or shape policy.
    #[error("invalid account usage: {source}")]
    EngineUsage {
        /// Domain-owned account-usage validation failure.
        #[source]
        source: EngineUsageError,
    },
    /// One sanitized engine observation row failed domain validation.
    ///
    /// Unknown provider labels, bound violations, and requested/resolved
    /// state mismatches all arrive here without carrying provider text.
    #[error("invalid engine observation: {source}")]
    Observation {
        /// Domain-owned observation validation failure.
        #[source]
        source: ObservationError,
    },
    /// A run-interaction answer list failed its domain bound.
    #[error("invalid run interaction answers: {source}")]
    RunInteraction {
        /// Domain-owned interaction validation failure.
        #[source]
        source: RunInteractionError,
    },
}

impl From<capnp::Error> for ProtocolDecodeError {
    fn from(source: capnp::Error) -> Self {
        Self::Capnp { source }
    }
}

impl From<capnp::NotInSchema> for ProtocolDecodeError {
    fn from(source: capnp::NotInSchema) -> Self {
        Self::UnknownDiscriminant { value: source.0 }
    }
}

impl From<ProtocolValueError> for ProtocolDecodeError {
    fn from(source: ProtocolValueError) -> Self {
        Self::ProtocolValue { source }
    }
}

impl From<LocalCapabilityError> for ProtocolDecodeError {
    fn from(source: LocalCapabilityError) -> Self {
        Self::LocalCapability { source }
    }
}

impl From<ReconnectCapabilityError> for ProtocolDecodeError {
    fn from(source: ReconnectCapabilityError) -> Self {
        Self::ReconnectCapability { source }
    }
}

impl From<VersionOfferError> for ProtocolDecodeError {
    fn from(source: VersionOfferError) -> Self {
        Self::VersionOffer { source }
    }
}

impl From<AssistantBodyError> for ProtocolDecodeError {
    fn from(source: AssistantBodyError) -> Self {
        Self::AssistantBody { source }
    }
}

impl From<QueueMessagePayloadError> for ProtocolDecodeError {
    fn from(source: QueueMessagePayloadError) -> Self {
        Self::MessagePayload { source }
    }
}

impl From<IncrementalTextError> for ProtocolDecodeError {
    fn from(source: IncrementalTextError) -> Self {
        Self::IncrementalText { source }
    }
}

impl From<QueryTurnCountError> for ProtocolDecodeError {
    fn from(source: QueryTurnCountError) -> Self {
        Self::QueryTurnCount { source }
    }
}

impl From<ConversationSnapshotError> for ProtocolDecodeError {
    fn from(source: ConversationSnapshotError) -> Self {
        Self::ConversationSnapshot { source }
    }
}

impl From<PatchBatchError> for ProtocolDecodeError {
    fn from(source: PatchBatchError) -> Self {
        Self::PatchBatch { source }
    }
}

impl From<EngineConfigError> for ProtocolDecodeError {
    fn from(source: EngineConfigError) -> Self {
        Self::EngineConfig { source }
    }
}

impl From<CatalogSnapshotWireError> for ProtocolDecodeError {
    fn from(source: CatalogSnapshotWireError) -> Self {
        Self::CatalogSnapshot { source }
    }
}

impl From<CatalogRevisionError> for ProtocolDecodeError {
    fn from(source: CatalogRevisionError) -> Self {
        Self::CatalogRevision { source }
    }
}

impl From<ModelFavoriteIdError> for ProtocolDecodeError {
    fn from(source: ModelFavoriteIdError) -> Self {
        Self::ModelFavoriteId { source }
    }
}

impl From<ModelFavoritesRevisionError> for ProtocolDecodeError {
    fn from(source: ModelFavoritesRevisionError) -> Self {
        Self::ModelFavoritesRevision { source }
    }
}

impl From<ModelFavoritesSnapshotError> for ProtocolDecodeError {
    fn from(source: ModelFavoritesSnapshotError) -> Self {
        Self::ModelFavoritesSnapshot { source }
    }
}

impl From<EngineUsageError> for ProtocolDecodeError {
    fn from(source: EngineUsageError) -> Self {
        Self::EngineUsage { source }
    }
}

impl From<ObservationError> for ProtocolDecodeError {
    fn from(source: ObservationError) -> Self {
        Self::Observation { source }
    }
}
