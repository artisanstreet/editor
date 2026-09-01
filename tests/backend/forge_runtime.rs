//! Focused real-crate tests for explicit Forge process assembly.
//!
//! These tests exercise the signal-free public facade with real migrated
//! storage, real loopback binding, and in-process cancellation. The binary's
//! signal registration remains owned by the synchronous library boundary.

use std::{
    ffi::OsString,
    fs,
    net::SocketAddr,
    num::{NonZeroU32, NonZeroUsize},
    path::{Path, PathBuf},
    process,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use artisan_backend::{
    CommandOriginClockError, ForgeApp, ForgeConfig, ForgeLaunchConfigInput, ForgeProcessCustody,
    ListenerLimits, NativeRunDispatcherConfig, NativeRunDispatcherConfigInput,
    conversation_commit_notifier::ConversationCommitNotifier,
    forge_runtime::{self, ForgeConfigError, ForgeLaunchConfig, ForgeRuntimeError},
    startup_reconciliation_sweep::StartupReconciliationSweepError,
};
use artisan_database::{
    AssistantChange, AttachProjectInput, BindRunProvider, BindRunProviderOutcome, CheckpointUpdate,
    ClaimMessageDispatch, CreateThreadInput, DispatchLeaseOwner, LaunchClaimedRun,
    LaunchClaimedRunOutcome, Repository, RunBatchScope, RunLaunchCredentials, RunStartKey,
    SetThreadEngineConfigInput, SqliteConfig, StartupReconciliationQuery, connect,
    entities::{
        self, AssistantRunLifecycle, ConversationPatchKind, DispatchState, EntityLifecycle,
    },
};
use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantMessagePhase, ByteLimit, CountLimit, DirectoryId,
    DisplayName, EngineAgentId, EngineConfigUpdatePrecondition, EngineModelId,
    EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FilesystemAccess, FiniteMillis, ItemId,
    MessageBody, MessageId, NetworkAccess, OpenCode2Selection, PatchId, PermissionId, ProjectId,
    RequestId, RootPath, RunId, ThreadId, ThreadTitle, TurnId, UnixMillis, WebSearchAccess,
};
use artisan_native_engine::NativeOpenCode2Authority;
use artisan_protocol::{
    APPLICATION_PROTOCOL_VERSION, ClientRequest, FrameId, Hello, HelloCredential,
    LOCAL_CAPABILITY_BYTES, LifecycleRequest, LifecycleResponse, LifecycleState, LifecycleStatus,
    LifecycleStopDisposition, LocalCapability, ProtocolVersion, ResponsePayload, ServerResponse,
    VersionOffer, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, EnvelopeReceiveError, FrameError, LOOPBACK_SERVER_NAME, PinnedIdentity,
    client_config, client_handshake, receive_envelope, send_envelope,
};
use quinn::{Connection, Endpoint, VarInt};
use rustls_pki_types::CertificateDer;
use sea_orm::EntityTrait;

const ADMISSION_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const STARTUP_WAIT: Duration = Duration::from_secs(5);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const FUTURE_WAIT: Duration = Duration::from_secs(10);

const TEST_CAPABILITY: [u8; LOCAL_CAPABILITY_BYTES] = [0x5a; LOCAL_CAPABILITY_BYTES];

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const _: fn() = || {
    struct DefaultMarker;
    trait AmbiguousIfDefault<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDefault<()> for T {}
    impl<T: Default> AmbiguousIfDefault<DefaultMarker> for T {}
    let _ = <ForgeLaunchConfig as AmbiguousIfDefault<_>>::marker;
};

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(label: &str) -> Self {
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "artisan-forge-runtime-{label}-{}-{sequence}",
            process::id()
        ));
        fs::create_dir(&path).expect("isolated Forge directory should be created");
        Self { path }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

struct Credentials {
    certificate: CertificateDer<'static>,
    certificate_path: PathBuf,
    private_key_path: PathBuf,
    capability_path: PathBuf,
}

fn credentials(directory: &TemporaryDirectory) -> Credentials {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])
        .expect("test certificate should be generated");
    let certificate = certified.cert.der().clone();
    let certificate_path = directory.path("leaf.der");
    let private_key_path = directory.path("private-key.der");
    let capability_path = directory.path("bootstrap.cap");
    fs::write(&certificate_path, certificate.as_ref()).expect("certificate should be written");
    fs::write(&private_key_path, certified.signing_key.serialize_der())
        .expect("private key should be written");
    fs::write(&capability_path, [0x5a; LOCAL_CAPABILITY_BYTES])
        .expect("bootstrap capability should be written");
    Credentials {
        certificate,
        certificate_path,
        private_key_path,
        capability_path,
    }
}

fn limits() -> ListenerLimits {
    ListenerLimits {
        admission: ADMISSION_TIMEOUT,
        handshake: HANDSHAKE_TIMEOUT,
        next_request: REQUEST_TIMEOUT,
        drain: DRAIN_TIMEOUT,
    }
}

fn native_run() -> NativeRunDispatcherConfig {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::from_millis(10),
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_millis(500),
            queue_capacity: NonZeroUsize::new(1).expect("one queue slot is nonzero"),
            max_command_retries: NonZeroUsize::new(1).expect("one retry is nonzero"),
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
    )
    .expect("native scheduler should be explicit and valid")
}

fn config(
    directory: &TemporaryDirectory,
    credentials: &Credentials,
    cancel: Arc<CancelHandle>,
) -> ForgeLaunchConfig {
    ForgeLaunchConfig::new(ForgeLaunchConfigInput {
        database: directory.path("forge.sqlite3"),
        custody: directory.path("forge.custody"),
        certificate_der: vec![credentials.certificate_path.clone()],
        private_key_der: credentials.private_key_path.clone(),
        bootstrap_capability: credentials.capability_path.clone(),
        ready_file: directory.path("forge.ready"),
        limits: limits(),
        admission_capacity: NonZeroU32::new(1).expect("one admission is nonzero"),
        requests_per_connection: NonZeroU32::new(2).expect("two requests are nonzero"),
        native_run: native_run(),
        cancel,
    })
    .expect("test launch configuration should be explicit and valid")
}

fn reconciliation_engine_config() -> EngineRunConfig {
    let one = FiniteMillis::new(1).expect("one millisecond should be valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget should be valid"),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit should be valid"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit should be valid"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit should be valid"),
        max_readiness_line_bytes: ByteLimit::new(4_096)
            .expect("readiness line limit should be valid"),
        max_header_count: CountLimit::new(8).expect("header count should be valid"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer limit should be valid"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr limit should be valid"),
        observation_capacity: CountLimit::new(16).expect("observation capacity should be valid"),
    })
    .expect("runtime relationships should be valid");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-runtime-test").expect("permission should parse"),
        EngineAgentId::parse("agent-runtime-test").expect("agent should parse"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-runtime-test").expect("profile should parse"),
            EngineModelId::parse("model-runtime-test").expect("model should parse"),
            EngineRouteId::parse("route-runtime-test").expect("route should parse"),
            None,
            permission,
        )),
        runtime,
    )
}

#[derive(Clone, Copy)]
struct ReconciliationSeed {
    label: &'static str,
    run_id: &'static str,
    item_id: &'static str,
    owner_byte: u8,
    lease_expires_at_ms: i64,
}

fn reconciliation_thread_id(seed: ReconciliationSeed) -> String {
    format!("reconcile-thread-{}", seed.label)
}

fn reconciliation_message_id(seed: ReconciliationSeed) -> String {
    format!("reconcile-message-{}", seed.label)
}

async fn seed_reconciliation_thread(repository: &Repository, seed: ReconciliationSeed) -> ThreadId {
    let project_id = ProjectId::parse(format!("reconcile-project-{}", seed.label))
        .expect("project id should parse");
    let thread_id =
        ThreadId::parse(reconciliation_thread_id(seed)).expect("thread id should parse");
    repository
        .attach_project(AttachProjectInput {
            request_id: RequestId::parse(format!("reconcile-project-request-{}", seed.label))
                .expect("project request should parse"),
            directory_id: DirectoryId::parse(format!("reconcile-directory-{}", seed.label))
                .expect("directory id should parse"),
            project_id: project_id.clone(),
            root_path: RootPath::parse(format!("C:/repos/reconcile-{}", seed.label))
                .expect("root path should parse"),
            display_name: DisplayName::parse(format!("Reconciliation {}", seed.label))
                .expect("display name should parse"),
            attached_at: UnixMillis::from_millis(1),
        })
        .await
        .expect("project should attach through the public repository");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse(format!("reconcile-thread-request-{}", seed.label))
                .expect("thread request should parse"),
            thread_id: thread_id.clone(),
            project_id,
            title: ThreadTitle::parse(format!("Reconciliation {}", seed.label))
                .expect("thread title should parse"),
            created_at: UnixMillis::from_millis(2),
            updated_at: UnixMillis::from_millis(2),
        })
        .await
        .expect("thread should be created through the public repository");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse(format!("reconcile-engine-request-{}", seed.label))
                .expect("engine request should parse"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: reconciliation_engine_config(),
            accepted_at: UnixMillis::from_millis(2),
        })
        .await
        .expect("engine configuration should be persisted");
    thread_id
}

async fn seed_reconciliation_dispatch(
    repository: &Repository,
    seed: ReconciliationSeed,
    thread_id: &ThreadId,
) -> artisan_database::ClaimedMessageDispatch {
    let message_id =
        MessageId::parse(reconciliation_message_id(seed)).expect("message id should parse");
    repository
        .queue_first_message(artisan_database::QueueFirstMessageInput {
            request_id: RequestId::parse(format!("reconcile-message-request-{}", seed.label))
                .expect("message request should parse"),
            message_id: message_id.clone(),
            thread_id: thread_id.clone(),
            body: MessageBody::parse("seeded reconciliation message")
                .expect("message body should parse"),
            accepted_at: UnixMillis::from_millis(3),
        })
        .await
        .expect("message should be queued through the public repository");
    repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([seed.owner_byte; 32]),
            claimed_at: UnixMillis::from_millis(4),
            lease_expires_at: UnixMillis::from_millis(seed.lease_expires_at_ms),
        })
        .await
        .expect("dispatch should be claimed")
        .expect("seeded dispatch should exist")
}

struct SeededRunningRun {
    claimed: artisan_database::ClaimedMessageDispatch,
    launched: artisan_database::LaunchedRunReceipt,
    bound: artisan_database::BoundRunReceipt,
    run_start_key: RunStartKey,
    credentials: RunLaunchCredentials,
    item_id: ItemId,
}

async fn seed_reconciliation_running_run(
    repository: &Repository,
    seed: ReconciliationSeed,
    thread_id: &ThreadId,
    claimed: artisan_database::ClaimedMessageDispatch,
) -> SeededRunningRun {
    let run_id = RunId::parse(seed.run_id).expect("run id should parse");
    let turn_id =
        TurnId::parse(format!("reconcile-turn-{}", seed.label)).expect("turn id should parse");
    let item_id = ItemId::parse(seed.item_id).expect("item id should parse");
    let launch_item_id = ItemId::parse(format!("reconcile-launch-item-{}", seed.label))
        .expect("launch item id should parse");
    let first_patch_id = PatchId::parse(format!("reconcile-launch-turn-{}", seed.label))
        .expect("launch turn patch should parse");
    let second_patch_id = PatchId::parse(format!("reconcile-launch-item-{}", seed.label))
        .expect("launch item patch should parse");
    let run_start_key = RunStartKey::new([seed.owner_byte ^ 0x5a; 32]);
    let credentials = RunLaunchCredentials::new(
        [seed.owner_byte.wrapping_add(0x10); 32],
        [seed.owner_byte.wrapping_add(0x20); 32],
        [seed.owner_byte.wrapping_add(0x30); 32],
    );
    let engine_settings = repository
        .read_thread_engine_settings(thread_id)
        .await
        .expect("engine settings should read")
        .expect("engine settings should be present");
    let launched = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &launch_item_id,
            first_patch_id: &first_patch_id,
            second_patch_id: &second_patch_id,
            operated_at: UnixMillis::from_millis(5),
            run_start_key: &run_start_key,
            credentials: &credentials,
            engine_settings: &engine_settings,
        })
        .await
        .expect("run should launch");
    let LaunchClaimedRunOutcome::Started(launched) = launched else {
        panic!("fresh seed should launch a new run");
    };
    let binding = artisan_database::ProviderBindingBytes::new(vec![seed.owner_byte; 16])
        .expect("provider binding should be valid");
    let bound = match repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &run_start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(5),
            bound_at: UnixMillis::from_millis(6),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await
        .expect("run provider should bind")
    {
        BindRunProviderOutcome::Bound(receipt) | BindRunProviderOutcome::AlreadyBound(receipt) => {
            receipt
        }
    };

    SeededRunningRun {
        claimed,
        launched,
        bound,
        run_start_key,
        credentials,
        item_id,
    }
}

async fn seed_reconciliation_batch(
    repository: &Repository,
    seed: ReconciliationSeed,
    running: &SeededRunningRun,
) {
    let body =
        AssistantBody::parse("seeded assistant output").expect("assistant body should parse");
    let turn_lifecycle_patch = PatchId::parse(format!("reconcile-batch-turn-{}", seed.label))
        .expect("batch turn patch should parse");
    let item_upsert_patch = PatchId::parse(format!("reconcile-batch-item-{}", seed.label))
        .expect("batch item patch should parse");
    let changes = [AssistantChange::Start {
        item_id: &running.item_id,
        phase: AssistantMessagePhase::Final,
        body: &body,
        patch_id: &item_upsert_patch,
    }];
    repository
        .commit_run_batch(artisan_database::CommitRunBatch {
            scope: RunBatchScope {
                claimed: &running.claimed,
                launched: &running.launched,
                bound: &running.bound,
                run_start_key: &running.run_start_key,
                credentials: &running.credentials,
                expected_launch_at: UnixMillis::from_millis(5),
                expected_updated_at: UnixMillis::from_millis(6),
            },
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(7),
            activate_turn_patch_id: Some(&turn_lifecycle_patch),
            changes: &changes,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("assistant output should be committed as a nonterminal batch");
}

async fn seed_reconciliation_candidate(repository: &Repository, seed: ReconciliationSeed) {
    let thread_id = seed_reconciliation_thread(repository, seed).await;
    let claimed = seed_reconciliation_dispatch(repository, seed, &thread_id).await;
    let running = seed_reconciliation_running_run(repository, seed, &thread_id, claimed).await;
    seed_reconciliation_batch(repository, seed, &running).await;
}

fn seed_reconciliation_database(directory: &TemporaryDirectory, seeds: &[ReconciliationSeed]) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("seed runtime should build");
    runtime.block_on(async {
        let app = ForgeApp::start(ForgeConfig::new(SqliteConfig::file(
            directory.path("forge.sqlite3"),
        )))
        .await
        .expect("seed database should start");
        let repository = app.repository().clone();
        for &seed in seeds {
            seed_reconciliation_candidate(&repository, seed).await;
        }
        app.shutdown().await.expect("seed database should close");
    });
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LifecyclePatchSnapshot {
    patch_id: String,
    sequence: i64,
    kind: ConversationPatchKind,
    lifecycle: Option<EntityLifecycle>,
    recorded_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconciliationPresence {
    Absent,
    Present,
}

impl ReconciliationPresence {
    fn from_option<T>(value: Option<&T>) -> Self {
        match value {
            Some(_) => Self::Present,
            None => Self::Absent,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReconciliationSnapshot {
    dispatch_state: DispatchState,
    dispatch_updated_at_ms: i64,
    dispatch_lease_owner: ReconciliationPresence,
    dispatch_lease_expires_at_ms: Option<i64>,
    dispatch_last_error: Option<String>,
    run_lifecycle: AssistantRunLifecycle,
    run_updated_at_ms: i64,
    run_owner: ReconciliationPresence,
    run_lease: ReconciliationPresence,
    run_provider_binding: ReconciliationPresence,
    run_error_code: Option<String>,
    run_error_message: Option<String>,
    turn_lifecycle: EntityLifecycle,
    turn_updated_at_ms: i64,
    item_lifecycle: EntityLifecycle,
    item_updated_at_ms: i64,
    lifecycle_patches: Vec<LifecyclePatchSnapshot>,
}

async fn reconciliation_snapshot(path: &Path, seed: ReconciliationSeed) -> ReconciliationSnapshot {
    let database = connect(
        SqliteConfig::file(path.to_path_buf())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("snapshot database should open");
    let dispatch = entities::message_dispatch::Entity::find_by_id(reconciliation_message_id(seed))
        .one(&database)
        .await
        .expect("dispatch should load")
        .expect("dispatch should exist");
    let run = entities::assistant_run::Entity::find_by_id(seed.run_id.to_owned())
        .one(&database)
        .await
        .expect("run should load")
        .expect("run should exist");
    let turn_id = format!("reconcile-turn-{}", seed.label);
    let turn = entities::conversation_turn::Entity::find_by_id(turn_id)
        .one(&database)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    let item = entities::conversation_item::Entity::find_by_id(seed.item_id.to_owned())
        .one(&database)
        .await
        .expect("item should load")
        .expect("item should exist");
    let thread_id = reconciliation_thread_id(seed);
    let mut lifecycle_patches = entities::conversation_patch::Entity::find()
        .all(&database)
        .await
        .expect("patches should load")
        .into_iter()
        .filter(|patch| patch.thread_id == thread_id)
        .filter(|patch| {
            patch.kind == ConversationPatchKind::TurnLifecycle
                || patch.kind == ConversationPatchKind::ItemLifecycle
        })
        .map(|patch| LifecyclePatchSnapshot {
            patch_id: patch.patch_id,
            sequence: patch.sequence,
            kind: patch.kind,
            lifecycle: patch.lifecycle,
            recorded_at_ms: patch.recorded_at_ms,
        })
        .collect::<Vec<_>>();
    lifecycle_patches.sort_by_key(|patch| patch.sequence);

    let snapshot = ReconciliationSnapshot {
        dispatch_state: dispatch.state,
        dispatch_updated_at_ms: dispatch.updated_at_ms,
        dispatch_lease_owner: ReconciliationPresence::from_option(dispatch.lease_owner.as_ref()),
        dispatch_lease_expires_at_ms: dispatch.lease_expires_at_ms,
        dispatch_last_error: dispatch.last_error,
        run_lifecycle: run.lifecycle,
        run_updated_at_ms: run.updated_at_ms,
        run_owner: ReconciliationPresence::from_option(run.owner.as_ref()),
        run_lease: ReconciliationPresence::from_option(run.lease.as_ref()),
        run_provider_binding: ReconciliationPresence::from_option(run.provider_binding.as_ref()),
        run_error_code: run.error_code,
        run_error_message: run.error_message,
        turn_lifecycle: turn.lifecycle,
        turn_updated_at_ms: turn.updated_at_ms,
        item_lifecycle: item.lifecycle,
        item_updated_at_ms: item.updated_at_ms,
        lifecycle_patches,
    };
    database
        .close()
        .await
        .expect("snapshot database should close");
    snapshot
}

async fn startup_candidate_count(path: &Path) -> usize {
    let database = connect(
        SqliteConfig::file(path.to_path_buf())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("candidate database should open");
    let repository = Repository::new(database.clone());
    let query = StartupReconciliationQuery::new(UnixMillis::from_millis(i64::MAX), 64)
        .expect("maximum startup reconciliation query should be valid");
    let count = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .expect("candidate query should succeed")
        .len();
    database
        .close()
        .await
        .expect("candidate database should close");
    count
}

fn client_endpoint(credentials: &Credentials) -> Endpoint {
    let identity = PinnedIdentity::from_certificate(&credentials.certificate);
    let client = client_config(credentials.certificate.clone(), identity)
        .expect("test client configuration should be valid");
    artisan_transport::bind_loopback_client(client).expect("test client should bind")
}

fn hello_envelope_with_lifecycle(supports_lifecycle_control: bool) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("forge-test-hello").expect("test frame id should be valid"),
        sent_at: UnixMillis::from_millis(1),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![APPLICATION_PROTOCOL_VERSION])
                .expect("test version offer should be valid"),
            credential: HelloCredential::Initial(LocalCapability::from_bytes(TEST_CAPABILITY)),
            supports_lifecycle_control,
        }),
    }
}

fn hello_envelope() -> WireEnvelope {
    hello_envelope_with_lifecycle(false)
}

async fn authenticate_client_with_offer(
    client: &Endpoint,
    address: SocketAddr,
    supports_lifecycle_control: bool,
) -> (Connection, bool) {
    let connecting = client
        .connect(address, LOOPBACK_SERVER_NAME)
        .expect("test client should begin connecting");
    let connection = tokio::time::timeout(FUTURE_WAIT, connecting)
        .await
        .expect("test client connection should settle")
        .expect("test client connection should establish");
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .expect("test control stream should open");
    let hello = if supports_lifecycle_control {
        hello_envelope_with_lifecycle(true)
    } else {
        hello_envelope()
    };
    let welcome = tokio::time::timeout(
        FUTURE_WAIT,
        client_handshake(&mut send, &mut receive, hello),
    )
    .await
    .expect("test handshake should settle")
    .expect("test handshake should succeed");
    let lifecycle_supported = welcome.welcome.lifecycle_control_supported;
    drop(send);
    drop(receive);
    (connection, lifecycle_supported)
}

async fn authenticate_client(client: &Endpoint, address: SocketAddr) -> Connection {
    let (connection, supported) = authenticate_client_with_offer(client, address, false).await;
    assert!(
        !supported,
        "lifecycle control must not be advertised without a client offer"
    );
    connection
}

async fn authenticate_lifecycle_client(
    client: &Endpoint,
    address: SocketAddr,
) -> (Connection, bool) {
    authenticate_client_with_offer(client, address, true).await
}

async fn lifecycle_request(
    connection: &Connection,
    frame: &str,
    request: LifecycleRequest,
    expect_peer_fin: bool,
) -> WireEnvelope {
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .expect("lifecycle request stream should open");
    let envelope = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame).expect("lifecycle frame id should be valid"),
        sent_at: UnixMillis::from_millis(2),
        body: WireEnvelopeBody::Request(ClientRequest::Lifecycle(request)),
    };
    // Keep the peer receive active before the server can finish an accepted
    // stop. The connection owner waits for Quinn's finished-send
    // acknowledgement before it commits the stop and cancels the runtime.
    let response = tokio::time::timeout(FUTURE_WAIT, receive_envelope(&mut receive));
    let (response, ()) = tokio::join!(response, async {
        tokio::time::timeout(FUTURE_WAIT, send_envelope(&mut send, &envelope))
            .await
            .expect("lifecycle request send should settle")
            .expect("lifecycle request should be written");
        send.finish().expect("lifecycle request FIN should succeed");
    },);
    let response = response
        .expect("lifecycle response should settle")
        .expect("lifecycle response should be readable");
    if expect_peer_fin {
        let end_of_stream = tokio::time::timeout(FUTURE_WAIT, receive_envelope(&mut receive))
            .await
            .expect("lifecycle response FIN should settle");
        assert!(matches!(
            end_of_stream,
            Err(EnvelopeReceiveError::Frame(FrameError::Truncated { .. }))
        ));
    }
    response
}

async fn connect_without_auth(client: &Endpoint, address: SocketAddr) -> Connection {
    let connecting = client
        .connect(address, LOOPBACK_SERVER_NAME)
        .expect("test client should begin connecting");
    tokio::time::timeout(FUTURE_WAIT, connecting)
        .await
        .expect("test transport connection should settle")
        .expect("test transport connection should establish")
}

fn append_path(arguments: &mut Vec<OsString>, option: &str, path: PathBuf) {
    arguments.push(OsString::from(option));
    arguments.push(path.into_os_string());
}

fn parser_arguments(directory: &TemporaryDirectory) -> Vec<OsString> {
    let mut arguments = Vec::new();
    append_path(
        &mut arguments,
        "--database",
        directory.path("database.sqlite3"),
    );
    append_path(
        &mut arguments,
        "--custody",
        directory.path("process.custody"),
    );
    append_path(
        &mut arguments,
        "--certificate-der",
        directory.path("leaf.der"),
    );
    append_path(
        &mut arguments,
        "--certificate-der",
        directory.path("intermediate.der"),
    );
    append_path(
        &mut arguments,
        "--private-key-der",
        directory.path("private-key.der"),
    );
    append_path(
        &mut arguments,
        "--bootstrap-capability",
        directory.path("bootstrap.cap"),
    );
    append_path(&mut arguments, "--ready-file", directory.path("ready.json"));
    arguments.extend([
        OsString::from("--admission-timeout-ms"),
        OsString::from("11"),
        OsString::from("--handshake-timeout-ms"),
        OsString::from("12"),
        OsString::from("--request-timeout-ms"),
        OsString::from("13"),
        OsString::from("--drain-timeout-ms"),
        OsString::from("14"),
        OsString::from("--admission-capacity"),
        OsString::from("3"),
        OsString::from("--requests-per-connection"),
        OsString::from("4"),
        OsString::from("--native-run-claim-lease-ms"),
        OsString::from("15"),
        OsString::from("--native-run-poll-interval-ms"),
        OsString::from("16"),
        OsString::from("--native-run-retry-backoff-ms"),
        OsString::from("17"),
        OsString::from("--native-run-shutdown-budget-ms"),
        OsString::from("18"),
        OsString::from("--native-run-queue-capacity"),
        OsString::from("5"),
        OsString::from("--native-run-max-command-retries"),
        OsString::from("6"),
        OsString::from("--native-run-prompt-delivery"),
        OsString::from("queue"),
        OsString::from("--native-run-stream-after"),
        OsString::from("7"),
    ]);
    arguments
}

fn replace_option_value(arguments: &mut [OsString], option: &str, value: OsString) {
    let position = arguments
        .iter()
        .position(|argument| argument.as_os_str().to_str() == Some(option))
        .expect("test option should exist");
    arguments[position + 1] = value;
}

fn parse(arguments: Vec<OsString>) -> Result<ForgeLaunchConfig, ForgeConfigError> {
    forge_runtime::parse_args(arguments, Arc::new(CancelHandle::new()))
}

#[test]
fn exact_parser_requires_every_field_and_preserves_certificate_order() {
    let directory = TemporaryDirectory::new("parser");
    let cancel = Arc::new(CancelHandle::new());
    let parsed = ForgeLaunchConfig::from_args(parser_arguments(&directory), Arc::clone(&cancel))
        .expect("the exact long-form contract should parse");

    assert_eq!(parsed.database_path(), directory.path("database.sqlite3"));
    assert_eq!(parsed.custody_path(), directory.path("process.custody"));
    assert_eq!(
        parsed.certificate_der_paths(),
        &[
            directory.path("leaf.der"),
            directory.path("intermediate.der"),
        ]
    );
    assert_eq!(
        parsed.private_key_der_path(),
        directory.path("private-key.der")
    );
    assert_eq!(
        parsed.bootstrap_capability_path(),
        directory.path("bootstrap.cap")
    );
    assert_eq!(parsed.ready_file_path(), directory.path("ready.json"));
    assert_eq!(
        parsed.listener_limits().admission,
        Duration::from_millis(11)
    );
    assert_eq!(
        parsed.listener_limits().handshake,
        Duration::from_millis(12)
    );
    assert_eq!(
        parsed.listener_limits().next_request,
        Duration::from_millis(13)
    );
    assert_eq!(parsed.listener_limits().drain, Duration::from_millis(14));
    assert_eq!(parsed.admission_capacity().get(), 3);
    assert_eq!(parsed.requests_per_connection().get(), 4);
    let debug = format!("{parsed:?}");
    assert!(debug.contains("native_run: \"configured\""));
    assert!(Arc::ptr_eq(parsed.cancel_handle(), &cancel));
}

#[test]
fn parser_rejects_missing_duplicate_unknown_relative_empty_zero_and_overflow() {
    let directory = TemporaryDirectory::new("parser-rejections");
    let base = parser_arguments(&directory);

    assert!(matches!(
        parse(Vec::new()),
        Err(ForgeConfigError::MissingOption { .. })
    ));
    assert!(matches!(
        parse(vec![OsString::from("--database")]),
        Err(ForgeConfigError::MissingValue { .. })
    ));

    let mut duplicate = base.clone();
    append_path(
        &mut duplicate,
        "--database",
        directory.path("another.sqlite3"),
    );
    assert!(matches!(
        parse(duplicate),
        Err(ForgeConfigError::Duplicate { .. })
    ));

    let mut unknown = base.clone();
    unknown.push(OsString::from("--data-dir"));
    unknown.push(directory.path("legacy").into_os_string());
    assert!(matches!(
        parse(unknown),
        Err(ForgeConfigError::UnknownOption)
    ));

    let mut relative = base.clone();
    replace_option_value(&mut relative, "--database", OsString::from("forge.sqlite3"));
    assert!(matches!(
        parse(relative),
        Err(ForgeConfigError::RelativePath { .. })
    ));

    let mut empty = base.clone();
    replace_option_value(&mut empty, "--ready-file", OsString::new());
    assert!(matches!(
        parse(empty),
        Err(ForgeConfigError::EmptyPath { .. })
    ));

    let mut zero = base.clone();
    replace_option_value(&mut zero, "--admission-capacity", OsString::from("0"));
    assert!(matches!(
        parse(zero),
        Err(ForgeConfigError::ZeroCapacity { .. })
    ));

    let mut native_zero = base.clone();
    replace_option_value(
        &mut native_zero,
        "--native-run-queue-capacity",
        OsString::from("0"),
    );
    assert!(matches!(
        parse(native_zero),
        Err(ForgeConfigError::ZeroCapacity { .. })
    ));

    let mut native_duration_zero = base.clone();
    replace_option_value(
        &mut native_duration_zero,
        "--native-run-claim-lease-ms",
        OsString::from("0"),
    );
    assert!(matches!(
        parse(native_duration_zero),
        Err(ForgeConfigError::NativeRunConfiguration { .. })
    ));

    let mut capacity_overflow = base.clone();
    replace_option_value(
        &mut capacity_overflow,
        "--requests-per-connection",
        OsString::from("4294967296"),
    );
    assert!(matches!(
        parse(capacity_overflow),
        Err(ForgeConfigError::NumberOverflow { .. })
    ));

    let mut timeout_overflow = base.clone();
    replace_option_value(
        &mut timeout_overflow,
        "--request-timeout-ms",
        OsString::from("18446744073709551616"),
    );
    assert!(matches!(
        parse(timeout_overflow),
        Err(ForgeConfigError::NumberOverflow { .. })
    ));

    let mut malformed = base;
    replace_option_value(&mut malformed, "--drain-timeout-ms", OsString::from("-1"));
    assert!(matches!(
        parse(malformed),
        Err(ForgeConfigError::InvalidNumber { .. })
    ));

    let mut duplicate_native = parser_arguments(&directory);
    duplicate_native.extend([
        OsString::from("--native-run-poll-interval-ms"),
        OsString::from("19"),
    ]);
    assert!(matches!(
        parse(duplicate_native),
        Err(ForgeConfigError::Duplicate { .. })
    ));
}

#[test]
fn parser_requires_each_native_dispatcher_option() {
    let directory = TemporaryDirectory::new("native-parser-requirements");
    let options = [
        "--native-run-claim-lease-ms",
        "--native-run-poll-interval-ms",
        "--native-run-retry-backoff-ms",
        "--native-run-shutdown-budget-ms",
        "--native-run-queue-capacity",
        "--native-run-max-command-retries",
        "--native-run-prompt-delivery",
        "--native-run-stream-after",
    ];

    for option in options {
        let mut missing = parser_arguments(&directory);
        let position = missing
            .iter()
            .position(|argument| argument.as_os_str().to_str() == Some(option))
            .expect("native option should exist");
        missing.drain(position..=position + 1);
        assert!(matches!(
            parse(missing),
            Err(ForgeConfigError::MissingOption { option: reported }) if reported == option
        ));
    }
}

#[test]
fn explicit_paths_are_the_only_configuration_and_secret_diagnostics_stay_clean() {
    let directory = TemporaryDirectory::new("explicit");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let config = config(&directory, &credentials, cancel);
    let debug = format!("{config:?}");
    let capability_hex = "5a".repeat(LOCAL_CAPABILITY_BYTES);
    assert_eq!(config.database_path(), directory.path("forge.sqlite3"));
    assert_eq!(config.custody_path(), directory.path("forge.custody"));
    assert_eq!(
        config.certificate_der_paths(),
        std::slice::from_ref(&credentials.certificate_path)
    );
    assert_eq!(
        config.private_key_der_path(),
        credentials.private_key_path.as_path()
    );
    assert_eq!(
        config.bootstrap_capability_path(),
        credentials.capability_path.as_path()
    );
    assert_eq!(config.ready_file_path(), directory.path("forge.ready"));
    assert!(!debug.contains(&capability_hex));
    assert!(!debug.contains("database.sqlite3"));

    let invalid_capability = directory.path("invalid.cap");
    fs::write(&invalid_capability, [0xa5; LOCAL_CAPABILITY_BYTES - 1])
        .expect("invalid capability should be written");
    let invalid_config = ForgeLaunchConfig::new(ForgeLaunchConfigInput {
        database: directory.path("not-default.sqlite3"),
        custody: directory.path("not-default.custody"),
        certificate_der: vec![credentials.certificate_path.clone()],
        private_key_der: credentials.private_key_path.clone(),
        bootstrap_capability: invalid_capability,
        ready_file: directory.path("not-default.ready"),
        limits: limits(),
        admission_capacity: NonZeroU32::new(1).expect("one is nonzero"),
        requests_per_connection: NonZeroU32::new(1).expect("one is nonzero"),
        native_run: native_run(),
        cancel: Arc::new(CancelHandle::new()),
    })
    .expect("paths should be explicit even when files are not ready");
    let error = run_config(invalid_config);
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_CONFIGURATION);
    let display = error.to_string();
    let debug = format!("{error:?}");
    assert!(!display.contains("a5a5"));
    assert!(!debug.contains("a5a5"));
    assert!(!directory.path("not-default.sqlite3").exists());
    assert!(!directory.path("not-default.custody").exists());
    assert!(!directory.path("not-default.ready").exists());
}

#[test]
fn exact_capability_length_is_checked_before_custody() {
    let directory = TemporaryDirectory::new("capability-length");
    let credentials = credentials(&directory);
    fs::write(
        &credentials.capability_path,
        [0x7f; LOCAL_CAPABILITY_BYTES + 1],
    )
    .expect("wrong-length capability should be written");
    let error = run_config(config(
        &directory,
        &credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_CONFIGURATION);
    assert!(!directory.path("forge.custody").exists());
    assert!(!directory.path("forge.sqlite3").exists());
    assert!(!directory.path("forge.ready").exists());
}

#[test]
fn custody_contention_returns_75_before_sqlite_creation() {
    let directory = TemporaryDirectory::new("custody-contention");
    let credentials = credentials(&directory);
    let custody_path = directory.path("forge.custody");
    let owner = ForgeProcessCustody::acquire(&custody_path).expect("test should own custody");

    let error = run_config(config(
        &directory,
        &credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_CUSTODY);
    assert!(!directory.path("forge.sqlite3").exists());
    assert!(!directory.path("forge.ready").exists());
    drop(owner);
}

#[test]
fn storage_failure_returns_70_without_readiness_and_releases_custody() {
    let directory = TemporaryDirectory::new("storage-failure");
    let credentials = credentials(&directory);
    fs::write(directory.path("forge.sqlite3"), b"not an sqlite database")
        .expect("malformed database should be written");

    let error = run_config(config(
        &directory,
        &credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(
        error.exit_code(),
        forge_runtime::EXIT_CODE_APPLICATION_STARTUP
    );
    assert!(!directory.path("forge.ready").exists());
    let custody = ForgeProcessCustody::acquire(directory.path("forge.custody"))
        .expect("custody should be released after storage startup failure");
    drop(custody);
}

#[test]
fn startup_reconciliation_failures_map_to_application_startup() {
    let clock = ForgeRuntimeError::StartupReconciliationClock(CommandOriginClockError);
    assert_eq!(
        clock.exit_code(),
        forge_runtime::EXIT_CODE_APPLICATION_STARTUP
    );
    assert_eq!(
        clock.to_string(),
        "Forge startup reconciliation clock failed"
    );

    let sweep = ForgeRuntimeError::StartupReconciliation(Box::new(
        StartupReconciliationSweepError::InvalidLimit { limit: 0 },
    ));
    assert_eq!(
        sweep.exit_code(),
        forge_runtime::EXIT_CODE_APPLICATION_STARTUP
    );
    assert_eq!(sweep.to_string(), "Forge startup reconciliation failed");
}

fn run_reconciliation_start_and_snapshot(
    directory: &TemporaryDirectory,
    credentials: &Credentials,
    seed: ReconciliationSeed,
    worker_owned_message: &str,
    shutdown_message: &str,
    readiness_message: Option<&str>,
) -> ReconciliationSnapshot {
    let cancel = Arc::new(CancelHandle::new());
    let mut worker = Some(spawn_runtime(config(
        directory,
        credentials,
        Arc::clone(&cancel),
    )));
    let _ = wait_for_readiness_or_stop(&directory.path("forge.ready"), &cancel, &mut worker);
    cancel.cancel();
    assert!(
        join_within(worker.take().expect(worker_owned_message), SHUTDOWN_WAIT,).is_ok(),
        "{shutdown_message}"
    );
    if let Some(readiness_message) = readiness_message {
        assert!(
            !directory.path("forge.ready").exists(),
            "{readiness_message}"
        );
    } else {
        assert!(!directory.path("forge.ready").exists());
    }
    run_snapshot(directory, seed)
}

fn assert_reconciled_snapshot(snapshot: &ReconciliationSnapshot, seed: ReconciliationSeed) {
    assert_eq!(snapshot.dispatch_state, DispatchState::Failed);
    assert_eq!(snapshot.dispatch_lease_expires_at_ms, None);
    assert_eq!(
        snapshot.dispatch_lease_owner,
        ReconciliationPresence::Absent
    );
    assert_eq!(
        snapshot.dispatch_last_error.as_deref(),
        Some("startup reconciliation: unknown outcome after lease expiry")
    );
    assert_eq!(snapshot.run_lifecycle, AssistantRunLifecycle::Interrupted);
    assert_eq!(snapshot.run_owner, ReconciliationPresence::Absent);
    assert_eq!(snapshot.run_lease, ReconciliationPresence::Absent);
    assert_eq!(
        snapshot.run_provider_binding,
        ReconciliationPresence::Present
    );
    assert_eq!(
        snapshot.run_error_code.as_deref(),
        Some("startup_reconciliation_unknown_outcome")
    );
    assert_eq!(
        snapshot.run_error_message.as_deref(),
        Some(
            "startup reconciliation interrupted with unknown outcome; provider state may have progressed"
        )
    );
    assert_eq!(snapshot.turn_lifecycle, EntityLifecycle::Interrupted);
    assert_eq!(snapshot.item_lifecycle, EntityLifecycle::Interrupted);
    assert_eq!(snapshot.dispatch_updated_at_ms, snapshot.run_updated_at_ms);
    assert_eq!(snapshot.run_updated_at_ms, snapshot.turn_updated_at_ms);
    assert_eq!(snapshot.turn_updated_at_ms, snapshot.item_updated_at_ms);
    assert!(snapshot.lifecycle_patches.iter().any(|patch| {
        patch.patch_id == seed.run_id
            && patch.kind == ConversationPatchKind::TurnLifecycle
            && patch.lifecycle.as_ref() == Some(&EntityLifecycle::Interrupted)
            && patch.recorded_at_ms == snapshot.run_updated_at_ms
    }));
    assert!(snapshot.lifecycle_patches.iter().any(|patch| {
        patch.patch_id == seed.item_id
            && patch.kind == ConversationPatchKind::ItemLifecycle
            && patch.lifecycle.as_ref() == Some(&EntityLifecycle::Interrupted)
            && patch.recorded_at_ms == snapshot.run_updated_at_ms
    }));
}

#[test]
fn startup_reconciliation_runs_before_readiness_and_second_start_is_idempotent() {
    let directory = TemporaryDirectory::new("startup-reconciliation");
    let credentials = credentials(&directory);
    let seed = ReconciliationSeed {
        label: "recovered",
        run_id: "reconcile-run-recovered",
        item_id: "reconcile-item-recovered",
        owner_byte: 0x11,
        lease_expires_at_ms: 50,
    };
    seed_reconciliation_database(&directory, &[seed]);

    let first_snapshot = run_reconciliation_start_and_snapshot(
        &directory,
        &credentials,
        seed,
        "first reconciliation worker should still be owned",
        "first Forge shutdown should be clean",
        Some("clean shutdown should remove the readiness receipt"),
    );
    assert_reconciled_snapshot(&first_snapshot, seed);
    assert_eq!(startup_candidate_count_blocking(&directory), 0);

    let second_snapshot = run_reconciliation_start_and_snapshot(
        &directory,
        &credentials,
        seed,
        "second reconciliation worker should still be owned",
        "second Forge shutdown should be clean",
        None,
    );
    assert_eq!(
        second_snapshot, first_snapshot,
        "a clean second startup should not duplicate lifecycle effects"
    );
    assert_eq!(startup_candidate_count_blocking(&directory), 0);
    let custody = ForgeProcessCustody::acquire(directory.path("forge.custody"))
        .expect("custody should be reacquirable after the second shutdown");
    drop(custody);
    assert_no_readiness_temporary(&directory);
}

#[test]
fn startup_reconciliation_failure_keeps_primary_and_committed_prefix() {
    let directory = TemporaryDirectory::new("startup-reconciliation-failure");
    let credentials = credentials(&directory);
    let valid = ReconciliationSeed {
        label: "prefix",
        run_id: "reconcile-run-prefix",
        item_id: "reconcile-item-prefix",
        owner_byte: 0x11,
        lease_expires_at_ms: 50,
    };
    let colliding = ReconciliationSeed {
        label: "collision",
        run_id: "reconcile-run-collision",
        item_id: "reconcile-run-collision",
        owner_byte: 0x22,
        lease_expires_at_ms: 60,
    };
    seed_reconciliation_database(&directory, &[valid, colliding]);
    let valid_before = run_snapshot(&directory, valid);
    let colliding_before = run_snapshot(&directory, colliding);

    let error = run_config(config(
        &directory,
        &credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(
        error.exit_code(),
        forge_runtime::EXIT_CODE_APPLICATION_STARTUP
    );
    let primary = error.primary_failure().unwrap_or(&error);
    match primary {
        ForgeRuntimeError::StartupReconciliation(error) => {
            let StartupReconciliationSweepError::PatchShape {
                report,
                failing_index,
                failing_run_id,
                reason,
            } = error.as_ref()
            else {
                panic!("expected typed startup sweep failure, got {error:?}");
            };
            assert_eq!(report.discovered, 2);
            assert_eq!(report.attempted, 1);
            assert_eq!(report.interrupted, 1);
            assert_eq!(*failing_index, 1);
            assert_eq!(failing_run_id.as_str(), colliding.run_id);
            assert_eq!(*reason, "turn and item patch identities collide");
        }
        other => panic!("expected typed startup sweep failure, got {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "Forge startup reconciliation failed",
        "the primary error display must remain bounded and content-free"
    );
    assert!(!directory.path("forge.ready").exists());
    assert_no_readiness_temporary(&directory);

    let valid_after = run_snapshot(&directory, valid);
    assert_eq!(valid_after.dispatch_state, DispatchState::Failed);
    assert_eq!(
        valid_after.run_lifecycle,
        AssistantRunLifecycle::Interrupted
    );
    assert_eq!(valid_after.turn_lifecycle, EntityLifecycle::Interrupted);
    assert_eq!(valid_after.item_lifecycle, EntityLifecycle::Interrupted);
    assert_eq!(
        valid_after.run_provider_binding,
        ReconciliationPresence::Present
    );
    assert_eq!(
        valid_after.lifecycle_patches.len(),
        valid_before.lifecycle_patches.len() + 2,
        "the successful first disposition must remain durable"
    );
    let colliding_after = run_snapshot(&directory, colliding);
    assert_eq!(
        colliding_after, colliding_before,
        "the failing candidate must remain byte-for-byte unchanged"
    );
    assert_eq!(
        startup_candidate_count_blocking(&directory),
        1,
        "the unprocessed candidate must remain eligible after the durable prefix"
    );
    let custody = ForgeProcessCustody::acquire(directory.path("forge.custody"))
        .expect("custody should be released after sweep failure");
    drop(custody);
}

#[test]
fn invalid_tls_and_existing_readiness_paths_return_71_after_cleanup() {
    let tls_directory = TemporaryDirectory::new("invalid-tls");
    let tls_credentials = credentials(&tls_directory);
    fs::write(&tls_credentials.private_key_path, [0xa5; 8])
        .expect("invalid private key should be written");
    let tls_error = run_config(config(
        &tls_directory,
        &tls_credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(
        tls_error.exit_code(),
        forge_runtime::EXIT_CODE_SERVER_STARTUP
    );
    assert!(!tls_directory.path("forge.ready").exists());
    assert!(!format!("{tls_error:?}").contains("a5a5"));
    let tls_custody = ForgeProcessCustody::acquire(tls_directory.path("forge.custody"))
        .expect("custody should be released after TLS startup failure");
    drop(tls_custody);

    let bind_directory = TemporaryDirectory::new("invalid-bind");
    let bind_credentials = credentials(&bind_directory);
    let bind_config = ForgeLaunchConfig::new(ForgeLaunchConfigInput {
        database: bind_directory.path("forge.sqlite3"),
        custody: bind_directory.path("forge.custody"),
        certificate_der: vec![bind_credentials.certificate_path.clone()],
        private_key_der: bind_credentials.private_key_path.clone(),
        bootstrap_capability: bind_credentials.capability_path.clone(),
        ready_file: bind_directory.path("forge.ready"),
        limits: ListenerLimits {
            admission: Duration::MAX,
            ..limits()
        },
        admission_capacity: NonZeroU32::new(1).expect("one is nonzero"),
        requests_per_connection: NonZeroU32::new(1).expect("one is nonzero"),
        native_run: native_run(),
        cancel: Arc::new(CancelHandle::new()),
    })
    .expect("listener-level invalid limits are still explicit configuration");
    let bind_error = run_config(bind_config);
    assert_eq!(
        bind_error.exit_code(),
        forge_runtime::EXIT_CODE_SERVER_STARTUP
    );
    assert!(!bind_directory.path("forge.ready").exists());
    let bind_custody = ForgeProcessCustody::acquire(bind_directory.path("forge.custody"))
        .expect("custody should be released after listener bind failure");
    drop(bind_custody);

    let readiness_directory = TemporaryDirectory::new("existing-readiness");
    let readiness_credentials = credentials(&readiness_directory);
    let sentinel = b"pre-existing readiness target";
    fs::write(readiness_directory.path("forge.ready"), sentinel)
        .expect("sentinel readiness target should be written");
    let readiness_error = run_config(config(
        &readiness_directory,
        &readiness_credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(
        readiness_error.exit_code(),
        forge_runtime::EXIT_CODE_SERVER_STARTUP
    );
    assert_eq!(
        fs::read(readiness_directory.path("forge.ready")).expect("sentinel should survive"),
        sentinel
    );
    let readiness_custody = ForgeProcessCustody::acquire(readiness_directory.path("forge.custody"))
        .expect("custody should be released after readiness failure");
    drop(readiness_custody);
}

#[test]
fn readiness_is_exact_and_shutdown_removes_only_this_receipt() {
    let directory = TemporaryDirectory::new("ready-lifecycle");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let mut worker = Some(spawn_runtime(config(
        &directory,
        &credentials,
        Arc::clone(&cancel),
    )));
    let ready_path = directory.path("forge.ready");
    let (bytes, value) = wait_for_readiness_or_stop(&ready_path, &cancel, &mut worker);

    assert_eq!(value["schema"].as_str(), Some(forge_runtime::READY_SCHEMA));
    let endpoint: SocketAddr = value["endpoint"]
        .as_str()
        .expect("readiness endpoint should be text")
        .parse()
        .expect("readiness endpoint should be a socket address");
    assert!(endpoint.ip().is_loopback());
    assert_ne!(endpoint.port(), 0);
    let expected_fingerprint = PinnedIdentity::from_certificate(&credentials.certificate).to_hex();
    assert_eq!(
        value["certificate_sha256"].as_str(),
        Some(expected_fingerprint.as_str())
    );
    assert_eq!(value["pid"].as_u64(), Some(u64::from(process::id())));
    let expected = format!(
        "{{\"schema\":\"{}\",\"endpoint\":\"{}\",\"certificate_sha256\":\"{}\",\"pid\":{}}}\n",
        forge_runtime::READY_SCHEMA,
        endpoint,
        expected_fingerprint,
        process::id(),
    );
    assert_eq!(bytes, expected.as_bytes());
    assert!(!String::from_utf8_lossy(&bytes).contains(&"5a".repeat(32)));
    assert!(
        !worker
            .as_ref()
            .expect("readiness worker should still be owned")
            .is_finished(),
        "service must remain alive before cancel"
    );

    cancel.cancel();
    let result = join_within(
        worker
            .take()
            .expect("readiness worker should still be owned"),
        SHUTDOWN_WAIT,
    );
    assert!(
        result.is_ok(),
        "cancellation should be a clean shutdown: {result:?}"
    );
    assert!(!ready_path.exists(), "this run's receipt should be removed");
    assert_no_readiness_temporary(&directory);

    let custody = ForgeProcessCustody::acquire(directory.path("forge.custody"))
        .expect("custody should be reacquirable after clean shutdown");
    drop(custody);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("reopen runtime should build");
    runtime.block_on(async {
        tokio::time::timeout(FUTURE_WAIT, async {
            let app = ForgeApp::start(ForgeConfig::new(SqliteConfig::file(
                directory.path("forge.sqlite3"),
            )))
            .await
            .expect("database should be released and reopenable");
            app.shutdown()
                .await
                .expect("reopened database should close");
        })
        .await
        .expect("reopened database future should be bounded");
    });
}

#[test]
fn lifecycle_offer_reports_idle_status_and_stops_after_correlated_finished_reply() {
    let directory = TemporaryDirectory::new("lifecycle-runtime");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let mut worker = Some(spawn_runtime(config(
        &directory,
        &credentials,
        Arc::clone(&cancel),
    )));
    let (_, value) =
        wait_for_readiness_or_stop(&directory.path("forge.ready"), &cancel, &mut worker);
    let address = readiness_endpoint(&value);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("lifecycle client runtime should build");
    let client = {
        let _entered = runtime.enter();
        client_endpoint(&credentials)
    };
    let (connection, supported) = runtime.block_on(authenticate_lifecycle_client(&client, address));
    assert!(
        supported,
        "an offered lifecycle feature should be advertised"
    );

    let status = runtime.block_on(lifecycle_request(
        &connection,
        "runtime-status",
        LifecycleRequest::Status,
        true,
    ));
    let WireEnvelopeBody::Response(ServerResponse {
        request_id,
        payload,
    }) = status.body
    else {
        panic!("idle lifecycle status should be a correlated response");
    };
    assert_eq!(request_id.as_str(), "runtime-status");
    let ResponsePayload::Lifecycle(LifecycleResponse::Status(status)) = payload else {
        panic!("idle lifecycle status should carry its lifecycle payload");
    };
    assert_eq!(
        status,
        LifecycleStatus::new(LifecycleState::Ready, 0)
            .expect("the idle lifecycle status should be representable")
    );
    assert!(!cancel.is_cancelled(), "status must not cancel the runtime");

    let stop = runtime.block_on(lifecycle_request(
        &connection,
        "runtime-stop",
        LifecycleRequest::Stop { require_idle: true },
        false,
    ));
    let WireEnvelopeBody::Response(ServerResponse {
        request_id,
        payload,
    }) = stop.body
    else {
        panic!("accepted lifecycle stop should be a correlated response");
    };
    assert_eq!(request_id.as_str(), "runtime-stop");
    let ResponsePayload::Lifecycle(LifecycleResponse::Stop(receipt)) = payload else {
        panic!("accepted lifecycle stop should carry its lifecycle payload");
    };
    assert_eq!(receipt.disposition, LifecycleStopDisposition::Accepted);
    assert_eq!(receipt.state, LifecycleState::Draining);
    // The response has been decoded before this observation. Cancellation is
    // published only after the server's finished response stream receives the
    // Quinn peer acknowledgement; wait for that real event across runtimes.
    runtime.block_on(async {
        tokio::time::timeout(FUTURE_WAIT, cancel.wait())
            .await
            .expect("runtime cancellation should follow peer acknowledgement");
    });
    assert!(
        cancel.is_cancelled(),
        "runtime cancellation follows the finished stop response"
    );
    drop(connection);

    let result = join_within(
        worker
            .take()
            .expect("lifecycle worker should still be owned"),
        SHUTDOWN_WAIT,
    );
    assert!(
        result.is_ok(),
        "accepted lifecycle stop should be a clean shutdown: {result:?}"
    );
    assert!(
        !directory.path("forge.ready").exists(),
        "lifecycle shutdown should remove its readiness receipt"
    );
    let custody = ForgeProcessCustody::acquire(directory.path("forge.custody"))
        .expect("custody should be reacquirable after lifecycle shutdown");
    drop(custody);

    runtime.block_on(async {
        tokio::time::timeout(FUTURE_WAIT, async {
            let app = ForgeApp::start(ForgeConfig::new(SqliteConfig::file(
                directory.path("forge.sqlite3"),
            )))
            .await
            .expect("database should reopen after lifecycle shutdown");
            app.shutdown()
                .await
                .expect("reopened lifecycle database should close");
        })
        .await
        .expect("reopened lifecycle database future should be bounded");
    });
}

#[test]
fn accepted_service_failure_maps_to_72_and_keeps_listener_error() {
    let directory = TemporaryDirectory::new("service-failure");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let mut worker = Some(spawn_runtime(config(
        &directory,
        &credentials,
        Arc::clone(&cancel),
    )));
    let (_, value) =
        wait_for_readiness_or_stop(&directory.path("forge.ready"), &cancel, &mut worker);
    let address = readiness_endpoint(&value);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("client runtime should build");
    let client = {
        let _entered = runtime.enter();
        client_endpoint(&credentials)
    };
    let connection = runtime.block_on(authenticate_client(&client, address));
    connection.close(VarInt::from_u32(2), b"test service failure");
    let _ =
        runtime.block_on(async { tokio::time::timeout(FUTURE_WAIT, connection.closed()).await });
    let _ = runtime.block_on(async { tokio::time::timeout(FUTURE_WAIT, client.wait_idle()).await });

    let error = join_within(
        worker
            .take()
            .expect("service-failure worker should still be owned"),
        SHUTDOWN_WAIT,
    )
    .expect_err("the accepted listener failure should end the process");
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_SERVICE);
    match &error {
        ForgeRuntimeError::Service(listener_error) => {
            assert!(listener_error.is_service_failure());
            assert!(listener_error.service_cause().is_some());
            assert!(listener_error.as_request_error().is_some());
            assert!(listener_error.drain_error().is_none());
        }
        other => panic!("expected the complete accepted service error, got {other:?}"),
    }
}

#[test]
fn accepted_drain_only_failure_maps_to_73() {
    let directory = TemporaryDirectory::new("drain-failure");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let drain_limits = ListenerLimits {
        drain: Duration::ZERO,
        ..limits()
    };
    let launch = ForgeLaunchConfig::new(ForgeLaunchConfigInput {
        database: directory.path("forge.sqlite3"),
        custody: directory.path("forge.custody"),
        certificate_der: vec![credentials.certificate_path.clone()],
        private_key_der: credentials.private_key_path.clone(),
        bootstrap_capability: credentials.capability_path.clone(),
        ready_file: directory.path("forge.ready"),
        limits: drain_limits,
        admission_capacity: NonZeroU32::new(1).expect("one admission is nonzero"),
        requests_per_connection: NonZeroU32::new(1).expect("one request is nonzero"),
        native_run: native_run(),
        cancel: Arc::clone(&cancel),
    })
    .expect("zero drain is explicit configuration");
    let mut worker = Some(spawn_runtime(launch));
    let (_, value) =
        wait_for_readiness_or_stop(&directory.path("forge.ready"), &cancel, &mut worker);
    let address = readiness_endpoint(&value);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("client runtime should build");
    let client = {
        let _entered = runtime.enter();
        client_endpoint(&credentials)
    };
    let connection = runtime.block_on(connect_without_auth(&client, address));
    cancel.cancel();
    let _ =
        runtime.block_on(async { tokio::time::timeout(FUTURE_WAIT, connection.closed()).await });

    let error = join_within(
        worker
            .take()
            .expect("drain-failure worker should still be owned"),
        SHUTDOWN_WAIT,
    )
    .expect_err("the zero-limit listener drain should fail");
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_SHUTDOWN);
    match &error {
        ForgeRuntimeError::ListenerDrain(listener_error) => {
            assert!(!listener_error.is_service_failure());
            assert!(listener_error.is_drain_failure());
            assert!(listener_error.drain_error().is_some());
            assert!(listener_error.service_cause().is_none());
        }
        other => panic!("expected the accepted drain-only error, got {other:?}"),
    }
}

#[test]
fn service_primary_survives_readiness_cleanup_failure_with_typed_cleanup() {
    let directory = TemporaryDirectory::new("service-primary-cleanup");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let mut worker = Some(spawn_runtime(config(
        &directory,
        &credentials,
        Arc::clone(&cancel),
    )));
    let ready_path = directory.path("forge.ready");
    let (_, value) = wait_for_readiness_or_stop(&ready_path, &cancel, &mut worker);
    let address = readiness_endpoint(&value);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("client runtime should build");
    let client = {
        let _entered = runtime.enter();
        client_endpoint(&credentials)
    };
    let connection = runtime.block_on(authenticate_client(&client, address));

    fs::remove_file(&ready_path).expect("the owned readiness receipt should be removable");
    fs::write(&ready_path, b"replacement readiness target")
        .expect("the replacement readiness target should be written");
    connection.close(VarInt::from_u32(2), b"test primary failure");
    let _ =
        runtime.block_on(async { tokio::time::timeout(FUTURE_WAIT, connection.closed()).await });
    let _ = runtime.block_on(async { tokio::time::timeout(FUTURE_WAIT, client.wait_idle()).await });

    let error = join_within(
        worker
            .take()
            .expect("primary-cleanup worker should still be owned"),
        SHUTDOWN_WAIT,
    )
    .expect_err("service failure should remain observable");
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_SERVICE);
    let composite = error
        .as_primary_with_cleanup()
        .expect("cleanup must be correlated with the service primary");
    assert!(matches!(
        composite.primary(),
        ForgeRuntimeError::Service(listener_error) if listener_error.is_service_failure()
    ));
    assert!(
        composite
            .cleanup_failures()
            .iter()
            .any(|failure| matches!(failure, ForgeRuntimeError::ReadinessCleanup(_)))
    );
    assert_eq!(
        fs::read(&ready_path).expect("replacement readiness target should survive cleanup"),
        b"replacement readiness target"
    );
    assert_no_readiness_temporary(&directory);
    assert_eq!(
        composite.primary().exit_code(),
        forge_runtime::EXIT_CODE_SERVICE
    );
}

#[test]
fn second_forge_cannot_start_until_first_releases_custody() {
    let directory = TemporaryDirectory::new("second-forge");
    let credentials = credentials(&directory);
    let first_cancel = Arc::new(CancelHandle::new());
    let mut first = Some(spawn_runtime(config(
        &directory,
        &credentials,
        Arc::clone(&first_cancel),
    )));
    let _ready =
        wait_for_readiness_or_stop(&directory.path("forge.ready"), &first_cancel, &mut first);

    let second_error = run_config(config(
        &directory,
        &credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(second_error.exit_code(), forge_runtime::EXIT_CODE_CUSTODY);

    first_cancel.cancel();
    assert!(
        join_within(
            first
                .take()
                .expect("first Forge worker should still be owned"),
            SHUTDOWN_WAIT,
        )
        .is_ok()
    );
    let reacquired = ForgeProcessCustody::acquire(directory.path("forge.custody"))
        .expect("custody should be available after first Forge shutdown");
    drop(reacquired);
}

#[test]
fn helper_dispatch_absent_keeps_normal_runtime_unconstructed() {
    assert!(
        artisan_backend::directory_helper::run_if_requested().is_none(),
        "the ordinary test invocation must not select helper mode"
    );
}

fn run_snapshot(
    directory: &TemporaryDirectory,
    seed: ReconciliationSeed,
) -> ReconciliationSnapshot {
    let path = directory.path("forge.sqlite3");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("snapshot runtime should build");
    runtime
        .block_on(async {
            tokio::time::timeout(FUTURE_WAIT, reconciliation_snapshot(&path, seed)).await
        })
        .expect("snapshot future should be bounded")
}

fn startup_candidate_count_blocking(directory: &TemporaryDirectory) -> usize {
    let path = directory.path("forge.sqlite3");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("candidate runtime should build");
    runtime
        .block_on(async { tokio::time::timeout(FUTURE_WAIT, startup_candidate_count(&path)).await })
        .expect("candidate future should be bounded")
}

fn run_config(config: ForgeLaunchConfig) -> ForgeRuntimeError {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    runtime
        .block_on(async {
            tokio::time::timeout(FUTURE_WAIT, Box::pin(forge_runtime::run(config))).await
        })
        .expect("Forge runtime future should be bounded")
        .expect_err("test scenario should produce a typed failure")
}

fn spawn_runtime(config: ForgeLaunchConfig) -> JoinHandle<Result<(), ForgeRuntimeError>> {
    thread::Builder::new()
        .name("forge-runtime-test".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("worker runtime should build");
            runtime
                .block_on(async {
                    tokio::time::timeout(FUTURE_WAIT, Box::pin(forge_runtime::run(config))).await
                })
                .expect("Forge runtime future should be bounded")
        })
        .expect("Forge runtime worker should spawn")
}

fn wait_for_readiness(path: &Path) -> Result<(Vec<u8>, serde_json::Value), &'static str> {
    let deadline = Instant::now() + STARTUP_WAIT;
    loop {
        if let Ok(bytes) = fs::read(path)
            && bytes.ends_with(b"\n")
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            return Ok((bytes, value));
        }
        if Instant::now() >= deadline {
            return Err("readiness should appear within the bounded startup wait");
        }
        thread::yield_now();
    }
}

fn wait_for_readiness_or_stop(
    path: &Path,
    cancel: &CancelHandle,
    worker: &mut Option<JoinHandle<Result<(), ForgeRuntimeError>>>,
) -> (Vec<u8>, serde_json::Value) {
    match wait_for_readiness(path) {
        Ok(readiness) => readiness,
        Err(error) => {
            cancel.cancel();
            let worker_result = join_within(
                worker
                    .take()
                    .expect("readiness worker should still be owned"),
                SHUTDOWN_WAIT,
            );
            panic!("{error}; worker result: {worker_result:?}");
        }
    }
}

fn readiness_endpoint(value: &serde_json::Value) -> SocketAddr {
    value["endpoint"]
        .as_str()
        .expect("readiness endpoint should be text")
        .parse()
        .expect("readiness endpoint should be a socket address")
}

fn assert_no_readiness_temporary(directory: &TemporaryDirectory) {
    let prefix = format!(".artisan-forge-ready-{}-", process::id());
    let leftovers = fs::read_dir(&directory.path)
        .expect("Forge test directory should remain readable")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "Forge readiness temporary files should be removed: {leftovers:?}"
    );
}

fn join_within<T>(handle: JoinHandle<T>, timeout: Duration) -> T {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() {
        assert!(
            Instant::now() < deadline,
            "Forge worker exceeded bounded shutdown"
        );
        thread::yield_now();
    }
    handle.join().expect("Forge worker should not panic")
}
