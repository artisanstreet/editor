//! Owned Phase 2 protocol conversion and malformed-input coverage.

use std::error::Error;

use artisan_domain::{
    AttachProject, Command, CreateThread, DIRECTORY_LISTING_MAX_ENTRIES,
    DIRECTORY_LISTING_MAX_PLACES, DirectoryEntry, DirectoryId, DirectoryKind, DirectoryListing,
    DirectoryListingError, DirectoryPlace, DisplayName, Event, FirstMessageQueued,
    ListAttachedProjects, ListDirectories, ListProjectThreads, MessageBody, MessageId,
    PROJECT_LISTING_MAX_PROJECTS, PlaceKind, ProjectAttached, ProjectId, ProjectListing,
    ProjectListingError, ProjectSummary, Query, QueueFirstMessage, QueuedMessage,
    ReceiptDisposition, RequestId, RootPath, THREAD_LISTING_MAX_THREADS, ThreadCreated, ThreadId,
    ThreadListing, ThreadListingError, ThreadSummary, ThreadTitle, UnixMillis,
};
use artisan_protocol::artisan_capnp::{ErrorCode as WireErrorCode, envelope};
use artisan_protocol::{
    CAPNP_NESTING_LIMIT, CAPNP_TRAVERSAL_LIMIT_WORDS, ClientRequest, ConnectionId, ErrorCode,
    ErrorDetail, EventCursor, FirstMessageReceipt, FrameId, Hello, HelloCredential,
    LocalCapability, LocalCapabilityError, ProtocolDecodeError, ProtocolEncodeError,
    ProtocolFailure, ProtocolValueError, ProtocolVersion, RECONNECT_CAPABILITY_BYTES,
    ReconnectCapability, ReconnectCapabilityError, ResponsePayload, ServerEvent, ServerResponse,
    VersionOffer, VersionOfferError, Welcome, WireEnvelope, WireEnvelopeBody, decode_envelope,
    encode_envelope,
};
use capnp::message::{Builder, HeapAllocator};
use capnp::serialize;

const INITIAL_CAPABILITY: [u8; 32] = [
    0x11, 0x82, 0x33, 0xa4, 0x55, 0xc6, 0x77, 0xe8, 0x19, 0x2a, 0x3b, 0x4c, 0x5d, 0x6e, 0x7f, 0x80,
    0x91, 0xa2, 0xb3, 0xc4, 0xd5, 0xe6, 0xf7, 0x08, 0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e, 0x8f, 0x90,
];
const RECONNECT_HELLO_CAPABILITY: [u8; RECONNECT_CAPABILITY_BYTES] = [
    0x23, 0x94, 0x45, 0xb6, 0x67, 0xd8, 0x89, 0xfa, 0x2b, 0x3c, 0x4d, 0x5e, 0x6f, 0x70, 0x81, 0x92,
    0xa3, 0xb4, 0xc5, 0xd6, 0xe7, 0xf8, 0x09, 0x1a, 0x3b, 0x4c, 0x5d, 0x6e, 0x7f, 0x80, 0x91, 0xa2,
];
const ROTATED_RECONNECT_CAPABILITY: [u8; RECONNECT_CAPABILITY_BYTES] = [
    0x35, 0xa6, 0x57, 0xc8, 0x79, 0xea, 0x9b, 0x0c, 0x4d, 0x5e, 0x6f, 0x70, 0x81, 0x92, 0xa3, 0xb4,
    0xc5, 0xd6, 0xe7, 0xf8, 0x09, 0x1a, 0x2b, 0x3c, 0x5d, 0x6e, 0x7f, 0x80, 0x91, 0xa2, 0xb3, 0xc4,
];

// These ambiguity checks fail to compile if the secret accidentally gains a
// common formatting or duplication trait. They need no test-only dependency.
const _: fn() = || {
    struct DebugMarker;
    trait AmbiguousIfDebug<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
    impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<DebugMarker> for T {}
    let _ = <LocalCapability as AmbiguousIfDebug<_>>::marker;
    let _ = <ReconnectCapability as AmbiguousIfDebug<_>>::marker;
    let _ = <HelloCredential as AmbiguousIfDebug<_>>::marker;
    let _ = <Hello as AmbiguousIfDebug<_>>::marker;
    let _ = <Welcome as AmbiguousIfDebug<_>>::marker;
};

const _: fn() = || {
    struct DisplayMarker;
    trait AmbiguousIfDisplay<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDisplay<()> for T {}
    impl<T: ?Sized + std::fmt::Display> AmbiguousIfDisplay<DisplayMarker> for T {}
    let _ = <LocalCapability as AmbiguousIfDisplay<_>>::marker;
    let _ = <ReconnectCapability as AmbiguousIfDisplay<_>>::marker;
    let _ = <HelloCredential as AmbiguousIfDisplay<_>>::marker;
    let _ = <Hello as AmbiguousIfDisplay<_>>::marker;
    let _ = <Welcome as AmbiguousIfDisplay<_>>::marker;
};

const _: fn() = || {
    struct CloneMarker;
    trait AmbiguousIfClone<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<CloneMarker> for T {}
    let _ = <LocalCapability as AmbiguousIfClone<_>>::marker;
    let _ = <ReconnectCapability as AmbiguousIfClone<_>>::marker;
    let _ = <HelloCredential as AmbiguousIfClone<_>>::marker;
    let _ = <Hello as AmbiguousIfClone<_>>::marker;
    let _ = <Welcome as AmbiguousIfClone<_>>::marker;
};

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("fixture request id is valid")
}

fn frame_id(value: &str) -> FrameId {
    FrameId::parse(value).expect("fixture frame id is valid")
}

fn project() -> ProjectSummary {
    ProjectSummary {
        project_id: ProjectId::parse("project-1").expect("fixture project id is valid"),
        display_name: DisplayName::parse("Artisan Editor").expect("fixture display name is valid"),
        root_path: RootPath::parse(r"C:\source\artisan-editor")
            .expect("fixture root path is valid"),
        attached_at: UnixMillis::MIN,
    }
}

fn thread() -> ThreadSummary {
    ThreadSummary {
        thread_id: ThreadId::parse("thread-1").expect("fixture thread id is valid"),
        project_id: ProjectId::parse("project-1").expect("fixture project id is valid"),
        title: ThreadTitle::parse("New thread").expect("fixture title is valid"),
        created_at: UnixMillis::MIN,
        updated_at: UnixMillis::MAX,
    }
}

fn envelope(frame: &str, body: WireEnvelopeBody) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: frame_id(frame),
        sent_at: UnixMillis::from_millis(-4_000),
        body,
    }
}

fn assert_roundtrip(value: &WireEnvelope) -> Result<(), Box<dyn Error>> {
    let encoded = encode_envelope(value)?;
    let decoded = decode_envelope(&encoded)?;
    assert!(
        decoded == *value,
        "owned envelope must survive field-for-field"
    );
    Ok(())
}

#[test]
fn capability_is_exact_length_and_errors_never_render_secret_material() {
    assert!(LocalCapability::try_from_slice(&INITIAL_CAPABILITY).is_ok());
    assert!(ReconnectCapability::try_from_slice(&RECONNECT_HELLO_CAPABILITY).is_ok());

    let short = [0xab; 31];
    let Err(error) = LocalCapability::try_from_slice(&short) else {
        panic!("31-byte capability must be rejected");
    };
    assert_eq!(
        error,
        LocalCapabilityError::InvalidLength {
            length: 31,
            expected: 32,
        }
    );
    let rendered = error.to_string();
    assert_eq!(
        rendered,
        "local capability is 31 bytes; exactly 32 bytes are required"
    );
    assert!(!rendered.contains("0xab"));
    assert!(!rendered.contains("[171"));

    let Err(error) = ReconnectCapability::try_from_slice(&short) else {
        panic!("31-byte reconnect capability must be rejected");
    };
    assert_eq!(
        error,
        ReconnectCapabilityError::InvalidLength {
            length: 31,
            expected: RECONNECT_CAPABILITY_BYTES,
        }
    );
    let rendered = error.to_string();
    assert_eq!(
        rendered,
        "reconnect capability is 31 bytes; exactly 32 bytes are required"
    );
    assert!(!rendered.contains("0xab"));
    assert!(!rendered.contains("[171"));
}

#[test]
fn capability_equality_uses_the_constant_time_boundary() {
    let initial = LocalCapability::from_bytes(INITIAL_CAPABILITY);
    let same_initial = LocalCapability::from_bytes(INITIAL_CAPABILITY);
    let mut different_initial_bytes = INITIAL_CAPABILITY;
    different_initial_bytes[0] ^= 0xff;
    let different_initial = LocalCapability::from_bytes(different_initial_bytes);

    assert!(initial.constant_time_eq(&same_initial));
    assert!(initial == same_initial);
    assert!(!initial.constant_time_eq(&different_initial));
    assert!(initial != different_initial);

    let reconnect = ReconnectCapability::from_bytes(RECONNECT_HELLO_CAPABILITY);
    let same_reconnect = ReconnectCapability::from_bytes(RECONNECT_HELLO_CAPABILITY);
    let mut different_reconnect_bytes = RECONNECT_HELLO_CAPABILITY;
    different_reconnect_bytes[RECONNECT_CAPABILITY_BYTES - 1] ^= 0xff;
    let different_reconnect = ReconnectCapability::from_bytes(different_reconnect_bytes);

    assert!(reconnect.constant_time_eq(&same_reconnect));
    assert!(reconnect == same_reconnect);
    assert!(!reconnect.constant_time_eq(&different_reconnect));
    assert!(reconnect != different_reconnect);
}

#[test]
fn version_offer_and_protocol_metadata_enforce_boundaries() {
    assert_eq!(VersionOffer::new(Vec::new()), Err(VersionOfferError::Empty));
    assert_eq!(
        VersionOffer::new(vec![1, 1]),
        Err(VersionOfferError::Duplicate { version: 1 })
    );
    assert_eq!(
        VersionOffer::new(vec![2, 1]),
        Err(VersionOfferError::OutOfOrder {
            previous: 2,
            actual: 1,
        })
    );
    assert_eq!(
        VersionOffer::new(vec![2]),
        Err(VersionOfferError::Unsupported { version: 2 })
    );
    assert_eq!(
        VersionOffer::new(vec![1; 9]),
        Err(VersionOfferError::TooMany {
            count: 9,
            maximum: 8,
        })
    );
    assert!(VersionOffer::new(vec![1]).is_ok());
    assert_eq!(
        ProtocolVersion::new(2),
        Err(ProtocolValueError::UnsupportedVersion { version: 2 })
    );
    assert!(ErrorDetail::parse("x".repeat(1_024)).is_ok());
    assert_eq!(
        ErrorDetail::parse("x".repeat(1_025)),
        Err(ProtocolValueError::ErrorDetailTooLong {
            length: 1_025,
            maximum: 1_024,
        })
    );
}

#[test]
fn handshake_frames_roundtrip_without_secret_formatting() -> Result<(), Box<dyn Error>> {
    let initial_hello = envelope(
        "client-initial-hello-1",
        WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])?,
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
        }),
    );
    assert_roundtrip(&initial_hello)?;

    let reconnect_hello = envelope(
        "client-reconnect-hello-1",
        WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])?,
            credential: HelloCredential::Reconnect(ReconnectCapability::from_bytes(
                RECONNECT_HELLO_CAPABILITY,
            )),
        }),
    );
    assert_roundtrip(&reconnect_hello)?;

    let welcome = envelope(
        "server-welcome-1",
        WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: ProtocolVersion::V1,
            connection_id: ConnectionId::parse("connection-1")?,
            reconnect_capability: ReconnectCapability::from_bytes(ROTATED_RECONNECT_CAPABILITY),
        }),
    );
    assert_roundtrip(&welcome)
}

#[test]
fn every_first_workflow_request_roundtrips() -> Result<(), Box<dyn Error>> {
    let list_roots = envelope(
        "request-list-roots",
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ListDirectories(
            ListDirectories { parent: None },
        ))),
    );
    assert_roundtrip(&list_roots)?;

    let list_child = envelope(
        "request-list-child",
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ListDirectories(
            ListDirectories {
                parent: Some(DirectoryId::parse("directory-1")?),
            },
        ))),
    );
    assert_roundtrip(&list_child)?;

    let list_projects = envelope(
        "request-list-projects",
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ListAttachedProjects(
            ListAttachedProjects,
        ))),
    );
    assert_roundtrip(&list_projects)?;

    let attach_id = request_id("request-attach");
    let attach = envelope(
        "request-attach",
        WireEnvelopeBody::Request(ClientRequest::Command(Command::AttachProject(
            AttachProject {
                request_id: attach_id,
                directory_id: DirectoryId::parse("directory-1")?,
            },
        ))),
    );
    assert_roundtrip(&attach)?;

    let list_threads = envelope(
        "request-list-threads",
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ListProjectThreads(
            ListProjectThreads {
                project_id: ProjectId::parse("project-1")?,
            },
        ))),
    );
    assert_roundtrip(&list_threads)?;

    let create_id = request_id("request-create-thread");
    let create = envelope(
        "request-create-thread",
        WireEnvelopeBody::Request(ClientRequest::Command(Command::CreateThread(
            CreateThread {
                request_id: create_id,
                project_id: ProjectId::parse("project-1")?,
                title: ThreadTitle::parse("New thread")?,
            },
        ))),
    );
    assert_roundtrip(&create)?;

    let queue_id = request_id("request-queue-message");
    let queue = envelope(
        "request-queue-message",
        WireEnvelopeBody::Request(ClientRequest::Command(Command::QueueFirstMessage(
            QueueFirstMessage {
                request_id: queue_id,
                thread_id: ThreadId::parse("thread-1")?,
                body: MessageBody::parse("Preserve this body exactly. 🦀")?,
            },
        ))),
    );
    assert_roundtrip(&queue)
}

#[test]
fn command_frame_id_is_the_stable_domain_request_id() {
    let value = envelope(
        "frame-id",
        WireEnvelopeBody::Request(ClientRequest::Command(Command::AttachProject(
            AttachProject {
                request_id: request_id("different-request-id"),
                directory_id: DirectoryId::parse("directory-1").expect("fixture id is valid"),
            },
        ))),
    );
    assert!(matches!(
        encode_envelope(&value),
        Err(ProtocolEncodeError::Value(
            ProtocolValueError::RequestCorrelationMismatch
        ))
    ));
}

#[test]
fn every_response_family_roundtrips_with_independent_server_frames() -> Result<(), Box<dyn Error>> {
    let places = vec![DirectoryPlace {
        directory_id: DirectoryId::parse("home-directory")?,
        display_name: DisplayName::parse("Home")?,
        kind: PlaceKind::Home,
    }];
    let entries = vec![DirectoryEntry {
        directory_id: DirectoryId::parse("child-directory")?,
        display_name: DisplayName::parse("Child")?,
        kind: DirectoryKind::Directory,
        has_children: true,
    }];
    let listing = DirectoryListing::new(
        places,
        entries,
        Some(DirectoryId::parse("parent-directory")?),
    )?;
    assert_roundtrip(&envelope(
        "server-frame-directory-list",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("request-directory-list"),
            payload: ResponsePayload::DirectoryListing(listing),
        }),
    ))?;

    assert_roundtrip(&envelope(
        "server-frame-project-list",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("request-project-list"),
            payload: ResponsePayload::ProjectListing(ProjectListing::new(vec![
                project();
                PROJECT_LISTING_MAX_PROJECTS
            ])?),
        }),
    ))?;

    assert_roundtrip(&envelope(
        "server-frame-attached",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("request-attach"),
            payload: ResponsePayload::AttachedProject {
                project: project(),
                disposition: ReceiptDisposition::Duplicate,
            },
        }),
    ))?;

    assert_roundtrip(&envelope(
        "server-frame-thread-list",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("request-thread-list"),
            payload: ResponsePayload::ThreadListing(ThreadListing::new(vec![thread()])?),
        }),
    ))?;

    assert_roundtrip(&envelope(
        "server-frame-created",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("request-create"),
            payload: ResponsePayload::CreatedThread {
                thread: thread(),
                disposition: ReceiptDisposition::Accepted,
            },
        }),
    ))?;

    let queued_request = request_id("request-queued");
    assert_roundtrip(&envelope(
        "server-frame-queued",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: queued_request.clone(),
            payload: ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                request_id: queued_request,
                message_id: MessageId::parse("message-1")?,
                thread_id: ThreadId::parse("thread-1")?,
                disposition: ReceiptDisposition::Duplicate,
            }),
        }),
    ))
}

#[test]
fn nested_receipt_correlation_cannot_disagree() {
    let value = envelope(
        "server-frame",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("outer-request"),
            payload: ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                request_id: request_id("inner-request"),
                message_id: MessageId::parse("message-1").expect("fixture id is valid"),
                thread_id: ThreadId::parse("thread-1").expect("fixture id is valid"),
                disposition: ReceiptDisposition::Accepted,
            }),
        }),
    );
    assert!(matches!(
        encode_envelope(&value),
        Err(ProtocolEncodeError::Value(
            ProtocolValueError::ResponseCorrelationMismatch
        ))
    ));
}

#[test]
fn every_event_and_error_family_roundtrips() -> Result<(), Box<dyn Error>> {
    assert_roundtrip(&envelope(
        "server-event-project",
        WireEnvelopeBody::Event(ServerEvent {
            cursor: EventCursor::new(1)?,
            event: Event::ProjectAttached(ProjectAttached { project: project() }),
        }),
    ))?;
    assert_roundtrip(&envelope(
        "server-event-thread",
        WireEnvelopeBody::Event(ServerEvent {
            cursor: EventCursor::new(2)?,
            event: Event::ThreadCreated(ThreadCreated { thread: thread() }),
        }),
    ))?;
    assert_roundtrip(&envelope(
        "server-event-message",
        WireEnvelopeBody::Event(ServerEvent {
            cursor: EventCursor::new(3)?,
            event: Event::FirstMessageQueued(FirstMessageQueued {
                message: QueuedMessage {
                    request_id: request_id("request-message"),
                    message_id: MessageId::parse("message-1")?,
                    thread_id: ThreadId::parse("thread-1")?,
                    body: MessageBody::parse("The event retains this complete body.")?,
                },
            }),
        }),
    ))?;

    assert_roundtrip(&envelope(
        "server-error-correlated",
        WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::DirectoryUnknown,
            detail: ErrorDetail::parse("opaque directory id expired")?,
            retryable: false,
            request_id: Some(request_id("request-directory")),
        }),
    ))?;
    assert_roundtrip(&envelope(
        "server-error-uncorrelated",
        WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::UnsupportedVersion,
            detail: ErrorDetail::default(),
            retryable: false,
            request_id: None,
        }),
    ))
}

fn raw_envelope() -> Builder<HeapAllocator> {
    Builder::new(HeapAllocator::new())
}

#[test]
fn malformed_version_and_capability_return_typed_errors() {
    let unsupported = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(99);
        root.set_message_id("hello-frame");
        root.reborrow().init_body().init_hello();
        serialize::write_message_to_words(&message)
    };
    assert!(matches!(
        decode_envelope(&unsupported),
        Err(ProtocolDecodeError::ProtocolValue {
            source: ProtocolValueError::UnsupportedVersion { version: 99 }
        })
    ));

    let short_initial_capability = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("hello-frame");
        let mut hello = root.reborrow().init_body().init_hello();
        hello.reborrow().init_supported_versions(1).set(0, 1);
        hello.reborrow().init_credential().set_initial(&[0x5a; 31]);
        serialize::write_message_to_words(&message)
    };
    let Err(error) = decode_envelope(&short_initial_capability) else {
        panic!("31-byte initial hello capability must be rejected");
    };
    assert!(matches!(
        &error,
        ProtocolDecodeError::LocalCapability {
            source: LocalCapabilityError::InvalidLength {
                length: 31,
                expected: 32
            }
        }
    ));
    let rendered = error.to_string();
    assert!(!rendered.contains("0x5a"));
    assert!(!rendered.contains("[90"));

    let short_reconnect_capability = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("reconnect-hello-frame");
        let mut hello = root.reborrow().init_body().init_hello();
        hello.reborrow().init_supported_versions(1).set(0, 1);
        hello
            .reborrow()
            .init_credential()
            .set_reconnect(&[0x6b; 31]);
        serialize::write_message_to_words(&message)
    };
    let Err(error) = decode_envelope(&short_reconnect_capability) else {
        panic!("31-byte reconnect hello capability must be rejected");
    };
    assert!(matches!(
        &error,
        ProtocolDecodeError::ReconnectCapability {
            source: ReconnectCapabilityError::InvalidLength {
                length: 31,
                expected: RECONNECT_CAPABILITY_BYTES,
            }
        }
    ));
    let rendered = error.to_string();
    assert!(!rendered.contains("0x6b"));
    assert!(!rendered.contains("[107"));

    let short_welcome_capability = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("welcome-frame");
        let mut welcome = root.reborrow().init_body().init_welcome();
        welcome.set_negotiated_version(1);
        welcome.set_connection_id("connection-1");
        welcome.set_reconnect_capability(&[0x7c; 31]);
        serialize::write_message_to_words(&message)
    };
    let Err(error) = decode_envelope(&short_welcome_capability) else {
        panic!("31-byte rotated welcome capability must be rejected");
    };
    assert!(matches!(
        &error,
        ProtocolDecodeError::ReconnectCapability {
            source: ReconnectCapabilityError::InvalidLength {
                length: 31,
                expected: RECONNECT_CAPABILITY_BYTES,
            }
        }
    ));
    let rendered = error.to_string();
    assert!(!rendered.contains("0x7c"));
    assert!(!rendered.contains("[124"));
}

fn raw_hello(version_count: u32) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("hello-frame");
    let mut hello = root.reborrow().init_body().init_hello();
    let mut versions = hello.reborrow().init_supported_versions(version_count);
    for index in 0..version_count {
        versions.set(index, 1);
    }
    hello
        .reborrow()
        .init_credential()
        .set_initial(&INITIAL_CAPABILITY);
    serialize::write_message_to_words(&message)
}

fn raw_hello_credential(reconnect: bool) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("hello-credential-frame");
    let mut hello = root.reborrow().init_body().init_hello();
    hello.reborrow().init_supported_versions(1).set(0, 1);
    let mut credential = hello.reborrow().init_credential();
    if reconnect {
        credential.set_reconnect(&INITIAL_CAPABILITY);
    } else {
        credential.set_initial(&INITIAL_CAPABILITY);
    }
    serialize::write_message_to_words(&message)
}

#[test]
fn malformed_hello_collection_bounds_return_typed_errors() {
    assert!(matches!(
        decode_envelope(&raw_hello(0)),
        Err(ProtocolDecodeError::VersionOffer {
            source: VersionOfferError::Empty
        })
    ));
    assert!(matches!(
        decode_envelope(&raw_hello(9)),
        Err(ProtocolDecodeError::VersionOffer {
            source: VersionOfferError::TooMany {
                count: 9,
                maximum: 8
            }
        })
    ));
}

fn raw_directory_listing(place_count: u32, entry_count: u32) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("server-directory-frame");
    let mut response = root.reborrow().init_body().init_response();
    response.set_request_id("request-directory-list");
    let mut listing = response.init_directory_list();
    listing.reborrow().init_parent().set_no_parent(());
    listing.reborrow().init_places(place_count);
    listing.init_entries(entry_count);
    serialize::write_message_to_words(&message)
}

fn raw_thread_listing(thread_count: u32) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("server-thread-frame");
    let mut response = root.reborrow().init_body().init_response();
    response.set_request_id("request-thread-list");
    response.init_thread_list().init_threads(thread_count);
    serialize::write_message_to_words(&message)
}

fn raw_project_listing(project_count: u32) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("server-project-frame");
    let mut response = root.reborrow().init_body().init_response();
    response.set_request_id("request-project-list");
    response.init_project_list().init_projects(project_count);
    serialize::write_message_to_words(&message)
}

#[test]
fn oversized_wire_collections_fail_before_element_conversion() {
    let too_many_places =
        u32::try_from(DIRECTORY_LISTING_MAX_PLACES + 1).expect("the protocol place bound fits u32");
    assert!(matches!(
        decode_envelope(&raw_directory_listing(too_many_places, 0)),
        Err(ProtocolDecodeError::DirectoryListing {
            source: DirectoryListingError::TooManyPlaces {
                count,
                maximum: DIRECTORY_LISTING_MAX_PLACES
            }
        }) if count == DIRECTORY_LISTING_MAX_PLACES + 1
    ));

    let too_many_entries = u32::try_from(DIRECTORY_LISTING_MAX_ENTRIES + 1)
        .expect("the protocol entry bound fits u32");
    assert!(matches!(
        decode_envelope(&raw_directory_listing(0, too_many_entries)),
        Err(ProtocolDecodeError::DirectoryListing {
            source: DirectoryListingError::TooManyEntries {
                count,
                maximum: DIRECTORY_LISTING_MAX_ENTRIES
            }
        }) if count == DIRECTORY_LISTING_MAX_ENTRIES + 1
    ));

    let too_many_threads =
        u32::try_from(THREAD_LISTING_MAX_THREADS + 1).expect("the protocol thread bound fits u32");
    assert!(matches!(
        decode_envelope(&raw_thread_listing(too_many_threads)),
        Err(ProtocolDecodeError::ThreadListing {
            source: ThreadListingError::TooManyThreads {
                count,
                maximum: THREAD_LISTING_MAX_THREADS
            }
        }) if count == THREAD_LISTING_MAX_THREADS + 1
    ));

    let too_many_projects = u32::try_from(PROJECT_LISTING_MAX_PROJECTS + 1)
        .expect("the protocol project bound fits u32");
    assert!(matches!(
        decode_envelope(&raw_project_listing(too_many_projects)),
        Err(ProtocolDecodeError::ProjectListing {
            source: ProjectListingError::TooManyProjects {
                count,
                maximum: PROJECT_LISTING_MAX_PROJECTS
            }
        }) if count == PROJECT_LISTING_MAX_PROJECTS + 1
    ));
}

fn raw_event(cursor: u64) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("server-event-frame");
    let mut event = root.reborrow().init_body().init_event();
    event.set_cursor(cursor);
    event.init_project_attached();
    serialize::write_message_to_words(&message)
}

#[test]
fn zero_server_event_cursor_is_rejected_before_payload_conversion() {
    assert!(matches!(
        EventCursor::new(0),
        Err(ProtocolValueError::ZeroEventCursor)
    ));
    assert!(matches!(
        decode_envelope(&raw_event(0)),
        Err(ProtocolDecodeError::ProtocolValue {
            source: ProtocolValueError::ZeroEventCursor
        })
    ));
}

fn raw_protocol_error(code: WireErrorCode) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("server-error-frame");
    let mut error = root.reborrow().init_body().init_protocol_error();
    error.set_code(code);
    error.set_message("");
    error.set_retryable(false);
    error.set_uncorrelated(());
    serialize::write_message_to_words(&message)
}

#[test]
fn unknown_wire_enum_discriminant_is_typed() {
    let mut malformed = raw_protocol_error(WireErrorCode::UnsupportedVersion);
    let comparison = raw_protocol_error(WireErrorCode::Internal);
    let differing: Vec<usize> = malformed
        .iter()
        .zip(comparison)
        .enumerate()
        .filter_map(|(index, (left, right))| (left != &right).then_some(index))
        .collect();
    assert_eq!(differing.len(), 1, "only the enum ordinal should differ");
    malformed[differing[0]] = u8::MAX;

    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::UnknownDiscriminant { value: 255 })
    ));
}

#[test]
fn unknown_hello_credential_discriminant_is_typed() {
    let mut malformed = raw_hello_credential(false);
    let comparison = raw_hello_credential(true);
    let differing: Vec<usize> = malformed
        .iter()
        .zip(comparison)
        .enumerate()
        .filter_map(|(index, (left, right))| (left != &right).then_some(index))
        .collect();
    assert_eq!(
        differing.len(),
        1,
        "only the credential union ordinal should differ"
    );
    malformed[differing[0]] = u8::MAX;

    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::UnknownDiscriminant { value: 255 })
    ));
}

#[test]
fn decoder_accepts_exactly_one_message_without_trailing_bytes() {
    let encoded = encode_envelope(&envelope(
        "server-welcome-frame",
        WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: ProtocolVersion::V1,
            connection_id: ConnectionId::parse("connection-1")
                .expect("fixture connection id is valid"),
            reconnect_capability: ReconnectCapability::from_bytes(ROTATED_RECONNECT_CAPABILITY),
        }),
    ))
    .expect("fixture envelope encodes");

    let mut concatenated = encoded.clone();
    concatenated.extend_from_slice(&encoded);
    assert!(matches!(
        decode_envelope(&concatenated),
        Err(ProtocolDecodeError::TrailingBytes { length }) if length == encoded.len()
    ));

    let mut padded = encoded;
    padded.extend_from_slice(&[0; 8]);
    assert!(matches!(
        decode_envelope(&padded),
        Err(ProtocolDecodeError::TrailingBytes { length: 8 })
    ));
}

#[test]
fn decoder_enforces_explicit_capnp_traversal_limit() {
    assert_eq!(CAPNP_TRAVERSAL_LIMIT_WORDS, 8 * 1024 * 1024);
    assert_eq!(CAPNP_NESTING_LIMIT, 32);
    let too_many_words = u32::try_from(CAPNP_TRAVERSAL_LIMIT_WORDS + 1)
        .expect("the traversal policy fits a u32 segment table");
    let mut segment_table = Vec::with_capacity(8);
    segment_table.extend_from_slice(&0_u32.to_le_bytes());
    segment_table.extend_from_slice(&too_many_words.to_le_bytes());

    assert!(matches!(
        decode_envelope(&segment_table),
        Err(ProtocolDecodeError::Capnp { .. })
    ));
}

#[test]
fn invalid_wire_text_returns_field_specific_utf8_error() {
    const TARGET: &str = "wire-utf8-target";

    let mut malformed = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("server-welcome-frame");
        let mut welcome = root.reborrow().init_body().init_welcome();
        welcome.set_negotiated_version(1);
        welcome.set_connection_id(TARGET);
        serialize::write_message_to_words(&message)
    };
    let offset = malformed
        .windows(TARGET.len())
        .position(|window| window == TARGET.as_bytes())
        .expect("the encoded fixture contains its connection id once");
    malformed[offset] = u8::MAX;

    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::InvalidUtf8 {
            field: "welcome.connectionId",
            ..
        })
    ));
}
