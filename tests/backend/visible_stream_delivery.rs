//! Visible-stream proof: admission streams at launch, chunks stream while
//! the provider turn is held, terminal closes the flow.
//!
//! A real Forge listener serves a real QUIC client on loopback. The first
//! test subscribes, invokes production `launch_claim`, and reads the user
//! admission patch batch off the wire with no provider ever admitted,
//! then proves silence (no terminal follows unlaunched work). The second
//! test sends one steered message over the wire and drives the live Codex
//! fixture turn (burst mode: reasoning frame plus 64 deltas, no terminal
//! until interrupt) through production `consume_turn` with a shared
//! interaction registry. It proves wire order: send receipt, user
//! admission patch, distinct `burst-01`/`burst-02` appends, one
//! `ReasoningSummaryDelta` observation event, all before any terminal
//! lifecycle, then the cancelled terminal after interrupt. Order
//! assertions pin admission/observation/terminal relative positions;
//! batch coalescing means no exact per-delta frame count is asserted.

use std::net::SocketAddr;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use artisan_database::{
    AttachProjectInput, BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch,
    CreateThreadInput, DispatchLeaseOwner, ProviderBindingBytes, QueueFirstMessageInput,
    Repository, RunBatchScope, RunLaunchCredentials, RunStartKey, SetThreadEngineConfigInput,
    SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CodexModelContextWindow, CodexReasoningEffort, CodexSelection,
    CodexServiceTier, Command, ConversationCursor, ConversationItem, ConversationPatch,
    ConversationRequest, ConversationSubscribe, CountLimit, DirectoryId, DisplayName,
    EngineAgentId, EngineConfigUpdatePrecondition, EngineId, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, ItemId, MessageBody, MessageId, NetworkAccess,
    PatchId, PermissionId, ProjectId, QueueMessage, QueueMessagePayload, RequestId, RootPath,
    RunId, SteerTarget, ThreadId, ThreadTitle, TurnId, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use artisan_native_engine::{NativeCodexAuthority, NativeOpenCode2Authority};
use artisan_protocol::{
    APPLICATION_PROTOCOL_VERSION, ClientRequest, FrameId, Hello, HelloCredential, LocalCapability,
    ProtocolVersion, ResponsePayload, VersionOffer, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, DeadlineError, LOOPBACK_SERVER_NAME, OperationKind, PinnedIdentity,
};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use sea_orm::EntityTrait;

use super::{
    ClaimExecution, ClaimIds, LoadedClaim, NativeRunDispatcherConfig,
    NativeRunDispatcherConfigInput, ResolvedLaunch, TurnConsumptionContext, consume_turn,
    launch_claim,
};
use crate::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, ForgeListener,
    RequestHandler, RequestTermination, SystemCommandOrigin,
    conversation_commit_notifier::ConversationCommitNotifier,
    engine_owner::{EngineCodexTurnInput, EngineOwner, EngineOwnerShutdown},
    run_cancellation::RunCancellationRegistry,
    run_interaction::RunInteractionRegistry,
};

const TEST_DEADLINE: Duration = Duration::from_secs(20);
const INITIAL_CAPABILITY: [u8; 32] = [0x4d; 32];

/// Real-clock base for seed chronology: observation commits fence the
/// dispatch lease against the real origin clock.
fn seed_base_ms() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time after epoch")
        .as_millis();
    i64::try_from(now).expect("millis fit") - 60_000
}

#[derive(Debug)]
struct StreamOrigin {
    next: AtomicU64,
}

impl StreamOrigin {
    fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
        }
    }
}

impl CommandOrigin for StreamOrigin {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        Ok(format!(
            "stream-origin-{}",
            self.next.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        Ok(UnixMillis::from_millis(1_000))
    }
}

struct StreamPki {
    pinned_identity: PinnedIdentity,
    private_key: PrivatePkcs8KeyDer<'static>,
    certificate: CertificateDer<'static>,
}

fn stream_pki() -> StreamPki {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("valid SAN");
    let certificate = certified_key.cert.der().clone();
    StreamPki {
        pinned_identity: PinnedIdentity::from_certificate(&certificate),
        private_key: PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der()),
        certificate,
    }
}

fn stream_server_config(pki: &StreamPki) -> ServerConfig {
    artisan_transport::server_config(vec![pki.certificate.clone()], pki.private_key.clone_key())
        .expect("server configuration")
}

fn stream_client_config(pki: &StreamPki) -> ClientConfig {
    artisan_transport::client_config(pki.certificate.clone(), pki.pinned_identity)
        .expect("client configuration")
}

fn stream_listener_limits() -> crate::ListenerLimits {
    crate::ListenerLimits {
        admission: Duration::from_secs(2),
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(20),
        drain: Duration::from_secs(2),
    }
}

/// Scratch project root (also the fixture cwd) plus database path.
struct StreamTempRoot {
    dir: PathBuf,
    root: RootPath,
    db_path: PathBuf,
}

impl StreamTempRoot {
    fn new(label: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "artisan-visible-stream-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("stream temp root");
        let root = RootPath::parse(dir.to_str().expect("temp path utf8")).expect("root");
        let db_path = dir.join("stream.sqlite3");
        Self { dir, root, db_path }
    }
}

impl Drop for StreamTempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Resolves the built Codex wire-fixture executable.
fn stream_fixture_program() -> PathBuf {
    if let Ok(path) = std::env::var("ARTISAN_CODEX_WIRE_FIXTURE") {
        let mapping = PathBuf::from(&path);
        let path = if mapping.is_absolute() {
            mapping
        } else {
            let runfiles = runfiles::Runfiles::create().expect("runfiles discovery");
            runfiles::rlocation!(runfiles, path.as_str()).expect("wire fixture runfile")
        };
        assert!(
            path.is_file(),
            "declared wire fixture must be a regular file"
        );
        return path;
    }
    panic!("wire fixture binary not found; set ARTISAN_CODEX_WIRE_FIXTURE");
}

/// Copies the built fixture to a per-test executable whose basename names
/// the frozen scenario.
fn stream_scenario_program(fixture: &Path, dir: &Path, scenario: &str) -> PathBuf {
    let named = dir.join(format!(
        "codex-wire-{scenario}{}",
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::copy(fixture, &named).expect("wire fixture copies per test");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(&named).expect("meta").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&named, permissions).expect("chmod");
    }
    named
}

fn stream_codex_config() -> EngineRunConfig {
    let budget = |ms: u64| FiniteMillis::new(ms).expect("finite millis valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: budget(45_000),
        readiness_budget: budget(5_000),
        health_budget: budget(5_000),
        prompt_budget: budget(15_000),
        stream_budget: budget(15_000),
        close_budget: budget(5_000),
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness limit"),
        max_header_count: CountLimit::new(32).expect("header count"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr"),
        // Four slots: the 64-frame burst causally requires the production
        // inline drain before its correlated ack can be read.
        observation_capacity: CountLimit::new(4).expect("observation cap"),
    })
    .expect("runtime valid");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-stream").expect("permission id"),
        EngineAgentId::parse("agent-stream").expect("agent id"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::Codex(
            CodexSelection::new(
                EngineProfileId::parse("codex-fixture").expect("profile id"),
                Some(EngineModelId::parse("codex-model").expect("model id")),
                permission,
                Some(CodexReasoningEffort::High),
                Some(CodexServiceTier::Fast),
                Some(CodexModelContextWindow::new(1_000).expect("window")),
            )
            .expect("codex stream selection valid"),
        ),
        runtime,
    )
}

fn stream_dispatcher_config(notifier: ConversationCommitNotifier) -> NativeRunDispatcherConfig {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        notifier,
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::from_millis(10),
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_millis(10),
            queue_capacity: NonZeroUsize::new(1).expect("one queue slot is nonzero"),
            max_command_retries: NonZeroUsize::new(1).expect("one retry is nonzero"),
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
    )
    .expect("test scheduler policy should validate")
}

fn hello_envelope() -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("stream-client-hello").expect("hello frame id"),
        sent_at: UnixMillis::from_millis(10),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![APPLICATION_PROTOCOL_VERSION])
                .expect("version offer"),
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
            supports_lifecycle_control: false,
        }),
    }
}

/// Seeds thread, Codex config, and first message on a real-clock base.
async fn seed_stream_thread(
    repository: &Repository,
    thread_id: &ThreadId,
    label: &str,
    root: &RootPath,
) -> i64 {
    let base = seed_base_ms();
    let at = |offset: i64| UnixMillis::from_millis(base + offset);
    let project_id = ProjectId::parse(format!("project-{label}")).expect("project id");
    repository
        .attach_project(AttachProjectInput {
            request_id: RequestId::parse(format!("request-project-{label}")).expect("request id"),
            directory_id: DirectoryId::parse(format!("directory-{label}")).expect("directory id"),
            project_id: project_id.clone(),
            root_path: root.clone(),
            display_name: DisplayName::parse("Project").expect("display name"),
            attached_at: at(0),
        })
        .await
        .expect("project should attach");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse(format!("request-thread-{label}")).expect("request id"),
            thread_id: thread_id.clone(),
            project_id,
            title: ThreadTitle::parse("Stream thread").expect("title"),
            created_at: at(0),
            updated_at: at(0),
        })
        .await
        .expect("thread should create");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse(format!("request-config-{label}")).expect("request id"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: stream_codex_config(),
            accepted_at: at(0),
        })
        .await
        .expect("engine configuration should persist");
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse(format!("request-message-{label}")).expect("request id"),
            message_id: MessageId::parse(format!("message-{label}")).expect("message id"),
            thread_id: thread_id.clone(),
            body: MessageBody::parse("hello stream").expect("message body"),
            accepted_at: at(40),
        })
        .await
        .expect("first message should queue");
    base
}

#[expect(
    clippy::too_many_lines,
    clippy::large_futures,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring and its timeout wrapper"
)]
#[tokio::test]
async fn launch_claim_streams_user_admission_before_provider_startup() {
    tokio::time::timeout(Duration::from_secs(90), async {
        let fixture = stream_fixture_program();
        let temp = StreamTempRoot::new("launch-wire");
        let program = stream_scenario_program(&fixture, &temp.dir, "steer_burst");
        let database = connect(SqliteConfig::file(&temp.db_path).sqlx_logging(false))
            .await
            .expect("file database should open");
        migrate_to_current(&database)
            .await
            .expect("migrations should apply");
        let repository = Repository::new(database.clone());
        let thread_id = ThreadId::parse("thread-stream-launch").expect("thread id");
        let base = seed_stream_thread(&repository, &thread_id, "launch", &temp.root).await;

        let notifier = ConversationCommitNotifier::new();
        let config = stream_dispatcher_config(notifier.clone());
        let handler = RequestHandler::new(repository.clone())
            .with_conversation_commit_notifier(notifier.clone());

        let pki = stream_pki();
        let endpoint = artisan_transport::bind_loopback_client(stream_client_config(&pki))
            .expect("loopback client binds");
        let listener = ForgeListener::bind(
            stream_server_config(&pki),
            LocalCapability::from_bytes(INITIAL_CAPABILITY),
            Box::new(StreamOrigin::new()),
            stream_listener_limits(),
            NonZeroU32::new(1).expect("admission capacity"),
            NonZeroU32::new(4).expect("request capacity"),
        )
        .expect("forge listener binds");
        let address = listener.local_addr().expect("listener address");
        let cancel = CancelHandle::new();
        let serve = listener.serve_one(&handler, &cancel);
        // The launch must happen strictly after the subscription is
        // active: otherwise the admission arrives via activation replay
        // instead of the launch wake, and the regression proves nothing.
        let (subscribed_tx, subscribed_rx) = tokio::sync::oneshot::channel::<()>();

        let wire = async {
            let connection = connect_stream_client(&endpoint, address).await;
            handshake_stream_client(&connection).await;
            // Subscribe before anything is projected: the activation
            // replay is Current, so no delivery stream opens yet.
            let (mut subscribe_send, mut subscribe_recv) =
                connection.open_bi().await.expect("subscribe stream opens");
            artisan_transport::send_envelope(&mut subscribe_send, &subscribe_envelope(&thread_id))
                .await
                .expect("subscribe sends");
            drop(subscribe_send);
            let subscribed = tokio::time::timeout(
                TEST_DEADLINE,
                artisan_transport::receive_envelope(&mut subscribe_recv),
            )
            .await
            .expect("subscribe response settles")
            .expect("subscribe response decodes");
            assert!(
                matches!(subscribed.body, WireEnvelopeBody::Response(_)),
                "subscribe must answer with a response",
            );
            // Barrier: the subscribe response is written before activation
            // finishes, so a second round trip is required. The driver
            // serves requests strictly in order only after the previous
            // activation (replay drain included) completes, hence this
            // re-subscribe response proves the subscription is active and
            // the launch below cannot slip into activation replay.
            let (mut barrier_send, mut barrier_recv) =
                connection.open_bi().await.expect("barrier stream opens");
            let mut barrier = subscribe_envelope(&thread_id);
            barrier.frame_id = FrameId::parse("stream-subscribe-again").expect("frame id");
            artisan_transport::send_envelope(&mut barrier_send, &barrier)
                .await
                .expect("barrier sends");
            drop(barrier_send);
            let barriered = tokio::time::timeout(
                TEST_DEADLINE,
                artisan_transport::receive_envelope(&mut barrier_recv),
            )
            .await
            .expect("barrier response settles")
            .expect("barrier response decodes");
            assert!(
                matches!(barriered.body, WireEnvelopeBody::Response(_)),
                "barrier must answer with a response",
            );
            subscribed_tx.send(()).expect("driver waits for subscribe");
            // The production launch below is the first publication: the
            // delivery stream opens exactly then with the user admission.
            let mut delivery = tokio::time::timeout(TEST_DEADLINE, connection.accept_uni())
                .await
                .expect("delivery stream opens on launch")
                .expect("delivery stream accepts");
            let admission = tokio::time::timeout(
                TEST_DEADLINE,
                artisan_transport::receive_envelope(&mut delivery),
            )
            .await
            .expect("admission frame settles")
            .expect("admission frame decodes");
            let WireEnvelopeBody::PatchBatch(batch) = admission.body else {
                panic!("launch must deliver a patch batch");
            };
            assert_eq!(batch.thread_id(), &thread_id);
            let mut saw_user = false;
            for patch in batch.patches() {
                if let ConversationPatch::ItemUpsert { item, .. } = patch
                    && let ConversationItem::UserMessage(user) = item
                        && user.body.as_str() == "hello stream" {
                            saw_user = true;
                        }
            }
            assert!(saw_user, "launch must stream the user admission");
            // Nothing else can commit: no provider was ever admitted, so
            // no terminal may follow the admission.
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(500),
                    artisan_transport::receive_envelope(&mut delivery),
                )
                .await
                .is_err(),
                "unlaunched thread must stay silent after admission"
            );
            cancel.cancel();
        };

        let drive = async {
            subscribed_rx.await.expect("wire subscribes before launch");
            let claimed = repository
                .claim_next_message_dispatch(ClaimMessageDispatch {
                    owner: DispatchLeaseOwner::new([0x11; 32]),
                    claimed_at: UnixMillis::from_millis(base + 390),
                    lease_expires_at: UnixMillis::from_millis(base + 3_600_000),
                })
                .await
                .expect("claim should persist")
                .expect("dispatch should be claimable");
            let payload = repository
                .read_queue_message_dispatch_payload(
                    &MessageId::parse("message-launch").expect("message id"),
                )
                .await
                .expect("payload should read")
                .expect("payload should exist");
            let settings = repository
                .read_thread_engine_settings(&thread_id)
                .await
                .expect("settings should read")
                .expect("settings should exist");
            let project_root = repository
                .read_thread_project_root(&thread_id)
                .await
                .expect("project root should read");
            let launch = NativeCodexAuthority::new()
                .resolve_launch_with_executable(
                    &temp.db_path,
                    &EngineProfileId::parse("codex-fixture").expect("profile id"),
                    &program,
                    "codex-cli 0.145.0",
                )
                .expect("wire launch resolves");
            let run_id = RunId::parse("run-stream-launch").expect("run id");
            let cancel_registry = RunCancellationRegistry::new(8).expect("registry");
            let interaction_registry = RunInteractionRegistry::new(8).expect("registry");
            let mut owner = EngineOwner::start_configured(
                NonZeroUsize::new(1).expect("one slot"),
                &tokio::runtime::Handle::current(),
            );
            let lease = cancel_registry
                .register(thread_id.clone(), run_id.clone())
                .expect("cancellation lease");
            let origin = SystemCommandOrigin;
            let stop = CancelHandle::new();
            let process_cancel = CancelHandle::new();
            let context = ClaimExecution {
                repository: &repository,
                database_path: &temp.db_path,
                config: &config,
                origin: &origin,
                stop: &stop,
                process_cancel: &process_cancel,
                cancellation: &cancel_registry,
                interactions: &interaction_registry,
                owner: &owner,
                claimed,
            };
            let ids = ClaimIds {
                run_id,
                turn_id: TurnId::parse("turn-stream-launch").expect("turn id"),
                item_id: ItemId::parse("item-stream-launch").expect("item id"),
                first_patch_id: PatchId::parse("patch-stream-launch-first").expect("patch id"),
                second_patch_id: PatchId::parse("patch-stream-launch-second").expect("patch id"),
                operated_at: UnixMillis::from_millis(base + 490),
                run_start_key: RunStartKey::new([0x44; 32]),
                credentials: RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]),
            };
            let launched = launch_claim(
                LoadedClaim {
                    context,
                    payload,
                    settings,
                    project_root,
                    launch: ResolvedLaunch::Codex(Box::new(launch)),
                },
                ids,
                lease,
                None,
            )
            .await
            .expect("production launch must start");
            drop(launched);
            assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
        };

        let (serve_result, (), ()) = tokio::join!(serve, wire, drive);
        let (listener, report) = serve_result.expect("serve must end cleanly");
        assert_eq!(report.completed_requests, 2);
        assert!(matches!(
            report.termination,
            RequestTermination::Failed {
                source: DeadlineError::Cancelled {
                    operation: OperationKind::Receive
                }
            }
        ));
        listener.drain().await.expect("listener drains");
        artisan_transport::shutdown(
            &endpoint,
            quinn::VarInt::from_u32(0),
            b"visible stream launch test complete",
            TEST_DEADLINE,
        )
        .await
        .expect("endpoint shuts down");
    })
    .await
    .expect("launch admission streams inside budget");
}

async fn connect_stream_client(endpoint: &Endpoint, address: SocketAddr) -> Connection {
    let connecting = endpoint
        .connect(address, LOOPBACK_SERVER_NAME)
        .expect("loopback connect");
    tokio::time::timeout(TEST_DEADLINE, connecting)
        .await
        .expect("connect settles")
        .expect("connect succeeds")
}

async fn handshake_stream_client(connection: &Connection) {
    let (mut control_send, mut control_recv) =
        connection.open_bi().await.expect("control stream opens");
    let _welcome = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::client_handshake(&mut control_send, &mut control_recv, hello_envelope()),
    )
    .await
    .expect("handshake settles")
    .expect("handshake succeeds");
}

fn subscribe_envelope(thread_id: &ThreadId) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("stream-subscribe").expect("frame id"),
        sent_at: UnixMillis::from_millis(20),
        body: WireEnvelopeBody::Request(ClientRequest::Conversation(
            ConversationRequest::Subscribe(ConversationSubscribe::resume(
                thread_id.clone(),
                ConversationCursor::default(),
            )),
        )),
    }
}

fn steer_send_envelope(
    frame_id: &str,
    command_id: &RequestId,
    thread_id: &ThreadId,
    run_id: &RunId,
) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("frame id"),
        sent_at: UnixMillis::from_millis(30),
        body: WireEnvelopeBody::Request(ClientRequest::Command(Command::QueueMessage(
            QueueMessage::new(
                command_id.clone(),
                thread_id.clone(),
                QueueMessagePayload::text_only("follow up").expect("payload"),
            )
            .with_steer_target(SteerTarget::new(run_id.clone())),
        ))),
    }
}

/// Frames collected from the server delivery stream, in arrival order.
#[expect(
    clippy::large_enum_variant,
    reason = "fixture frame vocabulary mirrors the wire variants; boxing would only add indirection in tests"
)]
#[derive(Debug)]
enum StreamFrame {
    PatchBatch(artisan_domain::PatchBatch),
    ObservationEvent(artisan_domain::EngineObservationEvent),
}

fn batch_has_assistant(batch: &artisan_domain::PatchBatch) -> bool {
    batch.patches().iter().any(|patch| {
        matches!(
            patch,
            ConversationPatch::ItemUpsert {
                item: ConversationItem::AssistantMessage(_),
                ..
            } | ConversationPatch::ItemAppend { .. }
        )
    })
}

#[expect(
    clippy::too_many_lines,
    clippy::large_futures,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring and its timeout wrapper"
)]
#[tokio::test]
async fn live_connection_streams_admission_chunks_and_observation_before_terminal() {
    tokio::time::timeout(Duration::from_secs(150), async {
        let fixture = stream_fixture_program();
        let temp = StreamTempRoot::new("live");
        let program = stream_scenario_program(&fixture, &temp.dir, "steer_burst");
        let database = connect(SqliteConfig::file(&temp.db_path).sqlx_logging(false))
            .await
            .expect("file database should open");
        migrate_to_current(&database)
            .await
            .expect("migrations should apply");
        let repository = Repository::new(database.clone());
        let thread_id = ThreadId::parse("thread-stream-live").expect("thread id");
        let base = seed_stream_thread(&repository, &thread_id, "live", &temp.root).await;

        let notifier = ConversationCommitNotifier::new();
        let config = stream_dispatcher_config(notifier.clone());
        let interaction_registry = RunInteractionRegistry::new(8).expect("registry");
        let handler = RequestHandler::new(repository.clone())
            .with_conversation_commit_notifier(notifier.clone())
            .with_run_interaction_registry(interaction_registry.clone());

        let pki = stream_pki();
        let endpoint = artisan_transport::bind_loopback_client(stream_client_config(&pki))
            .expect("loopback client binds");
        let listener = ForgeListener::bind(
            stream_server_config(&pki),
            LocalCapability::from_bytes(INITIAL_CAPABILITY),
            Box::new(StreamOrigin::new()),
            stream_listener_limits(),
            NonZeroU32::new(1).expect("admission capacity"),
            NonZeroU32::new(4).expect("request capacity"),
        )
        .expect("forge listener binds");
        let address = listener.local_addr().expect("listener address");
        let cancel = CancelHandle::new();
        let serve = listener.serve_one(&handler, &cancel);
        let run_cancel = std::sync::Arc::new(CancelHandle::new());

        // Steered message identity, from the wire receipt to the driver.
        let (message_tx, message_rx) = tokio::sync::oneshot::channel::<MessageId>();

        let wire = async {
            let connection = connect_stream_client(&endpoint, address).await;
            handshake_stream_client(&connection).await;
            let (mut subscribe_send, mut subscribe_recv) = connection
                .open_bi()
                .await
                .expect("subscribe stream opens");
            artisan_transport::send_envelope(
                &mut subscribe_send,
                &subscribe_envelope(&thread_id),
            )
            .await
            .expect("subscribe sends");
            drop(subscribe_send);
            let subscribed = tokio::time::timeout(
                TEST_DEADLINE,
                artisan_transport::receive_envelope(&mut subscribe_recv),
            )
            .await
            .expect("subscribe response settles")
            .expect("subscribe response decodes");
            assert!(
                matches!(subscribed.body, WireEnvelopeBody::Response(_)),
                "subscribe must answer with a response",
            );

            // Liveness gate: the turn must be consuming (assistant patch
            // durable) before the send names its run, otherwise the steer
            // is a typed stale refusal instead of a stream.
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let replay = repository
                        .read_conversation_patch_replay(&thread_id, ConversationCursor::default())
                        .await
                        .expect("replay should read");
                    if let artisan_database::ConversationPatchReplay::Batch(batch) = replay
                        && batch_has_assistant(&batch) {
                            return;
                        }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .expect("turn must go live");

            let command_id = RequestId::parse("request-stream-steer").expect("request id");
            let (mut send_send, mut send_recv) = connection
                .open_bi()
                .await
                .expect("send stream opens");
            artisan_transport::send_envelope(
                &mut send_send,
                &steer_send_envelope(
                    "request-stream-steer",
                    &command_id,
                    &thread_id,
                    &RunId::parse("run-stream-live").expect("run id"),
                ),
            )
            .await
            .expect("steer send transmits");
            drop(send_send);
            let receipted = tokio::time::timeout(
                TEST_DEADLINE,
                artisan_transport::receive_envelope(&mut send_recv),
            )
            .await
            .expect("send receipt settles")
            .expect("send receipt decodes");
            let WireEnvelopeBody::Response(receipted) = receipted.body else {
                panic!("expected the send receipt");
            };
            assert_eq!(receipted.request_id, command_id);
            let ResponsePayload::MessageQueued(receipt) = receipted.payload else {
                panic!("expected a queued receipt");
            };
            assert!(!receipt.message_id.as_str().is_empty());
            message_tx
                .send(receipt.message_id)
                .expect("driver takes the admitted message");

            // The delivery stream opens on the first published batch; every
            // frame is classified in arrival order. The receipt above
            // already proves the steer completed (burst drained and
            // projected), and the turn is still held: no terminal may have
            // arrived yet.
            let mut delivery = tokio::time::timeout(TEST_DEADLINE, connection.accept_uni())
                .await
                .expect("delivery stream opens")
                .expect("delivery stream accepts");
            let mut frames = Vec::new();
            let mut saw_burst_01 = false;
            let mut saw_burst_02 = false;
            let mut saw_observation = false;
            // A terminal lifecycle here preserves its full identity for
            // the post-loop diagnosis instead of failing blind: the run
            // row, dispatch row, and patch position are read below.
            let mut terminal_seen: Option<(artisan_domain::ConversationLifecycle, String, u64)> =
                None;
            // Read until two DISTINCT chunk updates plus the reasoning
            // observation are observed. Remaining burst patches may
            // legitimately still be in flight (commits coalesce into
            // replay batches), so there is no silence assertion here —
            // but no terminal lifecycle may appear while held.
            loop {
                let envelope = tokio::time::timeout(
                    TEST_DEADLINE,
                    artisan_transport::receive_envelope(&mut delivery),
                )
                .await
                .expect("delivery frame settles")
                .expect("delivery frame decodes");
                match envelope.body {
                    WireEnvelopeBody::PatchBatch(batch) => {
                        assert_eq!(batch.thread_id(), &thread_id);
                        for patch in batch.patches() {
                            match patch {
                                ConversationPatch::ItemAppend { text, .. } => {
                                    if text.as_str().contains("burst-01") {
                                        saw_burst_01 = true;
                                    }
                                    if text.as_str().contains("burst-02") {
                                        saw_burst_02 = true;
                                    }
                                }
                                ConversationPatch::TurnLifecycle {
                                    lifecycle,
                                    turn_id,
                                    sequence,
                                    ..
                                }
                                    if lifecycle.is_terminal() && terminal_seen.is_none() => {
                                        terminal_seen = Some((
                                            *lifecycle,
                                            turn_id.as_str().to_owned(),
                                            sequence.get(),
                                        ));
                                    }
                                _ => {}
                            }
                        }
                        frames.push(StreamFrame::PatchBatch(batch));
                    }
                    WireEnvelopeBody::Event(event) => {
                        let artisan_domain::Event::EngineObservation(observation) =
                            event.event
                        else {
                            panic!("expected an engine observation event");
                        };
                        assert_eq!(observation.thread_id, thread_id);
                        // Only the provider thinking trace counts here, not
                        // just any engine observation.
                        if matches!(
                            observation.observation,
                            artisan_domain::Observation::ReasoningSummaryDelta(_)
                        ) {
                            saw_observation = true;
                        }
                        frames.push(StreamFrame::ObservationEvent(observation));
                    }
                    _ => panic!("unexpected delivery frame"),
                }
                if terminal_seen.is_some()
                    || (saw_burst_01 && saw_burst_02 && saw_observation)
                {
                    break;
                }
            }
            if let Some((lifecycle, turn_id, sequence)) = terminal_seen {
                let run = artisan_database::entities::assistant_run::Entity::find_by_id(
                    "run-stream-live",
                )
                .one(&database)
                .await
                .expect("run row should read");
                // Isolated test database: read every dispatch row instead
                // of filtering on a thread column the table does not have.
                let dispatch = artisan_database::entities::message_dispatch::Entity::find()
                    .all(&database)
                    .await
                    .expect("dispatch rows should read");
                panic!(
                    "terminal lifecycle while held: {lifecycle:?} turn {turn_id} patch sequence {sequence} after {} frames (burst-01 {saw_burst_01}, burst-02 {saw_burst_02}, reasoning {saw_observation}); run lifecycle {:?}; dispatches {:?}",
                    frames.len(),
                    run.map(|row| (row.run_id, format!("{:?}", row.lifecycle))),
                    dispatch
                        .iter()
                        .map(|row| (
                            row.message_id.clone(),
                            format!("{:?}", row.state),
                            row.last_error.clone(),
                        ))
                        .collect::<Vec<_>>(),
                );
            }
            run_cancel.cancel();
            // Drain to the terminal close.
            loop {
                let envelope = tokio::time::timeout(
                    TEST_DEADLINE,
                    artisan_transport::receive_envelope(&mut delivery),
                )
                .await
                .expect("terminal frame settles")
                .expect("terminal frame decodes");
                let terminal_here = matches!(
                    &envelope.body,
                    WireEnvelopeBody::PatchBatch(batch)
                        if batch.patches().iter().any(|patch| matches!(
                            patch,
                            ConversationPatch::TurnLifecycle { lifecycle, .. }
                            if lifecycle.is_terminal()
                        ))
                );
                match envelope.body {
                    WireEnvelopeBody::PatchBatch(batch) => {
                        frames.push(StreamFrame::PatchBatch(batch));
                    }
                    WireEnvelopeBody::Event(event) => {
                        let artisan_domain::Event::EngineObservation(observation) =
                            event.event
                        else {
                            panic!("expected an engine observation event");
                        };
                        frames.push(StreamFrame::ObservationEvent(observation));
                    }
                    _ => panic!("unexpected delivery frame"),
                }
                if terminal_here {
                    break;
                }
            }
            // End the listener only after the terminal frame is in hand:
            // serve cleanup finishes the delivery stream, which must not
            // race the terminal read above.
            cancel.cancel();
            frames
        };

        let drive = async {
            let claimed = repository
                .claim_next_message_dispatch(ClaimMessageDispatch {
                    owner: DispatchLeaseOwner::new([0x11; 32]),
                    claimed_at: UnixMillis::from_millis(base + 390),
                    lease_expires_at: UnixMillis::from_millis(base + 3_600_000),
                })
                .await
                .expect("claim should persist")
                .expect("dispatch should be claimable");
            let payload = repository
                .read_queue_message_dispatch_payload(
                    &MessageId::parse("message-live").expect("message id"),
                )
                .await
                .expect("payload should read")
                .expect("payload should exist");
            let settings = repository
                .read_thread_engine_settings(&thread_id)
                .await
                .expect("settings should read")
                .expect("settings should exist");
            let project_root = repository
                .read_thread_project_root(&thread_id)
                .await
                .expect("project root should read");
            let launch = NativeCodexAuthority::new()
                .resolve_launch_with_executable(
                    &temp.db_path,
                    &EngineProfileId::parse("codex-fixture").expect("profile id"),
                    &program,
                    "codex-cli 0.145.0",
                )
                .expect("wire launch resolves");
            let run_id = RunId::parse("run-stream-live").expect("run id");
            let turn_id = TurnId::parse("turn-stream-live").expect("turn id");
            let cancel_registry = RunCancellationRegistry::new(8).expect("registry");
            let mut owner = EngineOwner::start_configured(
                NonZeroUsize::new(1).expect("one slot"),
                &tokio::runtime::Handle::current(),
            );
            let lease = cancel_registry
                .register(thread_id.clone(), run_id.clone())
                .expect("cancellation lease");
            let origin = SystemCommandOrigin;
            let stop = CancelHandle::new();
            let process_cancel = CancelHandle::new();
            let context = ClaimExecution {
                repository: &repository,
                database_path: &temp.db_path,
                config: &config,
                origin: &origin,
                stop: &stop,
                process_cancel: &process_cancel,
                cancellation: &cancel_registry,
                interactions: &interaction_registry,
                owner: &owner,
                claimed,
            };
            let ids = ClaimIds {
                run_id: run_id.clone(),
                turn_id: turn_id.clone(),
                item_id: ItemId::parse("item-stream-live").expect("item id"),
                first_patch_id: PatchId::parse("patch-stream-live-first").expect("patch id"),
                second_patch_id: PatchId::parse("patch-stream-live-second").expect("patch id"),
                operated_at: UnixMillis::from_millis(base + 490),
                run_start_key: RunStartKey::new([0x44; 32]),
                credentials: RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]),
            };
            let launched = launch_claim(
                LoadedClaim {
                    context,
                    payload,
                    settings: settings.clone(),
                    project_root: project_root.clone(),
                    launch: ResolvedLaunch::Codex(Box::new(launch)),
                },
                ids,
                lease,
                None,
            )
            .await
            .expect("production launch must start");
            let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding bytes");
            let bound = repository
                .bind_run_provider(BindRunProvider {
                    claimed: &launched.context.claimed,
                    receipt: &launched.receipt,
                    run_start_key: &launched.ids.run_start_key,
                    credentials: &launched.ids.credentials,
                    expected_launch_at: UnixMillis::from_millis(base + 490),
                    bound_at: UnixMillis::from_millis(base + 590),
                    binding_version: 1,
                    binding_bytes: &binding,
                })
                .await
                .expect("bind should persist");
            let bound = match bound {
                BindRunProviderOutcome::Bound(receipt)
                | BindRunProviderOutcome::AlreadyBound(receipt) => receipt,
            };
            // The live run registers its own routing inbox, exactly like
            // the production dispatch loop does before consuming a turn.
            let (_interaction_lease, mut inbox) = interaction_registry
                .register(thread_id.clone(), run_id.clone())
                .expect("live run should register");
            let mut turn = owner
                .admit_codex_turn(
                    EngineCodexTurnInput {
                        run_id: run_id.clone(),
                        thread_id: thread_id.clone(),
                        project_root: temp.root.clone(),
                        prompt_id: "prompt-stream-1".to_owned(),
                        prompt: QueueMessagePayload::text_only("hello steer").expect("payload"),
                        settings,
                        launch: match launched.launch {
                            ResolvedLaunch::Codex(launch) => *launch,
                            _ => panic!("codex launch must survive"),
                        },
                        continuation: None,
                        prompt_delivery: "immediate".to_owned(),
                        stream_after: 0,
                        control_capacity: 1,
                    },
                    Duration::from_secs(50),
                )
                .expect("wire turn admits");
            turn.prepare().await.expect("wire turn prepares");
            turn.authorize().expect("wire turn authorizes once");
            let run_cancel_ref: &CancelHandle = &run_cancel;
            let custody_unresolved = consume_turn(
                TurnConsumptionContext {
                    repository: &repository,
                    config: &config,
                    origin: &origin,
                    stop: &stop,
                    process_cancel: &process_cancel,
                    run_cancel: run_cancel_ref,
                },
                turn,
                RunBatchScope {
                    claimed: &launched.context.claimed,
                    launched: &launched.receipt,
                    bound: &bound,
                    run_start_key: &launched.ids.run_start_key,
                    credentials: &launched.ids.credentials,
                    expected_launch_at: UnixMillis::from_millis(base + 490),
                    expected_updated_at: UnixMillis::from_millis(base + 590),
                },
                EngineId::Codex,
                Some(&mut inbox),
            )
            .await;
            assert!(
                !custody_unresolved,
                "cancelled turn must settle without retained custody"
            );
            let message_id = tokio::time::timeout(TEST_DEADLINE, message_rx)
                .await
                .expect("admitted message arrives")
                .expect("message channel stays open");
            let (dispatch_state, _, _) = repository
                .read_steered_dispatch_state(&message_id)
                .await
                .expect("dispatch state should read");
            assert!(
                matches!(
                    dispatch_state,
                    artisan_database::entities::DispatchState::Completed
                ),
                "steered dispatch must complete, got {dispatch_state:?}"
            );
            assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
        };

        let (serve_result, frames, ()) = tokio::join!(serve, wire, drive);
        let (listener, report) = serve_result.expect("serve must end cleanly");
        assert_eq!(report.completed_requests, 2);
        assert!(matches!(
            report.termination,
            RequestTermination::Failed {
                source: DeadlineError::Cancelled {
                    operation: OperationKind::Receive
                }
            }
        ));
        listener.drain().await.expect("listener drains");
        artisan_transport::shutdown(
            &endpoint,
            quinn::VarInt::from_u32(0),
            b"visible stream test complete",
            TEST_DEADLINE,
        )
        .await
        .expect("endpoint shuts down");

        // Wire order on the collected delivery frames: user admission,
        // assistant chunks (burst-01 and burst-02 prove two increments),
        // one observation event, terminal last after the cancel.
        assert!(!frames.is_empty(), "delivery must stream");
        let mut user_index = None;
        let mut saw_burst_01 = false;
        let mut saw_burst_02 = false;
        let mut observation_index = None;
        let mut terminal_index = None;
        for (index, frame) in frames.iter().enumerate() {
            match frame {
                StreamFrame::PatchBatch(batch) => {
                    for patch in batch.patches() {
                        match patch {
                            ConversationPatch::ItemUpsert { item, .. } => {
                                if let ConversationItem::UserMessage(user) = item
                                    && user.body.as_str() == "hello stream" {
                                        user_index = Some(index);
                                    }
                            }
                            ConversationPatch::ItemAppend { text, .. } => {
                                // Two DISTINCT cumulative updates: one frame
                                // alone proves a single chunk.
                                // Two DISTINCT append updates (burst-01 at
                                // sequence 7 and burst-02 at 8): burst-00
                                // arrives as item patches, never an append.
                                if text.as_str().contains("burst-01") {
                                    saw_burst_01 = true;
                                }
                                if text.as_str().contains("burst-02") {
                                    saw_burst_02 = true;
                                }
                            }
                            ConversationPatch::TurnLifecycle { lifecycle, .. } => {
                                if matches!(
                                    lifecycle,
                                    artisan_domain::ConversationLifecycle::Cancelled
                                ) {
                                    terminal_index = Some(index);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                StreamFrame::ObservationEvent(observation) => {
                    if matches!(
                        observation.observation,
                        artisan_domain::Observation::ReasoningSummaryDelta(_)
                    ) && observation_index.is_none()
                    {
                        observation_index = Some(index);
                    }
                }
            }
        }
        let user_index = user_index.expect("user admission must stream");
        assert!(
            saw_burst_01 && saw_burst_02,
            "two distinct incremental chunks must stream"
        );
        let observation_index =
            observation_index.expect("observation event must stream before terminal");
        let terminal_index = terminal_index.expect("cancelled terminal must stream last");
        assert!(user_index < terminal_index, "admission must precede terminal");
        assert!(
            observation_index < terminal_index,
            "observation must precede terminal"
        );
    })
    .await
    .expect("visible stream settles inside budget");
}
