//! Total conversion between owned protocol values and generated Cap'n Proto.

use artisan_domain::{
    AttachProject, Command, CreateThread, DIRECTORY_LISTING_MAX_ENTRIES,
    DIRECTORY_LISTING_MAX_PLACES, DirectoryEntry, DirectoryId, DirectoryKind, DirectoryListing,
    DirectoryListingError, DirectoryPlace, DisplayName, DisplayNameError, Event,
    FirstMessageQueued, IdentifierError, ListAttachedProjects, ListDirectories, ListProjectThreads,
    MessageBody, MessageBodyError, MessageId, PROJECT_LISTING_MAX_PROJECTS, PlaceKind,
    ProjectAttached, ProjectId, ProjectListing, ProjectListingError, ProjectSummary, Query,
    QueueFirstMessage, QueuedMessage, ReceiptDisposition, RequestId, RootPath, RootPathError,
    THREAD_LISTING_MAX_THREADS, ThreadCreated, ThreadId, ThreadListing, ThreadListingError,
    ThreadSummary, ThreadTitle, ThreadTitleError, UnixMillis,
};
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use capnp::serialize;
use thiserror::Error;

use crate::artisan_capnp::{
    self, directory_listing, envelope, event, list_directories_request, protocol_error, request,
    response,
};
use crate::types::{
    ClientRequest, ConnectionId, ErrorCode, ErrorDetail, FirstMessageReceipt, FrameId, Hello,
    LocalCapability, LocalCapabilityError, ProtocolFailure, ProtocolValueError, ProtocolVersion,
    ResponsePayload, ServerResponse, VersionOffer, VersionOfferError, Welcome, WireEnvelope,
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
    /// Two wire correlation fields disagreed.
    #[error("{field} does not match its enclosing request correlation")]
    CorrelationMismatch {
        /// Nested correlation field.
        field: &'static str,
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

impl From<VersionOfferError> for ProtocolDecodeError {
    fn from(source: VersionOfferError) -> Self {
        Self::VersionOffer { source }
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
            hello.set_capability(value.capability.expose_for_wire());
        }
        WireEnvelopeBody::Welcome(value) => {
            let mut welcome = root.reborrow().init_body().init_welcome();
            welcome.set_negotiated_version(value.negotiated_version.get());
            welcome.set_connection_id(value.connection_id.as_str());
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
    }
    Ok(())
}

fn encode_event(mut builder: artisan_capnp::event::Builder<'_>, value: &Event) {
    match value {
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

const fn encode_error_code(value: ErrorCode) -> artisan_capnp::ErrorCode {
    match value {
        ErrorCode::UnsupportedVersion => artisan_capnp::ErrorCode::UnsupportedVersion,
        ErrorCode::InvalidInput => artisan_capnp::ErrorCode::InvalidInput,
        ErrorCode::DirectoryUnknown => artisan_capnp::ErrorCode::DirectoryUnknown,
        ErrorCode::ProjectUnknown => artisan_capnp::ErrorCode::ProjectUnknown,
        ErrorCode::ThreadUnknown => artisan_capnp::ErrorCode::ThreadUnknown,
        ErrorCode::Internal => artisan_capnp::ErrorCode::Internal,
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
    let capability = LocalCapability::try_from_slice(value.get_capability()?)?;
    Ok(Hello {
        supported_versions,
        capability,
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
            let command = command?;
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
    }
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
        response::Which::QueuedReceipt(receipt) => {
            let receipt = receipt?;
            let nested_request_id = parse_request_id(
                read_text(receipt.get_request_id(), "response.queuedReceipt.requestId")?,
                "response.queuedReceipt.requestId",
            )?;
            if nested_request_id != request_id {
                return Err(ProtocolDecodeError::CorrelationMismatch {
                    field: "response.queuedReceipt.requestId",
                });
            }
            let state = receipt.get_state()?;
            match state {
                artisan_capnp::QueuedState::Queued => {}
            }
            ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
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
            })
        }
    };
    Ok(ServerResponse {
        request_id,
        payload,
    })
}

fn decode_event(value: artisan_capnp::event::Reader<'_>) -> Result<Event, ProtocolDecodeError> {
    match value.which()? {
        event::Which::ProjectAttached(project) => Ok(Event::ProjectAttached(ProjectAttached {
            project: decode_project(project?)?,
        })),
        event::Which::ThreadCreated(thread) => Ok(Event::ThreadCreated(ThreadCreated {
            thread: decode_thread(thread?)?,
        })),
        event::Which::FirstMessageQueued(queued) => {
            let queued = queued?;
            Ok(Event::FirstMessageQueued(FirstMessageQueued {
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

const fn decode_error_code(value: artisan_capnp::ErrorCode) -> ErrorCode {
    match value {
        artisan_capnp::ErrorCode::UnsupportedVersion => ErrorCode::UnsupportedVersion,
        artisan_capnp::ErrorCode::InvalidInput => ErrorCode::InvalidInput,
        artisan_capnp::ErrorCode::DirectoryUnknown => ErrorCode::DirectoryUnknown,
        artisan_capnp::ErrorCode::ProjectUnknown => ErrorCode::ProjectUnknown,
        artisan_capnp::ErrorCode::ThreadUnknown => ErrorCode::ThreadUnknown,
        artisan_capnp::ErrorCode::Internal => ErrorCode::Internal,
    }
}
