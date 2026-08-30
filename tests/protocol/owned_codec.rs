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
use artisan_protocol::artisan_capnp::{
    ErrorCode as WireErrorCode, LifecycleState as WireLifecycleState,
    LifecycleStopDisposition as WireLifecycleStopDisposition, envelope,
};
use artisan_protocol::{
    CAPNP_NESTING_LIMIT, CAPNP_TRAVERSAL_LIMIT_WORDS, ClientRequest, ConnectionId,
    DirectoryPickOutcome, DispatchFailure, ErrorCode, ErrorDetail, EventCursor,
    FirstMessageReceipt, FrameId, Hello, HelloCredential, LifecycleRequest, LifecycleResponse,
    LifecycleState, LifecycleStatus, LifecycleStopDisposition, LifecycleStopReceipt,
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
            supports_lifecycle_control: false,
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
            supports_lifecycle_control: false,
        }),
    );
    assert_roundtrip(&reconnect_hello)?;

    let welcome = envelope(
        "server-welcome-1",
        WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: ProtocolVersion::V1,
            connection_id: ConnectionId::parse("connection-1")?,
            reconnect_capability: ReconnectCapability::from_bytes(ROTATED_RECONNECT_CAPABILITY),
            lifecycle_control_supported: false,
        }),
    );
    assert_roundtrip(&welcome)
}

#[test]
fn negotiated_lifecycle_owned_values_roundtrip() -> Result<(), Box<dyn Error>> {
    let hello = envelope(
        "client-lifecycle-hello",
        WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])?,
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
            supports_lifecycle_control: true,
        }),
    );
    let decoded_hello = decode_envelope(&encode_envelope(&hello)?)?;
    let WireEnvelopeBody::Hello(decoded_hello) = decoded_hello.body else {
        panic!("expected hello body");
    };
    assert!(decoded_hello.supports_lifecycle_control);

    let welcome = envelope(
        "server-lifecycle-welcome",
        WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: ProtocolVersion::V1,
            connection_id: ConnectionId::parse("connection-lifecycle")?,
            reconnect_capability: ReconnectCapability::from_bytes(ROTATED_RECONNECT_CAPABILITY),
            lifecycle_control_supported: true,
        }),
    );
    let decoded_welcome = decode_envelope(&encode_envelope(&welcome)?)?;
    let WireEnvelopeBody::Welcome(decoded_welcome) = decoded_welcome.body else {
        panic!("expected welcome body");
    };
    assert!(decoded_welcome.lifecycle_control_supported);

    for (frame, request) in [
        ("client-lifecycle-status", LifecycleRequest::Status),
        (
            "client-lifecycle-stop-false",
            LifecycleRequest::Stop {
                require_idle: false,
            },
        ),
        (
            "client-lifecycle-stop-true",
            LifecycleRequest::Stop { require_idle: true },
        ),
    ] {
        let value = envelope(
            frame,
            WireEnvelopeBody::Request(ClientRequest::Lifecycle(request)),
        );
        let decoded = decode_envelope(&encode_envelope(&value)?)?;
        assert_eq!(decoded.frame_id.as_str(), frame);
        assert!(
            decoded == value,
            "lifecycle request must survive field-for-field"
        );
    }

    for (index, (state, count)) in [
        (LifecycleState::Ready, 0),
        (LifecycleState::Busy, 1),
        (LifecycleState::Draining, 7),
    ]
    .into_iter()
    .enumerate()
    {
        let response_id = request_id(&format!("client-lifecycle-status-{index}"));
        let response = envelope(
            &format!("server-lifecycle-status-{index}"),
            WireEnvelopeBody::Response(ServerResponse {
                request_id: response_id,
                payload: ResponsePayload::Lifecycle(LifecycleResponse::Status(
                    LifecycleStatus::new(state, count)?,
                )),
            }),
        );
        assert_roundtrip(&response)?;
    }

    for (index, disposition) in [
        LifecycleStopDisposition::Accepted,
        LifecycleStopDisposition::Duplicate,
        LifecycleStopDisposition::AlreadyStopping,
    ]
    .into_iter()
    .enumerate()
    {
        let response = envelope(
            &format!("server-lifecycle-stop-{index}"),
            WireEnvelopeBody::Response(ServerResponse {
                request_id: request_id(&format!("client-lifecycle-stop-{index}")),
                payload: ResponsePayload::Lifecycle(LifecycleResponse::Stop(
                    LifecycleStopReceipt {
                        disposition,
                        state: LifecycleState::Draining,
                    },
                )),
            }),
        );
        assert_roundtrip(&response)?;
    }

    Ok(())
}

#[test]
fn lifecycle_status_validation_is_checked_and_typed() {
    assert!(LifecycleStatus::new(LifecycleState::Ready, 0).is_ok());
    assert!(LifecycleStatus::new(LifecycleState::Busy, 1).is_ok());
    assert!(LifecycleStatus::new(LifecycleState::Draining, 0).is_ok());
    assert!(LifecycleStatus::new(LifecycleState::Draining, u32::MAX).is_ok());
    assert_eq!(
        LifecycleStatus::new(LifecycleState::Ready, 1),
        Err(ProtocolValueError::InvalidLifecycleStatus {
            state: LifecycleState::Ready,
            active_work_count: 1,
        })
    );
    assert_eq!(
        LifecycleStatus::new(LifecycleState::Busy, 0),
        Err(ProtocolValueError::InvalidLifecycleStatus {
            state: LifecycleState::Busy,
            active_work_count: 0,
        })
    );
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
            payload: ResponsePayload::ProjectListing(ProjectListing::new(
                (0..PROJECT_LISTING_MAX_PROJECTS)
                    .map(|index| ProjectSummary {
                        project_id: ProjectId::parse(format!("project-{index}"))
                            .expect("fixture project id is valid"),
                        ..project()
                    })
                    .collect(),
            )?),
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
fn pick_directory_request_roundtrips_as_a_unit_host_interaction() -> Result<(), Box<dyn Error>> {
    let request_frame = envelope(
        "request-pick-directory",
        WireEnvelopeBody::Request(ClientRequest::PickDirectory),
    );
    assert_roundtrip(&request_frame)?;

    // The client-minted frame identity survives untouched; a deliberate new
    // attempt simply mints a different one.
    let decoded = decode_envelope(&encode_envelope(&request_frame)?)?;
    assert!(matches!(
        decoded.body,
        WireEnvelopeBody::Request(ClientRequest::PickDirectory)
    ));
    assert_eq!(decoded.frame_id.as_str(), "request-pick-directory");
    Ok(())
}

#[test]
fn directory_pick_outcomes_roundtrip_with_independent_server_frames() -> Result<(), Box<dyn Error>>
{
    let selected = envelope(
        "server-frame-pick-selected",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("request-pick"),
            payload: ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Selected(
                DirectoryId::parse("directory-picked-1")?,
            )),
        }),
    );
    assert_roundtrip(&selected)?;
    let decoded = decode_envelope(&encode_envelope(&selected)?)?;
    let WireEnvelopeBody::Response(response) = decoded.body else {
        panic!("expected a response body");
    };
    // The server-minted frame identity stays independent of the echoed
    // client request identity, and Selected carries only the opaque id.
    assert_eq!(decoded.frame_id.as_str(), "server-frame-pick-selected");
    assert_ne!(decoded.frame_id.as_str(), response.request_id.as_str());
    let ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Selected(directory_id)) =
        response.payload
    else {
        panic!("expected a directory-picked payload");
    };
    assert_eq!(directory_id.as_str(), "directory-picked-1");

    let cancelled = envelope(
        "server-frame-pick-cancelled",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("request-pick-cancelled"),
            payload: ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Cancelled),
        }),
    );
    assert_roundtrip(&cancelled)?;
    let decoded = decode_envelope(&encode_envelope(&cancelled)?)?;
    assert_ne!(decoded.frame_id.as_str(), "request-pick-cancelled");
    let WireEnvelopeBody::Response(response) = decoded.body else {
        panic!("expected a response body");
    };
    assert_eq!(response.request_id, request_id("request-pick-cancelled"));
    assert!(matches!(
        response.payload,
        ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Cancelled)
    ));
    Ok(())
}

#[test]
fn malformed_selected_directory_id_is_rejected_through_the_decoder() {
    let malformed_selected = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("server-frame-pick-selected");
        let mut response = root.reborrow().init_body().init_response();
        response.set_request_id("request-pick");
        response
            .reborrow()
            .init_directory_picked()
            .set_selected("not a valid directory id");
        serialize::write_message_to_words(&message)
    };
    assert!(matches!(
        decode_envelope(&malformed_selected),
        Err(ProtocolDecodeError::Identifier {
            field: "response.directoryPicked.selected",
            ..
        })
    ));
}

#[test]
fn appended_pick_directory_wire_arms_decode_from_raw_frames() -> Result<(), Box<dyn Error>> {
    let raw_request = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("request-pick-directory");
        root.reborrow()
            .init_body()
            .init_request()
            .reborrow()
            .set_pick_directory(());
        serialize::write_message_to_words(&message)
    };
    assert!(matches!(
        decode_envelope(&raw_request)?.body,
        WireEnvelopeBody::Request(ClientRequest::PickDirectory)
    ));

    let raw_cancelled = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("server-frame-pick-cancelled");
        let mut response = root.reborrow().init_body().init_response();
        response.set_request_id("request-pick-cancelled");
        response
            .reborrow()
            .init_directory_picked()
            .set_cancelled(());
        serialize::write_message_to_words(&message)
    };
    assert!(matches!(
        decode_envelope(&raw_cancelled)?.body,
        WireEnvelopeBody::Response(ServerResponse {
            payload: ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Cancelled),
            ..
        })
    ));

    Ok(())
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

#[test]
fn dispatch_failure_names_exactly_the_request_it_settles() -> Result<(), Box<dyn Error>> {
    let settled = envelope(
        "request-dispatch",
        WireEnvelopeBody::Request(ClientRequest::Command(Command::QueueFirstMessage(
            QueueFirstMessage {
                request_id: request_id("request-dispatch"),
                thread_id: ThreadId::parse("thread-1")?,
                body: MessageBody::parse("The dispatched message.")?,
            },
        ))),
    );
    let rejection = DispatchFailure::settling(
        &settled,
        ErrorCode::ThreadUnknown,
        ErrorDetail::parse("no thread matches the referenced thread id")?,
        false,
    )
    .expect("a request frame always yields a correlated dispatch failure");

    // The failure carries the settled request's identity, not a separately
    // supplied id that could drift from it.
    assert_eq!(rejection.code(), ErrorCode::ThreadUnknown);
    assert_eq!(
        rejection.detail().as_str(),
        "no thread matches the referenced thread id"
    );
    assert!(!rejection.retryable());
    assert_eq!(rejection.request_id(), &request_id("request-dispatch"));

    // Conversion always selects the correlated arm with the same identity.
    let failure = ProtocolFailure::from(rejection);
    assert_eq!(failure.code, ErrorCode::ThreadUnknown);
    assert_eq!(
        failure.detail.as_str(),
        "no thread matches the referenced thread id"
    );
    assert!(!failure.retryable);
    assert_eq!(
        failure.request_id.as_ref(),
        Some(&request_id("request-dispatch"))
    );

    // Correlation survives the wire and never conflates the echoed client
    // RequestId with the server-minted FrameId of the failure frame itself.
    let decoded = decode_envelope(&encode_envelope(&envelope(
        "server-error-dispatch",
        WireEnvelopeBody::ProtocolError(failure),
    ))?)?;
    let WireEnvelopeBody::ProtocolError(decoded_failure) = decoded.body else {
        panic!("expected a protocol error body");
    };
    assert_eq!(decoded.frame_id.as_str(), "server-error-dispatch");
    assert_eq!(
        decoded_failure.request_id.as_ref(),
        Some(&request_id("request-dispatch"))
    );
    Ok(())
}

#[test]
fn failures_without_a_settled_request_stay_uncorrelated() -> Result<(), Box<dyn Error>> {
    let hello = envelope(
        "client-hello-frame",
        WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])?,
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
            supports_lifecycle_control: false,
        }),
    );
    assert!(
        DispatchFailure::settling(
            &hello,
            ErrorCode::UnsupportedVersion,
            ErrorDetail::default(),
            true
        )
        .is_none()
    );

    let answer = envelope(
        "server-response-frame",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("request-thread-list"),
            payload: ResponsePayload::ThreadListing(ThreadListing::new(vec![thread()])?),
        }),
    );
    assert!(
        DispatchFailure::settling(&answer, ErrorCode::Internal, ErrorDetail::default(), false)
            .is_none()
    );

    // The general failure shape keeps the schema's uncorrelated arm for
    // hello-time rejections that implicate no request at all.
    assert_roundtrip(&envelope(
        "server-version-rejection",
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

fn raw_lifecycle_request(stop: bool, require_idle: bool) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("lifecycle-request-frame");
    let mut request = root.reborrow().init_body().init_request();
    let mut lifecycle = request.init_lifecycle_control();
    if stop {
        lifecycle.init_stop().set_require_idle(require_idle);
    } else {
        lifecycle.init_status();
    }
    serialize::write_message_to_words(&message)
}

fn raw_lifecycle_status(state: WireLifecycleState, active_work_count: u32) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("lifecycle-response-frame");
    let mut response = root.reborrow().init_body().init_response();
    response.set_request_id("lifecycle-request-id");
    let mut lifecycle = response.init_lifecycle_control();
    let mut status = lifecycle.init_status();
    status.set_state(state);
    status.set_active_work_count(active_work_count);
    serialize::write_message_to_words(&message)
}

fn raw_lifecycle_stop(
    disposition: WireLifecycleStopDisposition,
    state: WireLifecycleState,
) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("lifecycle-response-frame");
    let mut response = root.reborrow().init_body().init_response();
    response.set_request_id("lifecycle-request-id");
    let mut lifecycle = response.init_lifecycle_control();
    let mut receipt = lifecycle.init_stop();
    receipt.set_disposition(disposition);
    receipt.set_state(state);
    serialize::write_message_to_words(&message)
}

fn raw_lifecycle_response(stop: bool) -> Vec<u8> {
    if stop {
        raw_lifecycle_stop(
            WireLifecycleStopDisposition::Accepted,
            WireLifecycleState::Ready,
        )
    } else {
        raw_lifecycle_status(WireLifecycleState::Ready, 0)
    }
}

fn assert_unknown_discriminant(mut malformed: Vec<u8>, comparison: &[u8]) {
    let compared_length = malformed.len().min(comparison.len());
    for index in 0..compared_length {
        if malformed[index] == comparison[index] {
            continue;
        }
        let original = malformed[index];
        malformed[index] = u8::MAX;
        if matches!(
            decode_envelope(&malformed),
            Err(ProtocolDecodeError::UnknownDiscriminant { value: 255 })
        ) {
            return;
        }
        malformed[index] = original;
    }
    panic!("no changed byte produced an unknown discriminant");
}

#[test]
fn raw_lifecycle_request_arms_preserve_frame_correlation() -> Result<(), Box<dyn Error>> {
    for (stop, require_idle, expected) in [
        (false, false, LifecycleRequest::Status),
        (
            true,
            false,
            LifecycleRequest::Stop {
                require_idle: false,
            },
        ),
        (true, true, LifecycleRequest::Stop { require_idle: true }),
    ] {
        let decoded = decode_envelope(&raw_lifecycle_request(stop, require_idle))?;
        assert_eq!(decoded.frame_id.as_str(), "lifecycle-request-frame");
        assert!(
            decoded.body == WireEnvelopeBody::Request(ClientRequest::Lifecycle(expected)),
            "lifecycle request must decode field-for-field"
        );
    }
    Ok(())
}

#[test]
fn raw_lifecycle_response_arms_preserve_outer_correlation() -> Result<(), Box<dyn Error>> {
    for (index, (state, count)) in [
        (WireLifecycleState::Ready, 0),
        (WireLifecycleState::Busy, 1),
        (WireLifecycleState::Draining, 2),
    ]
    .into_iter()
    .enumerate()
    {
        let decoded = decode_envelope(&raw_lifecycle_status(state, count))?;
        assert_eq!(decoded.frame_id.as_str(), "lifecycle-response-frame");
        let WireEnvelopeBody::Response(response) = decoded.body else {
            panic!("expected lifecycle status response");
        };
        assert_eq!(response.request_id, request_id("lifecycle-request-id"));
        assert_eq!(
            response.payload,
            ResponsePayload::Lifecycle(LifecycleResponse::Status(LifecycleStatus::new(
                [
                    LifecycleState::Ready,
                    LifecycleState::Busy,
                    LifecycleState::Draining
                ][index],
                count,
            )?,))
        );
    }

    for (state, expected_state) in [
        (WireLifecycleState::Ready, LifecycleState::Ready),
        (WireLifecycleState::Busy, LifecycleState::Busy),
        (WireLifecycleState::Draining, LifecycleState::Draining),
    ] {
        for (disposition, expected_disposition) in [
            (
                WireLifecycleStopDisposition::Accepted,
                LifecycleStopDisposition::Accepted,
            ),
            (
                WireLifecycleStopDisposition::Duplicate,
                LifecycleStopDisposition::Duplicate,
            ),
            (
                WireLifecycleStopDisposition::AlreadyStopping,
                LifecycleStopDisposition::AlreadyStopping,
            ),
        ] {
            let decoded = decode_envelope(&raw_lifecycle_stop(disposition, state))?;
            let WireEnvelopeBody::Response(response) = decoded.body else {
                panic!("expected lifecycle stop response");
            };
            assert_eq!(response.request_id, request_id("lifecycle-request-id"));
            assert_eq!(
                response.payload,
                ResponsePayload::Lifecycle(LifecycleResponse::Stop(LifecycleStopReceipt {
                    disposition: expected_disposition,
                    state: expected_state,
                }))
            );
        }
    }
    Ok(())
}

#[test]
fn lifecycle_feature_fields_roundtrip_true_and_default_false() -> Result<(), Box<dyn Error>> {
    let true_hello = envelope(
        "lifecycle-feature-hello",
        WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])?,
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
            supports_lifecycle_control: true,
        }),
    );
    let WireEnvelopeBody::Hello(decoded_hello) =
        decode_envelope(&encode_envelope(&true_hello)?)?.body
    else {
        panic!("expected hello body");
    };
    assert!(decoded_hello.supports_lifecycle_control);

    let true_welcome = envelope(
        "lifecycle-feature-welcome",
        WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: ProtocolVersion::V1,
            connection_id: ConnectionId::parse("connection-feature")?,
            reconnect_capability: ReconnectCapability::from_bytes(ROTATED_RECONNECT_CAPABILITY),
            lifecycle_control_supported: true,
        }),
    );
    let WireEnvelopeBody::Welcome(decoded_welcome) =
        decode_envelope(&encode_envelope(&true_welcome)?)?.body
    else {
        panic!("expected welcome body");
    };
    assert!(decoded_welcome.lifecycle_control_supported);

    let WireEnvelopeBody::Hello(old_hello) = decode_envelope(&raw_hello_credential(false))?.body
    else {
        panic!("expected old hello fixture");
    };
    assert!(!old_hello.supports_lifecycle_control);

    let old_welcome = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("old-welcome-frame");
        let mut welcome = root.reborrow().init_body().init_welcome();
        welcome.set_negotiated_version(1);
        welcome.set_connection_id("connection-old");
        welcome.set_reconnect_capability(&ROTATED_RECONNECT_CAPABILITY);
        serialize::write_message_to_words(&message)
    };
    let WireEnvelopeBody::Welcome(old_welcome) = decode_envelope(&old_welcome)?.body else {
        panic!("expected old welcome fixture");
    };
    assert!(!old_welcome.lifecycle_control_supported);
    Ok(())
}

#[test]
fn lifecycle_error_codes_roundtrip_at_appended_ordinals() -> Result<(), Box<dyn Error>> {
    assert_eq!(WireErrorCode::UnsupportedFeature as u16, 7);
    assert_eq!(WireErrorCode::LifecycleBusy as u16, 8);
    assert_roundtrip(&envelope(
        "server-unsupported-feature",
        WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::UnsupportedFeature,
            detail: ErrorDetail::parse("lifecycle control was not negotiated")?,
            retryable: false,
            request_id: None,
        }),
    ))?;
    assert_roundtrip(&envelope(
        "server-lifecycle-busy",
        WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::LifecycleBusy,
            detail: ErrorDetail::parse("lifecycle work is still active")?,
            retryable: true,
            request_id: Some(request_id("lifecycle-request-id")),
        }),
    ))?;
    Ok(())
}

#[test]
fn invalid_lifecycle_statuses_are_rejected_at_owned_boundaries() {
    for (state, count) in [
        (WireLifecycleState::Ready, 1),
        (WireLifecycleState::Busy, 0),
    ] {
        assert!(matches!(
            decode_envelope(&raw_lifecycle_status(state, count)),
            Err(ProtocolDecodeError::ProtocolValue {
                source: ProtocolValueError::InvalidLifecycleStatus {
                    state: _,
                    active_work_count: _
                }
            })
        ));
    }

    let invalid = envelope(
        "server-invalid-lifecycle-status",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id("lifecycle-request-id"),
            payload: ResponsePayload::Lifecycle(LifecycleResponse::Status(LifecycleStatus {
                state: LifecycleState::Ready,
                active_work_count: 1,
            })),
        }),
    );
    assert!(matches!(
        encode_envelope(&invalid),
        Err(ProtocolEncodeError::Value(
            ProtocolValueError::InvalidLifecycleStatus {
                state: LifecycleState::Ready,
                active_work_count: 1
            }
        ))
    ));
}

#[test]
fn unknown_lifecycle_discriminants_remain_typed_failures() {
    assert_unknown_discriminant(
        raw_lifecycle_request(false, false),
        &raw_lifecycle_request(true, false),
    );
    assert_unknown_discriminant(raw_lifecycle_response(false), &raw_lifecycle_response(true));
    assert_unknown_discriminant(
        raw_lifecycle_status(WireLifecycleState::Ready, 0),
        &raw_lifecycle_status(WireLifecycleState::Busy, 1),
    );
    assert_unknown_discriminant(
        raw_lifecycle_stop(
            WireLifecycleStopDisposition::Accepted,
            WireLifecycleState::Ready,
        ),
        &raw_lifecycle_stop(
            WireLifecycleStopDisposition::Duplicate,
            WireLifecycleState::Ready,
        ),
    );
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
fn idempotency_conflict_roundtrips_correlated_and_never_retryable() -> Result<(), Box<dyn Error>> {
    let detail = ErrorDetail::parse("request id already accepted for a different command")?;
    let correlation = request_id("request-conflicting-retry");
    let frame = envelope(
        "server-error-idempotency-conflict",
        WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::IdempotencyConflict,
            detail: detail.clone(),
            retryable: false,
            request_id: Some(correlation.clone()),
        }),
    );

    // The failure survives field for field through the owned codec.
    let decoded = decode_envelope(&encode_envelope(&frame)?)?;
    assert!(
        decoded == frame,
        "owned envelope must survive field-for-field"
    );
    match decoded.body {
        WireEnvelopeBody::ProtocolError(failure) => {
            assert_eq!(failure.code, ErrorCode::IdempotencyConflict);
            assert_eq!(failure.detail, detail);
            assert!(
                !failure.retryable,
                "repeating a conflicting request must stay non-retryable"
            );
            assert_eq!(failure.request_id.as_ref(), Some(&correlation));
        }
        _ => panic!("expected protocolError body"),
    }

    // Raw-schema ordinal guard: every earlier enumerator keeps its committed
    // ordinal, the appended classification owns exactly the next unused one
    // at @6, and that raw wire value decodes into the owned variant.
    assert_eq!(WireErrorCode::UnsupportedVersion as u16, 0);
    assert_eq!(WireErrorCode::InvalidInput as u16, 1);
    assert_eq!(WireErrorCode::DirectoryUnknown as u16, 2);
    assert_eq!(WireErrorCode::ProjectUnknown as u16, 3);
    assert_eq!(WireErrorCode::ThreadUnknown as u16, 4);
    assert_eq!(WireErrorCode::Internal as u16, 5);
    assert_eq!(WireErrorCode::IdempotencyConflict as u16, 6);
    assert!(matches!(
        decode_envelope(&raw_protocol_error(WireErrorCode::IdempotencyConflict))?.body,
        WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::IdempotencyConflict,
            ..
        })
    ));

    Ok(())
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
            lifecycle_control_supported: false,
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
