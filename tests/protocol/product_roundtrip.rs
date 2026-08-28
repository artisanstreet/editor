//! Phase 2 product schema proof.
//!
//! Exercises every message family of `schema/artisan.capnp` through its
//! Bazel-generated bindings (`artisan_protocol::artisan_capnp`): hello and
//! welcome negotiation including the one-time client capability, all six
//! request/response pairs with optional-parent, place, entry, and
//! attached-project listings, all three events, both receipt dispositions,
//! correlated and uncorrelated protocol errors, and deterministic framing.
//! Wire shape only; owned domain conversions arrive in a later packet.
//!
//! Identifier vocabulary matches the schema header: opaque text ids of at
//! most 128 UTF-8 bytes, nonblank, without Unicode whitespace or control
//! characters. All string bounds are UTF-8 bytes.

use artisan_domain::PROJECT_LISTING_MAX_PROJECTS;
use artisan_protocol::artisan_capnp::{
    DirectoryEntryKind, ErrorCode, PlaceKind, QueuedState, ReceiptDisposition, directory_listing,
    envelope, event, hello, list_attached_projects_request, list_directories_request, project,
    project_list, protocol_error, request, response, thread_summary,
};
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use capnp::serialize;

const PROTOCOL_VERSION: u32 = 1;
const CLIENT_REQUEST_ID: &str = "client-request-000001";
const SENT_AT_MILLIS: i64 = 1_777_777_777_123;
const ATTACHED_AT_MILLIS: i64 = 1_777_777_800_456;
const CREATED_AT_MILLIS: i64 = 1_777_777_900_789;
const UPDATED_AT_MILLIS: i64 = 1_777_777_950_123;
const CONNECTION_ID: &str = "conn-0f1e2d";
/// One-time local client capability: exactly 32 secret bytes handed to the
/// editor out-of-band via restricted parent-child handoff.
const HELLO_CAPABILITY: [u8; 32] = [
    0x5a, 0x2e, 0x91, 0xc4, 0x07, 0xbb, 0x63, 0x18, 0xf0, 0x4d, 0xaa, 0x39, 0x76, 0x0e, 0xd2, 0x81,
    0x9b, 0x44, 0x6c, 0xef, 0x13, 0x58, 0xa7, 0x20, 0xcd, 0x8f, 0x71, 0xb6, 0x02, 0xe9, 0x3d, 0x94,
];
/// Single-use reconnect credential supplied by a reconnecting client.
const RECONNECT_CAPABILITY: [u8; 32] = [
    0xc3, 0x17, 0x4a, 0xf2, 0x69, 0xd8, 0x05, 0xbe, 0x71, 0xac, 0x93, 0x26, 0xe4, 0x5b, 0x18, 0xfd,
    0x80, 0x62, 0xdf, 0x37, 0xa9, 0x14, 0xcb, 0x70, 0x52, 0xe6, 0x9d, 0x48, 0xb1, 0x2f, 0x86, 0x03,
];
/// Fresh single-use reconnect credential rotated by a successful Welcome.
const ROTATED_RECONNECT_CAPABILITY: [u8; 32] = [
    0xd4, 0x28, 0x5b, 0x03, 0x7a, 0xe9, 0x16, 0xcf, 0x82, 0xbd, 0xa4, 0x37, 0xf5, 0x6c, 0x29, 0x0e,
    0x91, 0x73, 0xe0, 0x48, 0xba, 0x25, 0xdc, 0x81, 0x63, 0xf7, 0xae, 0x59, 0xc2, 0x30, 0x97, 0x14,
];
/// Deliberately malformed capability length for negative coverage.
const SHORT_CAPABILITY: [u8; 31] = [0xa5_u8; 31];
/// Deliberately malformed reconnect credential length for negative coverage.
const SHORT_RECONNECT_CAPABILITY: [u8; 31] = [0x3c_u8; 31];
const ROOT_DIRECTORY_ID: &str = "root_home";
const DESKTOP_DIRECTORY_ID: &str = "root_desktop";
const DIRECTORY_ID: &str = "directory_4a5b6c";
const PROJECT_ID: &str = "project_1123581321";
const SECOND_PROJECT_ID: &str = "project_22446688";
const FORGE_MESSAGE_ID: &str = "forge-msg-9f8e7d";
const THREAD_ID: &str = "thread_2718281828";
const DISPLAY_NAME: &str = "artisan-editor";
const HOME_DISPLAY_NAME: &str = "home";
const ROOT_PATH: &str = "\\\\?\\C:\\Users\\sander\\source\\artisan-editor";
const THREAD_TITLE: &str = "New thread";
const MESSAGE_BODY: &str = "Fix the flaky QUIC reconnect test.";
/// A second attachment instant so catalog rows are distinguishable.
const SECOND_ATTACHED_AT_MILLIS: i64 = 1_777_778_100_789;

const WELCOME_FRAME_ID: &str = "server-frame-000001";
const ROOTS_LISTING_FRAME_ID: &str = "server-frame-000002";
const CHILD_LISTING_FRAME_ID: &str = "server-frame-000003";
const ATTACH_RESPONSE_FRAME_ID: &str = "server-frame-000004";
const ATTACH_DUPLICATE_RESPONSE_FRAME_ID: &str = "server-frame-000004-duplicate";
const THREAD_LIST_RESPONSE_ID: &str = "server-frame-000005";
const THREAD_CREATE_RESPONSE_ID: &str = "server-frame-000006";
const THREAD_CREATE_DUPLICATE_RESPONSE_ID: &str = "server-frame-000006-duplicate";
const RECEIPT_ACCEPTED_RESPONSE_ID: &str = "server-frame-000007";
const RECEIPT_DUPLICATE_RESPONSE_ID: &str = "server-frame-000008";
const PROJECT_LIST_RESPONSE_ID: &str = "server-frame-000009";
const CORRELATED_ERROR_FRAME_ID: &str = "server-error-000001";
const VERSION_REJECTION_FRAME_ID: &str = "server-error-000002";
const PROJECT_ATTACHED_EVENT_ID: &str = "server-event-000001";
const THREAD_CREATED_EVENT_ID: &str = "server-event-000002";
const FIRST_MESSAGE_QUEUED_EVENT_ID: &str = "server-event-000003";

fn frame() -> Builder<HeapAllocator> {
    Builder::new(HeapAllocator::new())
}

/// Stamps the shared header fields every frame carries.
fn init_envelope<'a>(
    message: &'a mut Builder<HeapAllocator>,
    message_id: &'a str,
) -> envelope::Builder<'a> {
    let mut envelope = message.init_root::<envelope::Builder>();
    envelope.set_protocol_version(PROTOCOL_VERSION);
    envelope.set_message_id(message_id);
    envelope.set_sent_at_millis(SENT_AT_MILLIS);
    envelope
}

/// Serializes deterministically so wire comparisons stay stable.
fn encode(message: &Builder<HeapAllocator>) -> Vec<u8> {
    serialize::write_message_to_words(message)
}

/// Reads one encoded frame back from its canonical byte form.
fn decode(
    bytes: &[u8],
) -> capnp::Result<capnp::message::Reader<capnp::serialize::BufferSegments<&[u8]>>> {
    let mut encoded = bytes;
    let reader = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())?;
    Ok(reader)
}

/// Asserts the header fields survive a round trip unchanged.
fn assert_envelope_header(envelope: envelope::Reader<'_>, message_id: &str) -> capnp::Result<()> {
    assert_eq!(envelope.get_protocol_version(), PROTOCOL_VERSION);
    assert_eq!(envelope.get_message_id()?, message_id);
    assert_eq!(envelope.get_sent_at_millis(), SENT_AT_MILLIS);
    Ok(())
}

/// Asserts a Forge-originated frame owns an identity separate from the
/// client request identity it may correlate to.
fn assert_server_envelope(envelope: envelope::Reader<'_>, frame_id: &str) -> capnp::Result<()> {
    assert_envelope_header(envelope, frame_id)?;
    assert_ne!(envelope.get_message_id()?, CLIENT_REQUEST_ID);
    Ok(())
}

fn set_thread_summary(mut summary: thread_summary::Builder<'_>) {
    summary.set_thread_id(THREAD_ID);
    summary.set_project_id(PROJECT_ID);
    summary.set_title(THREAD_TITLE);
    summary.set_created_at_millis(CREATED_AT_MILLIS);
    summary.set_updated_at_millis(UPDATED_AT_MILLIS);
}

fn assert_project(project: project::Reader<'_>) -> capnp::Result<()> {
    assert_eq!(project.get_project_id()?, PROJECT_ID);
    assert_eq!(project.get_display_name()?, DISPLAY_NAME);
    assert_eq!(project.get_root_path()?, ROOT_PATH);
    assert_eq!(project.get_attached_at_millis(), ATTACHED_AT_MILLIS);
    Ok(())
}

fn assert_thread(thread: thread_summary::Reader<'_>) -> capnp::Result<()> {
    assert_eq!(thread.get_thread_id()?, THREAD_ID);
    assert_eq!(thread.get_project_id()?, PROJECT_ID);
    assert_eq!(thread.get_title()?, THREAD_TITLE);
    assert_eq!(thread.get_created_at_millis(), CREATED_AT_MILLIS);
    assert_eq!(thread.get_updated_at_millis(), UPDATED_AT_MILLIS);
    Ok(())
}

/// Stamps one complete catalog row; a listing row carries the full existing
/// `Project` shape, never a thinner projection.
fn set_project_row(mut row: project::Builder<'_>, project_id: &str, attached_at_millis: i64) {
    row.set_project_id(project_id);
    row.set_display_name(DISPLAY_NAME);
    row.set_root_path(ROOT_PATH);
    row.set_attached_at_millis(attached_at_millis);
}

/// Asserts one complete catalog row survived the round trip field for field.
fn assert_project_row(
    row: project::Reader<'_>,
    project_id: &str,
    attached_at_millis: i64,
) -> capnp::Result<()> {
    assert_eq!(row.get_project_id()?, project_id);
    assert_eq!(row.get_display_name()?, DISPLAY_NAME);
    assert_eq!(row.get_root_path()?, ROOT_PATH);
    assert_eq!(row.get_attached_at_millis(), attached_at_millis);
    Ok(())
}

#[test]
fn negotiates_hello_to_welcome_with_rotating_credentials() -> capnp::Result<()> {
    // Client offer: preferred version stamped on the envelope, full
    // supported list, and the single-use credential inside the hello body.
    for (reconnect, capability) in [
        (false, &HELLO_CAPABILITY[..]),
        (true, &RECONNECT_CAPABILITY[..]),
    ] {
        let client = {
            let mut message = frame();
            let mut hello = init_envelope(&mut message, CLIENT_REQUEST_ID)
                .init_body()
                .init_hello();
            let mut versions = hello.reborrow().init_supported_versions(1);
            versions.set(0, PROTOCOL_VERSION);
            let mut credential = hello.reborrow().init_credential();
            if reconnect {
                credential.set_reconnect(capability);
            } else {
                credential.set_initial(capability);
            }
            encode(&message)
        };

        let decoded = decode(&client)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        assert_envelope_header(envelope, CLIENT_REQUEST_ID)?;
        match envelope.get_body().which()? {
            envelope::body::Which::Hello(hello) => {
                let hello = hello?;
                let offered: Vec<u32> = hello.get_supported_versions()?.iter().collect();
                assert_eq!(offered, vec![PROTOCOL_VERSION]);
                match hello.get_credential().which()? {
                    hello::credential::Which::Initial(received) => {
                        assert!(!reconnect, "reconnect input decoded as initial credential");
                        assert_eq!(received?, capability);
                    }
                    hello::credential::Which::Reconnect(received) => {
                        assert!(reconnect, "initial input decoded as reconnect credential");
                        assert_eq!(received?, capability);
                    }
                }
            }
            _ => panic!("expected hello body"),
        }
    }

    // Server answer: exactly one negotiated version, a connection id, and
    // the next rotated single-use reconnect credential.
    let server = {
        let mut message = frame();
        let mut welcome = init_envelope(&mut message, WELCOME_FRAME_ID)
            .init_body()
            .init_welcome();
        welcome.set_negotiated_version(PROTOCOL_VERSION);
        welcome.set_connection_id(CONNECTION_ID);
        welcome.set_reconnect_capability(&ROTATED_RECONNECT_CAPABILITY);
        encode(&message)
    };

    let decoded = decode(&server)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(envelope, WELCOME_FRAME_ID)?;
    match envelope.get_body().which()? {
        envelope::body::Which::Welcome(welcome) => {
            let welcome = welcome?;
            assert_eq!(welcome.get_negotiated_version(), PROTOCOL_VERSION);
            assert_eq!(welcome.get_connection_id()?, CONNECTION_ID);
            assert_eq!(
                welcome.get_reconnect_capability()?,
                &ROTATED_RECONNECT_CAPABILITY[..]
            );
        }
        _ => panic!("expected welcome body"),
    }

    Ok(())
}

#[test]
fn capability_length_needs_owned_conversion_enforcement() -> capnp::Result<()> {
    // Negative coverage available at the schema layer: the Data wire type
    // cannot express an exact length, so a malformed 31-byte capability still
    // DECODES. This proves the exact-32-byte rule belongs to owned
    // conversion code, which returns typed invalidInput rejections there.
    let malformed = {
        let mut message = frame();
        init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_hello()
            .init_credential()
            .set_initial(&SHORT_CAPABILITY);
        encode(&message)
    };

    let decoded = decode(&malformed)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(envelope, CLIENT_REQUEST_ID)?;
    match envelope.get_body().which()? {
        envelope::body::Which::Hello(hello) => match hello?.get_credential().which()? {
            hello::credential::Which::Initial(received) => {
                let received = received?;
                assert_eq!(received.len(), SHORT_CAPABILITY.len());
                assert_ne!(received.len(), HELLO_CAPABILITY.len());
                assert_eq!(received, &SHORT_CAPABILITY[..]);
            }
            hello::credential::Which::Reconnect(_) => {
                panic!("expected initial credential union arm")
            }
        },
        _ => panic!("expected hello body"),
    }

    Ok(())
}

#[test]
fn reconnect_hello_capability_length_needs_owned_conversion_enforcement() -> capnp::Result<()> {
    let malformed = {
        let mut message = frame();
        init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_hello()
            .init_credential()
            .set_reconnect(&SHORT_RECONNECT_CAPABILITY);
        encode(&message)
    };

    let decoded = decode(&malformed)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(envelope, CLIENT_REQUEST_ID)?;
    match envelope.get_body().which()? {
        envelope::body::Which::Hello(hello) => match hello?.get_credential().which()? {
            hello::credential::Which::Initial(_) => {
                panic!("expected reconnect credential union arm")
            }
            hello::credential::Which::Reconnect(received) => {
                let received = received?;
                assert_eq!(received.len(), SHORT_RECONNECT_CAPABILITY.len());
                assert_ne!(received.len(), RECONNECT_CAPABILITY.len());
                assert_eq!(received, &SHORT_RECONNECT_CAPABILITY[..]);
            }
        },
        _ => panic!("expected hello body"),
    }

    Ok(())
}

#[test]
fn reconnect_capability_length_needs_owned_conversion_enforcement() -> capnp::Result<()> {
    // Negative coverage available at the schema layer: a malformed 31-byte
    // reconnect credential still DECODES, exactly like the initial
    // capability. The exact-32-byte rule for every rotated Welcome
    // credential belongs to owned conversion code, which returns typed
    // invalidInput rejections there; single-use consumption and session
    // binding belong to Phase 3.
    let malformed = {
        let mut message = frame();
        init_envelope(&mut message, WELCOME_FRAME_ID)
            .init_body()
            .init_welcome()
            .set_reconnect_capability(&SHORT_RECONNECT_CAPABILITY);
        encode(&message)
    };

    let decoded = decode(&malformed)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    assert_envelope_header(envelope, WELCOME_FRAME_ID)?;
    match envelope.get_body().which()? {
        envelope::body::Which::Welcome(welcome) => {
            let welcome = welcome?;
            let received = welcome.get_reconnect_capability()?;
            assert_eq!(received.len(), SHORT_RECONNECT_CAPABILITY.len());
            assert_ne!(received.len(), ROTATED_RECONNECT_CAPABILITY.len());
            assert_eq!(received, &SHORT_RECONNECT_CAPABILITY[..]);
        }
        _ => panic!("expected welcome body"),
    }

    Ok(())
}

#[test]
fn round_trips_both_directory_scope_request_arms() -> capnp::Result<()> {
    // Request: browse the Forge-visible roots (explicit noParent arm).
    let roots_request = {
        let mut message = frame();
        let mut scope = init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .init_list_directories()
            .init_scope();
        scope.set_no_parent(());
        encode(&message)
    };

    {
        let decoded = decode(&roots_request)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        match envelope.get_body().which()? {
            envelope::body::Which::Request(req) => match req?.which()? {
                request::Which::ListDirectories(listing) => match listing?.get_scope().which()? {
                    list_directories_request::scope::Which::NoParent(()) => {}
                    list_directories_request::scope::Which::Parent(_) => {
                        panic!("expected noParent scope")
                    }
                },
                _ => panic!("expected listDirectories request"),
            },
            _ => panic!("expected request body"),
        }
    }

    // Request: browse into one listed directory (explicit parent arm).
    let child_request = {
        let mut message = frame();
        init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .reborrow()
            .init_list_directories()
            .init_scope()
            .set_parent(DIRECTORY_ID);
        encode(&message)
    };

    let decoded = decode(&child_request)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    match envelope.get_body().which()? {
        envelope::body::Which::Request(req) => match req?.which()? {
            request::Which::ListDirectories(listing) => match listing?.get_scope().which()? {
                list_directories_request::scope::Which::NoParent(()) => {
                    panic!("expected parent scope")
                }
                list_directories_request::scope::Which::Parent(parent) => {
                    assert_eq!(parent?, DIRECTORY_ID);
                }
            },
            _ => panic!("expected listDirectories request"),
        },
        _ => panic!("expected request body"),
    }

    Ok(())
}

#[test]
fn round_trips_roots_directory_listing() -> capnp::Result<()> {
    // Response: root listing carrying optional parent (absent), bounded
    // places, and bounded entries -- all three represented losslessly.
    let roots_response = {
        let mut message = frame();
        let mut res = init_envelope(&mut message, ROOTS_LISTING_FRAME_ID)
            .init_body()
            .init_response();
        res.set_request_id(CLIENT_REQUEST_ID);
        let mut listing = res.init_directory_list();
        listing.reborrow().init_parent().set_no_parent(());

        let mut places = listing.reborrow().init_places(2);
        let mut home = places.reborrow().get(0);
        home.set_kind(PlaceKind::Home);
        home.set_directory_id(ROOT_DIRECTORY_ID);
        home.set_display_name(HOME_DISPLAY_NAME);

        let mut desktop = places.get(1);
        desktop.set_kind(PlaceKind::Desktop);
        desktop.set_directory_id(DESKTOP_DIRECTORY_ID);
        desktop.set_display_name(DISPLAY_NAME);

        let mut entries = listing.init_entries(2);

        let mut root = entries.reborrow().get(0);
        root.set_directory_id(ROOT_DIRECTORY_ID);
        root.set_display_name(DISPLAY_NAME);
        root.set_kind(DirectoryEntryKind::Root);
        root.set_has_children(true);

        let mut child = entries.get(1);
        child.set_directory_id(DIRECTORY_ID);
        child.set_display_name(DISPLAY_NAME);
        child.set_kind(DirectoryEntryKind::Directory);
        child.set_has_children(false);
        encode(&message)
    };

    let decoded = decode(&roots_response)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    match envelope.get_body().which()? {
        envelope::body::Which::Response(res) => {
            let res = res?;
            assert_eq!(res.get_request_id()?, CLIENT_REQUEST_ID);
            match res.which()? {
                response::Which::DirectoryList(listing) => {
                    let listing = listing?;
                    match listing.get_parent().which()? {
                        directory_listing::parent::Which::NoParent(()) => {}
                        directory_listing::parent::Which::Parent(_) => {
                            panic!("expected absent parent on roots listing")
                        }
                    }

                    let places = listing.get_places()?;
                    assert_eq!(places.len(), 2);
                    assert_eq!(places.get(0).get_kind()?, PlaceKind::Home);
                    assert_eq!(places.get(0).get_directory_id()?, ROOT_DIRECTORY_ID);
                    assert_eq!(places.get(1).get_kind()?, PlaceKind::Desktop,);
                    assert_eq!(places.get(1).get_directory_id()?, DESKTOP_DIRECTORY_ID,);

                    let entries = listing.get_entries()?;
                    assert_eq!(entries.len(), 2);
                    let root = entries.get(0);
                    assert_eq!(root.get_directory_id()?, ROOT_DIRECTORY_ID);
                    assert_eq!(root.get_display_name()?, DISPLAY_NAME);
                    assert_eq!(root.get_kind()?, DirectoryEntryKind::Root);
                    assert!(root.get_has_children());

                    let child = entries.get(1);
                    assert_eq!(child.get_directory_id()?, DIRECTORY_ID);
                    assert_eq!(child.get_display_name()?, DISPLAY_NAME);
                    assert_eq!(child.get_kind()?, DirectoryEntryKind::Directory);
                    assert!(!child.get_has_children());
                }
                _ => panic!("expected directoryList response"),
            }
        }
        _ => panic!("expected response body"),
    }

    Ok(())
}

#[test]
fn round_trips_child_directory_listing() -> capnp::Result<()> {
    // Response: child listing whose optional parent names the browsed
    // directory, with empty places and entries still explicitly present.
    let child_response = {
        let mut message = frame();
        let mut res = init_envelope(&mut message, CHILD_LISTING_FRAME_ID)
            .init_body()
            .init_response();
        res.set_request_id(CLIENT_REQUEST_ID);
        let mut listing = res.init_directory_list();
        listing.reborrow().init_parent().set_parent(DIRECTORY_ID);
        listing.reborrow().init_places(0);
        listing.reborrow().init_entries(0);
        encode(&message)
    };

    let decoded = decode(&child_response)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    match envelope.get_body().which()? {
        envelope::body::Which::Response(res) => match res?.which()? {
            response::Which::DirectoryList(listing) => {
                let listing = listing?;
                match listing.get_parent().which()? {
                    directory_listing::parent::Which::NoParent(()) => {
                        panic!("expected parent on child listing")
                    }
                    directory_listing::parent::Which::Parent(parent) => {
                        assert_eq!(parent?, DIRECTORY_ID);
                    }
                }
                assert_eq!(listing.get_places()?.len(), 0);
                assert_eq!(listing.get_entries()?.len(), 0);
            }
            _ => panic!("expected directoryList response"),
        },
        _ => panic!("expected response body"),
    }

    Ok(())
}

#[test]
fn round_trips_project_attachment_request() -> capnp::Result<()> {
    // Request: attach by opaque directory id only; Forge mints the project.
    let attach_request = {
        let mut message = frame();
        init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .reborrow()
            .init_attach_project()
            .set_directory_id(DIRECTORY_ID);
        encode(&message)
    };

    {
        let decoded = decode(&attach_request)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        match envelope.get_body().which()? {
            envelope::body::Which::Request(req) => match req?.which()? {
                request::Which::AttachProject(attach) => {
                    assert_eq!(attach?.get_directory_id()?, DIRECTORY_ID);
                }
                _ => panic!("expected attachProject request"),
            },
            _ => panic!("expected request body"),
        }
    }

    Ok(())
}

fn assert_attach_result(frame_id: &str, disposition: ReceiptDisposition) -> capnp::Result<()> {
    let encoded = {
        let mut message = frame();
        let mut response = init_envelope(&mut message, frame_id)
            .init_body()
            .init_response();
        response.set_request_id(CLIENT_REQUEST_ID);
        let mut result = response.init_attached_project();
        result.set_disposition(disposition);
        let mut project = result.init_project();
        project.set_project_id(PROJECT_ID);
        project.set_display_name(DISPLAY_NAME);
        project.set_root_path(ROOT_PATH);
        project.set_attached_at_millis(ATTACHED_AT_MILLIS);
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    assert_server_envelope(envelope, frame_id)?;
    match envelope.get_body().which()? {
        envelope::body::Which::Response(response) => {
            let response = response?;
            assert_eq!(response.get_request_id()?, CLIENT_REQUEST_ID);
            match response.which()? {
                response::Which::AttachedProject(result) => {
                    let result = result?;
                    assert_eq!(result.get_disposition()?, disposition);
                    assert_project(result.get_project()?)?;
                }
                _ => panic!("expected attachedProject response"),
            }
        }
        _ => panic!("expected response body"),
    }

    Ok(())
}

#[test]
fn round_trips_project_attachment_dispositions() -> capnp::Result<()> {
    assert_attach_result(ATTACH_RESPONSE_FRAME_ID, ReceiptDisposition::Accepted)?;
    assert_attach_result(
        ATTACH_DUPLICATE_RESPONSE_FRAME_ID,
        ReceiptDisposition::Duplicate,
    )?;
    Ok(())
}

#[test]
fn round_trips_attached_project_listing_request() -> capnp::Result<()> {
    let encoded = {
        let mut message = frame();
        init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .init_list_attached_projects();
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    match envelope.get_body().which()? {
        envelope::body::Which::Request(request) => match request?.which()? {
            request::Which::ListAttachedProjects(request) => {
                let _: list_attached_projects_request::Reader<'_> = request?;
            }
            _ => panic!("expected listAttachedProjects request"),
        },
        _ => panic!("expected request body"),
    }

    Ok(())
}

fn assert_project_listing_round_trip(project_count: usize) -> capnp::Result<()> {
    let encoded = {
        let mut message = frame();
        let mut response = init_envelope(&mut message, PROJECT_LIST_RESPONSE_ID)
            .init_body()
            .init_response();
        response.set_request_id(CLIENT_REQUEST_ID);
        let mut projects = response
            .init_project_list()
            .init_projects(u32::try_from(project_count).expect("fixture count fits u32"));

        for index in 0..project_count {
            let project_id = if index == 1 {
                SECOND_PROJECT_ID
            } else {
                PROJECT_ID
            };
            let attached_at = if index == 1 {
                SECOND_ATTACHED_AT_MILLIS
            } else {
                ATTACHED_AT_MILLIS
            };
            set_project_row(
                projects
                    .reborrow()
                    .get(u32::try_from(index).expect("fixture index fits u32")),
                project_id,
                attached_at,
            );
        }
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    assert_server_envelope(envelope, PROJECT_LIST_RESPONSE_ID)?;
    match envelope.get_body().which()? {
        envelope::body::Which::Response(response) => {
            let response = response?;
            assert_eq!(response.get_request_id()?, CLIENT_REQUEST_ID);
            match response.which()? {
                response::Which::ProjectList(listing) => {
                    let listing: project_list::Reader<'_> = listing?;
                    let projects = listing.get_projects()?;
                    assert_eq!(projects.len() as usize, project_count);
                    if project_count > 0 {
                        assert_project_row(projects.get(0), PROJECT_ID, ATTACHED_AT_MILLIS)?;
                    }
                    if project_count > 1 {
                        assert_project_row(
                            projects.get(1),
                            SECOND_PROJECT_ID,
                            SECOND_ATTACHED_AT_MILLIS,
                        )?;
                    }
                }
                _ => panic!("expected projectList response"),
            }
        }
        _ => panic!("expected response body"),
    }

    Ok(())
}

#[test]
fn round_trips_empty_representative_and_maximum_project_listings() -> capnp::Result<()> {
    assert_project_listing_round_trip(0)?;
    assert_project_listing_round_trip(2)?;
    assert_project_listing_round_trip(PROJECT_LISTING_MAX_PROJECTS)?;
    Ok(())
}

#[test]
fn round_trips_thread_listing() -> capnp::Result<()> {
    // Request: list threads of an attached project.
    let list_request = {
        let mut message = frame();
        init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .reborrow()
            .init_list_project_threads()
            .set_project_id(PROJECT_ID);
        encode(&message)
    };

    {
        let decoded = decode(&list_request)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        match envelope.get_body().which()? {
            envelope::body::Which::Request(req) => match req?.which()? {
                request::Which::ListProjectThreads(listing) => {
                    assert_eq!(listing?.get_project_id()?, PROJECT_ID);
                }
                _ => panic!("expected listProjectThreads request"),
            },
            _ => panic!("expected request body"),
        }
    }

    // Response: bounded thread list.
    let list_response = {
        let mut message = frame();
        let mut res = init_envelope(&mut message, THREAD_LIST_RESPONSE_ID)
            .init_body()
            .init_response();
        res.set_request_id(CLIENT_REQUEST_ID);
        let mut threads = res.init_thread_list().init_threads(1);
        set_thread_summary(threads.reborrow().get(0));
        encode(&message)
    };

    {
        let decoded = decode(&list_response)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        match envelope.get_body().which()? {
            envelope::body::Which::Response(res) => {
                let res = res?;
                assert_eq!(res.get_request_id()?, CLIENT_REQUEST_ID);
                match res.which()? {
                    response::Which::ThreadList(list) => {
                        let threads = list?.get_threads()?;
                        assert_eq!(threads.len(), 1);
                        assert_thread(threads.get(0))?;
                    }
                    _ => panic!("expected threadList response"),
                }
            }
            _ => panic!("expected response body"),
        }
    }

    Ok(())
}

#[test]
fn round_trips_create_thread_request() -> capnp::Result<()> {
    // Request: create a thread; Forge mints the never-before-seen thread id.
    let create_request = {
        let mut message = frame();
        let mut create = init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .init_create_project_thread();
        create.set_project_id(PROJECT_ID);
        create.set_title(THREAD_TITLE);
        encode(&message)
    };

    {
        let decoded = decode(&create_request)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        match envelope.get_body().which()? {
            envelope::body::Which::Request(req) => match req?.which()? {
                request::Which::CreateProjectThread(create) => {
                    let create = create?;
                    assert_eq!(create.get_project_id()?, PROJECT_ID);
                    assert_eq!(create.get_title()?, THREAD_TITLE);
                }
                _ => panic!("expected createProjectThread request"),
            },
            _ => panic!("expected request body"),
        }
    }

    Ok(())
}

fn assert_create_thread_result(
    frame_id: &str,
    disposition: ReceiptDisposition,
) -> capnp::Result<()> {
    let encoded = {
        let mut message = frame();
        let mut response = init_envelope(&mut message, frame_id)
            .init_body()
            .init_response();
        response.set_request_id(CLIENT_REQUEST_ID);
        let mut result = response.init_created_thread();
        result.set_disposition(disposition);
        set_thread_summary(result.init_thread());
        encode(&message)
    };

    let decoded = decode(&encoded)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    assert_server_envelope(envelope, frame_id)?;
    match envelope.get_body().which()? {
        envelope::body::Which::Response(response) => {
            let response = response?;
            assert_eq!(response.get_request_id()?, CLIENT_REQUEST_ID);
            match response.which()? {
                response::Which::CreatedThread(result) => {
                    let result = result?;
                    assert_eq!(result.get_disposition()?, disposition);
                    assert_thread(result.get_thread()?)?;
                }
                _ => panic!("expected createdThread response"),
            }
        }
        _ => panic!("expected response body"),
    }

    Ok(())
}

#[test]
fn round_trips_create_thread_dispositions() -> capnp::Result<()> {
    assert_create_thread_result(THREAD_CREATE_RESPONSE_ID, ReceiptDisposition::Accepted)?;
    assert_create_thread_result(
        THREAD_CREATE_DUPLICATE_RESPONSE_ID,
        ReceiptDisposition::Duplicate,
    )?;
    Ok(())
}

#[test]
fn round_trips_first_message_receipt_dispositions() -> capnp::Result<()> {
    // Request: queue the first message; the envelope message id doubles as
    // the stable retry key across identical retransmissions.
    let queue_request = {
        let mut message = frame();
        let mut queue = init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .init_queue_first_message();
        queue.set_thread_id(THREAD_ID);
        queue.set_body(MESSAGE_BODY);
        encode(&message)
    };

    {
        let decoded = decode(&queue_request)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        match envelope.get_body().which()? {
            envelope::body::Which::Request(req) => match req?.which()? {
                request::Which::QueueFirstMessage(queue) => {
                    let queue = queue?;
                    assert_eq!(queue.get_thread_id()?, THREAD_ID);
                    assert_eq!(queue.get_body()?, MESSAGE_BODY);
                }
                _ => panic!("expected queueFirstMessage request"),
            },
            _ => panic!("expected request body"),
        }
    }

    // The duplicate retry reuses the client's message id verbatim; Forge
    // answers with the same durable message identity and disposition either
    // way, so both receipt shapes round trip through the same assertion.
    for (response_id, disposition) in [
        (RECEIPT_ACCEPTED_RESPONSE_ID, ReceiptDisposition::Accepted),
        (RECEIPT_DUPLICATE_RESPONSE_ID, ReceiptDisposition::Duplicate),
    ] {
        let receipt_frame = {
            let mut message = frame();
            let mut res = init_envelope(&mut message, response_id)
                .init_body()
                .init_response();
            res.set_request_id(CLIENT_REQUEST_ID);
            let mut receipt = res.init_queued_receipt();
            receipt.set_request_id(CLIENT_REQUEST_ID);
            receipt.set_message_id(FORGE_MESSAGE_ID);
            receipt.set_thread_id(THREAD_ID);
            receipt.set_disposition(disposition);
            receipt.set_state(QueuedState::Queued);
            encode(&message)
        };

        let decoded = decode(&receipt_frame)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        match envelope.get_body().which()? {
            envelope::body::Which::Response(res) => {
                let res = res?;
                assert_eq!(res.get_request_id()?, CLIENT_REQUEST_ID);
                match res.which()? {
                    response::Which::QueuedReceipt(receipt) => {
                        let receipt = receipt?;
                        assert_eq!(receipt.get_request_id()?, CLIENT_REQUEST_ID);
                        assert_eq!(receipt.get_message_id()?, FORGE_MESSAGE_ID);
                        assert_eq!(receipt.get_thread_id()?, THREAD_ID);
                        assert_eq!(receipt.get_disposition()?, disposition);
                        assert_eq!(receipt.get_state()?, QueuedState::Queued);
                    }
                    _ => panic!("expected queuedReceipt response"),
                }
            }
            _ => panic!("expected response body"),
        }
    }

    Ok(())
}

#[test]
fn round_trips_all_events() -> capnp::Result<()> {
    // Event: project attached.
    let attached_event = {
        let mut message = frame();
        let mut attached = init_envelope(&mut message, PROJECT_ATTACHED_EVENT_ID)
            .init_body()
            .init_event()
            .init_project_attached();
        attached.set_project_id(PROJECT_ID);
        attached.set_display_name(DISPLAY_NAME);
        attached.set_root_path(ROOT_PATH);
        attached.set_attached_at_millis(ATTACHED_AT_MILLIS);
        encode(&message)
    };

    {
        let decoded = decode(&attached_event)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        assert_server_envelope(envelope, PROJECT_ATTACHED_EVENT_ID)?;
        match envelope.get_body().which()? {
            envelope::body::Which::Event(event) => match event?.which()? {
                event::Which::ProjectAttached(project) => assert_project(project?)?,
                _ => panic!("expected projectAttached event"),
            },
            _ => panic!("expected event body"),
        }
    }

    // Event: thread created.
    let created_event = {
        let mut message = frame();
        set_thread_summary(
            init_envelope(&mut message, THREAD_CREATED_EVENT_ID)
                .init_body()
                .init_event()
                .init_thread_created(),
        );
        encode(&message)
    };

    {
        let decoded = decode(&created_event)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        assert_server_envelope(envelope, THREAD_CREATED_EVENT_ID)?;
        match envelope.get_body().which()? {
            envelope::body::Which::Event(event) => match event?.which()? {
                event::Which::ThreadCreated(thread) => assert_thread(thread?)?,
                _ => panic!("expected threadCreated event"),
            },
            _ => panic!("expected event body"),
        }
    }

    // Event: first message queued, carrying the stable request correlation,
    // Forge's own durable message identity, and the bounded body losslessly.
    let queued_event = {
        let mut message = frame();
        let mut queued = init_envelope(&mut message, FIRST_MESSAGE_QUEUED_EVENT_ID)
            .init_body()
            .init_event()
            .init_first_message_queued();
        queued.set_request_id(CLIENT_REQUEST_ID);
        queued.set_message_id(FORGE_MESSAGE_ID);
        queued.set_thread_id(THREAD_ID);
        queued.set_body(MESSAGE_BODY);
        encode(&message)
    };

    let decoded = decode(&queued_event)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    assert_server_envelope(envelope, FIRST_MESSAGE_QUEUED_EVENT_ID)?;
    match envelope.get_body().which()? {
        envelope::body::Which::Event(event) => match event?.which()? {
            event::Which::FirstMessageQueued(queued) => {
                let queued = queued?;
                assert_eq!(queued.get_request_id()?, CLIENT_REQUEST_ID);
                assert_eq!(queued.get_message_id()?, FORGE_MESSAGE_ID);
                assert_eq!(queued.get_thread_id()?, THREAD_ID);
                assert_eq!(queued.get_body()?, MESSAGE_BODY);
            }
            _ => panic!("expected firstMessageQueued event"),
        },
        _ => panic!("expected event body"),
    }

    Ok(())
}

#[test]
fn correlates_errors_to_the_triggering_request_id() -> capnp::Result<()> {
    // Correlated error: typed code plus the triggering request id.
    let correlated_error = {
        let mut message = frame();
        let mut error = init_envelope(&mut message, CORRELATED_ERROR_FRAME_ID)
            .init_body()
            .init_protocol_error();
        error.set_code(ErrorCode::DirectoryUnknown);
        error.set_message("directory id expired");
        error.set_retryable(false);
        error.set_correlated(CLIENT_REQUEST_ID);
        encode(&message)
    };

    {
        let decoded = decode(&correlated_error)?;
        let envelope: envelope::Reader = decoded.get_root()?;
        match envelope.get_body().which()? {
            envelope::body::Which::ProtocolError(error) => {
                let error = error?;
                assert_eq!(error.get_code()?, ErrorCode::DirectoryUnknown);
                assert_eq!(error.get_message()?, "directory id expired");
                assert!(!error.get_retryable());
                match error.which()? {
                    protocol_error::Which::Correlated(request_id) => {
                        assert_eq!(request_id?, CLIENT_REQUEST_ID);
                    }
                    protocol_error::Which::Uncorrelated(()) => {
                        panic!("expected correlated error")
                    }
                }
            }
            _ => panic!("expected protocolError body"),
        }
    }

    // Uncorrelated error: hello-time version rejection carries no request id
    // and an empty message, because the typed code alone renders.
    let version_rejection = {
        let mut message = frame();
        let mut error = init_envelope(&mut message, VERSION_REJECTION_FRAME_ID)
            .init_body()
            .init_protocol_error();
        error.set_code(ErrorCode::UnsupportedVersion);
        error.set_retryable(false);
        error.set_uncorrelated(());
        encode(&message)
    };

    let decoded = decode(&version_rejection)?;
    let envelope: envelope::Reader = decoded.get_root()?;
    match envelope.get_body().which()? {
        envelope::body::Which::ProtocolError(error) => {
            let error = error?;
            assert_eq!(error.get_code()?, ErrorCode::UnsupportedVersion);
            assert_eq!(error.get_message()?, "");
            match error.which()? {
                protocol_error::Which::Correlated(_) => panic!("expected uncorrelated error"),
                protocol_error::Which::Uncorrelated(()) => {}
            }
        }
        _ => panic!("expected protocolError body"),
    }

    Ok(())
}

#[test]
fn serializes_identical_frames_to_identical_bytes() {
    let build_queue_frame = || {
        let mut message = frame();
        let mut queue = init_envelope(&mut message, CLIENT_REQUEST_ID)
            .init_body()
            .init_request()
            .init_queue_first_message();
        queue.set_thread_id(THREAD_ID);
        queue.set_body(MESSAGE_BODY);
        encode(&message)
    };

    let first = build_queue_frame();
    assert!(!first.is_empty());
    assert_eq!(first, build_queue_frame());
}
