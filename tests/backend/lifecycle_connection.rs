//! Composed configured lifecycle tests for the private connection adapter.
//!
//! These tests are path-linked from `connection.rs`, so they can install the
//! crate-private activity gate without enlarging the shipping API. The wire
//! half still uses real loopback QUIC, the migrated application repository,
//! the credential authority, and the connection owner.

use std::error::Error;
use std::fs;
use std::mem::size_of;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread::JoinHandle;
use std::time::Duration;

use artisan_database::SqliteConfig;
use artisan_domain::{RequestId, UnixMillis};
use artisan_protocol::{
    APPLICATION_PROTOCOL_VERSION, ClientRequest, ConnectionId, ErrorCode, FrameId, Hello,
    HelloCredential, LifecycleRequest, LifecycleResponse, LifecycleState, LifecycleStopDisposition,
    LocalCapability, ProtocolVersion, ReconnectCapability, ResponsePayload, ServerResponse,
    VersionOffer, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, DeadlineError, EnvelopeReceiveError, FrameError, OperationKind, PinnedIdentity,
    ServerDispatchError,
};
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig, VarInt};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

use super::{
    ConnectionLimits, ForgeConnection, RequestStageError, ServerFrameStamp, WelcomeMetadata,
};
use crate::credential_authority::CredentialAuthority;
use crate::lifecycle_control::{
    ActivityGate, ActivityGateError, ActivitySnapshot, ActivityStopReservation,
    LifecycleController, LifecycleDispatch, StopAdmission,
};
use crate::{ForgeApp, ForgeConfig, RequestHandler};

/// Bounded watchdog for every loopback operation and teardown.
pub(crate) const TEST_DEADLINE: Duration = Duration::from_secs(5);

const INITIAL_CAPABILITY: [u8; 32] = [0xb7; 32];
const CLIENT_HELLO_FRAME: &str = "lifecycle-client-hello";
const WELCOME_CONNECTION_ID: &str = "lifecycle-connection";
const WELCOME_FRAME: &str = "lifecycle-welcome";

/// Deterministic activity boundary used by both composed fixtures.
pub(crate) struct TestActivityGate {
    active_work_count: AtomicU32,
    begin_stop_calls: AtomicUsize,
    last_require_idle: AtomicU8,
    committed: Arc<AtomicUsize>,
    rolled_back: Arc<AtomicUsize>,
    reserved: Arc<AtomicBool>,
}

impl TestActivityGate {
    #[must_use]
    pub(crate) fn new(active_work_count: u32) -> Arc<Self> {
        Arc::new(Self {
            active_work_count: AtomicU32::new(active_work_count),
            begin_stop_calls: AtomicUsize::new(0),
            last_require_idle: AtomicU8::new(2),
            committed: Arc::new(AtomicUsize::new(0)),
            rolled_back: Arc::new(AtomicUsize::new(0)),
            reserved: Arc::new(AtomicBool::new(false)),
        })
    }

    pub(crate) fn begin_stop_calls(&self) -> usize {
        self.begin_stop_calls.load(Ordering::Acquire)
    }

    pub(crate) fn committed(&self) -> usize {
        self.committed.load(Ordering::Acquire)
    }

    pub(crate) fn rolled_back(&self) -> usize {
        self.rolled_back.load(Ordering::Acquire)
    }

    pub(crate) fn last_require_idle(&self) -> bool {
        match self.last_require_idle.load(Ordering::Acquire) {
            0 => false,
            1 => true,
            _ => panic!("the test gate did not receive a stop request"),
        }
    }

    pub(crate) fn is_reserved(&self) -> bool {
        self.reserved.load(Ordering::Acquire)
    }
}

impl ActivityGate for TestActivityGate {
    fn snapshot(&self) -> Result<ActivitySnapshot, ActivityGateError> {
        Ok(ActivitySnapshot::new(
            self.active_work_count.load(Ordering::Acquire),
        ))
    }

    fn begin_stop(&self, require_idle: bool) -> Result<StopAdmission, ActivityGateError> {
        self.begin_stop_calls.fetch_add(1, Ordering::AcqRel);
        self.last_require_idle
            .store(if require_idle { 1 } else { 0 }, Ordering::Release);
        let active_work_count = self.active_work_count.load(Ordering::Acquire);
        if require_idle && active_work_count != 0 {
            return Ok(StopAdmission::Busy { active_work_count });
        }

        assert!(!self.reserved.swap(true, Ordering::AcqRel));
        Ok(StopAdmission::Accepted {
            reservation: Box::new(TestActivityReservation {
                committed: Arc::clone(&self.committed),
                rolled_back: Arc::clone(&self.rolled_back),
                reserved: Arc::clone(&self.reserved),
                committed_already: false,
            }),
        })
    }
}

struct TestActivityReservation {
    committed: Arc<AtomicUsize>,
    rolled_back: Arc<AtomicUsize>,
    reserved: Arc<AtomicBool>,
    committed_already: bool,
}

impl ActivityStopReservation for TestActivityReservation {
    fn commit(mut self: Box<Self>) {
        self.committed_already = true;
        self.committed.fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for TestActivityReservation {
    fn drop(&mut self) {
        if !self.committed_already {
            self.rolled_back.fetch_add(1, Ordering::AcqRel);
            self.reserved.store(false, Ordering::Release);
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal real QUIC fixture shared with the listener composition test
// ---------------------------------------------------------------------------

pub(crate) struct TestPki {
    certificate: CertificateDer<'static>,
    private_key: PrivatePkcs8KeyDer<'static>,
    pinned_identity: PinnedIdentity,
}

pub(crate) fn test_pki() -> TestPki {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("test PKI");
    let certificate = certified_key.cert.der().clone();
    TestPki {
        pinned_identity: PinnedIdentity::from_certificate(&certificate),
        private_key: PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der()),
        certificate,
    }
}

pub(crate) fn server_config(pki: &TestPki) -> ServerConfig {
    artisan_transport::server_config(vec![pki.certificate.clone()], pki.private_key.clone_key())
        .expect("server configuration")
}

pub(crate) fn client_config(pki: &TestPki) -> ClientConfig {
    artisan_transport::client_config(pki.certificate.clone(), pki.pinned_identity)
        .expect("client configuration")
}

struct Loopback {
    server_addr: SocketAddr,
    client: Endpoint,
    server_connections: tokio::sync::mpsc::Receiver<Connection>,
    stop_server: Option<tokio::sync::oneshot::Sender<()>>,
    server_thread: Option<JoinHandle<()>>,
}

impl Loopback {
    fn join_server_thread(&mut self) {
        if let Some(stop) = self.stop_server.take() {
            let _signalled = stop.send(());
        }
        if let Some(thread) = self.server_thread.take() {
            thread.join().expect("loopback server thread finishes");
        }
    }

    async fn drain(mut self) {
        self.join_server_thread();
        artisan_transport::shutdown(
            &self.client,
            VarInt::from_u32(0),
            b"lifecycle connection test complete",
            TEST_DEADLINE,
        )
        .await
        .expect("client endpoint drains");
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.join_server_thread();
    }
}

fn spawn_loopback_pair(server: ServerConfig, client_config: ClientConfig) -> Loopback {
    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    let (connections_tx, connections_rx) = tokio::sync::mpsc::channel(1);
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();

    let server_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("loopback server runtime");
        runtime.block_on(async move {
            let server = artisan_transport::bind_loopback_server(server).expect("server bind");
            addr_tx
                .send(server.local_addr().expect("server address"))
                .expect("test waits for server address");

            loop {
                let incoming = tokio::select! {
                    _ = &mut stop_rx => break,
                    incoming = server.accept() => incoming,
                };
                let Some(incoming) = incoming else {
                    break;
                };
                let established = tokio::time::timeout(TEST_DEADLINE, incoming)
                    .await
                    .expect("server handshake completes")
                    .expect("server connection establishes");
                if connections_tx.send(established).await.is_err() {
                    break;
                }
            }

            artisan_transport::shutdown(
                &server,
                VarInt::from_u32(0),
                b"loopback complete",
                TEST_DEADLINE,
            )
            .await
            .expect("server endpoint drains");
        });
    });

    let server_addr = match addr_rx.recv_timeout(TEST_DEADLINE) {
        Ok(address) => address,
        Err(RecvTimeoutError::Timeout) => panic!("server did not bind"),
        Err(RecvTimeoutError::Disconnected) => panic!("server thread died before binding"),
    };
    let client = artisan_transport::bind_loopback_client(client_config).expect("client bind");
    Loopback {
        server_addr,
        client,
        server_connections: connections_rx,
        stop_server: Some(stop_tx),
        server_thread: Some(server_thread),
    }
}

fn spawn_loopback() -> Loopback {
    let pki = test_pki();
    spawn_loopback_pair(server_config(&pki), client_config(&pki))
}

async fn connect_client(loopback: &Loopback) -> Connection {
    let connecting = loopback
        .client
        .connect(
            loopback.server_addr,
            artisan_transport::LOOPBACK_SERVER_NAME,
        )
        .expect("connect request accepted");
    tokio::time::timeout(TEST_DEADLINE, connecting)
        .await
        .expect("client connection completes")
        .expect("client connection establishes")
}

async fn next_server_connection(loopback: &mut Loopback) -> Connection {
    tokio::time::timeout(TEST_DEADLINE, loopback.server_connections.recv())
        .await
        .expect("server connection arrives")
        .expect("server remains accepting")
}

// ---------------------------------------------------------------------------
// Application and protocol fixtures
// ---------------------------------------------------------------------------

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TemporaryDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "artisan-forge-lifecycle-connection-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("temporary database directory");
        Self {
            database: directory.join("forge.sqlite3"),
            directory,
        }
    }

    fn path(&self) -> &Path {
        &self.database
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.directory);
    }
}

async fn opened_app(label: &str) -> (TemporaryDatabase, ForgeApp) {
    let temporary = TemporaryDatabase::new(label);
    let app = ForgeApp::start(ForgeConfig::new(
        SqliteConfig::file(temporary.path()).sqlx_logging(false),
    ))
    .await
    .expect("migrated test application starts");
    (temporary, app)
}

fn bootstrap_authority() -> CredentialAuthority {
    CredentialAuthority::new(LocalCapability::from_bytes(INITIAL_CAPABILITY))
}

pub(crate) fn initial_capability() -> LocalCapability {
    LocalCapability::from_bytes(INITIAL_CAPABILITY)
}

pub(crate) fn initial_credential() -> HelloCredential {
    HelloCredential::Initial(initial_capability())
}

pub(crate) fn reconnect_credential(capability: ReconnectCapability) -> HelloCredential {
    HelloCredential::Reconnect(capability)
}

pub(crate) fn hello_envelope(
    credential: HelloCredential,
    supports_lifecycle_control: bool,
) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(CLIENT_HELLO_FRAME).expect("hello frame id"),
        sent_at: UnixMillis::from_millis(10),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![APPLICATION_PROTOCOL_VERSION])
                .expect("version offer"),
            credential,
            supports_lifecycle_control,
        }),
    }
}

pub(crate) fn lifecycle_request(frame: &str, request: LifecycleRequest) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame).expect("lifecycle frame id"),
        sent_at: UnixMillis::from_millis(3_000),
        body: WireEnvelopeBody::Request(ClientRequest::Lifecycle(request)),
    }
}

fn welcome_metadata() -> WelcomeMetadata {
    WelcomeMetadata {
        connection_id: ConnectionId::parse(WELCOME_CONNECTION_ID).expect("connection id"),
        frame: ServerFrameStamp {
            frame_id: FrameId::parse(WELCOME_FRAME).expect("welcome frame id"),
            sent_at: UnixMillis::from_millis(50),
        },
    }
}

fn response_stamp(frame: &str) -> ServerFrameStamp {
    ServerFrameStamp {
        frame_id: FrameId::parse(frame).expect("response frame id"),
        sent_at: UnixMillis::from_millis(60),
    }
}

fn connection_limits() -> ConnectionLimits {
    ConnectionLimits {
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
    }
}

// ---------------------------------------------------------------------------
// Real connection adapter helpers
// ---------------------------------------------------------------------------

struct AuthenticatedClient {
    connection: Connection,
    control_send: SendStream,
    control_recv: RecvStream,
    welcome: artisan_transport::ServerWelcome,
}

#[allow(clippy::too_many_arguments)]
async fn admit<'authority, 'handler, 'cancel, 'lifecycle>(
    loopback: &mut Loopback,
    authority: &'authority mut CredentialAuthority,
    handler: &'handler RequestHandler,
    lifecycle: &'lifecycle LifecycleController,
    cancel: &'cancel CancelHandle,
    credential: HelloCredential,
    supports_lifecycle_control: bool,
) -> Result<
    (
        AuthenticatedClient,
        ForgeConnection<'authority, 'handler, 'cancel, 'lifecycle>,
    ),
    Box<dyn Error>,
> {
    let client_connection = connect_client(loopback).await;
    let server_connection = next_server_connection(loopback).await;
    let client = async {
        let (mut send, mut recv) = client_connection.open_bi().await?;
        let welcome = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::client_handshake(
                &mut send,
                &mut recv,
                hello_envelope(credential, supports_lifecycle_control),
            ),
        )
        .await??;
        Ok::<AuthenticatedClient, Box<dyn Error>>(AuthenticatedClient {
            connection: client_connection,
            control_send: send,
            control_recv: recv,
            welcome,
        })
    };
    let server = ForgeConnection::authenticate(
        server_connection,
        authority,
        handler,
        lifecycle,
        welcome_metadata(),
        connection_limits(),
        cancel,
    );
    let (client, server) = tokio::join!(client, tokio::time::timeout(TEST_DEADLINE, server));
    Ok((client?, server.expect("authentication settles")?))
}

fn take_reconnect(client: AuthenticatedClient) -> ReconnectCapability {
    let AuthenticatedClient {
        connection,
        control_send,
        control_recv,
        welcome,
    } = client;
    drop(control_send);
    drop(control_recv);
    drop(connection);
    welcome.welcome.reconnect_capability
}

async fn exchange(
    client: &AuthenticatedClient,
    request: WireEnvelope,
    stamp: &ServerFrameStamp,
) -> Result<WireEnvelope, Box<dyn Error>> {
    let (mut send, mut recv) = client.connection.open_bi().await?;
    artisan_transport::send_envelope(&mut send, &request).await?;
    let reply = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut recv),
    )
    .await
    .expect("lifecycle reply arrives")?;
    assert_eq!(reply.frame_id, stamp.frame_id);
    assert_eq!(reply.sent_at, stamp.sent_at);
    match &reply.body {
        WireEnvelopeBody::Response(response) => assert_eq!(
            response.request_id,
            RequestId::parse(request.frame_id.as_str()).expect("request correlation"),
        ),
        WireEnvelopeBody::ProtocolError(failure) => assert_eq!(
            failure.request_id,
            Some(RequestId::parse(request.frame_id.as_str()).expect("error correlation")),
        ),
        _ => panic!("lifecycle adapter returned an unexpected wire family"),
    }
    let end = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut recv),
    )
    .await
    .expect("lifecycle response FIN arrives");
    match end {
        Err(EnvelopeReceiveError::Frame(FrameError::Truncated {
            expected,
            received: 0,
        })) if expected == size_of::<u32>() => {}
        Ok(_) => panic!("lifecycle dispatch returned more than one response"),
        Err(_) => panic!("lifecycle response did not end with a clean FIN"),
    }
    drop(send);
    drop(recv);
    Ok(reply)
}

fn lifecycle_payload(reply: WireEnvelope) -> LifecycleResponse {
    let WireEnvelopeBody::Response(ServerResponse {
        payload: ResponsePayload::Lifecycle(response),
        ..
    }) = reply.body
    else {
        panic!("expected a correlated lifecycle response")
    };
    response
}

async fn round_trip<'authority, 'handler, 'cancel, 'lifecycle>(
    client: &AuthenticatedClient,
    request: WireEnvelope,
    stamp: ServerFrameStamp,
    owner: ForgeConnection<'authority, 'handler, 'cancel, 'lifecycle>,
) -> Result<
    (
        WireEnvelope,
        ForgeConnection<'authority, 'handler, 'cancel, 'lifecycle>,
    ),
    Box<dyn Error>,
> {
    let client_stamp = stamp.clone();
    let replied = tokio::time::timeout(TEST_DEADLINE, exchange(client, request, &client_stamp));
    let served = tokio::time::timeout(TEST_DEADLINE, owner.respond_next(stamp));
    let (replied, served) = tokio::join!(replied, served);
    Ok((
        replied.expect("wire exchange settles")?,
        served.expect("connection dispatch settles")?,
    ))
}

fn stop_receipt(response: LifecycleResponse) -> artisan_protocol::LifecycleStopReceipt {
    match response {
        LifecycleResponse::Stop(receipt) => receipt,
        LifecycleResponse::Status(_) => panic!("expected a lifecycle stop receipt"),
    }
}

// ---------------------------------------------------------------------------
// Composed acceptance cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn configured_negotiation_requires_both_hello_offer_and_installed_gate() {
    let (temporary, app) = opened_app("negotiation").await;
    let mut loopback = spawn_loopback();
    let gate = TestActivityGate::new(0);
    let lifecycle = LifecycleController::with_activity_gate(gate.clone());
    let mut authority = bootstrap_authority();
    let handler = RequestHandler::new(app.repository().clone());
    let cancel = CancelHandle::new();
    let offer = false;

    let (without_offer, owner) = admit(
        &mut loopback,
        &mut authority,
        &handler,
        &lifecycle,
        &cancel,
        initial_credential(),
        offer,
    )
    .await
    .expect("false offer admission succeeds");
    assert_eq!(
        without_offer.welcome.welcome.lifecycle_control_supported,
        offer && lifecycle.implementation_available()
    );
    drop(owner);
    let reconnect = take_reconnect(without_offer);

    let offer = true;
    let (with_offer, owner) = admit(
        &mut loopback,
        &mut authority,
        &handler,
        &lifecycle,
        &cancel,
        reconnect_credential(reconnect),
        offer,
    )
    .await
    .expect("true offer admission succeeds");
    assert_eq!(
        with_offer.welcome.welcome.lifecycle_control_supported,
        offer && lifecycle.implementation_available()
    );
    drop(owner);
    drop(with_offer);

    drop(handler);
    app.shutdown().await.expect("test application shuts down");
    drop(temporary);
    loopback.drain().await;
}

#[tokio::test]
async fn configured_status_crosses_connection_as_correlated_lifecycle_response() {
    let (temporary, app) = opened_app("status").await;
    let mut loopback = spawn_loopback();
    let gate = TestActivityGate::new(0);
    let lifecycle = LifecycleController::with_activity_gate(gate.clone());
    let mut authority = bootstrap_authority();
    let handler = RequestHandler::new(app.repository().clone());
    let cancel = CancelHandle::new();
    let (client, owner) = admit(
        &mut loopback,
        &mut authority,
        &handler,
        &lifecycle,
        &cancel,
        initial_credential(),
        true,
    )
    .await
    .expect("configured admission succeeds");

    let request = lifecycle_request("status-correlated", LifecycleRequest::Status);
    let (reply, owner) = round_trip(&client, request, response_stamp("status-response"), owner)
        .await
        .expect("status dispatch succeeds");
    let status = match lifecycle_payload(reply) {
        LifecycleResponse::Status(status) => status,
        LifecycleResponse::Stop(_) => panic!("status request returned stop response"),
    };
    assert_eq!(status.state, LifecycleState::Ready);
    assert_eq!(status.active_work_count, 0);
    assert_eq!(gate.begin_stop_calls(), 0);
    drop(owner);
    drop(client);

    drop(handler);
    app.shutdown().await.expect("test application shuts down");
    drop(temporary);
    loopback.drain().await;
}

async fn configured_stop(require_idle: bool, label: &str) {
    let (temporary, app) = opened_app(label).await;
    let mut loopback = spawn_loopback();
    let gate = TestActivityGate::new(0);
    let lifecycle = LifecycleController::with_activity_gate(gate.clone());
    let mut authority = bootstrap_authority();
    let handler = RequestHandler::new(app.repository().clone());
    let cancel = CancelHandle::new();
    let (client, owner) = admit(
        &mut loopback,
        &mut authority,
        &handler,
        &lifecycle,
        &cancel,
        initial_credential(),
        true,
    )
    .await
    .expect("configured admission succeeds");

    let request_id = if require_idle {
        "stop-require-idle"
    } else {
        "stop-force"
    };
    let request = lifecycle_request(request_id, LifecycleRequest::Stop { require_idle });
    assert!(!cancel.is_cancelled());
    let (reply, owner) = round_trip(&client, request, response_stamp("stop-response"), owner)
        .await
        .expect("accepted stop dispatch succeeds");
    let receipt = stop_receipt(lifecycle_payload(reply));
    assert_eq!(receipt.disposition, LifecycleStopDisposition::Accepted);
    assert_eq!(receipt.state, LifecycleState::Draining);
    assert_eq!(gate.last_require_idle(), require_idle);
    assert_eq!(gate.committed(), 1);
    assert_eq!(gate.rolled_back(), 0);
    // `round_trip` returns only after the local dispatcher has observed the
    // response FIN and released its receipt, so cancellation is now legal.
    assert!(cancel.is_cancelled());
    drop(owner);
    drop(client);

    drop(handler);
    app.shutdown().await.expect("test application shuts down");
    drop(temporary);
    loopback.drain().await;
}

#[tokio::test]
async fn configured_stop_preserves_require_idle_true() {
    configured_stop(true, "stop-true").await;
}

#[tokio::test]
async fn configured_stop_preserves_require_idle_false() {
    configured_stop(false, "stop-false").await;
}

#[tokio::test]
async fn response_failure_rolls_back_pending_stop_without_cancellation() {
    let (temporary, app) = opened_app("response-failure").await;
    let mut loopback = spawn_loopback();
    let gate = TestActivityGate::new(0);
    let lifecycle = LifecycleController::with_activity_gate(gate.clone());
    let mut authority = bootstrap_authority();
    let handler = RequestHandler::new(app.repository().clone());
    let cancel = CancelHandle::new();
    let (client, owner) = admit(
        &mut loopback,
        &mut authority,
        &handler,
        &lifecycle,
        &cancel,
        initial_credential(),
        true,
    )
    .await
    .expect("configured admission succeeds");

    let (mut request_send, mut request_recv) =
        client.connection.open_bi().await.expect("request stream");
    artisan_transport::send_envelope(
        &mut request_send,
        &lifecycle_request("failed-stop", LifecycleRequest::Stop { require_idle: true }),
    )
    .await
    .expect("stop request crosses the wire");

    // Refuse the response direction before the server starts dispatching.
    // The request direction remains open, so the real adapter must receive
    // the request, reserve the stop, and then lose the response at write/FIN.
    let _stopped = request_recv.stop(VarInt::from_u32(1));
    drop(request_recv);
    assert!(!cancel.is_cancelled());

    let outcome = tokio::time::timeout(
        TEST_DEADLINE,
        owner.respond_next(response_stamp("failed-stop-response")),
    )
    .await
    .expect("response failure dispatch settles");
    let failure = outcome.err().unwrap_or_else(|| {
        panic!("the stopped response stream unexpectedly accepted the reply");
    });
    assert!(matches!(
        failure,
        DeadlineError::Peer {
            operation: OperationKind::Receive,
            error: RequestStageError::Dispatch(
                ServerDispatchError::Send(_) | ServerDispatchError::Finish(_)
            ),
        }
    ));
    assert_eq!(gate.begin_stop_calls(), 1);
    drop(request_send);
    assert_eq!(gate.committed(), 0);
    assert_eq!(gate.rolled_back(), 1);
    assert!(!gate.is_reserved());
    assert!(!cancel.is_cancelled());

    match lifecycle
        .dispatch(
            RequestId::parse("after-response-failure").expect("status request id"),
            LifecycleRequest::Status,
        )
        .await
    {
        LifecycleDispatch::Reply {
            response: LifecycleResponse::Status(status),
            receipt,
        } => {
            assert_eq!(status.state, LifecycleState::Ready);
            assert_eq!(status.active_work_count, 0);
            drop(receipt);
        }
        LifecycleDispatch::Reply { .. } => panic!("status returned a non-status response"),
        LifecycleDispatch::Failure(_) => panic!("rollback did not restore Ready"),
    }
    drop(client);

    drop(handler);
    app.shutdown().await.expect("test application shuts down");
    drop(temporary);
    loopback.drain().await;
}

#[tokio::test]
async fn reconnect_negotiates_fresh_witness_instead_of_reusing_old_offer() {
    let (temporary, app) = opened_app("reconnect-witness").await;
    let mut loopback = spawn_loopback();
    let gate = TestActivityGate::new(0);
    let lifecycle = LifecycleController::with_activity_gate(gate.clone());
    let mut authority = bootstrap_authority();
    let handler = RequestHandler::new(app.repository().clone());
    let cancel = CancelHandle::new();

    let (first, owner) = admit(
        &mut loopback,
        &mut authority,
        &handler,
        &lifecycle,
        &cancel,
        initial_credential(),
        true,
    )
    .await
    .expect("first configured admission succeeds");
    assert!(first.welcome.welcome.lifecycle_control_supported);
    drop(owner);
    let reconnect = take_reconnect(first);

    let offer = false;
    let (second, owner) = admit(
        &mut loopback,
        &mut authority,
        &handler,
        &lifecycle,
        &cancel,
        reconnect_credential(reconnect),
        offer,
    )
    .await
    .expect("reconnect admission succeeds");
    // The reconnect is intentionally negotiated with a new Hello offer.
    assert_eq!(
        second.welcome.welcome.lifecycle_control_supported,
        offer && lifecycle.implementation_available()
    );
    let (reply, owner) = round_trip(
        &second,
        lifecycle_request("old-witness", LifecycleRequest::Status),
        response_stamp("old-witness-response"),
        owner,
    )
    .await
    .expect("unsupported lifecycle request receives a response");
    let WireEnvelopeBody::ProtocolError(failure) = reply.body else {
        panic!("a witness from the old connection was reused")
    };
    assert_eq!(failure.code, ErrorCode::UnsupportedFeature);
    assert_eq!(
        failure.request_id,
        Some(RequestId::parse("old-witness").expect("request id"))
    );
    drop(owner);
    drop(second);

    drop(handler);
    app.shutdown().await.expect("test application shuts down");
    drop(temporary);
    loopback.drain().await;
}
