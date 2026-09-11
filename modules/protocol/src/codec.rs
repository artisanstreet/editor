//! Total conversion between owned protocol values and generated Cap'n Proto.

use artisan_domain::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation, ApprovalKind,
    ApprovalObservation, ApprovalRequest, ApprovalState, ArtisanCode, CompactionObservation,
    CompactionState, DiagnosticLevel, EngineErrorRef, EngineErrorRefInput, EngineObservationAttribution,
    EngineObservationEvent,
    FileAction, FileObservation, LimitScope, MessagePhase, NativeActionObservation,
    OBSERVATION_ANSWERS_MAX, OBSERVATION_PLAN_MAX_ENTRIES, OBSERVATION_QUESTION_MAX_OPTIONS,
    Observation, ObservationError, ObservationId, ObservationSequence, PlanEntry, PlanEntryStatus,
    PlanObservation, ProcessDiagnosticObservation, ProtocolDiagnosticObservation, QuestionInput,
    QuestionObservation, QuestionOption, QuestionState, ReasoningSummaryCompletedObservation,
    ReasoningSummaryDeltaObservation, RetryAttemptState, RetryObservation, RunState,
    RunStateObservation, RunTerminalObservation, RunTerminalState, SearchObservation, SearchScope,
    SearchState, SubagentInput, SubagentObservation, SubagentState, SubagentTranscriptObservation,
    TerminalActivityInput, TerminalActivityObservation, TerminalActivityState, TerminalChannel,
    ToolAction, ToolObservation, TranscriptAgentMessageCompleted, TranscriptAgentMessageDelta,
    TranscriptContent, TranscriptFile, TranscriptReasoningSummaryCompleted,
    TranscriptReasoningSummaryDelta, TranscriptSearch, TranscriptTerminalActivity, TranscriptTool,
    TurnState, TurnStateObservation, UsageBasis, UsageInput, UsageObservation,
};
use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantBodyError, AssistantMessageItem, AssistantMessagePhase,
    AttachProject, AuthoredText, ByteLimit, CONVERSATION_PATCH_BATCH_MAX_PATCHES,
    CONVERSATION_QUERY_MAX_TURNS, CatalogRevision, CatalogRevisionError, ClaudeEffort,
    ClaudePermissionMode, ClaudeSelection, CodexModelContextWindow, CodexReasoningEffort,
    CodexSelection, CodexServiceTier, Command, ConversationCursor, ConversationItem,
    ConversationLifecycle, ConversationPatch, ConversationQuery, ConversationQueryBounds,
    ConversationRequest, ConversationSnapshot, ConversationSnapshotError, ConversationSubscribe,
    ConversationSubscriptionStart, ConversationTurn, ConversationUnsubscribe, CountLimit,
    CounterError, CreateThread, CursorPermissionMode, CursorReasoningEffort, CursorSelection,
    CursorSpeed, DIRECTORY_LISTING_MAX_ENTRIES, DIRECTORY_LISTING_MAX_PLACES, DirectoryEntry,
    DirectoryId, DirectoryKind, DirectoryListing, DirectoryListingError, DirectoryPlace,
    DisplayName, DisplayNameError, ENGINE_USAGE_ENGINES_MAX, ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE, EngineAgentId, EngineConfigError, EngineConfigReason,
    EngineConfigRevision, EngineConfigUpdatePrecondition, EngineId, EngineModelId,
    EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, EngineUsageAuth, EngineUsageAuthentication, EngineUsageError, EngineUsageReport, EngineUsageSnapshot, EngineUsageWindow, EngineUsageWindowKind, EngineVariantId, Event, FilesystemAccess,
    FiniteMillis, FirstMessageQueued, GrokPermissionMode, GrokReasoningEffort, GrokSelection,
    HermesPermissionMode, HermesReasoningEffort, HermesSelection, IdentifierError, ImageAttachment,
    ImageAttachmentError, ImageAttachmentRef, ImageAttachmentRefError, IncrementalText,
    IncrementalTextError, ItemId, ItemOrdinal, ListAttachedProjects, ListDirectories,
    ListProjectThreads, MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES, MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
    MessageBody, MessageBodyError, MessageId, ModelFavoriteId, ModelFavoriteIdError,
    ModelFavoritesRevision, ModelFavoritesRevisionError, ModelFavoritesSnapshotError,
    MultimodalUserMessageItem, NetworkAccess, OpenCode2Selection, PROJECT_LISTING_MAX_PROJECTS,
    PatchBatch, PatchBatchError, PatchId, PatchSequence, PermissionId, PlaceKind, ProjectAttached,
    ProjectId, ProjectListing, ProjectListingError, ProjectSummary, Query, QueryTurnCount,
    QueryTurnCountError, QueueFirstMessage, QueueMessage, QueueMessagePayload,
    QueueMessagePayloadError, QueuedMessage, QuotaSurface, ReadAccountUsage, ReadActiveRun, ReadComposerCatalog,
    ReadModelFavorites, ReceiptDisposition, RequestId, RespondApproval, RespondQuestion, Revision,
    RootPath, RootPathError, RunId, RunInteractionError, SetModelFavorite, SetThreadEngineConfig,
    SteerTarget, StopRun, THREAD_LISTING_MAX_THREADS, ThreadCreated, ThreadId, ThreadListing,
    ThreadListingError, ThreadSummary, ThreadTitle, ThreadTitleError, TurnId, TurnOrdinal,
    UnixMillis, UserMessageItem, WebSearchAccess,
};
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use capnp::serialize;
use thiserror::Error;

use crate::artisan_capnp::{
    self, composer_catalog_result, conversation_item, conversation_patch,
    conversation_query_request, conversation_subscribe_request, conversation_subscription_started,
    directory_listing, directory_pick_outcome, engine_config_precondition, engine_run_config, engine_selection_v2,
    engine_usage_report, engine_usage_snapshot, engine_usage_window, envelope, event,
    lifecycle_request, lifecycle_response, list_directories_request, model_favorites_snapshot,
    protocol_error, query_range, read_account_usage_request, read_composer_catalog_request,
    request, response, set_model_favorite_receipt, set_model_favorite_request,
    set_thread_engine_config_request,
};
use crate::repository::{
    PROJECT_REPOSITORY_MAXIMUM_PROJECTS, ProjectRepository, ProjectRepositoryEntry,
    ProjectRepositoryQuery, ProjectRepositoryQueryResult, REPOSITORY_REMOTE_MAXIMUM,
    RepositoryBranchState, RepositoryHost, RepositoryRemote, RepositorySnapshot,
};
use crate::types::{
    ActiveRunResult, CatalogSnapshotWire, CatalogSnapshotWireError, ClientRequest,
    ComposerCatalogResult, ConnectionId, ConversationSubscriptionStarted,
    ConversationSubscriptionStopped, DirectoryPickOutcome, ErrorCode, ErrorDetail, EventCursor,
    FirstMessageReceipt, FrameId, Hello, HelloCredential, LifecycleRequest, LifecycleResponse,
    LifecycleState, LifecycleStatus, LifecycleStopDisposition, LifecycleStopReceipt,
    LocalCapability, LocalCapabilityError, MessageImageResult, ModelFavoritesSnapshot,
    ProtocolFailure, ProtocolValueError, ProtocolVersion, QueueMessageReceipt, ReconnectCapability,
    ReconnectCapabilityError, RegisteredEngineProfilesResult, ResolveRichLinkRequest,
    RespondApprovalReceipt, RespondQuestionReceipt, ResponsePayload, RichLinkPageMetadata,
    RunInteractionOutcome, RunLiveStatus, ServerEvent, ServerResponse, SetModelFavoriteReceipt,
    SetThreadEngineConfigResult, StopRunDisposition, StopRunReceipt, VersionOffer,
    VersionOfferError, Welcome, WireEnvelope, WireEnvelopeBody,
};

/// Maximum Cap'n Proto graph traversal for one already-framed application
/// message (8,388,608 words, or 64 MiB). Quinn framing applies a tighter byte
/// ceiling; this independent bound prevents pointer amplification in the
/// codec.
pub const CAPNP_TRAVERSAL_LIMIT_WORDS: usize = 8 * 1024 * 1024;
/// The Artisan schema is shallow; 32 levels leaves generous evolution room
/// while bounding hostile pointer nesting explicitly.
pub const CAPNP_NESTING_LIMIT: i32 = 32;

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

/// Serializes one validated owned envelope to canonical Cap'n Proto words.
///
/// # Errors
///
/// Returns [`ProtocolEncodeError`] for mismatched request correlation or a
/// collection not representable by the generated list API.
pub fn encode_envelope(value: &WireEnvelope) -> Result<Vec<u8>, ProtocolEncodeError> {
    value.validate_correlation()?;

    let mut message = Builder::new(HeapAllocator::new());
    {
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(value.protocol_version.get());
        root.set_message_id(value.frame_id.as_str());
        root.set_sent_at_millis(value.sent_at.as_millis());
        encode_body(root, &value.body)?;
    }
    Ok(serialize::write_message_to_words(&message))
}

/// Parses, owns, and validates one Cap'n Proto application envelope.
///
/// # Errors
///
/// Returns [`ProtocolDecodeError`] for malformed framing, unknown schema
/// ordinals, invalid UTF-8, bounds violations, invalid negotiation metadata,
/// or inconsistent correlation.
pub fn decode_envelope(bytes: &[u8]) -> Result<WireEnvelope, ProtocolDecodeError> {
    let mut encoded = bytes;
    let message = serialize::read_message_from_flat_slice(&mut encoded, reader_options())?;
    if !encoded.is_empty() {
        return Err(ProtocolDecodeError::TrailingBytes {
            length: encoded.len(),
        });
    }
    let root: envelope::Reader = message.get_root()?;

    let protocol_version = ProtocolVersion::new(root.get_protocol_version())?;
    let frame_id = FrameId::parse(read_text(root.get_message_id(), "envelope.messageId")?)?;
    let sent_at = UnixMillis::from_millis(root.get_sent_at_millis());
    let body = decode_body(root, &frame_id)?;
    let envelope = WireEnvelope {
        protocol_version,
        frame_id,
        sent_at,
        body,
    };
    envelope.validate_correlation()?;
    Ok(envelope)
}

fn reader_options() -> ReaderOptions {
    let mut options = ReaderOptions::new();
    options.traversal_limit_in_words(Some(CAPNP_TRAVERSAL_LIMIT_WORDS));
    options.nesting_limit(CAPNP_NESTING_LIMIT);
    options
}

fn list_length(field: &'static str, length: usize) -> Result<u32, ProtocolEncodeError> {
    u32::try_from(length).map_err(|_| ProtocolEncodeError::CollectionTooLarge { field, length })
}

fn list_index(field: &'static str, index: usize) -> Result<u32, ProtocolEncodeError> {
    u32::try_from(index).map_err(|_| ProtocolEncodeError::CollectionTooLarge {
        field,
        length: index,
    })
}

fn read_text(
    value: capnp::Result<capnp::text::Reader<'_>>,
    field: &'static str,
) -> Result<String, ProtocolDecodeError> {
    value?
        .to_str()
        .map(str::to_owned)
        .map_err(|source| ProtocolDecodeError::InvalidUtf8 { field, source })
}

fn parse_request_id(value: String, field: &'static str) -> Result<RequestId, ProtocolDecodeError> {
    RequestId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn parse_directory_id(
    value: String,
    field: &'static str,
) -> Result<DirectoryId, ProtocolDecodeError> {
    DirectoryId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn parse_project_id(value: String, field: &'static str) -> Result<ProjectId, ProtocolDecodeError> {
    ProjectId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn parse_thread_id(value: String, field: &'static str) -> Result<ThreadId, ProtocolDecodeError> {
    ThreadId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn parse_profile_id(
    value: String,
    field: &'static str,
) -> Result<EngineProfileId, ProtocolDecodeError> {
    EngineProfileId::parse(value)
        .map_err(|_| engine_config_error(field, EngineConfigReason::InvalidIdentifier))
}

fn parse_message_id(value: String, field: &'static str) -> Result<MessageId, ProtocolDecodeError> {
    MessageId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn parse_turn_id(value: String, field: &'static str) -> Result<TurnId, ProtocolDecodeError> {
    TurnId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn parse_item_id(value: String, field: &'static str) -> Result<ItemId, ProtocolDecodeError> {
    ItemId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn parse_patch_id(value: String, field: &'static str) -> Result<PatchId, ProtocolDecodeError> {
    PatchId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn parse_run_id(value: String, field: &'static str) -> Result<RunId, ProtocolDecodeError> {
    RunId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn encode_body(
    mut root: envelope::Builder<'_>,
    body: &WireEnvelopeBody,
) -> Result<(), ProtocolEncodeError> {
    match body {
        WireEnvelopeBody::Hello(value) => {
            let mut hello = root.reborrow().init_body().init_hello();
            let mut versions = hello.reborrow().init_supported_versions(list_length(
                "hello.supportedVersions",
                value.supported_versions.versions().len(),
            )?);
            for (index, version) in value.supported_versions.versions().iter().enumerate() {
                versions.set(list_index("hello.supportedVersions", index)?, version.get());
            }
            match &value.credential {
                HelloCredential::Initial(capability) => {
                    hello
                        .reborrow()
                        .init_credential()
                        .set_initial(capability.expose_for_wire());
                }
                HelloCredential::Reconnect(capability) => {
                    hello
                        .reborrow()
                        .init_credential()
                        .set_reconnect(capability.expose_for_wire());
                }
            }
            hello.set_supports_lifecycle_control(value.supports_lifecycle_control);
        }
        WireEnvelopeBody::Welcome(value) => {
            let mut welcome = root.reborrow().init_body().init_welcome();
            welcome.set_negotiated_version(value.negotiated_version.get());
            welcome.set_connection_id(value.connection_id.as_str());
            welcome.set_reconnect_capability(value.reconnect_capability.expose_for_wire());
            welcome.set_lifecycle_control_supported(value.lifecycle_control_supported);
        }
        WireEnvelopeBody::Request(value) => {
            encode_request(root.reborrow().init_body().init_request(), value)?;
        }
        WireEnvelopeBody::Response(value) => {
            encode_response(root.reborrow().init_body().init_response(), value)?;
        }
        WireEnvelopeBody::Event(value) => {
            encode_event(root.reborrow().init_body().init_event(), value)?;
        }
        WireEnvelopeBody::ProtocolError(value) => {
            encode_protocol_error(root.reborrow().init_body().init_protocol_error(), value);
        }
        WireEnvelopeBody::PatchBatch(value) => {
            encode_patch_batch(root.reborrow().init_body().init_patch_batch(), value)?;
        }
    }
    Ok(())
}

fn encode_request(
    mut builder: artisan_capnp::request::Builder<'_>,
    value: &ClientRequest,
) -> Result<(), ProtocolEncodeError> {
    match value {
        ClientRequest::Query(Query::ListDirectories(query)) => {
            let mut scope = builder.reborrow().init_list_directories().init_scope();
            if let Some(parent) = &query.parent {
                scope.set_parent(parent.as_str());
            } else {
                scope.set_no_parent(());
            }
        }
        ClientRequest::Query(Query::ListProjectThreads(query)) => {
            builder
                .reborrow()
                .init_list_project_threads()
                .set_project_id(query.project_id.as_str());
        }
        ClientRequest::Query(Query::ListAttachedProjects(_)) => {
            builder.reborrow().init_list_attached_projects();
        }
        ClientRequest::Command(Command::AttachProject(command)) => {
            builder
                .reborrow()
                .init_attach_project()
                .set_directory_id(command.directory_id.as_str());
        }
        ClientRequest::Command(Command::CreateThread(command)) => {
            let mut create = builder.reborrow().init_create_project_thread();
            create.set_project_id(command.project_id.as_str());
            create.set_title(command.title.as_str());
        }
        ClientRequest::Command(Command::QueueFirstMessage(command)) => {
            let mut queue = builder.reborrow().init_queue_first_message();
            queue.set_thread_id(command.thread_id.as_str());
            queue.set_body(command.body.as_str());
        }
        ClientRequest::Command(Command::QueueMessage(command)) => {
            let mut queue = builder.reborrow().init_queue_message();
            queue.set_thread_id(command.thread_id.as_str());
            match command.payload.text() {
                Some(text) => queue.reborrow().init_text().set_present(text.as_str()),
                None => queue.reborrow().init_text().set_absent(()),
            }
            let mut attachments = queue
                .reborrow()
                .init_attachments(command.payload.attachments().len() as u32);
            for (index, attachment) in command.payload.attachments().iter().enumerate() {
                let mut encoded = attachments.reborrow().get(index as u32);
                encoded.set_mime_type(attachment.mime_type_str());
                encoded.set_name(attachment.name());
                encoded.set_bytes(attachment.bytes());
            }
            // Empty steer text is the unnamed (fresh send) encoding; a
            // non-empty value must parse as a RunId on decode.
            match command.steer_target() {
                Some(target) => queue.set_steer_run_id(target.run_id().as_str()),
                None => queue.set_steer_run_id(""),
            }
        }
        ClientRequest::Command(Command::StopRun(command)) => {
            let mut stop = builder.reborrow().init_stop_run();
            stop.set_thread_id(command.thread_id().as_str());
            stop.set_run_id(command.run_id().as_str());
        }
        ClientRequest::Command(Command::RespondApproval(command)) => {
            let mut respond = builder.reborrow().init_respond_approval();
            respond.set_thread_id(command.thread_id().as_str());
            respond.set_run_id(command.run_id().as_str());
            respond.set_approval_id(command.approval_id().as_str());
            respond.set_approved(command.approved());
        }
        ClientRequest::Command(Command::RespondQuestion(command)) => {
            let mut respond = builder.reborrow().init_respond_question();
            respond.set_thread_id(command.thread_id().as_str());
            respond.set_run_id(command.run_id().as_str());
            respond.set_question_id(command.question_id().as_str());
            let mut answers = respond.reborrow().init_answers(list_length(
                "request.respondQuestion.answers",
                command.answers().len(),
            )?);
            for (index, answer) in command.answers().iter().enumerate() {
                answers.set(
                    list_index("request.respondQuestion.answers", index)?,
                    answer.as_str(),
                );
            }
        }
        ClientRequest::Command(Command::SetModelFavorite(command)) => {
            let mut favorite = builder.reborrow().init_set_model_favorite();
            favorite.set_thread_id(command.thread_id().as_str());
            favorite.set_profile_id(command.profile_id().as_str());
            favorite.set_catalog_revision(command.catalog_revision().as_str());
            favorite.set_model_id(command.model_id().as_str());
            favorite.set_favorite(command.favorite());
        }
        ClientRequest::Command(Command::SetThreadEngineConfig(command)) => {
            encode_set_thread_engine_config(builder.reborrow(), command.as_ref());
        }
        ClientRequest::Conversation(ConversationRequest::Query(query)) => {
            encode_conversation_query_request(builder, query);
        }
        ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe)) => {
            let mut encoded = builder.reborrow().init_conversation_subscribe();
            encoded.set_thread_id(subscribe.thread_id.as_str());
            let mut start = encoded.init_start();
            if let Some(after) = subscribe.after {
                start.set_resume_after(after.get());
            } else {
                start.set_fresh(());
            }
        }
        ClientRequest::Conversation(ConversationRequest::Unsubscribe(unsubscribe)) => {
            builder
                .reborrow()
                .init_conversation_unsubscribe()
                .set_thread_id(unsubscribe.thread_id.as_str());
        }
        ClientRequest::PickDirectory => {
            builder.reborrow().set_pick_directory(());
        }
        ClientRequest::Lifecycle(LifecycleRequest::Status) => {
            builder.reborrow().init_lifecycle_control().init_status();
        }
        ClientRequest::Lifecycle(LifecycleRequest::Stop { require_idle }) => {
            builder
                .reborrow()
                .init_lifecycle_control()
                .init_stop()
                .set_require_idle(*require_idle);
        }
        ClientRequest::Query(Query::ReadThreadEngineSettings(query)) => {
            builder
                .reborrow()
                .init_read_thread_engine_settings()
                .set_thread_id(query.thread_id().as_str());
        }
        ClientRequest::Query(Query::ListRegisteredEngineProfiles(_)) => {
            builder.reborrow().init_list_registered_engine_profiles();
        }
        ClientRequest::Query(Query::ReadMessageImage(query)) => {
            let mut encoded = builder.reborrow().init_read_message_image();
            encoded.set_thread_id(query.thread_id().as_str());
            encoded.set_message_id(query.message_id().as_str());
            encoded.set_index(query.index());
        }
        ClientRequest::Query(Query::ReadActiveRun(query)) => {
            builder
                .reborrow()
                .init_read_active_run()
                .set_thread_id(query.thread_id().as_str());
        }
        ClientRequest::Query(Query::ReadComposerCatalog(query)) => {
            let mut catalog = builder.reborrow().init_read_composer_catalog();
            catalog.set_thread_id(query.thread_id().as_str());
            catalog.set_profile_id(query.profile_id().as_str());
        }
        ClientRequest::Query(Query::ReadModelFavorites(_)) => {
            builder.reborrow().set_read_model_favorites(());
        }

        ClientRequest::Query(Query::ReadAccountUsage(query)) => {
            let mut encoded = builder.reborrow().init_read_account_usage();
            match query.engine_id() {
                Some(engine_id) => encoded.reborrow().init_scope().set_one(engine_id),
                None => encoded.reborrow().init_scope().set_all(()),
            }
            encoded.set_force(query.force());
        }

        ClientRequest::Query(Query::ListQueuedMessages(query)) => {
            crate::composer_state_codec::encode_list_queued_messages_request(
                builder.reborrow().init_list_queued_messages(),
                query,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?
        }
        ClientRequest::Command(Command::WithdrawQueuedMessage(command)) => {
            crate::composer_state_codec::encode_withdraw_queued_message_request(
                builder.reborrow().init_withdraw_queued_message(),
                command,
            )
        }
        ClientRequest::Query(Query::ReadRecalledMessage(query)) => {
            crate::composer_state_codec::encode_read_recalled_message_request(
                builder.reborrow().init_read_recalled_message(),
                query,
            )
        }
        ClientRequest::Query(Query::ReadRunUsage(query)) => {
            crate::composer_state_codec::encode_read_run_usage_request(
                builder.reborrow().init_read_run_usage(),
                query,
            )
        }
        ClientRequest::Query(Query::ListFailedMessages(query)) => {
            crate::composer_state_codec::encode_list_failed_messages_request(
                builder.reborrow().init_list_failed_messages(),
                query,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?
        }
        ClientRequest::ResolveRichLink(request) => {
            builder
                .reborrow()
                .init_resolve_rich_link()
                .set_url(request.url());
        }
        ClientRequest::QueryProjectRepository(query) => {
            encode_project_repository_query(
                builder.reborrow().init_query_project_repository(),
                query,
            )?;
        }
    }
    Ok(())
}

fn encode_conversation_query_request(
    mut builder: artisan_capnp::request::Builder<'_>,
    query: &ConversationQuery,
) {
    let mut encoded = builder.reborrow().init_conversation_query();
    encoded.set_thread_id(query.thread_id.as_str());
    match query.bounds {
        ConversationQueryBounds::Window { maximum_turn_count } => {
            encoded
                .init_bounds()
                .init_window()
                .set_maximum_turn_count(maximum_turn_count.get());
        }
        ConversationQueryBounds::Range {
            before_turn_ordinal,
            minimum_turn_ordinal,
            maximum_turn_count,
        } => {
            let mut range = encoded.init_bounds().init_range();
            range.set_before_turn_ordinal(before_turn_ordinal.get());
            let mut minimum = range.reborrow().init_minimum_turn_ordinal();
            if let Some(minimum_turn_ordinal) = minimum_turn_ordinal {
                minimum.set_minimum(minimum_turn_ordinal.get());
            } else {
                minimum.set_no_minimum(());
            }
            range.set_maximum_turn_count(maximum_turn_count.get());
        }
    }
}

fn encode_response(
    mut builder: artisan_capnp::response::Builder<'_>,
    value: &ServerResponse,
) -> Result<(), ProtocolEncodeError> {
    builder.set_request_id(value.request_id.as_str());
    encode_response_payload(builder, &value.payload, &value.request_id)
}

fn encode_response_payload(
    mut builder: artisan_capnp::response::Builder<'_>,
    payload: &ResponsePayload,
    outer_request_id: &RequestId,
) -> Result<(), ProtocolEncodeError> {
    match payload {
        ResponsePayload::QueuedMessages(value) => {
            crate::composer_state_codec::encode_queued_message_listing(
                builder.reborrow().init_queued_messages(),
                value,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?
        }
        ResponsePayload::MessageWithdrawn(value) => {
            crate::composer_state_codec::encode_queued_message_withdrawal_result(
                builder.reborrow().init_message_withdrawn(),
                outer_request_id,
                value,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?
        }
        ResponsePayload::RecalledMessage(value) => {
            crate::composer_state_codec::encode_recalled_message_result(
                builder.reborrow().init_recalled_message(),
                value,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?
        }
        ResponsePayload::RunUsage(value) => crate::composer_state_codec::encode_run_usage_result(
            builder.reborrow().init_run_usage(),
            value,
        )
        .map_err(|_| ProtocolEncodeError::ComposerState)?,
        ResponsePayload::FailedMessages(value) => {
            crate::composer_state_codec::encode_failed_message_listing(
                builder.reborrow().init_failed_messages(),
                value,
            )
            .map_err(|_| ProtocolEncodeError::ComposerState)?
        }
        ResponsePayload::AccountUsage(snapshot) => {
            encode_engine_usage_snapshot(builder.reborrow().init_account_usage(), snapshot)?;
        }
        ResponsePayload::DirectoryListing(listing) => {
            encode_directory_listing(builder.reborrow().init_directory_list(), listing)?;
        }
        ResponsePayload::ProjectListing(listing) => {
            encode_project_listing_response(builder.reborrow(), listing)?;
        }
        ResponsePayload::AttachedProject {
            project,
            disposition,
        } => {
            let mut result = builder.reborrow().init_attached_project();
            encode_project(result.reborrow().init_project(), project);
            result.set_disposition(encode_disposition(*disposition));
        }
        ResponsePayload::ThreadListing(listing) => {
            let mut threads = builder
                .reborrow()
                .init_thread_list()
                .init_threads(list_length(
                    "response.threadList.threads",
                    listing.threads().len(),
                )?);
            for (index, thread) in listing.threads().iter().enumerate() {
                encode_thread(
                    threads
                        .reborrow()
                        .get(list_index("response.threadList.threads", index)?),
                    thread,
                );
            }
        }
        ResponsePayload::CreatedThread {
            thread,
            disposition,
        } => {
            let mut result = builder.reborrow().init_created_thread();
            encode_thread(result.reborrow().init_thread(), thread);
            result.set_disposition(encode_disposition(*disposition));
        }
        ResponsePayload::FirstMessageQueued(receipt) => {
            let mut result = builder.reborrow().init_queued_receipt();
            result.set_request_id(receipt.request_id.as_str());
            result.set_message_id(receipt.message_id.as_str());
            result.set_thread_id(receipt.thread_id.as_str());
            result.set_disposition(encode_disposition(receipt.disposition));
            result.set_state(artisan_capnp::QueuedState::Queued);
        }
        ResponsePayload::MessageQueued(receipt) => {
            let mut result = builder.reborrow().init_queued_message_receipt();
            result.set_request_id(receipt.request_id.as_str());
            result.set_message_id(receipt.message_id.as_str());
            result.set_thread_id(receipt.thread_id.as_str());
            result.set_disposition(encode_disposition(receipt.disposition));
            result.set_state(artisan_capnp::QueuedState::Queued);
        }
        ResponsePayload::MessageImage(result) => {
            let mut encoded = builder.reborrow().init_message_image();
            encode_image_attachment_ref(encoded.reborrow().init_reference(), &result.reference);
            encoded.set_bytes(&result.bytes);
        }
        ResponsePayload::RunStopped(receipt) => {
            let mut encoded = builder.reborrow().init_stop_run_receipt();
            encoded.set_request_id(receipt.request_id.as_str());
            encoded.set_thread_id(receipt.thread_id.as_str());
            encoded.set_run_id(receipt.run_id.as_str());
            encoded.set_disposition(encode_stop_run_disposition(receipt.disposition));
        }
        ResponsePayload::ApprovalResponse(receipt) => {
            encode_respond_approval_receipt(builder.reborrow().init_approval_response(), receipt)?;
        }
        ResponsePayload::QuestionResponse(receipt) => {
            encode_respond_question_receipt(builder.reborrow().init_question_response(), receipt)?;
        }
        ResponsePayload::ActiveRun(result) => {
            let mut encoded = builder.reborrow().init_active_run();
            match result {
                ActiveRunResult::NoActive { thread_id } => {
                    encoded.set_thread_id(thread_id.as_str());
                    encoded.init_state().set_no_active(());
                }
                ActiveRunResult::Active {
                    thread_id,
                    run_id,
                    status,
                    engine_id,
                } => {
                    encoded.set_thread_id(thread_id.as_str());
                    encoded
                        .reborrow()
                        .init_state()
                        .set_active(run_id.as_str());
                    encoded.set_run_status(encode_run_status(*status));
                    encoded.set_run_engine_id(engine_id.as_str());
                }
            }
        }
        ResponsePayload::ComposerCatalog(result) => {
            result.validate_scope()?;
            let mut encoded = builder.reborrow().init_composer_catalog();
            encoded.set_thread_id(result.thread_id.as_str());
            encoded.set_profile_id(result.profile_id.as_str());
            encoded.set_snapshot_data(result.snapshot.as_bytes());
        }
        ResponsePayload::ModelFavorites(snapshot) => {
            encode_model_favorites_snapshot(builder.reborrow().init_model_favorites(), snapshot)?;
        }
        ResponsePayload::ModelFavoriteSet(receipt) => {
            let mut encoded = builder.reborrow().init_model_favorite_set();
            encoded.set_request_id(receipt.request_id.as_str());
            encoded.set_model_id(receipt.model_id.as_str());
            encoded.set_favorite(receipt.favorite);
            encoded.set_disposition(encode_disposition(receipt.disposition));
            encode_model_favorites_snapshot(encoded.init_snapshot(), &receipt.snapshot)?;
        }
        ResponsePayload::ConversationSnapshot(snapshot) => {
            encode_conversation_snapshot(
                builder.reborrow().init_conversation_snapshot(),
                snapshot,
            )?;
        }
        ResponsePayload::ConversationSubscriptionStarted(started) => {
            let encoded = builder.reborrow().init_conversation_subscription_started();
            match started {
                ConversationSubscriptionStarted::Fresh(start) => {
                    encode_conversation_snapshot(encoded.init_fresh(), start.snapshot())?;
                }
                ConversationSubscriptionStarted::Resumed { thread_id, cursor } => {
                    let mut point = encoded.init_resumed();
                    point.set_thread_id(thread_id.as_str());
                    point.set_cursor(cursor.get());
                }
            }
        }
        ResponsePayload::ConversationSubscriptionStopped(stopped) => {
            builder
                .reborrow()
                .init_conversation_subscription_stopped()
                .set_thread_id(stopped.thread_id.as_str());
        }
        ResponsePayload::DirectoryPicked(outcome) => {
            encode_directory_picked(builder.reborrow().init_directory_picked(), outcome);
        }
        ResponsePayload::Lifecycle(value) => {
            encode_lifecycle_response(builder.reborrow().init_lifecycle_control(), value)?;
        }
        ResponsePayload::ThreadEngineConfigSet(result) => {
            encode_thread_engine_config_result(builder.reborrow(), result);
        }
        ResponsePayload::ThreadEngineSettings(result) => {
            encode_thread_engine_settings_result(builder.reborrow(), result);
        }
        ResponsePayload::RegisteredEngineProfiles(result) => {
            encode_registered_engine_profiles_result(
                builder.reborrow().init_registered_engine_profiles(),
                result,
            )?;
        }
        ResponsePayload::RichLink(result) => {
            result.validate()?;
            let mut encoded = builder.reborrow().init_rich_link();
            encoded.set_requested_url(&result.requested_url);
            encoded.set_page_name(&result.page_name);
            encoded.set_cache_expires_at_ms(result.cache_expires_at_ms);
        }
        ResponsePayload::ProjectRepository(result) => {
            encode_project_repository_query_result(
                builder.reborrow().init_project_repository(),
                result,
            )?;
        }
    }
    Ok(())
}

fn encode_model_favorites_snapshot(
    mut builder: artisan_capnp::model_favorites_snapshot::Builder<'_>,
    snapshot: &ModelFavoritesSnapshot,
) -> Result<(), ProtocolEncodeError> {
    ModelFavoritesSnapshot::new(snapshot.revision, snapshot.model_ids.clone())
        .map_err(|source| ProtocolEncodeError::ModelFavoritesSnapshot { source })?;
    builder.set_revision(snapshot.revision.get());
    let mut model_ids = builder.init_model_ids(list_length(
        "modelFavoritesSnapshot.modelIds",
        snapshot.model_ids.len(),
    )?);
    for (index, model_id) in snapshot.model_ids.iter().enumerate() {
        model_ids.set(
            list_index("modelFavoritesSnapshot.modelIds", index)?,
            model_id.as_str(),
        );
    }
    Ok(())
}

fn encode_engine_usage_window_kind(
    kind: EngineUsageWindowKind,
) -> artisan_capnp::EngineUsageWindowKind {
    match kind {
        EngineUsageWindowKind::Session => artisan_capnp::EngineUsageWindowKind::Session,
        EngineUsageWindowKind::Weekly => artisan_capnp::EngineUsageWindowKind::Weekly,
        EngineUsageWindowKind::Monthly => artisan_capnp::EngineUsageWindowKind::Monthly,
        EngineUsageWindowKind::Unknown => artisan_capnp::EngineUsageWindowKind::Unknown,
    }
}

fn encode_engine_usage_authentication(
    state: EngineUsageAuthentication,
) -> artisan_capnp::EngineUsageAuthentication {
    match state {
        EngineUsageAuthentication::Authenticated => {
            artisan_capnp::EngineUsageAuthentication::Authenticated
        }
        EngineUsageAuthentication::Unauthenticated => {
            artisan_capnp::EngineUsageAuthentication::Unauthenticated
        }
        EngineUsageAuthentication::Unknown => artisan_capnp::EngineUsageAuthentication::Unknown,
    }
}

fn encode_quota_surface(surface: QuotaSurface) -> artisan_capnp::QuotaSurface {
    match surface {
        QuotaSurface::Supported => artisan_capnp::QuotaSurface::Supported,
        QuotaSurface::Unknown => artisan_capnp::QuotaSurface::Unknown,
        QuotaSurface::Unsupported => artisan_capnp::QuotaSurface::Unsupported,
    }
}

fn encode_engine_usage_window(
    mut builder: artisan_capnp::engine_usage_window::Builder<'_>,
    window: &EngineUsageWindow,
) {
    builder.set_id(window.id());
    builder.set_kind(encode_engine_usage_window_kind(window.kind()));
    builder.set_label(window.label().unwrap_or(""));
    builder.set_percent_used(window.percent_used());
    builder.set_resets_at(window.resets_at().unwrap_or(""));
    builder.set_window_minutes(window.window_minutes().unwrap_or(0));
}

fn encode_engine_usage_report(
    mut builder: artisan_capnp::engine_usage_report::Builder<'_>,
    report: &EngineUsageReport,
) -> Result<(), ProtocolEncodeError> {
    builder.set_engine_id(report.engine_id());
    builder.set_display_name(report.display_name());
    builder.set_authentication(encode_engine_usage_authentication(
        report.authentication().state(),
    ));
    builder.set_auth_reason(report.authentication().reason().unwrap_or(""));
    builder.set_account_email(report.account_email().unwrap_or(""));
    match report.quota_surface() {
        None => builder.reborrow().init_quota_surface().set_absent(()),
        Some(surface) => builder
            .reborrow()
            .init_quota_surface()
            .set_present(encode_quota_surface(surface)),
    }
    builder.set_failure(report.failure().unwrap_or(""));
    let mut windows = builder.init_windows(list_length(
        "response.accountUsage.windows",
        report.windows().len(),
    )?);
    for (index, window) in report.windows().iter().enumerate() {
        encode_engine_usage_window(
            windows
                .reborrow()
                .get(list_index("response.accountUsage.windows", index)?),
            window,
        );
    }
    Ok(())
}

fn encode_engine_usage_snapshot(
    mut builder: artisan_capnp::engine_usage_snapshot::Builder<'_>,
    snapshot: &EngineUsageSnapshot,
) -> Result<(), ProtocolEncodeError> {
    builder.set_fetched_at(snapshot.fetched_at());
    let mut engines = builder.init_engines(list_length(
        "response.accountUsage.engines",
        snapshot.engines().len(),
    )?);
    for (index, engine) in snapshot.engines().iter().enumerate() {
        encode_engine_usage_report(
            engines
                .reborrow()
                .get(list_index("response.accountUsage.engines", index)?),
            engine,
        )?;
    }
    Ok(())
}

fn encode_project_listing_response(
    builder: artisan_capnp::response::Builder<'_>,
    listing: &ProjectListing,
) -> Result<(), ProtocolEncodeError> {
    let mut projects = builder.init_project_list().init_projects(list_length(
        "response.projectList.projects",
        listing.projects().len(),
    )?);
    for (index, project) in listing.projects().iter().enumerate() {
        encode_project(
            projects
                .reborrow()
                .get(list_index("response.projectList.projects", index)?),
            project,
        );
    }
    Ok(())
}

fn encode_set_thread_engine_config(
    mut builder: artisan_capnp::request::Builder<'_>,
    command: &SetThreadEngineConfig,
) {
    let mut encoded = builder.reborrow().init_set_thread_engine_config();
    encoded.set_thread_id(command.thread_id().as_str());
    encode_engine_config_precondition(
        encoded.reborrow().init_precondition(),
        command.precondition(),
    );
    encode_engine_run_config(encoded.init_config(), command.config());
}

fn encode_thread_engine_config_result(
    mut builder: artisan_capnp::response::Builder<'_>,
    result: &SetThreadEngineConfigResult,
) {
    let mut encoded = builder.reborrow().init_thread_engine_config_set();
    encoded.set_request_id(result.request_id.as_str());
    encoded.set_thread_id(result.thread_id.as_str());
    encoded.set_revision(result.revision.get());
    encoded.set_disposition(encode_disposition(result.disposition));
}

fn encode_thread_engine_settings_result(
    mut builder: artisan_capnp::response::Builder<'_>,
    value: &crate::types::ThreadEngineSettingsResult,
) {
    let mut encoded = builder.reborrow().init_thread_engine_settings();
    encoded.set_thread_id(value.thread_id().as_str());
    match value {
        crate::types::ThreadEngineSettingsResult::Unconfigured { .. } => {
            encoded.init_state().set_unconfigured(());
        }
        crate::types::ThreadEngineSettingsResult::Configured {
            revision, config, ..
        } => {
            let mut configured = encoded.init_state().init_configured();
            configured.set_revision(revision.get());
            encode_engine_run_config(configured.init_config(), config);
        }
    }
}

fn encode_registered_engine_profiles_result(
    builder: artisan_capnp::registered_engine_profiles_result::Builder<'_>,
    value: &RegisteredEngineProfilesResult,
) -> Result<(), ProtocolEncodeError> {
    match value {
        RegisteredEngineProfilesResult::RegistryMissing => {
            builder.init_state().set_registry_missing(());
        }
        RegisteredEngineProfilesResult::RegistryPresent { profile_ids } => {
            if profile_ids.len() > 64 {
                return Err(ProtocolEncodeError::CollectionTooLarge {
                    field: "response.registeredEngineProfiles.profileIds",
                    length: profile_ids.len(),
                });
            }
            let mut seen = std::collections::HashSet::with_capacity(profile_ids.len());
            for id in profile_ids {
                if !seen.insert(id.as_str()) {
                    return Err(ProtocolEncodeError::Duplicate {
                        field: "response.registeredEngineProfiles.profileIds",
                        value: id.as_str().to_owned(),
                    });
                }
            }
            let mut list = builder
                .init_state()
                .init_registry_present()
                .init_profile_ids(list_length(
                    "response.registeredEngineProfiles.profileIds",
                    profile_ids.len(),
                )?);
            for (index, id) in profile_ids.iter().enumerate() {
                list.set(
                    list_index("response.registeredEngineProfiles.profileIds", index)?,
                    id.as_str(),
                );
            }
        }
    }
    Ok(())
}

fn encode_engine_config_precondition(
    mut builder: engine_config_precondition::Builder<'_>,
    value: EngineConfigUpdatePrecondition,
) {
    match value {
        EngineConfigUpdatePrecondition::Unconfigured => {
            builder.set_kind("unconfigured");
            builder.set_revision(0);
        }
        EngineConfigUpdatePrecondition::Exact(revision) => {
            builder.set_kind("exact_revision");
            builder.set_revision(revision.get());
        }
    }
}

fn encode_engine_variant(
    mut builder: artisan_capnp::engine_variant::Builder<'_>,
    variant: Option<&EngineVariantId>,
) {
    if let Some(id) = variant {
        builder.set_kind("selected");
        builder.set_id(id.as_str());
    } else {
        builder.set_kind("none");
        builder.set_id("");
    }
}

fn encode_engine_permission(
    mut builder: artisan_capnp::engine_permission_policy::Builder<'_>,
    permission: &EnginePermissionPolicy,
) {
    builder.set_permission_id(permission.permission_id().as_str());
    builder.set_agent_id(permission.agent_id().as_str());
    builder.set_approval(permission.approval().as_str());
    builder.set_filesystem(permission.filesystem().as_str());
    builder.set_network(permission.network().as_str());
    builder.set_web_search(permission.web_search().as_str());
}

fn encode_engine_runtime(
    mut builder: artisan_capnp::engine_runtime_controls::Builder<'_>,
    runtime: EngineRuntimeControls,
) {
    builder.set_attempt_budget_ms(runtime.attempt_budget().get());
    builder.set_readiness_budget_ms(runtime.readiness_budget().get());
    builder.set_health_budget_ms(runtime.health_budget().get());
    builder.set_prompt_budget_ms(runtime.prompt_budget().get());
    builder.set_stream_budget_ms(runtime.stream_budget().get());
    builder.set_close_budget_ms(runtime.close_budget().get());
    builder.set_max_json_body_bytes(runtime.max_json_body_bytes().get());
    builder.set_max_sse_line_bytes(runtime.max_sse_line_bytes().get());
    builder.set_max_sse_event_bytes(runtime.max_sse_event_bytes().get());
    builder.set_max_readiness_line_bytes(runtime.max_readiness_line_bytes().get());
    builder.set_max_header_count(runtime.max_header_count().get());
    builder.set_max_http_buffer_bytes(runtime.max_http_buffer_bytes().get());
    builder.set_max_stderr_bytes(runtime.max_stderr_bytes().get());
    builder.set_observation_capacity(runtime.observation_capacity().get());
}

fn encode_engine_run_config(mut builder: engine_run_config::Builder<'_>, value: &EngineRunConfig) {
    match value.selection() {
        EngineSelection::OpenCode2(selection) => {
            builder.set_schema_version(1);
            builder.set_engine(EngineId::OpenCode2.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().as_str());
            builder.set_route_id(selection.route_id().as_str());
            encode_engine_variant(builder.reborrow().init_variant(), selection.variant_id());
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            builder.init_selection_v2().set_unset(());
        }
        EngineSelection::Codex(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Codex.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            builder.set_route_id("");
            encode_engine_variant(builder.reborrow().init_variant(), None);
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_codex();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            arm.set_reasoning_effort(
                selection
                    .reasoning_effort()
                    .map_or("", CodexReasoningEffort::as_str),
            );
            arm.set_service_tier(
                selection
                    .service_tier()
                    .map_or("", CodexServiceTier::as_str),
            );
            arm.set_model_context_window(
                selection
                    .model_context_window()
                    .map_or(0, CodexModelContextWindow::get),
            );
            encode_engine_permission(arm.reborrow().init_permission(), selection.permission());
        }
        EngineSelection::Claude(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Claude.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            builder.set_route_id("");
            encode_engine_variant(builder.reborrow().init_variant(), None);
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_claude();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            arm.set_effort(selection.effort().map_or("", ClaudeEffort::as_str));
            arm.set_permission_mode(
                selection
                    .permission_mode()
                    .map_or("", ClaudePermissionMode::as_str),
            );
            arm.set_disable_tools(selection.disable_tools());
            arm.set_safe_mode(selection.safe_mode());
            encode_engine_permission(arm.reborrow().init_permission(), selection.permission());
        }
        EngineSelection::Grok(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Grok.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            builder.set_route_id("");
            encode_engine_variant(builder.reborrow().init_variant(), None);
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_grok();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            arm.set_reasoning_effort(
                selection
                    .reasoning_effort()
                    .map_or("", GrokReasoningEffort::as_str),
            );
            arm.set_permission_mode(
                selection
                    .permission_mode()
                    .map_or("", GrokPermissionMode::as_str),
            );
            encode_engine_permission(arm.reborrow().init_permission(), selection.permission());
        }
        EngineSelection::Cursor(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Cursor.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            builder.set_route_id("");
            encode_engine_variant(builder.reborrow().init_variant(), None);
            encode_engine_permission(builder.reborrow().init_permission(), selection.permission());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_cursor();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().map_or("", EngineModelId::as_str));
            arm.set_reasoning_effort(
                selection
                    .reasoning_effort()
                    .map_or("", CursorReasoningEffort::as_str),
            );
            arm.set_speed(selection.speed().map_or("", CursorSpeed::as_str));
            arm.set_permission_mode(
                selection
                    .permission_mode()
                    .map_or("", CursorPermissionMode::as_str),
            );
            encode_engine_permission(arm.reborrow().init_permission(), selection.permission());
        }
        EngineSelection::Hermes(selection) => {
            builder.set_schema_version(2);
            builder.set_engine(EngineId::Hermes.as_str());
            builder.set_profile_id(selection.profile_id().as_str());
            builder.set_model_id(selection.model_id().as_str());
            builder.set_route_id(selection.route_id().as_str());
            encode_engine_variant(builder.reborrow().init_variant(), None);
            // Hermes authorization is profile-owned; the legacy mirror
            // carries a restrictive sentinel that old readers reject along
            // with the unknown engine instead of misreading it.
            let mut permission = builder.reborrow().init_permission();
            permission.set_permission_id("hermes-managed");
            permission.set_agent_id("hermes-managed-agent");
            permission.set_approval(ApprovalMode::Never.as_str());
            permission.set_filesystem(FilesystemAccess::None.as_str());
            permission.set_network(NetworkAccess::Disabled.as_str());
            permission.set_web_search(WebSearchAccess::Disabled.as_str());
            encode_engine_runtime(builder.reborrow().init_runtime(), value.runtime());
            let mut arm = builder.reborrow().init_selection_v2().init_hermes();
            arm.set_profile_id(selection.profile_id().as_str());
            arm.set_model_id(selection.model_id().as_str());
            arm.set_route_id(selection.route_id().as_str());
            arm.set_reasoning_effort(
                selection
                    .reasoning_effort()
                    .map_or("", HermesReasoningEffort::as_str),
            );
            arm.set_permission_mode(selection.permission_mode().as_str());
            arm.set_fast(selection.fast());
        }
    }
}

fn encode_lifecycle_response(
    mut builder: artisan_capnp::lifecycle_response::Builder<'_>,
    value: &LifecycleResponse,
) -> Result<(), ProtocolEncodeError> {
    match value {
        LifecycleResponse::Status(status) => {
            status.validate()?;
            let mut encoded = builder.reborrow().init_status();
            encoded.set_state(encode_lifecycle_state(status.state));
            encoded.set_active_work_count(status.active_work_count);
        }
        LifecycleResponse::Stop(receipt) => {
            let mut encoded = builder.reborrow().init_stop();
            encoded.set_disposition(encode_lifecycle_stop_disposition(receipt.disposition));
            encoded.set_state(encode_lifecycle_state(receipt.state));
        }
    }
    Ok(())
}

fn encode_directory_picked(
    mut builder: artisan_capnp::directory_pick_outcome::Builder<'_>,
    outcome: &DirectoryPickOutcome,
) {
    match outcome {
        DirectoryPickOutcome::Selected(directory_id) => {
            builder.set_selected(directory_id.as_str());
        }
        DirectoryPickOutcome::Cancelled => {
            builder.set_cancelled(());
        }
    }
}

fn encode_event(
    mut builder: artisan_capnp::event::Builder<'_>,
    value: &ServerEvent,
) -> Result<(), ProtocolEncodeError> {
    builder.set_cursor(value.cursor.get());
    match &value.event {
        Event::ProjectAttached(event) => {
            encode_project(builder.reborrow().init_project_attached(), &event.project);
        }
        Event::ThreadCreated(event) => {
            encode_thread(builder.reborrow().init_thread_created(), &event.thread);
        }
        Event::FirstMessageQueued(event) => {
            let mut queued = builder.reborrow().init_first_message_queued();
            queued.set_request_id(event.message.request_id.as_str());
            queued.set_message_id(event.message.message_id.as_str());
            queued.set_thread_id(event.message.thread_id.as_str());
            queued.set_body(event.message.body.as_str());
        }
        Event::EngineObservation(event) => {
            let mut observation = builder.reborrow().init_engine_observation();
            observation.set_thread_id(event.thread_id.as_str());
            encode_engine_observation(observation.reborrow().init_observation(), &event.observation)?;
            match &event.attribution {
                Some(attribution) => {
                    let mut encoded = observation
                        .reborrow()
                        .init_attribution()
                        .init_attribution();
                    encoded.set_run_id(attribution.run_id.as_str());
                    encoded.set_turn_id(attribution.turn_id.as_str());
                    encoded.set_committed_at_millis(attribution.committed_at.as_millis());
                    encoded.set_delivery_sequence(attribution.delivery_sequence);
                }
                None => {
                    observation
                        .reborrow()
                        .init_attribution()
                        .set_no_attribution(());
                }
            }
        }
    }
    Ok(())
}

fn encode_protocol_error(
    mut builder: artisan_capnp::protocol_error::Builder<'_>,
    value: &ProtocolFailure,
) {
    builder.set_code(encode_error_code(value.code));
    builder.set_message(value.detail.as_str());
    builder.set_retryable(value.retryable);
    if let Some(request_id) = &value.request_id {
        builder.set_correlated(request_id.as_str());
    } else {
        builder.set_uncorrelated(());
    }
}

fn encode_directory_listing(
    mut builder: artisan_capnp::directory_listing::Builder<'_>,
    value: &DirectoryListing,
) -> Result<(), ProtocolEncodeError> {
    let mut parent = builder.reborrow().init_parent();
    if let Some(directory_id) = value.parent() {
        parent.set_parent(directory_id.as_str());
    } else {
        parent.set_no_parent(());
    }

    let mut places = builder.reborrow().init_places(list_length(
        "directoryListing.places",
        value.places().len(),
    )?);
    for (index, place) in value.places().iter().enumerate() {
        let mut encoded = places
            .reborrow()
            .get(list_index("directoryListing.places", index)?);
        encoded.set_kind(encode_place_kind(place.kind));
        encoded.set_directory_id(place.directory_id.as_str());
        encoded.set_display_name(place.display_name.as_str());
    }

    let mut entries = builder.reborrow().init_entries(list_length(
        "directoryListing.entries",
        value.entries().len(),
    )?);
    for (index, entry) in value.entries().iter().enumerate() {
        let mut encoded = entries
            .reborrow()
            .get(list_index("directoryListing.entries", index)?);
        encoded.set_directory_id(entry.directory_id.as_str());
        encoded.set_display_name(entry.display_name.as_str());
        encoded.set_kind(encode_directory_kind(entry.kind));
        encoded.set_has_children(entry.has_children);
    }
    Ok(())
}

fn encode_project(mut builder: artisan_capnp::project::Builder<'_>, value: &ProjectSummary) {
    builder.set_project_id(value.project_id.as_str());
    builder.set_display_name(value.display_name.as_str());
    builder.set_root_path(value.root_path.as_str());
    builder.set_attached_at_millis(value.attached_at.as_millis());
}

fn encode_thread(mut builder: artisan_capnp::thread_summary::Builder<'_>, value: &ThreadSummary) {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_project_id(value.project_id.as_str());
    builder.set_title(value.title.as_str());
    builder.set_created_at_millis(value.created_at.as_millis());
    builder.set_updated_at_millis(value.updated_at.as_millis());
}

fn encode_conversation_snapshot(
    mut builder: artisan_capnp::conversation_snapshot::Builder<'_>,
    value: &ConversationSnapshot,
) -> Result<(), ProtocolEncodeError> {
    builder.set_thread_id(value.thread_id().as_str());
    builder.set_cursor(value.cursor().get());

    let mut turns = builder.reborrow().init_turns(list_length(
        "conversationSnapshot.turns",
        value.turns().len(),
    )?);
    for (index, turn) in value.turns().iter().enumerate() {
        encode_conversation_turn(
            turns
                .reborrow()
                .get(list_index("conversationSnapshot.turns", index)?),
            turn,
        );
    }

    let mut items = builder.reborrow().init_items(list_length(
        "conversationSnapshot.items",
        value.items().len(),
    )?);
    for (index, item) in value.items().iter().enumerate() {
        encode_conversation_item(
            items
                .reborrow()
                .get(list_index("conversationSnapshot.items", index)?),
            item,
        );
    }

    builder.set_updated_at_millis(value.updated_at().as_millis());
    Ok(())
}

fn encode_conversation_turn(
    mut builder: artisan_capnp::conversation_turn::Builder<'_>,
    value: &ConversationTurn,
) {
    builder.set_turn_id(value.turn_id.as_str());
    builder.set_ordinal(value.ordinal.get());
    builder.set_revision(value.revision.get());
    builder.set_lifecycle(encode_conversation_lifecycle(value.lifecycle));
    builder.set_created_at_millis(value.created_at.as_millis());
    builder.set_updated_at_millis(value.updated_at.as_millis());
}

fn encode_conversation_item(
    builder: artisan_capnp::conversation_item::Builder<'_>,
    value: &ConversationItem,
) {
    match value {
        ConversationItem::UserMessage(message) => {
            let mut encoded = builder.init_user_message();
            encoded.set_item_id(message.item_id.as_str());
            encoded.set_turn_id(message.turn_id.as_str());
            encoded.set_ordinal(message.ordinal.get());
            encoded.set_revision(message.revision.get());
            encoded.set_lifecycle(encode_conversation_lifecycle(message.lifecycle));
            encoded.set_body(message.body.as_str());
            encoded.set_created_at_millis(message.created_at.as_millis());
            encoded.set_updated_at_millis(message.updated_at.as_millis());
            if let Some(source_message_id) = message.source_message_id.as_ref() {
                encoded.set_source_message_id(source_message_id.as_str());
            }
        }
        ConversationItem::MultimodalUserMessage(message) => {
            let mut encoded = builder.init_multimodal_user_message();
            encoded.set_item_id(message.item_id.as_str());
            encoded.set_turn_id(message.turn_id.as_str());
            encoded.set_ordinal(message.ordinal.get());
            encoded.set_revision(message.revision.get());
            encoded.set_lifecycle(encode_conversation_lifecycle(message.lifecycle));
            match message.text.as_ref() {
                Some(text) => encoded.reborrow().init_text().set_present(text.as_str()),
                None => encoded.reborrow().init_text().set_absent(()),
            }
            let mut attachments = encoded
                .reborrow()
                .init_attachments(message.attachments.len() as u32);
            for (index, attachment) in message.attachments.iter().enumerate() {
                let mut encoded_attachment = attachments.reborrow().get(index as u32);
                encoded_attachment.set_message_id(attachment.message_id.as_str());
                encoded_attachment.set_thread_id(attachment.thread_id.as_str());
                encoded_attachment.set_index(attachment.index);
                encoded_attachment.set_mime_type(attachment.mime_type_str());
                encoded_attachment.set_name(attachment.name.as_str());
                encoded_attachment.set_size_bytes(attachment.size_bytes);
                encoded_attachment.set_digest(&attachment.digest[..]);
            }
            encoded.set_created_at_millis(message.created_at.as_millis());
            encoded.set_updated_at_millis(message.updated_at.as_millis());
            if let Some(source_message_id) = message.source_message_id.as_ref() {
                encoded.set_source_message_id(source_message_id.as_str());
            }
        }
        ConversationItem::AssistantMessage(message) => {
            let mut encoded = builder.init_assistant_message();
            encoded.set_item_id(message.item_id.as_str());
            encoded.set_turn_id(message.turn_id.as_str());
            encoded.set_run_id(message.run_id.as_str());
            encoded.set_ordinal(message.ordinal.get());
            encoded.set_revision(message.revision.get());
            encoded.set_lifecycle(encode_conversation_lifecycle(message.lifecycle));
            encoded.set_body(message.body.as_str());
            encoded.set_phase(encode_assistant_message_phase(message.phase));
            encoded.set_created_at_millis(message.created_at.as_millis());
            encoded.set_updated_at_millis(message.updated_at.as_millis());
        }
    }
}

fn encode_image_attachment_ref(
    mut builder: artisan_capnp::image_attachment_ref::Builder<'_>,
    reference: &ImageAttachmentRef,
) {
    builder.set_message_id(reference.message_id.as_str());
    builder.set_thread_id(reference.thread_id.as_str());
    builder.set_index(reference.index);
    builder.set_mime_type(reference.mime_type_str());
    builder.set_name(reference.name.as_str());
    builder.set_size_bytes(reference.size_bytes);
    builder.set_digest(&reference.digest[..]);
}

fn encode_patch_batch(
    mut builder: artisan_capnp::patch_batch::Builder<'_>,
    value: &PatchBatch,
) -> Result<(), ProtocolEncodeError> {
    builder.set_thread_id(value.thread_id().as_str());
    builder.set_from_cursor(value.from_cursor().get());
    builder.set_to_cursor(value.to_cursor().get());
    let mut patches = builder
        .reborrow()
        .init_patches(list_length("patchBatch.patches", value.patches().len())?);
    for (index, patch) in value.patches().iter().enumerate() {
        encode_conversation_patch(
            patches
                .reborrow()
                .get(list_index("patchBatch.patches", index)?),
            patch,
        );
    }
    Ok(())
}

fn encode_conversation_patch(
    mut builder: artisan_capnp::conversation_patch::Builder<'_>,
    value: &ConversationPatch,
) {
    builder.set_patch_id(value.patch_id().as_str());
    builder.set_sequence(value.sequence().get());
    match value {
        ConversationPatch::TurnUpsert { turn, .. } => {
            encode_conversation_turn(builder.init_turn_upsert(), turn);
        }
        ConversationPatch::ItemUpsert { item, .. } => {
            encode_conversation_item(builder.init_item_upsert(), item);
        }
        ConversationPatch::ItemAppend {
            item_id,
            revision,
            text,
            updated_at,
            ..
        } => {
            let mut append = builder.init_item_append();
            append.set_item_id(item_id.as_str());
            append.set_revision(revision.get());
            append.set_text(text.as_str());
            append.set_updated_at_millis(updated_at.as_millis());
        }
        ConversationPatch::ItemLifecycle {
            item_id,
            revision,
            lifecycle,
            updated_at,
            ..
        } => {
            let mut transition = builder.init_item_lifecycle();
            transition.set_item_id(item_id.as_str());
            transition.set_revision(revision.get());
            transition.set_lifecycle(encode_conversation_lifecycle(*lifecycle));
            transition.set_updated_at_millis(updated_at.as_millis());
        }
        ConversationPatch::TurnLifecycle {
            turn_id,
            revision,
            lifecycle,
            updated_at,
            ..
        } => {
            let mut transition = builder.init_turn_lifecycle();
            transition.set_turn_id(turn_id.as_str());
            transition.set_revision(revision.get());
            transition.set_lifecycle(encode_conversation_lifecycle(*lifecycle));
            transition.set_updated_at_millis(updated_at.as_millis());
        }
    }
}

const fn encode_conversation_lifecycle(
    value: ConversationLifecycle,
) -> artisan_capnp::ConversationLifecycle {
    match value {
        ConversationLifecycle::Pending => artisan_capnp::ConversationLifecycle::Pending,
        ConversationLifecycle::Streaming => artisan_capnp::ConversationLifecycle::Streaming,
        ConversationLifecycle::Active => artisan_capnp::ConversationLifecycle::Active,
        ConversationLifecycle::Waiting => artisan_capnp::ConversationLifecycle::Waiting,
        ConversationLifecycle::Completed => artisan_capnp::ConversationLifecycle::Completed,
        ConversationLifecycle::Failed => artisan_capnp::ConversationLifecycle::Failed,
        ConversationLifecycle::Interrupted => artisan_capnp::ConversationLifecycle::Interrupted,
        ConversationLifecycle::Cancelled => artisan_capnp::ConversationLifecycle::Cancelled,
    }
}

const fn encode_assistant_message_phase(
    value: AssistantMessagePhase,
) -> artisan_capnp::AssistantMessagePhase {
    match value {
        AssistantMessagePhase::Unspecified => artisan_capnp::AssistantMessagePhase::Unspecified,
        AssistantMessagePhase::Commentary => artisan_capnp::AssistantMessagePhase::Commentary,
        AssistantMessagePhase::Final => artisan_capnp::AssistantMessagePhase::Final,
    }
}

const fn encode_disposition(value: ReceiptDisposition) -> artisan_capnp::ReceiptDisposition {
    match value {
        ReceiptDisposition::Accepted => artisan_capnp::ReceiptDisposition::Accepted,
        ReceiptDisposition::Duplicate => artisan_capnp::ReceiptDisposition::Duplicate,
    }
}

const fn encode_stop_run_disposition(
    value: StopRunDisposition,
) -> artisan_capnp::StopRunDisposition {
    match value {
        StopRunDisposition::Requested => artisan_capnp::StopRunDisposition::Requested,
        StopRunDisposition::AlreadyRequested => artisan_capnp::StopRunDisposition::AlreadyRequested,
        StopRunDisposition::NotActive => artisan_capnp::StopRunDisposition::NotActive,
    }
}

const fn encode_place_kind(value: PlaceKind) -> artisan_capnp::PlaceKind {
    match value {
        PlaceKind::Home => artisan_capnp::PlaceKind::Home,
        PlaceKind::Desktop => artisan_capnp::PlaceKind::Desktop,
        PlaceKind::Documents => artisan_capnp::PlaceKind::Documents,
        PlaceKind::Downloads => artisan_capnp::PlaceKind::Downloads,
        PlaceKind::Music => artisan_capnp::PlaceKind::Music,
        PlaceKind::Pictures => artisan_capnp::PlaceKind::Pictures,
        PlaceKind::Videos => artisan_capnp::PlaceKind::Videos,
    }
}

const fn encode_directory_kind(value: DirectoryKind) -> artisan_capnp::DirectoryEntryKind {
    match value {
        DirectoryKind::Root => artisan_capnp::DirectoryEntryKind::Root,
        DirectoryKind::Directory => artisan_capnp::DirectoryEntryKind::Directory,
    }
}

const fn encode_lifecycle_state(value: LifecycleState) -> artisan_capnp::LifecycleState {
    match value {
        LifecycleState::Ready => artisan_capnp::LifecycleState::Ready,
        LifecycleState::Busy => artisan_capnp::LifecycleState::Busy,
        LifecycleState::Draining => artisan_capnp::LifecycleState::Draining,
    }
}

const fn encode_lifecycle_stop_disposition(
    value: LifecycleStopDisposition,
) -> artisan_capnp::LifecycleStopDisposition {
    match value {
        LifecycleStopDisposition::Accepted => artisan_capnp::LifecycleStopDisposition::Accepted,
        LifecycleStopDisposition::Duplicate => artisan_capnp::LifecycleStopDisposition::Duplicate,
        LifecycleStopDisposition::AlreadyStopping => {
            artisan_capnp::LifecycleStopDisposition::AlreadyStopping
        }
    }
}

const fn encode_error_code(value: ErrorCode) -> artisan_capnp::ErrorCode {
    match value {
        ErrorCode::UnsupportedVersion => artisan_capnp::ErrorCode::UnsupportedVersion,
        ErrorCode::InvalidInput => artisan_capnp::ErrorCode::InvalidInput,
        ErrorCode::DirectoryUnknown => artisan_capnp::ErrorCode::DirectoryUnknown,
        ErrorCode::ProjectUnknown => artisan_capnp::ErrorCode::ProjectUnknown,
        ErrorCode::ThreadUnknown => artisan_capnp::ErrorCode::ThreadUnknown,
        ErrorCode::Internal => artisan_capnp::ErrorCode::Internal,
        ErrorCode::IdempotencyConflict => artisan_capnp::ErrorCode::IdempotencyConflict,
        ErrorCode::UnsupportedFeature => artisan_capnp::ErrorCode::UnsupportedFeature,
        ErrorCode::LifecycleBusy => artisan_capnp::ErrorCode::LifecycleBusy,
        ErrorCode::EngineConfigConflict => artisan_capnp::ErrorCode::EngineConfigConflict,
    }
}

fn decode_body(
    root: envelope::Reader<'_>,
    frame_id: &FrameId,
) -> Result<WireEnvelopeBody, ProtocolDecodeError> {
    match root.get_body().which()? {
        envelope::body::Which::Hello(value) => Ok(WireEnvelopeBody::Hello(decode_hello(value?)?)),
        envelope::body::Which::Welcome(value) => {
            Ok(WireEnvelopeBody::Welcome(decode_welcome(value?)?))
        }
        envelope::body::Which::Request(value) => {
            Ok(WireEnvelopeBody::Request(decode_request(value?, frame_id)?))
        }
        envelope::body::Which::Response(value) => {
            Ok(WireEnvelopeBody::Response(decode_response(value?)?))
        }
        envelope::body::Which::Event(value) => Ok(WireEnvelopeBody::Event(decode_event(value?)?)),
        envelope::body::Which::ProtocolError(value) => Ok(WireEnvelopeBody::ProtocolError(
            decode_protocol_error(value?)?,
        )),
        envelope::body::Which::PatchBatch(value) => {
            Ok(WireEnvelopeBody::PatchBatch(decode_patch_batch(value?)?))
        }
    }
}

fn decode_hello(value: artisan_capnp::hello::Reader<'_>) -> Result<Hello, ProtocolDecodeError> {
    let versions = value.get_supported_versions()?;
    let version_count = versions.len() as usize;
    if version_count > crate::HELLO_VERSION_MAX_ENTRIES {
        return Err(VersionOfferError::TooMany {
            count: version_count,
            maximum: crate::HELLO_VERSION_MAX_ENTRIES,
        }
        .into());
    }
    let supported_versions = VersionOffer::new(versions.iter().collect())?;
    let credential = match value.get_credential().which()? {
        artisan_capnp::hello::credential::Which::Initial(capability) => {
            HelloCredential::Initial(LocalCapability::try_from_slice(capability?)?)
        }
        artisan_capnp::hello::credential::Which::Reconnect(capability) => {
            HelloCredential::Reconnect(ReconnectCapability::try_from_slice(capability?)?)
        }
    };
    Ok(Hello {
        supported_versions,
        credential,
        supports_lifecycle_control: value.get_supports_lifecycle_control(),
    })
}

fn decode_welcome(
    value: artisan_capnp::welcome::Reader<'_>,
) -> Result<Welcome, ProtocolDecodeError> {
    Ok(Welcome {
        negotiated_version: ProtocolVersion::new(value.get_negotiated_version())?,
        connection_id: ConnectionId::parse(read_text(
            value.get_connection_id(),
            "welcome.connectionId",
        )?)?,
        reconnect_capability: ReconnectCapability::try_from_slice(
            value.get_reconnect_capability()?,
        )?,
        lifecycle_control_supported: value.get_lifecycle_control_supported(),
    })
}

fn decode_request(
    value: artisan_capnp::request::Reader<'_>,
    frame_id: &FrameId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let request_id =
        frame_id
            .to_request_id()
            .map_err(|source| ProtocolDecodeError::Identifier {
                field: "envelope.messageId",
                source,
            })?;
    match value.which()? {
        request::Which::ListDirectories(query) => {
            let parent = match query?.get_scope().which()? {
                list_directories_request::scope::Which::NoParent(()) => None,
                list_directories_request::scope::Which::Parent(value) => Some(parse_directory_id(
                    read_text(value, "request.listDirectories.parent")?,
                    "request.listDirectories.parent",
                )?),
            };
            Ok(ClientRequest::Query(Query::ListDirectories(
                ListDirectories { parent },
            )))
        }
        request::Which::AttachProject(command) => {
            let command = command?;
            Ok(ClientRequest::Command(Command::AttachProject(
                AttachProject {
                    request_id,
                    directory_id: parse_directory_id(
                        read_text(
                            command.get_directory_id(),
                            "request.attachProject.directoryId",
                        )?,
                        "request.attachProject.directoryId",
                    )?,
                },
            )))
        }
        request::Which::ListProjectThreads(query) => {
            let query = query?;
            Ok(ClientRequest::Query(Query::ListProjectThreads(
                ListProjectThreads {
                    project_id: parse_project_id(
                        read_text(
                            query.get_project_id(),
                            "request.listProjectThreads.projectId",
                        )?,
                        "request.listProjectThreads.projectId",
                    )?,
                },
            )))
        }
        request::Which::ListAttachedProjects(query) => {
            query?;
            Ok(ClientRequest::Query(Query::ListAttachedProjects(
                ListAttachedProjects,
            )))
        }
        request::Which::CreateProjectThread(command) => {
            let command = command?;
            Ok(ClientRequest::Command(Command::CreateThread(
                CreateThread {
                    request_id,
                    project_id: parse_project_id(
                        read_text(
                            command.get_project_id(),
                            "request.createProjectThread.projectId",
                        )?,
                        "request.createProjectThread.projectId",
                    )?,
                    title: ThreadTitle::parse(read_text(
                        command.get_title(),
                        "request.createProjectThread.title",
                    )?)
                    .map_err(|source| ProtocolDecodeError::ThreadTitle { source })?,
                },
            )))
        }
        request::Which::QueueFirstMessage(command) => {
            decode_queue_first_message(command?, request_id)
        }
        request::Which::QueueMessage(command) => decode_queue_message(command?, request_id),
        request::Which::StopRun(command) => decode_stop_run(command?, request_id),
        request::Which::RespondApproval(command) => decode_respond_approval(command?, request_id),
        request::Which::RespondQuestion(command) => decode_respond_question(command?, request_id),
        request::Which::SetThreadEngineConfig(command) => {
            decode_set_thread_engine_config(command?, request_id)
        }
        request::Which::ConversationQuery(query) => decode_conversation_query_request(query?),
        request::Which::ConversationSubscribe(subscribe) => {
            decode_conversation_subscribe_request(subscribe?)
        }
        request::Which::ConversationUnsubscribe(unsubscribe) => {
            decode_conversation_unsubscribe_request(unsubscribe?)
        }
        request::Which::PickDirectory(()) => Ok(ClientRequest::PickDirectory),
        request::Which::LifecycleControl(lifecycle) => decode_lifecycle_request(lifecycle?),
        request::Which::ReadThreadEngineSettings(query) => {
            decode_read_thread_engine_settings(query?)
        }
        request::Which::ListRegisteredEngineProfiles(query) => {
            query?;
            Ok(ClientRequest::Query(Query::ListRegisteredEngineProfiles(
                artisan_domain::commands::ListRegisteredEngineProfiles,
            )))
        }
        request::Which::ReadMessageImage(query) => {
            let query = query?;
            Ok(ClientRequest::Query(Query::ReadMessageImage(
                artisan_domain::ReadMessageImage::new(
                    parse_thread_id(
                        read_text(query.get_thread_id(), "request.readMessageImage.threadId")?,
                        "request.readMessageImage.threadId",
                    )?,
                    parse_message_id(
                        read_text(query.get_message_id(), "request.readMessageImage.messageId")?,
                        "request.readMessageImage.messageId",
                    )?,
                    query.get_index(),
                ),
            )))
        }
        request::Which::ReadActiveRun(query) => {
            let query = query?;
            Ok(ClientRequest::Query(Query::ReadActiveRun(
                ReadActiveRun::new(parse_thread_id(
                    read_text(query.get_thread_id(), "request.readActiveRun.threadId")?,
                    "request.readActiveRun.threadId",
                )?),
            )))
        }
        request::Which::ReadComposerCatalog(query) => {
            let query = query?;
            Ok(ClientRequest::Query(Query::ReadComposerCatalog(
                ReadComposerCatalog::new(
                    parse_thread_id(
                        read_text(
                            query.get_thread_id(),
                            "request.readComposerCatalog.threadId",
                        )?,
                        "request.readComposerCatalog.threadId",
                    )?,
                    parse_profile_id(
                        read_text(
                            query.get_profile_id(),
                            "request.readComposerCatalog.profileId",
                        )?,
                        "request.readComposerCatalog.profileId",
                    )?,
                ),
            )))
        }
        request::Which::ListQueuedMessages(value) => {
            Ok(ClientRequest::Query(Query::ListQueuedMessages(
                crate::composer_state_codec::decode_list_queued_messages_request(value?)?,
            )))
        }
        request::Which::WithdrawQueuedMessage(value) => {
            Ok(ClientRequest::Command(Command::WithdrawQueuedMessage(
                crate::composer_state_codec::decode_withdraw_queued_message_request(
                    value?, request_id,
                )?,
            )))
        }
        request::Which::ReadRecalledMessage(value) => {
            Ok(ClientRequest::Query(Query::ReadRecalledMessage(
                crate::composer_state_codec::decode_read_recalled_message_request(value?)?,
            )))
        }
        request::Which::ReadRunUsage(value) => Ok(ClientRequest::Query(Query::ReadRunUsage(
            crate::composer_state_codec::decode_read_run_usage_request(value?)?,
        ))),
        request::Which::ListFailedMessages(value) => {
            Ok(ClientRequest::Query(Query::ListFailedMessages(
                crate::composer_state_codec::decode_list_failed_messages_request(value?)?,
            )))
        }
        request::Which::ReadAccountUsage(value) => decode_read_account_usage(value?),
        request::Which::ResolveRichLink(query) => {
            let query = query?;
            Ok(ClientRequest::ResolveRichLink(ResolveRichLinkRequest::new(
                read_text(query.get_url(), "request.resolveRichLink.url")?,
            )?))
        }
        request::Which::QueryProjectRepository(query) => Ok(
            ClientRequest::QueryProjectRepository(decode_project_repository_query(query?)?),
        ),
        request::Which::ReadModelFavorites(()) => Ok(ClientRequest::Query(
            Query::ReadModelFavorites(ReadModelFavorites),
        )),
        request::Which::SetModelFavorite(command) => {
            let command = command?;
            let catalog_revision = CatalogRevision::parse(read_text(
                command.get_catalog_revision(),
                "request.setModelFavorite.catalogRevision",
            )?)?;
            let model_id = ModelFavoriteId::parse(read_text(
                command.get_model_id(),
                "request.setModelFavorite.modelId",
            )?)?;
            Ok(ClientRequest::Command(Command::SetModelFavorite(
                SetModelFavorite::new(
                    request_id,
                    parse_thread_id(
                        read_text(command.get_thread_id(), "request.setModelFavorite.threadId")?,
                        "request.setModelFavorite.threadId",
                    )?,
                    parse_profile_id(
                        read_text(
                            command.get_profile_id(),
                            "request.setModelFavorite.profileId",
                        )?,
                        "request.setModelFavorite.profileId",
                    )?,
                    catalog_revision,
                    model_id,
                    command.get_favorite(),
                ),
            )))
        }
    }
}

fn decode_lifecycle_request(
    value: artisan_capnp::lifecycle_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let request = match value.which()? {
        lifecycle_request::Which::Status(status) => {
            status?;
            LifecycleRequest::Status
        }
        lifecycle_request::Which::Stop(stop) => LifecycleRequest::Stop {
            require_idle: stop?.get_require_idle(),
        },
    };
    Ok(ClientRequest::Lifecycle(request))
}

fn decode_queue_first_message(
    command: artisan_capnp::queue_first_message_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Command(Command::QueueFirstMessage(
        QueueFirstMessage {
            request_id,
            thread_id: parse_thread_id(
                read_text(
                    command.get_thread_id(),
                    "request.queueFirstMessage.threadId",
                )?,
                "request.queueFirstMessage.threadId",
            )?,
            body: MessageBody::parse(read_text(
                command.get_body(),
                "request.queueFirstMessage.body",
            )?)
            .map_err(|source| ProtocolDecodeError::MessageBody { source })?,
        },
    )))
}

fn decode_queue_message(
    command: artisan_capnp::queue_message_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(command.get_thread_id(), "request.queueMessage.threadId")?,
        "request.queueMessage.threadId",
    )?;
    let text = match command.get_text().which()? {
        artisan_capnp::queue_message_request::text::Which::Absent(()) => None,
        artisan_capnp::queue_message_request::text::Which::Present(value) => Some(
            AuthoredText::parse(read_text(value, "request.queueMessage.text")?).map_err(
                |source| ProtocolDecodeError::MessagePayload {
                    source: QueueMessagePayloadError::Text(source),
                },
            )?,
        ),
    };
    let attachments = decode_image_attachments(
        command.get_attachments()?,
        "request.queueMessage.attachments.mimeType",
        "request.queueMessage.attachments.name",
        "request.queueMessage.attachments.bytes",
        "request.queueMessage.attachments",
    )?;
    let payload = QueueMessagePayload::new(text, attachments)?;
    let command = match read_text(
        command.get_steer_run_id(),
        "request.queueMessage.steerRunId",
    )? {
        steer_run_id if steer_run_id.is_empty() => {
            QueueMessage::new(request_id, thread_id, payload)
        }
        steer_run_id => {
            let run_id = parse_run_id(
                steer_run_id,
                "request.queueMessage.steerRunId",
            )?;
            QueueMessage::new(request_id, thread_id, payload)
                .with_steer_target(SteerTarget::new(run_id))
        }
    };
    Ok(ClientRequest::Command(Command::QueueMessage(command)))
}

fn decode_stop_run(
    command: artisan_capnp::stop_run_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Command(Command::StopRun(StopRun::new(
        request_id,
        parse_thread_id(
            read_text(command.get_thread_id(), "request.stopRun.threadId")?,
            "request.stopRun.threadId",
        )?,
        parse_run_id(
            read_text(command.get_run_id(), "request.stopRun.runId")?,
            "request.stopRun.runId",
        )?,
    ))))
}

fn decode_respond_approval(
    command: artisan_capnp::respond_approval_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Command(Command::RespondApproval(
        RespondApproval::new(
            request_id,
            parse_thread_id(
                read_text(command.get_thread_id(), "request.respondApproval.threadId")?,
                "request.respondApproval.threadId",
            )?,
            parse_run_id(
                read_text(command.get_run_id(), "request.respondApproval.runId")?,
                "request.respondApproval.runId",
            )?,
            parse_observation_id(
                read_text(
                    command.get_approval_id(),
                    "request.respondApproval.approvalId",
                )?,
                "request.respondApproval.approvalId",
            )?,
            command.get_approved(),
        ),
    )))
}

fn decode_answer_list(
    encoded: capnp::text_list::Reader<'_>,
    field: &'static str,
) -> Result<Vec<String>, ProtocolDecodeError> {
    let count = encoded.len() as usize;
    if count > OBSERVATION_ANSWERS_MAX {
        return Err(ProtocolDecodeError::RunInteraction {
            source: RunInteractionError::TooManyAnswers {
                count,
                maximum: OBSERVATION_ANSWERS_MAX,
            },
        });
    }
    let mut answers = Vec::with_capacity(count);
    for answer in encoded.iter() {
        answers.push(read_text(answer, field)?);
    }
    Ok(answers)
}

fn decode_respond_question(
    command: artisan_capnp::respond_question_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(command.get_thread_id(), "request.respondQuestion.threadId")?,
        "request.respondQuestion.threadId",
    )?;
    let run_id = parse_run_id(
        read_text(command.get_run_id(), "request.respondQuestion.runId")?,
        "request.respondQuestion.runId",
    )?;
    let question_id = parse_observation_id(
        read_text(
            command.get_question_id(),
            "request.respondQuestion.questionId",
        )?,
        "request.respondQuestion.questionId",
    )?;
    let answers = decode_answer_list(command.get_answers()?, "request.respondQuestion.answers")?;
    RespondQuestion::new(request_id, thread_id, run_id, question_id, answers)
        .map(Command::RespondQuestion)
        .map(ClientRequest::Command)
        .map_err(|source| ProtocolDecodeError::RunInteraction { source })
}

const fn encode_run_interaction_outcome(
    value: RunInteractionOutcome,
) -> artisan_capnp::RespondInteractionOutcome {
    match value {
        RunInteractionOutcome::Applied => artisan_capnp::RespondInteractionOutcome::Applied,
        RunInteractionOutcome::UnknownTarget => {
            artisan_capnp::RespondInteractionOutcome::UnknownTarget
        }
        RunInteractionOutcome::AlreadyResolved => {
            artisan_capnp::RespondInteractionOutcome::AlreadyResolved
        }
        RunInteractionOutcome::WrongRun => artisan_capnp::RespondInteractionOutcome::WrongRun,
    }
}

const fn decode_run_interaction_outcome(
    value: artisan_capnp::RespondInteractionOutcome,
) -> RunInteractionOutcome {
    match value {
        artisan_capnp::RespondInteractionOutcome::Applied => RunInteractionOutcome::Applied,
        artisan_capnp::RespondInteractionOutcome::UnknownTarget => {
            RunInteractionOutcome::UnknownTarget
        }
        artisan_capnp::RespondInteractionOutcome::AlreadyResolved => {
            RunInteractionOutcome::AlreadyResolved
        }
        artisan_capnp::RespondInteractionOutcome::WrongRun => RunInteractionOutcome::WrongRun,
    }
}

fn encode_respond_approval_receipt(
    mut receipt: artisan_capnp::respond_approval_receipt::Builder<'_>,
    value: &RespondApprovalReceipt,
) -> Result<(), ProtocolEncodeError> {
    receipt.set_request_id(value.request_id.as_str());
    receipt.set_thread_id(value.thread_id.as_str());
    receipt.set_run_id(value.run_id.as_str());
    receipt.set_approval_id(value.approval_id.as_str());
    receipt.set_approved(value.approved);
    receipt.set_outcome(encode_run_interaction_outcome(value.outcome));
    receipt.set_disposition(encode_disposition(value.disposition));
    Ok(())
}

fn encode_respond_question_receipt(
    mut receipt: artisan_capnp::respond_question_receipt::Builder<'_>,
    value: &RespondQuestionReceipt,
) -> Result<(), ProtocolEncodeError> {
    // Stored answers were validated when the response resolved; rebuilding
    // the domain command keeps encode total for hand-built receipts too.
    let command = RespondQuestion::new(
        value.request_id.clone(),
        value.thread_id.clone(),
        value.run_id.clone(),
        value.question_id.clone(),
        value.answers.clone(),
    )
    .map_err(|source| ProtocolEncodeError::RunInteraction { source })?;
    receipt.set_request_id(value.request_id.as_str());
    receipt.set_thread_id(value.thread_id.as_str());
    receipt.set_run_id(value.run_id.as_str());
    receipt.set_question_id(value.question_id.as_str());
    let mut answers = receipt.reborrow().init_answers(list_length(
        "response.questionResponse.answers",
        command.answers().len(),
    )?);
    for (index, answer) in command.answers().iter().enumerate() {
        answers.set(
            list_index("response.questionResponse.answers", index)?,
            answer.as_str(),
        );
    }
    receipt.set_outcome(encode_run_interaction_outcome(value.outcome));
    receipt.set_disposition(encode_disposition(value.disposition));
    Ok(())
}

fn decode_image_attachments(
    encoded_attachments: capnp::struct_list::Reader<'_, artisan_capnp::image_attachment::Owned>,
    mime_field: &'static str,
    name_field: &'static str,
    bytes_field: &'static str,
    attachment_field: &'static str,
) -> Result<Vec<ImageAttachment>, ProtocolDecodeError> {
    let count = encoded_attachments.len() as usize;
    if count > MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
        return Err(ProtocolDecodeError::MessagePayload {
            source: QueueMessagePayloadError::TooManyAttachments {
                count,
                maximum: MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
            },
        });
    }
    let mut attachments = Vec::with_capacity(count);
    for encoded in encoded_attachments.iter() {
        let bytes = encoded.get_bytes()?;
        if bytes.len() > MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES {
            return Err(ProtocolDecodeError::ImageAttachment {
                field: bytes_field,
                source: ImageAttachmentError::BytesTooLarge {
                    length: bytes.len(),
                    maximum: MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES,
                },
            });
        }
        let mime_type = read_text(encoded.get_mime_type(), mime_field)?;
        let name = read_text(encoded.get_name(), name_field)?;
        attachments.push(
            ImageAttachment::new(mime_type, bytes.to_vec(), name).map_err(|source| {
                ProtocolDecodeError::ImageAttachment {
                    field: attachment_field,
                    source,
                }
            })?,
        );
    }
    Ok(attachments)
}

fn decode_image_attachment_refs(
    encoded_refs: capnp::struct_list::Reader<'_, artisan_capnp::image_attachment_ref::Owned>,
    field: &'static str,
) -> Result<Vec<ImageAttachmentRef>, ProtocolDecodeError> {
    let count = encoded_refs.len() as usize;
    if count > MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
        return Err(ProtocolDecodeError::MessagePayload {
            source: QueueMessagePayloadError::TooManyAttachments {
                count,
                maximum: MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
            },
        });
    }
    let mut refs = Vec::with_capacity(count);
    for (expected_index, encoded) in encoded_refs.iter().enumerate() {
        let expected_index = u32::try_from(expected_index).map_err(|_| {
            ProtocolDecodeError::ImageAttachmentReference {
                field,
                reason: "attachment index overflow",
            }
        })?;
        refs.push(decode_image_attachment_ref(
            encoded,
            field,
            Some(expected_index),
        )?);
    }
    Ok(refs)
}

fn decode_image_attachment_ref(
    encoded: artisan_capnp::image_attachment_ref::Reader<'_>,
    field: &'static str,
    expected_index: Option<u32>,
) -> Result<ImageAttachmentRef, ProtocolDecodeError> {
    let index = encoded.get_index();
    if expected_index.is_some_and(|expected| expected != index) {
        return Err(ProtocolDecodeError::ImageAttachmentReference {
            field,
            reason: "attachment indexes are not ordered",
        });
    }
    let digest: [u8; 32] = encoded.get_digest()?.to_vec().try_into().map_err(|_| {
        ProtocolDecodeError::ImageAttachmentReference {
            field,
            reason: "digest must be exactly 32 bytes",
        }
    })?;
    ImageAttachmentRef::new(
        parse_message_id(
            read_text(encoded.get_message_id(), "imageAttachmentRef.messageId")?,
            "imageAttachmentRef.messageId",
        )?,
        parse_thread_id(
            read_text(encoded.get_thread_id(), "imageAttachmentRef.threadId")?,
            "imageAttachmentRef.threadId",
        )?,
        index,
        read_text(encoded.get_mime_type(), "imageAttachmentRef.mimeType")?,
        read_text(encoded.get_name(), "imageAttachmentRef.name")?,
        encoded.get_size_bytes(),
        digest,
    )
    .map_err(|source| ProtocolDecodeError::ImageAttachmentReference {
        field,
        reason: match source {
            ImageAttachmentRefError::UnsupportedMimeType => "unsupported MIME type",
            ImageAttachmentRefError::InvalidSize { .. } => "invalid image size",
            ImageAttachmentRefError::InvalidName => "invalid filename",
        },
    })
}

fn decode_set_thread_engine_config(
    command: set_thread_engine_config_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(
            command.get_thread_id(),
            "request.setThreadEngineConfig.threadId",
        )?,
        "request.setThreadEngineConfig.threadId",
    )?;
    let precondition = decode_engine_config_precondition(command.get_precondition()?)?;
    let config = decode_engine_run_config(command.get_config()?)?;
    Ok(ClientRequest::Command(Command::SetThreadEngineConfig(
        Box::new(SetThreadEngineConfig::new(
            request_id,
            thread_id,
            precondition,
            config,
        )),
    )))
}

fn decode_read_thread_engine_settings(
    query: artisan_capnp::read_thread_engine_settings_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(
            query.get_thread_id(),
            "request.readThreadEngineSettings.threadId",
        )?,
        "request.readThreadEngineSettings.threadId",
    )?;
    Ok(ClientRequest::Query(
        artisan_domain::Query::ReadThreadEngineSettings(
            artisan_domain::commands::ReadThreadEngineSettings::new(thread_id),
        ),
    ))
}

fn engine_config_error(field: &'static str, reason: EngineConfigReason) -> ProtocolDecodeError {
    ProtocolDecodeError::EngineConfig {
        source: EngineConfigError::new(field, reason),
    }
}

fn decode_engine_config_precondition(
    value: artisan_capnp::engine_config_precondition::Reader<'_>,
) -> Result<EngineConfigUpdatePrecondition, ProtocolDecodeError> {
    let kind = read_text(
        value.get_kind(),
        "request.setThreadEngineConfig.precondition.kind",
    )?;
    match kind.as_str() {
        "unconfigured" if value.get_revision() == 0 => {
            Ok(EngineConfigUpdatePrecondition::Unconfigured)
        }
        "exact_revision" => Ok(EngineConfigUpdatePrecondition::Exact(
            EngineConfigRevision::new(value.get_revision())
                .map_err(|error| ProtocolDecodeError::EngineConfig { source: error })?,
        )),
        "unconfigured" => Err(engine_config_error(
            "request.setThreadEngineConfig.precondition.revision",
            EngineConfigReason::Inconsistent,
        )),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.precondition.kind",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn decode_engine_run_config(
    value: artisan_capnp::engine_run_config::Reader<'_>,
) -> Result<EngineRunConfig, ProtocolDecodeError> {
    match value.get_schema_version() {
        1 => decode_engine_run_config_v1(value),
        2 => decode_engine_run_config_v2(value),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.schemaVersion",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn decode_engine_run_config_v1(
    value: artisan_capnp::engine_run_config::Reader<'_>,
) -> Result<EngineRunConfig, ProtocolDecodeError> {
    if !matches!(
        value.get_selection_v2()?.which()?,
        engine_selection_v2::Which::Unset(())
    ) {
        return Err(engine_config_error(
            "request.setThreadEngineConfig.config.selectionV2",
            EngineConfigReason::Inconsistent,
        ));
    }
    let engine = read_text(
        value.get_engine(),
        "request.setThreadEngineConfig.config.engine",
    )?;
    if engine != EngineId::OpenCode2.as_str() {
        return Err(engine_config_error(
            "request.setThreadEngineConfig.config.engine",
            EngineConfigReason::Unsupported,
        ));
    }
    let profile_id = EngineProfileId::parse(read_text(
        value.get_profile_id(),
        "request.setThreadEngineConfig.config.profileId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.profileId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let model_id = EngineModelId::parse(read_text(
        value.get_model_id(),
        "request.setThreadEngineConfig.config.modelId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.modelId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let route_id = EngineRouteId::parse(read_text(
        value.get_route_id(),
        "request.setThreadEngineConfig.config.routeId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.routeId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let variant = decode_engine_variant(value.get_variant()?)?;
    let permission = decode_engine_permission(value.get_permission()?)?;
    let runtime = decode_engine_runtime(value.get_runtime()?)?;
    Ok(EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            profile_id, model_id, route_id, variant, permission,
        )),
        runtime,
    ))
}

fn decode_engine_run_config_v2(
    value: artisan_capnp::engine_run_config::Reader<'_>,
) -> Result<EngineRunConfig, ProtocolDecodeError> {
    let engine = read_text(
        value.get_engine(),
        "request.setThreadEngineConfig.config.engine",
    )?;
    let engine = EngineId::parse(&engine).map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.engine",
            EngineConfigReason::Unsupported,
        )
    })?;
    // The legacy profile field stays populated as a diagnostic mirror; it
    // must agree with the authority arm below.
    let legacy_profile_id = read_text(
        value.get_profile_id(),
        "request.setThreadEngineConfig.config.profileId",
    )?;
    let runtime = decode_engine_runtime(value.get_runtime()?)?;
    let selection = match value.get_selection_v2()?.which()? {
        engine_selection_v2::Which::Codex(arm) => {
            if engine != EngineId::Codex {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Codex(decode_codex_selection(arm?)?)
        }
        engine_selection_v2::Which::Claude(arm) => {
            if engine != EngineId::Claude {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Claude(decode_claude_selection(arm?)?)
        }
        engine_selection_v2::Which::Grok(arm) => {
            if engine != EngineId::Grok {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Grok(decode_grok_selection(arm?)?)
        }
        engine_selection_v2::Which::Cursor(arm) => {
            if engine != EngineId::Cursor {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Cursor(decode_cursor_selection(arm?)?)
        }
        engine_selection_v2::Which::Hermes(arm) => {
            if engine != EngineId::Hermes {
                return Err(engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2",
                    EngineConfigReason::Inconsistent,
                ));
            }
            EngineSelection::Hermes(decode_hermes_selection(arm?)?)
        }
        engine_selection_v2::Which::Unset(()) => {
            return Err(engine_config_error(
                "request.setThreadEngineConfig.config.selectionV2",
                EngineConfigReason::Inconsistent,
            ));
        }
    };
    if selection.profile_id().as_str() != legacy_profile_id {
        return Err(engine_config_error(
            "request.setThreadEngineConfig.config.profileId",
            EngineConfigReason::Inconsistent,
        ));
    }
    Ok(EngineRunConfig::new(selection, runtime))
}

fn parse_optional_model_id(
    value: String,
    field: &'static str,
) -> Result<Option<EngineModelId>, ProtocolDecodeError> {
    if value.is_empty() {
        Ok(None)
    } else {
        EngineModelId::parse(value)
            .map(Some)
            .map_err(|_| engine_config_error(field, EngineConfigReason::InvalidIdentifier))
    }
}

fn parse_optional_setting<T>(
    value: String,
    field: &'static str,
    parse: impl FnOnce(&str) -> Result<T, EngineConfigError>,
) -> Result<Option<T>, ProtocolDecodeError> {
    if value.is_empty() {
        Ok(None)
    } else {
        parse(&value)
            .map(Some)
            .map_err(|error| engine_config_error(field, error.reason()))
    }
}

fn parse_required_model_id(
    value: String,
    field: &'static str,
) -> Result<EngineModelId, ProtocolDecodeError> {
    if value.is_empty() {
        return Err(engine_config_error(
            field,
            EngineConfigReason::InvalidIdentifier,
        ));
    }
    EngineModelId::parse(value)
        .map_err(|_| engine_config_error(field, EngineConfigReason::InvalidIdentifier))
}

fn parse_required_route_id(
    value: String,
    field: &'static str,
) -> Result<EngineRouteId, ProtocolDecodeError> {
    if value.is_empty() {
        return Err(engine_config_error(
            field,
            EngineConfigReason::InvalidIdentifier,
        ));
    }
    EngineRouteId::parse(value)
        .map_err(|_| engine_config_error(field, EngineConfigReason::InvalidIdentifier))
}

fn decode_codex_selection(
    arm: artisan_capnp::codex_engine_selection::Reader<'_>,
) -> Result<CodexSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_optional_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let reasoning_effort = parse_optional_setting(
        read_text(
            arm.get_reasoning_effort(),
            "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        CodexReasoningEffort::parse,
    )?;
    let service_tier = parse_optional_setting(
        read_text(
            arm.get_service_tier(),
            "request.setThreadEngineConfig.config.selectionV2.serviceTier",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.serviceTier",
        CodexServiceTier::parse,
    )?;
    let model_context_window = {
        let window = arm.get_model_context_window();
        if window == 0 {
            None
        } else {
            Some(CodexModelContextWindow::new(window).map_err(|error| {
                engine_config_error(
                    "request.setThreadEngineConfig.config.selectionV2.modelContextWindow",
                    error.reason(),
                )
            })?)
        }
    };
    let permission = decode_engine_permission(arm.get_permission()?)?;
    Ok(CodexSelection::new(
        profile_id,
        model_id,
        permission,
        reasoning_effort,
        service_tier,
        model_context_window,
    )?)
}

fn decode_claude_selection(
    arm: artisan_capnp::claude_engine_selection::Reader<'_>,
) -> Result<ClaudeSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_optional_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let effort = parse_optional_setting(
        read_text(
            arm.get_effort(),
            "request.setThreadEngineConfig.config.selectionV2.effort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.effort",
        ClaudeEffort::parse,
    )?;
    let permission_mode = parse_optional_setting(
        read_text(
            arm.get_permission_mode(),
            "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        ClaudePermissionMode::parse,
    )?;
    let permission = decode_engine_permission(arm.get_permission()?)?;
    Ok(ClaudeSelection::new(
        profile_id,
        model_id,
        permission,
        effort,
        permission_mode,
        arm.get_disable_tools(),
        arm.get_safe_mode(),
    )?)
}

fn decode_grok_selection(
    arm: artisan_capnp::grok_engine_selection::Reader<'_>,
) -> Result<GrokSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_optional_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let reasoning_effort = parse_optional_setting(
        read_text(
            arm.get_reasoning_effort(),
            "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        |text| GrokReasoningEffort::parse(text),
    )?;
    let permission_mode = parse_optional_setting(
        read_text(
            arm.get_permission_mode(),
            "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        GrokPermissionMode::parse,
    )?;
    let permission = decode_engine_permission(arm.get_permission()?)?;
    Ok(GrokSelection::new(
        profile_id,
        model_id,
        permission,
        reasoning_effort,
        permission_mode,
    ))
}

fn decode_cursor_selection(
    arm: artisan_capnp::cursor_engine_selection::Reader<'_>,
) -> Result<CursorSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_optional_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let reasoning_effort = parse_optional_setting(
        read_text(
            arm.get_reasoning_effort(),
            "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        |text| CursorReasoningEffort::parse(text),
    )?;
    let speed = parse_optional_setting(
        read_text(
            arm.get_speed(),
            "request.setThreadEngineConfig.config.selectionV2.speed",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.speed",
        CursorSpeed::parse,
    )?;
    let permission_mode = parse_optional_setting(
        read_text(
            arm.get_permission_mode(),
            "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.permissionMode",
        CursorPermissionMode::parse,
    )?;
    let permission = decode_engine_permission(arm.get_permission()?)?;
    Ok(CursorSelection::new(
        profile_id,
        model_id,
        permission,
        reasoning_effort,
        speed,
        permission_mode,
    ))
}

fn decode_hermes_selection(
    arm: artisan_capnp::hermes_engine_selection::Reader<'_>,
) -> Result<HermesSelection, ProtocolDecodeError> {
    let profile_id = parse_profile_id(
        read_text(
            arm.get_profile_id(),
            "request.setThreadEngineConfig.config.selectionV2.profileId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.profileId",
    )?;
    let model_id = parse_required_model_id(
        read_text(
            arm.get_model_id(),
            "request.setThreadEngineConfig.config.selectionV2.modelId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.modelId",
    )?;
    let route_id = parse_required_route_id(
        read_text(
            arm.get_route_id(),
            "request.setThreadEngineConfig.config.selectionV2.routeId",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.routeId",
    )?;
    let reasoning_effort = parse_optional_setting(
        read_text(
            arm.get_reasoning_effort(),
            "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        )?,
        "request.setThreadEngineConfig.config.selectionV2.reasoningEffort",
        |text| HermesReasoningEffort::parse(text),
    )?;
    let permission_mode = HermesPermissionMode::parse(&read_text(
        arm.get_permission_mode(),
        "request.setThreadEngineConfig.config.selectionV2.permissionMode",
    )?)
    .map_err(|error| {
        engine_config_error(
            "request.setThreadEngineConfig.config.selectionV2.permissionMode",
            error.reason(),
        )
    })?;
    Ok(HermesSelection::new(
        profile_id,
        model_id,
        route_id,
        permission_mode,
        reasoning_effort,
        arm.get_fast(),
    ))
}

fn decode_engine_variant(
    value: artisan_capnp::engine_variant::Reader<'_>,
) -> Result<Option<EngineVariantId>, ProtocolDecodeError> {
    let kind = read_text(
        value.get_kind(),
        "request.setThreadEngineConfig.config.variant.kind",
    )?;
    let id = read_text(
        value.get_id(),
        "request.setThreadEngineConfig.config.variant.id",
    )?;
    match kind.as_str() {
        "none" if id.is_empty() => Ok(None),
        "none" => Err(engine_config_error(
            "request.setThreadEngineConfig.config.variant.id",
            EngineConfigReason::Inconsistent,
        )),
        "selected" => EngineVariantId::parse(id).map(Some).map_err(|_| {
            engine_config_error(
                "request.setThreadEngineConfig.config.variant.id",
                EngineConfigReason::InvalidIdentifier,
            )
        }),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.variant.kind",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn decode_engine_permission(
    value: artisan_capnp::engine_permission_policy::Reader<'_>,
) -> Result<EnginePermissionPolicy, ProtocolDecodeError> {
    let permission_id = PermissionId::parse(read_text(
        value.get_permission_id(),
        "request.setThreadEngineConfig.config.permission.permissionId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.permission.permissionId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let agent_id = EngineAgentId::parse(read_text(
        value.get_agent_id(),
        "request.setThreadEngineConfig.config.permission.agentId",
    )?)
    .map_err(|_| {
        engine_config_error(
            "request.setThreadEngineConfig.config.permission.agentId",
            EngineConfigReason::InvalidIdentifier,
        )
    })?;
    let approval = parse_approval(&read_text(
        value.get_approval(),
        "request.setThreadEngineConfig.config.permission.approval",
    )?)?;
    let filesystem = parse_filesystem(&read_text(
        value.get_filesystem(),
        "request.setThreadEngineConfig.config.permission.filesystem",
    )?)?;
    let network = parse_network(&read_text(
        value.get_network(),
        "request.setThreadEngineConfig.config.permission.network",
    )?)?;
    let web_search = parse_web_search(&read_text(
        value.get_web_search(),
        "request.setThreadEngineConfig.config.permission.webSearch",
    )?)?;
    Ok(EnginePermissionPolicy::new(
        permission_id,
        agent_id,
        approval,
        filesystem,
        network,
        web_search,
    ))
}

fn parse_approval(value: &str) -> Result<ApprovalMode, ProtocolDecodeError> {
    match value {
        "never" => Ok(ApprovalMode::Never),
        "on_request" => Ok(ApprovalMode::OnRequest),
        "always" => Ok(ApprovalMode::Always),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.permission.approval",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn parse_filesystem(value: &str) -> Result<FilesystemAccess, ProtocolDecodeError> {
    match value {
        "none" => Ok(FilesystemAccess::None),
        "workspace" => Ok(FilesystemAccess::Workspace),
        "host" => Ok(FilesystemAccess::Host),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.permission.filesystem",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn parse_network(value: &str) -> Result<NetworkAccess, ProtocolDecodeError> {
    match value {
        "disabled" => Ok(NetworkAccess::Disabled),
        "enabled" => Ok(NetworkAccess::Enabled),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.permission.network",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn parse_web_search(value: &str) -> Result<WebSearchAccess, ProtocolDecodeError> {
    match value {
        "disabled" => Ok(WebSearchAccess::Disabled),
        "enabled" => Ok(WebSearchAccess::Enabled),
        _ => Err(engine_config_error(
            "request.setThreadEngineConfig.config.permission.webSearch",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn decode_engine_runtime(
    value: artisan_capnp::engine_runtime_controls::Reader<'_>,
) -> Result<EngineRuntimeControls, ProtocolDecodeError> {
    let millis = |value: u64, field: &'static str| {
        FiniteMillis::new(value)
            .map_err(|_| engine_config_error(field, EngineConfigReason::OutOfRange))
    };
    let bytes = |value: u64, field: &'static str| {
        ByteLimit::new(value)
            .map_err(|_| engine_config_error(field, EngineConfigReason::OutOfRange))
    };
    let count = |value: u64, field: &'static str| {
        CountLimit::new(value)
            .map_err(|_| engine_config_error(field, EngineConfigReason::OutOfRange))
    };
    Ok(EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: millis(
            value.get_attempt_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.attemptBudgetMs",
        )?,
        readiness_budget: millis(
            value.get_readiness_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.readinessBudgetMs",
        )?,
        health_budget: millis(
            value.get_health_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.healthBudgetMs",
        )?,
        prompt_budget: millis(
            value.get_prompt_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.promptBudgetMs",
        )?,
        stream_budget: millis(
            value.get_stream_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.streamBudgetMs",
        )?,
        close_budget: millis(
            value.get_close_budget_ms(),
            "request.setThreadEngineConfig.config.runtime.closeBudgetMs",
        )?,
        max_json_body_bytes: bytes(
            value.get_max_json_body_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxJsonBodyBytes",
        )?,
        max_sse_line_bytes: bytes(
            value.get_max_sse_line_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxSseLineBytes",
        )?,
        max_sse_event_bytes: bytes(
            value.get_max_sse_event_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxSseEventBytes",
        )?,
        max_readiness_line_bytes: bytes(
            value.get_max_readiness_line_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxReadinessLineBytes",
        )?,
        max_header_count: count(
            value.get_max_header_count(),
            "request.setThreadEngineConfig.config.runtime.maxHeaderCount",
        )?,
        max_http_buffer_bytes: bytes(
            value.get_max_http_buffer_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxHttpBufferBytes",
        )?,
        max_stderr_bytes: bytes(
            value.get_max_stderr_bytes(),
            "request.setThreadEngineConfig.config.runtime.maxStderrBytes",
        )?,
        observation_capacity: count(
            value.get_observation_capacity(),
            "request.setThreadEngineConfig.config.runtime.observationCapacity",
        )?,
    })?)
}

fn decode_conversation_query_request(
    query: artisan_capnp::conversation_query_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(query.get_thread_id(), "request.conversationQuery.threadId")?,
        "request.conversationQuery.threadId",
    )?;
    let bounds = match query.get_bounds().which()? {
        conversation_query_request::bounds::Which::Window(window) => {
            ConversationQueryBounds::Window {
                maximum_turn_count: QueryTurnCount::new(u64::from(
                    window?.get_maximum_turn_count(),
                ))?,
            }
        }
        conversation_query_request::bounds::Which::Range(range) => {
            let range = range?;
            let minimum_turn_ordinal = match range.get_minimum_turn_ordinal().which()? {
                query_range::minimum_turn_ordinal::Which::NoMinimum(()) => None,
                query_range::minimum_turn_ordinal::Which::Minimum(value) => {
                    Some(TurnOrdinal::new(value))
                }
            };
            ConversationQueryBounds::Range {
                before_turn_ordinal: TurnOrdinal::new(range.get_before_turn_ordinal()),
                minimum_turn_ordinal,
                maximum_turn_count: QueryTurnCount::new(u64::from(range.get_maximum_turn_count()))?,
            }
        }
    };
    Ok(ClientRequest::Conversation(ConversationRequest::Query(
        ConversationQuery { thread_id, bounds },
    )))
}

fn decode_conversation_subscribe_request(
    subscribe: artisan_capnp::conversation_subscribe_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(
            subscribe.get_thread_id(),
            "request.conversationSubscribe.threadId",
        )?,
        "request.conversationSubscribe.threadId",
    )?;
    let value = match subscribe.get_start().which()? {
        conversation_subscribe_request::start::Which::Fresh(()) => {
            ConversationSubscribe::fresh(thread_id)
        }
        conversation_subscribe_request::start::Which::ResumeAfter(cursor) => {
            ConversationSubscribe::resume(thread_id, ConversationCursor::new(cursor))
        }
    };
    Ok(ClientRequest::Conversation(ConversationRequest::Subscribe(
        value,
    )))
}

fn decode_conversation_unsubscribe_request(
    unsubscribe: artisan_capnp::conversation_unsubscribe_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Conversation(
        ConversationRequest::Unsubscribe(ConversationUnsubscribe {
            thread_id: parse_thread_id(
                read_text(
                    unsubscribe.get_thread_id(),
                    "request.conversationUnsubscribe.threadId",
                )?,
                "request.conversationUnsubscribe.threadId",
            )?,
        }),
    ))
}

fn decode_response(
    value: artisan_capnp::response::Reader<'_>,
) -> Result<ServerResponse, ProtocolDecodeError> {
    let request_id = parse_request_id(
        read_text(value.get_request_id(), "response.requestId")?,
        "response.requestId",
    )?;
    let payload = match value.which()? {
        response::Which::DirectoryList(listing) => {
            ResponsePayload::DirectoryListing(decode_directory_listing(listing?)?)
        }
        response::Which::ProjectList(listing) => {
            let projects = listing?.get_projects()?;
            let project_count = projects.len() as usize;
            if project_count > PROJECT_LISTING_MAX_PROJECTS {
                return Err(ProtocolDecodeError::ProjectListing {
                    source: ProjectListingError::TooManyProjects {
                        count: project_count,
                        maximum: PROJECT_LISTING_MAX_PROJECTS,
                    },
                });
            }
            let decoded = projects
                .iter()
                .map(decode_project)
                .collect::<Result<Vec<_>, _>>()?;
            ResponsePayload::ProjectListing(
                ProjectListing::new(decoded)
                    .map_err(|source| ProtocolDecodeError::ProjectListing { source })?,
            )
        }
        response::Which::AttachedProject(result) => {
            let result = result?;
            ResponsePayload::AttachedProject {
                project: decode_project(result.get_project()?)?,
                disposition: decode_disposition(result.get_disposition()?),
            }
        }
        response::Which::ThreadList(listing) => {
            let threads = listing?.get_threads()?;
            let thread_count = threads.len() as usize;
            if thread_count > THREAD_LISTING_MAX_THREADS {
                return Err(ProtocolDecodeError::ThreadListing {
                    source: ThreadListingError::TooManyThreads {
                        count: thread_count,
                        maximum: THREAD_LISTING_MAX_THREADS,
                    },
                });
            }
            let decoded = threads
                .iter()
                .map(decode_thread)
                .collect::<Result<Vec<_>, _>>()?;
            ResponsePayload::ThreadListing(
                ThreadListing::new(decoded)
                    .map_err(|source| ProtocolDecodeError::ThreadListing { source })?,
            )
        }
        response::Which::CreatedThread(result) => {
            let result = result?;
            ResponsePayload::CreatedThread {
                thread: decode_thread(result.get_thread()?)?,
                disposition: decode_disposition(result.get_disposition()?),
            }
        }
        response::Which::QueuedReceipt(receipt) => decode_queued_receipt(receipt?, &request_id)?,
        response::Which::QueuedMessageReceipt(receipt) => {
            decode_queue_message_receipt(receipt?, &request_id)?
        }
        response::Which::MessageImage(result) => decode_message_image(result?)?,
        response::Which::StopRunReceipt(receipt) => decode_stop_run_receipt(receipt?, &request_id)?,
        response::Which::ApprovalResponse(receipt) => {
            decode_respond_approval_receipt(receipt?, &request_id)?
        }
        response::Which::QuestionResponse(receipt) => {
            decode_respond_question_receipt(receipt?, &request_id)?
        }
        response::Which::ActiveRun(result) => decode_active_run_result(result?)?,
        response::Which::QueuedMessages(value) => ResponsePayload::QueuedMessages(
            crate::composer_state_codec::decode_queued_message_listing(value?)?,
        ),
        response::Which::MessageWithdrawn(value) => ResponsePayload::MessageWithdrawn(
            crate::composer_state_codec::decode_queued_message_withdrawal_result(
                value?,
                &request_id,
            )?,
        ),
        response::Which::RecalledMessage(value) => ResponsePayload::RecalledMessage(
            crate::composer_state_codec::decode_recalled_message_result(value?)?,
        ),
        response::Which::RunUsage(value) => ResponsePayload::RunUsage(
            crate::composer_state_codec::decode_run_usage_result(value?)?,
        ),
        response::Which::FailedMessages(value) => ResponsePayload::FailedMessages(
            crate::composer_state_codec::decode_failed_message_listing(value?)?,
        ),
        response::Which::AccountUsage(value) => decode_engine_usage_snapshot(value?)?,
        response::Which::ComposerCatalog(result) => decode_composer_catalog(result?)?,
        response::Which::ModelFavorites(snapshot) => ResponsePayload::ModelFavorites(
            decode_model_favorites_snapshot(snapshot?, "response.modelFavorites.modelIds")?,
        ),
        response::Which::ModelFavoriteSet(receipt) => {
            decode_model_favorite_set(receipt?, &request_id)?
        }
        response::Which::ConversationSnapshot(snapshot) => {
            ResponsePayload::ConversationSnapshot(decode_conversation_snapshot(snapshot?)?)
        }
        response::Which::ConversationSubscriptionStarted(started) => {
            ResponsePayload::ConversationSubscriptionStarted(
                decode_conversation_subscription_started(started?)?,
            )
        }
        response::Which::ConversationSubscriptionStopped(stopped) => {
            ResponsePayload::ConversationSubscriptionStopped(
                decode_conversation_subscription_stopped(stopped?)?,
            )
        }
        response::Which::DirectoryPicked(picked) => decode_directory_picked(picked?)?,
        response::Which::LifecycleControl(lifecycle) => {
            ResponsePayload::Lifecycle(decode_lifecycle_response(lifecycle?)?)
        }
        response::Which::ThreadEngineConfigSet(result) => {
            decode_thread_engine_config_set(result?, &request_id)?
        }
        response::Which::ThreadEngineSettings(result) => {
            decode_thread_engine_settings_result(result?)?
        }
        response::Which::RegisteredEngineProfiles(result) => {
            decode_registered_engine_profiles_result(result?)?
        }
        response::Which::RichLink(result) => decode_rich_link_page_metadata(result?)?,
        response::Which::ProjectRepository(result) => {
            decode_project_repository_query_result(result?)?
        }
    };
    Ok(ServerResponse {
        request_id,
        payload,
    })
}

fn decode_thread_engine_config_set(
    value: artisan_capnp::set_thread_engine_config_result::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            value.get_request_id(),
            "response.threadEngineConfigSet.requestId",
        )?,
        "response.threadEngineConfigSet.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.threadEngineConfigSet.requestId",
        });
    }
    let revision = EngineConfigRevision::new(value.get_revision())?;
    Ok(ResponsePayload::ThreadEngineConfigSet(
        SetThreadEngineConfigResult {
            request_id: nested_request_id,
            thread_id: parse_thread_id(
                read_text(
                    value.get_thread_id(),
                    "response.threadEngineConfigSet.threadId",
                )?,
                "response.threadEngineConfigSet.threadId",
            )?,
            revision,
            disposition: decode_disposition(value.get_disposition()?),
        },
    ))
}

fn decode_thread_engine_settings_result(
    value: artisan_capnp::thread_engine_settings_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(
            value.get_thread_id(),
            "response.threadEngineSettings.threadId",
        )?,
        "response.threadEngineSettings.threadId",
    )?;
    let result = match value.get_state().which()? {
        artisan_capnp::thread_engine_settings_result::state::Which::Unconfigured(()) => {
            crate::types::ThreadEngineSettingsResult::Unconfigured { thread_id }
        }
        artisan_capnp::thread_engine_settings_result::state::Which::Configured(configured) => {
            let configured = configured?;
            let revision = EngineConfigRevision::new(configured.get_revision())
                .map_err(|source| ProtocolDecodeError::EngineConfig { source })?;
            let config = decode_engine_run_config(configured.get_config()?)?;
            crate::types::ThreadEngineSettingsResult::Configured {
                thread_id,
                revision,
                config: Box::new(config),
            }
        }
    };
    Ok(ResponsePayload::ThreadEngineSettings(result))
}

fn decode_registered_engine_profiles_result(
    value: artisan_capnp::registered_engine_profiles_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let result = match value.get_state().which()? {
        artisan_capnp::registered_engine_profiles_result::state::Which::RegistryMissing(()) => {
            RegisteredEngineProfilesResult::RegistryMissing
        }
        artisan_capnp::registered_engine_profiles_result::state::Which::RegistryPresent(
            present,
        ) => {
            let present = present?;
            let ids = present.get_profile_ids()?;
            let count = ids.len() as usize;
            if count > 64 {
                return Err(engine_config_error(
                    "response.registeredEngineProfiles.profileIds",
                    EngineConfigReason::OutOfRange,
                ));
            }
            let mut profile_ids = Vec::with_capacity(count);
            let mut seen = std::collections::HashSet::with_capacity(count);
            for raw in ids {
                let text = read_text(raw, "response.registeredEngineProfiles.profileIds")?;
                let id = EngineProfileId::parse(text).map_err(|_| {
                    engine_config_error(
                        "response.registeredEngineProfiles.profileIds",
                        EngineConfigReason::InvalidIdentifier,
                    )
                })?;
                if !seen.insert(id.as_str().to_owned()) {
                    return Err(engine_config_error(
                        "response.registeredEngineProfiles.profileIds",
                        EngineConfigReason::Inconsistent,
                    ));
                }
                profile_ids.push(id);
            }
            RegisteredEngineProfilesResult::RegistryPresent { profile_ids }
        }
    };
    Ok(ResponsePayload::RegisteredEngineProfiles(result))
}

fn decode_rich_link_page_metadata(
    value: artisan_capnp::rich_link_page_metadata::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    Ok(ResponsePayload::RichLink(RichLinkPageMetadata::new(
        read_text(
            value.get_requested_url(),
            "response.richLink.requestedUrl",
        )?,
        read_text(value.get_page_name(), "response.richLink.pageName")?,
        value.get_cache_expires_at_ms(),
    )?))
}

fn encode_project_repository_query(
    mut builder: artisan_capnp::project_repository_query::Builder<'_>,
    query: &ProjectRepositoryQuery,
) -> Result<(), ProtocolEncodeError> {
    let project_ids = query.project_ids();
    let mut encoded = builder.reborrow().init_project_ids(list_length(
        "request.queryProjectRepository.projectIds",
        project_ids.len(),
    )?);
    for (index, project_id) in project_ids.iter().enumerate() {
        encoded.set(
            list_index("request.queryProjectRepository.projectIds", index)?,
            project_id.as_str(),
        );
    }
    Ok(())
}

fn decode_project_repository_query(
    value: artisan_capnp::project_repository_query::Reader<'_>,
) -> Result<ProjectRepositoryQuery, ProtocolDecodeError> {
    let encoded = value.get_project_ids()?;
    let count = encoded.len() as usize;
    if count > PROJECT_REPOSITORY_MAXIMUM_PROJECTS {
        return Err(ProtocolDecodeError::ProtocolValue {
            source: ProtocolValueError::Repository {
                reason: "repository query names more projects than its bound",
            },
        });
    }
    let mut project_ids = Vec::with_capacity(count);
    for project_id in encoded.iter() {
        project_ids.push(parse_project_id(
            read_text(project_id, "request.queryProjectRepository.projectIds")?,
            "request.queryProjectRepository.projectIds",
        )?);
    }
    ProjectRepositoryQuery::new(project_ids).map_err(|source| {
        ProtocolDecodeError::ProtocolValue { source }
    })
}

fn encode_project_repository_query_result(
    builder: artisan_capnp::project_repository_query_result::Builder<'_>,
    result: &ProjectRepositoryQueryResult,
) -> Result<(), ProtocolEncodeError> {
    let repositories = result.repositories();
    let mut encoded = builder.init_repositories(list_length(
        "response.projectRepository.repositories",
        repositories.len(),
    )?);
    for (index, entry) in repositories.iter().enumerate() {
        encode_project_repository_entry(
            encoded
                .reborrow()
                .get(list_index(
                    "response.projectRepository.repositories",
                    index,
                )?),
            entry,
        )?;
    }
    Ok(())
}

fn decode_project_repository_query_result(
    value: artisan_capnp::project_repository_query_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let encoded = value.get_repositories()?;
    let count = encoded.len() as usize;
    if count > PROJECT_REPOSITORY_MAXIMUM_PROJECTS {
        return Err(ProtocolDecodeError::ProtocolValue {
            source: ProtocolValueError::Repository {
                reason: "repository result holds more projects than its bound",
            },
        });
    }
    let mut repositories = Vec::with_capacity(count);
    for entry in encoded.iter() {
        repositories.push(ProjectRepositoryEntry::new(
            parse_project_id(
                read_text(
                    entry.get_project_id(),
                    "response.projectRepository.projectId",
                )?,
                "response.projectRepository.projectId",
            )?,
            decode_project_repository(entry.get_repository()?)?,
        ));
    }
    Ok(ResponsePayload::ProjectRepository(
        ProjectRepositoryQueryResult::new(repositories).map_err(|source| {
            ProtocolDecodeError::ProtocolValue { source }
        })?,
    ))
}

fn encode_project_repository_entry(
    mut builder: artisan_capnp::project_repository_entry::Builder<'_>,
    entry: &ProjectRepositoryEntry,
) -> Result<(), ProtocolEncodeError> {
    builder.set_project_id(entry.project_id().as_str());
    encode_project_repository(builder.init_repository(), entry.repository())
}

fn encode_project_repository(
    mut builder: artisan_capnp::project_repository::Builder<'_>,
    repository: &ProjectRepository,
) -> Result<(), ProtocolEncodeError> {
    match repository {
        ProjectRepository::NotRepository => {
            builder.set_state(artisan_capnp::ProjectRepositoryState::NotRepository);
        }
        ProjectRepository::Repository(snapshot) => {
            builder.set_state(artisan_capnp::ProjectRepositoryState::Repository);
            let mut encoded = builder.reborrow().init_snapshot();
            encode_repository_branch(encoded.reborrow().init_branch(), snapshot.branch());
            encoded.set_default_remote(snapshot.default_remote().unwrap_or(""));
            let remotes = snapshot.remotes();
            let mut list = encoded.init_remotes(list_length(
                "response.projectRepository.remotes",
                remotes.len(),
            )?);
            for (index, remote) in remotes.iter().enumerate() {
                encode_repository_remote(
                    list.reborrow().get(list_index(
                        "response.projectRepository.remotes",
                        index,
                    )?),
                    remote,
                );
            }
        }
    }
    Ok(())
}

fn encode_repository_branch(
    mut builder: artisan_capnp::repository_branch::Builder<'_>,
    branch: &RepositoryBranchState,
) {
    match branch {
        RepositoryBranchState::Attached { name } => {
            builder.set_kind(artisan_capnp::RepositoryBranchKind::Attached);
            builder.set_name(name);
        }
        RepositoryBranchState::Detached => {
            builder.set_kind(artisan_capnp::RepositoryBranchKind::Detached);
        }
        RepositoryBranchState::Unborn { name } => {
            builder.set_kind(artisan_capnp::RepositoryBranchKind::Unborn);
            builder.set_name(name);
        }
    }
}

fn encode_repository_remote(
    mut builder: artisan_capnp::repository_remote::Builder<'_>,
    remote: &RepositoryRemote,
) {
    builder.set_host(encode_repository_host(remote.host()));
    builder.set_name(remote.name());
    builder.set_url(remote.url());
    builder.set_web_url(remote.web_url().unwrap_or(""));
}

fn encode_repository_host(host: RepositoryHost) -> artisan_capnp::RepositoryHost {
    match host {
        RepositoryHost::Azure => artisan_capnp::RepositoryHost::Azure,
        RepositoryHost::Bitbucket => artisan_capnp::RepositoryHost::Bitbucket,
        RepositoryHost::Codeberg => artisan_capnp::RepositoryHost::Codeberg,
        RepositoryHost::Gitea => artisan_capnp::RepositoryHost::Gitea,
        RepositoryHost::GitHub => artisan_capnp::RepositoryHost::Github,
        RepositoryHost::GitLab => artisan_capnp::RepositoryHost::Gitlab,
        RepositoryHost::Other => artisan_capnp::RepositoryHost::Other,
        RepositoryHost::Sourcehut => artisan_capnp::RepositoryHost::Sourcehut,
        RepositoryHost::Unknown => artisan_capnp::RepositoryHost::Unknown,
    }
}

fn decode_repository_host(host: artisan_capnp::RepositoryHost) -> RepositoryHost {
    match host {
        artisan_capnp::RepositoryHost::Azure => RepositoryHost::Azure,
        artisan_capnp::RepositoryHost::Bitbucket => RepositoryHost::Bitbucket,
        artisan_capnp::RepositoryHost::Codeberg => RepositoryHost::Codeberg,
        artisan_capnp::RepositoryHost::Gitea => RepositoryHost::Gitea,
        artisan_capnp::RepositoryHost::Github => RepositoryHost::GitHub,
        artisan_capnp::RepositoryHost::Gitlab => RepositoryHost::GitLab,
        artisan_capnp::RepositoryHost::Other => RepositoryHost::Other,
        artisan_capnp::RepositoryHost::Sourcehut => RepositoryHost::Sourcehut,
        artisan_capnp::RepositoryHost::Unknown => RepositoryHost::Unknown,
    }
}

fn decode_project_repository(
    value: artisan_capnp::project_repository::Reader<'_>,
) -> Result<ProjectRepository, ProtocolDecodeError> {
    match value.get_state()? {
        artisan_capnp::ProjectRepositoryState::NotRepository => Ok(ProjectRepository::NotRepository),
        artisan_capnp::ProjectRepositoryState::Repository => {
            let snapshot = value.get_snapshot()?;
            let branch = decode_repository_branch(snapshot.get_branch()?)?;
            let default_remote = match read_text(
                snapshot.get_default_remote(),
                "response.projectRepository.defaultRemote",
            )? {
                name if name.is_empty() => None,
                name => Some(name),
            };
            let encoded = snapshot.get_remotes()?;
            let count = encoded.len() as usize;
            if count > REPOSITORY_REMOTE_MAXIMUM {
                return Err(ProtocolDecodeError::ProtocolValue {
                    source: ProtocolValueError::Repository {
                        reason: "repository holds more remotes than its bound",
                    },
                });
            }
            let mut remotes = Vec::with_capacity(count);
            for remote in encoded.iter() {
                let web_url = match read_text(
                    remote.get_web_url(),
                    "response.projectRepository.webUrl",
                )? {
                    url if url.is_empty() => None,
                    url => Some(url),
                };
                remotes.push(
                    RepositoryRemote::new(
                        decode_repository_host(remote.get_host()?),
                        read_text(remote.get_name(), "response.projectRepository.name")?,
                        read_text(remote.get_url(), "response.projectRepository.url")?,
                        web_url,
                    )
                    .map_err(|source| ProtocolDecodeError::ProtocolValue { source })?,
                );
            }
            let snapshot = RepositorySnapshot::new(branch, default_remote, remotes)
                .map_err(|source| ProtocolDecodeError::ProtocolValue { source })?;
            Ok(ProjectRepository::Repository(snapshot))
        }
    }
}

fn decode_repository_branch(
    value: artisan_capnp::repository_branch::Reader<'_>,
) -> Result<RepositoryBranchState, ProtocolDecodeError> {
    let branch = match value.get_kind()? {
        artisan_capnp::RepositoryBranchKind::Attached => {
            RepositoryBranchState::attached(read_text(
                value.get_name(),
                "response.projectRepository.branch.name",
            )?)
        }
        artisan_capnp::RepositoryBranchKind::Detached => Ok(RepositoryBranchState::detached()),
        artisan_capnp::RepositoryBranchKind::Unborn => {
            RepositoryBranchState::unborn(read_text(
                value.get_name(),
                "response.projectRepository.branch.name",
            )?)
        }
    };
    branch.map_err(|source| ProtocolDecodeError::ProtocolValue { source })
}

fn decode_lifecycle_response(
    value: artisan_capnp::lifecycle_response::Reader<'_>,
) -> Result<LifecycleResponse, ProtocolDecodeError> {
    match value.which()? {
        lifecycle_response::Which::Status(status) => {
            let status = status?;
            Ok(LifecycleResponse::Status(LifecycleStatus::new(
                decode_lifecycle_state(status.get_state()?),
                status.get_active_work_count(),
            )?))
        }
        lifecycle_response::Which::Stop(receipt) => {
            let receipt = receipt?;
            Ok(LifecycleResponse::Stop(LifecycleStopReceipt {
                disposition: decode_lifecycle_stop_disposition(receipt.get_disposition()?),
                state: decode_lifecycle_state(receipt.get_state()?),
            }))
        }
    }
}

fn decode_queued_receipt(
    receipt: artisan_capnp::first_message_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(receipt.get_request_id(), "response.queuedReceipt.requestId")?,
        "response.queuedReceipt.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.queuedReceipt.requestId",
        });
    }
    match receipt.get_state()? {
        artisan_capnp::QueuedState::Queued => {}
    }
    Ok(ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
        request_id: nested_request_id,
        message_id: parse_message_id(
            read_text(receipt.get_message_id(), "response.queuedReceipt.messageId")?,
            "response.queuedReceipt.messageId",
        )?,
        thread_id: parse_thread_id(
            read_text(receipt.get_thread_id(), "response.queuedReceipt.threadId")?,
            "response.queuedReceipt.threadId",
        )?,
        disposition: decode_disposition(receipt.get_disposition()?),
    }))
}

fn decode_queue_message_receipt(
    receipt: artisan_capnp::queue_message_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.queuedMessageReceipt.requestId",
        )?,
        "response.queuedMessageReceipt.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.queuedMessageReceipt.requestId",
        });
    }
    match receipt.get_state()? {
        artisan_capnp::QueuedState::Queued => {}
    }
    Ok(ResponsePayload::MessageQueued(QueueMessageReceipt {
        request_id: nested_request_id,
        message_id: parse_message_id(
            read_text(
                receipt.get_message_id(),
                "response.queuedMessageReceipt.messageId",
            )?,
            "response.queuedMessageReceipt.messageId",
        )?,
        thread_id: parse_thread_id(
            read_text(
                receipt.get_thread_id(),
                "response.queuedMessageReceipt.threadId",
            )?,
            "response.queuedMessageReceipt.threadId",
        )?,
        disposition: decode_disposition(receipt.get_disposition()?),
    }))
}

fn decode_message_image(
    result: artisan_capnp::message_image_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let reference = decode_image_attachment_ref(
        result.get_reference()?,
        "response.messageImage.reference",
        None,
    )?;
    let bytes = result.get_bytes()?.to_vec();
    if bytes.len() != usize::try_from(reference.size_bytes).unwrap_or(usize::MAX) {
        return Err(ProtocolDecodeError::ImageAttachmentReference {
            field: "response.messageImage.bytes",
            reason: "image bytes do not match reference size",
        });
    }
    Ok(ResponsePayload::MessageImage(MessageImageResult {
        reference,
        bytes,
    }))
}

fn decode_stop_run_receipt(
    receipt: artisan_capnp::stop_run_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.stopRunReceipt.requestId",
        )?,
        "response.stopRunReceipt.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.stopRunReceipt.requestId",
        });
    }
    Ok(ResponsePayload::RunStopped(StopRunReceipt {
        request_id: nested_request_id,
        thread_id: parse_thread_id(
            read_text(receipt.get_thread_id(), "response.stopRunReceipt.threadId")?,
            "response.stopRunReceipt.threadId",
        )?,
        run_id: parse_run_id(
            read_text(receipt.get_run_id(), "response.stopRunReceipt.runId")?,
            "response.stopRunReceipt.runId",
        )?,
        disposition: decode_stop_run_disposition(receipt.get_disposition()?),
    }))
}

fn decode_respond_approval_receipt(
    receipt: artisan_capnp::respond_approval_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.approvalResponse.requestId",
        )?,
        "response.approvalResponse.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.approvalResponse.requestId",
        });
    }
    Ok(ResponsePayload::ApprovalResponse(RespondApprovalReceipt {
        request_id: nested_request_id,
        thread_id: parse_thread_id(
            read_text(
                receipt.get_thread_id(),
                "response.approvalResponse.threadId",
            )?,
            "response.approvalResponse.threadId",
        )?,
        run_id: parse_run_id(
            read_text(receipt.get_run_id(), "response.approvalResponse.runId")?,
            "response.approvalResponse.runId",
        )?,
        approval_id: parse_observation_id(
            read_text(
                receipt.get_approval_id(),
                "response.approvalResponse.approvalId",
            )?,
            "response.approvalResponse.approvalId",
        )?,
        approved: receipt.get_approved(),
        outcome: decode_run_interaction_outcome(receipt.get_outcome()?),
        disposition: decode_disposition(receipt.get_disposition()?),
    }))
}

fn decode_respond_question_receipt(
    receipt: artisan_capnp::respond_question_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.questionResponse.requestId",
        )?,
        "response.questionResponse.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.questionResponse.requestId",
        });
    }
    let thread_id = parse_thread_id(
        read_text(
            receipt.get_thread_id(),
            "response.questionResponse.threadId",
        )?,
        "response.questionResponse.threadId",
    )?;
    let run_id = parse_run_id(
        read_text(receipt.get_run_id(), "response.questionResponse.runId")?,
        "response.questionResponse.runId",
    )?;
    let question_id = parse_observation_id(
        read_text(
            receipt.get_question_id(),
            "response.questionResponse.questionId",
        )?,
        "response.questionResponse.questionId",
    )?;
    let answers = decode_answer_list(receipt.get_answers()?, "response.questionResponse.answers")?;
    // The echoed answers prove the identical intent; rebuilding the domain
    // command validates them exactly once.
    let command = RespondQuestion::new(
        nested_request_id.clone(),
        thread_id.clone(),
        run_id.clone(),
        question_id.clone(),
        answers,
    )
    .map_err(|source| ProtocolDecodeError::RunInteraction { source })?;
    Ok(ResponsePayload::QuestionResponse(RespondQuestionReceipt {
        request_id: nested_request_id,
        thread_id,
        run_id,
        question_id,
        answers: command.answers().clone(),
        outcome: decode_run_interaction_outcome(receipt.get_outcome()?),
        disposition: decode_disposition(receipt.get_disposition()?),
    }))
}

fn decode_active_run_result(
    result: artisan_capnp::active_run_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(result.get_thread_id(), "response.activeRun.threadId")?,
        "response.activeRun.threadId",
    )?;
    let result = match result.get_state().which()? {
        artisan_capnp::active_run_result::state::Which::NoActive(()) => {
            ActiveRunResult::NoActive { thread_id }
        }
        artisan_capnp::active_run_result::state::Which::Active(run_id) => {
            let status = decode_run_status(result.get_run_status()?)?;
            let engine_id = EngineId::parse(
                read_text(result.get_run_engine_id(), "response.activeRun.runEngineId")?.as_str(),
            )?;
            ActiveRunResult::Active {
                thread_id,
                run_id: parse_run_id(
                    read_text(run_id, "response.activeRun.runId")?,
                    "response.activeRun.runId",
                )?,
                status,
                engine_id,
            }
        }
    };
    Ok(ResponsePayload::ActiveRun(result))
}

fn decode_composer_catalog(
    result: artisan_capnp::composer_catalog_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(result.get_thread_id(), "response.composerCatalog.threadId")?,
        "response.composerCatalog.threadId",
    )?;
    let profile_id = parse_profile_id(
        read_text(
            result.get_profile_id(),
            "response.composerCatalog.profileId",
        )?,
        "response.composerCatalog.profileId",
    )?;
    let snapshot = CatalogSnapshotWire::new(result.get_snapshot_data()?.to_vec())?;
    Ok(ResponsePayload::ComposerCatalog(
        ComposerCatalogResult::new(thread_id, profile_id, snapshot)?,
    ))
}

fn decode_read_account_usage(
    value: read_account_usage_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let engine_id = match value.get_scope().which()? {
        read_account_usage_request::scope::Which::All(()) => None,
        read_account_usage_request::scope::Which::One(engine_id) => {
            Some(read_text(engine_id, "request.readAccountUsage.engineId")?)
        }
    };
    Ok(ClientRequest::Query(Query::ReadAccountUsage(
        ReadAccountUsage::new(engine_id, value.get_force())?,
    )))
}

fn decode_engine_usage_window_kind(
    kind: artisan_capnp::EngineUsageWindowKind,
) -> EngineUsageWindowKind {
    match kind {
        artisan_capnp::EngineUsageWindowKind::Session => EngineUsageWindowKind::Session,
        artisan_capnp::EngineUsageWindowKind::Weekly => EngineUsageWindowKind::Weekly,
        artisan_capnp::EngineUsageWindowKind::Monthly => EngineUsageWindowKind::Monthly,
        artisan_capnp::EngineUsageWindowKind::Unknown => EngineUsageWindowKind::Unknown,
    }
}

fn decode_engine_usage_authentication(
    state: artisan_capnp::EngineUsageAuthentication,
) -> EngineUsageAuthentication {
    match state {
        artisan_capnp::EngineUsageAuthentication::Authenticated => {
            EngineUsageAuthentication::Authenticated
        }
        artisan_capnp::EngineUsageAuthentication::Unauthenticated => {
            EngineUsageAuthentication::Unauthenticated
        }
        artisan_capnp::EngineUsageAuthentication::Unknown => EngineUsageAuthentication::Unknown,
    }
}

fn decode_quota_surface(surface: artisan_capnp::QuotaSurface) -> QuotaSurface {
    match surface {
        artisan_capnp::QuotaSurface::Supported => QuotaSurface::Supported,
        artisan_capnp::QuotaSurface::Unknown => QuotaSurface::Unknown,
        artisan_capnp::QuotaSurface::Unsupported => QuotaSurface::Unsupported,
    }
}

fn optional_wire_text(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn decode_engine_usage_window(
    window: engine_usage_window::Reader<'_>,
) -> Result<EngineUsageWindow, ProtocolDecodeError> {
    let window_minutes = match window.get_window_minutes() {
        0 => None,
        minutes => Some(minutes),
    };
    Ok(EngineUsageWindow::new(
        read_text(window.get_id(), "response.accountUsage.window.id")?,
        decode_engine_usage_window_kind(window.get_kind()?),
        optional_wire_text(read_text(
            window.get_label(),
            "response.accountUsage.window.label",
        )?),
        window.get_percent_used(),
        optional_wire_text(read_text(
            window.get_resets_at(),
            "response.accountUsage.window.resetsAt",
        )?),
        window_minutes,
    )?)
}

fn decode_engine_usage_report(
    report: engine_usage_report::Reader<'_>,
) -> Result<EngineUsageReport, ProtocolDecodeError> {
    let authentication = EngineUsageAuth::new(
        decode_engine_usage_authentication(report.get_authentication()?),
        optional_wire_text(read_text(
            report.get_auth_reason(),
            "response.accountUsage.authReason",
        )?),
    )?;
    let quota_surface = match report.get_quota_surface().which()? {
        engine_usage_report::quota_surface::Which::Absent(()) => None,
        engine_usage_report::quota_surface::Which::Present(surface) => {
            Some(decode_quota_surface(surface?))
        }
    };
    let encoded_windows = report.get_windows()?;
    let count = encoded_windows.len() as usize;
    if count > ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE {
        return Err(EngineUsageError::TooManyWindows {
            count,
            maximum: ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE,
        }
        .into());
    }
    let mut windows = Vec::with_capacity(count);
    for encoded_window in encoded_windows.iter() {
        windows.push(decode_engine_usage_window(encoded_window)?);
    }
    Ok(EngineUsageReport::new(
        optional_wire_text(read_text(
            report.get_account_email(),
            "response.accountUsage.accountEmail",
        )?),
        authentication,
        read_text(
            report.get_display_name(),
            "response.accountUsage.displayName",
        )?,
        read_text(report.get_engine_id(), "response.accountUsage.engineId")?,
        optional_wire_text(read_text(
            report.get_failure(),
            "response.accountUsage.failure",
        )?),
        quota_surface,
        windows,
    )?)
}

fn decode_engine_usage_snapshot(
    value: engine_usage_snapshot::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let fetched_at = read_text(value.get_fetched_at(), "response.accountUsage.fetchedAt")?;
    let encoded_engines = value.get_engines()?;
    let count = encoded_engines.len() as usize;
    if count > ENGINE_USAGE_ENGINES_MAX {
        return Err(EngineUsageError::TooManyEngines {
            count,
            maximum: ENGINE_USAGE_ENGINES_MAX,
        }
        .into());
    }
    let mut engines = Vec::with_capacity(count);
    for encoded_engine in encoded_engines.iter() {
        engines.push(decode_engine_usage_report(encoded_engine)?);
    }
    Ok(ResponsePayload::AccountUsage(EngineUsageSnapshot::new(
        engines, fetched_at,
    )?))
}

fn decode_model_favorites_snapshot(
    value: artisan_capnp::model_favorites_snapshot::Reader<'_>,
    model_ids_field: &'static str,
) -> Result<ModelFavoritesSnapshot, ProtocolDecodeError> {
    let encoded_model_ids = value.get_model_ids()?;
    let count = encoded_model_ids.len() as usize;
    if count > artisan_domain::MODEL_FAVORITES_MAX_MODELS {
        return Err(ProtocolDecodeError::ModelFavoritesSnapshot {
            source: ModelFavoritesSnapshotError::TooManyModels {
                count,
                maximum: artisan_domain::MODEL_FAVORITES_MAX_MODELS,
            },
        });
    }
    let mut model_ids = Vec::with_capacity(count);
    for encoded_model_id in encoded_model_ids.iter() {
        model_ids.push(ModelFavoriteId::parse(read_text(
            encoded_model_id,
            model_ids_field,
        )?)?);
    }
    let revision = ModelFavoritesRevision::new(value.get_revision())?;
    ModelFavoritesSnapshot::new(revision, model_ids).map_err(ProtocolDecodeError::from)
}

fn decode_model_favorite_set(
    receipt: artisan_capnp::set_model_favorite_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.modelFavoriteSet.requestId",
        )?,
        "response.modelFavoriteSet.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.modelFavoriteSet.requestId",
        });
    }
    Ok(ResponsePayload::ModelFavoriteSet(SetModelFavoriteReceipt {
        request_id: nested_request_id,
        model_id: ModelFavoriteId::parse(read_text(
            receipt.get_model_id(),
            "response.modelFavoriteSet.modelId",
        )?)?,
        favorite: receipt.get_favorite(),
        disposition: decode_disposition(receipt.get_disposition()?),
        snapshot: decode_model_favorites_snapshot(
            receipt.get_snapshot()?,
            "response.modelFavoriteSet.snapshot.modelIds",
        )?,
    }))
}

fn decode_conversation_subscription_started(
    started: artisan_capnp::conversation_subscription_started::Reader<'_>,
) -> Result<ConversationSubscriptionStarted, ProtocolDecodeError> {
    match started.which()? {
        conversation_subscription_started::Which::Fresh(snapshot) => {
            Ok(ConversationSubscriptionStarted::Fresh(
                ConversationSubscriptionStart::new(decode_conversation_snapshot(snapshot?)?),
            ))
        }
        conversation_subscription_started::Which::Resumed(point) => {
            let point = point?;
            Ok(ConversationSubscriptionStarted::Resumed {
                thread_id: parse_thread_id(
                    read_text(
                        point.get_thread_id(),
                        "response.conversationSubscriptionStarted.resumed.threadId",
                    )?,
                    "response.conversationSubscriptionStarted.resumed.threadId",
                )?,
                cursor: ConversationCursor::new(point.get_cursor()),
            })
        }
    }
}

fn decode_conversation_subscription_stopped(
    stopped: artisan_capnp::conversation_subscription_stopped::Reader<'_>,
) -> Result<ConversationSubscriptionStopped, ProtocolDecodeError> {
    Ok(ConversationSubscriptionStopped {
        thread_id: parse_thread_id(
            read_text(
                stopped.get_thread_id(),
                "response.conversationSubscriptionStopped.threadId",
            )?,
            "response.conversationSubscriptionStopped.threadId",
        )?,
    })
}

fn decode_directory_picked(
    picked: artisan_capnp::directory_pick_outcome::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let outcome = match picked.which()? {
        directory_pick_outcome::Which::Selected(directory_id) => {
            DirectoryPickOutcome::Selected(parse_directory_id(
                read_text(directory_id, "response.directoryPicked.selected")?,
                "response.directoryPicked.selected",
            )?)
        }
        directory_pick_outcome::Which::Cancelled(()) => DirectoryPickOutcome::Cancelled,
    };
    Ok(ResponsePayload::DirectoryPicked(outcome))
}

fn decode_event(
    value: artisan_capnp::event::Reader<'_>,
) -> Result<ServerEvent, ProtocolDecodeError> {
    let cursor = EventCursor::new(value.get_cursor())?;
    let event = match value.which()? {
        event::Which::ProjectAttached(project) => Event::ProjectAttached(ProjectAttached {
            project: decode_project(project?)?,
        }),
        event::Which::ThreadCreated(thread) => Event::ThreadCreated(ThreadCreated {
            thread: decode_thread(thread?)?,
        }),
        event::Which::FirstMessageQueued(queued) => {
            let queued = queued?;
            Event::FirstMessageQueued(FirstMessageQueued {
                message: QueuedMessage {
                    request_id: parse_request_id(
                        read_text(
                            queued.get_request_id(),
                            "event.firstMessageQueued.requestId",
                        )?,
                        "event.firstMessageQueued.requestId",
                    )?,
                    message_id: parse_message_id(
                        read_text(
                            queued.get_message_id(),
                            "event.firstMessageQueued.messageId",
                        )?,
                        "event.firstMessageQueued.messageId",
                    )?,
                    thread_id: parse_thread_id(
                        read_text(queued.get_thread_id(), "event.firstMessageQueued.threadId")?,
                        "event.firstMessageQueued.threadId",
                    )?,
                    body: MessageBody::parse(read_text(
                        queued.get_body(),
                        "event.firstMessageQueued.body",
                    )?)
                    .map_err(|source| ProtocolDecodeError::MessageBody { source })?,
                },
            })
        }
        event::Which::EngineObservation(value) => {
            let value = value?;
            let thread_id = parse_thread_id(
                read_text(value.get_thread_id(), "event.engineObservation.threadId")?,
                "event.engineObservation.threadId",
            )?;
            let attribution = decode_engine_observation_attribution(value.get_attribution())?;
            Event::EngineObservation(EngineObservationEvent {
                thread_id,
                observation: decode_engine_observation(value.get_observation()?)?,
                attribution,
            })
        }
    };
    Ok(ServerEvent { cursor, event })
}

/// Decodes the additive optional observation attribution.
///
/// Old frames never set the union and decode as `None`, preserving the frozen
/// v1 bytes. When present, run/turn ids must parse, the commit time must be
/// positive, and the delivery sequence must be positive; any malformed or
/// nonpositive value is a typed rejection, never a silent default.
fn decode_engine_observation_attribution(
    value: artisan_capnp::engine_observation_event::attribution::Reader<'_>,
) -> Result<Option<EngineObservationAttribution>, ProtocolDecodeError> {
    use artisan_capnp::engine_observation_event::attribution::Which;
    match value.which()? {
        Which::NoAttribution(()) => Ok(None),
        Which::Attribution(attribution) => {
            let attribution = attribution?;
            let run_id = parse_run_id(
                read_text(
                    attribution.get_run_id(),
                    "event.engineObservation.attribution.runId",
                )?,
                "event.engineObservation.attribution.runId",
            )?;
            let turn_id = parse_turn_id(
                read_text(
                    attribution.get_turn_id(),
                    "event.engineObservation.attribution.turnId",
                )?,
                "event.engineObservation.attribution.turnId",
            )?;
            let committed_at_millis = attribution.get_committed_at_millis();
            if committed_at_millis <= 0 {
                return Err(ProtocolDecodeError::Observation {
                    source: artisan_domain::ObservationError::OutOfRange { field: "committed_at" },
                });
            }
            let delivery_sequence = attribution.get_delivery_sequence();
            if delivery_sequence == 0 {
                return Err(ProtocolDecodeError::Observation {
                    source: artisan_domain::ObservationError::OutOfRange {
                        field: "delivery_sequence",
                    },
                });
            }
            Ok(Some(EngineObservationAttribution {
                run_id,
                turn_id,
                committed_at: UnixMillis::from_millis(committed_at_millis),
                delivery_sequence,
            }))
        }
    }
}

fn decode_protocol_error(
    value: artisan_capnp::protocol_error::Reader<'_>,
) -> Result<ProtocolFailure, ProtocolDecodeError> {
    let request_id = match value.which()? {
        protocol_error::Which::Correlated(request_id) => Some(parse_request_id(
            read_text(request_id, "protocolError.correlated")?,
            "protocolError.correlated",
        )?),
        protocol_error::Which::Uncorrelated(()) => None,
    };
    Ok(ProtocolFailure {
        code: decode_error_code(value.get_code()?),
        detail: ErrorDetail::parse(read_text(value.get_message(), "protocolError.message")?)?,
        retryable: value.get_retryable(),
        request_id,
    })
}

fn decode_directory_listing(
    value: artisan_capnp::directory_listing::Reader<'_>,
) -> Result<DirectoryListing, ProtocolDecodeError> {
    let places = value.get_places()?;
    let places_count = places.len() as usize;
    if places_count > DIRECTORY_LISTING_MAX_PLACES {
        return Err(ProtocolDecodeError::DirectoryListing {
            source: DirectoryListingError::TooManyPlaces {
                count: places_count,
                maximum: DIRECTORY_LISTING_MAX_PLACES,
            },
        });
    }

    let entries = value.get_entries()?;
    let entries_count = entries.len() as usize;
    if entries_count > DIRECTORY_LISTING_MAX_ENTRIES {
        return Err(ProtocolDecodeError::DirectoryListing {
            source: DirectoryListingError::TooManyEntries {
                count: entries_count,
                maximum: DIRECTORY_LISTING_MAX_ENTRIES,
            },
        });
    }

    let parent = match value.get_parent().which()? {
        directory_listing::parent::Which::NoParent(()) => None,
        directory_listing::parent::Which::Parent(parent) => Some(parse_directory_id(
            read_text(parent, "directoryListing.parent")?,
            "directoryListing.parent",
        )?),
    };

    let places = places
        .iter()
        .map(|place| {
            Ok(DirectoryPlace {
                kind: decode_place_kind(place.get_kind()?),
                directory_id: parse_directory_id(
                    read_text(
                        place.get_directory_id(),
                        "directoryListing.places.directoryId",
                    )?,
                    "directoryListing.places.directoryId",
                )?,
                display_name: DisplayName::parse(read_text(
                    place.get_display_name(),
                    "directoryListing.places.displayName",
                )?)
                .map_err(|source| ProtocolDecodeError::DisplayName {
                    field: "directoryListing.places.displayName",
                    source,
                })?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;

    let entries = entries
        .iter()
        .map(|entry| {
            Ok(DirectoryEntry {
                directory_id: parse_directory_id(
                    read_text(
                        entry.get_directory_id(),
                        "directoryListing.entries.directoryId",
                    )?,
                    "directoryListing.entries.directoryId",
                )?,
                display_name: DisplayName::parse(read_text(
                    entry.get_display_name(),
                    "directoryListing.entries.displayName",
                )?)
                .map_err(|source| ProtocolDecodeError::DisplayName {
                    field: "directoryListing.entries.displayName",
                    source,
                })?,
                kind: decode_directory_kind(entry.get_kind()?),
                has_children: entry.get_has_children(),
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;

    DirectoryListing::new(places, entries, parent)
        .map_err(|source| ProtocolDecodeError::DirectoryListing { source })
}

fn decode_project(
    value: artisan_capnp::project::Reader<'_>,
) -> Result<ProjectSummary, ProtocolDecodeError> {
    Ok(ProjectSummary {
        project_id: parse_project_id(
            read_text(value.get_project_id(), "project.projectId")?,
            "project.projectId",
        )?,
        display_name: DisplayName::parse(read_text(
            value.get_display_name(),
            "project.displayName",
        )?)
        .map_err(|source| ProtocolDecodeError::DisplayName {
            field: "project.displayName",
            source,
        })?,
        root_path: RootPath::parse(read_text(value.get_root_path(), "project.rootPath")?)
            .map_err(|source| ProtocolDecodeError::RootPath { source })?,
        attached_at: UnixMillis::from_millis(value.get_attached_at_millis()),
    })
}

fn decode_thread(
    value: artisan_capnp::thread_summary::Reader<'_>,
) -> Result<ThreadSummary, ProtocolDecodeError> {
    Ok(ThreadSummary {
        thread_id: parse_thread_id(
            read_text(value.get_thread_id(), "thread.threadId")?,
            "thread.threadId",
        )?,
        project_id: parse_project_id(
            read_text(value.get_project_id(), "thread.projectId")?,
            "thread.projectId",
        )?,
        title: ThreadTitle::parse(read_text(value.get_title(), "thread.title")?)
            .map_err(|source| ProtocolDecodeError::ThreadTitle { source })?,
        created_at: UnixMillis::from_millis(value.get_created_at_millis()),
        updated_at: UnixMillis::from_millis(value.get_updated_at_millis()),
    })
}

fn decode_conversation_snapshot(
    value: artisan_capnp::conversation_snapshot::Reader<'_>,
) -> Result<ConversationSnapshot, ProtocolDecodeError> {
    let turns = value.get_turns()?;
    let turn_count = turns.len() as usize;
    let maximum_turn_count = usize::from(CONVERSATION_QUERY_MAX_TURNS);
    if turn_count > maximum_turn_count {
        return Err(ConversationSnapshotError::TooManyTurns {
            count: turn_count,
            maximum: maximum_turn_count,
        }
        .into());
    }
    let turns = turns
        .iter()
        .map(decode_conversation_turn)
        .collect::<Result<Vec<_>, _>>()?;
    let items = value
        .get_items()?
        .iter()
        .map(decode_conversation_item)
        .collect::<Result<Vec<_>, _>>()?;
    ConversationSnapshot::new(
        parse_thread_id(
            read_text(value.get_thread_id(), "conversationSnapshot.threadId")?,
            "conversationSnapshot.threadId",
        )?,
        ConversationCursor::new(value.get_cursor()),
        turns,
        items,
        UnixMillis::from_millis(value.get_updated_at_millis()),
    )
    .map_err(ProtocolDecodeError::from)
}

fn decode_conversation_turn(
    value: artisan_capnp::conversation_turn::Reader<'_>,
) -> Result<ConversationTurn, ProtocolDecodeError> {
    Ok(ConversationTurn {
        turn_id: parse_turn_id(
            read_text(value.get_turn_id(), "conversationTurn.turnId")?,
            "conversationTurn.turnId",
        )?,
        ordinal: TurnOrdinal::new(value.get_ordinal()),
        revision: Revision::new(value.get_revision()),
        lifecycle: decode_conversation_lifecycle(value.get_lifecycle()?),
        created_at: UnixMillis::from_millis(value.get_created_at_millis()),
        updated_at: UnixMillis::from_millis(value.get_updated_at_millis()),
    })
}

/// Decodes the optional source-message identity carried by user items.
///
/// Empty or absent wire text means the row predates the field and decodes
/// to `None` (legacy compatibility). Present text validates as a message
/// id; corrupt text fails typed instead of fabricating an identity.
fn decode_source_message_id(
    value: capnp::Result<capnp::text::Reader<'_>>,
    field: &'static str,
) -> Result<Option<MessageId>, ProtocolDecodeError> {
    let text = read_text(value, field)?;
    if text.is_empty() {
        Ok(None)
    } else {
        parse_message_id(text, field).map(Some)
    }
}

fn decode_conversation_item(
    value: artisan_capnp::conversation_item::Reader<'_>,
) -> Result<ConversationItem, ProtocolDecodeError> {
    match value.which()? {
        conversation_item::Which::UserMessage(message) => {
            let message = message?;
            Ok(ConversationItem::UserMessage(UserMessageItem {
                item_id: parse_item_id(
                    read_text(message.get_item_id(), "conversationItem.userMessage.itemId")?,
                    "conversationItem.userMessage.itemId",
                )?,
                source_message_id: decode_source_message_id(
                    message.get_source_message_id(),
                    "conversationItem.userMessage.sourceMessageId",
                )?,
                turn_id: parse_turn_id(
                    read_text(message.get_turn_id(), "conversationItem.userMessage.turnId")?,
                    "conversationItem.userMessage.turnId",
                )?,
                ordinal: ItemOrdinal::new(message.get_ordinal()),
                revision: Revision::new(message.get_revision()),
                lifecycle: decode_conversation_lifecycle(message.get_lifecycle()?),
                body: MessageBody::parse(read_text(
                    message.get_body(),
                    "conversationItem.userMessage.body",
                )?)
                .map_err(|source| ProtocolDecodeError::MessageBody { source })?,
                created_at: UnixMillis::from_millis(message.get_created_at_millis()),
                updated_at: UnixMillis::from_millis(message.get_updated_at_millis()),
            }))
        }
        conversation_item::Which::MultimodalUserMessage(message) => {
            let message = message?;
            let text = match message.get_text().which()? {
                artisan_capnp::multimodal_user_message_item::text::Which::Absent(()) => None,
                artisan_capnp::multimodal_user_message_item::text::Which::Present(value) => Some(
                    AuthoredText::parse(read_text(
                        value,
                        "conversationItem.multimodalUserMessage.text",
                    )?)
                    .map_err(|source| {
                        ProtocolDecodeError::MessagePayload {
                            source: QueueMessagePayloadError::Text(source),
                        }
                    })?,
                ),
            };
            let attachments = decode_image_attachment_refs(
                message.get_attachments()?,
                "conversationItem.multimodalUserMessage.attachments",
            )?;
            if attachments.is_empty() {
                return Err(ProtocolDecodeError::MessagePayload {
                    source: QueueMessagePayloadError::Empty,
                });
            }
            Ok(ConversationItem::MultimodalUserMessage(
                MultimodalUserMessageItem {
                    item_id: parse_item_id(
                        read_text(
                            message.get_item_id(),
                            "conversationItem.multimodalUserMessage.itemId",
                        )?,
                        "conversationItem.multimodalUserMessage.itemId",
                    )?,
                    source_message_id: decode_source_message_id(
                        message.get_source_message_id(),
                        "conversationItem.multimodalUserMessage.sourceMessageId",
                    )?,
                    turn_id: parse_turn_id(
                        read_text(
                            message.get_turn_id(),
                            "conversationItem.multimodalUserMessage.turnId",
                        )?,
                        "conversationItem.multimodalUserMessage.turnId",
                    )?,
                    ordinal: ItemOrdinal::new(message.get_ordinal()),
                    revision: Revision::new(message.get_revision()),
                    lifecycle: decode_conversation_lifecycle(message.get_lifecycle()?),
                    text,
                    attachments,
                    created_at: UnixMillis::from_millis(message.get_created_at_millis()),
                    updated_at: UnixMillis::from_millis(message.get_updated_at_millis()),
                },
            ))
        }
        conversation_item::Which::AssistantMessage(message) => {
            let message = message?;
            Ok(ConversationItem::AssistantMessage(AssistantMessageItem {
                item_id: parse_item_id(
                    read_text(
                        message.get_item_id(),
                        "conversationItem.assistantMessage.itemId",
                    )?,
                    "conversationItem.assistantMessage.itemId",
                )?,
                turn_id: parse_turn_id(
                    read_text(
                        message.get_turn_id(),
                        "conversationItem.assistantMessage.turnId",
                    )?,
                    "conversationItem.assistantMessage.turnId",
                )?,
                run_id: parse_run_id(
                    read_text(
                        message.get_run_id(),
                        "conversationItem.assistantMessage.runId",
                    )?,
                    "conversationItem.assistantMessage.runId",
                )?,
                ordinal: ItemOrdinal::new(message.get_ordinal()),
                revision: Revision::new(message.get_revision()),
                lifecycle: decode_conversation_lifecycle(message.get_lifecycle()?),
                body: AssistantBody::parse(read_text(
                    message.get_body(),
                    "conversationItem.assistantMessage.body",
                )?)
                .map_err(|source| ProtocolDecodeError::AssistantBody { source })?,
                phase: decode_assistant_message_phase(message.get_phase()?),
                created_at: UnixMillis::from_millis(message.get_created_at_millis()),
                updated_at: UnixMillis::from_millis(message.get_updated_at_millis()),
            }))
        }
        conversation_item::Which::Unmodeled(()) => {
            Err(ProtocolDecodeError::UnmodeledConversationItem)
        }
    }
}

fn decode_patch_batch(
    value: artisan_capnp::patch_batch::Reader<'_>,
) -> Result<PatchBatch, ProtocolDecodeError> {
    let patches = value.get_patches()?;
    let patch_count = patches.len() as usize;
    if patch_count > CONVERSATION_PATCH_BATCH_MAX_PATCHES {
        return Err(PatchBatchError::TooManyPatches {
            count: patch_count,
            maximum: CONVERSATION_PATCH_BATCH_MAX_PATCHES,
        }
        .into());
    }
    let patches = patches
        .iter()
        .map(decode_conversation_patch)
        .collect::<Result<Vec<_>, _>>()?;
    PatchBatch::new(
        parse_thread_id(
            read_text(value.get_thread_id(), "patchBatch.threadId")?,
            "patchBatch.threadId",
        )?,
        ConversationCursor::new(value.get_from_cursor()),
        ConversationCursor::new(value.get_to_cursor()),
        patches,
    )
    .map_err(ProtocolDecodeError::from)
}

fn decode_conversation_patch(
    value: artisan_capnp::conversation_patch::Reader<'_>,
) -> Result<ConversationPatch, ProtocolDecodeError> {
    let patch_id = parse_patch_id(
        read_text(value.get_patch_id(), "conversationPatch.patchId")?,
        "conversationPatch.patchId",
    )?;
    let sequence = PatchSequence::new(value.get_sequence()).map_err(|source| {
        ProtocolDecodeError::Counter {
            field: "conversationPatch.sequence",
            source,
        }
    })?;
    match value.which()? {
        conversation_patch::Which::TurnUpsert(turn) => Ok(ConversationPatch::TurnUpsert {
            patch_id,
            sequence,
            turn: decode_conversation_turn(turn?)?,
        }),
        conversation_patch::Which::ItemUpsert(item) => Ok(ConversationPatch::ItemUpsert {
            patch_id,
            sequence,
            item: decode_conversation_item(item?)?,
        }),
        conversation_patch::Which::ItemAppend(append) => {
            let append = append?;
            Ok(ConversationPatch::ItemAppend {
                patch_id,
                sequence,
                item_id: parse_item_id(
                    read_text(append.get_item_id(), "conversationPatch.itemAppend.itemId")?,
                    "conversationPatch.itemAppend.itemId",
                )?,
                revision: Revision::new(append.get_revision()),
                text: IncrementalText::parse(read_text(
                    append.get_text(),
                    "conversationPatch.itemAppend.text",
                )?)?,
                updated_at: UnixMillis::from_millis(append.get_updated_at_millis()),
            })
        }
        conversation_patch::Which::ItemLifecycle(transition) => {
            let transition = transition?;
            Ok(ConversationPatch::ItemLifecycle {
                patch_id,
                sequence,
                item_id: parse_item_id(
                    read_text(
                        transition.get_item_id(),
                        "conversationPatch.itemLifecycle.itemId",
                    )?,
                    "conversationPatch.itemLifecycle.itemId",
                )?,
                revision: Revision::new(transition.get_revision()),
                lifecycle: decode_conversation_lifecycle(transition.get_lifecycle()?),
                updated_at: UnixMillis::from_millis(transition.get_updated_at_millis()),
            })
        }
        conversation_patch::Which::TurnLifecycle(transition) => {
            let transition = transition?;
            Ok(ConversationPatch::TurnLifecycle {
                patch_id,
                sequence,
                turn_id: parse_turn_id(
                    read_text(
                        transition.get_turn_id(),
                        "conversationPatch.turnLifecycle.turnId",
                    )?,
                    "conversationPatch.turnLifecycle.turnId",
                )?,
                revision: Revision::new(transition.get_revision()),
                lifecycle: decode_conversation_lifecycle(transition.get_lifecycle()?),
                updated_at: UnixMillis::from_millis(transition.get_updated_at_millis()),
            })
        }
    }
}

const fn decode_conversation_lifecycle(
    value: artisan_capnp::ConversationLifecycle,
) -> ConversationLifecycle {
    match value {
        artisan_capnp::ConversationLifecycle::Pending => ConversationLifecycle::Pending,
        artisan_capnp::ConversationLifecycle::Streaming => ConversationLifecycle::Streaming,
        artisan_capnp::ConversationLifecycle::Active => ConversationLifecycle::Active,
        artisan_capnp::ConversationLifecycle::Waiting => ConversationLifecycle::Waiting,
        artisan_capnp::ConversationLifecycle::Completed => ConversationLifecycle::Completed,
        artisan_capnp::ConversationLifecycle::Failed => ConversationLifecycle::Failed,
        artisan_capnp::ConversationLifecycle::Interrupted => ConversationLifecycle::Interrupted,
        artisan_capnp::ConversationLifecycle::Cancelled => ConversationLifecycle::Cancelled,
    }
}

const fn decode_assistant_message_phase(
    value: artisan_capnp::AssistantMessagePhase,
) -> AssistantMessagePhase {
    match value {
        artisan_capnp::AssistantMessagePhase::Unspecified => AssistantMessagePhase::Unspecified,
        artisan_capnp::AssistantMessagePhase::Commentary => AssistantMessagePhase::Commentary,
        artisan_capnp::AssistantMessagePhase::Final => AssistantMessagePhase::Final,
    }
}

const fn encode_run_status(status: RunLiveStatus) -> artisan_capnp::RunStatus {
    match status {
        RunLiveStatus::Queued => artisan_capnp::RunStatus::Queued,
        RunLiveStatus::Running => artisan_capnp::RunStatus::Running,
        RunLiveStatus::Waiting => artisan_capnp::RunStatus::Waiting,
    }
}

/// Strict run-status decode: `unknown` is a typed failure, never a
/// tolerated state. The current backend always emits a live status with
/// its engine; native QUIC is a same-version build.
fn decode_run_status(
    value: artisan_capnp::RunStatus,
) -> Result<RunLiveStatus, ProtocolDecodeError> {
    match value {
        artisan_capnp::RunStatus::Queued => Ok(RunLiveStatus::Queued),
        artisan_capnp::RunStatus::Running => Ok(RunLiveStatus::Running),
        artisan_capnp::RunStatus::Waiting => Ok(RunLiveStatus::Waiting),
        artisan_capnp::RunStatus::Unknown => {
            Err(ProtocolDecodeError::UnknownDiscriminant { value: 0 })
        }
    }
}

const fn decode_disposition(value: artisan_capnp::ReceiptDisposition) -> ReceiptDisposition {
    match value {
        artisan_capnp::ReceiptDisposition::Accepted => ReceiptDisposition::Accepted,
        artisan_capnp::ReceiptDisposition::Duplicate => ReceiptDisposition::Duplicate,
    }
}

const fn decode_stop_run_disposition(
    value: artisan_capnp::StopRunDisposition,
) -> StopRunDisposition {
    match value {
        artisan_capnp::StopRunDisposition::Requested => StopRunDisposition::Requested,
        artisan_capnp::StopRunDisposition::AlreadyRequested => StopRunDisposition::AlreadyRequested,
        artisan_capnp::StopRunDisposition::NotActive => StopRunDisposition::NotActive,
    }
}

const fn decode_place_kind(value: artisan_capnp::PlaceKind) -> PlaceKind {
    match value {
        artisan_capnp::PlaceKind::Home => PlaceKind::Home,
        artisan_capnp::PlaceKind::Desktop => PlaceKind::Desktop,
        artisan_capnp::PlaceKind::Documents => PlaceKind::Documents,
        artisan_capnp::PlaceKind::Downloads => PlaceKind::Downloads,
        artisan_capnp::PlaceKind::Music => PlaceKind::Music,
        artisan_capnp::PlaceKind::Pictures => PlaceKind::Pictures,
        artisan_capnp::PlaceKind::Videos => PlaceKind::Videos,
    }
}

const fn decode_directory_kind(value: artisan_capnp::DirectoryEntryKind) -> DirectoryKind {
    match value {
        artisan_capnp::DirectoryEntryKind::Root => DirectoryKind::Root,
        artisan_capnp::DirectoryEntryKind::Directory => DirectoryKind::Directory,
    }
}

const fn decode_lifecycle_state(value: artisan_capnp::LifecycleState) -> LifecycleState {
    match value {
        artisan_capnp::LifecycleState::Ready => LifecycleState::Ready,
        artisan_capnp::LifecycleState::Busy => LifecycleState::Busy,
        artisan_capnp::LifecycleState::Draining => LifecycleState::Draining,
    }
}

const fn decode_lifecycle_stop_disposition(
    value: artisan_capnp::LifecycleStopDisposition,
) -> LifecycleStopDisposition {
    match value {
        artisan_capnp::LifecycleStopDisposition::Accepted => LifecycleStopDisposition::Accepted,
        artisan_capnp::LifecycleStopDisposition::Duplicate => LifecycleStopDisposition::Duplicate,
        artisan_capnp::LifecycleStopDisposition::AlreadyStopping => {
            LifecycleStopDisposition::AlreadyStopping
        }
    }
}

const fn decode_error_code(value: artisan_capnp::ErrorCode) -> ErrorCode {
    match value {
        artisan_capnp::ErrorCode::UnsupportedVersion => ErrorCode::UnsupportedVersion,
        artisan_capnp::ErrorCode::InvalidInput => ErrorCode::InvalidInput,
        artisan_capnp::ErrorCode::DirectoryUnknown => ErrorCode::DirectoryUnknown,
        artisan_capnp::ErrorCode::ProjectUnknown => ErrorCode::ProjectUnknown,
        artisan_capnp::ErrorCode::ThreadUnknown => ErrorCode::ThreadUnknown,
        artisan_capnp::ErrorCode::Internal => ErrorCode::Internal,
        artisan_capnp::ErrorCode::IdempotencyConflict => ErrorCode::IdempotencyConflict,
        artisan_capnp::ErrorCode::UnsupportedFeature => ErrorCode::UnsupportedFeature,
        artisan_capnp::ErrorCode::LifecycleBusy => ErrorCode::LifecycleBusy,
        artisan_capnp::ErrorCode::EngineConfigConflict => ErrorCode::EngineConfigConflict,
    }
}

// ---------------------------------------------------------------------------
// S1b: finite engine-observation delivery.
// ---------------------------------------------------------------------------
//
// Every observation below is an already-validated, sanitized S1a domain
// value. Encoding is infallible except where a collection must fit Cap'n
// Proto's 32-bit list length; decoding re-validates every bound through the
// domain constructors so a hostile peer can never smuggle an over-long,
// empty-required, unknown-label, or state-inconsistent row across the wire.

fn parse_observation_id(
    value: String,
    field: &'static str,
) -> Result<ObservationId, ProtocolDecodeError> {
    ObservationId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

fn parse_observation_sequence(value: u64) -> Result<ObservationSequence, ProtocolDecodeError> {
    ObservationSequence::new(value).map_err(ProtocolDecodeError::from)
}

/// Maps an empty wire string to an absent optional value.
///
/// Every optional text below rejects empty content at the domain boundary,
/// so empty decodes as absent without loss. Required texts never pass
/// through here: they go to the constructors, which reject emptiness.
fn absent_if_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

const fn encode_observation_message_phase(
    value: MessagePhase,
) -> artisan_capnp::ObservationMessagePhase {
    match value {
        MessagePhase::Commentary => artisan_capnp::ObservationMessagePhase::Commentary,
        MessagePhase::Final => artisan_capnp::ObservationMessagePhase::Final,
        MessagePhase::Unspecified => artisan_capnp::ObservationMessagePhase::Unspecified,
    }
}

const fn decode_observation_message_phase(
    value: artisan_capnp::ObservationMessagePhase,
) -> MessagePhase {
    match value {
        artisan_capnp::ObservationMessagePhase::Commentary => MessagePhase::Commentary,
        artisan_capnp::ObservationMessagePhase::Final => MessagePhase::Final,
        artisan_capnp::ObservationMessagePhase::Unspecified => MessagePhase::Unspecified,
    }
}

const fn encode_observation_tool_action(value: ToolAction) -> artisan_capnp::ObservationToolAction {
    match value {
        ToolAction::Started => artisan_capnp::ObservationToolAction::Started,
        ToolAction::Progress => artisan_capnp::ObservationToolAction::Progress,
        ToolAction::Completed => artisan_capnp::ObservationToolAction::Completed,
        ToolAction::Failed => artisan_capnp::ObservationToolAction::Failed,
    }
}

const fn decode_observation_tool_action(value: artisan_capnp::ObservationToolAction) -> ToolAction {
    match value {
        artisan_capnp::ObservationToolAction::Started => ToolAction::Started,
        artisan_capnp::ObservationToolAction::Progress => ToolAction::Progress,
        artisan_capnp::ObservationToolAction::Completed => ToolAction::Completed,
        artisan_capnp::ObservationToolAction::Failed => ToolAction::Failed,
    }
}

const fn encode_observation_file_action(value: FileAction) -> artisan_capnp::ObservationFileAction {
    match value {
        FileAction::Created => artisan_capnp::ObservationFileAction::Created,
        FileAction::Modified => artisan_capnp::ObservationFileAction::Modified,
        FileAction::Deleted => artisan_capnp::ObservationFileAction::Deleted,
        FileAction::Read => artisan_capnp::ObservationFileAction::Read,
    }
}

const fn decode_observation_file_action(value: artisan_capnp::ObservationFileAction) -> FileAction {
    match value {
        artisan_capnp::ObservationFileAction::Created => FileAction::Created,
        artisan_capnp::ObservationFileAction::Modified => FileAction::Modified,
        artisan_capnp::ObservationFileAction::Deleted => FileAction::Deleted,
        artisan_capnp::ObservationFileAction::Read => FileAction::Read,
    }
}

const fn encode_observation_search_scope(
    value: SearchScope,
) -> artisan_capnp::ObservationSearchScope {
    match value {
        SearchScope::Workspace => artisan_capnp::ObservationSearchScope::Workspace,
        SearchScope::Web => artisan_capnp::ObservationSearchScope::Web,
    }
}

const fn decode_observation_search_scope(
    value: artisan_capnp::ObservationSearchScope,
) -> SearchScope {
    match value {
        artisan_capnp::ObservationSearchScope::Workspace => SearchScope::Workspace,
        artisan_capnp::ObservationSearchScope::Web => SearchScope::Web,
    }
}

const fn encode_observation_search_state(
    value: SearchState,
) -> artisan_capnp::ObservationSearchState {
    match value {
        SearchState::Started => artisan_capnp::ObservationSearchState::Started,
        SearchState::Completed => artisan_capnp::ObservationSearchState::Completed,
    }
}

const fn decode_observation_search_state(
    value: artisan_capnp::ObservationSearchState,
) -> SearchState {
    match value {
        artisan_capnp::ObservationSearchState::Started => SearchState::Started,
        artisan_capnp::ObservationSearchState::Completed => SearchState::Completed,
    }
}

const fn encode_observation_terminal_channel(
    value: TerminalChannel,
) -> artisan_capnp::ObservationTerminalChannel {
    match value {
        TerminalChannel::Stdout => artisan_capnp::ObservationTerminalChannel::Stdout,
        TerminalChannel::Stderr => artisan_capnp::ObservationTerminalChannel::Stderr,
    }
}

const fn decode_observation_terminal_channel(
    value: artisan_capnp::ObservationTerminalChannel,
) -> TerminalChannel {
    match value {
        artisan_capnp::ObservationTerminalChannel::Stdout => TerminalChannel::Stdout,
        artisan_capnp::ObservationTerminalChannel::Stderr => TerminalChannel::Stderr,
    }
}

const fn encode_observation_terminal_state(
    value: TerminalActivityState,
) -> artisan_capnp::ObservationTerminalState {
    match value {
        TerminalActivityState::Started => artisan_capnp::ObservationTerminalState::Started,
        TerminalActivityState::Output => artisan_capnp::ObservationTerminalState::Output,
        TerminalActivityState::Completed => artisan_capnp::ObservationTerminalState::Completed,
        TerminalActivityState::Failed => artisan_capnp::ObservationTerminalState::Failed,
    }
}

const fn decode_observation_terminal_state(
    value: artisan_capnp::ObservationTerminalState,
) -> TerminalActivityState {
    match value {
        artisan_capnp::ObservationTerminalState::Started => TerminalActivityState::Started,
        artisan_capnp::ObservationTerminalState::Output => TerminalActivityState::Output,
        artisan_capnp::ObservationTerminalState::Completed => TerminalActivityState::Completed,
        artisan_capnp::ObservationTerminalState::Failed => TerminalActivityState::Failed,
    }
}

const fn encode_observation_approval_state(
    value: ApprovalState,
) -> artisan_capnp::ObservationApprovalState {
    match value {
        ApprovalState::Requested => artisan_capnp::ObservationApprovalState::Requested,
        ApprovalState::Resolved => artisan_capnp::ObservationApprovalState::Resolved,
    }
}

const fn decode_observation_approval_state(
    value: artisan_capnp::ObservationApprovalState,
) -> ApprovalState {
    match value {
        artisan_capnp::ObservationApprovalState::Requested => ApprovalState::Requested,
        artisan_capnp::ObservationApprovalState::Resolved => ApprovalState::Resolved,
    }
}

const fn encode_observation_approval_kind(
    value: ApprovalKind,
) -> artisan_capnp::ObservationApprovalKind {
    match value {
        ApprovalKind::Command => artisan_capnp::ObservationApprovalKind::Command,
        ApprovalKind::FileChange => artisan_capnp::ObservationApprovalKind::FileChange,
        ApprovalKind::Action => artisan_capnp::ObservationApprovalKind::Action,
    }
}

const fn decode_observation_approval_kind(
    value: artisan_capnp::ObservationApprovalKind,
) -> ApprovalKind {
    match value {
        artisan_capnp::ObservationApprovalKind::Command => ApprovalKind::Command,
        artisan_capnp::ObservationApprovalKind::FileChange => ApprovalKind::FileChange,
        artisan_capnp::ObservationApprovalKind::Action => ApprovalKind::Action,
    }
}

const fn encode_observation_question_state(
    value: QuestionState,
) -> artisan_capnp::ObservationQuestionState {
    match value {
        QuestionState::Requested => artisan_capnp::ObservationQuestionState::Requested,
        QuestionState::Resolved => artisan_capnp::ObservationQuestionState::Resolved,
    }
}

const fn decode_observation_question_state(
    value: artisan_capnp::ObservationQuestionState,
) -> QuestionState {
    match value {
        artisan_capnp::ObservationQuestionState::Requested => QuestionState::Requested,
        artisan_capnp::ObservationQuestionState::Resolved => QuestionState::Resolved,
    }
}

const fn encode_observation_plan_entry_status(
    value: PlanEntryStatus,
) -> artisan_capnp::ObservationPlanEntryStatus {
    match value {
        PlanEntryStatus::Pending => artisan_capnp::ObservationPlanEntryStatus::Pending,
        PlanEntryStatus::InProgress => artisan_capnp::ObservationPlanEntryStatus::InProgress,
        PlanEntryStatus::Completed => artisan_capnp::ObservationPlanEntryStatus::Completed,
    }
}

const fn decode_observation_plan_entry_status(
    value: artisan_capnp::ObservationPlanEntryStatus,
) -> PlanEntryStatus {
    match value {
        artisan_capnp::ObservationPlanEntryStatus::Pending => PlanEntryStatus::Pending,
        artisan_capnp::ObservationPlanEntryStatus::InProgress => PlanEntryStatus::InProgress,
        artisan_capnp::ObservationPlanEntryStatus::Completed => PlanEntryStatus::Completed,
    }
}

const fn encode_observation_compaction_state(
    value: CompactionState,
) -> artisan_capnp::ObservationCompactionState {
    match value {
        CompactionState::Started => artisan_capnp::ObservationCompactionState::Started,
        CompactionState::Completed => artisan_capnp::ObservationCompactionState::Completed,
    }
}

const fn decode_observation_compaction_state(
    value: artisan_capnp::ObservationCompactionState,
) -> CompactionState {
    match value {
        artisan_capnp::ObservationCompactionState::Started => CompactionState::Started,
        artisan_capnp::ObservationCompactionState::Completed => CompactionState::Completed,
    }
}

const fn encode_observation_retry_attempt_state(
    value: RetryAttemptState,
) -> artisan_capnp::ObservationRetryAttemptState {
    match value {
        RetryAttemptState::Retrying => artisan_capnp::ObservationRetryAttemptState::Retrying,
        RetryAttemptState::Terminal => artisan_capnp::ObservationRetryAttemptState::Terminal,
    }
}

const fn decode_observation_retry_attempt_state(
    value: artisan_capnp::ObservationRetryAttemptState,
) -> RetryAttemptState {
    match value {
        artisan_capnp::ObservationRetryAttemptState::Retrying => RetryAttemptState::Retrying,
        artisan_capnp::ObservationRetryAttemptState::Terminal => RetryAttemptState::Terminal,
    }
}

const fn encode_observation_run_state(value: RunState) -> artisan_capnp::ObservationRunState {
    match value {
        RunState::Opening => artisan_capnp::ObservationRunState::Opening,
        RunState::Running => artisan_capnp::ObservationRunState::Running,
        RunState::Waiting => artisan_capnp::ObservationRunState::Waiting,
    }
}

const fn decode_observation_run_state(value: artisan_capnp::ObservationRunState) -> RunState {
    match value {
        artisan_capnp::ObservationRunState::Opening => RunState::Opening,
        artisan_capnp::ObservationRunState::Running => RunState::Running,
        artisan_capnp::ObservationRunState::Waiting => RunState::Waiting,
    }
}

const fn encode_observation_turn_state(value: TurnState) -> artisan_capnp::ObservationTurnState {
    match value {
        TurnState::Started => artisan_capnp::ObservationTurnState::Started,
        TurnState::Waiting => artisan_capnp::ObservationTurnState::Waiting,
        TurnState::Completed => artisan_capnp::ObservationTurnState::Completed,
        TurnState::Cancelled => artisan_capnp::ObservationTurnState::Cancelled,
        TurnState::Failed => artisan_capnp::ObservationTurnState::Failed,
    }
}

const fn decode_observation_turn_state(value: artisan_capnp::ObservationTurnState) -> TurnState {
    match value {
        artisan_capnp::ObservationTurnState::Started => TurnState::Started,
        artisan_capnp::ObservationTurnState::Waiting => TurnState::Waiting,
        artisan_capnp::ObservationTurnState::Completed => TurnState::Completed,
        artisan_capnp::ObservationTurnState::Cancelled => TurnState::Cancelled,
        artisan_capnp::ObservationTurnState::Failed => TurnState::Failed,
    }
}

const fn encode_observation_subagent_state(
    value: SubagentState,
) -> artisan_capnp::ObservationSubagentState {
    match value {
        SubagentState::Discovered => artisan_capnp::ObservationSubagentState::Discovered,
        SubagentState::Running => artisan_capnp::ObservationSubagentState::Running,
        SubagentState::Waiting => artisan_capnp::ObservationSubagentState::Waiting,
        SubagentState::Completed => artisan_capnp::ObservationSubagentState::Completed,
        SubagentState::Failed => artisan_capnp::ObservationSubagentState::Failed,
        SubagentState::Interrupted => artisan_capnp::ObservationSubagentState::Interrupted,
    }
}

const fn decode_observation_subagent_state(
    value: artisan_capnp::ObservationSubagentState,
) -> SubagentState {
    match value {
        artisan_capnp::ObservationSubagentState::Discovered => SubagentState::Discovered,
        artisan_capnp::ObservationSubagentState::Running => SubagentState::Running,
        artisan_capnp::ObservationSubagentState::Waiting => SubagentState::Waiting,
        artisan_capnp::ObservationSubagentState::Completed => SubagentState::Completed,
        artisan_capnp::ObservationSubagentState::Failed => SubagentState::Failed,
        artisan_capnp::ObservationSubagentState::Interrupted => SubagentState::Interrupted,
    }
}

const fn encode_observation_usage_basis(value: UsageBasis) -> artisan_capnp::ObservationUsageBasis {
    match value {
        UsageBasis::Delta => artisan_capnp::ObservationUsageBasis::Delta,
        UsageBasis::Cumulative => artisan_capnp::ObservationUsageBasis::Cumulative,
        UsageBasis::Unknown => artisan_capnp::ObservationUsageBasis::Unknown,
    }
}

const fn decode_observation_usage_basis(value: artisan_capnp::ObservationUsageBasis) -> UsageBasis {
    match value {
        artisan_capnp::ObservationUsageBasis::Delta => UsageBasis::Delta,
        artisan_capnp::ObservationUsageBasis::Cumulative => UsageBasis::Cumulative,
        artisan_capnp::ObservationUsageBasis::Unknown => UsageBasis::Unknown,
    }
}

const fn encode_observation_diagnostic_level(
    value: DiagnosticLevel,
) -> artisan_capnp::ObservationDiagnosticLevel {
    match value {
        DiagnosticLevel::Info => artisan_capnp::ObservationDiagnosticLevel::Info,
        DiagnosticLevel::Warning => artisan_capnp::ObservationDiagnosticLevel::Warning,
        DiagnosticLevel::Error => artisan_capnp::ObservationDiagnosticLevel::Error,
    }
}

const fn decode_observation_diagnostic_level(
    value: artisan_capnp::ObservationDiagnosticLevel,
) -> DiagnosticLevel {
    match value {
        artisan_capnp::ObservationDiagnosticLevel::Info => DiagnosticLevel::Info,
        artisan_capnp::ObservationDiagnosticLevel::Warning => DiagnosticLevel::Warning,
        artisan_capnp::ObservationDiagnosticLevel::Error => DiagnosticLevel::Error,
    }
}

const fn encode_observation_run_terminal_state(
    value: RunTerminalState,
) -> artisan_capnp::ObservationRunTerminalState {
    match value {
        RunTerminalState::Completed => artisan_capnp::ObservationRunTerminalState::Completed,
        RunTerminalState::Cancelled => artisan_capnp::ObservationRunTerminalState::Cancelled,
        RunTerminalState::Failed => artisan_capnp::ObservationRunTerminalState::Failed,
        RunTerminalState::Interrupted => artisan_capnp::ObservationRunTerminalState::Interrupted,
        RunTerminalState::Closed => artisan_capnp::ObservationRunTerminalState::Closed,
    }
}

const fn decode_observation_run_terminal_state(
    value: artisan_capnp::ObservationRunTerminalState,
) -> RunTerminalState {
    match value {
        artisan_capnp::ObservationRunTerminalState::Completed => RunTerminalState::Completed,
        artisan_capnp::ObservationRunTerminalState::Cancelled => RunTerminalState::Cancelled,
        artisan_capnp::ObservationRunTerminalState::Failed => RunTerminalState::Failed,
        artisan_capnp::ObservationRunTerminalState::Interrupted => RunTerminalState::Interrupted,
        artisan_capnp::ObservationRunTerminalState::Closed => RunTerminalState::Closed,
    }
}

const fn encode_observation_limit_scope(value: LimitScope) -> artisan_capnp::ObservationLimitScope {
    match value {
        LimitScope::Shared => artisan_capnp::ObservationLimitScope::Shared,
        LimitScope::Model => artisan_capnp::ObservationLimitScope::Model,
        LimitScope::Unknown => artisan_capnp::ObservationLimitScope::Unknown,
    }
}

const fn decode_observation_limit_scope(value: artisan_capnp::ObservationLimitScope) -> LimitScope {
    match value {
        artisan_capnp::ObservationLimitScope::Shared => LimitScope::Shared,
        artisan_capnp::ObservationLimitScope::Model => LimitScope::Model,
        artisan_capnp::ObservationLimitScope::Unknown => LimitScope::Unknown,
    }
}

fn encode_engine_observation(
    mut builder: artisan_capnp::engine_observation::Builder<'_>,
    value: &Observation,
) -> Result<(), ProtocolEncodeError> {
    match value {
        Observation::AgentMessageDelta(observation) => {
            let mut encoded = builder.reborrow().init_agent_message_delta();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_item_id(observation.item_id().as_str());
            encoded.set_phase(encode_observation_message_phase(observation.phase()));
            encoded.set_delta(observation.delta());
            encoded.set_turn_id(observation.turn_id().as_str());
        }
        Observation::AgentMessageCompleted(observation) => {
            let mut encoded = builder.reborrow().init_agent_message_completed();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_item_id(observation.item_id().as_str());
            encoded.set_phase(encode_observation_message_phase(observation.phase()));
            encoded.set_message(observation.message());
            encoded.set_turn_id(observation.turn_id().as_str());
        }
        Observation::Approval(observation) => {
            let mut encoded = builder.reborrow().init_approval();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_approval_id(observation.approval_id().as_str());
            encoded.set_state(encode_observation_approval_state(observation.state()));
            encoded.set_description(observation.description());
            encode_observation_approval_request(
                encoded.reborrow().init_request(),
                observation.request(),
            );
            match observation.approved() {
                Some(decision) => {
                    encoded.reborrow().init_decision().set_decision(decision);
                }
                None => {
                    encoded.reborrow().init_decision().set_no_decision(());
                }
            }
        }
        Observation::Compaction(observation) => {
            let mut encoded = builder.reborrow().init_compaction();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_state(encode_observation_compaction_state(observation.state()));
            encoded.set_compaction_id(
                observation
                    .compaction_id()
                    .map(ObservationId::as_str)
                    .unwrap_or(""),
            );
            match observation.duration_ms() {
                Some(duration) => {
                    encoded
                        .reborrow()
                        .init_duration_ms()
                        .set_duration_ms(duration);
                }
                None => {
                    encoded.reborrow().init_duration_ms().set_no_duration_ms(());
                }
            }
            encoded.set_summary(observation.summary().unwrap_or(""));
        }
        Observation::File(observation) => {
            let mut encoded = builder.reborrow().init_file();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_path(observation.path());
            encoded.set_action(encode_observation_file_action(observation.action()));
            match observation.lines_added() {
                Some(count) => {
                    encoded.reborrow().init_lines_added().set_lines_added(count);
                }
                None => {
                    encoded.reborrow().init_lines_added().set_no_lines_added(());
                }
            }
            match observation.lines_deleted() {
                Some(count) => {
                    encoded
                        .reborrow()
                        .init_lines_deleted()
                        .set_lines_deleted(count);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_lines_deleted()
                        .set_no_lines_deleted(());
                }
            }
        }
        Observation::NativeAction(observation) => {
            let mut encoded = builder.reborrow().init_native_action();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_action(observation.action());
            encoded.set_detail(observation.detail().unwrap_or(""));
            encoded.set_diagnostic(observation.diagnostic());
            match observation.error_ref() {
                Some(error) => {
                    encode_observation_engine_error_ref(
                        encoded.reborrow().init_error_ref().init_error_ref(),
                        error,
                    );
                }
                None => {
                    encoded.reborrow().init_error_ref().set_no_error_ref(());
                }
            }
        }
        Observation::Plan(observation) => {
            let mut encoded = builder.reborrow().init_plan();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            let mut entries = encoded.reborrow().init_entries(list_length(
                "event.engineObservation.plan.entries",
                observation.entries().len(),
            )?);
            for (index, entry) in observation.entries().iter().enumerate() {
                let mut encoded_entry = entries
                    .reborrow()
                    .get(list_index("event.engineObservation.plan.entries", index)?);
                encoded_entry.set_id(entry.id().as_str());
                encoded_entry.set_status(encode_observation_plan_entry_status(entry.status()));
                encoded_entry.set_text(entry.text());
            }
            encoded.set_turn_id(
                observation
                    .turn_id()
                    .map(ObservationId::as_str)
                    .unwrap_or(""),
            );
        }
        Observation::ProcessDiagnostic(observation) => {
            let mut encoded = builder.reborrow().init_process_diagnostic();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_level(encode_observation_diagnostic_level(observation.level()));
            encoded.set_message(observation.message());
            match observation.error_ref() {
                Some(error) => {
                    encode_observation_engine_error_ref(
                        encoded.reborrow().init_error_ref().init_error_ref(),
                        error,
                    );
                }
                None => {
                    encoded.reborrow().init_error_ref().set_no_error_ref(());
                }
            }
        }
        Observation::ProtocolDiagnostic(observation) => {
            let mut encoded = builder.reborrow().init_protocol_diagnostic();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_level(encode_observation_diagnostic_level(observation.level()));
            encoded.set_message(observation.message());
        }
        Observation::Question(observation) => {
            let mut encoded = builder.reborrow().init_question();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_question_id(observation.question_id().as_str());
            encoded.set_state(encode_observation_question_state(observation.state()));
            encoded.set_text(observation.text());
            encoded.set_header(observation.header().unwrap_or(""));
            encoded.set_multi_select(observation.multi_select());
            let option_count = observation.options().map_or(0, Vec::len);
            let mut options = encoded.reborrow().init_options(list_length(
                "event.engineObservation.question.options",
                option_count,
            )?);
            if let Some(option_list) = observation.options() {
                for (index, option) in option_list.iter().enumerate() {
                    let mut encoded_option = options.reborrow().get(list_index(
                        "event.engineObservation.question.options",
                        index,
                    )?);
                    encoded_option.set_label(option.label());
                    encoded_option.set_description(option.description().unwrap_or(""));
                }
            }
            match observation.answers() {
                Some(answers) => {
                    let mut list = encoded.reborrow().init_answers().init_answers(list_length(
                        "event.engineObservation.question.answers",
                        answers.len(),
                    )?);
                    for (index, answer) in answers.iter().enumerate() {
                        list.set(
                            list_index("event.engineObservation.question.answers", index)?,
                            answer.as_str(),
                        );
                    }
                }
                None => {
                    encoded.reborrow().init_answers().set_no_answers(());
                }
            }
        }
        Observation::ReasoningSummaryCompleted(observation) => {
            let mut encoded = builder.reborrow().init_reasoning_summary_completed();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_item_id(observation.item_id().as_str());
            encoded.set_text(observation.text().unwrap_or(""));
            encoded.set_turn_id(observation.turn_id().as_str());
        }
        Observation::ReasoningSummaryDelta(observation) => {
            let mut encoded = builder.reborrow().init_reasoning_summary_delta();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_item_id(observation.item_id().as_str());
            encoded.set_summary_index(observation.summary_index());
            encoded.set_delta(observation.delta());
            match observation.thinking_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_thinking_tokens()
                        .set_thinking_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_thinking_tokens()
                        .set_no_thinking_tokens(());
                }
            }
            encoded.set_turn_id(observation.turn_id().as_str());
        }
        Observation::Retry(observation) => {
            let mut encoded = builder.reborrow().init_retry();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_turn_id(observation.turn_id().as_str());
            encoded.set_attempt_state(encode_observation_retry_attempt_state(
                observation.attempt_state(),
            ));
            encoded.set_will_retry(observation.will_retry());
            encoded.set_message(observation.message());
        }
        Observation::RunState(observation) => {
            let mut encoded = builder.reborrow().init_run_state();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_state(encode_observation_run_state(observation.state()));
        }
        Observation::RunTerminal(observation) => {
            let mut encoded = builder.reborrow().init_run_terminal();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_state(encode_observation_run_terminal_state(observation.state()));
            match observation.error_ref() {
                Some(error) => {
                    encode_observation_engine_error_ref(
                        encoded.reborrow().init_error_ref().init_error_ref(),
                        error,
                    );
                }
                None => {
                    encoded.reborrow().init_error_ref().set_no_error_ref(());
                }
            }
            encoded.set_summary_title(observation.summary_title().unwrap_or(""));
        }
        Observation::Search(observation) => {
            let mut encoded = builder.reborrow().init_search();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_query(observation.query());
            match observation.scope() {
                Some(scope) => {
                    encoded
                        .reborrow()
                        .init_scope()
                        .set_scope(encode_observation_search_scope(scope));
                }
                None => {
                    encoded.reborrow().init_scope().set_no_scope(());
                }
            }
            encoded.set_search_id(
                observation
                    .search_id()
                    .map(ObservationId::as_str)
                    .unwrap_or(""),
            );
            encoded.set_state(encode_observation_search_state(observation.state()));
            match observation.result_count() {
                Some(count) => {
                    encoded
                        .reborrow()
                        .init_result_count()
                        .set_result_count(count);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_result_count()
                        .set_no_result_count(());
                }
            }
        }
        Observation::Subagent(observation) => {
            let mut encoded = builder.reborrow().init_subagent();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_agent_native_thread_id(observation.agent_native_thread_id().as_str());
            encoded.set_parent_native_thread_id(observation.parent_native_thread_id().as_str());
            encoded.set_state(encode_observation_subagent_state(observation.state()));
            encoded.set_activity(observation.activity().unwrap_or(""));
            encoded.set_agent_path(observation.agent_path().unwrap_or(""));
            encoded.set_turn_id(
                observation
                    .turn_id()
                    .map(ObservationId::as_str)
                    .unwrap_or(""),
            );
        }
        Observation::SubagentTranscript(observation) => {
            let mut encoded = builder.reborrow().init_subagent_transcript();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_agent_native_thread_id(observation.agent_native_thread_id().as_str());
            encoded.set_parent_native_thread_id(observation.parent_native_thread_id().as_str());
            encode_transcript_content(encoded.reborrow().init_content(), observation.content());
        }
        Observation::TerminalActivity(observation) => {
            let mut encoded = builder.reborrow().init_terminal_activity();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_activity_id(observation.activity_id().as_str());
            match observation.channel() {
                Some(channel) => {
                    encoded
                        .reborrow()
                        .init_channel()
                        .set_channel(encode_observation_terminal_channel(channel));
                }
                None => {
                    encoded.reborrow().init_channel().set_no_channel(());
                }
            }
            encoded.set_command(observation.command().unwrap_or(""));
            encoded.set_shell(observation.shell().unwrap_or(""));
            match observation.output() {
                Some(output) => {
                    encoded.reborrow().init_output().set_output(output);
                }
                None => {
                    encoded.reborrow().init_output().set_no_output(());
                }
            }
            match observation.exit_code() {
                Some(code) => {
                    encoded.reborrow().init_exit_code().set_exit_code(code);
                }
                None => {
                    encoded.reborrow().init_exit_code().set_no_exit_code(());
                }
            }
            encoded.set_state(encode_observation_terminal_state(observation.state()));
        }
        Observation::Tool(observation) => {
            let mut encoded = builder.reborrow().init_tool();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_tool_id(observation.tool_id().as_str());
            encoded.set_tool_name(observation.tool_name());
            encoded.set_action(encode_observation_tool_action(observation.action()));
            encoded.set_detail(observation.detail().unwrap_or(""));
        }
        Observation::TurnState(observation) => {
            let mut encoded = builder.reborrow().init_turn_state();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_turn_id(observation.turn_id().as_str());
            encoded.set_state(encode_observation_turn_state(observation.state()));
        }
        Observation::Usage(observation) => {
            let mut encoded = builder.reborrow().init_usage();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_basis(encode_observation_usage_basis(observation.basis()));
            match observation.input_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_input_tokens()
                        .set_input_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_input_tokens()
                        .set_no_input_tokens(());
                }
            }
            match observation.cached_input_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_cached_input_tokens()
                        .set_cached_input_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_cached_input_tokens()
                        .set_no_cached_input_tokens(());
                }
            }
            match observation.output_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_output_tokens()
                        .set_output_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_output_tokens()
                        .set_no_output_tokens(());
                }
            }
            match observation.context_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_context_tokens()
                        .set_context_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_context_tokens()
                        .set_no_context_tokens(());
                }
            }
            encoded.set_context_window_tokens(observation.context_window_tokens().unwrap_or(0));
            match observation.cost_usd() {
                Some(cost) => {
                    encoded.reborrow().init_cost().set_cost(cost);
                }
                None => {
                    encoded.reborrow().init_cost().set_no_cost(());
                }
            }
            encoded.set_provider_route_id(
                observation
                    .provider_route_id()
                    .map(ObservationId::as_str)
                    .unwrap_or(""),
            );
            encoded.set_turn_id(
                observation
                    .turn_id()
                    .map(ObservationId::as_str)
                    .unwrap_or(""),
            );
        }
    }
    Ok(())
}

fn encode_observation_approval_request(
    mut builder: artisan_capnp::observation_approval_request::Builder<'_>,
    value: &ApprovalRequest,
) {
    builder.set_kind(encode_observation_approval_kind(value.kind()));
    builder.set_command(value.command_text().unwrap_or(""));
    builder.set_cwd(value.cwd().unwrap_or(""));
    builder.set_reason(value.reason().unwrap_or(""));
}

fn encode_observation_engine_error_ref(
    mut builder: artisan_capnp::observation_engine_error_ref::Builder<'_>,
    value: &EngineErrorRef,
) {
    builder.set_artisan_code(value.artisan_code().as_str());
    builder.set_provider_code(value.provider_code().unwrap_or(""));
    builder.set_detail(value.detail().unwrap_or(""));
    builder.set_affected_model_id(value.affected_model_id().unwrap_or(""));
    builder.set_limit_id(value.limit_id().unwrap_or(""));
    builder.set_limit_label(value.limit_label().unwrap_or(""));
    match value.limit_scope() {
        Some(scope) => {
            builder
                .reborrow()
                .init_limit_scope()
                .set_limit_scope(encode_observation_limit_scope(scope));
        }
        None => {
            builder.reborrow().init_limit_scope().set_no_limit_scope(());
        }
    }
    builder.set_resets_at(value.resets_at().unwrap_or(""));
}

fn encode_transcript_content(
    builder: artisan_capnp::observation_subagent_transcript_content::Builder<'_>,
    value: &TranscriptContent,
) {
    match value {
        TranscriptContent::AgentMessageDelta(content) => {
            let mut encoded = builder.init_agent_message_delta();
            encoded.set_item_id(content.item_id().as_str());
            encoded.set_phase(encode_observation_message_phase(content.phase()));
            encoded.set_delta(content.delta());
        }
        TranscriptContent::AgentMessageCompleted(content) => {
            let mut encoded = builder.init_agent_message_completed();
            encoded.set_item_id(content.item_id().as_str());
            encoded.set_phase(encode_observation_message_phase(content.phase()));
            encoded.set_message(content.message());
        }
        TranscriptContent::ReasoningSummaryDelta(content) => {
            let mut encoded = builder.init_reasoning_summary_delta();
            encoded.set_item_id(content.item_id().as_str());
            encoded.set_summary_index(content.summary_index());
            encoded.set_delta(content.delta());
        }
        TranscriptContent::ReasoningSummaryCompleted(content) => {
            let mut encoded = builder.init_reasoning_summary_completed();
            encoded.set_item_id(content.item_id().as_str());
            encoded.set_text(content.text().unwrap_or(""));
        }
        TranscriptContent::TerminalActivity(content) => {
            let mut encoded = builder.init_terminal_activity();
            encoded.set_activity_id(content.activity_id().as_str());
            match content.channel() {
                Some(channel) => {
                    encoded
                        .reborrow()
                        .init_channel()
                        .set_channel(encode_observation_terminal_channel(channel));
                }
                None => {
                    encoded.reborrow().init_channel().set_no_channel(());
                }
            }
            encoded.set_command(content.command().unwrap_or(""));
            match content.exit_code() {
                Some(code) => {
                    encoded.reborrow().init_exit_code().set_exit_code(code);
                }
                None => {
                    encoded.reborrow().init_exit_code().set_no_exit_code(());
                }
            }
            match content.output() {
                Some(output) => {
                    encoded.reborrow().init_output().set_output(output);
                }
                None => {
                    encoded.reborrow().init_output().set_no_output(());
                }
            }
            encoded.set_state(encode_observation_terminal_state(content.state()));
        }
        TranscriptContent::Tool(content) => {
            let mut encoded = builder.init_tool();
            encoded.set_tool_id(content.tool_id().as_str());
            encoded.set_tool_name(content.tool_name());
            encoded.set_action(encode_observation_tool_action(content.action()));
            encoded.set_detail(content.detail().unwrap_or(""));
        }
        TranscriptContent::File(content) => {
            let mut encoded = builder.init_file();
            encoded.set_path(content.path());
            encoded.set_action(encode_observation_file_action(content.action()));
            match content.lines_added() {
                Some(count) => {
                    encoded.reborrow().init_lines_added().set_lines_added(count);
                }
                None => {
                    encoded.reborrow().init_lines_added().set_no_lines_added(());
                }
            }
            match content.lines_deleted() {
                Some(count) => {
                    encoded
                        .reborrow()
                        .init_lines_deleted()
                        .set_lines_deleted(count);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_lines_deleted()
                        .set_no_lines_deleted(());
                }
            }
        }
        TranscriptContent::Search(content) => {
            let mut encoded = builder.init_search();
            encoded.set_query(content.query());
            match content.result_count() {
                Some(count) => {
                    encoded
                        .reborrow()
                        .init_result_count()
                        .set_result_count(count);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_result_count()
                        .set_no_result_count(());
                }
            }
            match content.scope() {
                Some(scope) => {
                    encoded
                        .reborrow()
                        .init_scope()
                        .set_scope(encode_observation_search_scope(scope));
                }
                None => {
                    encoded.reborrow().init_scope().set_no_scope(());
                }
            }
            encoded.set_search_id(content.search_id().map(ObservationId::as_str).unwrap_or(""));
            encoded.set_state(encode_observation_search_state(content.state()));
        }
    }
}

fn decode_engine_observation(
    value: artisan_capnp::engine_observation::Reader<'_>,
) -> Result<Observation, ProtocolDecodeError> {
    match value.which()? {
        artisan_capnp::engine_observation::Which::AgentMessageDelta(observation) => {
            let observation = observation?;
            Ok(Observation::AgentMessageDelta(
                AgentMessageDeltaObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.agentMessageDelta.id",
                        )?,
                        "event.engineObservation.agentMessageDelta.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_item_id(),
                            "event.engineObservation.agentMessageDelta.itemId",
                        )?,
                        "event.engineObservation.agentMessageDelta.itemId",
                    )?,
                    decode_observation_message_phase(observation.get_phase()?),
                    read_text(
                        observation.get_delta(),
                        "event.engineObservation.agentMessageDelta.delta",
                    )?,
                    parse_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.agentMessageDelta.turnId",
                        )?,
                        "event.engineObservation.agentMessageDelta.turnId",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::AgentMessageCompleted(observation) => {
            let observation = observation?;
            Ok(Observation::AgentMessageCompleted(
                AgentMessageCompletedObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.agentMessageCompleted.id",
                        )?,
                        "event.engineObservation.agentMessageCompleted.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_item_id(),
                            "event.engineObservation.agentMessageCompleted.itemId",
                        )?,
                        "event.engineObservation.agentMessageCompleted.itemId",
                    )?,
                    decode_observation_message_phase(observation.get_phase()?),
                    read_text(
                        observation.get_message(),
                        "event.engineObservation.agentMessageCompleted.message",
                    )?,
                    parse_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.agentMessageCompleted.turnId",
                        )?,
                        "event.engineObservation.agentMessageCompleted.turnId",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::Approval(observation) => {
            let observation = observation?;
            let id = parse_observation_id(
                read_text(observation.get_id(), "event.engineObservation.approval.id")?,
                "event.engineObservation.approval.id",
            )?;
            let sequence = parse_observation_sequence(observation.get_sequence())?;
            let approval_id = parse_observation_id(
                read_text(
                    observation.get_approval_id(),
                    "event.engineObservation.approval.approvalId",
                )?,
                "event.engineObservation.approval.approvalId",
            )?;
            let state = decode_observation_approval_state(observation.get_state()?);
            let description = read_text(
                observation.get_description(),
                "event.engineObservation.approval.description",
            )?;
            let request = decode_observation_approval_request(observation.get_request()?)?;
            let decision = match observation.get_decision().which()? {
                artisan_capnp::observation_approval::decision::Which::NoDecision(()) => None,
                artisan_capnp::observation_approval::decision::Which::Decision(decision) => {
                    Some(decision)
                }
            };
            match (state, decision) {
                (ApprovalState::Requested, None) => {
                    Ok(Observation::Approval(ApprovalObservation::requested(
                        id,
                        sequence,
                        approval_id,
                        description,
                        request,
                    )?))
                }
                (ApprovalState::Resolved, Some(approved)) => {
                    Ok(Observation::Approval(ApprovalObservation::resolved(
                        id,
                        sequence,
                        approval_id,
                        description,
                        request,
                        approved,
                    )?))
                }
                (ApprovalState::Requested, Some(_)) => {
                    Err(ObservationError::UnexpectedField { field: "decision" }.into())
                }
                (ApprovalState::Resolved, None) => {
                    Err(ObservationError::MissingField { field: "decision" }.into())
                }
            }
        }
        artisan_capnp::engine_observation::Which::Compaction(observation) => {
            let observation = observation?;
            Ok(Observation::Compaction(CompactionObservation::new(
                parse_observation_id(
                    read_text(
                        observation.get_id(),
                        "event.engineObservation.compaction.id",
                    )?,
                    "event.engineObservation.compaction.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                decode_observation_compaction_state(observation.get_state()?),
                parse_optional_observation_id(
                    read_text(
                        observation.get_compaction_id(),
                        "event.engineObservation.compaction.compactionId",
                    )?,
                    "event.engineObservation.compaction.compactionId",
                )?,
                match observation.get_duration_ms().which()? {
                    artisan_capnp::observation_compaction::duration_ms::Which::NoDurationMs(()) => {
                        None
                    }
                    artisan_capnp::observation_compaction::duration_ms::Which::DurationMs(
                        duration,
                    ) => Some(duration),
                },
                absent_if_empty(read_text(
                    observation.get_summary(),
                    "event.engineObservation.compaction.summary",
                )?),
            )?))
        }
        artisan_capnp::engine_observation::Which::File(observation) => {
            let observation = observation?;
            Ok(Observation::File(FileObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.file.id")?,
                    "event.engineObservation.file.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                read_text(observation.get_path(), "event.engineObservation.file.path")?,
                decode_observation_file_action(observation.get_action()?),
                match observation.get_lines_added().which()? {
                    artisan_capnp::observation_file::lines_added::Which::NoLinesAdded(()) => None,
                    artisan_capnp::observation_file::lines_added::Which::LinesAdded(count) => {
                        Some(count)
                    }
                },
                match observation.get_lines_deleted().which()? {
                    artisan_capnp::observation_file::lines_deleted::Which::NoLinesDeleted(()) => {
                        None
                    }
                    artisan_capnp::observation_file::lines_deleted::Which::LinesDeleted(count) => {
                        Some(count)
                    }
                },
            )?))
        }
        artisan_capnp::engine_observation::Which::NativeAction(observation) => {
            let observation = observation?;
            Ok(Observation::NativeAction(NativeActionObservation::new(
                parse_observation_id(
                    read_text(
                        observation.get_id(),
                        "event.engineObservation.nativeAction.id",
                    )?,
                    "event.engineObservation.nativeAction.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                read_text(
                    observation.get_action(),
                    "event.engineObservation.nativeAction.action",
                )?,
                absent_if_empty(read_text(
                    observation.get_detail(),
                    "event.engineObservation.nativeAction.detail",
                )?),
                observation.get_diagnostic(),
                match observation.get_error_ref().which()? {
                    artisan_capnp::observation_native_action::error_ref::Which::NoErrorRef(()) => {
                        None
                    }
                    artisan_capnp::observation_native_action::error_ref::Which::ErrorRef(error) => {
                        Some(decode_observation_engine_error_ref(
                            error?,
                            "event.engineObservation.nativeAction.errorRef",
                        )?)
                    }
                },
            )?))
        }
        artisan_capnp::engine_observation::Which::Plan(observation) => {
            let observation = observation?;
            let encoded_entries = observation.get_entries()?;
            let entry_count = encoded_entries.len() as usize;
            if entry_count > OBSERVATION_PLAN_MAX_ENTRIES {
                return Err(ProtocolDecodeError::Observation {
                    source: ObservationError::TooMany {
                        field: "entries",
                        count: entry_count,
                        maximum: OBSERVATION_PLAN_MAX_ENTRIES,
                    },
                });
            }
            let mut entries = Vec::with_capacity(entry_count);
            for encoded_entry in encoded_entries.iter() {
                entries.push(PlanEntry::new(
                    parse_observation_id(
                        read_text(
                            encoded_entry.get_id(),
                            "event.engineObservation.plan.entries.id",
                        )?,
                        "event.engineObservation.plan.entries.id",
                    )?,
                    decode_observation_plan_entry_status(encoded_entry.get_status()?),
                    read_text(
                        encoded_entry.get_text(),
                        "event.engineObservation.plan.entries.text",
                    )?,
                )?);
            }
            Ok(Observation::Plan(PlanObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.plan.id")?,
                    "event.engineObservation.plan.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                entries,
                parse_optional_observation_id(
                    read_text(
                        observation.get_turn_id(),
                        "event.engineObservation.plan.turnId",
                    )?,
                    "event.engineObservation.plan.turnId",
                )?,
            )?))
        }
        artisan_capnp::engine_observation::Which::ProcessDiagnostic(observation) => {
            let observation = observation?;
            Ok(Observation::ProcessDiagnostic(
                ProcessDiagnosticObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.processDiagnostic.id",
                        )?,
                        "event.engineObservation.processDiagnostic.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    decode_observation_diagnostic_level(observation.get_level()?),
                    read_text(
                        observation.get_message(),
                        "event.engineObservation.processDiagnostic.message",
                    )?,
                    match observation.get_error_ref().which()? {
                        artisan_capnp::observation_process_diagnostic::error_ref::Which::NoErrorRef(
                            (),
                        ) => None,
                        artisan_capnp::observation_process_diagnostic::error_ref::Which::ErrorRef(
                            error,
                        ) => Some(decode_observation_engine_error_ref(
                            error?,
                            "event.engineObservation.processDiagnostic.errorRef",
                        )?),
                    },
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::ProtocolDiagnostic(observation) => {
            let observation = observation?;
            Ok(Observation::ProtocolDiagnostic(
                ProtocolDiagnosticObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.protocolDiagnostic.id",
                        )?,
                        "event.engineObservation.protocolDiagnostic.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    decode_observation_diagnostic_level(observation.get_level()?),
                    read_text(
                        observation.get_message(),
                        "event.engineObservation.protocolDiagnostic.message",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::Question(observation) => {
            let observation = observation?;
            let id = parse_observation_id(
                read_text(observation.get_id(), "event.engineObservation.question.id")?,
                "event.engineObservation.question.id",
            )?;
            let sequence = parse_observation_sequence(observation.get_sequence())?;
            let question_id = parse_observation_id(
                read_text(
                    observation.get_question_id(),
                    "event.engineObservation.question.questionId",
                )?,
                "event.engineObservation.question.questionId",
            )?;
            let state = decode_observation_question_state(observation.get_state()?);
            let text = read_text(
                observation.get_text(),
                "event.engineObservation.question.text",
            )?;
            let header = absent_if_empty(read_text(
                observation.get_header(),
                "event.engineObservation.question.header",
            )?);
            let multi_select = observation.get_multi_select();
            let encoded_options = observation.get_options()?;
            let option_count = encoded_options.len() as usize;
            if option_count > OBSERVATION_QUESTION_MAX_OPTIONS {
                return Err(ProtocolDecodeError::Observation {
                    source: ObservationError::TooMany {
                        field: "options",
                        count: option_count,
                        maximum: OBSERVATION_QUESTION_MAX_OPTIONS,
                    },
                });
            }
            let options = if option_count == 0 {
                None
            } else {
                let mut options = Vec::with_capacity(option_count);
                for encoded_option in encoded_options.iter() {
                    options.push(QuestionOption::new(
                        read_text(
                            encoded_option.get_label(),
                            "event.engineObservation.question.options.label",
                        )?,
                        absent_if_empty(read_text(
                            encoded_option.get_description(),
                            "event.engineObservation.question.options.description",
                        )?),
                    )?);
                }
                Some(options)
            };
            let answers = match observation.get_answers().which()? {
                artisan_capnp::observation_question::answers::Which::NoAnswers(()) => None,
                artisan_capnp::observation_question::answers::Which::Answers(encoded) => {
                    let encoded = encoded?;
                    let answer_count = encoded.len() as usize;
                    if answer_count > OBSERVATION_ANSWERS_MAX {
                        return Err(ProtocolDecodeError::Observation {
                            source: ObservationError::TooMany {
                                field: "answers",
                                count: answer_count,
                                maximum: OBSERVATION_ANSWERS_MAX,
                            },
                        });
                    }
                    let mut answers = Vec::with_capacity(answer_count);
                    for answer in encoded.iter() {
                        answers.push(read_text(
                            answer,
                            "event.engineObservation.question.answers",
                        )?);
                    }
                    Some(answers)
                }
            };
            let input = QuestionInput {
                question_id,
                text,
                header,
                multi_select,
                options,
            };
            match (state, answers) {
                (QuestionState::Requested, None) => Ok(Observation::Question(
                    QuestionObservation::requested(id, sequence, input)?,
                )),
                (QuestionState::Resolved, Some(answers)) => Ok(Observation::Question(
                    QuestionObservation::resolved(id, sequence, input, answers)?,
                )),
                (QuestionState::Requested, Some(_)) => {
                    Err(ObservationError::UnexpectedField { field: "answers" }.into())
                }
                (QuestionState::Resolved, None) => {
                    Err(ObservationError::MissingField { field: "answers" }.into())
                }
            }
        }
        artisan_capnp::engine_observation::Which::ReasoningSummaryCompleted(observation) => {
            let observation = observation?;
            Ok(Observation::ReasoningSummaryCompleted(
                ReasoningSummaryCompletedObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.reasoningSummaryCompleted.id",
                        )?,
                        "event.engineObservation.reasoningSummaryCompleted.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_item_id(),
                            "event.engineObservation.reasoningSummaryCompleted.itemId",
                        )?,
                        "event.engineObservation.reasoningSummaryCompleted.itemId",
                    )?,
                    absent_if_empty(read_text(
                        observation.get_text(),
                        "event.engineObservation.reasoningSummaryCompleted.text",
                    )?),
                    parse_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.reasoningSummaryCompleted.turnId",
                        )?,
                        "event.engineObservation.reasoningSummaryCompleted.turnId",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::ReasoningSummaryDelta(observation) => {
            let observation = observation?;
            Ok(Observation::ReasoningSummaryDelta(
                ReasoningSummaryDeltaObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.reasoningSummaryDelta.id",
                        )?,
                        "event.engineObservation.reasoningSummaryDelta.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_item_id(),
                            "event.engineObservation.reasoningSummaryDelta.itemId",
                        )?,
                        "event.engineObservation.reasoningSummaryDelta.itemId",
                    )?,
                    observation.get_summary_index(),
                    read_text(
                        observation.get_delta(),
                        "event.engineObservation.reasoningSummaryDelta.delta",
                    )?,
                    match observation.get_thinking_tokens().which()? {
                        artisan_capnp::observation_reasoning_summary_delta::thinking_tokens::Which::NoThinkingTokens(
                            (),
                        ) => None,
                        artisan_capnp::observation_reasoning_summary_delta::thinking_tokens::Which::ThinkingTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    parse_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.reasoningSummaryDelta.turnId",
                        )?,
                        "event.engineObservation.reasoningSummaryDelta.turnId",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::Retry(observation) => {
            let observation = observation?;
            Ok(Observation::Retry(RetryObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.retry.id")?,
                    "event.engineObservation.retry.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                parse_observation_id(
                    read_text(
                        observation.get_turn_id(),
                        "event.engineObservation.retry.turnId",
                    )?,
                    "event.engineObservation.retry.turnId",
                )?,
                decode_observation_retry_attempt_state(observation.get_attempt_state()?),
                observation.get_will_retry(),
                read_text(
                    observation.get_message(),
                    "event.engineObservation.retry.message",
                )?,
            )?))
        }
        artisan_capnp::engine_observation::Which::RunState(observation) => {
            let observation = observation?;
            Ok(Observation::RunState(RunStateObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.runState.id")?,
                    "event.engineObservation.runState.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                decode_observation_run_state(observation.get_state()?),
            )))
        }
        artisan_capnp::engine_observation::Which::RunTerminal(observation) => {
            let observation = observation?;
            Ok(Observation::RunTerminal(RunTerminalObservation::new(
                parse_observation_id(
                    read_text(
                        observation.get_id(),
                        "event.engineObservation.runTerminal.id",
                    )?,
                    "event.engineObservation.runTerminal.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                decode_observation_run_terminal_state(observation.get_state()?),
                match observation.get_error_ref().which()? {
                    artisan_capnp::observation_run_terminal::error_ref::Which::NoErrorRef(()) => {
                        None
                    }
                    artisan_capnp::observation_run_terminal::error_ref::Which::ErrorRef(error) => {
                        Some(decode_observation_engine_error_ref(
                            error?,
                            "event.engineObservation.runTerminal.errorRef",
                        )?)
                    }
                },
                absent_if_empty(read_text(
                    observation.get_summary_title(),
                    "event.engineObservation.runTerminal.summaryTitle",
                )?),
            )?))
        }
        artisan_capnp::engine_observation::Which::Search(observation) => {
            let observation = observation?;
            Ok(Observation::Search(SearchObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.search.id")?,
                    "event.engineObservation.search.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                read_text(
                    observation.get_query(),
                    "event.engineObservation.search.query",
                )?,
                match observation.get_scope().which()? {
                    artisan_capnp::observation_search::scope::Which::NoScope(()) => None,
                    artisan_capnp::observation_search::scope::Which::Scope(scope) => {
                        Some(decode_observation_search_scope(scope?))
                    }
                },
                parse_optional_observation_id(
                    read_text(
                        observation.get_search_id(),
                        "event.engineObservation.search.searchId",
                    )?,
                    "event.engineObservation.search.searchId",
                )?,
                decode_observation_search_state(observation.get_state()?),
                match observation.get_result_count().which()? {
                    artisan_capnp::observation_search::result_count::Which::NoResultCount(()) => {
                        None
                    }
                    artisan_capnp::observation_search::result_count::Which::ResultCount(count) => {
                        Some(count)
                    }
                },
            )?))
        }
        artisan_capnp::engine_observation::Which::Subagent(observation) => {
            let observation = observation?;
            Ok(Observation::Subagent(SubagentObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.subagent.id")?,
                    "event.engineObservation.subagent.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                SubagentInput {
                    agent_native_thread_id: parse_observation_id(
                        read_text(
                            observation.get_agent_native_thread_id(),
                            "event.engineObservation.subagent.agentNativeThreadId",
                        )?,
                        "event.engineObservation.subagent.agentNativeThreadId",
                    )?,
                    parent_native_thread_id: parse_observation_id(
                        read_text(
                            observation.get_parent_native_thread_id(),
                            "event.engineObservation.subagent.parentNativeThreadId",
                        )?,
                        "event.engineObservation.subagent.parentNativeThreadId",
                    )?,
                    state: decode_observation_subagent_state(observation.get_state()?),
                    activity: absent_if_empty(read_text(
                        observation.get_activity(),
                        "event.engineObservation.subagent.activity",
                    )?),
                    agent_path: absent_if_empty(read_text(
                        observation.get_agent_path(),
                        "event.engineObservation.subagent.agentPath",
                    )?),
                    turn_id: parse_optional_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.subagent.turnId",
                        )?,
                        "event.engineObservation.subagent.turnId",
                    )?,
                },
            )?))
        }
        artisan_capnp::engine_observation::Which::SubagentTranscript(observation) => {
            let observation = observation?;
            Ok(Observation::SubagentTranscript(
                SubagentTranscriptObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.subagentTranscript.id",
                        )?,
                        "event.engineObservation.subagentTranscript.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_agent_native_thread_id(),
                            "event.engineObservation.subagentTranscript.agentNativeThreadId",
                        )?,
                        "event.engineObservation.subagentTranscript.agentNativeThreadId",
                    )?,
                    parse_observation_id(
                        read_text(
                            observation.get_parent_native_thread_id(),
                            "event.engineObservation.subagentTranscript.parentNativeThreadId",
                        )?,
                        "event.engineObservation.subagentTranscript.parentNativeThreadId",
                    )?,
                    decode_transcript_content(observation.get_content()?)?,
                ),
            ))
        }
        artisan_capnp::engine_observation::Which::TerminalActivity(observation) => {
            let observation = observation?;
            Ok(Observation::TerminalActivity(
                TerminalActivityObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.terminalActivity.id",
                        )?,
                        "event.engineObservation.terminalActivity.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    TerminalActivityInput {
                        activity_id: parse_observation_id(
                            read_text(
                                observation.get_activity_id(),
                                "event.engineObservation.terminalActivity.activityId",
                            )?,
                            "event.engineObservation.terminalActivity.activityId",
                        )?,
                        channel: match observation.get_channel().which()? {
                            artisan_capnp::observation_terminal_activity::channel::Which::NoChannel(
                                (),
                            ) => None,
                            artisan_capnp::observation_terminal_activity::channel::Which::Channel(
                                channel,
                            ) => Some(decode_observation_terminal_channel(channel?)),
                        },
                        command: absent_if_empty(read_text(
                            observation.get_command(),
                            "event.engineObservation.terminalActivity.command",
                        )?),
                        shell: absent_if_empty(read_text(
                            observation.get_shell(),
                            "event.engineObservation.terminalActivity.shell",
                        )?),
                        output: match observation.get_output().which()? {
                            artisan_capnp::observation_terminal_activity::output::Which::NoOutput(
                                (),
                            ) => None,
                            artisan_capnp::observation_terminal_activity::output::Which::Output(
                                output,
                            ) => Some(read_text(
                                output,
                                "event.engineObservation.terminalActivity.output",
                            )?),
                        },
                        exit_code: match observation.get_exit_code().which()? {
                            artisan_capnp::observation_terminal_activity::exit_code::Which::NoExitCode(
                                (),
                            ) => None,
                            artisan_capnp::observation_terminal_activity::exit_code::Which::ExitCode(
                                code,
                            ) => Some(code),
                        },
                        state: decode_observation_terminal_state(observation.get_state()?),
                    },
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::Tool(observation) => {
            let observation = observation?;
            Ok(Observation::Tool(ToolObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.tool.id")?,
                    "event.engineObservation.tool.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                parse_observation_id(
                    read_text(
                        observation.get_tool_id(),
                        "event.engineObservation.tool.toolId",
                    )?,
                    "event.engineObservation.tool.toolId",
                )?,
                read_text(
                    observation.get_tool_name(),
                    "event.engineObservation.tool.toolName",
                )?,
                decode_observation_tool_action(observation.get_action()?),
                absent_if_empty(read_text(
                    observation.get_detail(),
                    "event.engineObservation.tool.detail",
                )?),
            )?))
        }
        artisan_capnp::engine_observation::Which::TurnState(observation) => {
            let observation = observation?;
            Ok(Observation::TurnState(TurnStateObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.turnState.id")?,
                    "event.engineObservation.turnState.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                parse_observation_id(
                    read_text(
                        observation.get_turn_id(),
                        "event.engineObservation.turnState.turnId",
                    )?,
                    "event.engineObservation.turnState.turnId",
                )?,
                decode_observation_turn_state(observation.get_state()?),
            )))
        }
        artisan_capnp::engine_observation::Which::Usage(observation) => {
            let observation = observation?;
            Ok(Observation::Usage(UsageObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.usage.id")?,
                    "event.engineObservation.usage.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                UsageInput {
                    basis: decode_observation_usage_basis(observation.get_basis()?),
                    input_tokens: match observation.get_input_tokens().which()? {
                        artisan_capnp::observation_usage::input_tokens::Which::NoInputTokens(()) => {
                            None
                        }
                        artisan_capnp::observation_usage::input_tokens::Which::InputTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    cached_input_tokens: match observation.get_cached_input_tokens().which()? {
                        artisan_capnp::observation_usage::cached_input_tokens::Which::NoCachedInputTokens(
                            (),
                        ) => None,
                        artisan_capnp::observation_usage::cached_input_tokens::Which::CachedInputTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    output_tokens: match observation.get_output_tokens().which()? {
                        artisan_capnp::observation_usage::output_tokens::Which::NoOutputTokens(
                            (),
                        ) => None,
                        artisan_capnp::observation_usage::output_tokens::Which::OutputTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    context_tokens: match observation.get_context_tokens().which()? {
                        artisan_capnp::observation_usage::context_tokens::Which::NoContextTokens(
                            (),
                        ) => None,
                        artisan_capnp::observation_usage::context_tokens::Which::ContextTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    context_window_tokens: {
                        let window = observation.get_context_window_tokens();
                        if window == 0 { None } else { Some(window) }
                    },
                    cost_usd: match observation.get_cost().which()? {
                        artisan_capnp::observation_usage::cost::Which::NoCost(()) => None,
                        artisan_capnp::observation_usage::cost::Which::Cost(cost) => Some(cost),
                    },
                    provider_route_id: parse_optional_observation_id(
                        read_text(
                            observation.get_provider_route_id(),
                            "event.engineObservation.usage.providerRouteId",
                        )?,
                        "event.engineObservation.usage.providerRouteId",
                    )?,
                    turn_id: parse_optional_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.usage.turnId",
                        )?,
                        "event.engineObservation.usage.turnId",
                    )?,
                },
            )?))
        }
    }
}

fn parse_optional_observation_id(
    value: String,
    field: &'static str,
) -> Result<Option<ObservationId>, ProtocolDecodeError> {
    match absent_if_empty(value) {
        None => Ok(None),
        Some(text) => parse_observation_id(text, field).map(Some),
    }
}

fn decode_observation_approval_request(
    value: artisan_capnp::observation_approval_request::Reader<'_>,
) -> Result<ApprovalRequest, ProtocolDecodeError> {
    let kind = decode_observation_approval_kind(value.get_kind()?);
    let command = absent_if_empty(read_text(
        value.get_command(),
        "event.engineObservation.approval.request.command",
    )?);
    let cwd = absent_if_empty(read_text(
        value.get_cwd(),
        "event.engineObservation.approval.request.cwd",
    )?);
    let reason = absent_if_empty(read_text(
        value.get_reason(),
        "event.engineObservation.approval.request.reason",
    )?);
    match kind {
        ApprovalKind::Command => {
            let Some(command) = command else {
                return Err(ObservationError::MissingField { field: "command" }.into());
            };
            Ok(ApprovalRequest::command(command, cwd, reason)?)
        }
        ApprovalKind::FileChange => {
            if command.is_some() {
                return Err(ObservationError::UnexpectedField { field: "command" }.into());
            }
            if cwd.is_some() {
                return Err(ObservationError::UnexpectedField { field: "cwd" }.into());
            }
            Ok(ApprovalRequest::file_change(reason)?)
        }
        ApprovalKind::Action => {
            if command.is_some() {
                return Err(ObservationError::UnexpectedField { field: "command" }.into());
            }
            if cwd.is_some() {
                return Err(ObservationError::UnexpectedField { field: "cwd" }.into());
            }
            Ok(ApprovalRequest::action(reason)?)
        }
    }
}

fn decode_observation_engine_error_ref(
    value: artisan_capnp::observation_engine_error_ref::Reader<'_>,
    field: &'static str,
) -> Result<EngineErrorRef, ProtocolDecodeError> {
    // Sub-field failures report the enclosing error-reference label: every
    // label below stays a static string so no provider text ever enters an
    // error value.
    Ok(EngineErrorRef::new(EngineErrorRefInput {
        artisan_code: ArtisanCode::parse(read_text(value.get_artisan_code(), field)?)
            .map_err(ProtocolDecodeError::from)?,
        provider_code: absent_if_empty(read_text(value.get_provider_code(), field)?),
        detail: absent_if_empty(read_text(value.get_detail(), field)?),
        affected_model_id: absent_if_empty(read_text(value.get_affected_model_id(), field)?),
        limit_id: absent_if_empty(read_text(value.get_limit_id(), field)?),
        limit_label: absent_if_empty(read_text(value.get_limit_label(), field)?),
        limit_scope: match value.get_limit_scope().which()? {
            artisan_capnp::observation_engine_error_ref::limit_scope::Which::NoLimitScope(()) => {
                None
            }
            artisan_capnp::observation_engine_error_ref::limit_scope::Which::LimitScope(scope) => {
                Some(decode_observation_limit_scope(scope?))
            }
        },
        resets_at: absent_if_empty(read_text(value.get_resets_at(), field)?),
    })?)
}

fn decode_transcript_content(
    value: artisan_capnp::observation_subagent_transcript_content::Reader<'_>,
) -> Result<TranscriptContent, ProtocolDecodeError> {
    match value.which()? {
        artisan_capnp::observation_subagent_transcript_content::Which::AgentMessageDelta(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::AgentMessageDelta(
                TranscriptAgentMessageDelta::new(
                    parse_observation_id(
                        read_text(
                            content.get_item_id(),
                            "event.engineObservation.subagentTranscript.content.agentMessageDelta.itemId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.agentMessageDelta.itemId",
                    )?,
                    decode_observation_message_phase(content.get_phase()?),
                    read_text(
                        content.get_delta(),
                        "event.engineObservation.subagentTranscript.content.agentMessageDelta.delta",
                    )?,
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::AgentMessageCompleted(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::AgentMessageCompleted(
                TranscriptAgentMessageCompleted::new(
                    parse_observation_id(
                        read_text(
                            content.get_item_id(),
                            "event.engineObservation.subagentTranscript.content.agentMessageCompleted.itemId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.agentMessageCompleted.itemId",
                    )?,
                    decode_observation_message_phase(content.get_phase()?),
                    read_text(
                        content.get_message(),
                        "event.engineObservation.subagentTranscript.content.agentMessageCompleted.message",
                    )?,
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::ReasoningSummaryDelta(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::ReasoningSummaryDelta(
                TranscriptReasoningSummaryDelta::new(
                    parse_observation_id(
                        read_text(
                            content.get_item_id(),
                            "event.engineObservation.subagentTranscript.content.reasoningSummaryDelta.itemId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.reasoningSummaryDelta.itemId",
                    )?,
                    content.get_summary_index(),
                    read_text(
                        content.get_delta(),
                        "event.engineObservation.subagentTranscript.content.reasoningSummaryDelta.delta",
                    )?,
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::ReasoningSummaryCompleted(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::ReasoningSummaryCompleted(
                TranscriptReasoningSummaryCompleted::new(
                    parse_observation_id(
                        read_text(
                            content.get_item_id(),
                            "event.engineObservation.subagentTranscript.content.reasoningSummaryCompleted.itemId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.reasoningSummaryCompleted.itemId",
                    )?,
                    absent_if_empty(read_text(
                        content.get_text(),
                        "event.engineObservation.subagentTranscript.content.reasoningSummaryCompleted.text",
                    )?),
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::TerminalActivity(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::TerminalActivity(
                TranscriptTerminalActivity::new(
                    parse_observation_id(
                        read_text(
                            content.get_activity_id(),
                            "event.engineObservation.subagentTranscript.content.terminalActivity.activityId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.terminalActivity.activityId",
                    )?,
                    match content.get_channel().which()? {
                        artisan_capnp::observation_transcript_terminal_activity::channel::Which::NoChannel(
                            (),
                        ) => None,
                        artisan_capnp::observation_transcript_terminal_activity::channel::Which::Channel(
                            channel,
                        ) => Some(decode_observation_terminal_channel(channel?)),
                    },
                    absent_if_empty(read_text(
                        content.get_command(),
                        "event.engineObservation.subagentTranscript.content.terminalActivity.command",
                    )?),
                    match content.get_exit_code().which()? {
                        artisan_capnp::observation_transcript_terminal_activity::exit_code::Which::NoExitCode(
                            (),
                        ) => None,
                        artisan_capnp::observation_transcript_terminal_activity::exit_code::Which::ExitCode(
                            code,
                        ) => Some(code),
                    },
                    match content.get_output().which()? {
                        artisan_capnp::observation_transcript_terminal_activity::output::Which::NoOutput(
                            (),
                        ) => None,
                        artisan_capnp::observation_transcript_terminal_activity::output::Which::Output(
                            output,
                        ) => Some(read_text(
                            output,
                            "event.engineObservation.subagentTranscript.content.terminalActivity.output",
                        )?),
                    },
                    decode_observation_terminal_state(content.get_state()?),
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::Tool(content) => {
            let content = content?;
            Ok(TranscriptContent::Tool(TranscriptTool::new(
                parse_observation_id(
                    read_text(
                        content.get_tool_id(),
                        "event.engineObservation.subagentTranscript.content.tool.toolId",
                    )?,
                    "event.engineObservation.subagentTranscript.content.tool.toolId",
                )?,
                read_text(
                    content.get_tool_name(),
                    "event.engineObservation.subagentTranscript.content.tool.toolName",
                )?,
                decode_observation_tool_action(content.get_action()?),
                absent_if_empty(read_text(
                    content.get_detail(),
                    "event.engineObservation.subagentTranscript.content.tool.detail",
                )?),
            )?))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::File(content) => {
            let content = content?;
            Ok(TranscriptContent::File(TranscriptFile::new(
                read_text(
                    content.get_path(),
                    "event.engineObservation.subagentTranscript.content.file.path",
                )?,
                decode_observation_file_action(content.get_action()?),
                match content.get_lines_added().which()? {
                    artisan_capnp::observation_transcript_file::lines_added::Which::NoLinesAdded(
                        (),
                    ) => None,
                    artisan_capnp::observation_transcript_file::lines_added::Which::LinesAdded(
                        count,
                    ) => Some(count),
                },
                match content.get_lines_deleted().which()? {
                    artisan_capnp::observation_transcript_file::lines_deleted::Which::NoLinesDeleted(
                        (),
                    ) => None,
                    artisan_capnp::observation_transcript_file::lines_deleted::Which::LinesDeleted(
                        count,
                    ) => Some(count),
                },
            )?))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::Search(content) => {
            let content = content?;
            Ok(TranscriptContent::Search(TranscriptSearch::new(
                read_text(
                    content.get_query(),
                    "event.engineObservation.subagentTranscript.content.search.query",
                )?,
                match content.get_result_count().which()? {
                    artisan_capnp::observation_transcript_search::result_count::Which::NoResultCount(
                        (),
                    ) => None,
                    artisan_capnp::observation_transcript_search::result_count::Which::ResultCount(
                        count,
                    ) => Some(count),
                },
                match content.get_scope().which()? {
                    artisan_capnp::observation_transcript_search::scope::Which::NoScope(()) => None,
                    artisan_capnp::observation_transcript_search::scope::Which::Scope(scope) => {
                        Some(decode_observation_search_scope(scope?))
                    }
                },
                parse_optional_observation_id(
                    read_text(
                        content.get_search_id(),
                        "event.engineObservation.subagentTranscript.content.search.searchId",
                    )?,
                    "event.engineObservation.subagentTranscript.content.search.searchId",
                )?,
                decode_observation_search_state(content.get_state()?),
            )?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ProtocolVersion, ResolveRichLinkRequest, RichLinkPageMetadata};

    fn envelope(body: WireEnvelopeBody) -> WireEnvelope {
        WireEnvelope {
            protocol_version: ProtocolVersion::V1,
            frame_id: FrameId::parse("frame-1").expect("frame id is valid"),
            sent_at: UnixMillis::from_millis(1_700_000_000_000),
            body,
        }
    }

    #[test]
    fn resolve_rich_link_request_round_trips() {
        let request = ResolveRichLinkRequest::new("https://example.com/docs?q=1#frag")
            .expect("request is valid");
        let wire = envelope(WireEnvelopeBody::Request(ClientRequest::ResolveRichLink(
            request,
        )));
        let encoded = encode_envelope(&wire).expect("request encodes");
        let decoded = decode_envelope(&encoded).expect("request decodes");
        assert!(decoded.body == wire.body);
    }

    #[test]
    fn rich_link_response_round_trips() {
        let metadata =
            RichLinkPageMetadata::new("https://example.com/docs", "Example — Docs", 1_700_000_060_000)
                .expect("metadata is valid");
        let wire = envelope(WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("frame-1").expect("request id is valid"),
            payload: ResponsePayload::RichLink(metadata),
        }));
        let encoded = encode_envelope(&wire).expect("response encodes");
        let decoded = decode_envelope(&encoded).expect("response decodes");
        assert!(decoded.body == wire.body);
    }

    #[test]
    fn decode_rejects_non_http_rich_link_url() {
        let mut message = Builder::new(HeapAllocator::new());
        {
            let mut root = message.init_root::<artisan_capnp::envelope::Builder>();
            root.set_protocol_version(1);
            root.set_message_id("frame-1");
            root.set_sent_at_millis(0);
            root.reborrow()
                .init_body()
                .init_request()
                .init_resolve_rich_link()
                .set_url("file:///etc/passwd");
        }
        let bytes = serialize::write_message_to_words(&message);
        assert!(matches!(
            decode_envelope(&bytes),
            Err(ProtocolDecodeError::ProtocolValue { .. })
        ));
    }

    fn repository_snapshot() -> RepositorySnapshot {
        RepositorySnapshot::new(
            RepositoryBranchState::attached("main").expect("branch"),
            Some("origin".to_owned()),
            vec![
                RepositoryRemote::new(
                    RepositoryHost::GitHub,
                    "origin",
                    "git@github.com:artisanstreet/editor.git",
                    Some("https://github.com/artisanstreet/editor".to_owned()),
                )
                .expect("github remote"),
                RepositoryRemote::new(
                    RepositoryHost::Unknown,
                    "backup",
                    "/srv/backup/editor.git",
                    None,
                )
                .expect("local remote"),
            ],
        )
        .expect("snapshot")
    }

    #[test]
    fn project_repository_query_request_round_trips() {
        let query = ProjectRepositoryQuery::new(vec![
            ProjectId::parse("project-1").expect("project"),
            ProjectId::parse("project-2").expect("project"),
        ])
        .expect("query");
        let wire = envelope(WireEnvelopeBody::Request(
            ClientRequest::QueryProjectRepository(query),
        ));
        let encoded = encode_envelope(&wire).expect("request encodes");
        let decoded = decode_envelope(&encoded).expect("request decodes");
        assert!(decoded.body == wire.body);
    }

    #[test]
    fn project_repository_response_round_trips_every_observation() {
        let result = ProjectRepositoryQueryResult::new(vec![
            ProjectRepositoryEntry::new(
                ProjectId::parse("project-1").expect("project"),
                ProjectRepository::Repository(repository_snapshot()),
            ),
            ProjectRepositoryEntry::new(
                ProjectId::parse("project-2").expect("project"),
                ProjectRepository::NotRepository,
            ),
        ])
        .expect("result");
        let wire = envelope(WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("frame-1").expect("request id is valid"),
            payload: ResponsePayload::ProjectRepository(result),
        }));
        let encoded = encode_envelope(&wire).expect("response encodes");
        let decoded = decode_envelope(&encoded).expect("response decodes");
        assert!(decoded.body == wire.body);
    }

    #[test]
    fn project_repository_response_preserves_unborn_and_detached_branches() {
        for branch in [
            RepositoryBranchState::unborn("main").expect("branch"),
            RepositoryBranchState::detached(),
        ] {
            let snapshot = RepositorySnapshot::new(branch, None, vec![]).expect("snapshot");
            let result = ProjectRepositoryQueryResult::new(vec![ProjectRepositoryEntry::new(
                ProjectId::parse("project-1").expect("project"),
                ProjectRepository::Repository(snapshot),
            )])
            .expect("result");
            let wire = envelope(WireEnvelopeBody::Response(ServerResponse {
                request_id: RequestId::parse("frame-1").expect("request id is valid"),
                payload: ResponsePayload::ProjectRepository(result),
            }));
            let encoded = encode_envelope(&wire).expect("response encodes");
            let decoded = decode_envelope(&encoded).expect("response decodes");
            assert!(decoded.body == wire.body);
        }
    }

    #[test]
    fn decode_rejects_repository_with_remotes_but_no_default() {
        let mut message = Builder::new(HeapAllocator::new());
        {
            let mut root = message.init_root::<artisan_capnp::envelope::Builder>();
            root.set_protocol_version(1);
            root.set_message_id("frame-1");
            root.set_sent_at_millis(0);
            let mut response = root.reborrow().init_body().init_response();
            response.set_request_id("frame-1");
            let result = response.init_project_repository();
            let mut entries = result.init_repositories(1);
            let mut entry = entries.reborrow().get(0);
            entry.set_project_id("project-1");
            let mut repository = entry.init_repository();
            repository.set_state(artisan_capnp::ProjectRepositoryState::Repository);
            let mut snapshot = repository.reborrow().init_snapshot();
            snapshot
                .reborrow()
                .init_branch()
                .set_kind(artisan_capnp::RepositoryBranchKind::Detached);
            snapshot.set_default_remote("");
            let mut remotes = snapshot.init_remotes(1);
            let mut remote = remotes.reborrow().get(0);
            remote.set_host(artisan_capnp::RepositoryHost::Github);
            remote.set_name("origin");
            remote.set_url("https://github.com/artisanstreet/editor.git");
            remote.set_web_url("https://github.com/artisanstreet/editor");
        }
        let bytes = serialize::write_message_to_words(&message);
        assert!(matches!(
            decode_envelope(&bytes),
            Err(ProtocolDecodeError::ProtocolValue { .. })
        ));
    }
}
