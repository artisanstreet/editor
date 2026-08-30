//! Composed configured lifecycle tests for the private listener adapter.
//!
//! This path-linked fixture owns a listener on its normal split runtime and
//! drives three real client connections through the same listener-owned
//! controller. The activity gate remains test-only and crate-private.

use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use artisan_domain::UnixMillis;
use artisan_protocol::{
    FrameId, HelloCredential, LifecycleRequest, LifecycleResponse, LifecycleState,
    LifecycleStopDisposition, ReconnectCapability, ResponsePayload, ServerResponse, WireEnvelope,
    WireEnvelopeBody,
};
use artisan_transport::{CancelHandle, ServerWelcome};
use quinn::{Connection, Endpoint, RecvStream, SendStream, ServerConfig, VarInt};

use crate::command_admission::{CommandOrigin, CommandOriginClockError, CommandOriginEntropyError};
use crate::connection::lifecycle_connection_tests::{
    TEST_DEADLINE, TestActivityGate, client_config, hello_envelope, initial_capability,
    initial_credential, lifecycle_request, reconnect_credential, server_config, test_pki,
};
use crate::lifecycle_control::LifecycleController;
use crate::request_handler::RequestHandler;
use crate::{ForgeApp, ForgeConfig};

use super::{ForgeListener, ListenerLimits};

const FIRST_STOP_ID: &str = "listener-shared-stop";
const SECOND_RESPONSE_FRAME: &str = "listener-lifecycle-5";
const THIRD_RESPONSE_FRAME: &str = "listener-lifecycle-8";

#[derive(Debug)]
struct TestOrigin {
    next: AtomicU64,
}

impl TestOrigin {
    fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
        }
    }
}

impl CommandOrigin for TestOrigin {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        Ok(format!("listener-lifecycle-{sequence}"))
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        Ok(UnixMillis::from_millis(1_000))
    }
}

#[derive(Debug)]
enum BrokerCommand {
    Serve(Arc<CancelHandle>),
    Shutdown,
}

#[derive(Debug)]
enum BrokerReply {
    Served(bool),
}

struct ListenerBroker {
    address: SocketAddr,
    commands: Option<tokio::sync::mpsc::UnboundedSender<BrokerCommand>>,
    replies: tokio::sync::mpsc::UnboundedReceiver<BrokerReply>,
    active_cancel: Option<Arc<CancelHandle>>,
    thread: Option<JoinHandle<()>>,
}

impl ListenerBroker {
    fn serve(&mut self, cancel: Arc<CancelHandle>) {
        self.active_cancel = Some(Arc::clone(&cancel));
        self.commands
            .as_ref()
            .expect("listener broker command custody")
            .send(BrokerCommand::Serve(cancel))
            .expect("listener broker remains alive");
    }

    async fn served(&mut self) -> bool {
        let reply = tokio::time::timeout(TEST_DEADLINE, self.replies.recv())
            .await
            .expect("listener serve settles")
            .expect("listener broker remains alive");
        self.active_cancel = None;
        let BrokerReply::Served(success) = reply;
        success
    }

    fn shutdown(mut self) {
        if let Some(cancel) = self.active_cancel.take() {
            cancel.cancel();
        }
        if let Some(commands) = self.commands.take() {
            let _shutdown_sent = commands.send(BrokerCommand::Shutdown);
        }
        if let Some(thread) = self.thread.take() {
            thread.join().expect("listener broker thread finishes");
        }
    }
}

impl Drop for ListenerBroker {
    fn drop(&mut self) {
        if let Some(cancel) = self.active_cancel.take() {
            cancel.cancel();
        }
        if let Some(commands) = self.commands.take() {
            let _shutdown_sent = commands.send(BrokerCommand::Shutdown);
        }
        if let Some(thread) = self.thread.take() {
            thread.join().expect("listener broker thread finishes");
        }
    }
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TemporaryDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "artisan-forge-lifecycle-listener-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("temporary listener database directory");
        Self {
            database: directory.join("forge.sqlite3"),
            directory,
        }
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.directory);
    }
}

fn listener_limits() -> ListenerLimits {
    ListenerLimits {
        admission: Duration::from_secs(2),
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
        drain: Duration::from_secs(2),
    }
}

fn spawn_listener_broker(server: ServerConfig, gate: Arc<TestActivityGate>) -> ListenerBroker {
    let (address_tx, address_rx) = std::sync::mpsc::channel();
    let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (reply_tx, reply_rx) = tokio::sync::mpsc::unbounded_channel();
    let temporary = TemporaryDatabase::new("broker");

    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("listener broker runtime");
        runtime.block_on(async move {
            let app = ForgeApp::start(ForgeConfig::new(
                artisan_database::SqliteConfig::file(&temporary.database).sqlx_logging(false),
            ))
            .await
            .expect("listener broker application starts");
            let handler = RequestHandler::new(app.repository().clone());
            let lifecycle = LifecycleController::with_activity_gate(gate);
            let listener = ForgeListener::bind_with_lifecycle(
                server,
                initial_capability(),
                Box::new(TestOrigin::new()),
                listener_limits(),
                std::num::NonZeroU32::new(3).expect("admission capacity"),
                std::num::NonZeroU32::new(1).expect("request capacity"),
                lifecycle,
            )
            .expect("configured listener binds");
            let address = listener.local_addr().expect("listener address");
            address_tx
                .send(address)
                .expect("test waits for listener address");

            let mut custody = Some(listener);
            while let Some(command) = command_rx.recv().await {
                match command {
                    BrokerCommand::Serve(cancel) => {
                        let owner = custody.take().expect("listener custody between serves");
                        match owner.serve_one(&handler, cancel.as_ref()).await {
                            Ok((returned, _report)) => {
                                custody = Some(returned);
                                reply_tx
                                    .send(BrokerReply::Served(true))
                                    .expect("serve reply receiver remains alive");
                            }
                            Err(_error) => {
                                reply_tx
                                    .send(BrokerReply::Served(false))
                                    .expect("serve reply receiver remains alive");
                                break;
                            }
                        }
                    }
                    BrokerCommand::Shutdown => break,
                }
            }

            if let Some(listener) = custody {
                listener.drain().await.expect("listener endpoint drains");
            }
            drop(handler);
            app.shutdown()
                .await
                .expect("listener broker application shuts down");
        });
        drop(runtime);
    });

    let address = match address_rx.recv_timeout(TEST_DEADLINE) {
        Ok(address) => address,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("listener broker did not bind")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("listener broker died before binding")
        }
    };

    ListenerBroker {
        address,
        commands: Some(command_tx),
        replies: reply_rx,
        active_cancel: None,
        thread: Some(thread),
    }
}

struct AuthenticatedClient {
    connection: Connection,
    control_send: SendStream,
    control_recv: RecvStream,
    welcome: ServerWelcome,
}

async fn connect_client(endpoint: &Endpoint, address: SocketAddr) -> Connection {
    let connecting = endpoint
        .connect(address, artisan_transport::LOOPBACK_SERVER_NAME)
        .expect("client connect request accepted");
    tokio::time::timeout(TEST_DEADLINE, connecting)
        .await
        .expect("client connection settles")
        .expect("client connection establishes")
}

async fn admit_client(
    endpoint: &Endpoint,
    address: SocketAddr,
    credential: HelloCredential,
) -> AuthenticatedClient {
    let connection = connect_client(endpoint, address).await;
    let (mut send, mut recv) = connection.open_bi().await.expect("control stream opens");
    let welcome = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::client_handshake(&mut send, &mut recv, hello_envelope(credential, true)),
    )
    .await
    .expect("client handshake settles")
    .expect("configured listener handshake succeeds");
    AuthenticatedClient {
        connection,
        control_send: send,
        control_recv: recv,
        welcome,
    }
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
    expected_frame: &str,
) -> WireEnvelope {
    let (mut send, mut recv) = client
        .connection
        .open_bi()
        .await
        .expect("request stream opens");
    artisan_transport::send_envelope(&mut send, &request)
        .await
        .expect("lifecycle request crosses the wire");
    let reply = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut recv),
    )
    .await
    .expect("lifecycle response arrives")
    .expect("lifecycle response decodes");
    assert_eq!(
        reply.frame_id,
        FrameId::parse(expected_frame).expect("response frame id")
    );
    let request_id = artisan_domain::RequestId::parse(request.frame_id.as_str())
        .expect("request correlation id");
    match &reply.body {
        WireEnvelopeBody::Response(response) => assert_eq!(response.request_id, request_id),
        WireEnvelopeBody::ProtocolError(failure) => {
            assert_eq!(failure.request_id, Some(request_id))
        }
        _ => panic!("listener returned an unexpected lifecycle wire family"),
    }
    drop(send);
    drop(recv);
    reply
}

fn stop_response(reply: WireEnvelope) -> artisan_protocol::LifecycleStopReceipt {
    let WireEnvelopeBody::Response(ServerResponse {
        payload: ResponsePayload::Lifecycle(LifecycleResponse::Stop(receipt)),
        ..
    }) = reply.body
    else {
        panic!("expected a lifecycle stop response")
    };
    receipt
}

#[tokio::test]
async fn one_listener_controller_serializes_first_committed_stop_across_connections() {
    let pki = test_pki();
    let endpoint = artisan_transport::bind_loopback_client(client_config(&pki))
        .expect("client endpoint binds");
    let gate = TestActivityGate::new(0);
    let mut broker = spawn_listener_broker(server_config(&pki), Arc::clone(&gate));

    let first_cancel = Arc::new(CancelHandle::new());
    broker.serve(Arc::clone(&first_cancel));
    let first = admit_client(&endpoint, broker.address, initial_credential()).await;
    assert!(first.welcome.welcome.lifecycle_control_supported);
    let (mut first_send, first_recv) = first
        .connection
        .open_bi()
        .await
        .expect("first stop stream opens");
    artisan_transport::send_envelope(
        &mut first_send,
        &lifecycle_request(
            FIRST_STOP_ID,
            LifecycleRequest::Stop {
                require_idle: false,
            },
        ),
    )
    .await
    .expect("first stop request crosses the wire");
    first_send.finish().expect("first stop request finishes");
    // The server commits after its local response FIN; peer-side response
    // reading is intentionally not required before the listener closes.
    assert!(broker.served().await);
    assert!(first_cancel.is_cancelled());
    assert_eq!(gate.committed(), 1);
    drop(first_recv);
    let reconnect = take_reconnect(first);

    let duplicate_cancel = Arc::new(CancelHandle::new());
    broker.serve(Arc::clone(&duplicate_cancel));
    let duplicate = admit_client(&endpoint, broker.address, reconnect_credential(reconnect)).await;
    let duplicate_reply = exchange(
        &duplicate,
        lifecycle_request(
            FIRST_STOP_ID,
            LifecycleRequest::Stop {
                require_idle: false,
            },
        ),
        SECOND_RESPONSE_FRAME,
    )
    .await;
    let duplicate_receipt = stop_response(duplicate_reply);
    assert_eq!(
        duplicate_receipt.disposition,
        LifecycleStopDisposition::Duplicate
    );
    assert_eq!(duplicate_receipt.state, LifecycleState::Draining);
    assert!(broker.served().await);
    assert!(!duplicate_cancel.is_cancelled());
    let reconnect = take_reconnect(duplicate);

    let already_stopping_cancel = Arc::new(CancelHandle::new());
    broker.serve(Arc::clone(&already_stopping_cancel));
    let already_stopping =
        admit_client(&endpoint, broker.address, reconnect_credential(reconnect)).await;
    let already_stopping_reply = exchange(
        &already_stopping,
        lifecycle_request(
            "listener-other-stop",
            LifecycleRequest::Stop { require_idle: true },
        ),
        THIRD_RESPONSE_FRAME,
    )
    .await;
    let already_stopping_receipt = stop_response(already_stopping_reply);
    assert_eq!(
        already_stopping_receipt.disposition,
        LifecycleStopDisposition::AlreadyStopping
    );
    assert_eq!(already_stopping_receipt.state, LifecycleState::Draining);
    assert!(broker.served().await);
    assert!(!already_stopping_cancel.is_cancelled());
    assert_eq!(gate.committed(), 1);
    assert_eq!(gate.rolled_back(), 0);
    assert_eq!(gate.begin_stop_calls(), 1);
    let _reconnect = take_reconnect(already_stopping);

    broker.shutdown();
    artisan_transport::shutdown(
        &endpoint,
        VarInt::from_u32(0),
        b"lifecycle listener test complete",
        TEST_DEADLINE,
    )
    .await
    .expect("client endpoint drains");
}
