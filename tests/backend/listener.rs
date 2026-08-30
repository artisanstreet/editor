//! Behavioral tests for the owning loopback Forge listener.
//!
//! Every scenario drives real split-runtime Quinn loopback (the Windows
//! constraint documented by the transport harness: one endpoint per thread),
//! pinned ephemeral test PKI, and a real migrated temporary [`ForgeApp`].
//! A broker thread owns the application, the repository-backed handler, and
//! sequential [`ForgeListener`] custody over Tokio channels, so the broker
//! runtime keeps driving Quinn while commands flow. The test runtime hosts
//! only client endpoints. Written tests are not executed evidence without an
//! explicit build lease.

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use artisan_backend::listener::ServeUntilCancelError;
use artisan_backend::{
    AuthenticationStageError, CommandOrigin, CommandOriginClockError, CommandOriginEntropyError,
    ForgeApp, ForgeConfig, ForgeListener, ForgeShutdownError, ListenerError, ListenerLimits,
    MetadataError, RequestTermination, ServiceReport,
};
use artisan_database::{AttachProjectInput, SqliteConfig};
use artisan_domain::{
    Command, CreateThread, DirectoryId, DisplayName, ListAttachedProjects, ListDirectories,
    ProjectId, Query, ReceiptDisposition, RequestId, RootPath, ThreadTitle, UnixMillis,
};
use artisan_protocol::{
    APPLICATION_PROTOCOL_VERSION, ClientRequest, FrameId, Hello, HelloCredential, LocalCapability,
    ProtocolVersion, ReconnectCapability, ResponsePayload, VersionOffer, WireEnvelope,
    WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, DeadlineError, EnvelopeReceiveError, FrameError, HandshakeError,
    LOOPBACK_SERVER_NAME, OperationKind, PinnedIdentity, ServerWelcome, TransportError,
    bind_loopback_client, client_config, client_handshake, receive_envelope, send_envelope,
};
use quinn::{
    Connection, ConnectionError, Endpoint, RecvStream, SendStream, TransportConfig, VarInt,
};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

/// Generous-but-bounded watchdog so a regression fails fast instead of
/// hanging the runner.
const WATCHDOG: Duration = Duration::from_secs(5);

/// Deterministic issued bootstrap fixture; successful admissions rotate real
/// system entropy behind the authority.
const INITIAL_CAPABILITY: [u8; 32] = [0xb7; 32];
/// Wrong-value bootstrap fixture rejected before any consumption.
const WRONG_VALUE_CAPABILITY: [u8; 32] = [0x5c; 32];

/// Close code shared by both release boundaries in this slice.
const RELEASE_CLOSE_CODE: u32 = 0x01;
/// Reason carried by the owned connection drop — the connection boundary,
/// distinct from the listener endpoint/guard boundary below.
const CONNECTION_RELEASE_REASON: &[u8] = b"forge connection released";
/// Reason carried by the listener's endpoint/pre-metadata-guard boundary.
const LISTENER_CLOSE_REASON: &[u8] = b"forge listener released";

// ---------------------------------------------------------------------------
// Fixtures: PKI, origins, envelopes, temporary database
// ---------------------------------------------------------------------------

struct TestPki {
    certificate: CertificateDer<'static>,
    private_key: PrivatePkcs8KeyDer<'static>,
}

fn test_pki() -> TestPki {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("valid SANs");
    TestPki {
        certificate: certified_key.cert.der().clone(),
        private_key: PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der()),
    }
}

fn test_server_config(pki: &TestPki) -> quinn::ServerConfig {
    artisan_transport::server_config(vec![pki.certificate.clone()], pki.private_key.clone_key())
        .expect("server configuration")
}

fn client_endpoint(pki: &TestPki) -> Endpoint {
    let pinned = PinnedIdentity::from_certificate(&pki.certificate);
    let config = client_config(pki.certificate.clone(), pinned).expect("client configuration");
    bind_loopback_client(config).expect("client bind")
}

/// Client endpoint whose sixteen-byte receive windows can never drain a full
/// server Welcome write: deterministic post-consumption stall evidence.
fn constrained_client_endpoint(pki: &TestPki) -> Endpoint {
    let pinned = PinnedIdentity::from_certificate(&pki.certificate);
    let mut config = client_config(pki.certificate.clone(), pinned).expect("client configuration");
    let mut transport = TransportConfig::default();
    transport.stream_receive_window(VarInt::from_u32(16));
    transport.receive_window(VarInt::from_u32(16));
    config.transport_config(Arc::new(transport));
    bind_loopback_client(config).expect("client bind")
}

/// Deterministic metadata origin: counter identities plus increasing instants.
///
/// `fetch_add` returns the previous value, so the first minted identity is
/// `forge-meta-0` and the first acceptance instant is exactly 1000 millis.
#[derive(Debug)]
struct SequencedOrigin {
    next: AtomicU64,
    millis: AtomicI64,
}

impl SequencedOrigin {
    fn boxed() -> Box<dyn CommandOrigin> {
        Box::new(Self {
            next: AtomicU64::new(0),
            millis: AtomicI64::new(1000),
        })
    }
}

impl CommandOrigin for SequencedOrigin {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        Ok(format!("forge-meta-{sequence}"))
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        let offset = self.millis.fetch_add(1, Ordering::Relaxed);
        Ok(UnixMillis::from_millis(offset))
    }
}

/// Origin whose metadata acquisition fails deterministically after TLS.
#[derive(Debug)]
struct FailingOrigin;

impl CommandOrigin for FailingOrigin {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        Err(CommandOriginEntropyError::from(
            getrandom::Error::UNEXPECTED,
        ))
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        Err(CommandOriginClockError)
    }
}

fn initial_credential() -> HelloCredential {
    HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY))
}

fn wrong_value_credential() -> HelloCredential {
    HelloCredential::Initial(LocalCapability::from_bytes(WRONG_VALUE_CAPABILITY))
}

fn hello_envelope(credential: HelloCredential) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("client-hello-frame").expect("valid fixture frame id"),
        sent_at: UnixMillis::from_millis(10),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![APPLICATION_PROTOCOL_VERSION])
                .expect("valid fixture version offer"),
            credential,
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

fn directory_browse_request(frame: &str) -> WireEnvelope {
    request_envelope(
        frame,
        ClientRequest::Query(Query::ListDirectories(ListDirectories { parent: None })),
    )
}

fn create_thread_command(frame: &str, title: &str) -> WireEnvelope {
    request_envelope(
        frame,
        ClientRequest::Command(Command::CreateThread(CreateThread {
            request_id: RequestId::parse(frame).expect("valid fixture request id"),
            project_id: ProjectId::parse("project-1").expect("valid fixture project id"),
            title: ThreadTitle::parse(title).expect("valid fixture title"),
        })),
    )
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

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TemporaryDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "artisan-forge-listener-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("temporary database directory created");
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
        let _cleanup = std::fs::remove_dir_all(&self.directory);
    }
}

// ---------------------------------------------------------------------------
// Split-runtime broker: owns app, handler, and sequential listener custody
// ---------------------------------------------------------------------------

enum BrokerCommand {
    Serve,
    CancelWork,
    Drain,
    DropUnpolledServe,
    DropPendingServe,
    ShutdownApp,
}

#[derive(Debug)]
enum BrokerReply {
    Bound(SocketAddr),
    StartupFailed(String),
    Served(Result<ServiceReport, ListenerError>),
    Drained(Result<(), TransportError>),
    ServeFutureDropped,
    AppClosed(Result<(), ForgeShutdownError>),
}

/// Private cleanup owner for the supervised broker thread, installed
/// immediately after spawn and before any fallible startup await. It owns
/// the ONLY command sender and the thread owner: releasing it closes the
/// command channel (which cancels and settles outstanding work inside
/// supervision, reaching the shared awaited drain -> handler -> storage
/// shutdown tail), then observes the explicit post-runtime-teardown signal
/// and the finished thread handle BEFORE the blocking join. A timeout is a
/// surfaced failure with an explicit detach report, never termination
/// proof. The release is one-shot: a later call finds the state consumed
/// and does nothing, and failures are returned so unwind paths can log
/// them without causing a double-panic abort.
struct BrokerGuard {
    commands: Option<tokio::sync::mpsc::UnboundedSender<BrokerCommand>>,
    done: std::sync::mpsc::Receiver<()>,
    handle: Option<JoinHandle<()>>,
}

impl BrokerGuard {
    /// One-shot release: closes the sole command sender, waits boundedly for
    /// the explicit completion signal and the finished handle, and only then
    /// joins.
    fn bounded_complete(&mut self, context: &str) -> Result<(), String> {
        // Taking the handle exactly once makes every later call (for
        // example the guard Drop running after an explicit completion) an
        // idempotent no-op instead of a repeated wait.
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        // The guard owns the ONLY sender, so this drop truly closes the
        // command channel: supervision cancels/reclaims outstanding work and
        // runs its awaited drain -> handler -> storage shutdown tail.
        drop(self.commands.take());
        // The broker thread sends this signal only after dropping its
        // runtime, so receiving it means user work AND runtime teardown
        // finished. Disconnection without a signal means the thread unwound;
        // the join outcome below preserves that panic. A timeout surfaces as
        // failure and reports the detach explicitly — it is never treated
        // as termination.
        let signalled = match self.done.recv_timeout(WATCHDOG) {
            Ok(()) => true,
            Err(RecvTimeoutError::Disconnected) => false,
            Err(RecvTimeoutError::Timeout) => {
                drop(handle);
                return Err(format!(
                    "{context}: broker thread still running after {WATCHDOG:?}; \
                     detaching it without joining"
                ));
            }
        };
        // Finished-handle predicate before any blocking join: the thread
        // body (including runtime teardown or panic unwind) must be
        // observed complete under its own bound first.
        let finish_deadline = Instant::now() + WATCHDOG;
        while !handle.is_finished() {
            if Instant::now() >= finish_deadline {
                drop(handle);
                return Err(format!(
                    "{context}: broker thread did not finish within {WATCHDOG:?} \
                     after its completion window; detaching it without joining"
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // After is_finished, the join awaits only final OS thread teardown —
        // a scheduler-level wait, not a hard real-time guarantee.
        match handle.join() {
            Ok(()) if signalled => Ok(()),
            Ok(()) => Err(format!(
                "{context}: broker thread exited without its completion signal"
            )),
            Err(_panic) => Err(format!("{context}: supervised broker thread panicked")),
        }
    }

    /// Normal-path completion: runs the one-shot release and surfaces any
    /// cleanup failure to the caller (no unwind is active here). The
    /// subsequent guard Drop finds the state consumed and does nothing.
    fn complete(mut self, context: &str) -> Result<(), String> {
        self.bounded_complete(context)
    }
}

impl Drop for BrokerGuard {
    fn drop(&mut self) {
        // Unwind / dropped-test-side path: identical one-shot bounded
        // release, with failures logged instead of re-panicked so a primary
        // panic or error is preserved without a double-panic abort.
        if let Err(failure) = self.bounded_complete("dropped test side") {
            eprintln!("broker cleanup failure on dropped test side: {failure}");
        }
    }
}

struct Broker {
    replies: tokio::sync::mpsc::UnboundedReceiver<BrokerReply>,
    addr: SocketAddr,
    guard: BrokerGuard,
}

impl Broker {
    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn send(&self, command: BrokerCommand) {
        // Sole sender custody lives in the guard until completion; every
        // send happens strictly before the guard is released.
        self.guard
            .commands
            .as_ref()
            .expect("command custody held until completion")
            .send(command)
            .expect("broker alive");
    }

    fn begin_serve(&self) {
        self.send(BrokerCommand::Serve);
    }

    async fn reply(&mut self) -> BrokerReply {
        match tokio::time::timeout(WATCHDOG, self.replies.recv()).await {
            Ok(Some(reply)) => reply,
            Ok(None) => panic!("broker thread died"),
            Err(elapsed) => panic!("broker reply exceeded the watchdog: {elapsed}"),
        }
    }

    async fn await_served(&mut self) -> Result<ServiceReport, ListenerError> {
        match self.reply().await {
            BrokerReply::Served(outcome) => outcome,
            other => panic!("unexpected broker reply: {other:?}"),
        }
    }

    async fn drained(&mut self) {
        self.send(BrokerCommand::Drain);
        assert!(
            matches!(self.reply().await, BrokerReply::Drained(Ok(()))),
            "awaited drain proves endpoint idle"
        );
    }

    async fn shutdown_app(mut self) {
        self.send(BrokerCommand::ShutdownApp);
        assert!(
            matches!(self.reply().await, BrokerReply::AppClosed(Ok(()))),
            "caller-owned application shutdown must succeed"
        );
        // Bounded completion observed before the join; a timeout or thread
        // panic surfaces as its own failure without masking the primary
        // AppClosed assertion above.
        if let Err(failure) = self.guard.complete("shutdown_app") {
            panic!("broker cleanup failed after app shutdown: {failure}");
        }
    }
}

/// Bounded non-completion window for the two owning-future drop scenarios.
const PENDING_WINDOW: Duration = Duration::from_millis(600);

/// Owns one listener through sequential serve cycles. Cancels any
/// outstanding serve when the test side disappears, finishes that future to
/// reclaim custody, drains the returned listener, and returns — leaving
/// handler and application shutdown to the awaiting caller.
async fn supervise_listener<Fut>(
    listener: ForgeListener,
    work_cancel: Arc<CancelHandle>,
    mut start_serve: impl FnMut(ForgeListener) -> Pin<Box<Fut>>,
    mut command_rx: tokio::sync::mpsc::UnboundedReceiver<BrokerCommand>,
    reply_tx: tokio::sync::mpsc::UnboundedSender<BrokerReply>,
) where
    Fut: Future<Output = Result<(ForgeListener, ServiceReport), ListenerError>>,
{
    // Owning service slot: while `Some`, the pinned future owns the listener
    // and borrows `handler` for exactly this function's lifetime; clearing
    // the slot ends those borrows. Every reply send is best-effort, so a
    // dropped test receiver can never panic here before cleanup runs.
    let mut active: Option<Pin<Box<Fut>>> = None;
    let mut custody: Option<ForgeListener> = Some(listener);
    loop {
        if let Some(future) = active.as_mut() {
            let settled = loop {
                tokio::select! {
                    outcome = future.as_mut() => break Some(outcome),
                    maybe_command = command_rx.recv() => match maybe_command {
                        // The test side disappeared while a serve was
                        // outstanding: cancel it, finish the future to
                        // reclaim the listener, then stop supervising.
                        None => {
                            work_cancel.cancel();
                            if let Ok((returned, _report)) = future.as_mut().await {
                                custody = Some(returned);
                            }
                            break None;
                        }
                        Some(BrokerCommand::CancelWork) => work_cancel.cancel(),
                        Some(_unexpected) => panic!(
                            "no other commands are expected while a client is being served"
                        ),
                    },
                }
            };
            match settled {
                Some(Ok((returned, report))) => {
                    custody = Some(returned);
                    let _ignored = reply_tx.send(BrokerReply::Served(Ok(report)));
                }
                Some(Err(error)) => {
                    let _ignored = reply_tx.send(BrokerReply::Served(Err(error)));
                }
                None => {
                    // The test-side close was fully handled in the select arm
                    // above (cancelled, reclaimed); nothing remains to report.
                }
            }
            active = None;
            continue;
        }

        match command_rx.recv().await {
            // Explicit shutdown and test-side closure share this one exit
            // into the custody-release tail below.
            None | Some(BrokerCommand::ShutdownApp) => break,
            Some(BrokerCommand::Serve) => {
                let owner = custody.take().expect("custody held between serves");
                active = Some(start_serve(owner));
            }
            Some(BrokerCommand::Drain) => {
                let owner = custody.take().expect("custody held for drain");
                let outcome = owner.drain().await;
                let _ignored = reply_tx.send(BrokerReply::Drained(outcome));
            }
            Some(BrokerCommand::DropUnpolledServe) => {
                let owner = custody.take().expect("custody held for the owning future");
                drop(start_serve(owner));
                let _ignored = reply_tx.send(BrokerReply::ServeFutureDropped);
            }
            Some(BrokerCommand::DropPendingServe) => {
                let owner = custody.take().expect("custody held for the owning future");
                let mut future = start_serve(owner);
                // Expiry of this window guards that the owning future is
                // genuinely uncompleted when dropped; the cleanup claim
                // itself is witnessed by peer behavior in the driving test.
                assert!(
                    tokio::time::timeout(PENDING_WINDOW, future.as_mut())
                        .await
                        .is_err(),
                    "serve must remain pending without any peer",
                );
                drop(future);
                let _ignored = reply_tx.send(BrokerReply::ServeFutureDropped);
            }
            Some(BrokerCommand::CancelWork) => work_cancel.cancel(),
        }
    }

    // Release/drain whatever custody remains (bounded by the listener's own
    // drain limit). Primary failures reported earlier are never masked.
    if let Some(owner) = custody.take() {
        let _best_effort_drain = owner.drain().await;
    }
}

/// Waits boundedly for the broker's bound address while holding the private
/// owner; every non-Bound outcome releases custody through the guard first
/// and then surfaces the primary startup failure.
async fn await_bound_addr(
    replies: &mut tokio::sync::mpsc::UnboundedReceiver<BrokerReply>,
    guard: BrokerGuard,
) -> (SocketAddr, BrokerGuard) {
    loop {
        match tokio::time::timeout(WATCHDOG, replies.recv()).await {
            Ok(Some(BrokerReply::Bound(addr))) => break (addr, guard),
            Ok(Some(BrokerReply::StartupFailed(message))) => {
                // Bounded cleanup completes before the primary startup
                // failure is surfaced verbatim.
                if let Err(failure) = guard.complete("startup-failure") {
                    eprintln!("broker cleanup failure after startup failure: {failure}");
                }
                panic!("broker startup failed: {message}")
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(failure) = guard.complete("startup-death") {
                    eprintln!("broker cleanup failure after thread death: {failure}");
                }
                panic!("broker died before binding")
            }
            Err(_elapsed) => {
                // The startup wait expired: release test-side ownership and
                // observe bounded completion, then surface both outcomes —
                // the watchdog expiry stays the primary failure.
                let cleanup = guard.complete("startup-watchdog");
                panic!("listener must bind under the watchdog; cleanup outcome: {cleanup:?}")
            }
        }
    }
}

async fn start_broker(
    label: &str,
    seed_attach: Option<AttachProjectInput>,
    bootstrap: LocalCapability,
    origin: Box<dyn CommandOrigin>,
    limits: ListenerLimits,
    admission_capacity: NonZeroU32,
    requests_per_connection: NonZeroU32,
) -> (Broker, TemporaryDatabase, TestPki) {
    let temporary = TemporaryDatabase::new(label);
    let pki = test_pki();
    let config = test_server_config(&pki);
    let database_path = temporary.path().to_path_buf();
    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel::<BrokerCommand>();
    let (reply_tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel::<BrokerReply>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();

    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("broker runtime");
        runtime.block_on(async move {
            let app = match ForgeApp::start(ForgeConfig::new(
                SqliteConfig::file(&database_path).sqlx_logging(false),
            ))
            .await
            {
                Ok(app) => app,
                Err(error) => {
                    let _ignored = reply_tx.send(BrokerReply::StartupFailed(format!(
                        "migrated Forge application should start: {error}"
                    )));
                    return;
                }
            };

            // Once the application exists, every later startup failure keeps
            // it available to the single awaited consuming-shutdown path in
            // this error arm — nothing is dropped unawaited here.
            let remainder = async {
                if let Some(input) = seed_attach {
                    app.repository()
                        .attach_project(input)
                        .await
                        .map_err(|error| format!("seed attach should persist: {error}"))?;
                }
                let work_cancel = Arc::new(CancelHandle::new());
                let handler = artisan_backend::RequestHandler::new(app.repository().clone());
                let listener = ForgeListener::bind(
                    config,
                    bootstrap,
                    origin,
                    limits,
                    admission_capacity,
                    requests_per_connection,
                )
                .map_err(|error| format!("listener should bind: {error}"))?;
                let bound_addr = listener
                    .local_addr()
                    .map_err(|error| format!("bound address observable: {error}"))?;
                Ok((work_cancel, handler, listener, bound_addr))
            }
            .await;

            let (work_cancel, handler, listener, bound_addr) = match remainder {
                Ok(ready) => ready,
                Err(message) => {
                    // Awaited consuming storage shutdown on this error path.
                    let _storage_shutdown = app.shutdown().await;
                    let _ignored = reply_tx.send(BrokerReply::StartupFailed(message));
                    return;
                }
            };
            let _ignored = reply_tx.send(BrokerReply::Bound(bound_addr));

            // Supervision owns listener/handler interaction end to end and
            // releases everything (including bounded drain) before returning,
            // on normal, cancelled, and dropped-test-side exits alike.
            supervise_listener(
                listener,
                work_cancel.clone(),
                |listener| Box::pin(listener.serve_one(&handler, work_cancel.as_ref())),
                command_rx,
                reply_tx.clone(),
            )
            .await;
            drop(handler);

            // Consuming storage shutdown: awaited exactly once, after every
            // listener boundary has been released above.
            let app_outcome = app.shutdown().await;
            let _ignored = reply_tx.send(BrokerReply::AppClosed(app_outcome));
        });
        // Runtime teardown belongs inside the completion boundary: drop it
        // before the explicit signal, so receiving the signal proves the
        // runtime is gone. Any panic above unwinds past this send and drops
        // the sender unsignalled, which the guard reads as failure.
        drop(runtime);
        let _receiver_may_be_gone = done_tx.send(());
    });

    // The private cleanup owner takes the ONLY command sender and the thread
    // owner immediately after spawn, before the first fallible startup
    // await: startup failure, watchdog expiry, a dropped setup future, and
    // assertion unwind all release custody through the same one-shot path.
    let guard = BrokerGuard {
        commands: Some(command_tx),
        done: done_rx,
        handle: Some(handle),
    };

    let (addr, guard) = await_bound_addr(&mut reply_rx, guard).await;

    (
        Broker {
            replies: reply_rx,
            addr,
            guard,
        },
        temporary,
        pki,
    )
}

// ---------------------------------------------------------------------------
// Client fixtures on the test runtime
// ---------------------------------------------------------------------------

struct AuthenticatedClient {
    connection: Connection,
    welcome: ServerWelcome,
}

async fn try_admit(
    client: &Endpoint,
    addr: SocketAddr,
    credential: HelloCredential,
) -> Result<AuthenticatedClient, HandshakeError> {
    let connecting = client
        .connect(addr, LOOPBACK_SERVER_NAME)
        .expect("connect accepted");
    let connection = tokio::time::timeout(WATCHDOG, connecting)
        .await
        .expect("connection settles under the watchdog")
        .expect("connection established");
    let (mut control_send, mut control_recv) =
        connection.open_bi().await.expect("control stream opens");
    let welcome = tokio::time::timeout(
        WATCHDOG,
        client_handshake(
            &mut control_send,
            &mut control_recv,
            hello_envelope(credential),
        ),
    )
    .await
    .expect("handshake settles under the watchdog")?;
    drop(control_send);
    drop(control_recv);
    Ok(AuthenticatedClient {
        connection,
        welcome,
    })
}

async fn admit(
    client: &Endpoint,
    addr: SocketAddr,
    credential: HelloCredential,
) -> AuthenticatedClient {
    try_admit(client, addr, credential)
        .await
        .expect("bootstrap admission succeeds")
}

/// Writes only the Hello and keeps the control stream open: the caller then
/// drives a deterministically blocked Welcome write.
async fn send_hello_only(
    client: &Endpoint,
    addr: SocketAddr,
    credential: HelloCredential,
) -> (Connection, SendStream, RecvStream) {
    let connecting = client
        .connect(addr, LOOPBACK_SERVER_NAME)
        .expect("connect accepted");
    let connection = tokio::time::timeout(WATCHDOG, connecting)
        .await
        .expect("connection settles under the watchdog")
        .expect("connection established");
    let (mut control_send, control_recv) =
        connection.open_bi().await.expect("control stream opens");
    send_envelope(&mut control_send, &hello_envelope(credential))
        .await
        .expect("hello crosses the wire");
    (connection, control_send, control_recv)
}

/// Writes one request without reading any reply and hands back the UNREAD
/// stream owner: Quinn's `RecvStream` implicitly sends stop(0) when dropped,
/// so the caller must retain it until after the typed budget report to keep
/// that stop from racing the server's local finish.
async fn write_unread_budget_request(
    client: &AuthenticatedClient,
    request: &WireEnvelope,
) -> RecvStream {
    let (mut send, recv) = client
        .connection
        .open_bi()
        .await
        .expect("budget stream opens");
    send_envelope(&mut send, request)
        .await
        .expect("budget request written");
    drop(send);
    recv
}

/// Exchanges one request, asserting the fresh server stamp verbatim plus the
/// correlated request identity on both reply families against the triggering
/// frame.
async fn exchange_envelope(
    client: &AuthenticatedClient,
    request: &WireEnvelope,
    expected_frame: &str,
    expected_sent_at: UnixMillis,
) -> WireEnvelope {
    let (mut send, mut recv) = client.connection.open_bi().await.expect("request stream");
    send_envelope(&mut send, request)
        .await
        .expect("request crosses the wire");
    let reply = tokio::time::timeout(WATCHDOG, receive_envelope(&mut recv))
        .await
        .expect("reply arrives under the watchdog")
        .expect("reply decodes");
    assert_eq!(
        reply.frame_id.as_str(),
        expected_frame,
        "fresh server stamp identity"
    );
    assert_eq!(reply.sent_at, expected_sent_at, "fresh acceptance instant");
    let trigger_frame = request.frame_id.as_str();
    let expected_correlation =
        RequestId::parse(trigger_frame).expect("valid fixture correlation id");
    match &reply.body {
        WireEnvelopeBody::Response(response) => assert_eq!(
            response.request_id.as_str(),
            trigger_frame,
            "response correlation must name the triggering frame"
        ),
        WireEnvelopeBody::ProtocolError(failure) => assert_eq!(
            failure.request_id.as_ref(),
            Some(&expected_correlation),
            "typed failure correlation must name the triggering frame"
        ),
        _other => panic!("unexpected reply family"),
    }
    // Clean-FIN fence for a completed response: exactly one correlated reply
    // followed by the server's finished send side and nothing else.
    let stream_end = tokio::time::timeout(WATCHDOG, receive_envelope(&mut recv))
        .await
        .expect("stream end arrives under the watchdog");
    match stream_end {
        Err(EnvelopeReceiveError::Frame(FrameError::Truncated {
            expected: 4,
            received: 0,
        })) => {}
        Err(other) => {
            panic!("expected clean end-of-stream behind one reply, got {other:?}")
        }
        Ok(extra) => panic!(
            "expected clean end-of-stream behind one reply, got an extra frame {}",
            extra.frame_id.as_str()
        ),
    }
    drop(send);
    drop(recv);
    reply
}

async fn exchange_response(
    client: &AuthenticatedClient,
    request: &WireEnvelope,
    expected_frame: &str,
    expected_sent_at: UnixMillis,
) -> ResponsePayload {
    let reply = exchange_envelope(client, request, expected_frame, expected_sent_at).await;
    match reply.body {
        WireEnvelopeBody::Response(response) => response.payload,
        WireEnvelopeBody::ProtocolError(failure) => {
            panic!("unexpected correlated failure: {:?}", failure.code)
        }
        _other => panic!("unexpected reply family"),
    }
}

/// Asserts the peer observes one specific application-release boundary: the
/// owned connection drop or the listener's guard/endpoint boundary.
async fn expect_application_close(connection: &Connection, reason: &[u8]) {
    let closed = tokio::time::timeout(WATCHDOG, connection.closed())
        .await
        .expect("closure arrives within the watchdog");
    match closed {
        ConnectionError::ApplicationClosed(close) => {
            assert_eq!(close.error_code, VarInt::from_u32(RELEASE_CLOSE_CODE));
            assert_eq!(close.reason.as_ref(), reason, "boundary-specific reason");
        }
        unexpected => panic!("expected an application close, got {unexpected:?}"),
    }
}

/// Witness that nothing establishes against the closed endpoint: bounded
/// refusal or bounded timeout both prove non-establishment.
async fn assert_connect_fails(client: &Endpoint, addr: SocketAddr) {
    let connecting = client
        .connect(addr, LOOPBACK_SERVER_NAME)
        .expect("connect accepted");
    match tokio::time::timeout(WATCHDOG, connecting).await {
        Err(_elapsed) => {}
        Ok(Err(_failure)) => {}
        Ok(Ok(_established)) => {
            panic!("connection must not establish against a closed endpoint")
        }
    }
}

fn default_limits(admission: Duration) -> ListenerLimits {
    ListenerLimits {
        admission,
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
        drain: Duration::from_secs(2),
    }
}

fn capacity(count: u32) -> NonZeroU32 {
    NonZeroU32::new(count).expect("fixture capacities are nonzero")
}

fn assert_budget(report: &ServiceReport, completed: u32) {
    assert_eq!(report.completed_requests, completed);
    assert!(matches!(
        report.termination,
        RequestTermination::BudgetReached
    ));
}

fn assert_no_secret_diagnostics(error: &ListenerError) {
    let rendered = format!("{error}{error:?}");
    assert!(!rendered.contains("b7b7"), "bootstrap material leaked");
    assert!(!rendered.contains("5c5c"), "wrong-value material leaked");
}

// ---------------------------------------------------------------------------
// Coverage — service is always driven before any client connect
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bootstrap_listing_and_durable_command_reopen_replay() {
    let (mut broker, temporary, pki) = start_broker(
        "bootstrap",
        Some(attach_input()),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_secs(2)),
        capacity(4),
        capacity(3),
    )
    .await;
    let client_ep = client_endpoint(&pki);

    // Spare capacity (3) covers the two reply-required exchanges below; the
    // causal subsequent termination ends this connection instead of a budget
    // closure racing the second observed reply.
    broker.begin_serve();
    let client = admit(&client_ep, broker.addr(), initial_credential()).await;
    // Injected metadata verbatim: identity mint zero, Welcome stamp mint one
    // carrying the FIRST acceptance instant (1000).
    assert_eq!(
        client.welcome.welcome.connection_id.as_str(),
        "forge-meta-0",
        "validated connection identity"
    );
    assert_eq!(client.welcome.frame_id.as_str(), "forge-meta-1");
    assert_eq!(client.welcome.sent_at, UnixMillis::from_millis(1000));
    assert_eq!(
        client.welcome.welcome.negotiated_version,
        ProtocolVersion::V1
    );

    let payload = exchange_response(
        &client,
        &list_projects_request("frame-listing"),
        "forge-meta-2",
        UnixMillis::from_millis(1001),
    )
    .await;
    let ResponsePayload::ProjectListing(listing) = payload else {
        panic!("expected a correlated project listing");
    };
    assert_eq!(listing.projects().len(), 1, "seeded catalog lists");

    let payload = exchange_response(
        &client,
        &create_thread_command("request-thread-wire", "Wire Thread"),
        "forge-meta-3",
        UnixMillis::from_millis(1002),
    )
    .await;
    let ResponsePayload::CreatedThread { disposition, .. } = payload else {
        panic!("expected a created-thread receipt");
    };
    assert_eq!(disposition, ReceiptDisposition::Accepted);

    // Causal termination with spare capacity: both replies above were
    // observed by this client before the work-cancel command fired.
    broker.send(BrokerCommand::CancelWork);
    let report = broker.await_served().await.expect("custody returns");
    assert_eq!(report.completed_requests, 2);
    assert!(matches!(
        report.termination,
        RequestTermination::Failed {
            source: DeadlineError::Cancelled {
                operation: OperationKind::Receive,
            },
        }
    ));
    expect_application_close(&client.connection, CONNECTION_RELEASE_REASON).await;
    drop(client);
    broker.drained().await;
    broker.shutdown_app().await;

    // Durable reopen on the caller side replays the wire-accepted receipt.
    let reopened = ForgeApp::start(ForgeConfig::new(
        SqliteConfig::file(temporary.path()).sqlx_logging(false),
    ))
    .await
    .expect("storage reopens");
    let replay = reopened
        .repository()
        .lookup_create_thread(
            &RequestId::parse("request-thread-wire").expect("valid request id"),
            &ProjectId::parse("project-1").expect("valid project id"),
            &ThreadTitle::parse("Wire Thread").expect("valid title"),
        )
        .await
        .expect("receipt lookup works")
        .expect("accepted receipt survives reopen");
    assert_eq!(replay.receipt.disposition, ReceiptDisposition::Duplicate);
    reopened.shutdown().await.expect("reopen shuts down");
}

#[tokio::test]
async fn budget_completion_reconnects_without_delivery_assumptions() {
    let (mut broker, _temporary, pki) = start_broker(
        "budget-reconnect",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_secs(2)),
        capacity(4),
        capacity(1),
    )
    .await;
    let client_ep = client_endpoint(&pki);

    // Leg one: the single request is written and locally finished by the
    // server; its reply is deliberately never read, and completion is judged
    // only by the typed report.
    broker.begin_serve();
    let first = admit(&client_ep, broker.addr(), initial_credential()).await;
    let unread_budget_a =
        write_unread_budget_request(&first, &list_projects_request("frame-budget-a")).await;
    let report = broker.await_served().await.expect("custody returns");
    assert_budget(&report, 1);
    drop(unread_budget_a);
    expect_application_close(&first.connection, CONNECTION_RELEASE_REASON).await;
    // ReconnectCapability is non-Clone: move it out of the nested field; the
    // remaining client fields drop normally at the end of the scope.
    let rotated = first.welcome.welcome.reconnect_capability;

    // Leg two reconnects with the rotated secret and completes identically.
    broker.begin_serve();
    let second = admit(
        &client_ep,
        broker.addr(),
        HelloCredential::Reconnect(rotated),
    )
    .await;
    let unread_budget_b =
        write_unread_budget_request(&second, &directory_browse_request("frame-budget-b")).await;
    let report = broker.await_served().await.expect("custody returns again");
    assert_budget(&report, 1);
    drop(unread_budget_b);
    expect_application_close(&second.connection, CONNECTION_RELEASE_REASON).await;
    drop(second);
    broker.shutdown_app().await;
}

#[tokio::test]
async fn post_ready_failure_returns_listener_then_reconnects_rotated() {
    let (mut broker, _temporary, pki) = start_broker(
        "post-ready",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_secs(2)),
        capacity(4),
        capacity(8),
    )
    .await;
    let client_ep = client_endpoint(&pki);

    broker.begin_serve();
    let first = admit(&client_ep, broker.addr(), initial_credential()).await;
    // ReconnectCapability is deliberately non-Clone: move it out of the
    // nested welcome field; sibling fields remain usable afterwards.
    let rotated_once = first.welcome.welcome.reconnect_capability;
    first.connection.close(VarInt::from_u32(0x02), b"peer done");
    let report = broker.await_served().await.expect("custody returns");
    assert_eq!(report.completed_requests, 0);
    assert!(matches!(
        report.termination,
        RequestTermination::Failed {
            source: DeadlineError::Peer {
                operation: OperationKind::Receive,
                error: artisan_backend::RequestStageError::Accept { .. },
            },
        }
    ));
    // `first` drops at scope end; only its rotated secret was moved out.

    // Reconnect with the rotated secret: two reply-required exchanges with
    // wide spare capacity (a success and a typed wire failure that still
    // counts as a completed dispatch), then causal cancellation.
    broker.begin_serve();
    let second = admit(
        &client_ep,
        broker.addr(),
        HelloCredential::Reconnect(rotated_once),
    )
    .await;
    exchange_response(
        &second,
        &list_projects_request("frame-reconnected"),
        "forge-meta-5",
        UnixMillis::from_millis(1003),
    )
    .await;
    let failure_reply = exchange_envelope(
        &second,
        &directory_browse_request("request-browse-unbacked"),
        "forge-meta-6",
        UnixMillis::from_millis(1004),
    )
    .await;
    assert!(
        matches!(failure_reply.body, WireEnvelopeBody::ProtocolError(_)),
        "typed wire failures count as completed dispatches"
    );
    broker.send(BrokerCommand::CancelWork);
    let report = broker.await_served().await.expect("custody returns again");
    assert_eq!(report.completed_requests, 2);
    assert!(matches!(
        report.termination,
        RequestTermination::Failed {
            source: DeadlineError::Cancelled {
                operation: OperationKind::Receive,
            },
        }
    ));
    expect_application_close(&second.connection, CONNECTION_RELEASE_REASON).await;
    drop(second);
    broker.shutdown_app().await;
}

#[tokio::test]
async fn admission_exhaustion_precedes_any_further_accept() {
    let (mut broker, _temporary, pki) = start_broker(
        "exhaustion",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_secs(2)),
        capacity(1),
        capacity(1),
    )
    .await;
    let client_ep = client_endpoint(&pki);

    broker.begin_serve();
    let only = admit(&client_ep, broker.addr(), initial_credential()).await;
    let unread_only =
        write_unread_budget_request(&only, &list_projects_request("frame-only")).await;
    let report = broker.await_served().await.expect("custody returns");
    assert_budget(&report, 1);
    drop(unread_only);
    expect_application_close(&only.connection, CONNECTION_RELEASE_REASON).await;
    drop(only);

    // Capacity is already zero when this serve begins: exhaustion must be
    // decided before touching the endpoint, and dropping the terminal owner
    // closes the endpoint the eager peer was about to use.
    broker.begin_serve();
    let queued = client_ep
        .connect(broker.addr(), LOOPBACK_SERVER_NAME)
        .expect("connect accepted");
    let outcome = broker.await_served().await;
    assert!(matches!(
        outcome,
        Err(ListenerError::AdmissionCapacityExhausted)
    ));
    match tokio::time::timeout(WATCHDOG, queued).await {
        Err(_elapsed) => {}
        Ok(Err(_refused)) => {}
        Ok(Ok(_established)) => panic!("queued peer must never establish"),
    }
    broker.shutdown_app().await;
}

#[tokio::test]
async fn invalid_credential_is_terminal_without_secret_diagnostics() {
    let (mut broker, _temporary, pki) = start_broker(
        "invalid-credential",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_secs(2)),
        capacity(4),
        capacity(4),
    )
    .await;
    let client_ep = client_endpoint(&pki);

    broker.begin_serve();
    let rejected = try_admit(&client_ep, broker.addr(), wrong_value_credential()).await;
    assert!(
        rejected.is_err(),
        "a wrong-value credential never receives a Welcome"
    );
    let error = broker
        .await_served()
        .await
        .expect_err("authentication failure is terminal");
    assert!(matches!(
        error,
        ListenerError::Authentication {
            source: DeadlineError::Peer {
                operation: OperationKind::Handshake,
                error: AuthenticationStageError::Credential(_),
            },
        }
    ));
    assert_no_secret_diagnostics(&error);

    // Terminal ownership: the listener dropped its endpoint, so no retry or
    // further connection is possible against this owner.
    assert_connect_fails(&client_ep, broker.addr()).await;
    broker.shutdown_app().await;
}

#[tokio::test]
async fn metadata_failure_after_tls_closes_the_established_peer() {
    let (mut broker, _temporary, pki) = start_broker(
        "metadata-failure",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        Box::new(FailingOrigin),
        default_limits(Duration::from_secs(2)),
        capacity(4),
        capacity(4),
    )
    .await;
    let client_ep = client_endpoint(&pki);

    broker.begin_serve();
    // Manual client half so the Connection survives whatever the failing
    // metadata close does to the handshake: an established-TLS peer close may
    // legitimately surface while opening the control stream, sending the
    // Hello, or awaiting the Welcome, and no single stage is required to be
    // the one that observes it.
    let connecting = client_ep
        .connect(broker.addr(), LOOPBACK_SERVER_NAME)
        .expect("connect accepted");
    let connection = tokio::time::timeout(WATCHDOG, connecting)
        .await
        .expect("TLS establishment completes under the watchdog")
        .expect("QUIC/TLS connection established");
    let client_half = async {
        // An established-TLS peer close may legitimately land here too; `?`
        // surfaces it as the typed client-half error instead of pretending
        // the stream always opens.
        let (mut control_send, mut control_recv) = connection.open_bi().await?;
        client_handshake(
            &mut control_send,
            &mut control_recv,
            hello_envelope(initial_credential()),
        )
        .await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let handshake = tokio::time::timeout(WATCHDOG, client_half)
        .await
        .expect("client half settles under the watchdog");
    // Any typed error is acceptable: the guarded close may surface while
    // opening, sending, or receiving. A Welcome can never arrive.
    assert!(
        handshake.is_err(),
        "failing metadata must prevent the Welcome"
    );

    // Required typed evidence on the actual established peer: the guarded
    // listener boundary closed it with the fixed code/reason.
    expect_application_close(&connection, LISTENER_CLOSE_REASON).await;

    let error = broker
        .await_served()
        .await
        .expect_err("metadata failure is terminal");
    assert!(matches!(
        error,
        ListenerError::Metadata {
            source: MetadataError::Entropy { .. },
        }
    ));
    assert_no_secret_diagnostics(&error);
    broker.shutdown_app().await;
}

#[tokio::test]
async fn unrepresentable_zero_and_positive_admission_bounds_behave_typed() {
    // Unrepresentable limits are rejected before any endpoint bind. Match
    // explicitly: ForgeListener deliberately implements neither Clone nor
    // Debug, so expect_err is unavailable on its success type.
    match ForgeListener::bind(
        test_server_config(&test_pki()),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        ListenerLimits {
            admission: Duration::MAX,
            handshake: Duration::from_secs(2),
            next_request: Duration::from_secs(2),
            drain: Duration::from_secs(2),
        },
        capacity(4),
        capacity(4),
    ) {
        Err(ListenerError::UnrepresentableLimits) => {}
        Err(other) => panic!("expected unrepresentable-limits error, got {other:?}"),
        Ok(_listener) => panic!("Duration::MAX must not produce a listener"),
    }

    // Zero admission is valid configuration: the typed timeout is decided
    // before the bounded stage is polled (precedence evidence only, never
    // in-flight proof).
    let (mut zero_broker, _temporary_zero, _zero_pki) = start_broker(
        "zero-admission",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::ZERO),
        capacity(4),
        capacity(4),
    )
    .await;
    let outcome = {
        zero_broker.begin_serve();
        zero_broker.await_served().await
    };
    assert!(matches!(
        outcome,
        Err(ListenerError::Admission {
            source: DeadlineError::Timeout {
                operation: OperationKind::Connect,
                ..
            },
        })
    ));
    zero_broker.shutdown_app().await;

    // Positive admission with no peer genuinely waits in flight: the bounded
    // accept-and-TLS operation itself expires.
    let (mut positive_broker, _temporary_positive, _positive_pki) = start_broker(
        "positive-admission",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_millis(400)),
        capacity(4),
        capacity(4),
    )
    .await;
    let outcome = {
        positive_broker.begin_serve();
        positive_broker.await_served().await
    };
    assert!(matches!(
        outcome,
        Err(ListenerError::Admission {
            source: DeadlineError::Timeout {
                operation: OperationKind::Connect,
                ..
            },
        })
    ));
    positive_broker.shutdown_app().await;
}

#[tokio::test]
async fn cancelled_blocked_welcome_is_terminal_with_causal_prefix_witness() {
    let (mut broker, _temporary, pki) = start_broker(
        "blocked-welcome",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        ListenerLimits {
            admission: Duration::from_secs(2),
            handshake: Duration::from_secs(30),
            next_request: Duration::from_secs(2),
            drain: Duration::from_secs(2),
        },
        capacity(4),
        capacity(4),
    )
    .await;
    let client_ep = constrained_client_endpoint(&pki);

    broker.begin_serve();
    let (connection, control_send, mut control_recv) =
        send_hello_only(&client_ep, broker.addr(), initial_credential()).await;

    // Causal witness, not a timer: these are actual bytes of the server's
    // Welcome, so the credential was consumed and rotation staged while the
    // sixteen-byte receive window provably blocks the full write. A watchdog
    // expiry here fails the test; it can never be a success witness.
    let mut welcome_prefix = [0_u8; 8];
    tokio::time::timeout(WATCHDOG, control_recv.read_exact(&mut welcome_prefix))
        .await
        .expect("prefix arrives under the watchdog")
        .expect("prefix readable");

    broker.send(BrokerCommand::CancelWork);
    let error = broker
        .await_served()
        .await
        .expect_err("cancelled handshake is terminal");
    assert!(matches!(
        error,
        ListenerError::Authentication {
            source: DeadlineError::Cancelled {
                operation: OperationKind::Handshake,
            },
        }
    ));
    // The abandoned authentication drops its owning guards: peer observation
    // is the connection-release boundary.
    expect_application_close(&connection, CONNECTION_RELEASE_REASON).await;
    drop(control_send);
    drop(control_recv);
    drop(connection);
    broker.shutdown_app().await;
}

#[tokio::test]
async fn cancelled_idle_dispatch_wait_returns_custody_then_drains_idle() {
    let (mut broker, _temporary, pki) = start_broker(
        "idle-dispatch-cancel",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        ListenerLimits {
            admission: Duration::from_secs(2),
            handshake: Duration::from_secs(2),
            next_request: Duration::from_secs(30),
            drain: Duration::from_secs(2),
        },
        capacity(4),
        capacity(4),
    )
    .await;
    let client_ep = client_endpoint(&pki);

    broker.begin_serve();
    let client = admit(&client_ep, broker.addr(), initial_credential()).await;
    // The completed correlated response proves dispatch one fully finished
    // (locally accepted and FIN issued). Truthful boundary claim: the
    // cancellation then ends the sequential wait BETWEEN requests — no
    // stronger mid-receive stage is claimed, because no further client bytes
    // are in flight to witness one.
    exchange_response(
        &client,
        &list_projects_request("frame-warmup"),
        "forge-meta-2",
        UnixMillis::from_millis(1001),
    )
    .await;

    broker.send(BrokerCommand::CancelWork);
    let report = broker
        .await_served()
        .await
        .expect("custody returns after cancel");
    assert_eq!(report.completed_requests, 1);
    assert!(matches!(
        report.termination,
        RequestTermination::Failed {
            source: DeadlineError::Cancelled {
                operation: OperationKind::Receive,
            },
        }
    ));
    expect_application_close(&client.connection, CONNECTION_RELEASE_REASON).await;
    drop(client);

    // Cancellation is sticky, yet teardown cannot skip cleanup: the awaited
    // drain still proves endpoint idle before handler/storage teardown.
    broker.drained().await;
    broker.shutdown_app().await;
}

#[tokio::test]
async fn unpolled_owning_serve_future_drop_closes_the_endpoint_locally() {
    let (mut broker, _temporary, pki) = start_broker(
        "unpolled-drop",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_secs(2)),
        capacity(4),
        capacity(4),
    )
    .await;
    let client_ep = client_endpoint(&pki);

    broker.send(BrokerCommand::DropUnpolledServe);
    assert!(matches!(
        broker.reply().await,
        BrokerReply::ServeFutureDropped
    ));

    // Drop executed inside the broker (local closure); the observable peer
    // consequence is non-establishment under the bounded watchdog.
    assert_connect_fails(&client_ep, broker.addr()).await;
    broker.shutdown_app().await;
}

#[tokio::test]
async fn pending_owning_serve_future_drop_abandons_admission() {
    let (mut broker, _temporary, pki) = start_broker(
        "pending-drop",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_secs(2)),
        capacity(4),
        capacity(4),
    )
    .await;
    let client_ep = client_endpoint(&pki);

    broker.send(BrokerCommand::DropPendingServe);
    assert!(matches!(
        broker.reply().await,
        BrokerReply::ServeFutureDropped
    ));
    assert_connect_fails(&client_ep, broker.addr()).await;
    broker.shutdown_app().await;
}

// ---------------------------------------------------------------------------
// Long-lived production loop: serve_until_cancel
// ---------------------------------------------------------------------------

struct UntilCancelBroker {
    addr: SocketAddr,
    cancel: Arc<CancelHandle>,
    result_rx: Option<tokio::sync::oneshot::Receiver<Result<(), ServeUntilCancelError>>>,
    done: std::sync::mpsc::Receiver<()>,
    handle: Option<JoinHandle<()>>,
    _temporary: TemporaryDatabase,
}

impl UntilCancelBroker {
    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn cancel(&self) {
        self.cancel.cancel();
    }

    async fn await_result(&mut self) -> Result<(), Box<ServeUntilCancelError>> {
        let receiver = self
            .result_rx
            .as_mut()
            .expect("result receiver held until completion");
        tokio::time::timeout(WATCHDOG, receiver)
            .await
            .expect("until-cancel settles under watchdog")
            .expect("broker sent result")
            .map_err(Box::new)
    }

    fn bounded_complete(&mut self, context: &str) -> Result<(), String> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        drop(self.result_rx.take());
        let signalled = match self.done.recv_timeout(WATCHDOG) {
            Ok(()) => true,
            Err(RecvTimeoutError::Disconnected) => false,
            Err(RecvTimeoutError::Timeout) => {
                drop(handle);
                return Err(format!("{context}: until-cancel broker timeout"));
            }
        };
        let finish_deadline = Instant::now() + WATCHDOG;
        while !handle.is_finished() {
            if Instant::now() >= finish_deadline {
                drop(handle);
                return Err(format!(
                    "{context}: broker did not finish within {WATCHDOG:?} after its completion window; detaching it without joining"
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        match handle.join() {
            Ok(()) if signalled => Ok(()),
            Ok(()) => Err(format!(
                "{context}: broker thread exited without its completion signal"
            )),
            Err(_) => Err(format!("{context}: broker panicked")),
        }
    }

    fn complete(mut self, context: &str) -> Result<(), String> {
        self.bounded_complete(context)
    }
}

impl Drop for UntilCancelBroker {
    fn drop(&mut self) {
        if let Err(failure) = self.bounded_complete("dropped until-cancel test side") {
            eprintln!("broker cleanup failure on dropped until-cancel test side: {failure}");
        }
    }
}

async fn start_until_cancel_broker(
    label: &str,
    limits: ListenerLimits,
    admission_capacity: NonZeroU32,
    requests_per_connection: NonZeroU32,
) -> (UntilCancelBroker, TestPki) {
    let temporary = TemporaryDatabase::new(label);
    let pki = test_pki();
    let config = test_server_config(&pki);
    let database_path = temporary.path().to_path_buf();
    let cancel = Arc::new(CancelHandle::new());
    let cancel_for_thread = cancel.clone();
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let (result_tx, result_rx) =
        tokio::sync::oneshot::channel::<Result<(), ServeUntilCancelError>>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();

    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("until-cancel broker runtime");
        runtime.block_on(async move {
            let app = ForgeApp::start(ForgeConfig::new(
                SqliteConfig::file(&database_path).sqlx_logging(false),
            ))
            .await
            .expect("app starts");
            let handler = artisan_backend::RequestHandler::new(app.repository().clone());
            let listener = ForgeListener::bind(
                config,
                LocalCapability::from_bytes(INITIAL_CAPABILITY),
                SequencedOrigin::boxed(),
                limits,
                admission_capacity,
                requests_per_connection,
            )
            .expect("listener binds");
            let bound = listener.local_addr().expect("addr observable");
            let _ = addr_tx.send(bound);
            let outcome = listener
                .serve_until_cancel(&handler, cancel_for_thread.as_ref())
                .await;
            let _ = result_tx.send(outcome);
            drop(handler);
            let _ = app.shutdown().await;
        });
        drop(runtime);
        let _ = done_tx.send(());
    });

    let addr = tokio::time::timeout(WATCHDOG, addr_rx)
        .await
        .expect("addr under watchdog")
        .expect("broker bound");
    let broker = UntilCancelBroker {
        addr,
        cancel,
        result_rx: Some(result_rx),
        done: done_rx,
        handle: Some(handle),
        _temporary: temporary,
    };
    (broker, pki)
}

#[tokio::test]
async fn until_cancel_idle_timeout_leaves_same_listener_alive() {
    let limits = ListenerLimits {
        admission: Duration::from_millis(180),
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
        drain: Duration::from_secs(2),
    };
    let (mut broker, pki) =
        start_until_cancel_broker("until-idle-alive", limits, capacity(4), capacity(4)).await;
    let client_ep = client_endpoint(&pki);
    let start_addr = broker.addr();

    // Let several idle timeouts fire without a peer; the loop must continue.
    tokio::time::sleep(Duration::from_millis(650)).await;

    // Same endpoint must still be reachable and serve a valid client.
    let client = admit(&client_ep, start_addr, initial_credential()).await;
    exchange_response(
        &client,
        &list_projects_request("frame-until-idle"),
        "forge-meta-2",
        UnixMillis::from_millis(1001),
    )
    .await;
    // Cancel and prove drain success via result Ok and orphan check.
    broker.cancel();
    let outcome = broker.await_result().await;
    assert!(outcome.is_ok(), "cancel with drain must succeed");
    // Broker thread will have drained; endpoint must be closed.
    assert_connect_fails(&client_ep, start_addr).await;
    expect_application_close(&client.connection, CONNECTION_RELEASE_REASON).await;
    broker
        .complete("until-idle-alive")
        .expect("bounded completion");
}

#[tokio::test]
async fn until_cancel_idle_timeouts_do_not_consume_capacity() {
    let limits = ListenerLimits {
        admission: Duration::from_millis(150),
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
        drain: Duration::from_secs(2),
    };
    // Capacity 1: if timeouts consumed capacity, no peer could ever succeed.
    let (mut broker, pki) =
        start_until_cancel_broker("until-idle-capacity", limits, capacity(1), capacity(1)).await;
    let client_ep = client_endpoint(&pki);
    tokio::time::sleep(Duration::from_millis(520)).await;
    let client = admit(&client_ep, broker.addr(), initial_credential()).await;
    let unread = write_unread_budget_request(&client, &list_projects_request("frame-cap")).await;
    // After one successful admission the capacity is exhausted; the loop will
    // become terminal and drain. Cancel to avoid hanging if not terminal.
    broker.cancel();
    let outcome = broker.await_result().await;
    // Outcome may be Ok (cancel before exhaustion check) or service failure;
    // the key proof is that the first client succeeded, so timeouts did not
    // consume capacity.
    assert!(
        outcome.is_ok() || outcome.unwrap_err().is_service_failure(),
        "after one admission the loop either cancelled or hit exhaustion"
    );
    drop(unread);
    assert_connect_fails(&client_ep, broker.addr()).await;
    broker
        .complete("until-idle-capacity")
        .expect("bounded completion");
}

#[tokio::test]
async fn until_cancel_rejected_client_does_not_terminate() {
    let limits = default_limits(Duration::from_secs(2));
    let (mut broker, pki) =
        start_until_cancel_broker("until-rejected", limits, capacity(4), capacity(4)).await;
    let client_ep = client_endpoint(&pki);
    let addr = broker.addr();

    let rejected = try_admit(&client_ep, addr, wrong_value_credential()).await;
    assert!(rejected.is_err(), "wrong credential must not get Welcome");

    // The same endpoint must still serve a valid client after the rejection.
    // First attempt consumed two identities (0,1) before failing, so the next
    // valid Welcome uses 2/3 and its first request uses 4/1002.
    let client = admit(&client_ep, addr, initial_credential()).await;
    assert_eq!(client.welcome.frame_id.as_str(), "forge-meta-3");
    assert_eq!(client.welcome.sent_at, UnixMillis::from_millis(1001));
    exchange_response(
        &client,
        &list_projects_request("frame-after-reject"),
        "forge-meta-4",
        UnixMillis::from_millis(1002),
    )
    .await;
    broker.cancel();
    let outcome = broker.await_result().await;
    assert!(outcome.is_ok(), "loop must survive auth rejection");
    expect_application_close(&client.connection, CONNECTION_RELEASE_REASON).await;
    assert_connect_fails(&client_ep, addr).await;
    broker
        .complete("until-rejected")
        .expect("bounded completion");
}

#[tokio::test]
async fn until_cancel_cancellation_before_admission_drains() {
    let limits = ListenerLimits {
        admission: Duration::from_secs(30),
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
        drain: Duration::from_secs(2),
    };
    let cancel = Arc::new(CancelHandle::new());
    cancel.cancel();
    // Build listener directly in a dedicated thread so cancellation is already
    // set before the loop checks it.
    let temporary = TemporaryDatabase::new("until-cancel-before");
    let pki = test_pki();
    let config = test_server_config(&pki);
    let database_path = temporary.path().to_path_buf();
    let cancel_clone = cancel.clone();
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<SocketAddr>();
    let (result_tx, result_rx) =
        tokio::sync::oneshot::channel::<Result<(), ServeUntilCancelError>>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let app = ForgeApp::start(ForgeConfig::new(
                SqliteConfig::file(&database_path).sqlx_logging(false),
            ))
            .await
            .expect("app");
            let handler = artisan_backend::RequestHandler::new(app.repository().clone());
            let listener = ForgeListener::bind(
                config,
                LocalCapability::from_bytes(INITIAL_CAPABILITY),
                SequencedOrigin::boxed(),
                limits,
                capacity(4),
                capacity(4),
            )
            .expect("bind");
            let addr = listener.local_addr().expect("addr");
            let _ = addr_tx.send(addr);
            let outcome = listener
                .serve_until_cancel(&handler, cancel_clone.as_ref())
                .await;
            let _ = result_tx.send(outcome);
            drop(handler);
            let _ = app.shutdown().await;
        });
        drop(runtime);
        let _ = done_tx.send(());
    });
    let addr = tokio::time::timeout(WATCHDOG, addr_rx)
        .await
        .expect("addr")
        .expect("sent");
    let outcome = tokio::time::timeout(WATCHDOG, result_rx)
        .await
        .expect("result timeout")
        .expect("sent");
    assert!(outcome.is_ok(), "pre-cancelled loop must drain Ok");
    let client_ep = client_endpoint(&pki);
    assert_connect_fails(&client_ep, addr).await;
    assert!(done_rx.recv_timeout(WATCHDOG).is_ok());
    let deadline = Instant::now() + WATCHDOG;
    while !handle.is_finished() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    handle.join().expect("join");
}

#[tokio::test]
async fn until_cancel_cancellation_while_waiting_drains() {
    let limits = ListenerLimits {
        admission: Duration::from_secs(30),
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
        drain: Duration::from_secs(2),
    };
    let (mut broker, pki) =
        start_until_cancel_broker("until-cancel-waiting", limits, capacity(4), capacity(4)).await;
    let client_ep = client_endpoint(&pki);
    let addr = broker.addr();
    // Give the loop a moment to enter the admission wait.
    tokio::time::sleep(Duration::from_millis(100)).await;
    broker.cancel();
    let outcome = broker.await_result().await;
    assert!(outcome.is_ok(), "cancel while waiting must drain Ok");
    assert_connect_fails(&client_ep, addr).await;
    broker
        .complete("until-cancel-waiting")
        .expect("bounded completion");
}

#[tokio::test]
async fn until_cancel_request_failure_is_terminal_with_primary() {
    let limits = default_limits(Duration::from_secs(2));
    let (mut broker, pki) =
        start_until_cancel_broker("until-request-terminal", limits, capacity(4), capacity(8)).await;
    let client_ep = client_endpoint(&pki);
    let addr = broker.addr();
    let client = admit(&client_ep, addr, initial_credential()).await;
    // Cause a non-cancellation request failure: peer closes, server's next
    // accept fails with Peer.
    client
        .connection
        .close(VarInt::from_u32(0x02), b"peer done");
    let outcome = broker.await_result().await;
    assert!(outcome.is_err(), "request failure must be terminal");
    let error = outcome.unwrap_err();
    assert!(
        error.is_service_failure(),
        "must be classified as service failure"
    );
    assert!(error.drain_error().is_none(), "drain should succeed");
    assert!(
        error.as_request_error().is_some(),
        "primary must preserve request error"
    );
    let rendered = format!("{error}{error:?}");
    assert!(!rendered.contains("b7b7"), "must not leak bootstrap");
    assert_connect_fails(&client_ep, addr).await;
    broker
        .complete("until-request-terminal")
        .expect("bounded completion");
}

#[tokio::test]
async fn serve_one_retains_consuming_error_contract() {
    // Prove serve_one still drops on auth failure and idle timeout.
    let (mut broker, _temporary, pki) = start_broker(
        "serve-one-contract",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_millis(250)),
        capacity(4),
        capacity(4),
    )
    .await;
    let client_ep = client_endpoint(&pki);
    // Idle timeout path: serve_one must be terminal.
    broker.begin_serve();
    let outcome = broker.await_served().await;
    assert!(matches!(
        outcome,
        Err(ListenerError::Admission {
            source: DeadlineError::Timeout {
                operation: OperationKind::Connect,
                ..
            },
        })
    ));
    assert_connect_fails(&client_ep, broker.addr()).await;
    // Need fresh broker for auth failure because previous listener dropped.
    drop(broker);
    let (mut broker2, _tmp2, pki2) = start_broker(
        "serve-one-auth",
        None,
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        SequencedOrigin::boxed(),
        default_limits(Duration::from_secs(2)),
        capacity(4),
        capacity(4),
    )
    .await;
    let client_ep2 = client_endpoint(&pki2);
    broker2.begin_serve();
    let rejected = try_admit(&client_ep2, broker2.addr(), wrong_value_credential()).await;
    assert!(rejected.is_err());
    let err = broker2.await_served().await.expect_err("must be terminal");
    assert!(matches!(err, ListenerError::Authentication { .. }));
    let rendered = format!("{err}{err:?}");
    assert!(!rendered.contains("b7b7"));
    assert!(!rendered.contains("5c5c"));
    assert_connect_fails(&client_ep2, broker2.addr()).await;
    broker2.shutdown_app().await;
}

#[tokio::test]
async fn until_cancel_error_hides_secrets_in_debug_display() {
    let limits = default_limits(Duration::from_secs(2));
    let (mut broker, pki) =
        start_until_cancel_broker("until-secret", limits, capacity(4), capacity(8)).await;
    let client_ep = client_endpoint(&pki);
    let client = admit(&client_ep, broker.addr(), initial_credential()).await;
    client
        .connection
        .close(VarInt::from_u32(0x02), b"peer done");
    let error = broker.await_result().await.expect_err("terminal");
    let rendered = format!("{error}{error:?}");
    assert!(!rendered.contains("b7b7"));
    assert!(!rendered.contains("5c5c"));
    broker.complete("until-secret").expect("bounded completion");
}

#[tokio::test]
async fn until_cancel_family_mismatch_is_retryable_then_valid_succeeds() {
    // FamilyMismatch (present Reconnect while Initial expected) is retryable per
    // the fenced contract and must not terminate the loop.
    let limits = default_limits(Duration::from_secs(2));
    let (mut broker, pki) =
        start_until_cancel_broker("until-family", limits, capacity(4), capacity(4)).await;
    let client_ep = client_endpoint(&pki);
    let addr = broker.addr();

    let mismatch = HelloCredential::Reconnect(ReconnectCapability::from_bytes([0x11; 32]));
    let rejected = try_admit(&client_ep, addr, mismatch).await;
    assert!(rejected.is_err(), "family mismatch must not get Welcome");

    // Same endpoint must still serve a valid Initial credential.
    let client = admit(&client_ep, addr, initial_credential()).await;
    assert_eq!(client.welcome.frame_id.as_str(), "forge-meta-3");
    exchange_response(
        &client,
        &list_projects_request("frame-after-family"),
        "forge-meta-4",
        UnixMillis::from_millis(1002),
    )
    .await;
    broker.cancel();
    let outcome = broker.await_result().await;
    assert!(outcome.is_ok(), "family mismatch must be retryable");
    expect_application_close(&client.connection, CONNECTION_RELEASE_REASON).await;
    assert_connect_fails(&client_ep, addr).await;
    broker.complete("until-family").expect("bounded completion");
}

#[tokio::test]
async fn until_cancel_handshake_timeout_is_terminal_service_failure() {
    // Handshake timeout is terminal: authority may have already been touched,
    // so the loop must drain and report service failure, not retry.
    let limits = ListenerLimits {
        admission: Duration::from_secs(2),
        handshake: Duration::from_millis(120),
        next_request: Duration::from_secs(2),
        drain: Duration::from_secs(2),
    };
    let (mut broker, pki) =
        start_until_cancel_broker("until-hs-timeout", limits, capacity(4), capacity(4)).await;
    let client_ep = client_endpoint(&pki);
    let addr = broker.addr();

    // Connect but delay opening the control stream past the handshake deadline.
    let connecting = client_ep
        .connect(addr, LOOPBACK_SERVER_NAME)
        .expect("connect");
    let connection = tokio::time::timeout(WATCHDOG, connecting)
        .await
        .expect("connect watchdog")
        .expect("established");
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Attempt to open after the server has timed out; the server's
    // deadline decision is already terminal.
    let _ = connection.open_bi().await;

    let outcome = broker.await_result().await;
    assert!(outcome.is_err(), "handshake timeout must be terminal");
    let error = outcome.unwrap_err();
    assert!(
        error.is_service_failure(),
        "timeout must be service failure, not drain-only"
    );
    assert!(error.drain_error().is_none(), "drain should succeed");
    assert!(
        error.as_listener_error().is_some(),
        "primary must be listener authentication timeout"
    );
    if let Some(ListenerError::Authentication { source }) = error.as_listener_error() {
        assert!(matches!(
            source,
            DeadlineError::Timeout {
                operation: OperationKind::Handshake,
                ..
            }
        ));
    } else {
        panic!("expected authentication timeout");
    }
    let rendered = format!("{error}{error:?}");
    assert!(!rendered.contains("b7b7"));

    assert_connect_fails(&client_ep, addr).await;
    broker
        .complete("until-hs-timeout")
        .expect("bounded completion");
}
