//! Total conversion between owned protocol values and generated Cap'n Proto.

use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantBodyError, AssistantMessageItem, AssistantMessagePhase,
    AttachProject, ByteLimit, CONVERSATION_PATCH_BATCH_MAX_PATCHES, CONVERSATION_QUERY_MAX_TURNS,
    Command, ConversationCursor, ConversationItem, ConversationLifecycle, ConversationPatch,
    ConversationQuery, ConversationQueryBounds, ConversationRequest, ConversationSnapshot,
    ConversationSnapshotError, ConversationSubscribe, ConversationSubscriptionStart,
    ConversationTurn, ConversationUnsubscribe, CountLimit, CounterError, CreateThread,
    DIRECTORY_LISTING_MAX_ENTRIES, DIRECTORY_LISTING_MAX_PLACES, DirectoryEntry, DirectoryId,
    DirectoryKind, DirectoryListing, DirectoryListingError, DirectoryPlace, DisplayName,
    DisplayNameError, EngineAgentId, EngineConfigError, EngineConfigReason, EngineConfigRevision,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, EngineVariantId, Event, FilesystemAccess, FiniteMillis, FirstMessageQueued,
    IdentifierError, IncrementalText, IncrementalTextError, ItemId, ItemOrdinal,
    ListAttachedProjects, ListDirectories, ListProjectThreads, MessageBody, MessageBodyError,
    MessageId, NetworkAccess, OpenCode2Selection, PROJECT_LISTING_MAX_PROJECTS, PatchBatch,
    PatchBatchError, PatchId, PatchSequence, PermissionId, PlaceKind, ProjectAttached, ProjectId,
    ProjectListing, ProjectListingError, ProjectSummary, Query, QueryTurnCount,
    QueryTurnCountError, QueueFirstMessage, QueuedMessage, ReceiptDisposition, RequestId, Revision,
    RootPath, RootPathError, RunId, SetThreadEngineConfig, THREAD_LISTING_MAX_THREADS,
    ThreadCreated, ThreadId, ThreadListing, ThreadListingError, ThreadSummary, ThreadTitle,
    ThreadTitleError, TurnId, TurnOrdinal, UnixMillis, UserMessageItem, WebSearchAccess,
};
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use capnp::serialize;
use thiserror::Error;

use crate::artisan_capnp::{
    self, conversation_item, conversation_patch, conversation_query_request,
    conversation_subscribe_request, conversation_subscription_started, directory_listing,
    directory_pick_outcome, engine_config_precondition, engine_run_config, envelope, event,
    lifecycle_request, lifecycle_response, list_directories_request, protocol_error, query_range,
    request, response, set_thread_engine_config_request,
};
use crate::types::{
    ClientRequest, ConnectionId, ConversationSubscriptionStarted, ConversationSubscriptionStopped,
    DirectoryPickOutcome, ErrorCode, ErrorDetail, EventCursor, FirstMessageReceipt, FrameId, Hello,
    HelloCredential, LifecycleRequest, LifecycleResponse, LifecycleState, LifecycleStatus,
    LifecycleStopDisposition, LifecycleStopReceipt, LocalCapability, LocalCapabilityError,
    ProtocolFailure, ProtocolValueError, ProtocolVersion, ReconnectCapability,
    ReconnectCapabilityError, ResponsePayload, ServerEvent, ServerResponse,
    SetThreadEngineConfigResult, VersionOffer, VersionOfferError, Welcome, WireEnvelope,
    WireEnvelopeBody,
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
}

/// Failure while reading and validating one external protocol frame.
#[derive(Debug, Error)]
pub enum ProtocolDecodeError {
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
            encode_request(root.reborrow().init_body().init_request(), value);
        }
        WireEnvelopeBody::Response(value) => {
            encode_response(root.reborrow().init_body().init_response(), value)?;
        }
        WireEnvelopeBody::Event(value) => {
            encode_event(root.reborrow().init_body().init_event(), value);
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

fn encode_request(mut builder: artisan_capnp::request::Builder<'_>, value: &ClientRequest) {
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
        ClientRequest::Command(Command::SetThreadEngineConfig(command)) => {
            encode_set_thread_engine_config(builder.reborrow(), command.as_ref());
        }
        ClientRequest::Conversation(ConversationRequest::Query(query)) => {
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
    }
}

fn encode_response(
    mut builder: artisan_capnp::response::Builder<'_>,
    value: &ServerResponse,
) -> Result<(), ProtocolEncodeError> {
    builder.set_request_id(value.request_id.as_str());
    match &value.payload {
        ResponsePayload::DirectoryListing(listing) => {
            encode_directory_listing(builder.reborrow().init_directory_list(), listing)?;
        }
        ResponsePayload::ProjectListing(listing) => {
            let mut projects = builder
                .reborrow()
                .init_project_list()
                .init_projects(list_length(
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
            encode_thread_engine_config_result(builder.reborrow(), result)
        }
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

fn encode_engine_run_config(mut builder: engine_run_config::Builder<'_>, value: &EngineRunConfig) {
    let selection = value.selection().as_opencode2();
    builder.set_schema_version(1);
    builder.set_engine(artisan_domain::EngineId::OpenCode2.as_str());
    builder.set_profile_id(selection.profile_id().as_str());
    builder.set_model_id(selection.model_id().as_str());
    builder.set_route_id(selection.route_id().as_str());
    let mut variant = builder.reborrow().init_variant();
    if let Some(id) = selection.variant_id() {
        variant.set_kind("selected");
        variant.set_id(id.as_str());
    } else {
        variant.set_kind("none");
        variant.set_id("");
    }
    let permission = selection.permission();
    let mut encoded_permission = builder.reborrow().init_permission();
    encoded_permission.set_permission_id(permission.permission_id().as_str());
    encoded_permission.set_agent_id(permission.agent_id().as_str());
    encoded_permission.set_approval(permission.approval().as_str());
    encoded_permission.set_filesystem(permission.filesystem().as_str());
    encoded_permission.set_network(permission.network().as_str());
    encoded_permission.set_web_search(permission.web_search().as_str());

    let runtime = value.runtime();
    let mut encoded_runtime = builder.init_runtime();
    encoded_runtime.set_attempt_budget_ms(runtime.attempt_budget().get());
    encoded_runtime.set_readiness_budget_ms(runtime.readiness_budget().get());
    encoded_runtime.set_health_budget_ms(runtime.health_budget().get());
    encoded_runtime.set_prompt_budget_ms(runtime.prompt_budget().get());
    encoded_runtime.set_stream_budget_ms(runtime.stream_budget().get());
    encoded_runtime.set_close_budget_ms(runtime.close_budget().get());
    encoded_runtime.set_max_json_body_bytes(runtime.max_json_body_bytes().get());
    encoded_runtime.set_max_sse_line_bytes(runtime.max_sse_line_bytes().get());
    encoded_runtime.set_max_sse_event_bytes(runtime.max_sse_event_bytes().get());
    encoded_runtime.set_max_readiness_line_bytes(runtime.max_readiness_line_bytes().get());
    encoded_runtime.set_max_header_count(runtime.max_header_count().get());
    encoded_runtime.set_max_http_buffer_bytes(runtime.max_http_buffer_bytes().get());
    encoded_runtime.set_max_stderr_bytes(runtime.max_stderr_bytes().get());
    encoded_runtime.set_observation_capacity(runtime.observation_capacity().get());
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

fn encode_event(mut builder: artisan_capnp::event::Builder<'_>, value: &ServerEvent) {
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
    }
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
    if value.get_schema_version() != 1 {
        return Err(engine_config_error(
            "request.setThreadEngineConfig.config.schemaVersion",
            EngineConfigReason::Unsupported,
        ));
    }
    let engine = read_text(
        value.get_engine(),
        "request.setThreadEngineConfig.config.engine",
    )?;
    if engine != artisan_domain::EngineId::OpenCode2.as_str() {
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
    };
    Ok(ServerEvent { cursor, event })
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

const fn decode_disposition(value: artisan_capnp::ReceiptDisposition) -> ReceiptDisposition {
    match value {
        artisan_capnp::ReceiptDisposition::Accepted => ReceiptDisposition::Accepted,
        artisan_capnp::ReceiptDisposition::Duplicate => ReceiptDisposition::Duplicate,
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
