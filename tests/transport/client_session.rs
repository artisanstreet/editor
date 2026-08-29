//! Component tests for the sequential caller-driven client session leaf
//! over real loopback QUIC.
//!
//! Scope: these tests exercise the public [`ClientSession`] surface end to
//! end against a real pinned server — handshake ownership, sequential
//! requests on one owner, correlated outcomes versus terminal local
//! errors, guarded abandonment, and finite shutdown. They are component
//! tests, not production backend integration: the server side is a small
//! handwritten fixture inside this file, and no backend module
//! participates.
//!
//! Runtime constraint (see the shared harness notes): two Quinn endpoints
//! sharing one Tokio runtime blackhole each other on Windows, so the
//! fixture below spawns a SERVER-ONLY endpoint on its own thread and
//! runtime. [`ClientSession`] creates its own fresh client endpoint on
//! the test runtime; the paired-client `spawn_loopback()` helper is
//! deliberately not used anywhere here.
//!
//! Causal evidence rules: every claim about wire activity is witnessed by
//! the peer observing frames, stream openings, or a typed application
//! connection close — never by sleeps. After the local action under test
//! (cancellation, timeout expiry, or a dropped future), witnesses wait —
//! bounded by failure-only watchdogs — for the PEER-SIDE positive closure
//! event and check its typed application-close cause; arbitrary write
//! failures are never treated as evidence because a peer may enqueue
//! output before processing the close packet. Absence claims are stated
//! at their exact strength: an empty fixture channel or an acceptance
//! resolving through closure is an observation at that check point, and
//! the stronger no-I/O claims additionally cite the production path where
//! the operation returns before its deadline-wrapped stage is ever
//! polled. The scripted server keeps its `Connection` handle alive until the client has consumed the final reply and
//! performed its expected shutdown or abandonment, so no fixture drop
//! can preempt a client-side close with an implicit one.

use std::error::Error;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use artisan_domain::{
    ConversationCursor, ConversationLifecycle, ConversationPatch, ConversationTurn, Event,
    ListAttachedProjects, PatchBatch, PatchId, PatchSequence, ProjectId, ProjectListing, Query,
    RequestId, Revision, ThreadCreated, ThreadId, ThreadSummary, ThreadTitle, TurnId, TurnOrdinal,
    UnixMillis,
};
use artisan_protocol::artisan_capnp;
use artisan_protocol::{
    ClientRequest, ConnectionId, ErrorCode, ErrorDetail, EventCursor, FrameId, Hello,
    HelloCredential, LocalCapability, ProtocolDecodeError, ProtocolFailure, ProtocolValueError,
    ProtocolVersion, ReconnectCapability, ResponsePayload, ServerEvent, ServerResponse,
    VersionOffer, Welcome, WireEnvelope, WireEnvelopeBody, encode_envelope,
};
use artisan_transport as transport;
use artisan_transport::{
    CancelHandle, ClientHello, ClientRequestError, ClientSession, ClientSessionError,
    ClientSessionLimits, DeadlineError, DeliveryLost, EnvelopeReceiveError, ExchangeError,
    FrameError, HandshakeError, HandshakeMessageKind, HandshakeStageError, LoopbackTarget,
    OperationKind, PinnedIdentity, ReplyRejection, RequestOutcome, TransportError,
};
use capnp::message::{Builder, HeapAllocator};
use capnp::serialize;
use quinn::{Connection, ConnectionError, ReadError, VarInt};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

/// Fixed rotated-capability bytes carried by every scripted Welcome;
/// public test material, never a secret.
const ROTATED_CAPABILITY: [u8; 32] = [0xc2; 32];

/// Fixed initial-capability bytes presented by every scripted Hello.
const INITIAL_CAPABILITY: [u8; 32] = [0x49; 32];

/// Connection diagnostic identity every scripted Welcome carries.
const CONNECTION_TAG: &str = "component-conn";

/// Test-only STOP code the scripted server applies to its OWN receive
/// half immediately after reading one Hello — the deliberate post-Hello
/// STOP the session must tolerate. Distinct from the leaf's private
/// teardown codes.
const FIXTURE_HELLO_STOP_CODE: u32 = 0x114;

/// The session leaf's fixed cross-process abandon code
/// (`client_session/link.rs`): the application-close cause every
/// abandonment witness expects.
const LEAF_ABANDON_CODE: u64 = 1;

/// The session leaf's fixed cross-process awaited-shutdown code
/// (`client_session/link.rs`): the application-close cause the drain
/// witness expects.
const LEAF_SHUTDOWN_CODE: u64 = 2;

/// The session leaf's fixed private STOP code for an abandoned delivery
/// stream (`client_session/link.rs`).
const LEAF_STREAM_STOP_CODE: u32 = 4;

/// Short bounded witness for the one-live-incoming-stream credit check.
const DELIVERY_BLOCKED_WINDOW: Duration = Duration::from_millis(100);

/// Server-only loopback fixture.
///
/// One Quinn server endpoint lives on its own thread and current-thread
/// runtime and forwards every established connection to the test. It
/// deliberately binds no client endpoint: the session under test creates
/// the only client endpoint on the test runtime.
mod fixture {
    use std::net::SocketAddr;
    use std::thread::JoinHandle;
    use std::time::Duration;

    use artisan_transport as transport;
    use quinn::{Connection, VarInt};

    /// Generous-but-bounded watchdog so a regression fails fast instead
    /// of hanging the runner.
    pub(super) const TEST_DEADLINE: Duration = Duration::from_secs(5);

    /// Private test-only owner of the spawned server thread, installed
    /// immediately after spawn and before any fallible setup wait so no
    /// early return can detach the OS thread.
    struct ThreadOwner {
        stop_signal: Option<tokio::sync::oneshot::Sender<()>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Drop for ThreadOwner {
        fn drop(&mut self) {
            // Taking through the option keeps this legal for a Drop
            // receiver; oneshot send consumes its sender.
            if let Some(stop_signal) = self.stop_signal.take() {
                let _signalled = stop_signal.send(());
            }
            if let Some(thread) = self.thread.take() {
                if std::thread::panicking() {
                    // Already unwinding: swallow a secondary join failure.
                    let _joined = thread.join();
                } else {
                    thread.join().expect("server thread finishes");
                }
            }
        }
    }

    /// The server side of one loopback pairing.
    ///
    /// Dropping the value always signals and joins the server thread, so
    /// early returns and panics never leave the OS thread detached; on a
    /// non-unwinding drop the join result is asserted.
    pub(super) struct TestServer {
        /// Address the server bound on `127.0.0.1`.
        pub(super) addr: SocketAddr,
        /// Established server-side connections, handed to the test
        /// exactly once.
        connections: Option<tokio::sync::mpsc::Receiver<Connection>>,
        /// Completing this signal asks the server thread to shut down.
        stop_signal: Option<tokio::sync::oneshot::Sender<()>>,
        /// The thread hosting the server runtime and endpoint.
        thread: Option<JoinHandle<()>>,
    }

    impl TestServer {
        /// Spawns the server endpoint with `server_config` and waits for
        /// its bound address.
        ///
        /// # Panics
        ///
        /// Panics when the server thread cannot start or bind within
        /// [`TEST_DEADLINE`].
        pub(super) fn start(server_config: quinn::ServerConfig) -> Self {
            let (addr_tx, addr_rx) = std::sync::mpsc::channel();
            let (connections_tx, connections_rx) = tokio::sync::mpsc::channel(4);
            let (stop_signal, mut stop_rx) = tokio::sync::oneshot::channel();

            let thread = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("server runtime");
                runtime.block_on(async move {
                    let server =
                        transport::bind_loopback_server(server_config).expect("server bind");
                    let bound = server.local_addr().expect("bound server address");
                    addr_tx
                        .send(bound)
                        .expect("the test waits for the server address");

                    loop {
                        let incoming = tokio::select! {
                            _ = &mut stop_rx => break,
                            incoming = server.accept() => incoming,
                        };
                        let Some(incoming) = incoming else {
                            break;
                        };
                        // A refused TLS acceptance (for example the
                        // wrong-pin case) surfaces here as an ordinary
                        // establishment failure and must not panic the
                        // fixture thread. Establishment and forwarding
                        // each have a watchdog; the stop signal is checked
                        // at the next accept, not during either wait.
                        match tokio::time::timeout(TEST_DEADLINE, incoming).await {
                            Ok(Ok(established)) => {
                                let forwarded = tokio::time::timeout(
                                    TEST_DEADLINE,
                                    connections_tx.send(established),
                                )
                                .await;
                                if forwarded.map_or(true, |sent| sent.is_err()) {
                                    break;
                                }
                            }
                            Ok(Err(_rejected)) => {}
                            Err(_watchdog) => break,
                        }
                    }

                    // Teardown failure is preserved: it panics the
                    // fixture thread so the owned-thread join in
                    // [`TestServer::drop`] surfaces it on every
                    // non-unwinding path. It is never silently swallowed.
                    match tokio::time::timeout(
                        TEST_DEADLINE,
                        transport::shutdown(
                            &server,
                            VarInt::from_u32(0),
                            b"component fixture complete",
                            TEST_DEADLINE,
                        ),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        _ => panic!("the fixture endpoint failed to drain within its watchdog"),
                    }
                });
            });

            // The thread owner is installed IMMEDIATELY after spawn,
            // before any fallible wait, so a setup failure or panic can
            // never detach the OS thread: dropping this guard signals
            // and joins that same thread.
            let mut thread_owner = ThreadOwner {
                stop_signal: Some(stop_signal),
                thread: Some(thread),
            };

            let addr = match addr_rx.recv_timeout(TEST_DEADLINE) {
                Ok(addr) => addr,
                Err(_setup_failure) => {
                    drop(thread_owner);
                    panic!("server did not bind in time");
                }
            };
            // Hand the already-owned thread to the returned server; the
            // guard's fields are taken, so its later drop is a no-op.
            Self {
                addr,
                connections: Some(connections_rx),
                stop_signal: thread_owner.stop_signal.take(),
                thread: thread_owner.thread.take(),
            }
        }

        /// Hands the fixture's connection channel to the test exactly
        /// once so an owned receiver can move into a spawned task or an
        /// inline dialogue.
        ///
        /// # Panics
        ///
        /// Panics when called twice; each test pairs with one server.
        pub(super) fn take_connections(&mut self) -> tokio::sync::mpsc::Receiver<Connection> {
            self.connections
                .take()
                .expect("the fixture connection channel is taken exactly once")
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            // Taking through the option keeps this legal for a Drop
            // receiver; oneshot send consumes its sender.
            if let Some(stop_signal) = self.stop_signal.take() {
                let _signalled = stop_signal.send(());
            }
            if let Some(thread) = self.thread.take() {
                if std::thread::panicking() {
                    // Already unwinding: swallow a secondary join failure.
                    let _joined = thread.join();
                } else {
                    thread.join().expect("server thread finishes");
                }
            }
        }
    }
}

use fixture::{TEST_DEADLINE, TestServer};

/// Generates an ephemeral localhost certificate, its private key, and
/// the matching pin.
fn ephemeral_identity() -> (
    CertificateDer<'static>,
    PrivatePkcs8KeyDer<'static>,
    PinnedIdentity,
) {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("valid SANs");
    let certificate = certified_key.cert.der().clone();
    let private_key = PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der());
    let pin = PinnedIdentity::from_certificate(&certificate);
    (certificate, private_key, pin)
}

/// Builds the fixture server configuration presenting `certificate`.
fn fixture_server_config(
    certificate: CertificateDer<'static>,
    private_key: PrivatePkcs8KeyDer<'static>,
) -> quinn::ServerConfig {
    transport::server_config(vec![certificate], private_key).expect("server configuration")
}

/// Whole-stage limits with a generous watchdog on every stage and the
/// caller-chosen lifetime admission budget.
fn component_limits(admission_budget: usize) -> ClientSessionLimits {
    ClientSessionLimits {
        connect: TEST_DEADLINE,
        handshake: TEST_DEADLINE,
        request: TEST_DEADLINE,
        shutdown: TEST_DEADLINE,
        admission_budget,
    }
}

/// Validates `addr` as the loopback target.
fn target(addr: SocketAddr) -> LoopbackTarget {
    LoopbackTarget::new(addr).expect("valid loopback target")
}

/// Builds an owned Hello envelope carrying the fixture credential.
fn hello_envelope(frame: &str) -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame)?,
        sent_at: UnixMillis::from_millis(1),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])?,
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
        }),
    })
}

/// Builds the scripted Welcome envelope.
fn welcome_envelope() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("fixture-welcome")?,
        sent_at: UnixMillis::from_millis(2),
        body: WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: ProtocolVersion::V1,
            connection_id: ConnectionId::parse(CONNECTION_TAG)?,
            reconnect_capability: ReconnectCapability::from_bytes(ROTATED_CAPABILITY),
        }),
    })
}

/// Builds a bounded pure-read request envelope.
fn request_envelope(frame: &str) -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame)?,
        sent_at: UnixMillis::from_millis(3),
        body: WireEnvelopeBody::Request(ClientRequest::Query(Query::ListAttachedProjects(
            ListAttachedProjects,
        ))),
    })
}

/// Builds the successful correlated response for `request_id`.
fn correlated_response(request_id: &RequestId) -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(format!("reply-{request_id}"))?,
        sent_at: UnixMillis::from_millis(4),
        body: WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id.clone(),
            payload: ResponsePayload::ProjectListing(ProjectListing::new(Vec::new())?),
        }),
    })
}

/// Builds a failure correlated to exactly `request_id`.
fn correlated_failure(request_id: &RequestId) -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(format!("reject-{request_id}"))?,
        sent_at: UnixMillis::from_millis(4),
        body: WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::Internal,
            detail: ErrorDetail::parse("scripted correlated failure")?,
            retryable: false,
            request_id: Some(request_id.clone()),
        }),
    })
}

/// Builds an uncorrelated failure envelope.
fn uncorrelated_failure() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("fixture-uncorrelated")?,
        sent_at: UnixMillis::from_millis(4),
        body: WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::InvalidInput,
            detail: ErrorDetail::parse("scripted uncorrelated failure")?,
            retryable: false,
            request_id: None,
        }),
    })
}

/// Builds the scripted thread summary used by event and patch fixtures.
fn thread_summary() -> Result<ThreadSummary, Box<dyn Error>> {
    Ok(ThreadSummary {
        thread_id: ThreadId::parse("fixture-thread")?,
        project_id: ProjectId::parse("fixture-project")?,
        title: ThreadTitle::parse("fixture title")?,
        created_at: UnixMillis::from_millis(5),
        updated_at: UnixMillis::from_millis(6),
    })
}

/// Builds an Event-family reply, which can never settle a request.
fn event_reply() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("fixture-event")?,
        sent_at: UnixMillis::from_millis(4),
        body: WireEnvelopeBody::Event(ServerEvent {
            cursor: EventCursor::new(1)?,
            event: Event::ThreadCreated(ThreadCreated {
                thread: thread_summary()?,
            }),
        }),
    })
}

/// Builds a PatchBatch-family reply, which can never settle a request.
fn patch_batch_reply() -> Result<WireEnvelope, Box<dyn Error>> {
    let turn = ConversationTurn {
        turn_id: TurnId::parse("fixture-turn")?,
        ordinal: TurnOrdinal::new(1),
        revision: Revision::new(1),
        lifecycle: ConversationLifecycle::Pending,
        created_at: UnixMillis::from_millis(7),
        updated_at: UnixMillis::from_millis(8),
    };
    let batch = PatchBatch::new(
        thread_summary()?.thread_id,
        ConversationCursor::new(1),
        ConversationCursor::new(2),
        vec![ConversationPatch::TurnUpsert {
            patch_id: PatchId::parse("fixture-patch")?,
            sequence: PatchSequence::new(2)?,
            turn,
        }],
    )?;
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("fixture-patch-batch")?,
        sent_at: UnixMillis::from_millis(4),
        body: WireEnvelopeBody::PatchBatch(batch),
    })
}

/// Serializes a raw wire envelope stamped with an unsupported revision.
///
/// `ProtocolVersion::new` admits only V1, so the malformed input is
/// produced at the generated-schema layer — the same pattern the
/// protocol conformance tests use — instead of corrupting typed values.
fn raw_unsupported_version_bytes() -> Vec<u8> {
    let mut message = Builder::new(HeapAllocator::new());
    let mut root = message.init_root::<artisan_capnp::envelope::Builder>();
    root.set_protocol_version(99);
    root.set_message_id("wire-v99");
    root.reborrow().init_body().init_hello();
    serialize::write_message_to_words(&message)
}

/// Encodes one envelope into the existing four-byte-length-prefixed wire
/// representation so a delivery fixture can deliberately split its output.
fn encoded_delivery_frame(envelope: &WireEnvelope) -> Result<Vec<u8>, Box<dyn Error>> {
    let encoded = encode_envelope(envelope)?;
    let length = u32::try_from(encoded.len())?;
    let mut framed = Vec::with_capacity(4 + encoded.len());
    framed.extend_from_slice(&length.to_le_bytes());
    framed.extend_from_slice(&encoded);
    Ok(framed)
}

/// One scripted reply the fixture server produces after reading exactly
/// one request.
enum Step {
    /// A successful response correlated to the request it settles.
    CorrelatedResponse,
    /// A typed failure correlated to the request it settles.
    CorrelatedFailure,
    /// A verbatim envelope regardless of what the request carried.
    Fixed(WireEnvelope),
    /// Raw framed bytes that bypass the owned-envelope encoder.
    Raw(Vec<u8>),
}

/// Receives the next fixture connection under the shared watchdog.
async fn next_connection(
    connections: &mut tokio::sync::mpsc::Receiver<Connection>,
) -> Result<Connection, Box<dyn Error>> {
    Ok(tokio::time::timeout(TEST_DEADLINE, connections.recv())
        .await
        .map_err(|_| "fixture connection timed out")?
        .ok_or("server stopped accepting before the dialogue completed")?)
}

/// Runs the application handshake as the scripted server: reads one
/// Hello, applies the deliberate post-Hello STOP to the server's own
/// receive half, then answers with `welcome` over the untouched opposite
/// direction and FINs that completed output. The STOP is the intended
/// behavior the session must tolerate; the FIN is ordinary completion of
/// the server's send side and is deliberately distinct from the STOP.
///
/// Returns the accepted stream pair so the caller can keep the server
/// handles alive until the client side has fully consumed the exchange.
async fn drive_handshake(
    connection: &Connection,
    welcome: &WireEnvelope,
) -> Result<(ClientHello, quinn::SendStream, quinn::RecvStream), Box<dyn Error>> {
    let (mut send, mut receive) = tokio::time::timeout(TEST_DEADLINE, connection.accept_bi())
        .await
        .map_err(|_| "handshake stream timed out")??;
    let hello = tokio::time::timeout(TEST_DEADLINE, transport::receive_client_hello(&mut receive))
        .await
        .map_err(|_| "hello read timed out")??;
    assert_eq!(hello.protocol_version, ProtocolVersion::V1);
    assert!(
        hello.hello.credential
            == HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY))
    );
    // Deliberate STOP of the server's receive half after exactly one
    // Hello; the client's send side observes this and must not treat it
    // as an authentication failure. The stop's ACTUAL success is part of
    // the scripted evidence, not a discarded detail.
    receive
        .stop(VarInt::from_u32(FIXTURE_HELLO_STOP_CODE))
        .map_err(|_| "the scripted post-Hello STOP could not be applied")?;
    tokio::time::timeout(
        TEST_DEADLINE,
        transport::send_server_welcome(&mut send, welcome, &hello.hello.supported_versions),
    )
    .await
    .map_err(|_| "welcome send timed out")??;
    send.finish()?;
    Ok((hello, send, receive))
}

/// Full scripted dialogue over an owned connection channel: accepts one
/// connection, performs the handshake with its deliberate STOP, serves
/// each step against one fresh request stream, and RETURNS the retained
/// connection so the test can keep the server handles alive until after
/// the client result and explicit shutdown or abandonment.
async fn serve_full(
    mut connections: tokio::sync::mpsc::Receiver<Connection>,
    steps: Vec<Step>,
) -> Result<Connection, Box<dyn Error>> {
    let connection = next_connection(&mut connections).await?;
    let welcome = welcome_envelope()?;
    // The handshake handles end here; the CONNECTION is returned below
    // so the test retains live peer handles past the client result.
    let (_hello, _handshake_send, _handshake_receive) =
        drive_handshake(&connection, &welcome).await?;

    for step in steps {
        let (mut send, mut receive) = tokio::time::timeout(TEST_DEADLINE, connection.accept_bi())
            .await
            .map_err(|_| "request stream timed out")??;
        let request =
            tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut receive))
                .await
                .map_err(|_| "request read timed out")??;
        assert!(
            matches!(&request.body, WireEnvelopeBody::Request(_)),
            "the fixture only scripts request streams"
        );
        let request_id = request.frame_id.to_request_id()?;
        match step {
            Step::CorrelatedResponse => {
                let reply = correlated_response(&request_id)?;
                tokio::time::timeout(TEST_DEADLINE, transport::send_envelope(&mut send, &reply))
                    .await
                    .map_err(|_| "response send timed out")??;
            }
            Step::CorrelatedFailure => {
                let reply = correlated_failure(&request_id)?;
                tokio::time::timeout(TEST_DEADLINE, transport::send_envelope(&mut send, &reply))
                    .await
                    .map_err(|_| "failure send timed out")??;
            }
            Step::Fixed(reply) => {
                tokio::time::timeout(TEST_DEADLINE, transport::send_envelope(&mut send, &reply))
                    .await
                    .map_err(|_| "fixed reply send timed out")??;
            }
            Step::Raw(bytes) => {
                tokio::time::timeout(TEST_DEADLINE, transport::write_frame(&mut send, &bytes))
                    .await
                    .map_err(|_| "raw reply write timed out")??;
            }
        }
        send.finish()?;
    }
    // The handshake stream handles end here; the CONNECTION is returned
    // and retained by the caller until the client result and shutdown.
    Ok(connection)
}

/// Shared spawned-task scenario for absence witnesses: handshakes with
/// the deliberate STOP, then waits for a stream that must never come.
/// The acceptance resolving through the leaf's typed application close —
/// positively checked by code — is the peer-side observation of closure
/// with no accepted stream at this check; combined with the production
/// path where pre-stage cancellation returns before the exchange stage
/// is ever polled, it evidences the no-stream claim. Every await is
/// bounded.
async fn handshake_then_witness_closed(
    mut connections: tokio::sync::mpsc::Receiver<Connection>,
) -> bool {
    let Ok(Some(connection)) = tokio::time::timeout(TEST_DEADLINE, connections.recv()).await else {
        return false;
    };
    let Ok(welcome) = welcome_envelope() else {
        return false;
    };
    let Ok((_hello, _send, _receive)) = drive_handshake(&connection, &welcome).await else {
        return false;
    };
    // Resolves through closure only once the session closes: positive
    // proof that no request stream existed before that point, carrying
    // the leaf's typed application-close cause.
    match tokio::time::timeout(TEST_DEADLINE, connection.accept_bi()).await {
        Ok(Err(ConnectionError::ApplicationClosed(close))) => {
            u64::from(close.error_code) == LEAF_ABANDON_CODE
        }
        _ => false,
    }
}

/// Opens exactly one server-owned delivery stream, writes only the prefix and
/// one body byte of one envelope, and waits until the client has polled the
/// receive future again after that write. The client-side future is then
/// known to have accepted the stream and to be blocked inside its frame read;
/// the server witnesses the receiver's synchronous STOP before serving one
/// ordinary request on the still-live session connection.
async fn serve_fragmented_delivery(
    mut connections: tokio::sync::mpsc::Receiver<Connection>,
    initial_poll: tokio::sync::oneshot::Receiver<()>,
    post_poll: tokio::sync::oneshot::Receiver<()>,
    ready: tokio::sync::oneshot::Sender<()>,
) -> Result<Connection, Box<dyn Error>> {
    let connection = next_connection(&mut connections).await?;
    let welcome = welcome_envelope()?;
    let (_hello, _handshake_send, _handshake_receive) =
        drive_handshake(&connection, &welcome).await?;

    tokio::time::timeout(TEST_DEADLINE, initial_poll)
        .await
        .map_err(|_| "delivery receive future did not start")?
        .map_err(|_| "delivery receive poll witness was dropped before acceptance")?;

    let mut delivery_send = tokio::time::timeout(TEST_DEADLINE, connection.open_uni())
        .await
        .map_err(|_| "delivery stream did not open within the fixture watchdog")??;
    let framed = encoded_delivery_frame(&event_reply()?)?;
    assert!(
        framed.len() > 5,
        "the scripted envelope must have a body byte after its four-byte prefix"
    );
    delivery_send.write_all(&framed[..5]).await?;

    tokio::time::timeout(TEST_DEADLINE, post_poll)
        .await
        .map_err(|_| "delivery receive future did not poll after the fragment")?
        .map_err(|_| "delivery receive poll witness was dropped before the STOP")?;
    ready
        .send(())
        .map_err(|()| "the client abandoned before receiving the mid-frame witness")?;

    let stopped = tokio::time::timeout(TEST_DEADLINE, delivery_send.stopped())
        .await
        .map_err(|_| "the peer did not witness the delivery STOP within the watchdog")?
        .map_err(|_| "the delivery STOP witness failed")?;
    assert_eq!(
        stopped,
        Some(VarInt::from_u32(LEAF_STREAM_STOP_CODE)),
        "an abandoned fragmented delivery must use the private stream STOP code"
    );

    let (mut send, mut receive) = tokio::time::timeout(TEST_DEADLINE, connection.accept_bi())
        .await
        .map_err(|_| "the post-delivery request stream timed out")??;
    let request = tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut receive))
        .await
        .map_err(|_| "the post-delivery request read timed out")??;
    let reply = correlated_response(&request.frame_id.to_request_id()?)?;
    tokio::time::timeout(TEST_DEADLINE, transport::send_envelope(&mut send, &reply))
        .await
        .map_err(|_| "the post-delivery response send timed out")??;
    send.finish()?;
    Ok(connection)
}

/// Sends two complete envelopes on one still-open delivery stream and checks
/// that the client's one-stream credit prevents a second server stream while
/// the first stream remains owned by the consuming receiver.
async fn serve_two_delivery_frames(
    mut connections: tokio::sync::mpsc::Receiver<Connection>,
    mut client_read_done: tokio::sync::oneshot::Receiver<()>,
    second_stream_witness: tokio::sync::oneshot::Sender<bool>,
) -> Result<Connection, Box<dyn Error>> {
    let connection = next_connection(&mut connections).await?;
    let welcome = welcome_envelope()?;
    let (_hello, _handshake_send, _handshake_receive) =
        drive_handshake(&connection, &welcome).await?;

    let mut delivery_send = tokio::time::timeout(TEST_DEADLINE, connection.open_uni())
        .await
        .map_err(|_| "delivery stream did not open within the fixture watchdog")??;
    let first = event_reply()?;
    let second = patch_batch_reply()?;
    tokio::time::timeout(
        TEST_DEADLINE,
        transport::send_envelope(&mut delivery_send, &first),
    )
    .await
    .map_err(|_| "the first delivery frame send timed out")??;
    tokio::time::timeout(
        TEST_DEADLINE,
        transport::send_envelope(&mut delivery_send, &second),
    )
    .await
    .map_err(|_| "the second delivery frame send timed out")??;

    tokio::time::timeout(TEST_DEADLINE, &mut client_read_done)
        .await
        .map_err(|_| "the client did not consume both delivery frames")?
        .map_err(|_| "the client read witness was dropped")?;
    let second_stream_blocked =
        tokio::time::timeout(DELIVERY_BLOCKED_WINDOW, connection.open_uni())
            .await
            .is_err();
    second_stream_witness
        .send(second_stream_blocked)
        .map_err(|bool| format!("the second-stream witness receiver dropped: {bool}"))?;
    Ok(connection)
}

/// Failure mode scripted by the peer for a delivery receiver.
#[derive(Clone, Copy)]
enum DeliveryFailure {
    /// The server cleanly FINs before writing a frame.
    CleanStream,
    /// The server writes a zero-length frame prefix.
    MalformedFrame,
}

/// Produces one clean or malformed delivery stream, then serves one ordinary
/// request so the stream-local failure remains distinct from session loss.
async fn serve_failed_delivery(
    mut connections: tokio::sync::mpsc::Receiver<Connection>,
    failure: DeliveryFailure,
) -> Result<Connection, Box<dyn Error>> {
    let connection = next_connection(&mut connections).await?;
    let welcome = welcome_envelope()?;
    let (_hello, _handshake_send, _handshake_receive) =
        drive_handshake(&connection, &welcome).await?;

    let mut delivery_send = tokio::time::timeout(TEST_DEADLINE, connection.open_uni())
        .await
        .map_err(|_| "delivery stream did not open within the fixture watchdog")??;
    match failure {
        DeliveryFailure::CleanStream => delivery_send.finish()?,
        DeliveryFailure::MalformedFrame => delivery_send.write_all(&0u32.to_le_bytes()).await?,
    }

    if matches!(failure, DeliveryFailure::MalformedFrame) {
        let stopped = tokio::time::timeout(TEST_DEADLINE, delivery_send.stopped())
            .await
            .map_err(|_| "the malformed delivery STOP was not witnessed")?
            .map_err(|_| "the malformed delivery STOP witness failed")?;
        assert_eq!(
            stopped,
            Some(VarInt::from_u32(LEAF_STREAM_STOP_CODE)),
            "malformed delivery input must stop the abandoned stream"
        );
    }

    let (mut send, mut receive) = tokio::time::timeout(TEST_DEADLINE, connection.accept_bi())
        .await
        .map_err(|_| "the post-failure request stream timed out")??;
    let request = tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut receive))
        .await
        .map_err(|_| "the post-failure request read timed out")??;
    let reply = correlated_response(&request.frame_id.to_request_id()?)?;
    tokio::time::timeout(TEST_DEADLINE, transport::send_envelope(&mut send, &reply))
        .await
        .map_err(|_| "the post-failure response send timed out")??;
    send.finish()?;
    Ok(connection)
}

/// Runs the fragmented-frame scenario once with cancellation and once with a
/// dropped receive future. Both paths must stop the stream, return no
/// receiver, and leave the separately held session usable.
async fn fragmented_delivery_abandonment(cancel_mid_frame: bool) -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let (initial_tx, initial_rx) = tokio::sync::oneshot::channel();
    let (post_tx, post_rx) = tokio::sync::oneshot::channel();
    let (ready_tx, mut ready_rx) = tokio::sync::oneshot::channel();
    let connections = server.take_connections();
    let server_task = tokio::spawn(async move {
        serve_fragmented_delivery(connections, initial_rx, post_rx, ready_tx)
            .await
            .map_err(|err| err.to_string())
    });
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server.addr),
            certificate,
            pin,
            hello_envelope("fragmented-hello")?,
            component_limits(2),
            &CancelHandle::new(),
        )
        .await?;
        let (session, receiver) = session.take_delivery()?;
        let delivery_cancel = CancelHandle::new();
        let mut receive = Box::pin(receiver.recv(&delivery_cancel));
        let mut initial_tx = Some(initial_tx);
        let mut post_tx = Some(post_tx);
        let mut witnessed = Box::pin(std::future::poll_fn(move |context| {
            let poll = receive.as_mut().poll(context);
            if poll.is_pending() {
                if let Some(tx) = initial_tx.take() {
                    let _ = tx.send(());
                } else if let Some(tx) = post_tx.take() {
                    let _ = tx.send(());
                }
            }
            poll
        }));

        tokio::select! {
            biased;
            _ = &mut witnessed => {
                panic!("fragmented delivery receive completed before the abort");
            }
            ready = &mut ready_rx => {
                ready.map_err(|_| "fragmented delivery readiness witness dropped")?;
            }
        }

        if cancel_mid_frame {
            delivery_cancel.cancel();
            let result = witnessed.await;
            assert!(matches!(result, Err(DeliveryLost)));
        } else {
            drop(witnessed);
        }

        let request_cancel = CancelHandle::new();
        let (session, resolved) = session
            .request(request_envelope("after-fragmented-abort")?, &request_cancel)
            .await?;
        assert!(matches!(resolved.outcome(), RequestOutcome::Response(_)));
        session.shutdown(&request_cancel).await?;
        Ok::<_, Box<dyn Error>>(())
    };

    let client_result = client.await;
    let server_result = tokio::time::timeout(TEST_DEADLINE, server_task)
        .await
        .map_err(|_| "fragmented delivery server task timed out")?
        .map_err(|err| -> Box<dyn Error> {
            format!("fragmented delivery server task panicked: {err}").into()
        })?
        .map_err(|err: String| -> Box<dyn Error> { err.into() })?;
    client_result?;
    drop(server_result);
    drop(server);
    Ok(())
}

/// Runs a clean or malformed delivery stream and proves the consuming
/// receiver exposes only the payload-free loss condition while the session
/// owner remains usable for an ordinary request.
async fn failed_delivery_is_typed(failure: DeliveryFailure) -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_side = serve_failed_delivery(server.take_connections(), failure);
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server.addr),
            certificate,
            pin,
            hello_envelope("failed-delivery-hello")?,
            component_limits(2),
            &CancelHandle::new(),
        )
        .await?;
        let (session, receiver) = session.take_delivery()?;
        let delivery_cancel = CancelHandle::new();
        let outcome = receiver.recv(&delivery_cancel).await;
        assert!(matches!(outcome, Err(DeliveryLost)));

        let request_cancel = CancelHandle::new();
        let (session, resolved) = session
            .request(request_envelope("after-delivery-loss")?, &request_cancel)
            .await?;
        assert!(matches!(resolved.outcome(), RequestOutcome::Response(_)));
        session.shutdown(&request_cancel).await?;
        Ok::<_, Box<dyn Error>>(())
    };

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// The first delivery take is a synchronous ownership split: the server has
/// not opened a uni stream, yet the returned session still completes an
/// ordinary request over its original connection.
#[tokio::test]
async fn first_delivery_take_is_immediate_and_requests_remain_usable() -> Result<(), Box<dyn Error>>
{
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("take-hello")?,
            component_limits(2),
            &CancelHandle::new(),
        )
        .await?;
        let (session, receiver) = session.take_delivery()?;
        assert_eq!(format!("{receiver:?}"), "DeliveryReceiver { .. }");
        assert_eq!(format!("{DeliveryLost:?}"), "DeliveryLost");
        assert_eq!(DeliveryLost.to_string(), "delivery stream was lost");
        drop(receiver);

        let request_cancel = CancelHandle::new();
        let (session, resolved) = session
            .request(request_envelope("after-take")?, &request_cancel)
            .await?;
        assert!(matches!(resolved.outcome(), RequestOutcome::Response(_)));
        session.shutdown(&request_cancel).await?;
        Ok::<_, Box<dyn Error>>(())
    };
    let server_side = serve_full(server.take_connections(), vec![Step::CorrelatedResponse]);

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// A second delivery take is the exact typed terminal error; consuming that
/// error drops the session and the peer witnesses the existing abandon close.
#[tokio::test]
async fn second_delivery_take_is_terminal_and_closes_the_session() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_task = tokio::spawn(handshake_then_witness_closed(server.take_connections()));
    let joined = tokio::time::timeout(TEST_DEADLINE, server_task);
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server.addr),
            certificate,
            pin,
            hello_envelope("second-take-hello")?,
            component_limits(1),
            &CancelHandle::new(),
        )
        .await?;
        let (session, receiver) = session.take_delivery()?;
        drop(receiver);
        let second = session.take_delivery();
        assert!(matches!(
            second,
            Err(ClientSessionError::DeliveryAlreadyTaken)
        ));
        Ok::<_, Box<dyn Error>>(())
    };

    let (client_result, joined) = tokio::join!(client, joined);
    let closed_without_streams = joined
        .expect("second-take fixture task finishes")
        .expect("second-take fixture task does not panic");
    assert!(closed_without_streams);
    client_result?;
    drop(server);
    Ok(())
}

/// Two consuming receives read two frames from one server-owned stream; the
/// receiver never accepts a second stream, and the live first stream keeps a
/// second server open blocked by the one-stream credit.
#[tokio::test]
async fn delivery_receives_two_frames_sequentially_on_one_stream() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let (read_done_tx, read_done_rx) = tokio::sync::oneshot::channel();
    let (second_stream_tx, second_stream_rx) = tokio::sync::oneshot::channel();
    let server_side =
        serve_two_delivery_frames(server.take_connections(), read_done_rx, second_stream_tx);
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server.addr),
            certificate,
            pin,
            hello_envelope("two-delivery-hello")?,
            component_limits(1),
            &CancelHandle::new(),
        )
        .await?;
        let (session, receiver) = session.take_delivery()?;
        let delivery_cancel = CancelHandle::new();
        let (receiver, first) = receiver.recv(&delivery_cancel).await?;
        assert_eq!(first.frame_id.as_str(), "fixture-event");
        assert!(matches!(first.body, WireEnvelopeBody::Event(_)));
        let (receiver, second) = receiver.recv(&delivery_cancel).await?;
        assert_eq!(second.frame_id.as_str(), "fixture-patch-batch");
        assert!(matches!(second.body, WireEnvelopeBody::PatchBatch(_)));
        read_done_tx
            .send(())
            .map_err(|()| "the two-frame server stopped before the read witness")?;
        let second_stream_blocked = second_stream_rx
            .await
            .map_err(|_| "the second-stream witness was dropped")?;
        assert!(second_stream_blocked);
        drop(receiver);
        session.shutdown(&delivery_cancel).await?;
        Ok::<_, Box<dyn Error>>(())
    };

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// Cancellation before the first accept returns `DeliveryLost` without
/// touching the wire, while the separately held session can still issue a
/// request on the fixture's connection.
#[tokio::test]
async fn pre_accept_delivery_cancellation_is_typed_and_nonterminal_to_session()
-> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_side = serve_full(server.take_connections(), vec![Step::CorrelatedResponse]);
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server.addr),
            certificate,
            pin,
            hello_envelope("pre-accept-hello")?,
            component_limits(2),
            &CancelHandle::new(),
        )
        .await?;
        let (session, receiver) = session.take_delivery()?;
        let delivery_cancel = CancelHandle::new();
        delivery_cancel.cancel();
        assert!(matches!(
            receiver.recv(&delivery_cancel).await,
            Err(DeliveryLost)
        ));

        let request_cancel = CancelHandle::new();
        let (session, resolved) = session
            .request(
                request_envelope("after-pre-accept-cancel")?,
                &request_cancel,
            )
            .await?;
        assert!(matches!(resolved.outcome(), RequestOutcome::Response(_)));
        session.shutdown(&request_cancel).await?;
        Ok::<_, Box<dyn Error>>(())
    };

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// A cancellation while a fragmented envelope is being read stops the exact
/// stream with the private code and leaves the session's request owner live.
#[tokio::test]
async fn mid_frame_delivery_cancellation_stops_without_returning_receiver()
-> Result<(), Box<dyn Error>> {
    fragmented_delivery_abandonment(true).await
}

/// Dropping a receive future after a fragmented frame has begun has the same
/// stream custody result, without closing the separately held session.
#[tokio::test]
async fn dropping_mid_frame_delivery_stops_without_returning_receiver() -> Result<(), Box<dyn Error>>
{
    fragmented_delivery_abandonment(false).await
}

/// A cleanly finished delivery stream is a payload-free loss and does not
/// prevent the separate session owner from completing another request.
#[tokio::test]
async fn clean_delivery_stream_failure_maps_to_delivery_lost() -> Result<(), Box<dyn Error>> {
    failed_delivery_is_typed(DeliveryFailure::CleanStream).await
}

/// A malformed bounded frame is a payload-free loss and its abandoned stream
/// is stopped before the still-live session serves another request.
#[tokio::test]
async fn malformed_delivery_frame_maps_to_delivery_lost() -> Result<(), Box<dyn Error>> {
    failed_delivery_is_typed(DeliveryFailure::MalformedFrame).await
}

/// A successful pinned handshake hands the Welcome — and its rotated
/// capability custody — to the caller, retains only negotiated
/// non-secret metadata in the session, and tolerates the scripted
/// server's deliberate post-Hello STOP.
#[tokio::test]
async fn pinned_handshake_hands_the_welcome_to_the_caller() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let mut connections = server.take_connections();
    let hello = hello_envelope("caller-hello")?;
    let client = async {
        ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello,
            component_limits(2),
            &cancel,
        )
        .await
    };
    let server_side = async {
        let connection = next_connection(&mut connections).await?;
        let welcome = welcome_envelope()?;
        let (hello, send, receive) = drive_handshake(&connection, &welcome).await?;
        assert_eq!(hello.frame_id.as_str(), "caller-hello");
        // Retained until after the client branch has consumed everything
        // and performed its shutdown inside the join below.
        Ok::<_, Box<dyn Error>>((connection, send, receive))
    };

    let (client_result, server_result) = tokio::join!(client, server_side);
    let (session, welcome) = client_result?;
    let (_connection, _send, _receive) = server_result?;
    drop(connections);

    assert_eq!(welcome.protocol_version, ProtocolVersion::V1);
    // Field-wise equality: Welcome deliberately derives no Debug, so no
    // assertion may format the rotated capability.
    assert!(
        welcome.welcome.negotiated_version == ProtocolVersion::V1
            && welcome.welcome.connection_id.as_str() == CONNECTION_TAG
            && welcome.welcome.reconnect_capability
                == ReconnectCapability::from_bytes(ROTATED_CAPABILITY),
        "the caller receives the complete Welcome including rotated custody"
    );
    assert_eq!(session.protocol_version(), ProtocolVersion::V1);
    assert_eq!(session.connection_id().as_str(), CONNECTION_TAG);
    assert_eq!(session.pending_capacity(), transport::PENDING_CAPACITY);
    assert_eq!(session.admission_budget(), 2);
    assert_eq!(session.admitted(), 0);

    session.shutdown(&cancel).await?;
    drop(server);
    Ok(())
}

/// Three sequential successful requests preserve one live owner; the
/// lifecycle's admitted count delegates through the session.
#[tokio::test]
async fn sequential_requests_preserve_one_live_owner() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let client = async {
        let (mut session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("owner-hello")?,
            component_limits(4),
            &cancel,
        )
        .await?;

        for index in 0..3 {
            let frame = format!("owner-request-{index}");
            let (owner, resolved) = session.request(request_envelope(&frame)?, &cancel).await?;
            session = owner;
            assert_eq!(resolved.request_id().as_str(), frame);
            assert!(
                matches!(resolved.outcome(), RequestOutcome::Response(_)),
                "each scripted response settles successfully"
            );
            assert_eq!(session.admitted(), index + 1);
        }
        assert_eq!(session.admission_budget(), 4);
        session.shutdown(&cancel).await?;
        Ok::<_, Box<dyn Error>>(())
    };

    let server_side = serve_full(
        server.take_connections(),
        vec![
            Step::CorrelatedResponse,
            Step::CorrelatedResponse,
            Step::CorrelatedResponse,
        ],
    );

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// Replaying a retired correlation identity is rejected by the registry
/// before any network attempt, which consumes the session: the fixture
/// serves exactly one request stream because the replay never dials.
#[tokio::test]
async fn retired_identity_admission_consumes_the_session() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("retire-hello")?,
            component_limits(4),
            &cancel,
        )
        .await?;
        let (session, _) = session
            .request(request_envelope("retire-one")?, &cancel)
            .await?;

        let replayed = session
            .request(request_envelope("retire-one")?, &cancel)
            .await;
        assert!(
            matches!(
                replayed,
                Err(ClientRequestError::Admission(
                    transport::RequestCorrelationError::Retired
                ))
            ),
            "the retired identity must be rejected before the wire"
        );
        Ok::<_, Box<dyn Error>>(())
    };

    let server_side = serve_full(server.take_connections(), vec![Step::CorrelatedResponse]);

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// Exhausting the caller-supplied lifetime budget delegates the
/// registry's permanent rejection through the session.
#[tokio::test]
async fn lifetime_budget_exhaustion_delegates_to_the_registry() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("budget-hello")?,
            component_limits(1),
            &cancel,
        )
        .await?;
        let (session, _) = session
            .request(request_envelope("budget-one")?, &cancel)
            .await?;

        let exhausted = session
            .request(request_envelope("budget-two")?, &cancel)
            .await;
        assert!(matches!(
            exhausted,
            Err(ClientRequestError::Admission(
                transport::RequestCorrelationError::LifetimeExhausted { budget: 1 }
            ))
        ));
        Ok::<_, Box<dyn Error>>(())
    };

    let server_side = serve_full(server.take_connections(), vec![Step::CorrelatedResponse]);

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// A correlated peer failure is an ordinary outcome: the SAME live owner
/// returns and immediately serves another request.
#[tokio::test]
async fn correlated_failure_is_an_ordinary_outcome_on_the_same_live_owner()
-> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("failure-hello")?,
            component_limits(4),
            &cancel,
        )
        .await?;

        let (session, failed) = session
            .request(request_envelope("failure-request")?, &cancel)
            .await?;
        let RequestOutcome::Failure(failure) = failed.outcome() else {
            panic!("the scripted correlated failure must settle as an outcome");
        };
        assert_eq!(failure.code, ErrorCode::Internal);
        assert_eq!(
            failure.request_id.as_ref().map(RequestId::as_str),
            Some("failure-request")
        );

        let (session, recovered) = session
            .request(request_envelope("recovery-request")?, &cancel)
            .await?;
        assert!(matches!(recovered.outcome(), RequestOutcome::Response(_)));
        assert_eq!(session.admitted(), 2);
        session.shutdown(&cancel).await?;
        Ok::<_, Box<dyn Error>>(())
    };

    let server_side = serve_full(
        server.take_connections(),
        vec![Step::CorrelatedFailure, Step::CorrelatedResponse],
    );

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// A reply stamped with an unsupported wire revision is a terminal
/// local error surfaced by the codec authority; no waiter settles and
/// the session closes. The malformed bytes come from the generated
/// schema layer because the typed constructor admits only V1.
#[tokio::test]
async fn unsupported_wire_version_reply_is_a_terminal_local_error() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("v99-hello")?,
            component_limits(2),
            &cancel,
        )
        .await?;

        let rejected = session
            .request(request_envelope("v99-request")?, &cancel)
            .await;
        assert!(matches!(
            rejected,
            Err(ClientRequestError::Exchange(DeadlineError::Peer {
                operation: OperationKind::Receive,
                error: ExchangeError::Receive(EnvelopeReceiveError::Decode(
                    ProtocolDecodeError::ProtocolValue {
                        source: ProtocolValueError::UnsupportedVersion { version: 99 }
                    }
                ))
            }))
        ));
        Ok::<_, Box<dyn Error>>(())
    };

    let server_side = serve_full(
        server.take_connections(),
        vec![Step::Raw(raw_unsupported_version_bytes())],
    );

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// An Event reply is a terminal wrong-family rejection: events never
/// settle requests, no cursor advances, and the session closes.
#[tokio::test]
async fn event_family_reply_is_terminal() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("event-hello")?,
            component_limits(2),
            &cancel,
        )
        .await?;

        let rejected = session
            .request(request_envelope("event-request")?, &cancel)
            .await;
        assert!(matches!(
            rejected,
            Err(ClientRequestError::Reply(
                ReplyRejection::UnexpectedFamily {
                    received: HandshakeMessageKind::Event
                }
            ))
        ));
        Ok::<_, Box<dyn Error>>(())
    };

    let server_side = serve_full(server.take_connections(), vec![Step::Fixed(event_reply()?)]);

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// A `PatchBatch` reply is a terminal wrong-family rejection for the same
/// reason; conversation delivery never routes through a request waiter.
#[tokio::test]
async fn patch_batch_family_reply_is_terminal() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("patch-hello")?,
            component_limits(2),
            &cancel,
        )
        .await?;

        let rejected = session
            .request(request_envelope("patch-request")?, &cancel)
            .await;
        assert!(matches!(
            rejected,
            Err(ClientRequestError::Reply(
                ReplyRejection::UnexpectedFamily {
                    received: HandshakeMessageKind::PatchBatch
                }
            ))
        ));
        Ok::<_, Box<dyn Error>>(())
    };

    let server_side = serve_full(
        server.take_connections(),
        vec![Step::Fixed(patch_batch_reply()?)],
    );

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// A failure carrying no request id correlates to nothing: terminal
/// rejection, no settlement, session closed.
#[tokio::test]
async fn missing_correlation_reply_is_terminal() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("missing-hello")?,
            component_limits(2),
            &cancel,
        )
        .await?;

        let rejected = session
            .request(request_envelope("missing-request")?, &cancel)
            .await;
        assert!(matches!(
            rejected,
            Err(ClientRequestError::Reply(
                ReplyRejection::MissingCorrelation
            ))
        ));
        Ok::<_, Box<dyn Error>>(())
    };

    let server_side = serve_full(
        server.take_connections(),
        vec![Step::Fixed(uncorrelated_failure()?)],
    );

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// A reply settling a DIFFERENT (already-retired) request identity is a
/// terminal rejection: the stale id must never resolve the current
/// operation's private waiter. The server retains its handles until the
/// client branch has consumed both exchanges.
#[tokio::test]
async fn stale_correlation_reply_settles_nothing() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let server_addr = server.addr;

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server_addr),
            certificate,
            pin,
            hello_envelope("stale-hello")?,
            component_limits(4),
            &cancel,
        )
        .await?;

        // First request retires its identity normally.
        let (session, first) = session
            .request(request_envelope("stale-first")?, &cancel)
            .await?;
        assert!(matches!(first.outcome(), RequestOutcome::Response(_)));

        // Second request is answered with the FIRST request's id.
        let rejected = session
            .request(request_envelope("stale-second")?, &cancel)
            .await;
        assert!(matches!(
            rejected,
            Err(ClientRequestError::Reply(
                ReplyRejection::DifferentCorrelation
            ))
        ));
        Ok::<_, Box<dyn Error>>(())
    };

    let server_side = async {
        let mut connections = server.take_connections();
        let connection = next_connection(&mut connections).await?;
        let welcome = welcome_envelope()?;
        let (_hello, handshake_send, handshake_receive) =
            drive_handshake(&connection, &welcome).await?;

        // Stream one: answer correctly.
        let (mut send, mut receive) = tokio::time::timeout(TEST_DEADLINE, connection.accept_bi())
            .await
            .map_err(|_| "first request stream timed out")??;
        let request =
            tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut receive))
                .await
                .map_err(|_| "first request read timed out")??;
        let reply = correlated_response(&request.frame_id.to_request_id()?)?;
        tokio::time::timeout(TEST_DEADLINE, transport::send_envelope(&mut send, &reply))
            .await
            .map_err(|_| "first reply send timed out")??;
        send.finish()?;

        // Stream two: replay the FIRST request's correlation.
        let (mut send, mut receive) = tokio::time::timeout(TEST_DEADLINE, connection.accept_bi())
            .await
            .map_err(|_| "second request stream timed out")??;
        let request =
            tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut receive))
                .await
                .map_err(|_| "second request read timed out")??;
        assert_eq!(request.frame_id.as_str(), "stale-second");
        let stale = correlated_response(&RequestId::parse("stale-first")?)?;
        tokio::time::timeout(TEST_DEADLINE, transport::send_envelope(&mut send, &stale))
            .await
            .map_err(|_| "stale reply send timed out")??;
        send.finish()?;
        // Retained until the client branch has consumed both exchanges.
        let _retained = (handshake_send, handshake_receive);
        Ok::<_, Box<dyn Error>>(connection)
    };

    let (client_result, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    client_result?;
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// Target validation rejects remote addresses, IPv6 loopback, other
/// loopback spellings, and zero port — with discriminants that separate
/// unsupported addresses from zero ports — entirely before any network
/// resource could exist. No server runs during this test at all.
#[test]
fn invalid_targets_fail_before_any_network_attempt() -> Result<(), Box<dyn Error>> {
    let remote = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)), 44_321);
    assert_eq!(
        LoopbackTarget::new(remote),
        Err(transport::SessionTargetError::UnsupportedAddress)
    );

    let ipv6_loopback = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 44_321);
    assert_eq!(
        LoopbackTarget::new(ipv6_loopback),
        Err(transport::SessionTargetError::UnsupportedAddress),
        "::1 is loopback but unsupported; it must not be called remote"
    );

    let other_loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), 44_321);
    assert_eq!(
        LoopbackTarget::new(other_loopback),
        Err(transport::SessionTargetError::UnsupportedAddress)
    );

    let zero_port = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    assert_eq!(
        LoopbackTarget::new(zero_port),
        Err(transport::SessionTargetError::ZeroPort)
    );

    let valid = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 44_321);
    assert_eq!(LoopbackTarget::new(valid)?.addr(), valid);
    Ok(())
}

/// Pinning the WRONG fingerprint fails the QUIC stage before any
/// application stream exists while everything else — trust root
/// included — stays identical to the successful pairing, isolating the
/// pin as the sole varied cause; the fixture handles the refused TLS
/// acceptance as an ordinary event.
#[tokio::test]
async fn wrong_pin_fails_the_quic_stage_without_application_streams() -> Result<(), Box<dyn Error>>
{
    let (certificate, private_key, _correct_pin) = ephemeral_identity();
    // Same trust anchor as the presented certificate; ONLY the pin is
    // replaced with a different digest, so ordinary PKI cannot be the
    // failing check.
    let wrong_pin = PinnedIdentity::from_digest([0xa5; 32]);
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let mut connections = server.take_connections();

    let cancel = CancelHandle::new();
    let outcome = ClientSession::connect(
        target(server.addr),
        certificate,
        wrong_pin,
        hello_envelope("pin-hello")?,
        component_limits(1),
        &cancel,
    )
    .await;

    assert!(matches!(
        outcome,
        Err(ClientSessionError::Connect(DeadlineError::Peer {
            operation: OperationKind::Connect,
            error: TransportError::Connection(_)
        }))
    ));
    assert!(
        connections.try_recv().is_err(),
        "no established fixture connection was observed at this check under the pin failure"
    );
    drop(connections);
    drop(server);
    Ok(())
}

/// A typed Hello rejection survives the deadline boundary as a
/// handshake-stage failure: the peer's `ProtocolFailure` is preserved, not
/// fabricated locally and not converted into a local error class. The
/// server replies on the SAME handshake stream and retains its
/// connection until the client has consumed the rejection.
#[tokio::test]
async fn typed_hello_rejection_survives_the_deadline_boundary() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let cancel = CancelHandle::new();
    let client = ClientSession::connect(
        target(server.addr),
        certificate,
        pin,
        hello_envelope("rejected-hello")?,
        component_limits(1),
        &cancel,
    );

    let server_side = async {
        let mut connections = server.take_connections();
        let connection = next_connection(&mut connections).await?;
        let (mut send, mut receive) = tokio::time::timeout(TEST_DEADLINE, connection.accept_bi())
            .await
            .map_err(|_| "rejection stream timed out")??;
        let _hello =
            tokio::time::timeout(TEST_DEADLINE, transport::receive_client_hello(&mut receive))
                .await
                .map_err(|_| "rejected hello read timed out")??;
        tokio::time::timeout(
            TEST_DEADLINE,
            transport::send_envelope(&mut send, &uncorrelated_failure()?),
        )
        .await
        .map_err(|_| "rejection send timed out")??;
        send.finish()?;
        // The connection outlives this block so the client consumes the
        // rejection against live peer handles.
        Ok::<_, Box<dyn Error>>(connection)
    };

    let (outcome, server_result) = tokio::join!(client, server_side);
    let retained_connection = server_result?;
    let Err(ClientSessionError::Handshake(DeadlineError::Peer {
        operation: OperationKind::Handshake,
        error: HandshakeStageError::Handshake(HandshakeError::Rejected { failure }),
    })) = outcome
    else {
        panic!("the typed peer rejection must surface through the handshake stage");
    };
    assert_eq!(failure.code, ErrorCode::InvalidInput);
    assert!(failure.request_id.is_none());
    drop(retained_connection);
    drop(server);
    Ok(())
}

/// The peer closing the WHOLE connection after reading Hello and before
/// any Welcome is a typed LOCAL wire failure of the handshake stage —
/// not a fabricated peer `ProtocolFailure` and not a success. The fixture
/// positively witnesses that its Hello arrived and that ITS fixed
/// auth-close code is the cause nested in the client's typed error.
/// Handwritten component fixture only; production Forge integration is
/// separately approved.
#[tokio::test]
async fn peer_auth_close_before_welcome_is_a_typed_local_handshake_failure()
-> Result<(), Box<dyn Error>> {
    const AUTH_CLOSE_CODE: u32 = 0x117;
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let (hello_read_tx, hello_read_rx) = tokio::sync::oneshot::channel();
    let connections = server.take_connections();
    let server_task = tokio::spawn(async move {
        let mut connections = connections;
        let action = async {
            let connection = tokio::time::timeout(TEST_DEADLINE, connections.recv())
                .await
                .map_err(|_| "auth-close connection timed out")?
                .ok_or("auth-close server stopped accepting")?;
            let (_send, mut receive) = tokio::time::timeout(TEST_DEADLINE, connection.accept_bi())
                .await
                .map_err(|_| "auth-close stream timed out")??;
            let _hello =
                tokio::time::timeout(TEST_DEADLINE, transport::receive_client_hello(&mut receive))
                    .await
                    .map_err(|_| "auth-close hello read timed out")??;
            // Positive proof the Hello reached this peer before the
            // close was raised.
            hello_read_tx
                .send(())
                .map_err(|()| "auth-close witness channel closed")?;
            connection.close(
                VarInt::from_u32(AUTH_CLOSE_CODE),
                b"component pre-welcome auth close",
            );
            Ok::<_, Box<dyn Error>>(())
        };
        action.await.is_ok()
    });

    let cancel = CancelHandle::new();
    let outcome = ClientSession::connect(
        target(server.addr),
        certificate,
        pin,
        hello_envelope("auth-close-hello")?,
        component_limits(1),
        &cancel,
    )
    .await;

    // Positive reachability evidence BEFORE asserting the failure shape.
    tokio::time::timeout(TEST_DEADLINE, hello_read_rx)
        .await
        .map_err(|_| "auth-close witness watchdog fired")?
        .map_err(|_recv_error| "the auth-close fixture reports Hello arrival")?;

    // Nested typed cause chain: the local handshake failed at the frame
    // reader because the QUIC connection closed with the fixture's
    // application close code.
    let Err(ClientSessionError::Handshake(stage_deadline)) = outcome else {
        panic!("a peer connection close before Welcome must fail the handshake locally");
    };
    let DeadlineError::Peer {
        operation: OperationKind::Handshake,
        error,
    } = stage_deadline
    else {
        panic!("the handshake stage must report its own typed failure");
    };
    let HandshakeStageError::Handshake(HandshakeError::Receive(receive_error)) = error else {
        panic!("expected a receive-stage wire failure");
    };
    let EnvelopeReceiveError::Frame(FrameError::Read(read_error)) = receive_error else {
        panic!("expected a framing-layer QUIC read failure");
    };
    let ReadError::ConnectionLost(connection_error) = read_error else {
        panic!("expected a connection-level close behind the read failure");
    };
    let ConnectionError::ApplicationClosed(close) = connection_error else {
        panic!("expected the peer's application close as the close cause");
    };
    assert_eq!(
        u64::from(close.error_code),
        u64::from(AUTH_CLOSE_CODE),
        "the fixture's fixed auth-close code must be the cause"
    );

    let fixture_succeeded = tokio::time::timeout(TEST_DEADLINE, server_task)
        .await
        .expect("fixture task finishes")
        .expect("fixture task does not panic");
    assert!(fixture_succeeded, "the auth-close fixture must run cleanly");
    drop(server);
    Ok(())
}

/// A connect future created after cancellation resolves when it is first
/// polled, reporting cancellation WITHOUT ever polling its
/// deadline-wrapped I/O stage: no dial occurs, so the fixture never sees
/// a connection and the dropped Hello capability is the only owned
/// material.
#[tokio::test]
async fn cancelled_connect_never_creates_transport() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let mut connections = server.take_connections();

    let cancel = CancelHandle::new();
    cancel.cancel();
    let outcome = ClientSession::connect(
        target(server.addr),
        certificate,
        pin,
        hello_envelope("cancelled-hello")?,
        component_limits(1),
        &cancel,
    )
    .await;

    assert!(matches!(
        outcome,
        Err(ClientSessionError::Connect(DeadlineError::Cancelled {
            operation: OperationKind::Connect
        }))
    ));
    assert!(
        connections.try_recv().is_err(),
        "the fixture channel stayed empty at this check; pre-start cancellation returns before its stage is polled"
    );
    drop(connections);
    drop(server);
    Ok(())
}

/// A zero connect limit follows the existing typed deadline semantics:
/// the deadline-wrapped stage expires BEFORE its inner future is first
/// polled, so nothing dials and no fallback limit is invented. This is
/// distinct from an outer connect future being dropped without any poll.
#[tokio::test]
async fn zero_limit_connect_times_out_before_polling() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let mut connections = server.take_connections();

    let cancel = CancelHandle::new();
    let mut limits = component_limits(1);
    limits.connect = Duration::ZERO;
    let outcome = ClientSession::connect(
        target(server.addr),
        certificate,
        pin,
        hello_envelope("zero-limit-hello")?,
        limits,
        &cancel,
    )
    .await;

    assert!(
        matches!(
            outcome,
            Err(ClientSessionError::Connect(DeadlineError::Timeout {
                operation: OperationKind::Connect,
                limit
            })) if limit == Duration::ZERO
        ),
        "the zero limit must expire the whole stage before polling"
    );
    assert!(
        connections.try_recv().is_err(),
        "the fixture channel stayed empty at this check; the zero limit expires before the stage is polled"
    );
    drop(connections);
    drop(server);
    Ok(())
}

/// Cancelling a request BEFORE its deadline-wrapped exchange stage is
/// ever polled keeps the wire silent at the peer: the acceptance below
/// resolves only through the leaf's typed application close — an
/// observation of no accepted stream at that check, combined with the
/// production path where the cancelled operation returns before its
/// exchange stage is ever polled.
#[tokio::test]
async fn pre_start_cancelled_request_keeps_the_wire_silent() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let server_task = tokio::spawn(handshake_then_witness_closed(server.take_connections()));
    let joined = tokio::time::timeout(TEST_DEADLINE, server_task);

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server.addr),
            certificate,
            pin,
            hello_envelope("silent-hello")?,
            component_limits(2),
            &cancel,
        )
        .await?;

        cancel.cancel();
        let outcome = session
            .request(request_envelope("silent-request")?, &cancel)
            .await;
        assert!(
            matches!(
                outcome,
                Err(ClientRequestError::Exchange(DeadlineError::Cancelled {
                    operation: OperationKind::Receive
                }))
            ),
            "pre-stage cancellation must surface as the typed cancelled exchange"
        );
        Ok::<_, Box<dyn Error>>(())
    };

    // The bounded join runs concurrently with the client flow, so the
    // task is always awaited before any assertion can fail the test.
    let (client_result, joined) = tokio::join!(client, joined);
    let closed_without_streams = joined
        .expect("fixture task finishes")
        .expect("fixture task does not panic");
    assert!(
        closed_without_streams,
        "peer-observed typed closure without any request stream is the causal witness"
    );
    client_result?;
    drop(server);
    Ok(())
}

/// Shared spawned-task scenario for mid-flight and drop cases: the
/// fixture performs the handshake, reads exactly ONE FULL REQUEST — the
/// causal pending-I/O witness — and withholds its reply until released.
/// After the release it awaits the POSITIVE peer-side connection-closed
/// event, bounded, and reports whether its typed application-close cause
/// carries the leaf's abandon code. Arbitrary write failures are
/// deliberately NOT consulted: a peer may enqueue output before
/// processing the close packet.
async fn withhold_and_witness_close(
    mut connections: tokio::sync::mpsc::Receiver<Connection>,
    witnessed: tokio::sync::mpsc::Sender<()>,
    mut release: tokio::sync::mpsc::Receiver<()>,
    peer_closed: tokio::sync::oneshot::Sender<bool>,
) {
    let Ok(Some(connection)) = tokio::time::timeout(TEST_DEADLINE, connections.recv()).await else {
        let _reported = peer_closed.send(false);
        return;
    };
    let Ok(welcome) = welcome_envelope() else {
        let _reported = peer_closed.send(false);
        return;
    };
    let Ok((_hello, _send, _receive)) = drive_handshake(&connection, &welcome).await else {
        let _reported = peer_closed.send(false);
        return;
    };
    let Ok(Ok((_send, mut receive))) =
        tokio::time::timeout(TEST_DEADLINE, connection.accept_bi()).await
    else {
        let _reported = peer_closed.send(false);
        return;
    };
    let Ok(Ok(_request)) =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut receive)).await
    else {
        let _reported = peer_closed.send(false);
        return;
    };
    // Causal pending-I/O witness: the FULL request reached the peer.
    if tokio::time::timeout(TEST_DEADLINE, witnessed.send(()))
        .await
        .is_err()
    {
        let _reported = peer_closed.send(false);
        return;
    }
    // Hold the reply until the test has made its local move, so the
    // closure cause below is attributable to that move alone.
    if !matches!(
        tokio::time::timeout(TEST_DEADLINE, release.recv()).await,
        Ok(Some(()))
    ) {
        let _reported = peer_closed.send(false);
        return;
    }
    let cause = tokio::time::timeout(TEST_DEADLINE, connection.closed()).await;
    let witnessed_close = match cause {
        Ok(ConnectionError::ApplicationClosed(close)) => {
            u64::from(close.error_code) == LEAF_ABANDON_CODE
        }
        _ => false,
    };
    let _reported = peer_closed.send(witnessed_close);
}

/// Cancelling a genuinely mid-flight request — proven pending by the
/// peer having READ the full request bytes while the reply was withheld
/// — closes the connection, and the peer positively witnesses the typed
/// application close carrying the leaf's abandon code. The outbound
/// request had been fully delivered, so this proves abandonment of a
/// causally pending RECEIVE plus connection closure.
#[tokio::test]
async fn mid_flight_cancellation_closes_connection_during_pending_receive()
-> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let (witnessed_tx, mut witnessed_rx) = tokio::sync::mpsc::channel(1);
    let (release_tx, release_rx) = tokio::sync::mpsc::channel(1);
    let (peer_closed_tx, peer_closed_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(withhold_and_witness_close(
        server.take_connections(),
        witnessed_tx,
        release_rx,
        peer_closed_tx,
    ));

    let cancel = CancelHandle::new();
    let (session, _) = ClientSession::connect(
        target(server.addr),
        certificate,
        pin,
        hello_envelope("mid-flight-hello")?,
        component_limits(2),
        &cancel,
    )
    .await?;

    let mut request = Box::pin(session.request(request_envelope("mid-cancel")?, &cancel));
    tokio::select! {
        biased;
        _ = &mut request => {
            panic!("the request settled while the peer withheld its reply");
        }
        event = tokio::time::timeout(TEST_DEADLINE, witnessed_rx.recv()) => {
            assert_eq!(
                event.expect("witness wait is bounded"),
                Some(()),
                "the request-read witness must be a real event"
            );
        }
    }

    cancel.cancel();
    let outcome = request.await;
    assert!(matches!(
        outcome,
        Err(ClientRequestError::Exchange(DeadlineError::Cancelled {
            operation: OperationKind::Receive
        }))
    ));

    release_tx.send(()).await.expect("release channel alive");
    let peer_saw_close = tokio::time::timeout(TEST_DEADLINE, peer_closed_rx)
        .await
        .expect("closure witness is bounded")
        .expect("fixture reports the closure witness");
    assert!(
        peer_saw_close,
        "the peer must observe the typed application close with the abandon code"
    );
    tokio::time::timeout(TEST_DEADLINE, server_task)
        .await
        .expect("fixture task finishes")
        .expect("fixture task does not panic");
    drop(server);
    Ok(())
}

/// A genuinely pending request whose whole-stage limit expires is a
/// typed local timeout with the same causal chain: the peer had already
/// read the full request, so the expiry happened while awaiting the
/// withheld reply, and the peer afterwards witnesses the typed
/// application close. The margin is wide and the success branch is
/// structurally impossible because the scripted peer never answers
/// before it is told to.
#[tokio::test]
async fn mid_flight_timeout_witnesses_pending_io() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let (witnessed_tx, mut witnessed_rx) = tokio::sync::mpsc::channel(1);
    let (release_tx, release_rx) = tokio::sync::mpsc::channel(1);
    let (peer_closed_tx, peer_closed_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(withhold_and_witness_close(
        server.take_connections(),
        witnessed_tx,
        release_rx,
        peer_closed_tx,
    ));

    let cancel = CancelHandle::new();
    let mut limits = component_limits(2);
    limits.request = Duration::from_secs(2);
    let (session, _) = ClientSession::connect(
        target(server.addr),
        certificate,
        pin,
        hello_envelope("timeout-hello")?,
        limits,
        &cancel,
    )
    .await?;

    let mut request = Box::pin(session.request(request_envelope("mid-timeout")?, &cancel));
    tokio::select! {
        biased;
        _ = &mut request => {
            panic!("the request settled while the peer withheld its reply");
        }
        event = tokio::time::timeout(TEST_DEADLINE, witnessed_rx.recv()) => {
            assert_eq!(
                event.expect("witness wait is bounded"),
                Some(()),
                "the request-read witness must be a real event"
            );
        }
    }

    let outcome = request.await;
    assert!(matches!(
        outcome,
        Err(ClientRequestError::Exchange(DeadlineError::Timeout {
            operation: OperationKind::Receive,
            ..
        }))
    ));

    release_tx.send(()).await.expect("release channel alive");
    let peer_saw_close = tokio::time::timeout(TEST_DEADLINE, peer_closed_rx)
        .await
        .expect("closure witness is bounded")
        .expect("fixture reports the closure witness");
    assert!(
        peer_saw_close,
        "the peer must observe the typed application close with the abandon code"
    );
    tokio::time::timeout(TEST_DEADLINE, server_task)
        .await
        .expect("fixture task finishes")
        .expect("fixture task does not panic");
    drop(server);
    Ok(())
}

/// Dropping a request future that consumed a ready owner but was never
/// polled closes the acquired connection; the peer witnesses the typed
/// application close — positively checked by the leaf's abandon code —
/// as its pending stream acceptance resolves, observing no accepted
/// stream at that check (the production stage future is dropped before
/// its first poll).
#[tokio::test]
async fn dropping_unpolled_request_future_closes_the_connection() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let server_task = tokio::spawn(handshake_then_witness_closed(server.take_connections()));
    let joined = tokio::time::timeout(TEST_DEADLINE, server_task);

    let cancel = CancelHandle::new();
    let client = async {
        let (session, _) = ClientSession::connect(
            target(server.addr),
            certificate,
            pin,
            hello_envelope("unpolled-hello")?,
            component_limits(2),
            &cancel,
        )
        .await?;

        {
            let _unpolled = Box::pin(session.request(request_envelope("never-polled")?, &cancel));
            // Dropped here without a single poll: the READY owner inside
            // is consumed and synchronously closed.
        }
        Ok::<_, Box<dyn Error>>(())
    };

    // The bounded join runs concurrently with the client flow, so the
    // task is always awaited before any assertion can fail the test.
    let (client_result, joined) = tokio::join!(client, joined);
    let closed_without_streams = joined
        .expect("fixture task finishes")
        .expect("fixture task does not panic");
    assert!(
        closed_without_streams,
        "peer-observed typed closure is the causal witness"
    );
    client_result?;
    drop(server);
    Ok(())
}

/// Dropping a cold connect future before its first poll creates no
/// endpoint and no connection, so there is nothing for the fixture to
/// observe — the only owned material lost is the Hello capability, which
/// is dropped and zeroized inside the future. The no-dial claim is
/// witnessed by the fixture channel staying empty.
#[tokio::test]
async fn cold_unpolled_connect_only_drops_its_hello() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));
    let mut connections = server.take_connections();

    let cancel = CancelHandle::new();
    {
        let _cold = Box::pin(ClientSession::connect(
            target(server.addr),
            certificate,
            pin,
            hello_envelope("cold-hello")?,
            component_limits(1),
            &cancel,
        ));
        // Dropped before the first poll: no dial, no endpoint, no
        // connection — only the Hello capability existed to lose.
    }

    assert!(
        connections.try_recv().is_err(),
        "the fixture channel stayed empty at this check after the unpolled drop; the cold connect returns before its stage is polled"
    );
    drop(connections);
    drop(server);
    Ok(())
}

/// Dropping a request future while it is genuinely pending stops
/// inbound delivery and closes the connection with the reply still
/// pending: the outbound request had ALREADY been fully delivered (the
/// witness proves the peer read it), so this case honestly proves a
/// causally pending RECEIVE abandoned through the typed connection
/// closure — not a reset of unfinished outbound data.
#[tokio::test]
async fn dropping_pending_request_closes_connection_with_reply_pending()
-> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let (witnessed_tx, mut witnessed_rx) = tokio::sync::mpsc::channel(1);
    let (release_tx, release_rx) = tokio::sync::mpsc::channel(1);
    let (peer_closed_tx, peer_closed_rx) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(withhold_and_witness_close(
        server.take_connections(),
        witnessed_tx,
        release_rx,
        peer_closed_tx,
    ));

    let cancel = CancelHandle::new();
    let (session, _) = ClientSession::connect(
        target(server.addr),
        certificate,
        pin,
        hello_envelope("pending-drop-hello")?,
        component_limits(2),
        &cancel,
    )
    .await?;

    let mut request = Box::pin(session.request(request_envelope("drop-pending")?, &cancel));
    tokio::select! {
        biased;
        _ = &mut request => {
            panic!("the request settled while the peer withheld its reply");
        }
        event = tokio::time::timeout(TEST_DEADLINE, witnessed_rx.recv()) => {
            assert_eq!(
                event.expect("witness wait is bounded"),
                Some(()),
                "the request-read witness must be a real event before the drop"
            );
        }
    }

    // Drop the PENDING future: inbound stops and the owned connection
    // closes with the reply still pending.
    drop(request);

    release_tx.send(()).await.expect("release channel alive");
    let peer_saw_close = tokio::time::timeout(TEST_DEADLINE, peer_closed_rx)
        .await
        .expect("closure witness is bounded")
        .expect("fixture reports the closure witness");
    assert!(
        peer_saw_close,
        "the peer must observe the typed application close with the abandon code"
    );
    tokio::time::timeout(TEST_DEADLINE, server_task)
        .await
        .expect("fixture task finishes")
        .expect("fixture task does not panic");
    drop(server);
    Ok(())
}

/// Abandoning a connect future after the Hello reached the peer closes
/// the mid-flight resources: the fixture reads the Hello, waits for the
/// test's drop, and then positively witnesses the connection close with
/// the leaf's abandon code. A cold unpolled connect has no such
/// resources; see the dedicated cold-connect expectations.
#[tokio::test]
async fn handshake_abandonment_closes_midflight_resources() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let (hello_read_tx, mut hello_read_rx) = tokio::sync::mpsc::channel(1);
    let (release_tx, mut release_rx) = tokio::sync::mpsc::channel(1);
    let (peer_closed_tx, peer_closed_rx) = tokio::sync::oneshot::channel();
    let connections = server.take_connections();
    let server_task = tokio::spawn(async move {
        let mut connections = connections;
        let Ok(Some(connection)) = tokio::time::timeout(TEST_DEADLINE, connections.recv()).await
        else {
            let _reported = peer_closed_tx.send(false);
            return;
        };
        let Ok(Ok((_send, mut receive))) =
            tokio::time::timeout(TEST_DEADLINE, connection.accept_bi()).await
        else {
            let _reported = peer_closed_tx.send(false);
            return;
        };
        let Ok(Ok(_hello)) =
            tokio::time::timeout(TEST_DEADLINE, transport::receive_client_hello(&mut receive))
                .await
        else {
            let _reported = peer_closed_tx.send(false);
            return;
        };
        // Causal reachability witness: the Hello fully arrived.
        if tokio::time::timeout(TEST_DEADLINE, hello_read_tx.send(()))
            .await
            .is_err()
        {
            let _reported = peer_closed_tx.send(false);
            return;
        }
        // Only proceed once the test has dropped its connect future, so
        // the closure below is attributable to that drop alone.
        if !matches!(
            tokio::time::timeout(TEST_DEADLINE, release_rx.recv()).await,
            Ok(Some(()))
        ) {
            let _reported = peer_closed_tx.send(false);
            return;
        }
        let cause = tokio::time::timeout(TEST_DEADLINE, connection.closed()).await;
        let witnessed_close = match cause {
            Ok(ConnectionError::ApplicationClosed(close)) => {
                u64::from(close.error_code) == LEAF_ABANDON_CODE
            }
            _ => false,
        };
        let _reported = peer_closed_tx.send(witnessed_close);
    });

    let cancel = CancelHandle::new();
    let mut connecting = Box::pin(ClientSession::connect(
        target(server.addr),
        certificate,
        pin,
        hello_envelope("abandoned-hello")?,
        component_limits(1),
        &cancel,
    ));
    tokio::select! {
        biased;
        _ = &mut connecting => {
            panic!("the connect settled although the peer withheld its Welcome");
        }
        event = tokio::time::timeout(TEST_DEADLINE, hello_read_rx.recv()) => {
            assert_eq!(
                event.expect("witness wait is bounded"),
                Some(()),
                "the hello-read witness must be a real event"
            );
        }
    }

    drop(connecting);
    release_tx.send(()).await.expect("release channel alive");
    let peer_saw_close = tokio::time::timeout(TEST_DEADLINE, peer_closed_rx)
        .await
        .expect("closure witness is bounded")
        .expect("fixture reports the closure witness");
    assert!(
        peer_saw_close,
        "the peer must observe the typed application close with the abandon code"
    );
    tokio::time::timeout(TEST_DEADLINE, server_task)
        .await
        .expect("fixture task finishes")
        .expect("fixture task does not panic");
    drop(server);
    Ok(())
}

/// A normal awaited shutdown drains the privately owned endpoint within
/// its finite limit, and the peer positively witnesses the connection
/// closing with the leaf's SHUTDOWN application code — distinguishing
/// the awaited path from abandonment closures.
#[tokio::test]
async fn finite_shutdown_drains_and_notifies_peer() -> Result<(), Box<dyn Error>> {
    let (certificate, private_key, pin) = ephemeral_identity();
    let mut server = TestServer::start(fixture_server_config(certificate.clone(), private_key));

    let connections = server.take_connections();
    let server_task = tokio::spawn(async move {
        let mut connections = connections;
        let Ok(Some(connection)) = tokio::time::timeout(TEST_DEADLINE, connections.recv()).await
        else {
            return false;
        };
        let Ok(welcome) = welcome_envelope() else {
            return false;
        };
        if drive_handshake(&connection, &welcome).await.is_err() {
            return false;
        }
        // `closed` resolves with the closure's typed cause: the positive
        // peer-side witness of the drained shutdown, carrying the leaf's
        // awaited-shutdown application code.
        let cause = tokio::time::timeout(TEST_DEADLINE, connection.closed()).await;
        matches!(
            cause,
            Ok(ConnectionError::ApplicationClosed(close))
                if u64::from(close.error_code) == LEAF_SHUTDOWN_CODE
        )
    });

    let cancel = CancelHandle::new();
    let (session, _) = ClientSession::connect(
        target(server.addr),
        certificate,
        pin,
        hello_envelope("shutdown-hello")?,
        component_limits(2),
        &cancel,
    )
    .await?;

    session.shutdown(&cancel).await?;
    let peer_saw_shutdown_close = tokio::time::timeout(TEST_DEADLINE, server_task)
        .await
        .expect("fixture task finishes")
        .expect("fixture task does not panic");
    assert!(
        peer_saw_shutdown_close,
        "peer-observed application close with the shutdown code is the drain witness"
    );
    drop(server);
    Ok(())
}
