//! Total conversion between owned protocol values and generated Cap'n Proto.

use artisan_domain::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation, ApprovalKind,
    ApprovalObservation, ApprovalRequest, ApprovalState, ArtisanCode, CompactionObservation,
    CompactionState, DiagnosticLevel, EngineErrorRef, EngineErrorRefInput,
    EngineObservationAttribution, EngineObservationEvent, FileAction, FileObservation, LimitScope,
    MessagePhase, NativeActionObservation, OBSERVATION_ANSWERS_MAX, OBSERVATION_PLAN_MAX_ENTRIES,
    OBSERVATION_QUESTION_MAX_OPTIONS, Observation, ObservationError, ObservationId,
    ObservationSequence, PlanEntry, PlanEntryStatus, PlanObservation, ProcessDiagnosticObservation,
    ProtocolDiagnosticObservation, QuestionInput, QuestionObservation, QuestionOption,
    QuestionState, ReasoningSummaryCompletedObservation, ReasoningSummaryDeltaObservation,
    RetryAttemptState, RetryObservation, RunState, RunStateObservation, RunTerminalObservation,
    RunTerminalState, SearchObservation, SearchScope, SearchState, SubagentInput,
    SubagentObservation, SubagentState, SubagentTranscriptObservation, TerminalActivityInput,
    TerminalActivityObservation, TerminalActivityState, TerminalChannel, ToolAction,
    ToolObservation, TranscriptAgentMessageCompleted, TranscriptAgentMessageDelta,
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
    DisplayName, DisplayNameError, ENGINE_USAGE_ENGINES_MAX, ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE,
    EngineAgentId, EngineConfigError, EngineConfigReason, EngineConfigRevision,
    EngineConfigUpdatePrecondition, EngineId, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, EngineUsageAuth, EngineUsageAuthentication,
    EngineUsageError, EngineUsageReport, EngineUsageSnapshot, EngineUsageWindow,
    EngineUsageWindowKind, EngineVariantId, Event, FilesystemAccess, FiniteMillis,
    FirstMessageQueued, GrokPermissionMode, GrokReasoningEffort, GrokSelection,
    IdentifierError, ImageAttachment,
    ImageAttachmentError, ImageAttachmentRef, ImageAttachmentRefError, IncrementalText,
    IncrementalTextError, ItemId, ItemOrdinal, ListAttachedProjects, ListDirectories,
    ListProjectThreads, MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES, MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
    MessageBody, MessageBodyError, MessageId, ModelFavoriteId, ModelFavoriteIdError,
    ModelFavoritesRevision, ModelFavoritesRevisionError, ModelFavoritesSnapshotError,
    MultimodalUserMessageItem, NetworkAccess, OpenCode2Selection, PROJECT_LISTING_MAX_PROJECTS,
    PatchBatch, PatchBatchError, PatchId, PatchSequence, PermissionId, PlaceKind, ProjectAttached,
    ProjectId, ProjectListing, ProjectListingError, ProjectSummary, Query, QueryTurnCount,
    QueryTurnCountError, QueueFirstMessage, QueueMessage, QueueMessagePayload,
    QueueMessagePayloadError, QueuedMessage, QuotaSurface, ReadAccountUsage, ReadActiveRun,
    ReadComposerCatalog, ReadModelFavorites, ReceiptDisposition, RequestId, RespondApproval,
    RespondQuestion, Revision, RootPath, RootPathError, RunId, RunInteractionError,
    SetModelFavorite, SetThreadEngineConfig, SteerTarget, StopRun, THREAD_LISTING_MAX_THREADS,
    ThreadCreated, ThreadId, ThreadListing, ThreadListingError, ThreadSummary, ThreadTitle,
    ThreadTitleError, TurnId, TurnOrdinal, UnixMillis, UserMessageItem, WebSearchAccess,
};
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use capnp::serialize;

use crate::artisan_capnp::{
    self, conversation_item, conversation_patch, conversation_query_request,
    conversation_subscribe_request, conversation_subscription_started, directory_listing,
    directory_pick_outcome, engine_config_precondition, engine_run_config, engine_selection_v2,
    engine_usage_report, engine_usage_snapshot, engine_usage_window, envelope as capnp_envelope,
    event, lifecycle_request, lifecycle_response, list_directories_request, protocol_error,
    query_range, read_account_usage_request, request, response, set_thread_engine_config_request,
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

mod conversation;
mod engine_config;
mod envelope;
mod error;
mod interaction;
mod observation;
mod repository;
mod server;

pub(crate) use conversation::*;
pub(crate) use engine_config::*;
pub(crate) use envelope::*;
pub(crate) use interaction::*;
pub(crate) use observation::*;
pub(crate) use repository::*;
pub(crate) use server::*;

pub use error::{ProtocolDecodeError, ProtocolEncodeError};

/// Maximum Cap'n Proto graph traversal for one already-framed application
/// message (8,388,608 words, or 64 MiB). Quinn framing applies a tighter byte
/// ceiling; this independent bound prevents pointer amplification in the
/// codec.
pub const CAPNP_TRAVERSAL_LIMIT_WORDS: usize = 8 * 1024 * 1024;
/// The Artisan schema is shallow; 32 levels leaves generous evolution room
/// while bounding hostile pointer nesting explicitly.
pub const CAPNP_NESTING_LIMIT: i32 = 32;

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
        let mut root = message.init_root::<capnp_envelope::Builder>();
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
    let root: capnp_envelope::Reader = message.get_root()?;

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
