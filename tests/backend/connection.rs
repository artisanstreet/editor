//! Composition tests for the owned authenticated Forge connection.
//!
//! Every scenario drives real Quinn loopback QUIC with one ephemeral rcgen
//! certificate shared by both endpoint configurations and exact-leaf
//! pinning, a real migrated `ForgeApp` repository closed through its typed
//! shutdown before deletion, the real single-use [`CredentialAuthority`]
//! rotating through actual system entropy, and the public [`RequestHandler`]
//! seam. The Windows/Quinn endpoint constraint documented by the transport
//! harness applies unchanged: the server endpoint lives on its own thread
//! and runtime while the client endpoint and both application sides run on
//! the test runtime. Every test thread joins and every endpoint shuts down
//! under a bound; every retained handle is dropped before that drain.
//!
//! Stage evidence comes from actual transport behavior and direct public
//! inspection of the caller-owned authority; no production await hook
//! exists between the Welcome finish and its commit.

use std::error::Error;
use std::fs;
use std::mem::size_of;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread::JoinHandle;
use std::time::Duration;

use artisan_backend::conversation_subscription_registry::SubscriptionState;
use artisan_backend::{
    AuthenticationStageError, ConnectionLimits, CredentialAuthenticationError, CredentialAuthority,
    ForgeApp, ForgeConfig, ForgeConnection, LifecycleController, RequestHandler, RequestStageError,
    ServerFrameStamp, WelcomeMetadata,
};
use artisan_database::{
    AttachProjectInput, BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch,
    CreateThreadInput, DispatchLeaseOwner, LaunchClaimedRun, LaunchClaimedRunOutcome,
    ProviderBindingBytes, QueueFirstMessageInput, RunLaunchCredentials, RunStartKey,
    SetThreadEngineConfigInput, SqliteConfig,
};
use artisan_domain::{
    ApprovalMode, AttachProject, ByteLimit, Command, ConversationCursor, ConversationRequest,
    ConversationSubscribe, ConversationUnsubscribe, CountLimit, DirectoryId, DisplayName,
    EngineAgentId, EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FilesystemAccess, FiniteMillis, ItemId,
    ListAttachedProjects, ListDirectories, MessageBody, MessageId, NetworkAccess,
    OpenCode2Selection, PatchId, PermissionId, ProjectId, Query, QueueFirstMessage,
    ReceiptDisposition, RequestId, RootPath, ThreadId, ThreadTitle, TurnId, UnixMillis,
    WebSearchAccess,
};
use artisan_protocol::{
    APPLICATION_PROTOCOL_VERSION, ClientRequest, ConnectionId, ConversationSubscriptionStarted,
    ConversationSubscriptionStopped, ErrorCode, FirstMessageReceipt, FrameId, Hello,
    HelloCredential, LifecycleRequest, LocalCapability, ProtocolDecodeError, ProtocolVersion,
    ReconnectCapability, ResponsePayload, ServerResponse, VersionOffer, WireEnvelope,
    WireEnvelopeBody, encode_envelope,
};
use artisan_transport::{
    CancelHandle, DeadlineError, EnvelopeReceiveError, FrameError, HandshakeError,
    HandshakeMessageKind, OperationKind, PinnedIdentity, ServerDispatchError,
};
use quinn::{
    ClientConfig, Connection, ConnectionError, Endpoint, ReadError, RecvStream, SendStream,
    ServerConfig, TransportConfig, VarInt,
};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

/// Generous-but-bounded watchdog so a regression fails fast instead of
/// hanging the runner.
const TEST_DEADLINE: Duration = Duration::from_secs(5);

/// Deterministic issued bootstrap fixture; every successful admission still
/// rotates through actual system entropy.
const INITIAL_CAPABILITY: [u8; 32] = [0xb7; 32];
/// Wrong-value bootstrap fixture rejected before any consumption.
const WRONG_VALUE_CAPABILITY: [u8; 32] = [0x5c; 32];
/// Reconnect-family probe presented where an initial credential is expected.
const RECONNECT_PROBE_CAPABILITY: [u8; 32] = [0x11; 32];

/// Client-minted Hello frame identity; server frames must never echo it.
const CLIENT_HELLO_FRAME: &str = "client-hello-frame";
/// Injected diagnostic connection identity the Welcome must carry verbatim.
const WELCOME_CONNECTION_ID: &str = "forge-connection-under-test";
/// Injected server Welcome frame identity the Welcome must carry verbatim.
const WELCOME_FRAME: &str = "forge-welcome-frame";

/// Fixed close code and reason the component documents for every release.
const EXPECTED_CLOSE_CODE: u32 = 0x01;
const EXPECTED_CLOSE_REASON: &[u8] = b"forge connection released";
/// Fixed inbound-stop code the component documents for discarded streams.
const EXPECTED_STOP_CODE: u32 = 0x01;

// ---------------------------------------------------------------------------
// Loopback rig: one certificate pair shared by every configuration
// ---------------------------------------------------------------------------

struct Loopback {
    server_addr: SocketAddr,
    client: Endpoint,
    server_connections: tokio::sync::mpsc::Receiver<Connection>,
    stop_server: Option<tokio::sync::oneshot::Sender<()>>,
    server_thread: Option<JoinHandle<()>>,
}

impl Loopback {
    /// Signals the accept loop and joins the server thread. Idempotent, so
    /// both normal teardown and the [`Drop`] fallback share one path; the
    /// thread's own shutdown is bounded internally.
    fn join_server_thread(&mut self) {
        if let Some(stop) = self.stop_server.take() {
            let _signalled = stop.send(());
        }
        if let Some(thread) = self.server_thread.take() {
            thread.join().expect("server thread finishes");
        }
    }

    /// Full deterministic teardown within the watchdog. Every connection,
    /// stream, owner, abandoned future, and storage lease must already be
    /// released; a leak shows up here as a failed drain.
    ///
    /// # Panics
    ///
    /// Panics when either side fails to shut down deterministically.
    async fn drain(mut self) {
        self.join_server_thread();
        artisan_transport::shutdown(
            &self.client,
            VarInt::from_u32(0),
            b"connection test complete",
            TEST_DEADLINE,
        )
        .await
        .expect("client endpoint drains");
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        // Ownership fallback for early `?` returns or unwinding: the server
        // thread is always signalled and joined, never detached. Endpoint
        // draining remains `drain`'s job on the normal path.
        self.join_server_thread();
    }
}

/// One ephemeral certificate, key, and exact-leaf pin generated once per
/// harness so the server identity and every client trust decision match.
struct TestPki {
    certificate: CertificateDer<'static>,
    private_key: PrivatePkcs8KeyDer<'static>,
    pinned_identity: PinnedIdentity,
}

fn test_pki() -> TestPki {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("valid SANs");
    let certificate = certified_key.cert.der().clone();
    TestPki {
        pinned_identity: PinnedIdentity::from_certificate(&certificate),
        private_key: PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der()),
        certificate,
    }
}

fn server_config(pki: &TestPki) -> ServerConfig {
    // The pinned rustls-pki-types deliberately gives private keys no
    // `Clone` impl; `clone_key` is the explicit owned-copy API, used once
    // per harness so the served identity stays the pinned one.
    artisan_transport::server_config(vec![pki.certificate.clone()], pki.private_key.clone_key())
        .expect("server configuration")
}

fn client_config(pki: &TestPki) -> ClientConfig {
    artisan_transport::client_config(pki.certificate.clone(), pki.pinned_identity)
        .expect("client configuration")
}

/// Constrains the client's advertised receive credit so a server Welcome
/// write can never drain: deterministic post-consumption stage evidence
/// without any production hook. Pinning and ALPN stay exactly as built.
fn constrained_receive_window_config(mut base: ClientConfig) -> ClientConfig {
    let mut transport_config = TransportConfig::default();
    transport_config.stream_receive_window(VarInt::from_u32(16));
    transport_config.receive_window(VarInt::from_u32(16));
    base.transport_config(Arc::new(transport_config));
    base
}

fn spawn_loopback_pair(server: ServerConfig, client_config: ClientConfig) -> Loopback {
    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    let (connections_tx, connections_rx) = tokio::sync::mpsc::channel(1);
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();

    let server_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("server runtime");
        runtime.block_on(async move {
            let server = artisan_transport::bind_loopback_server(server).expect("server bind");
            let server_addr = server.local_addr().expect("bound server address");
            addr_tx
                .send(server_addr)
                .expect("the test waits for the server address");

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
                    .expect("server handshake within deadline")
                    .expect("server connection established");
                if connections_tx.send(established).await.is_err() {
                    break;
                }
            }

            artisan_transport::shutdown(
                &server,
                VarInt::from_u32(0),
                b"test complete",
                TEST_DEADLINE,
            )
            .await
            .expect("server endpoint drains");
        });
    });

    let server_addr = match addr_rx.recv_timeout(TEST_DEADLINE) {
        Ok(server_addr) => server_addr,
        Err(RecvTimeoutError::Timeout) => panic!("server did not bind within deadline"),
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

fn spawn_loopback_constrained_receive() -> Loopback {
    let pki = test_pki();
    spawn_loopback_pair(
        server_config(&pki),
        constrained_receive_window_config(client_config(&pki)),
    )
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
        .expect("handshake completes within deadline")
        .expect("connection established")
}

async fn next_server_connection(loopback: &mut Loopback) -> Connection {
    tokio::time::timeout(TEST_DEADLINE, loopback.server_connections.recv())
        .await
        .expect("server connection arrives within deadline")
        .expect("server keeps accepting")
}

// ---------------------------------------------------------------------------
// Real migrated Forge application fixture
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
            "artisan-forge-connection-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("temporary database directory should be created");
        let database = directory.join("forge.sqlite3");
        Self {
            directory,
            database,
        }
    }

    fn path(&self) -> &Path {
        &self.database
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.directory);
    }
}

async fn opened_app(label: &str) -> (TemporaryDatabase, ForgeApp) {
    let temporary = TemporaryDatabase::new(label);
    let app = ForgeApp::start(ForgeConfig::new(
        SqliteConfig::file(temporary.path()).sqlx_logging(false),
    ))
    .await
    .expect("migrated Forge application should start");
    (temporary, app)
}

fn bootstrap_authority() -> CredentialAuthority {
    CredentialAuthority::new(LocalCapability::from_bytes(INITIAL_CAPABILITY))
}

fn attach_input() -> AttachProjectInput {
    AttachProjectInput {
        request_id: RequestId::parse("request-project-1").expect("valid request id"),
        directory_id: DirectoryId::parse("directory-project-1").expect("valid directory id"),
        project_id: ProjectId::parse("project-1").expect("valid project id"),
        root_path: RootPath::parse("C:/repos/project-1").expect("valid root path"),
        display_name: DisplayName::parse("Project One").expect("valid display name"),
        attached_at: UnixMillis::from_millis(100),
    }
}

fn create_thread_input() -> CreateThreadInput {
    CreateThreadInput {
        request_id: RequestId::parse("request-thread-1").expect("valid request id"),
        thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
        project_id: ProjectId::parse("project-1").expect("valid project id"),
        title: ThreadTitle::parse("First thread").expect("valid title"),
        created_at: UnixMillis::from_millis(200),
        updated_at: UnixMillis::from_millis(200),
    }
}

fn queue_input() -> QueueFirstMessageInput {
    QueueFirstMessageInput {
        request_id: RequestId::parse("request-message-1").expect("valid request id"),
        message_id: MessageId::parse("message-1").expect("valid message id"),
        thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
        body: MessageBody::parse("first body").expect("valid body"),
        accepted_at: UnixMillis::from_millis(300),
    }
}

// ---------------------------------------------------------------------------
// Protocol fixtures (static-valid, therefore infallible)
// ---------------------------------------------------------------------------

fn initial_credential() -> HelloCredential {
    HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY))
}

fn wrong_value_credential() -> HelloCredential {
    HelloCredential::Initial(LocalCapability::from_bytes(WRONG_VALUE_CAPABILITY))
}

fn reconnect_probe() -> HelloCredential {
    HelloCredential::Reconnect(ReconnectCapability::from_bytes(RECONNECT_PROBE_CAPABILITY))
}

fn reconnect_credential(capability: ReconnectCapability) -> HelloCredential {
    HelloCredential::Reconnect(capability)
}

fn hello_envelope(credential: HelloCredential) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(CLIENT_HELLO_FRAME).expect("valid fixture frame id"),
        sent_at: UnixMillis::from_millis(10),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![APPLICATION_PROTOCOL_VERSION])
                .expect("valid fixture version offer"),
            credential,
            supports_lifecycle_control: false,
        }),
    }
}

fn request_envelope(frame: &str, request: ClientRequest) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame).expect("valid fixture frame id"),
        sent_at: UnixMillis::from_millis(3_000),
        body: WireEnvelopeBody::Request(request),
    }
}

fn list_projects_request(frame: &str) -> WireEnvelope {
    request_envelope(
        frame,
        ClientRequest::Query(Query::ListAttachedProjects(ListAttachedProjects)),
    )
}

fn list_directories_request(frame: &str) -> WireEnvelope {
    request_envelope(
        frame,
        ClientRequest::Query(Query::ListDirectories(ListDirectories { parent: None })),
    )
}

fn lifecycle_request(frame: &str, request: LifecycleRequest) -> WireEnvelope {
    request_envelope(frame, ClientRequest::Lifecycle(request))
}

fn attach_project_command(frame: &str) -> WireEnvelope {
    request_envelope(
        frame,
        ClientRequest::Command(Command::AttachProject(AttachProject {
            request_id: RequestId::parse(frame).expect("valid fixture request id"),
            directory_id: DirectoryId::parse("directory-project-1")
                .expect("valid fixture directory id"),
        })),
    )
}

fn queue_first_message_command(frame: &str) -> WireEnvelope {
    request_envelope(
        frame,
        ClientRequest::Command(Command::QueueFirstMessage(QueueFirstMessage {
            request_id: RequestId::parse(frame).expect("valid fixture request id"),
            thread_id: ThreadId::parse("thread-1").expect("valid fixture thread id"),
            body: MessageBody::parse("first body").expect("valid fixture body"),
        })),
    )
}

fn fresh_subscription_request(frame: &str, thread_id: ThreadId) -> WireEnvelope {
    request_envelope(
        frame,
        ClientRequest::Conversation(ConversationRequest::Subscribe(
            ConversationSubscribe::fresh(thread_id),
        )),
    )
}

fn resume_subscription_request(
    frame: &str,
    thread_id: ThreadId,
    after: ConversationCursor,
) -> WireEnvelope {
    request_envelope(
        frame,
        ClientRequest::Conversation(ConversationRequest::Subscribe(
            ConversationSubscribe::resume(thread_id, after),
        )),
    )
}

fn unsubscribe_request(frame: &str, thread_id: ThreadId) -> WireEnvelope {
    request_envelope(
        frame,
        ClientRequest::Conversation(ConversationRequest::Unsubscribe(ConversationUnsubscribe {
            thread_id,
        })),
    )
}

fn fixture_engine_config() -> EngineRunConfig {
    let one = FiniteMillis::new(1).expect("one millisecond is valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget is valid"),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit is valid"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit is valid"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit is valid"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness line limit is valid"),
        max_header_count: CountLimit::new(8).expect("header count is valid"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer limit is valid"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr limit is valid"),
        observation_capacity: CountLimit::new(16).expect("observation capacity is valid"),
    })
    .expect("runtime relationships are valid");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-connection").expect("permission id is valid"),
        EngineAgentId::parse("agent-connection").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-connection").expect("profile id is valid"),
            EngineModelId::parse("model-connection").expect("model id is valid"),
            EngineRouteId::parse("route-connection").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

async fn seed_subscription_thread(app: &ForgeApp) -> Result<ThreadId, Box<dyn Error>> {
    let repository = app.repository();
    repository.attach_project(attach_input()).await?;
    repository.create_thread(create_thread_input()).await?;
    let thread_id = ThreadId::parse("thread-1")?;
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("request-engine-connection")?,
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: fixture_engine_config(),
            accepted_at: UnixMillis::from_millis(200),
        })
        .await?;
    repository.queue_first_message(queue_input()).await?;

    // Finish the same public repository workflow that creates durable
    // conversation patches, so the resume case exercises the replay-batch
    // preparation branch while still asserting activation at the request
    // cursor.
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x11; 32]),
            claimed_at: UnixMillis::from_millis(400),
            lease_expires_at: UnixMillis::from_millis(900),
        })
        .await?
        .ok_or("seed dispatch should be claimable")?;
    let run_id = artisan_domain::RunId::parse("run-1")?;
    let turn_id = TurnId::parse("turn-1")?;
    let item_id = ItemId::parse("item-1")?;
    let first_patch_id = PatchId::parse("patch-1-first")?;
    let second_patch_id = PatchId::parse("patch-1-second")?;
    let run_start_key = RunStartKey::new([0x44; 32]);
    let credentials = RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]);
    let engine_settings = repository
        .read_thread_engine_settings(&thread_id)
        .await?
        .ok_or("seed engine settings should be present")?;
    let launched = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &item_id,
            first_patch_id: &first_patch_id,
            second_patch_id: &second_patch_id,
            operated_at: UnixMillis::from_millis(500),
            run_start_key: &run_start_key,
            credentials: &credentials,
            engine_settings: &engine_settings,
        })
        .await?;
    let launched = match launched {
        LaunchClaimedRunOutcome::Started(receipt)
        | LaunchClaimedRunOutcome::AlreadyStarted(receipt) => receipt,
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16])?;
    let bound = repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &run_start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(500),
            bound_at: UnixMillis::from_millis(600),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await?;
    if !matches!(
        bound,
        BindRunProviderOutcome::Bound(_) | BindRunProviderOutcome::AlreadyBound(_)
    ) {
        return Err("seed provider binding did not persist".into());
    }

    Ok(ThreadId::parse("thread-1")?)
}

fn welcome_metadata() -> WelcomeMetadata {
    WelcomeMetadata {
        connection_id: ConnectionId::parse(WELCOME_CONNECTION_ID).expect("valid connection id"),
        frame: ServerFrameStamp {
            frame_id: FrameId::parse(WELCOME_FRAME).expect("valid frame id"),
            sent_at: UnixMillis::from_millis(50),
        },
    }
}

fn response_stamp(frame: &str) -> ServerFrameStamp {
    ServerFrameStamp {
        frame_id: FrameId::parse(frame).expect("valid frame id"),
        sent_at: UnixMillis::from_millis(60),
    }
}

fn default_limits() -> ConnectionLimits {
    ConnectionLimits {
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
    }
}

fn default_lifecycle() -> &'static LifecycleController {
    static CONTROLLER: OnceLock<LifecycleController> = OnceLock::new();
    CONTROLLER.get_or_init(LifecycleController::new)
}

/// Builds framed bytes whose nested first-message receipt id disagrees with
/// its enclosing response id. The decoder derives command identities from
/// their own frames, so this nested-response break is the correlation
/// violation that is genuinely reachable from the wire; the codec must
/// reject it before any handler runs. Both ids are twelve ASCII bytes so
/// the same-length splice leaves the Cap'n Proto framing intact.
fn broken_receipt_correlation_frame_bytes() -> Result<Vec<u8>, Box<dyn Error>> {
    let envelope = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("server-receipt-frame").expect("valid fixture frame id"),
        sent_at: UnixMillis::from_millis(3_500),
        body: WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("corr-frame-x").expect("valid fixture request id"),
            payload: ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                request_id: RequestId::parse("corr-frame-x").expect("valid fixture request id"),
                message_id: MessageId::parse("message-1").expect("valid fixture message id"),
                thread_id: ThreadId::parse("thread-1").expect("valid fixture thread id"),
                disposition: ReceiptDisposition::Duplicate,
            }),
        }),
    };
    let mut encoded = encode_envelope(&envelope)?;
    let needle = b"corr-frame-x";
    let last = encoded
        .windows(needle.len())
        .rposition(|window| window == needle)
        .ok_or("fixture identity missing from encoded envelope")?;
    encoded[last..last + needle.len()].copy_from_slice(b"corr-frame-y");
    Ok(encoded)
}

// ---------------------------------------------------------------------------
// Composition helpers
// ---------------------------------------------------------------------------

/// Client half of one authenticated connection, holding its control stream
/// open for the connection lifetime.
struct AuthenticatedClient {
    connection: Connection,
    control_send: SendStream,
    control_recv: RecvStream,
    welcome: artisan_transport::ServerWelcome,
}

/// Moves the rotated capability out of an authenticated client and
/// explicitly disposes of every other handle so nothing survives to the
/// endpoint drain.
fn dispose_client(client: AuthenticatedClient) -> ReconnectCapability {
    let AuthenticatedClient {
        connection,
        control_send,
        control_recv,
        welcome,
    } = client;
    drop(connection);
    drop(control_send);
    drop(control_recv);
    welcome.welcome.reconnect_capability
}

/// Extracts the typed deadline failure from a bounded outcome whose success
/// side carries an owned connection. Matching the whole value releases every
/// borrowed lease the success side would otherwise keep alive past this
/// point.
///
/// # Panics
///
/// Panics with `context` when the operation unexpectedly succeeded.
fn expect_failure<T, E>(outcome: Result<T, DeadlineError<E>>, context: &str) -> DeadlineError<E> {
    match outcome {
        Err(failure) => failure,
        Ok(_) => panic!("{context}"),
    }
}

/// Admits one connection end to end: the real client handshake against the
/// real component, both driven concurrently on the test runtime.
///
/// # Panics
///
/// Panics when either side does not settle under the watchdog or fails.
async fn admitted_client<'authority, 'handler, 'cancel>(
    loopback: &mut Loopback,
    authority: &'authority mut CredentialAuthority,
    handler: &'handler RequestHandler,
    cancel: &'cancel CancelHandle,
    credential: HelloCredential,
) -> Result<
    (
        AuthenticatedClient,
        ForgeConnection<'authority, 'handler, 'cancel, 'static>,
    ),
    Box<dyn Error>,
> {
    let client_connection = connect_client(loopback).await;
    let server_connection = next_server_connection(loopback).await;

    let client = async {
        let (mut send, mut recv) = client_connection.open_bi().await?;
        let welcome = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::client_handshake(&mut send, &mut recv, hello_envelope(credential)),
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
        default_lifecycle(),
        welcome_metadata(),
        default_limits(),
        cancel,
    );

    let (client, owner) = tokio::join!(client, tokio::time::timeout(TEST_DEADLINE, server),);
    let owner = owner.expect("authentication settles under the watchdog")?;
    Ok((client?, owner))
}

/// Drives a doomed admission: the client completes its handshake half, the
/// component rejects the presented credential at its typed credential
/// stage, and the fixed application close reaches the client. Returns the
/// typed reason the handshake ended.
///
/// # Panics
///
/// Panics when the credential unexpectedly authenticates, when the server
/// failure is not the credential stage, or either side stalls past the
/// watchdog.
async fn rejected_admission(
    loopback: &mut Loopback,
    authority: &mut CredentialAuthority,
    handler: &RequestHandler,
    cancel: &CancelHandle,
    credential: HelloCredential,
) -> HandshakeError {
    let client_connection = connect_client(loopback).await;
    let server_connection = next_server_connection(loopback).await;

    // Fixture setup resolves its own watchdog layers so the async block
    // below deals only in the real handshake types.
    let (mut send, mut recv) = tokio::time::timeout(TEST_DEADLINE, client_connection.open_bi())
        .await
        .expect("the doomed control stream opens under the watchdog")
        .expect("the doomed control stream opens");

    let client = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::client_handshake(&mut send, &mut recv, hello_envelope(credential)),
    );
    let server = ForgeConnection::authenticate(
        server_connection,
        authority,
        handler,
        default_lifecycle(),
        welcome_metadata(),
        default_limits(),
        cancel,
    );

    let (client_outcome, server_outcome) = tokio::join!(client, server);
    // The rejection must be exactly the credential stage — family mismatch,
    // value rejection, or awaiting rotation — never any other stage.
    let failure = expect_failure(
        server_outcome,
        "the presented credential must not authenticate",
    );
    assert!(
        matches!(
            &failure,
            DeadlineError::Peer {
                operation: OperationKind::Handshake,
                error: AuthenticationStageError::Credential(_),
            }
        ),
        "expected a typed credential rejection, got {failure:?}"
    );
    drop(failure);

    match client_outcome.expect("client handshake settles under the watchdog") {
        Ok(_) => panic!("the client must not observe a Welcome"),
        Err(error) => error,
    }
}

/// Runs one complete request/response round: one owner dispatch against one
/// client exchange, concurrently, both under the watchdog.
///
/// # Panics
///
/// Panics when either side does not settle under the watchdog or fails.
async fn round_trip<'authority, 'handler, 'cancel>(
    client: &AuthenticatedClient,
    request: WireEnvelope,
    stamp: ServerFrameStamp,
    owner: ForgeConnection<'authority, 'handler, 'cancel, 'static>,
) -> Result<
    (
        WireEnvelope,
        ForgeConnection<'authority, 'handler, 'cancel, 'static>,
    ),
    Box<dyn Error>,
> {
    // A deliberately cloned stamp serves the borrowed client half while the
    // original moves into the consuming dispatch half.
    let client_stamp = stamp.clone();
    let replied = tokio::time::timeout(TEST_DEADLINE, exchange(client, &request, &client_stamp));
    let served = tokio::time::timeout(TEST_DEADLINE, owner.respond_next(stamp));
    let (replied, served) = tokio::join!(replied, served);
    let reply = replied.expect("exchange settles under the watchdog")?;
    let owner = served.expect("dispatch settles under the watchdog")?;
    Ok((reply, owner))
}

/// Exchanges one request on a fresh stream, asserting the reply carries the
/// supplied fresh server stamp and exactly one reply precedes a clean FIN.
///
/// # Panics
///
/// Panics when framing, stamps, or end-of-stream behavior regress.
async fn exchange(
    client: &AuthenticatedClient,
    request: &WireEnvelope,
    stamp: &ServerFrameStamp,
) -> Result<WireEnvelope, Box<dyn Error>> {
    let (mut send, mut recv) = client.connection.open_bi().await?;
    artisan_transport::send_envelope(&mut send, request).await?;

    let reply = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut recv),
    )
    .await
    .expect("reply arrives under the watchdog")?;
    assert_reply_stamp(&reply, stamp);

    // The watchdog is resolved first so the typed match below examines the
    // real stream outcome, never an outer timeout layer.
    let end = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut recv),
    )
    .await
    .expect("stream end arrives under the watchdog");
    match end {
        Ok(envelope) => panic!(
            "expected exactly one reply, got a second envelope with frame id {}",
            envelope.frame_id
        ),
        Err(EnvelopeReceiveError::Frame(FrameError::Truncated {
            expected,
            received: 0,
        })) if expected == size_of::<u32>() => {}
        Err(other) => panic!("expected clean end-of-stream behind one reply, got {other:?}"),
    }
    drop(send);
    drop(recv);
    Ok(reply)
}

fn assert_reply_stamp(reply: &WireEnvelope, stamp: &ServerFrameStamp) {
    assert_eq!(
        reply.frame_id, stamp.frame_id,
        "server frame stamp identity"
    );
    assert_eq!(reply.sent_at, stamp.sent_at, "server frame stamp timestamp");
}

/// Asserts the control stream's server direction ended cleanly behind
/// exactly one Welcome.
///
/// # Panics
///
/// Panics when stray envelopes precede the end of the control stream.
async fn expect_control_stream_finished(client: &mut AuthenticatedClient) {
    let end = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut client.control_recv),
    )
    .await
    .expect("control stream end arrives under the watchdog");
    match end {
        Ok(envelope) => panic!(
            "expected only one Welcome, got another envelope with frame id {}",
            envelope.frame_id
        ),
        Err(EnvelopeReceiveError::Frame(FrameError::Truncated {
            expected,
            received: 0,
        })) if expected == size_of::<u32>() => {}
        Err(other) => {
            panic!("expected the control stream to finish behind one Welcome, got {other:?}")
        }
    }
}

/// Asserts the peer observes exactly the fixed secret-free application
/// close the component documents.
///
/// # Panics
///
/// Panics when closure does not arrive or carries a different code/reason.
async fn expect_application_close(connection: &Connection) {
    let closed = tokio::time::timeout(TEST_DEADLINE, connection.closed())
        .await
        .expect("connection closure arrives within the watchdog");
    match closed {
        ConnectionError::ApplicationClosed(close) => {
            assert_eq!(close.error_code, VarInt::from_u32(EXPECTED_CLOSE_CODE));
            assert_eq!(close.reason.as_ref(), EXPECTED_CLOSE_REASON);
        }
        unexpected => panic!("expected the fixed application close, got {unexpected:?}"),
    }
}

/// Writes `payload` to a fresh stream as one raw bounded frame.
async fn send_raw_frame(connection: &Connection, payload: &[u8]) -> Result<(), Box<dyn Error>> {
    let (mut send, _recv) = connection.open_bi().await?;
    let mut framed = Vec::with_capacity(size_of::<u32>() + payload.len());
    framed.extend_from_slice(&u32::try_from(payload.len())?.to_le_bytes());
    framed.extend_from_slice(payload);
    tokio::time::timeout(TEST_DEADLINE, send.write_all(&framed)).await??;
    drop(send);
    Ok(())
}

// ---------------------------------------------------------------------------
// Composition coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lifecycle_request_without_a_negotiated_witness_is_correlated_and_fail_closed()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("lifecycle-unsupported").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;
    assert!(!client.welcome.welcome.lifecycle_control_supported);

    let (reply, owner) = round_trip(
        &client,
        lifecycle_request("lifecycle-unsupported", LifecycleRequest::Status),
        response_stamp("lifecycle-unsupported-reply"),
        owner,
    )
    .await?;
    let WireEnvelopeBody::ProtocolError(failure) = reply.body else {
        panic!("expected unsupported-feature protocol failure");
    };
    assert_eq!(failure.code, ErrorCode::UnsupportedFeature);
    assert!(!failure.retryable);
    assert_eq!(
        failure.request_id,
        Some(RequestId::parse("lifecycle-unsupported").expect("valid request id"))
    );
    assert!(!cancel.is_cancelled());

    drop(owner);
    expect_application_close(&client.connection).await;
    drop(client);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn bootstrap_admission_serves_a_real_listing_and_finishes_every_stream()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("bootstrap").await;
    app.repository().attach_project(attach_input()).await?;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let (mut client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;

    // Injected metadata crosses verbatim; nothing echoes the client frame.
    assert_eq!(client.welcome.frame_id.as_str(), WELCOME_FRAME);
    assert_eq!(client.welcome.sent_at, UnixMillis::from_millis(50));
    assert_eq!(
        client.welcome.welcome.negotiated_version,
        ProtocolVersion::V1
    );
    assert_eq!(
        client.welcome.welcome.connection_id.as_str(),
        WELCOME_CONNECTION_ID
    );
    assert_ne!(client.welcome.frame_id.as_str(), CLIENT_HELLO_FRAME);
    expect_control_stream_finished(&mut client).await;

    let (reply, owner) = round_trip(
        &client,
        list_projects_request("frame-list-projects"),
        response_stamp("forge-list-frame"),
        owner,
    )
    .await?;
    let WireEnvelopeBody::Response(response) = reply.body else {
        panic!("expected a correlated response");
    };
    assert_eq!(response.request_id.as_str(), "frame-list-projects");
    let ResponsePayload::ProjectListing(listing) = response.payload else {
        panic!("expected a project listing payload");
    };
    assert_eq!(listing.projects().len(), 1);
    assert_eq!(listing.projects()[0].project_id.as_str(), "project-1");

    // The caller loops sequentially over the same ready owner.
    let (_second, owner) = round_trip(
        &client,
        list_projects_request("frame-list-projects-again"),
        response_stamp("forge-list-frame-2"),
        owner,
    )
    .await?;

    drop(owner);
    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn fresh_subscription_activates_at_snapshot_cursor_after_response_fin()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("subscription-fresh").await;
    let thread_id = seed_subscription_thread(&app).await?;
    let handler = RequestHandler::with_subscriptions(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;

    let (reply, owner) = round_trip(
        &client,
        fresh_subscription_request("frame-subscribe-fresh", thread_id.clone()),
        response_stamp("forge-subscribe-fresh-frame"),
        owner,
    )
    .await?;
    let WireEnvelopeBody::Response(response) = reply.body else {
        panic!("expected a correlated fresh-subscription response");
    };
    assert_eq!(response.request_id.as_str(), "frame-subscribe-fresh");
    let ResponsePayload::ConversationSubscriptionStarted(ConversationSubscriptionStarted::Fresh(
        start,
    )) = response.payload
    else {
        panic!("expected a fresh conversation subscription response");
    };
    assert_eq!(start.snapshot().thread_id(), &thread_id);
    let response_cursor = start.snapshot().cursor();

    // `round_trip` returns only after the consuming owner has returned, so
    // this view observes the post-FIN activation rather than the preparation
    // state held while the response was still in flight.
    let active = handler
        .subscription_view(&thread_id)
        .await
        .expect("fresh subscription should remain registered");
    assert_eq!(active.state(), SubscriptionState::Active);
    assert_eq!(active.cursor(), response_cursor);

    drop(owner);
    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn resumed_subscription_activates_at_requested_cursor_after_response_fin()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("subscription-resume").await;
    let thread_id = seed_subscription_thread(&app).await?;
    let handler = RequestHandler::with_subscriptions(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;

    let requested_cursor = ConversationCursor::new(1);
    let (reply, owner) = round_trip(
        &client,
        resume_subscription_request(
            "frame-subscribe-resume",
            thread_id.clone(),
            requested_cursor,
        ),
        response_stamp("forge-subscribe-resume-frame"),
        owner,
    )
    .await?;
    let WireEnvelopeBody::Response(response) = reply.body else {
        panic!("expected a correlated resume response");
    };
    assert_eq!(response.request_id.as_str(), "frame-subscribe-resume");
    let ResponsePayload::ConversationSubscriptionStarted(
        ConversationSubscriptionStarted::Resumed {
            thread_id: response_thread_id,
            cursor,
        },
    ) = response.payload
    else {
        panic!("expected a resumed conversation subscription response");
    };
    assert_eq!(response_thread_id, thread_id);
    assert_eq!(cursor, requested_cursor);

    // The preparation seam intentionally discards any replay batch endpoint;
    // activation must retain the cursor named by the response.
    let active = handler
        .subscription_view(&thread_id)
        .await
        .expect("resumed subscription should remain registered");
    assert_eq!(active.state(), SubscriptionState::Active);
    assert_eq!(active.cursor(), requested_cursor);

    drop(owner);
    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn unsubscribe_is_idempotent_on_a_sequential_connection_without_activation_work()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("subscription-unsubscribe").await;
    let thread_id = seed_subscription_thread(&app).await?;
    let handler = RequestHandler::with_subscriptions(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;

    let (reply, owner) = round_trip(
        &client,
        fresh_subscription_request("frame-unsubscribe-seed", thread_id.clone()),
        response_stamp("forge-unsubscribe-seed-frame"),
        owner,
    )
    .await?;
    let WireEnvelopeBody::Response(response) = reply.body else {
        panic!("expected a correlated seed response");
    };
    assert!(matches!(
        response.payload,
        ResponsePayload::ConversationSubscriptionStarted(ConversationSubscriptionStarted::Fresh(_))
    ));
    assert_eq!(
        handler
            .subscription_view(&thread_id)
            .await
            .expect("seed subscription should be active")
            .state(),
        SubscriptionState::Active
    );

    let (reply, owner) = round_trip(
        &client,
        unsubscribe_request("frame-unsubscribe", thread_id.clone()),
        response_stamp("forge-unsubscribe-frame"),
        owner,
    )
    .await?;
    let WireEnvelopeBody::Response(response) = reply.body else {
        panic!("expected a correlated unsubscribe response");
    };
    assert_eq!(response.request_id.as_str(), "frame-unsubscribe");
    let ResponsePayload::ConversationSubscriptionStopped(stopped) = response.payload else {
        panic!("expected an idempotent stopped response");
    };
    assert_eq!(
        stopped,
        ConversationSubscriptionStopped {
            thread_id: thread_id.clone()
        }
    );
    assert!(handler.subscription_view(&thread_id).await.is_none());

    // A second stop crosses the same sequential owner and proves the absent
    // path is the same successful no-work acknowledgement.
    let (reply, owner) = round_trip(
        &client,
        unsubscribe_request("frame-unsubscribe-again", thread_id.clone()),
        response_stamp("forge-unsubscribe-again-frame"),
        owner,
    )
    .await?;
    let WireEnvelopeBody::Response(response) = reply.body else {
        panic!("expected a correlated repeated-unsubscribe response");
    };
    assert_eq!(response.request_id.as_str(), "frame-unsubscribe-again");
    let ResponsePayload::ConversationSubscriptionStopped(stopped) = response.payload else {
        panic!("expected an idempotent stopped response for an absent entry");
    };
    assert_eq!(stopped, ConversationSubscriptionStopped { thread_id });
    assert!(
        handler
            .subscription_view(&ThreadId::parse("thread-1")?)
            .await
            .is_none()
    );

    drop(owner);
    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn handler_failures_cross_the_wire_and_broken_correlation_stays_a_codec_rejection()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("wire-failures").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;

    // A real handler failure maps onto a correlated wire failure.
    let (reply, owner) = round_trip(
        &client,
        list_directories_request("frame-directories"),
        response_stamp("forge-directories-frame"),
        owner,
    )
    .await?;
    let WireEnvelopeBody::ProtocolError(failure) = reply.body else {
        panic!("expected a correlated wire failure");
    };
    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(!failure.retryable);
    assert_eq!(
        failure.request_id,
        Some(RequestId::parse("frame-directories")?)
    );

    // A frame whose nested receipt id disagrees with its enclosing response
    // id is rejected by the codec before any handler runs; it never becomes
    // a fabricated handler response.
    let framed = broken_receipt_correlation_frame_bytes()?;
    let sending = send_raw_frame(&client.connection, &framed);
    let serving = tokio::time::timeout(
        TEST_DEADLINE,
        owner.respond_next(response_stamp("forge-broken-frame")),
    );
    let (_, serving) = tokio::join!(sending, serving);
    let failure = expect_failure(
        serving.expect("the corrupted dispatch settles under the watchdog"),
        "a codec-invalid frame must not dispatch",
    );
    assert!(
        matches!(
            &failure,
            DeadlineError::Peer {
                operation: OperationKind::Receive,
                error: RequestStageError::Dispatch(ServerDispatchError::Receive(
                    EnvelopeReceiveError::Decode(ProtocolDecodeError::CorrelationMismatch { .. }),
                )),
            }
        ),
        "expected a codec-level correlation rejection, got {failure:?}"
    );
    drop(failure);

    // The failed dispatch consumed its owner: the connection closes with
    // the fixed reason.
    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn rotation_replaces_consumed_credentials_across_genuinely_new_connections()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("rotation").await;
    let handler = RequestHandler::new(app.repository().clone());
    let cancel = CancelHandle::new();

    // Bootstrap admission mints the first real rotated capability.
    let mut authority = bootstrap_authority();
    let (first_client, first_owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;

    // Dropping the old owner closes its connection observably.
    drop(first_owner);
    expect_application_close(&first_client.connection).await;
    let rotated_once = dispose_client(first_client);

    // The consumed bootstrap credential can never authenticate again.
    rejected_admission(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await;

    // The genuinely rotated credential admits a new connection and rotates
    // again.
    let (second_client, second_owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        reconnect_credential(rotated_once),
    )
    .await?;
    drop(second_owner);
    expect_application_close(&second_client.connection).await;
    let rotated_twice = dispose_client(second_client);

    // The current rotation still admits on a genuinely new connection; the
    // chain stays usable end to end.
    let (third_client, third_owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        reconnect_credential(rotated_twice),
    )
    .await?;
    drop(third_owner);
    expect_application_close(&third_client.connection).await;
    drop(third_client);

    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn wrong_family_and_value_leave_the_genuine_credential_usable() -> Result<(), Box<dyn Error>>
{
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("wrong-credentials").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    // Wrong credential family, presented before any consumption.
    rejected_admission(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        reconnect_probe(),
    )
    .await;
    // Right family, wrong value: equally non-consuming.
    rejected_admission(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        wrong_value_credential(),
    )
    .await;

    // The genuine bootstrap credential still admits and serves.
    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;
    let (_reply, owner) = round_trip(
        &client,
        list_projects_request("frame-after-wrong"),
        response_stamp("forge-after-wrong-frame"),
        owner,
    )
    .await?;
    drop(owner);
    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn non_hello_first_frames_never_touch_the_authority() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("non-hello").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    // A valid Request envelope as the very first control message is
    // rejected by the handshake seam before the authority sees anything.
    // The writer outcome is deliberately ignored: the server may reset the
    // stream after rejecting, which can fail a trailing write.
    let client_connection = connect_client(&loopback).await;
    let server_connection = next_server_connection(&mut loopback).await;
    let writer = async {
        let (mut send, _recv) = client_connection.open_bi().await?;
        artisan_transport::send_envelope(&mut send, &list_projects_request("frame-too-early"))
            .await?;
        Ok::<(), Box<dyn Error>>(())
    };
    let server = ForgeConnection::authenticate(
        server_connection,
        &mut authority,
        &handler,
        default_lifecycle(),
        welcome_metadata(),
        default_limits(),
        &cancel,
    );
    let (_, settled) = tokio::join!(
        tokio::time::timeout(TEST_DEADLINE, writer),
        tokio::time::timeout(TEST_DEADLINE, server),
    );
    // The watchdog layer resolves first so the assertion below examines the
    // real typed stage outcome.
    let failure = expect_failure(
        settled.expect("authentication settles under the watchdog"),
        "a non-Hello first frame must not authenticate",
    );
    assert!(
        matches!(
            &failure,
            DeadlineError::Peer {
                operation: OperationKind::Handshake,
                error: AuthenticationStageError::Handshake(HandshakeError::UnexpectedMessage {
                    expected: HandshakeMessageKind::Hello,
                    received: HandshakeMessageKind::Request,
                }),
            }
        ),
        "expected a typed first-frame rejection, got {failure:?}"
    );
    drop(failure);
    drop(client_connection);

    // The authority was untouched: the genuine credential still admits and
    // serves a real listing end to end.
    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;
    let (_reply, owner) = round_trip(
        &client,
        list_projects_request("frame-after-non-hello"),
        response_stamp("forge-after-non-hello-frame"),
        owner,
    )
    .await?;
    drop(owner);
    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

/// Writes a truncated first frame from the client while the component
/// authenticates concurrently, returning the typed handshake failure.
async fn truncated_first_frame_failure(
    loopback: &mut Loopback,
    authority: &mut CredentialAuthority,
    handler: &RequestHandler,
    cancel: &CancelHandle,
) -> DeadlineError<AuthenticationStageError> {
    let client_connection = connect_client(loopback).await;
    let server_connection = next_server_connection(loopback).await;
    let writer = async {
        let (mut send, _recv) = client_connection.open_bi().await?;
        tokio::time::timeout(TEST_DEADLINE, send.write_all(&64u32.to_le_bytes())).await??;
        tokio::time::timeout(TEST_DEADLINE, send.write_all(&[7_u8; 10])).await??;
        send.finish()?;
        Ok::<(), Box<dyn Error>>(())
    };
    let server = ForgeConnection::authenticate(
        server_connection,
        authority,
        handler,
        default_lifecycle(),
        welcome_metadata(),
        default_limits(),
        cancel,
    );
    let (_, settled) = tokio::join!(
        tokio::time::timeout(TEST_DEADLINE, writer),
        tokio::time::timeout(TEST_DEADLINE, server),
    );
    let failure = expect_failure(
        settled.expect("authentication settles under the watchdog"),
        "a truncated first frame must not authenticate",
    );
    drop(client_connection);
    failure
}

/// Writes an oversized first-frame prefix from the client while the
/// component authenticates concurrently, returning the typed handshake
/// failure together with the announced length it was rejected against.
async fn oversized_first_frame_failure(
    loopback: &mut Loopback,
    authority: &mut CredentialAuthority,
    handler: &RequestHandler,
    cancel: &CancelHandle,
) -> (DeadlineError<AuthenticationStageError>, u32) {
    let announced = artisan_transport::MAX_FRAME_LEN + 1;
    let client_connection = connect_client(loopback).await;
    let server_connection = next_server_connection(loopback).await;
    let writer = async {
        let (mut send, _recv) = client_connection.open_bi().await?;
        tokio::time::timeout(TEST_DEADLINE, send.write_all(&announced.to_le_bytes())).await??;
        Ok::<(), Box<dyn Error>>(())
    };
    let server = ForgeConnection::authenticate(
        server_connection,
        authority,
        handler,
        default_lifecycle(),
        welcome_metadata(),
        default_limits(),
        cancel,
    );
    let (_, settled) = tokio::join!(
        tokio::time::timeout(TEST_DEADLINE, writer),
        tokio::time::timeout(TEST_DEADLINE, server),
    );
    let failure = expect_failure(
        settled.expect("authentication settles under the watchdog"),
        "an oversized first frame must not authenticate",
    );
    drop(client_connection);
    (failure, announced)
}

#[tokio::test]
async fn malformed_and_oversized_first_frames_never_touch_the_authority()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("malformed-first").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    // A truncated frame: the prefix announces 64 bytes, only 10 arrive.
    let failure =
        truncated_first_frame_failure(&mut loopback, &mut authority, &handler, &cancel).await;
    assert!(
        matches!(
            &failure,
            DeadlineError::Peer {
                operation: OperationKind::Handshake,
                error: AuthenticationStageError::Handshake(HandshakeError::Receive(
                    EnvelopeReceiveError::Frame(FrameError::Truncated {
                        expected: 64,
                        received: 10,
                    }),
                )),
            }
        ),
        "expected typed truncation, got {failure:?}"
    );
    drop(failure);

    // An oversized prefix is rejected against the transport ceiling before
    // any body allocation or authority involvement.
    let (failure, announced) =
        oversized_first_frame_failure(&mut loopback, &mut authority, &handler, &cancel).await;
    match &failure {
        DeadlineError::Peer {
            operation: OperationKind::Handshake,
            error:
                AuthenticationStageError::Handshake(HandshakeError::Receive(
                    EnvelopeReceiveError::Frame(FrameError::TooLarge { length, bound }),
                )),
        } => {
            assert_eq!(*length, announced);
            assert_eq!(*bound, artisan_transport::MAX_FRAME_LEN);
        }
        other => panic!("expected an oversized rejection, got {other:?}"),
    }
    drop(failure);

    // Neither attempt consumed anything: the genuine credential admits.
    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;
    let (_reply, owner) = round_trip(
        &client,
        list_projects_request("frame-after-malformed"),
        response_stamp("forge-after-malformed-frame"),
        owner,
    )
    .await?;
    drop(owner);
    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

/// Admits with a zero-length handshake limit — a representable instant that
/// simply expires first — and returns the typed timeout decided before any
/// stage is polled.
async fn zero_limit_handshake_failure(
    loopback: &mut Loopback,
    authority: &mut CredentialAuthority,
    handler: &RequestHandler,
) -> DeadlineError<AuthenticationStageError> {
    let client_connection = connect_client(loopback).await;
    let server_connection = next_server_connection(loopback).await;
    let cancel = CancelHandle::new();
    let outcome = tokio::time::timeout(
        TEST_DEADLINE,
        ForgeConnection::authenticate(
            server_connection,
            authority,
            handler,
            default_lifecycle(),
            welcome_metadata(),
            ConnectionLimits {
                handshake: Duration::ZERO,
                next_request: Duration::from_secs(2),
            },
            &cancel,
        ),
    )
    .await
    .expect("authentication settles under the watchdog");
    drop(client_connection);
    expect_failure(outcome, "a zero-length limit must not authenticate")
}

/// Admits under an already-cancelled handle and returns the typed
/// cancellation decided before any poll.
async fn precancelled_handshake_failure(
    loopback: &mut Loopback,
    authority: &mut CredentialAuthority,
    handler: &RequestHandler,
) -> DeadlineError<AuthenticationStageError> {
    let client_connection = connect_client(loopback).await;
    let server_connection = next_server_connection(loopback).await;
    let cancel = CancelHandle::new();
    cancel.cancel();
    let outcome = tokio::time::timeout(
        TEST_DEADLINE,
        ForgeConnection::authenticate(
            server_connection,
            authority,
            handler,
            default_lifecycle(),
            welcome_metadata(),
            default_limits(),
            &cancel,
        ),
    )
    .await
    .expect("authentication settles under the watchdog");
    drop(client_connection);
    expect_failure(outcome, "a pre-cancelled handle must not authenticate")
}

#[tokio::test]
async fn pre_consumption_deadline_and_cancellation_keep_the_bootstrap_credential_usable()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("pre-consumption").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();

    // A zero-length handshake limit expires before any stage is polled.
    let failure = zero_limit_handshake_failure(&mut loopback, &mut authority, &handler).await;
    assert!(
        matches!(
            &failure,
            DeadlineError::Timeout {
                operation: OperationKind::Handshake,
                ..
            }
        ),
        "expected a typed handshake timeout, got {failure:?}"
    );
    drop(failure);

    // A handle cancelled before the future is polled wins immediately.
    let failure = precancelled_handshake_failure(&mut loopback, &mut authority, &handler).await;
    assert!(
        matches!(
            &failure,
            DeadlineError::Cancelled {
                operation: OperationKind::Handshake,
            }
        ),
        "expected typed cancellation, got {failure:?}"
    );
    drop(failure);

    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;

    // Retention is proven by a real successful admission on fresh endpoints.
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("post-abort").await;
    let handler = RequestHandler::new(app.repository().clone());
    let cancel = CancelHandle::new();
    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;
    let (_reply, owner) = round_trip(
        &client,
        list_projects_request("frame-after-abort"),
        response_stamp("forge-after-abort-frame"),
        owner,
    )
    .await?;
    drop(owner);
    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn a_stalled_welcome_write_times_out_leaving_the_authority_awaiting_rotation()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback_constrained_receive();
    let (_temporary, app) = opened_app("stalled-welcome").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let client_connection = connect_client(&loopback).await;
    let server_connection = next_server_connection(&mut loopback).await;

    // The client sends its Hello and never reads; its sixteen-byte receive
    // window guarantees the server Welcome write can never drain, so the
    // credential is consumed while rotation cannot commit.
    let (mut control_send, control_recv) = client_connection.open_bi().await?;
    artisan_transport::send_envelope(&mut control_send, &hello_envelope(initial_credential()))
        .await?;
    drop(control_send);

    let outcome = tokio::time::timeout(
        TEST_DEADLINE,
        ForgeConnection::authenticate(
            server_connection,
            &mut authority,
            &handler,
            default_lifecycle(),
            welcome_metadata(),
            ConnectionLimits {
                handshake: Duration::from_millis(400),
                next_request: Duration::from_secs(2),
            },
            &cancel,
        ),
    )
    .await
    .expect("the stalled handshake settles");
    let failure = expect_failure(
        outcome,
        "a blocked Welcome write must not complete authentication",
    );
    assert!(
        matches!(
            &failure,
            DeadlineError::Timeout {
                operation: OperationKind::Handshake,
                ..
            }
        ),
        "expected the stalled handshake to time out, got {failure:?}"
    );
    drop(failure);

    // Deterministic stage evidence on the caller-owned authority: had the
    // commit fired, this probe would answer Rejected or FamilyMismatch, so
    // only a genuinely uncommitted rotation answers AwaitingRotation.
    assert!(matches!(
        authority.authenticate(reconnect_probe()),
        Err(CredentialAuthenticationError::AwaitingRotation)
    ));

    expect_application_close(&client_connection).await;
    drop(control_recv);
    drop(client_connection);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn cancelling_a_blocked_welcome_keeps_queued_requests_undispatched()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback_constrained_receive();
    let (_temporary, app) = opened_app("cancelled-welcome").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();

    let client_connection = connect_client(&loopback).await;
    let server_connection = next_server_connection(&mut loopback).await;

    // The client sends its Hello, immediately queues a real request on a
    // separate stream, and never reads anything beyond one bounded Welcome
    // prefix; its sixteen-byte receive window guarantees the consumed
    // rotation stays blocked on the unservable Welcome write.
    let (mut control_send, mut control_recv) = client_connection.open_bi().await?;
    artisan_transport::send_envelope(&mut control_send, &hello_envelope(initial_credential()))
        .await?;
    drop(control_send);
    let (mut queued_send, mut queued_recv) = client_connection.open_bi().await?;
    artisan_transport::send_envelope(
        &mut queued_send,
        &list_projects_request("frame-queued-early"),
    )
    .await?;
    drop(queued_send);

    // Causal stage proof. The bounded Welcome prefix is the real witness:
    // these are actual bytes of the server's Welcome, so the credential was
    // consumed and the rotation staged — the commit strictly follows the
    // completed write and finish — while the sixteen-byte receive window
    // keeps the full Welcome from ever completing. The admission is polled
    // concurrently with the witness, but only the successful witness
    // completion triggers the existing CancelHandle; an admission that ends
    // before that moment fails the test outright, and the witness's own
    // bounded read watchdog fails it if no progress ever occurs. No timer
    // expiry can take the successful-cancellation path.
    let mut welcome_prefix = [0_u8; 8];
    let cancel = CancelHandle::new();
    let mut admission = Box::pin(tokio::time::timeout(
        TEST_DEADLINE,
        ForgeConnection::authenticate(
            server_connection,
            &mut authority,
            &handler,
            default_lifecycle(),
            welcome_metadata(),
            default_limits(),
            &cancel,
        ),
    ));
    tokio::select! {
        result = &mut admission => match result {
            Ok(_) => panic!("the admission completed before the causal cancellation fired"),
            Err(failure) => {
                panic!("the admission failed before the causal cancellation fired: {failure:?}")
            }
        },
        () = async {
            tokio::time::timeout(TEST_DEADLINE, control_recv.read_exact(&mut welcome_prefix))
                .await
                .expect("the Welcome prefix arrives under the watchdog")
                .expect("the Welcome prefix is readable");
        } => {}
    }

    // The witness has provably completed first; cancellation now follows as
    // its direct causal consequence.
    cancel.cancel();
    let failure = expect_failure(
        admission
            .as_mut()
            .await
            .expect("the cancelled admission settles under the watchdog"),
        "a cancelled admission must not succeed",
    );
    assert!(
        matches!(
            &failure,
            DeadlineError::Cancelled {
                operation: OperationKind::Handshake,
            }
        ),
        "expected typed cancellation after consumption, got {failure:?}"
    );
    drop(failure);
    // The owning boxed future is released explicitly: awaiting only its
    // pinned reference leaves the completed-but-retained admission — and
    // with it the exclusive authority lease — alive inside the box, so the
    // caller-owned authority below may only be probed once this is gone.
    drop(admission);

    // Stage evidence on the caller-owned authority: consumption happened
    // while the rotation never committed.
    assert!(matches!(
        authority.authenticate(reconnect_probe()),
        Err(CredentialAuthenticationError::AwaitingRotation)
    ));

    // The queued request stream carried no envelope at all: nothing may be
    // dispatched before the Welcome finish and commit point, and closure is
    // what the abandoned stream observes instead.
    let observed = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut queued_recv),
    )
    .await
    .expect("queued stream outcome arrives under the watchdog");
    match observed {
        Ok(envelope) => panic!(
            "a queued request was dispatched before commit: frame id {} crossed the stream",
            envelope.frame_id
        ),
        Err(EnvelopeReceiveError::Frame(FrameError::Read(ReadError::ConnectionLost(
            ConnectionError::ApplicationClosed(_),
        )))) => {}
        Err(other) => panic!("expected closure without any dispatch, got {other:?}"),
    }

    expect_application_close(&client_connection).await;
    drop(control_recv);
    drop(queued_recv);
    drop(client_connection);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

/// Pipelines Hello followed immediately by one real request on a single
/// control stream, reads the resulting Welcome envelope, and returns every
/// retained direction plus that owned Welcome for inspection.
async fn pipelined_client_half(
    connection: &Connection,
) -> Result<(SendStream, RecvStream, WireEnvelope), Box<dyn Error>> {
    let (mut control_send, mut control_recv) = connection.open_bi().await?;
    artisan_transport::send_envelope(&mut control_send, &hello_envelope(initial_credential()))
        .await?;
    artisan_transport::send_envelope(&mut control_send, &list_projects_request("frame-pipelined"))
        .await?;
    let welcomed = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut control_recv),
    )
    .await
    .expect("the Welcome arrives under the watchdog")?;
    Ok((control_send, control_recv, welcomed))
}

/// Asserts the control send direction was stopped outright with exactly the
/// documented inbound-stop code.
///
/// # Panics
///
/// Panics when the direction is never stopped or carries another code.
async fn expect_control_direction_stopped(control_send: &mut SendStream) {
    // The watchdog and stopped-result layers resolve before matching the
    // real stop code: pinned Quinn resolves `stopped()` to `Option<VarInt>`.
    let stop_code = tokio::time::timeout(TEST_DEADLINE, control_send.stopped())
        .await
        .expect("stop observation settles under the watchdog")
        .expect("the control send stream remains observable");
    match stop_code {
        Some(code) => assert_eq!(code, VarInt::from_u32(EXPECTED_STOP_CODE)),
        None => panic!("expected the discarded control direction to be stopped"),
    }
}

#[tokio::test]
async fn pipelined_control_bytes_are_discarded_before_any_dispatch() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("pipelined").await;
    app.repository().attach_project(attach_input()).await?;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let client_connection = connect_client(&loopback).await;
    let server_connection = next_server_connection(&mut loopback).await;

    let server = ForgeConnection::authenticate(
        server_connection,
        &mut authority,
        &handler,
        default_lifecycle(),
        welcome_metadata(),
        default_limits(),
        &cancel,
    );
    let (client_outcome, owner_outcome) = tokio::join!(
        tokio::time::timeout(TEST_DEADLINE, pipelined_client_half(&client_connection)),
        tokio::time::timeout(TEST_DEADLINE, server),
    );
    let owner = owner_outcome.expect("authentication settles under the watchdog")?;
    let (control_send, control_recv, welcomed) =
        client_outcome.expect("the pipelined client settles under the watchdog")?;

    // The Welcome carries exactly the injected server metadata.
    assert_reply_stamp(&welcomed, &welcome_metadata().frame);
    let WireEnvelopeBody::Welcome(welcome_body) = welcomed.body else {
        panic!("expected a Welcome on the control stream");
    };
    assert_eq!(welcome_body.negotiated_version, ProtocolVersion::V1);
    assert_eq!(welcome_body.connection_id.as_str(), WELCOME_CONNECTION_ID);

    let mut link = AuthenticatedClient {
        connection: client_connection,
        control_send,
        control_recv,
        welcome: artisan_transport::ServerWelcome {
            protocol_version: welcomed.protocol_version,
            frame_id: welcomed.frame_id,
            sent_at: welcomed.sent_at,
            welcome: welcome_body,
        },
    };

    // The control stream carried only the Welcome behind a clean FIN: the
    // pipelined request produced no dispatch and no reply there.
    expect_control_stream_finished(&mut link).await;

    // The component stopped reading that direction outright, discarding the
    // trailing bytes instead of letting them become requests.
    expect_control_direction_stopped(&mut link.control_send).await;

    // Legitimately opened streams keep dispatching with real correlation.
    let (_reply, owner) = round_trip(
        &link,
        list_projects_request("frame-after-pipeline"),
        response_stamp("forge-after-pipeline-frame"),
        owner,
    )
    .await?;

    drop(owner);
    expect_application_close(&link.connection).await;
    drop(link);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn late_hello_on_a_request_stream_is_rejected_and_closes_the_owner()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("late-hello").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let (client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;

    // A Hello arriving on a later request stream is rejected by the
    // established request dispatcher before any handler runs.
    let (mut late_send, late_recv) = client.connection.open_bi().await?;
    artisan_transport::send_envelope(&mut late_send, &hello_envelope(reconnect_probe())).await?;
    drop(late_send);
    drop(late_recv);

    let outcome = tokio::time::timeout(
        TEST_DEADLINE,
        owner.respond_next(response_stamp("forge-late-frame")),
    )
    .await
    .expect("dispatch settles under the watchdog");
    let failure = expect_failure(outcome, "a Hello on a request stream must not dispatch");
    assert!(
        matches!(
            &failure,
            DeadlineError::Peer {
                operation: OperationKind::Receive,
                error: RequestStageError::Dispatch(ServerDispatchError::UnexpectedMessage {
                    received: HandshakeMessageKind::Hello,
                }),
            }
        ),
        "expected a typed Hello-on-request-stream rejection, got {failure:?}"
    );
    drop(failure);

    expect_application_close(&client.connection).await;
    drop(client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn dropping_unpolled_futures_and_ready_owners_closes_connections()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("drop-cleanup").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    // An authentication future dropped before its first poll still closes
    // the accepted connection with the fixed reason.
    let client_connection = connect_client(&loopback).await;
    let server_connection = next_server_connection(&mut loopback).await;
    let unpolled = ForgeConnection::authenticate(
        server_connection,
        &mut authority,
        &handler,
        default_lifecycle(),
        welcome_metadata(),
        default_limits(),
        &cancel,
    );
    drop(unpolled);
    expect_application_close(&client_connection).await;
    drop(client_connection);

    // A ready owner dropped after full admission closes likewise, and the
    // rotated capability survives for a genuinely new connection.
    let (first_client, first_owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;
    drop(first_owner);
    expect_application_close(&first_client.connection).await;
    let rotated = dispose_client(first_client);

    let (second_client, second_owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        reconnect_credential(rotated),
    )
    .await?;
    drop(second_owner);
    expect_application_close(&second_client.connection).await;
    drop(second_client);

    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

#[tokio::test]
async fn dropping_an_in_flight_dispatch_after_a_partial_request_closes_the_owner()
-> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("in-flight-drop").await;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    let (first_client, first_owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;
    // The rotated capability is extracted through the disposal helper only
    // after this connection's closure has been observed below, so the
    // client is never partially moved while still in use.

    // A partial request frame keeps the dispatcher genuinely mid-receive on
    // an accepted stream: the prefix announces far more bytes than arrive,
    // and the send side is deliberately left unfinished.
    let (mut request_send, request_recv) = first_client.connection.open_bi().await?;
    tokio::time::timeout(
        TEST_DEADLINE,
        request_send.write_all(&4096u32.to_le_bytes()),
    )
    .await??;
    tokio::time::timeout(TEST_DEADLINE, request_send.write_all(&[7_u8; 4])).await??;

    // The dispatch is owned by a boxed pinned future, so polling it pending
    // proves a genuinely in-flight accepted-stream operation whose owner —
    // exclusive authority lease included — lives inside that future.
    let mut serving = Box::pin(first_owner.respond_next(response_stamp("forge-in-flight-frame")));
    let still_pending = tokio::time::timeout(Duration::from_millis(150), serving.as_mut()).await;
    assert!(
        still_pending.is_err(),
        "the dispatch must remain genuinely in flight on a partial request"
    );
    // The watchdog result carries the owner type in its success arm, so it
    // is released explicitly before anything else touches the lease.
    drop(still_pending);

    // Dropping the owned future releases the owner itself: the private
    // stream guard resets the unfinished send side and stops inbound
    // synchronously, and the connection closes observably.
    drop(serving);
    expect_application_close(&first_client.connection).await;
    drop(request_send);
    drop(request_recv);
    let rotated = dispose_client(first_client);

    // Peer loss during an otherwise idle owner surfaces as a typed accept
    // failure on the next bounded dispatch; cleanup still closes cleanly.
    let (second_client, second_owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        reconnect_credential(rotated),
    )
    .await?;
    second_client
        .connection
        .close(VarInt::from_u32(0x02), b"peer done");
    let outcome = tokio::time::timeout(
        TEST_DEADLINE,
        second_owner.respond_next(response_stamp("forge-peer-loss-frame")),
    )
    .await
    .expect("dispatch settles under the watchdog");
    let failure = expect_failure(outcome, "dispatch after peer loss must not succeed");
    assert!(
        matches!(
            &failure,
            DeadlineError::Peer {
                operation: OperationKind::Receive,
                error: RequestStageError::Accept { .. },
            }
        ),
        "expected a typed accept failure after peer loss, got {failure:?}"
    );
    drop(failure);
    drop(second_client);

    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

/// Asserts one attach reply replays its seeded durable receipt as a
/// duplicate for the expected project, correlated to `request_id`.
///
/// # Panics
///
/// Panics when the reply family, correlation, disposition, or identity
/// regress.
fn assert_duplicate_attach_reply(reply: WireEnvelope, request_id: &str) {
    let WireEnvelopeBody::Response(response) = reply.body else {
        panic!("expected a correlated response");
    };
    assert_eq!(response.request_id.as_str(), request_id);
    let ResponsePayload::AttachedProject {
        project,
        disposition,
    } = response.payload
    else {
        panic!("expected an attached-project payload");
    };
    assert_eq!(disposition, ReceiptDisposition::Duplicate);
    assert_eq!(project.project_id.as_str(), "project-1");
}

#[tokio::test]
async fn durable_receipts_replay_across_genuinely_new_connections() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("durable-replay").await;
    app.repository().attach_project(attach_input()).await?;
    app.repository()
        .create_thread(create_thread_input())
        .await?;
    app.repository().queue_first_message(queue_input()).await?;
    let handler = RequestHandler::new(app.repository().clone());
    let mut authority = bootstrap_authority();
    let cancel = CancelHandle::new();

    // The first connection replays the seeded attach receipt.
    let (first_client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        initial_credential(),
    )
    .await?;
    let (reply, owner) = round_trip(
        &first_client,
        attach_project_command("request-project-1"),
        response_stamp("forge-replay-frame-a"),
        owner,
    )
    .await?;
    assert_duplicate_attach_reply(reply, "request-project-1");

    drop(owner);
    expect_application_close(&first_client.connection).await;
    let rotated = dispose_client(first_client);

    // On a genuinely new connection, identical correlation ids replay from
    // durable receipts alone. This leaf keeps no registry, so no live
    // connection state could have produced these replies, and retired ids
    // are never reused within a live connection.
    let (second_client, owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        reconnect_credential(rotated),
    )
    .await?;
    let (reply, owner) = round_trip(
        &second_client,
        attach_project_command("request-project-1"),
        response_stamp("forge-replay-frame-b"),
        owner,
    )
    .await?;
    assert_duplicate_attach_reply(reply, "request-project-1");

    let (reply, owner) = round_trip(
        &second_client,
        queue_first_message_command("request-message-1"),
        response_stamp("forge-replay-frame-c"),
        owner,
    )
    .await?;
    let WireEnvelopeBody::Response(response) = reply.body else {
        panic!("expected a correlated response");
    };
    let ResponsePayload::FirstMessageQueued(receipt) = response.payload else {
        panic!("expected a first-message receipt payload");
    };
    assert_eq!(
        receipt,
        FirstMessageReceipt {
            request_id: RequestId::parse("request-message-1")?,
            message_id: MessageId::parse("message-1")?,
            thread_id: ThreadId::parse("thread-1")?,
            disposition: ReceiptDisposition::Duplicate,
        }
    );

    drop(owner);
    expect_application_close(&second_client.connection).await;
    drop(second_client);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}

/// Admits on a fresh rig with a zero-length next-request limit — a
/// representable instant that expires first, distinct from any invalid-limit
/// case — asserts the typed request timeout through full teardown, and hands
/// back the rotated reconnect capability that committed admission produced.
/// The expiry-before-poll means this does not prove an in-flight timeout;
/// in-flight coverage lives in the cancellation and drop scenarios.
async fn zero_next_request_timeout_returns_rotated(
    authority: &mut CredentialAuthority,
) -> Result<ReconnectCapability, Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("request-cancel").await;
    let handler = RequestHandler::new(app.repository().clone());
    let cancel = CancelHandle::new();

    let client_connection = connect_client(&loopback).await;
    let server_connection = next_server_connection(&mut loopback).await;

    let client = async {
        let (mut send, mut recv) = client_connection.open_bi().await?;
        let welcome = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::client_handshake(
                &mut send,
                &mut recv,
                hello_envelope(initial_credential()),
            ),
        )
        .await??;
        Ok::<_, Box<dyn Error>>((send, recv, welcome))
    };
    let server = ForgeConnection::authenticate(
        server_connection,
        authority,
        &handler,
        default_lifecycle(),
        welcome_metadata(),
        ConnectionLimits {
            handshake: Duration::from_secs(2),
            next_request: Duration::ZERO,
        },
        &cancel,
    );
    let (client_outcome, owner_outcome) =
        tokio::join!(tokio::time::timeout(TEST_DEADLINE, client), server);
    let owner = owner_outcome.expect("authentication settles under its own deadline");
    let (control_send, control_recv, welcome) =
        client_outcome.expect("the client settles under the watchdog")?;

    let outcome = tokio::time::timeout(
        TEST_DEADLINE,
        owner.respond_next(response_stamp("forge-request-timeout-frame")),
    )
    .await
    .expect("dispatch settles under the watchdog");
    let failure = expect_failure(outcome, "a zero-length request limit must not dispatch");
    assert!(
        matches!(
            &failure,
            DeadlineError::Timeout {
                operation: OperationKind::Receive,
                ..
            }
        ),
        "expected a typed request timeout, got {failure:?}"
    );
    drop(failure);

    expect_application_close(&client_connection).await;
    drop(control_send);
    drop(control_recv);
    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(welcome.welcome.reconnect_capability)
}

/// Opens one request stream and writes an accepted but deliberately
/// unfinished partial frame, keeping any concurrent dispatch mid-receive.
async fn write_unfinished_partial_frame(
    connection: &Connection,
    announced: u32,
    prefix_bytes: [u8; 4],
) -> Result<(SendStream, RecvStream), Box<dyn Error>> {
    let (mut send, recv) = connection.open_bi().await?;
    tokio::time::timeout(TEST_DEADLINE, send.write_all(&announced.to_le_bytes())).await??;
    tokio::time::timeout(TEST_DEADLINE, send.write_all(&prefix_bytes)).await??;
    Ok((send, recv))
}

#[tokio::test]
async fn request_stage_deadline_and_cancellation_preserve_the_committed_rotation()
-> Result<(), Box<dyn Error>> {
    let mut authority = bootstrap_authority();

    // Request-stage timeout through the established typed mechanism; see
    // the helper for the honest ZERO-limit limitation.
    let rotated_once = zero_next_request_timeout_returns_rotated(&mut authority).await?;

    // Request-stage cancellation mid-receive: an accepted partial frame
    // parks the dispatch, then the caller's handle fires.
    let mut loopback = spawn_loopback();
    let (_temporary, app) = opened_app("request-cancel-mid").await;
    let handler = RequestHandler::new(app.repository().clone());
    let cancel = CancelHandle::new();
    let (first_client, first_owner) = admitted_client(
        &mut loopback,
        &mut authority,
        &handler,
        &cancel,
        reconnect_credential(rotated_once),
    )
    .await?;

    let (request_send, request_recv) =
        write_unfinished_partial_frame(&first_client.connection, 8192, [9_u8; 4]).await?;

    let mut serving =
        Box::pin(first_owner.respond_next(response_stamp("forge-request-cancel-frame")));
    let still_pending = tokio::time::timeout(Duration::from_millis(150), serving.as_mut()).await;
    assert!(
        still_pending.is_err(),
        "the dispatch must remain genuinely in flight on a partial request"
    );
    // The watchdog result carries the owner type in its success arm, so it
    // is released explicitly before cancellation proceeds.
    drop(still_pending);

    let canceller = {
        let cancel = &cancel;
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        }
    };
    let outcome = tokio::time::timeout(TEST_DEADLINE, serving.as_mut());
    let (outcome, ()) = tokio::join!(outcome, canceller);
    let failure = expect_failure(
        outcome.expect("the cancelled dispatch settles under the watchdog"),
        "a cancelled dispatch must not succeed",
    );
    assert!(
        matches!(
            &failure,
            DeadlineError::Cancelled {
                operation: OperationKind::Receive,
            }
        ),
        "expected a typed request cancellation, got {failure:?}"
    );
    drop(failure);
    // The owned boxed future — owner and lease inside — is released before
    // anything else touches the authority or the fixture handles.
    drop(serving);

    expect_application_close(&first_client.connection).await;
    drop(request_send);
    drop(request_recv);
    drop(first_client);

    // The committed rotation survived both abandonments: the authority
    // still expects a reconnect credential, so a same-family probe answers
    // Rejected rather than AwaitingRotation.
    assert!(matches!(
        authority.authenticate(reconnect_probe()),
        Err(CredentialAuthenticationError::Rejected { .. })
    ));

    drop(handler);
    app.shutdown()
        .await
        .expect("application storage should close");
    loopback.drain().await;
    Ok(())
}
