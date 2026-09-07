//! Server-side single-request dispatch over real loopback QUIC.

#[allow(
    dead_code,
    reason = "this target reuses loopback setup but not the proof-only echo helpers"
)]
mod harness;

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use artisan_domain::{
    AttachProject, Command, DirectoryId, DirectoryListing, ListDirectories, Query, RequestId,
    UnixMillis,
};
use artisan_protocol::{
    ClientRequest, ConnectionId, ErrorCode, ErrorDetail, FrameId, Hello, HelloCredential,
    LocalCapability, ProtocolFailure, ProtocolVersion, ReconnectCapability, ResponsePayload,
    ServerResponse, VersionOffer, Welcome, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport as transport;
use artisan_transport::{HandshakeMessageKind, ReplyValidationError, ServerDispatchError};
use harness::{Loopback, TEST_DEADLINE, connect_client, server_connection, spawn_loopback};
use quinn::{Connection, RecvStream, VarInt};

const INITIAL_CAPABILITY: [u8; 32] = [0x51; 32];
const ROTATED_CAPABILITY: [u8; 32] = [0x8d; 32];

/// Frame identity of the dispatched request and the correlation id every
/// accepted reply must carry.
const REQUEST_ID: &str = "client-request-1";
/// Correlation id carried by a reply that settles a different request.
const OTHER_REQUEST_ID: &str = "other-request-9";

/// A distinct local handler failure type, proving local failures stay typed
/// instead of becoming protocol failures.
#[derive(Debug, Eq, PartialEq)]
struct HandlerRejected;

impl std::fmt::Display for HandlerRejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("handler rejected locally")
    }
}

impl std::error::Error for HandlerRejected {}

fn list_directories_request() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(REQUEST_ID)?,
        sent_at: UnixMillis::from_millis(3_000),
        body: WireEnvelopeBody::Request(ClientRequest::Query(Query::ListDirectories(
            ListDirectories { parent: None },
        ))),
    })
}

fn attach_project_request() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(REQUEST_ID)?,
        sent_at: UnixMillis::from_millis(3_010),
        body: WireEnvelopeBody::Request(ClientRequest::Command(Command::AttachProject(
            AttachProject {
                request_id: RequestId::parse(REQUEST_ID)?,
                directory_id: DirectoryId::parse("directory-1")?,
            },
        ))),
    })
}

fn empty_directory_response(request_id: &str) -> Result<ServerResponse, Box<dyn Error>> {
    Ok(ServerResponse {
        request_id: RequestId::parse(request_id)?,
        payload: ResponsePayload::DirectoryListing(DirectoryListing::new(vec![], vec![], None)?),
    })
}

fn correlated_response() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("server-response-1")?,
        sent_at: UnixMillis::from_millis(3_001),
        body: WireEnvelopeBody::Response(empty_directory_response(REQUEST_ID)?),
    })
}

fn correlated_failure() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("server-failure-1")?,
        sent_at: UnixMillis::from_millis(3_002),
        body: WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::Internal,
            detail: ErrorDetail::parse("forge unavailable")?,
            retryable: true,
            request_id: Some(RequestId::parse(REQUEST_ID)?),
        }),
    })
}

fn mismatched_response() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("server-response-mismatched")?,
        sent_at: UnixMillis::from_millis(3_003),
        body: WireEnvelopeBody::Response(empty_directory_response(OTHER_REQUEST_ID)?),
    })
}

fn uncorrelated_failure() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("server-failure-uncorrelated")?,
        sent_at: UnixMillis::from_millis(3_004),
        body: WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::InvalidInput,
            detail: ErrorDetail::parse("uncorrelated rejection")?,
            retryable: false,
            request_id: None,
        }),
    })
}

fn early_hello() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("client-hello-early")?,
        sent_at: UnixMillis::from_millis(3_005),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])?,
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
            supports_lifecycle_control: false,
        }),
    })
}

fn early_welcome() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("server-welcome-early")?,
        sent_at: UnixMillis::from_millis(3_006),
        body: WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: ProtocolVersion::V1,
            connection_id: ConnectionId::parse("connection-dispatch")?,
            reconnect_capability: ReconnectCapability::from_bytes(ROTATED_CAPABILITY),
            lifecycle_control_supported: false,
        }),
    })
}

async fn drain(loopback: Loopback) {
    loopback
        .drain(VarInt::from_u32(0), b"server dispatch test complete")
        .await;
}

/// Asserts the stream reaches clean end-of-stream having carried no reply
/// bytes at all: dropping an unwritten server send side finishes it cleanly,
/// so the four-byte length-prefix read truncates at zero received bytes
/// instead of yielding any envelope.
async fn expect_no_reply_bytes(recv: &mut RecvStream) {
    let outcome = tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(recv))
        .await
        .expect("stream end arrives within deadline");
    match outcome {
        Err(transport::EnvelopeReceiveError::Frame(transport::FrameError::Truncated {
            expected,
            received,
        })) => {
            assert_eq!(expected, size_of::<u32>());
            assert_eq!(received, 0);
        }
        Err(other) => panic!("expected an empty finished stream, got {other:?}"),
        Ok(envelope) => panic!(
            "expected no reply bytes, got an envelope with frame id {}",
            envelope.frame_id
        ),
    }
}

/// Sends `inbound`, runs dispatch against a handler that answers with
/// `reply`, and proves the failed exchange emits no reply bytes before
/// returning the typed dispatch failure.
async fn exchange_with_reply(
    client_connection: &Connection,
    server_connection: &Connection,
    inbound: &WireEnvelope,
    reply: WireEnvelope,
    calls: Arc<AtomicUsize>,
) -> Result<ServerDispatchError<HandlerRejected>, Box<dyn Error>> {
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;
    // The client writes first: Quinn delivers a bidirectional stream to the
    // accepting peer only once its first bytes arrive.
    transport::send_envelope(&mut client_send, inbound).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::dispatch_server_request(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok::<WireEnvelope, HandlerRejected>(reply)
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");

    drop(server_send);
    expect_no_reply_bytes(&mut client_recv).await;
    drop(server_recv);
    drop(client_send);
    drop(client_recv);
    Ok(result.unwrap_err())
}

#[tokio::test]
async fn one_request_reaches_the_handler_once_and_a_correlated_response_crosses_then_ends_the_stream()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;

    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let reply = correlated_response()?;
    // The client writes first: Quinn delivers a bidirectional stream to the
    // accepting peer only once its first bytes arrive.
    transport::send_envelope(&mut client_send, &list_directories_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    tokio::time::timeout(
        TEST_DEADLINE,
        transport::dispatch_server_request(
            &mut server_send,
            &mut server_recv,
            move |request| async move {
                observed.fetch_add(1, Ordering::SeqCst);
                assert_eq!(request.protocol_version, ProtocolVersion::V1);
                assert_eq!(request.frame_id.as_str(), REQUEST_ID);
                assert_eq!(request.sent_at, UnixMillis::from_millis(3_000));
                assert_eq!(request.request_id.as_str(), REQUEST_ID);
                assert_eq!(
                    request.request,
                    ClientRequest::Query(Query::ListDirectories(ListDirectories { parent: None }))
                );
                Ok::<WireEnvelope, HandlerRejected>(reply)
            },
        ),
    )
    .await??;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Exactly one correlated response crossed, then the finished server
    // send side ended the stream behind it.
    let received =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut client_recv))
            .await??;
    assert!(
        received == correlated_response()?,
        "the reply crossing the stream must equal the deterministic fixture"
    );
    let end_of_stream =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut client_recv)).await?;
    match end_of_stream {
        Err(transport::EnvelopeReceiveError::Frame(transport::FrameError::Truncated {
            expected,
            received,
        })) => {
            assert_eq!(expected, size_of::<u32>());
            assert_eq!(received, 0);
        }
        Err(other) => panic!("expected clean end-of-stream behind one reply, got {other:?}"),
        Ok(envelope) => panic!(
            "expected exactly one reply, got a second envelope with frame id {}",
            envelope.frame_id
        ),
    }

    client_send.finish()?;
    drop(client_send);
    drop(client_recv);
    drop(server_send);
    drop(server_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn a_correlated_protocol_error_reply_crosses_the_same_stream() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;

    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let reply = correlated_failure()?;
    // The client writes first so the accepting peer observes the stream.
    transport::send_envelope(&mut client_send, &attach_project_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    tokio::time::timeout(
        TEST_DEADLINE,
        transport::dispatch_server_request(
            &mut server_send,
            &mut server_recv,
            move |_request| async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok::<WireEnvelope, HandlerRejected>(reply)
            },
        ),
    )
    .await??;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let received =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut client_recv))
            .await??;
    assert!(
        received == correlated_failure()?,
        "the failure crossing the stream must equal the deterministic fixture"
    );
    let end_of_stream =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut client_recv)).await?;
    match end_of_stream {
        Err(transport::EnvelopeReceiveError::Frame(transport::FrameError::Truncated {
            expected,
            received,
        })) => {
            assert_eq!(expected, size_of::<u32>());
            assert_eq!(received, 0);
        }
        Err(other) => panic!("expected clean end-of-stream behind one reply, got {other:?}"),
        Ok(envelope) => panic!(
            "expected exactly one reply, got a second envelope with frame id {}",
            envelope.frame_id
        ),
    }

    client_send.finish()?;
    drop(client_send);
    drop(client_recv);
    drop(server_send);
    drop(server_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn non_request_envelopes_are_rejected_before_the_handler_runs() -> Result<(), Box<dyn Error>>
{
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    let calls = Arc::new(AtomicUsize::new(0));
    for (inbound, expected_kind) in [
        (early_hello()?, HandshakeMessageKind::Hello),
        (early_welcome()?, HandshakeMessageKind::Welcome),
        (mismatched_response()?, HandshakeMessageKind::Response),
        (correlated_failure()?, HandshakeMessageKind::ProtocolError),
    ] {
        let observed = Arc::clone(&calls);
        let (mut client_send, mut client_recv) = client_connection.open_bi().await?;
        // The client writes first so the accepting peer observes the stream.
        transport::send_envelope(&mut client_send, &inbound).await?;
        let (mut server_send, mut server_recv) =
            tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;

        let result = tokio::time::timeout(
            TEST_DEADLINE,
            transport::dispatch_server_request(
                &mut server_send,
                &mut server_recv,
                move |_| async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Err(HandlerRejected)
                },
            ),
        )
        .await
        .expect("dispatch settles within deadline");

        match result {
            Err(error @ ServerDispatchError::UnexpectedMessage { received }) => {
                assert_eq!(received, expected_kind);
                assert_eq!(
                    error.to_string(),
                    format!("expected Request on the server stream, received {expected_kind}")
                );
            }
            other => panic!("expected UnexpectedMessage for {expected_kind}, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        drop(server_send);
        expect_no_reply_bytes(&mut client_recv).await;
        drop(server_recv);
        drop(client_send);
        drop(client_recv);
    }

    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn malformed_prefixes_fail_receive_typed_and_skip_the_handler() -> Result<(), Box<dyn Error>>
{
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let calls = Arc::new(AtomicUsize::new(0));

    // A zero-length prefix is rejected as an empty transport frame.
    let observed = Arc::clone(&calls);
    let (mut client_send, client_recv) = client_connection.open_bi().await?;
    // The client writes first so the accepting peer observes the stream.
    tokio::time::timeout(TEST_DEADLINE, client_send.write_all(&0u32.to_le_bytes())).await??;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::dispatch_server_request(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Err(HandlerRejected)
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");
    assert!(
        matches!(
            result,
            Err(ServerDispatchError::Receive(
                transport::EnvelopeReceiveError::Frame(transport::FrameError::Empty)
            ))
        ),
        "expected Receive(Frame(Empty)), got {result:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(server_send);
    drop(server_recv);
    drop(client_send);
    drop(client_recv);

    // An oversized prefix is rejected against the transport byte ceiling
    // before any body allocation.
    let observed = Arc::clone(&calls);
    let announced_length = transport::MAX_FRAME_LEN + 1;
    let (mut client_send, client_recv) = client_connection.open_bi().await?;
    // The client writes first so the accepting peer observes the stream.
    tokio::time::timeout(
        TEST_DEADLINE,
        client_send.write_all(&announced_length.to_le_bytes()),
    )
    .await??;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::dispatch_server_request(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Err(HandlerRejected)
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");
    match result {
        Err(ServerDispatchError::Receive(transport::EnvelopeReceiveError::Frame(
            transport::FrameError::TooLarge { length, bound },
        ))) => {
            assert_eq!(length, announced_length);
            assert_eq!(bound, transport::MAX_FRAME_LEN);
        }
        other => panic!("expected Receive(Frame(TooLarge)), got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    drop(client_send);
    drop(client_recv);
    drop(server_send);
    drop(server_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn a_truncated_receive_keeps_the_truncation_cause_and_skips_the_handler()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);

    let (mut client_send, client_recv) = client_connection.open_bi().await?;
    // The client writes first so the accepting peer observes the stream.
    tokio::time::timeout(TEST_DEADLINE, client_send.write_all(&64u32.to_le_bytes())).await??;
    tokio::time::timeout(TEST_DEADLINE, client_send.write_all(&[7u8; 10])).await??;
    client_send.finish()?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::dispatch_server_request(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Err(HandlerRejected)
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");
    match result {
        Err(ServerDispatchError::Receive(transport::EnvelopeReceiveError::Frame(
            transport::FrameError::Truncated { expected, received },
        ))) => {
            assert_eq!(expected, 64);
            assert_eq!(received, 10);
        }
        other => panic!("expected Receive(Frame(Truncated)), got {other:?}"),
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    drop(client_send);
    drop(client_recv);
    drop(server_send);
    drop(server_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn mismatched_or_uncorrelated_replies_are_rejected_before_any_reply_byte()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let request = list_directories_request()?;

    // A response settling a different request never reaches the stream.
    let calls = Arc::new(AtomicUsize::new(0));
    let error = exchange_with_reply(
        &client_connection,
        &server_connection,
        &request,
        mismatched_response()?,
        Arc::clone(&calls),
    )
    .await?;
    assert_eq!(
        error.to_string(),
        "validating the handler reply failed: handler reply settles a different request"
    );
    assert!(
        matches!(
            error,
            ServerDispatchError::Reply(ReplyValidationError::DifferentRequest)
        ),
        "expected Reply(DifferentRequest)"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // A protocol failure carrying no request id correlates to nothing.
    let calls = Arc::new(AtomicUsize::new(0));
    let error = exchange_with_reply(
        &client_connection,
        &server_connection,
        &request,
        uncorrelated_failure()?,
        Arc::clone(&calls),
    )
    .await?;
    assert!(
        matches!(
            error,
            ServerDispatchError::Reply(ReplyValidationError::Uncorrelated)
        ),
        "expected Reply(Uncorrelated)"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // A reply outside the two reply families is refused outright.
    let calls = Arc::new(AtomicUsize::new(0));
    let error = exchange_with_reply(
        &client_connection,
        &server_connection,
        &request,
        list_directories_request()?,
        Arc::clone(&calls),
    )
    .await?;
    assert!(
        matches!(
            error,
            ServerDispatchError::Reply(ReplyValidationError::WrongFamily {
                received: HandshakeMessageKind::Request
            })
        ),
        "expected Reply(WrongFamily {{ Request }})"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn a_local_handler_failure_is_returned_typed_without_reply_bytes()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;
    // The client writes first so the accepting peer observes the stream.
    transport::send_envelope(&mut client_send, &list_directories_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;

    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::dispatch_server_request(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Err(HandlerRejected)
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");

    let Err(error) = &result else {
        panic!("a local handler failure must fail dispatch");
    };
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    match error {
        ServerDispatchError::Local(failure) => {
            assert_eq!(*failure, HandlerRejected);
            // The local failure stays caller-owned: the dispatcher's own
            // Display carries none of the handler's detail.
            assert_eq!(error.to_string(), "the request handler failed");
        }
        other => panic!("expected Local failure, got {other:?}"),
    }

    drop(server_send);
    expect_no_reply_bytes(&mut client_recv).await;
    drop(server_recv);
    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

/// Non-Clone receipt whose drop is observable without sleeps.
#[derive(Debug)]
struct Receipt {
    value: u32,
    drops: Arc<AtomicUsize>,
}

impl Receipt {
    fn new(value: u32, drops: Arc<AtomicUsize>) -> Self {
        Self { value, drops }
    }
}

impl Drop for Receipt {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn receipt_is_returned_only_after_successful_response_write_and_finish()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;

    let drops = Arc::new(AtomicUsize::new(0));
    let receipt_drops = Arc::clone(&drops);
    let reply = correlated_response()?;

    transport::send_envelope(&mut client_send, &list_directories_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;

    let receipt =
        tokio::time::timeout(
            TEST_DEADLINE,
            transport::server_dispatch::dispatch_server_request_with_receipt(
                &mut server_send,
                &mut server_recv,
                move |_| async move {
                    Ok::<_, HandlerRejected>((reply, Receipt::new(42, receipt_drops)))
                },
            ),
        )
        .await??;

    assert_eq!(receipt.value, 42);
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    let received =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut client_recv))
            .await??;
    assert!(
        received == correlated_response()?,
        "peer must observe the correlated reply"
    );
    let end_of_stream =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut client_recv)).await?;
    match end_of_stream {
        Err(transport::EnvelopeReceiveError::Frame(transport::FrameError::Truncated {
            expected,
            received,
        })) => {
            assert_eq!(expected, size_of::<u32>());
            assert_eq!(received, 0);
        }
        Err(other) => panic!("expected clean end-of-stream, got {other:?}"),
        Ok(envelope) => panic!(
            "expected exactly one reply, got second envelope with frame id {}",
            envelope.frame_id
        ),
    }
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(receipt);
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    drop(server_send);
    drop(server_recv);
    client_send.finish()?;
    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn protocol_failure_reply_like_a_receipt_is_returned_after_wire_success()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;

    let drops = Arc::new(AtomicUsize::new(0));
    let receipt_drops = Arc::clone(&drops);
    let reply = correlated_failure()?;

    transport::send_envelope(&mut client_send, &attach_project_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;

    let receipt =
        tokio::time::timeout(
            TEST_DEADLINE,
            transport::server_dispatch::dispatch_server_request_with_receipt(
                &mut server_send,
                &mut server_recv,
                move |_| async move {
                    Ok::<_, HandlerRejected>((reply, Receipt::new(7, receipt_drops)))
                },
            ),
        )
        .await??;

    assert_eq!(receipt.value, 7);
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    let received =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut client_recv))
            .await??;
    assert!(
        received == correlated_failure()?,
        "protocol failure must cross the wire unchanged"
    );
    let end_of_stream =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut client_recv)).await?;
    match end_of_stream {
        Err(transport::EnvelopeReceiveError::Frame(transport::FrameError::Truncated {
            expected,
            received,
        })) => {
            assert_eq!(expected, size_of::<u32>());
            assert_eq!(received, 0);
        }
        Err(other) => panic!("expected clean end-of-stream, got {other:?}"),
        Ok(envelope) => panic!(
            "expected one failure reply, got second envelope with frame id {}",
            envelope.frame_id
        ),
    }
    drop(receipt);
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    drop(server_send);
    drop(server_recv);
    client_send.finish()?;
    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn differently_correlated_reply_drops_receipt_and_writes_no_bytes()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;

    let drops = Arc::new(AtomicUsize::new(0));
    let receipt_drops = Arc::clone(&drops);
    transport::send_envelope(&mut client_send, &list_directories_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;

    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::server_dispatch::dispatch_server_request_with_receipt(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                Ok::<_, HandlerRejected>((
                    mismatched_response().expect("fixture"),
                    Receipt::new(99, receipt_drops),
                ))
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");

    assert!(
        matches!(
            result,
            Err(ServerDispatchError::Reply(
                ReplyValidationError::DifferentRequest
            ))
        ),
        "expected Reply(DifferentRequest), got {result:?}"
    );
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "receipt must be dropped on validation failure"
    );

    drop(server_send);
    expect_no_reply_bytes(&mut client_recv).await;
    drop(server_recv);
    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn wrong_family_reply_drops_receipt_and_writes_no_bytes() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;

    let drops = Arc::new(AtomicUsize::new(0));
    let receipt_drops = Arc::clone(&drops);
    transport::send_envelope(&mut client_send, &list_directories_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;

    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::server_dispatch::dispatch_server_request_with_receipt(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                Ok::<_, HandlerRejected>((
                    list_directories_request().expect("fixture"),
                    Receipt::new(100, receipt_drops),
                ))
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");

    assert!(
        matches!(
            result,
            Err(ServerDispatchError::Reply(
                ReplyValidationError::WrongFamily {
                    received: HandshakeMessageKind::Request
                }
            ))
        ),
        "expected Reply(WrongFamily(Request)), got {result:?}"
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    drop(server_send);
    expect_no_reply_bytes(&mut client_recv).await;
    drop(server_recv);
    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn uncorrelated_protocol_failure_drops_receipt() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;

    let drops = Arc::new(AtomicUsize::new(0));
    let receipt_drops = Arc::clone(&drops);
    transport::send_envelope(&mut client_send, &list_directories_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;

    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::server_dispatch::dispatch_server_request_with_receipt(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                Ok::<_, HandlerRejected>((
                    uncorrelated_failure().expect("fixture"),
                    Receipt::new(101, receipt_drops),
                ))
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");

    assert!(
        matches!(
            result,
            Err(ServerDispatchError::Reply(
                ReplyValidationError::Uncorrelated
            ))
        ),
        "expected Reply(Uncorrelated), got {result:?}"
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    drop(server_send);
    expect_no_reply_bytes(&mut client_recv).await;
    drop(server_recv);
    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn local_handler_error_preserves_local_source_and_produces_no_receipt()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;

    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    transport::send_envelope(&mut client_send, &list_directories_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;

    let result: Result<Receipt, ServerDispatchError<HandlerRejected>> = tokio::time::timeout(
        TEST_DEADLINE,
        transport::server_dispatch::dispatch_server_request_with_receipt(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Err(HandlerRejected)
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");

    let Err(error) = &result else {
        panic!("local handler failure must fail dispatch");
    };
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    match error {
        ServerDispatchError::Local(failure) => {
            assert_eq!(*failure, HandlerRejected);
            assert_eq!(error.to_string(), "the request handler failed");
        }
        other => panic!("expected Local failure, got {other:?}"),
    }

    drop(server_send);
    expect_no_reply_bytes(&mut client_recv).await;
    drop(server_recv);
    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn pre_finished_send_side_causes_send_or_finish_failure_and_drops_receipt()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, client_recv) = client_connection.open_bi().await?;

    let drops = Arc::new(AtomicUsize::new(0));
    let receipt_drops = Arc::clone(&drops);
    transport::send_envelope(&mut client_send, &list_directories_request()?).await?;
    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;

    // Deterministically break the post-handler wire step without sleeps: the
    // server send side is already finished before dispatch attempts to write
    // and finish again.
    server_send.finish()?;

    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::server_dispatch::dispatch_server_request_with_receipt(
            &mut server_send,
            &mut server_recv,
            move |_| async move {
                Ok::<_, HandlerRejected>((
                    correlated_response().expect("fixture"),
                    Receipt::new(55, receipt_drops),
                ))
            },
        ),
    )
    .await
    .expect("dispatch settles within deadline");

    assert!(
        matches!(
            result,
            Err(ServerDispatchError::Send(_) | ServerDispatchError::Finish(_))
        ),
        "expected Send or Finish failure after pre-finished stream, got {result:?}"
    );
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "receipt must be dropped on wire failure"
    );

    drop(server_send);
    drop(server_recv);
    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}
