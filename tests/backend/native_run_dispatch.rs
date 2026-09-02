//! Focused checks for the explicit configured-run scheduler boundary.

use std::{net::SocketAddr, num::NonZeroUsize, path::PathBuf, sync::Arc, time::Duration};

use artisan_database::{RepositoryError, RunLaunchError, SqliteConfig, connect};
use artisan_domain::MessageId;
use artisan_migrations::migrate_to_current;
use artisan_native_engine::NativeOpenCode2Authority;
use artisan_transport::CancelHandle;

use super::{
    NativeRunDispatcherConfig, NativeRunDispatcherConfigError, NativeRunDispatcherConfigInput,
    conversation_commit_notifier::{ConversationCommitNotifier, ConversationCommitSubscription},
};
use crate::native_run_dispatch::{
    FixtureScenarioLaunch, LaunchAuthority, NativeRunDispatcher, NativeRunDispatcherShutdown,
    PromptAuthorization, SettingsLoadDecision, classify_launch_result, classify_settings_load,
    notify_after_commit, prompt_authorization_after_binding,
};
use crate::{
    CommandOrigin,
    lifecycle_control::{ActivityGate, ActivityGateImpl},
};

fn config(
    prompt_delivery: &str,
) -> Result<NativeRunDispatcherConfig, NativeRunDispatcherConfigError> {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::from_millis(10),
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_millis(10),
            queue_capacity: NonZeroUsize::new(1).expect("one queue slot is nonzero"),
            max_command_retries: NonZeroUsize::new(1).expect("one retry is nonzero"),
            prompt_delivery: prompt_delivery.to_owned(),
            stream_after: 0,
        },
    )
}

fn config_with_notifier(
    notifier: ConversationCommitNotifier,
    poll_interval: Duration,
) -> Result<NativeRunDispatcherConfig, NativeRunDispatcherConfigError> {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        notifier,
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::from_millis(10),
            poll_interval,
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_millis(500),
            queue_capacity: NonZeroUsize::new(1).expect("one queue slot is nonzero"),
            max_command_retries: NonZeroUsize::new(1).expect("one retry is nonzero"),
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
    )
}

fn config_for_shutdown_custody() -> Result<NativeRunDispatcherConfig, NativeRunDispatcherConfigError>
{
    // Keep orderly custody within the existing finite 500 ms CI policy.
    config_with_notifier(ConversationCommitNotifier::new(), Duration::from_millis(10))
}

fn config_for_fixture_dispatch(
    notifier: ConversationCommitNotifier,
) -> Result<NativeRunDispatcherConfig, NativeRunDispatcherConfigError> {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        notifier,
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::from_secs(30),
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_secs(5),
            queue_capacity: NonZeroUsize::new(1).expect("one queue slot is nonzero"),
            max_command_retries: NonZeroUsize::new(3).expect("three retries are nonzero"),
            prompt_delivery: "immediate".to_owned(),
            stream_after: 0,
        },
    )
}

#[test]
fn scheduler_requires_positive_injected_durations() {
    let error = NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::ZERO,
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_millis(10),
            queue_capacity: NonZeroUsize::new(1).expect("one queue slot is nonzero"),
            max_command_retries: NonZeroUsize::new(1).expect("one retry is nonzero"),
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
    )
    .expect_err("zero claim lease must be rejected");
    assert_eq!(error, NativeRunDispatcherConfigError::ZeroDuration);
}

#[test]
fn scheduler_rejects_unstructured_prompt_selector() {
    let error = config("queue\n").expect_err("line breaks must not enter the selector");
    assert_eq!(
        error,
        NativeRunDispatcherConfigError::InvalidPromptDeliveryCharacter
    );
}

#[test]
fn scheduler_debug_contains_policy_shape_without_prompt_bytes() {
    let config = config("prompt-selector-sentinel").expect("complete scheduler policy");
    let debug = format!("{config:?}");
    assert!(debug.contains("claim_lease"));
    assert!(debug.contains("prompt_delivery_bytes"));
    assert!(!debug.contains("prompt-selector-sentinel"));
    assert!(!debug.contains("profiles.json"));
}

#[test]
fn unconfigured_settings_requeue_before_provider_launch() {
    let decision = classify_settings_load(Ok(None));
    assert_eq!(
        decision,
        SettingsLoadDecision::Requeue("engine unconfigured")
    );

    let decision = classify_settings_load(Err(RepositoryError::Database {
        operation: "read settings",
        source: sea_orm::DbErr::Custom("temporary".to_owned()),
    }));
    assert_eq!(
        decision,
        SettingsLoadDecision::Requeue("engine settings unavailable")
    );
}

#[test]
fn launch_snapshot_mismatch_is_requeued_without_provider_authority() {
    let mismatch = Err(RunLaunchError::SnapshotMismatch {
        message_id: MessageId::parse("message-snapshot").expect("bounded message id"),
    });
    assert_eq!(classify_launch_result(&mismatch), LaunchAuthority::Requeue);
}

#[test]
fn provider_binding_controls_prompt_authorization() {
    assert_eq!(
        prompt_authorization_after_binding(false),
        PromptAuthorization::Authorize
    );
    assert_eq!(
        prompt_authorization_after_binding(true),
        PromptAuthorization::DoNotAuthorize
    );
}

#[test]
fn observations_notify_only_after_sqlite_commit() {
    let mut notifications = Vec::new();
    assert!(!notify_after_commit(false, || notifications.push("not committed")));
    assert!(notifications.is_empty());
    assert!(notify_after_commit(true, || notifications.push("committed")));
    assert_eq!(notifications, ["committed"]);
}

#[tokio::test(flavor = "current_thread")]
async fn dispatcher_shutdown_joins_owner_custody() {
    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database)
        .await
        .expect("memory database should migrate");
    let repository = artisan_database::Repository::new(database);
    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher = NativeRunDispatcher::start(
        repository,
        PathBuf::from("C:/forge/database.sqlite3"),
        config_for_shutdown_custody().expect("complete dispatcher custody policy"),
        Arc::clone(&process_cancel),
        ActivityGateImpl::new(),
        &tokio::runtime::Handle::current(),
    );

    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
}

// ---------------------------------------------------------------------------
// Helpers for live-recovery dispatch tests
// ---------------------------------------------------------------------------

use std::path::{Path, PathBuf as StdPathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use artisan_database::entities::{
    self, AssistantRunLifecycle, ConversationItemKind, ConversationPatchKind, DispatchState,
    EntityLifecycle, RenderPhase,
};
use artisan_database::{
    AssistantChange, BindRunProvider, ClaimMessageDispatch, CreateThreadInput,
    ProviderBindingBytes, QueueFirstMessageInput, Repository, RunLaunchCredentials, RunStartKey,
    SetThreadEngineConfigInput,
};
use artisan_domain::ThreadId as DomainThreadId;
use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantMessagePhase, ByteLimit, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationPatch, ConversationRequest,
    ConversationSubscribe, CountLimit, EngineAgentId, EngineConfigUpdatePrecondition,
    EngineModelId, EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig,
    EngineRuntimeControls, EngineRuntimeControlsInput, EngineSelection, FilesystemAccess,
    FiniteMillis, ItemId, MessageBody, NetworkAccess, OpenCode2Selection, PatchBatch, PatchId,
    ProjectId, RequestId, RunId, ThreadId, TurnId, UnixMillis, WebSearchAccess,
};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

use crate::{ForgeListener, ListenerLimits, RequestHandler, RequestTermination};
use artisan_database::ConversationPatchReplay;
use artisan_protocol::{
    APPLICATION_PROTOCOL_VERSION, ClientRequest, ConversationSubscriptionStarted, FrameId, Hello,
    HelloCredential, LocalCapability, ProtocolVersion, ResponsePayload, VersionOffer, WireEnvelope,
    WireEnvelopeBody,
};
use artisan_transport::{
    DeadlineError, LOOPBACK_SERVER_NAME, OperationKind, PinnedIdentity, bind_loopback_client,
    client_handshake, receive_envelope, send_envelope,
};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDatabase {
    dir: StdPathBuf,
    file: StdPathBuf,
}

impl TempDatabase {
    fn new(label: &str) -> Self {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "artisan-dispatch-{}-{}-{}",
            label,
            std::process::id(),
            seq
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let file = dir.join("test.db");
        Self { dir, file }
    }

    fn path(&self) -> &Path {
        &self.file
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = StdPathBuf::from(format!("{}{}", self.file.display(), suffix));
            let _ = std::fs::remove_file(sidecar);
        }
        let _ = std::fs::remove_dir(&self.dir);
    }
}

fn registered_fixture_program() -> PathBuf {
    let mapping = std::env::var("ARTISAN_ENGINE_OWNER_FIXTURE")
        .expect("ARTISAN_ENGINE_OWNER_FIXTURE must be set via rlocationpath");
    let runfiles =
        runfiles::Runfiles::create().expect("official runfiles discovery should succeed");
    let path = runfiles::rlocation!(runfiles, mapping.as_str())
        .unwrap_or_else(|| panic!("declared fixture artifact must resolve: {mapping}"));
    assert!(
        path.is_file(),
        "declared fixture artifact must be a regular file: {}",
        path.display()
    );
    path
}

async fn temp_repository(label: &str) -> (DatabaseConnection, Repository, TempDatabase) {
    let temp = TempDatabase::new(label);
    let database = connect(
        SqliteConfig::file(temp.path())
            .min_connections(1)
            .max_connections(4)
            .sqlx_logging(false),
    )
    .await
    .expect("temp db");
    migrate_to_current(&database).await.expect("migrate");
    (database.clone(), Repository::new(database), temp)
}

fn fixture_engine_config() -> EngineRunConfig {
    fixture_engine_config_with_budgets("profile-reconcile", 100, 1, 1)
}

fn fixture_engine_config_with_budgets(
    profile_id: &str,
    attempt_budget: u64,
    phase_budget: u64,
    close_budget: u64,
) -> EngineRunConfig {
    let phase = FiniteMillis::new(phase_budget).expect("phase budget is valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(attempt_budget).expect("attempt budget is valid"),
        readiness_budget: phase,
        health_budget: phase,
        prompt_budget: phase,
        stream_budget: phase,
        close_budget: FiniteMillis::new(close_budget).expect("close budget is valid"),
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
        PermissionId::parse("permission-reconcile").expect("permission id is valid"),
        EngineAgentId::parse("agent-reconcile").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse(profile_id).expect("profile id is valid"),
            EngineModelId::parse("model-reconcile").expect("model id is valid"),
            EngineRouteId::parse("route-reconcile").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

use artisan_domain::PermissionId;

async fn seed_project_and_thread(
    database: &DatabaseConnection,
    repository: &Repository,
    thread_id: &str,
) {
    seed_project_and_thread_with_profile(
        database,
        repository,
        thread_id,
        "profile-reconcile",
        100,
        1,
        1,
    )
    .await;
}

async fn seed_project_and_thread_with_profile(
    database: &DatabaseConnection,
    repository: &Repository,
    thread_id: &str,
    profile_id: &str,
    attempt_budget: u64,
    phase_budget: u64,
    close_budget: u64,
) {
    let project = entities::attached_project::ActiveModel {
        project_id: Set("project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    };
    let _ = entities::attached_project::Entity::insert(project)
        .exec(database)
        .await;
    let _ = repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse(format!("req-{thread_id}")).expect("req"),
            thread_id: ThreadId::parse(thread_id).expect("tid"),
            project_id: ProjectId::parse("project-1").expect("pid"),
            title: artisan_domain::ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(10),
            updated_at: UnixMillis::from_millis(10),
        })
        .await;
    let thread = ThreadId::parse(thread_id).expect("tid");
    if repository
        .read_thread_engine_settings(&thread)
        .await
        .expect("engine configuration should read")
        .is_none()
    {
        repository
            .set_thread_engine_config(SetThreadEngineConfigInput {
                request_id: RequestId::parse(format!("engine-{thread_id}")).expect("request id"),
                thread_id: thread,
                precondition: EngineConfigUpdatePrecondition::Unconfigured,
                config: fixture_engine_config_with_budgets(
                    profile_id,
                    attempt_budget,
                    phase_budget,
                    close_budget,
                ),
                accepted_at: UnixMillis::from_millis(10),
            })
            .await
            .expect("engine configuration should create");
    }
}

const OWNER_BYTES: [u8; 32] = [0xa1; 32];
const LEASE_BYTES: [u8; 32] = [0xb2; 32];
const CLAIM_TOKEN_BYTES: [u8; 32] = [0xc3; 32];
const DISPATCH_OWNER_BYTE: u8 = 0x11;

async fn queue_claim_launch(
    repository: &Repository,
    thread_id: &str,
    message_id: &str,
    run_id: &str,
    turn_id: &str,
) -> (
    artisan_database::ClaimedMessageDispatch,
    artisan_database::LaunchedRunReceipt,
    RunStartKey,
    RunLaunchCredentials,
) {
    let queue = QueueFirstMessageInput {
        request_id: RequestId::parse(format!("req-{message_id}")).expect("req"),
        message_id: MessageId::parse(message_id).expect("mid"),
        thread_id: ThreadId::parse(thread_id).expect("tid"),
        body: MessageBody::parse("hello").expect("body"),
        accepted_at: UnixMillis::from_millis(50),
    };
    repository.queue_first_message(queue).await.expect("queue");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: artisan_database::DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            claimed_at: UnixMillis::from_millis(100),
            lease_expires_at: UnixMillis::from_millis(600),
        })
        .await
        .expect("claim")
        .expect("claimed");
    let run = RunId::parse(run_id).expect("run");
    let turn = TurnId::parse(turn_id).expect("turn");
    let item = ItemId::parse(format!("item-{run_id}")).expect("item");
    let p1 = PatchId::parse(format!("patch-{run_id}-a")).expect("p");
    let p2 = PatchId::parse(format!("patch-{run_id}-b")).expect("p");
    let mut bytes = [0u8; 32];
    for (idx, byte) in run_id.bytes().cycle().take(32).enumerate() {
        bytes[idx] = byte ^ 0x5a;
    }
    let len_u8 = u8::try_from(run_id.len()).unwrap_or(255);
    bytes[0] = bytes[0].wrapping_add(len_u8);
    let start_key = RunStartKey::new(bytes);
    let creds = RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES);
    let engine_settings = repository
        .read_thread_engine_settings(&ThreadId::parse(thread_id).expect("tid"))
        .await
        .expect("engine configuration should read")
        .expect("engine configuration should be present");
    let outcome = repository
        .launch_claimed_run(artisan_database::LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run,
            turn_id: &turn,
            item_id: &item,
            first_patch_id: &p1,
            second_patch_id: &p2,
            operated_at: UnixMillis::from_millis(150),
            run_start_key: &start_key,
            credentials: &creds,
            engine_settings: &engine_settings,
        })
        .await
        .expect("launch");
    let artisan_database::LaunchClaimedRunOutcome::Started(receipt) = outcome else {
        panic!("started");
    };
    (claimed, receipt, start_key, creds)
}

async fn bind_running(
    repository: &Repository,
    claimed: &artisan_database::ClaimedMessageDispatch,
    receipt: &artisan_database::LaunchedRunReceipt,
    start_key: &RunStartKey,
    creds: &RunLaunchCredentials,
) -> artisan_database::BoundRunReceipt {
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let outcome = repository
        .bind_run_provider(BindRunProvider {
            claimed,
            receipt,
            run_start_key: start_key,
            credentials: creds,
            expected_launch_at: UnixMillis::from_millis(150),
            bound_at: UnixMillis::from_millis(200),
            binding_version: 2,
            binding_bytes: &binding,
        })
        .await
        .expect("bind");
    match outcome {
        artisan_database::BindRunProviderOutcome::Bound(r)
        | artisan_database::BindRunProviderOutcome::AlreadyBound(r) => r,
    }
}

struct RunningItemSeed<'a> {
    claimed: &'a artisan_database::ClaimedMessageDispatch,
    receipt: &'a artisan_database::LaunchedRunReceipt,
    bound: &'a artisan_database::BoundRunReceipt,
    start_key: &'a RunStartKey,
    credentials: &'a RunLaunchCredentials,
    item_id: &'a ItemId,
    turn_patch_id: &'a PatchId,
    item_patch_id: &'a PatchId,
}

async fn commit_running_item(repository: &Repository, seed: RunningItemSeed<'_>) {
    let body = AssistantBody::parse("hello assistant").expect("body");
    repository
        .commit_run_batch(artisan_database::CommitRunBatch {
            scope: artisan_database::RunBatchScope {
                claimed: seed.claimed,
                launched: seed.receipt,
                bound: seed.bound,
                run_start_key: seed.start_key,
                credentials: seed.credentials,
                expected_launch_at: UnixMillis::from_millis(150),
                expected_updated_at: UnixMillis::from_millis(200),
            },
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(250),
            activate_turn_patch_id: Some(seed.turn_patch_id),
            changes: &[AssistantChange::Start {
                item_id: seed.item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: seed.item_patch_id,
            }],
            checkpoint: artisan_database::CheckpointUpdate::Keep,
        })
        .await
        .expect("commit batch");
}

async fn fetch_all(database: &DatabaseConnection) -> AllRows {
    async fn all<E>(db: &DatabaseConnection) -> Vec<E::Model>
    where
        E: EntityTrait,
    {
        E::find().all(db).await.expect("rows")
    }
    let mut dispatches = all::<entities::message_dispatch::Entity>(database).await;
    let mut runs = all::<entities::assistant_run::Entity>(database).await;
    let mut turns = all::<entities::conversation_turn::Entity>(database).await;
    let mut items = all::<entities::conversation_item::Entity>(database).await;
    let mut patches = all::<entities::conversation_patch::Entity>(database).await;
    let mut states = all::<entities::conversation_state::Entity>(database).await;
    dispatches.sort_by(|a, b| a.message_id.cmp(&b.message_id));
    runs.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    turns.sort_by(|a, b| a.turn_id.cmp(&b.turn_id));
    items.sort_by(|a, b| a.item_id.cmp(&b.item_id));
    patches.sort_by_key(|a| a.sequence);
    states.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
    AllRows {
        dispatches,
        runs,
        turns,
        items,
        patches,
        states,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AllRows {
    dispatches: Vec<entities::MessageDispatch>,
    runs: Vec<entities::AssistantRun>,
    turns: Vec<entities::ConversationTurn>,
    items: Vec<entities::ConversationItem>,
    patches: Vec<entities::ConversationPatch>,
    states: Vec<entities::ConversationState>,
}

async fn setup_binding_scenario(
    database: &DatabaseConnection,
    repository: &Repository,
) -> Option<entities::OpaqueBytes> {
    seed_project_and_thread(database, repository, "thread-launch").await;
    let _ = queue_claim_launch(
        repository,
        "thread-launch",
        "msg-launch",
        "run-launch",
        "turn-launch",
    )
    .await;
    seed_project_and_thread(database, repository, "thread-run").await;
    let (claimed2, receipt2, sk2, cr2) =
        queue_claim_launch(repository, "thread-run", "msg-run", "run-run", "turn-run").await;
    let bound2 = bind_running(repository, &claimed2, &receipt2, &sk2, &cr2).await;
    let before_binding = entities::assistant_run::Entity::find_by_id("run-run")
        .one(database)
        .await
        .expect("find")
        .expect("run")
        .provider_binding
        .clone();
    let item_id = ItemId::parse("assistant-run").expect("item");
    let turn_patch_id = PatchId::parse("p-turn-run").expect("p");
    let item_patch_id = PatchId::parse("p-item-run").expect("p");
    commit_running_item(
        repository,
        RunningItemSeed {
            claimed: &claimed2,
            receipt: &receipt2,
            bound: &bound2,
            start_key: &sk2,
            credentials: &cr2,
            item_id: &item_id,
            turn_patch_id: &turn_patch_id,
            item_patch_id: &item_patch_id,
        },
    )
    .await;
    before_binding
}

fn assert_binding_lifecycle(after: &AllRows, before_binding: Option<&entities::OpaqueBytes>) {
    for msg in ["msg-launch", "msg-run"] {
        let dispatch = after
            .dispatches
            .iter()
            .find(|x| x.message_id == msg)
            .expect("dispatch");
        assert_eq!(dispatch.state, DispatchState::Failed);
        assert_eq!(
            dispatch.last_error.as_deref(),
            Some("startup reconciliation: unknown outcome after lease expiry")
        );
    }
    for run_id in ["run-launch", "run-run"] {
        let run = after.runs.iter().find(|x| x.run_id == run_id).expect("run");
        assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
        assert_eq!(
            run.error_code.as_deref(),
            Some("startup_reconciliation_unknown_outcome")
        );
    }
    let run_launch = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-launch")
        .expect("launch");
    assert!(run_launch.provider_binding.is_none());
    let run_run = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-run")
        .expect("run");
    assert_eq!(run_run.provider_binding.as_ref(), before_binding);
    assert!(run_run.provider_binding_version.is_some());
    let turn_launch = after
        .turns
        .iter()
        .find(|t| t.turn_id == "turn-launch")
        .expect("turn");
    assert_eq!(turn_launch.lifecycle, EntityLifecycle::Interrupted);
    let turn_run = after
        .turns
        .iter()
        .find(|t| t.turn_id == "turn-run")
        .expect("turn");
    assert_eq!(turn_run.lifecycle, EntityLifecycle::Interrupted);
    let item = after
        .items
        .iter()
        .find(|i| i.item_id == "assistant-run")
        .expect("item");
    assert_eq!(item.lifecycle, EntityLifecycle::Interrupted);
}

// ---------------------------------------------------------------------------
// Dispatch live-recovery tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn dispatch_recovers_65_expired_across_bounded_pages() {
    let (database, repository, _temp) = temp_repository("dispatch-65").await;
    for i in 0..65 {
        let thread = format!("thread-65-{i}");
        let msg = format!("msg-65-{i}");
        let run = format!("run-65-{i:03}");
        let turn = format!("turn-65-{i:03}");
        seed_project_and_thread(&database, &repository, &thread).await;
        let (_cl, _rc, _sk, _cr) =
            queue_claim_launch(&repository, &thread, &msg, &run, &turn).await;
    }
    let before = fetch_all(&database).await;
    let before_patches = before.patches.len();

    let notifier = ConversationCommitNotifier::new();
    let config = config_with_notifier(notifier, Duration::from_millis(20)).expect("config");
    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher = NativeRunDispatcher::start(
        repository.clone(),
        StdPathBuf::from("C:/forge/database.sqlite3"),
        config,
        Arc::clone(&process_cancel),
        ActivityGateImpl::new(),
        &tokio::runtime::Handle::current(),
    );

    tokio::time::sleep(Duration::from_millis(600)).await;
    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );

    let after = fetch_all(&database).await;
    let interrupted = after
        .runs
        .iter()
        .filter(|r| r.lifecycle == AssistantRunLifecycle::Interrupted)
        .count();
    assert_eq!(interrupted, 65, "all 65 expired runs should be interrupted");
    assert_eq!(after.patches.len(), before_patches + 65);
    for run in &after.runs {
        if run.run_id.starts_with("run-65-") {
            assert_eq!(run.lease, None);
            assert_eq!(run.owner, None);
        }
    }
    for dispatch in &after.dispatches {
        if dispatch.message_id.starts_with("msg-65-") {
            assert_eq!(dispatch.state, DispatchState::Failed);
            assert_eq!(dispatch.lease_owner, None);
            assert_eq!(dispatch.lease_expires_at_ms, None);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_unexpired_candidate_remains_byte_stable_until_expiry() {
    let (database, repository, _temp) = temp_repository("dispatch-unexpired").await;
    seed_project_and_thread(&database, &repository, "thread-unexp").await;
    let (_cl, _rc, _sk, _cr) = queue_claim_launch(
        &repository,
        "thread-unexp",
        "msg-unexp",
        "run-unexp",
        "turn-unexp",
    )
    .await;
    let wall = crate::SystemCommandOrigin
        .acceptance_instant()
        .expect("clock should succeed");
    let future_ms = wall.as_millis() + 10_000;
    let dispatch_row = entities::message_dispatch::Entity::find_by_id("msg-unexp")
        .one(&database)
        .await
        .expect("find")
        .expect("dispatch");
    let mut active: entities::message_dispatch::ActiveModel = dispatch_row.into();
    active.lease_expires_at_ms = Set(Some(future_ms));
    active.update(&database).await.expect("update future lease");
    let run_row = entities::assistant_run::Entity::find_by_id("run-unexp")
        .one(&database)
        .await
        .expect("find")
        .expect("run");
    let mut run_active: entities::assistant_run::ActiveModel = run_row.into();
    run_active.updated_at_ms = Set(wall.as_millis());
    run_active.update(&database).await.expect("update run");

    let before = fetch_all(&database).await;

    let notifier = ConversationCommitNotifier::new();
    let config = config_with_notifier(notifier, Duration::from_millis(15)).expect("config");
    let process_cancel = Arc::new(CancelHandle::new());
    let activity = ActivityGateImpl::new();
    let mut dispatcher = NativeRunDispatcher::start(
        repository.clone(),
        StdPathBuf::from("C:/forge/database.sqlite3"),
        config,
        Arc::clone(&process_cancel),
        activity.clone(),
        &tokio::runtime::Handle::current(),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    let mid = fetch_all(&database).await;
    assert_eq!(before.dispatches, mid.dispatches);
    assert_eq!(before.runs, mid.runs);
    assert_eq!(before.patches, mid.patches);
    assert_eq!(
        activity
            .snapshot()
            .expect("no-claim activity should remain readable")
            .active_work_count(),
        0
    );

    let dispatch_row2 = entities::message_dispatch::Entity::find_by_id("msg-unexp")
        .one(&database)
        .await
        .expect("find")
        .expect("dispatch");
    let mut active2: entities::message_dispatch::ActiveModel = dispatch_row2.into();
    active2.lease_expires_at_ms = Set(Some(0));
    active2.update(&database).await.expect("update expired");
    let run_row2 = entities::assistant_run::Entity::find_by_id("run-unexp")
        .one(&database)
        .await
        .expect("find")
        .expect("run");
    let mut run_active2: entities::assistant_run::ActiveModel = run_row2.into();
    run_active2.updated_at_ms = Set(150);
    run_active2
        .update(&database)
        .await
        .expect("update run expired");

    tokio::time::sleep(Duration::from_millis(400)).await;
    let after = fetch_all(&database).await;
    let run = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-unexp")
        .expect("run");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
    let dispatch = after
        .dispatches
        .iter()
        .find(|d| d.message_id == "msg-unexp")
        .expect("dispatch");
    assert_eq!(dispatch.state, DispatchState::Failed);

    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_failed_dispatch_interrupted_run_binding_retained() {
    let (database, repository, _temp) = temp_repository("dispatch-binding").await;
    let before_binding = setup_binding_scenario(&database, &repository).await;
    assert!(before_binding.is_some());
    let notifier = ConversationCommitNotifier::new();
    let config = config_with_notifier(notifier, Duration::from_millis(15)).expect("config");
    let process_cancel = Arc::new(CancelHandle::new());
    let activity = ActivityGateImpl::new();
    let mut dispatcher = NativeRunDispatcher::start(
        repository.clone(),
        StdPathBuf::from("C:/forge/database.sqlite3"),
        config,
        Arc::clone(&process_cancel),
        activity.clone(),
        &tokio::runtime::Handle::current(),
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    let after = fetch_all(&database).await;
    assert_binding_lifecycle(&after, before_binding.as_ref());
    assert_eq!(
        activity
            .snapshot()
            .expect("failed dispatch activity should remain readable")
            .active_work_count(),
        0
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_post_disposition_notifier_wake() {
    let (database, repository, _temp) = temp_repository("dispatch-notify").await;
    seed_project_and_thread(&database, &repository, "thread-notify").await;
    let (_cl, _rc, _sk, _cr) = queue_claim_launch(
        &repository,
        "thread-notify",
        "msg-notify",
        "run-notify",
        "turn-notify",
    )
    .await;

    let notifier = ConversationCommitNotifier::new();
    let mut subscription = notifier
        .subscribe(DomainThreadId::parse("thread-notify").expect("tid"))
        .expect("subscribe");
    let config = config_with_notifier(notifier, Duration::from_millis(15)).expect("config");
    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher = NativeRunDispatcher::start(
        repository.clone(),
        StdPathBuf::from("C:/forge/database.sqlite3"),
        config,
        Arc::clone(&process_cancel),
        ActivityGateImpl::new(),
        &tokio::runtime::Handle::current(),
    );

    let wake_result = tokio::time::timeout(Duration::from_millis(800), subscription.wait()).await;
    assert!(
        wake_result.is_ok(),
        "notifier should wake after durable disposition"
    );

    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    // Ensure durable state
    let after = fetch_all(&database).await;
    let run = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-notify")
        .expect("run");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_final_bounded_cancellation_page_plus_owner_join() {
    let (database, repository, _temp) = temp_repository("dispatch-final").await;
    let notifier = ConversationCommitNotifier::new();
    let config = config_with_notifier(notifier, Duration::from_millis(500)).expect("config");
    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher = NativeRunDispatcher::start(
        repository.clone(),
        StdPathBuf::from("C:/forge/database.sqlite3"),
        config,
        Arc::clone(&process_cancel),
        ActivityGateImpl::new(),
        &tokio::runtime::Handle::current(),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    seed_project_and_thread(&database, &repository, "thread-final").await;
    let (_cl, _rc, _sk, _cr) = queue_claim_launch(
        &repository,
        "thread-final",
        "msg-final",
        "run-final",
        "turn-final",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    process_cancel.cancel();
    let shutdown = dispatcher.shutdown().await;
    assert_eq!(shutdown, NativeRunDispatcherShutdown::Joined);

    let after = fetch_all(&database).await;
    let run = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-final")
        .expect("run");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
    let dispatch = after
        .dispatches
        .iter()
        .find(|d| d.message_id == "msg-final")
        .expect("dispatch");
    assert_eq!(dispatch.state, DispatchState::Failed);
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_no_provider_child_for_recovery_only() {
    let (database, repository, _temp) = temp_repository("dispatch-noprovider").await;
    for i in 0..3 {
        let thread = format!("thread-np-{i}");
        let msg = format!("msg-np-{i}");
        let run = format!("run-np-{i}");
        let turn = format!("turn-np-{i}");
        seed_project_and_thread(&database, &repository, &thread).await;
        let (_cl, _rc, _sk, _cr) =
            queue_claim_launch(&repository, &thread, &msg, &run, &turn).await;
    }
    let before = fetch_all(&database).await;
    let before_run_count = before.runs.len();
    let before_patches = before.patches.len();

    crate::engine_owner::reset_witnesses();
    let notifier = ConversationCommitNotifier::new();
    let config = config_with_notifier(notifier, Duration::from_millis(15)).expect("config");
    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher = NativeRunDispatcher::start(
        repository.clone(),
        StdPathBuf::from("C:/forge/database.sqlite3"),
        config,
        Arc::clone(&process_cancel),
        ActivityGateImpl::new(),
        &tokio::runtime::Handle::current(),
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );

    let after = fetch_all(&database).await;
    assert_eq!(after.runs.len(), before_run_count);
    assert_eq!(after.patches.len(), before_patches + 3);
    for run in &after.runs {
        if run.run_id.starts_with("run-np-") {
            assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
        }
    }
    let counts = crate::engine_owner::witness_counts();
    assert_eq!(counts.spawned, 0);
}

fn assert_queued_fixture_message(after: &AllRows, message_id: &MessageId) {
    assert_eq!(after.dispatches.len(), 1);
    let dispatch = &after.dispatches[0];
    assert_eq!(dispatch.message_id, message_id.as_str());
    assert_eq!(dispatch.state, DispatchState::Queued);
    assert_eq!(dispatch.attempt_count, 0);
}

fn assert_fixture_dispatch(after: &AllRows, message_id: &MessageId) {
    let dispatch = after
        .dispatches
        .iter()
        .find(|dispatch| dispatch.message_id == message_id.as_str())
        .expect("fixture dispatch");
    assert_eq!(dispatch.state, DispatchState::Completed);
    assert_eq!(dispatch.attempt_count, 1);
    assert!(dispatch.lease_owner.is_none());
    assert!(dispatch.lease_expires_at_ms.is_none());
    assert!(dispatch.last_error.is_none());
}

fn assert_fixture_run(after: &AllRows) {
    assert_eq!(after.runs.len(), 1);
    let run = after
        .runs
        .iter()
        .find(|run| run.run_id == "fixture-run")
        .expect("fixture run");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Completed);
    assert_eq!(run.generation, 1);
    assert!(run.terminal_at_ms.is_some());
    assert!(run.owner.is_none());
    assert!(run.lease.is_none());
    assert!(run.claim_token.is_none());
    assert!(run.error_code.is_none());
    assert!(run.error_message.is_none());
    assert_eq!(run.provider_binding_version, Some(1));
    let binding: serde_json::Value = serde_json::from_slice(
        run.provider_binding
            .as_ref()
            .expect("provider binding")
            .as_slice(),
    )
    .expect("provider binding JSON");
    assert_eq!(
        binding,
        serde_json::json!({
            "engine": "opencode2",
            "profile_id": "fixture-test",
            "session_id": "test-session",
        })
    );
}

fn assert_fixture_transcript(after: &AllRows, thread_id: &ThreadId) {
    assert_eq!(after.turns.len(), 1);
    assert_eq!(after.items.len(), 2);
    let turn = after
        .turns
        .iter()
        .find(|turn| turn.turn_id == "fixture-turn")
        .expect("origin turn");
    assert_eq!(turn.lifecycle, EntityLifecycle::Completed);
    assert_eq!(turn.revision, 2);
    let assistant_items: Vec<_> = after
        .items
        .iter()
        .filter(|item| item.item_kind == ConversationItemKind::AssistantMessage)
        .collect();
    assert_eq!(assistant_items.len(), 1);
    let user_items: Vec<_> = after
        .items
        .iter()
        .filter(|item| item.item_kind == ConversationItemKind::UserMessage)
        .collect();
    assert_eq!(user_items.len(), 1);
    assert_eq!(user_items[0].body, "hello world");
    let assistant = assistant_items[0];
    assert_eq!(assistant.lifecycle, EntityLifecycle::Completed);
    assert_eq!(assistant.phase, Some(RenderPhase::Final));
    assert_eq!(assistant.revision, 1);
    assert_eq!(assistant.body, "hello world");
    assert_eq!(assistant.run_id.as_deref(), Some("fixture-run"));

    assert_eq!(after.patches.len(), 6);
    assert_eq!(
        after
            .patches
            .iter()
            .map(|patch| patch.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6]
    );
    assert_eq!(
        after
            .patches
            .iter()
            .map(|patch| patch.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            ConversationPatchKind::TurnUpsert,
            ConversationPatchKind::ItemUpsert,
            ConversationPatchKind::TurnLifecycle,
            ConversationPatchKind::ItemUpsert,
            ConversationPatchKind::ItemLifecycle,
            ConversationPatchKind::TurnLifecycle,
        ]
    );
    assert_eq!(after.patches[1].body.as_deref(), Some("hello world"));
    assert_eq!(
        after.patches[1].item_kind,
        Some(ConversationItemKind::UserMessage)
    );
    assert_eq!(after.patches[3].body.as_deref(), Some("hello world"));
    assert_eq!(
        after.patches[3].item_kind,
        Some(ConversationItemKind::AssistantMessage)
    );

    let state = after
        .states
        .iter()
        .find(|state| state.thread_id == thread_id.as_str())
        .expect("conversation state");
    assert_eq!(state.last_patch_sequence, 6);
    assert_eq!(state.next_renderer_ordinal, 3);
}

async fn assert_fixture_batch_durability(database: &DatabaseConnection) {
    let checkpoints = entities::run_checkpoint::Entity::find()
        .all(database)
        .await
        .expect("checkpoint rows");
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].run_id, "fixture-run");
    assert_eq!(checkpoints[0].generation, 1);
    assert_eq!(checkpoints[0].last_batch_sequence, 1);

    let receipts = entities::run_batch_receipt::Entity::find()
        .all(database)
        .await
        .expect("batch receipt rows");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].run_id, "fixture-run");
    assert_eq!(receipts[0].batch_sequence, 1);
    assert_eq!(receipts[0].generation, 1);
    assert!(receipts[0].committed);
    assert_eq!(receipts[0].digest.as_slice().len(), 32);
}

fn assert_fixture_custody() {
    let counts = crate::engine_owner::witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    assert_eq!(counts.kills_requested, 0);
}

async fn sample_fixture_activity(
    activity: ActivityGateImpl,
    observed_active: Arc<AtomicBool>,
    sample_cancel: Arc<CancelHandle>,
) {
    loop {
        if activity
            .snapshot()
            .expect("fixture activity should remain readable")
            .active_work_count()
            != 0
        {
            observed_active.store(true, Ordering::Relaxed);
        }
        tokio::select! {
            biased;
            () = sample_cancel.wait() => break,
            () = tokio::task::yield_now() => {},
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_fixture_composes_claim_through_durable_settlement() {
    let fixture = registered_fixture_program();
    let (database, repository, temp) = temp_repository("dispatch-composition").await;
    let thread_id = ThreadId::parse("fixture-thread").expect("thread id");
    let message_id = MessageId::parse("fixture-message").expect("message id");
    seed_project_and_thread_with_profile(
        &database,
        &repository,
        thread_id.as_str(),
        "fixture-test",
        30_000,
        5_000,
        2_000,
    )
    .await;
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("fixture-request").expect("request id"),
            message_id: message_id.clone(),
            thread_id: thread_id.clone(),
            body: MessageBody::parse("hello world").expect("message body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("one fixture message should queue");
    assert_queued_fixture_message(&fetch_all(&database).await, &message_id);

    let notifier = ConversationCommitNotifier::new();
    let mut subscription = notifier
        .subscribe(thread_id.clone())
        .expect("fixture thread subscription");
    let config = config_for_fixture_dispatch(notifier).expect("fixture dispatch policy");
    crate::engine_owner::reset_witnesses();
    let process_cancel = Arc::new(CancelHandle::new());
    let activity = ActivityGateImpl::new();
    let observed_active = Arc::new(AtomicBool::new(false));
    let sample_cancel = Arc::new(CancelHandle::new());
    let sampler = tokio::spawn(sample_fixture_activity(
        activity.clone(),
        Arc::clone(&observed_active),
        Arc::clone(&sample_cancel),
    ));
    let mut dispatcher = NativeRunDispatcher::start_with_fixture_for_tests(
        repository.clone(),
        temp.path().to_owned(),
        config,
        Arc::clone(&process_cancel),
        activity.clone(),
        &tokio::runtime::Handle::current(),
        fixture,
    );

    let mut settled = false;
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(10), subscription.wait())
            .await
            .expect("fixture commit wake should arrive")
            .expect("fixture notifier should remain open");
        let run = entities::assistant_run::Entity::find_by_id("fixture-run")
            .one(&database)
            .await
            .expect("fixture run query")
            .expect("fixture run should exist");
        if run.lifecycle == AssistantRunLifecycle::Completed {
            settled = true;
            break;
        }
    }
    assert!(
        settled,
        "fixture run must settle after bounded commit wakes"
    );

    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    sample_cancel.cancel();
    sampler.await.expect("activity sampler should finish");
    assert!(
        observed_active.load(Ordering::Relaxed),
        "a claimed fixture dispatch should hold activity custody"
    );
    assert_eq!(
        activity
            .snapshot()
            .expect("settled fixture activity should remain readable")
            .active_work_count(),
        0
    );
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );

    let after = fetch_all(&database).await;
    assert_fixture_dispatch(&after, &message_id);
    assert_fixture_run(&after);
    assert_fixture_transcript(&after, &thread_id);
    assert_fixture_batch_durability(&database).await;
    assert_fixture_custody();
}

// ---------------------------------------------------------------------------
// Forge delivery and restart proof
// ---------------------------------------------------------------------------

const FORGE_RESTART_CAPABILITY: [u8; 32] = [0x6e; 32];

const FORGE_REPLAY_PROOF_DEADLINE: Duration = Duration::from_secs(5);

struct FixtureForgePki {
    certificate: CertificateDer<'static>,
    private_key: PrivatePkcs8KeyDer<'static>,
    pinned_identity: PinnedIdentity,
}

fn fixture_forge_pki() -> FixtureForgePki {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("valid SAN");
    let certificate = certified_key.cert.der().clone();
    FixtureForgePki {
        pinned_identity: PinnedIdentity::from_certificate(&certificate),
        private_key: PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der()),
        certificate,
    }
}

fn fixture_forge_server_config(pki: &FixtureForgePki) -> ServerConfig {
    artisan_transport::server_config(vec![pki.certificate.clone()], pki.private_key.clone_key())
        .expect("Forge server configuration")
}

fn fixture_forge_client_config(pki: &FixtureForgePki) -> ClientConfig {
    artisan_transport::client_config(pki.certificate.clone(), pki.pinned_identity)
        .expect("Forge client configuration")
}

fn fixture_forge_listener_limits() -> ListenerLimits {
    ListenerLimits {
        admission: Duration::from_secs(2),
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
        drain: Duration::from_secs(2),
    }
}

fn bind_fixture_forge_listener(
    repository: Repository,
    notifier: ConversationCommitNotifier,
    pki: &FixtureForgePki,
) -> (ForgeListener, RequestHandler, Arc<CancelHandle>, SocketAddr) {
    let handler =
        RequestHandler::with_subscriptions(repository).with_conversation_commit_notifier(notifier);
    let listener = ForgeListener::bind(
        fixture_forge_server_config(pki),
        LocalCapability::from_bytes(FORGE_RESTART_CAPABILITY),
        Box::new(crate::SystemCommandOrigin),
        fixture_forge_listener_limits(),
        std::num::NonZeroU32::new(1).expect("one listener admission slot"),
        std::num::NonZeroU32::new(4).expect("bounded request capacity"),
    )
    .expect("Forge listener should bind");
    let address = listener.local_addr().expect("Forge listener address");
    (listener, handler, Arc::new(CancelHandle::new()), address)
}

async fn serve_fixture_forge_listener(
    listener: ForgeListener,
    handler: RequestHandler,
    cancel: Arc<CancelHandle>,
) {
    let (listener, report) = listener
        .serve_one(&handler, cancel.as_ref())
        .await
        .expect("Forge listener should return reusable custody");
    assert_eq!(report.completed_requests, 1);
    assert!(matches!(
        report.termination,
        RequestTermination::Failed {
            source: DeadlineError::Cancelled {
                operation: OperationKind::Receive
            }
        }
    ));
    listener.drain().await.expect("Forge listener should drain");
}

struct FixtureForgeClient {
    endpoint: Endpoint,
    connection: Connection,
}

fn fixture_hello_envelope() -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("fixture-forge-hello").expect("hello frame id"),
        sent_at: UnixMillis::from_millis(1),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![APPLICATION_PROTOCOL_VERSION])
                .expect("version offer"),
            credential: HelloCredential::Initial(LocalCapability::from_bytes(
                FORGE_RESTART_CAPABILITY,
            )),
            supports_lifecycle_control: false,
        }),
    }
}

fn fixture_resume_envelope(thread_id: ThreadId, cursor: ConversationCursor) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("fixture-forge-subscribe").expect("subscription frame id"),
        sent_at: UnixMillis::from_millis(2),
        body: WireEnvelopeBody::Request(ClientRequest::Conversation(
            ConversationRequest::Subscribe(ConversationSubscribe::resume(thread_id, cursor)),
        )),
    }
}

async fn connect_fixture_forge_client(
    pki: &FixtureForgePki,
    address: SocketAddr,
) -> FixtureForgeClient {
    let endpoint = bind_loopback_client(fixture_forge_client_config(pki))
        .expect("Forge client endpoint should bind");
    let connecting = endpoint
        .connect(address, LOOPBACK_SERVER_NAME)
        .expect("Forge client should connect");
    let connection = tokio::time::timeout(FORGE_REPLAY_PROOF_DEADLINE, connecting)
        .await
        .unwrap_or_else(|_| panic!("Forge client connection exceeded the test deadline"))
        .unwrap_or_else(|_| panic!("Forge client connection failed"));
    let (mut control_send, mut control_recv) = connection
        .open_bi()
        .await
        .expect("Forge control stream should open");
    let _welcome = tokio::time::timeout(
        FORGE_REPLAY_PROOF_DEADLINE,
        client_handshake(
            &mut control_send,
            &mut control_recv,
            fixture_hello_envelope(),
        ),
    )
    .await
    .unwrap_or_else(|_| panic!("Forge client handshake exceeded the test deadline"))
    .unwrap_or_else(|_| panic!("Forge client handshake failed"));
    drop(control_send);
    drop(control_recv);
    FixtureForgeClient {
        endpoint,
        connection,
    }
}

async fn subscribe_fixture_forge_client(
    connection: &Connection,
    thread_id: ThreadId,
    cursor: ConversationCursor,
) -> ConversationSubscriptionStarted {
    let (mut request_send, mut request_recv) = connection
        .open_bi()
        .await
        .expect("Forge subscription stream should open");
    send_envelope(
        &mut request_send,
        &fixture_resume_envelope(thread_id, cursor),
    )
    .await
    .expect("Forge subscription request should cross the wire");
    drop(request_send);
    let response = tokio::time::timeout(
        FORGE_REPLAY_PROOF_DEADLINE,
        receive_envelope(&mut request_recv),
    )
    .await
    .unwrap_or_else(|_| panic!("Forge subscription response exceeded the test deadline"))
    .unwrap_or_else(|_| panic!("Forge subscription response could not be received"));
    drop(request_recv);
    let WireEnvelopeBody::Response(response) = response.body else {
        panic!("Forge subscription should receive a response");
    };
    assert!(
        response.request_id.as_str() == "fixture-forge-subscribe",
        "Forge subscription response correlation must be preserved"
    );
    let ResponsePayload::ConversationSubscriptionStarted(started) = response.payload else {
        panic!("Forge subscription should acknowledge its start");
    };
    started
}

fn assert_fixture_resumed_subscription(
    started: ConversationSubscriptionStarted,
    expected_thread_id: &ThreadId,
    expected_cursor: ConversationCursor,
) {
    let ConversationSubscriptionStarted::Resumed { thread_id, cursor } = started else {
        panic!("Forge subscription should resume from its requested cursor");
    };
    assert!(
        thread_id == *expected_thread_id,
        "Forge subscription resumed the requested thread"
    );
    assert_eq!(cursor, expected_cursor);
}

async fn accept_fixture_delivery(connection: &Connection) -> quinn::RecvStream {
    tokio::time::timeout(FORGE_REPLAY_PROOF_DEADLINE, connection.accept_uni())
        .await
        .unwrap_or_else(|_| panic!("Forge delivery stream did not open before the deadline"))
        .unwrap_or_else(|_| panic!("Forge delivery stream could not be accepted"))
}

async fn receive_fixture_patch_batch(stream: &mut quinn::RecvStream) -> PatchBatch {
    let envelope = tokio::time::timeout(FORGE_REPLAY_PROOF_DEADLINE, receive_envelope(stream))
        .await
        .unwrap_or_else(|_| panic!("Forge delivery batch exceeded the test deadline"))
        .unwrap_or_else(|_| panic!("Forge delivery batch could not be received"));
    let WireEnvelopeBody::PatchBatch(batch) = envelope.body else {
        panic!("Forge delivery stream should carry a patch batch");
    };
    batch
}

async fn shutdown_fixture_forge_client(client: FixtureForgeClient, reason: &'static [u8]) {
    let FixtureForgeClient {
        endpoint,
        connection,
    } = client;
    drop(connection);
    artisan_transport::shutdown(
        &endpoint,
        quinn::VarInt::from_u32(0),
        reason,
        FORGE_REPLAY_PROOF_DEADLINE,
    )
    .await
    .expect("Forge client endpoint should shut down");
    drop(endpoint);
}

async fn fixture_durable_replay(
    repository: &Repository,
    thread_id: &ThreadId,
    cursor: ConversationCursor,
) -> PatchBatch {
    let replay = repository
        .read_conversation_patch_replay(thread_id, cursor)
        .await
        .unwrap_or_else(|_| panic!("durable conversation replay should be readable"));
    match replay {
        ConversationPatchReplay::Batch(batch) => batch,
        ConversationPatchReplay::Current { .. }
        | ConversationPatchReplay::ResnapshotRequired { .. } => {
            panic!("durable conversation replay should contain the requested patches");
        }
    }
}

async fn fixture_durable_replay_prefix(
    repository: &Repository,
    thread_id: &ThreadId,
    from_cursor: ConversationCursor,
    to_cursor: ConversationCursor,
) -> PatchBatch {
    // The terminal commit may already be durable by the time this read runs;
    // the delivered cursor range is therefore selected from the authoritative
    // replay rather than reconstructed from wire payloads.
    let complete = fixture_durable_replay(repository, thread_id, from_cursor).await;
    let count = usize::try_from(
        to_cursor
            .get()
            .checked_sub(from_cursor.get())
            .expect("replay cursor should advance forwards"),
    )
    .expect("replay prefix should fit in usize");
    assert!(count > 0, "replay prefix should contain patches");
    assert!(
        count <= complete.patches().len(),
        "durable replay should reach the delivered cursor"
    );
    PatchBatch::new(
        thread_id.clone(),
        from_cursor,
        to_cursor,
        complete.patches()[..count].to_vec(),
    )
    .unwrap_or_else(|_| panic!("durable replay prefix should remain contiguous"))
}

fn fixture_replay_has_terminal_completion(patches: &[ConversationPatch]) -> bool {
    patches.iter().any(|patch| {
        matches!(
            patch,
            ConversationPatch::ItemLifecycle {
                lifecycle: ConversationLifecycle::Completed,
                ..
            }
        )
    }) && patches.iter().any(|patch| {
        matches!(
            patch,
            ConversationPatch::TurnLifecycle {
                lifecycle: ConversationLifecycle::Completed,
                ..
            }
        )
    })
}

async fn wait_for_fixture_final_replay(
    repository: &Repository,
    thread_id: &ThreadId,
    commit_subscription: &mut ConversationCommitSubscription,
) -> PatchBatch {
    tokio::time::timeout(FORGE_REPLAY_PROOF_DEADLINE, async {
        loop {
            let replay = repository
                .read_conversation_patch_replay(thread_id, ConversationCursor::default())
                .await
                .unwrap_or_else(|_| panic!("durable conversation replay should be readable"));
            match replay {
                ConversationPatchReplay::Batch(batch)
                    if fixture_replay_has_terminal_completion(batch.patches()) =>
                {
                    return batch;
                }
                ConversationPatchReplay::Batch(_) | ConversationPatchReplay::Current { .. } => {}
                ConversationPatchReplay::ResnapshotRequired { .. } => {
                    panic!("durable fixture replay should not require a resnapshot")
                }
            }
            commit_subscription
                .wait()
                .await
                .expect("fixture commit notifier should remain open");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("fixture terminal replay exceeded the test deadline"))
}

fn assert_fixture_delivery_batch(
    batch: &PatchBatch,
    expected_from: ConversationCursor,
    final_cursor: ConversationCursor,
    expected_cursor: &mut ConversationCursor,
) {
    assert_eq!(batch.thread_id().as_str(), "fixture-thread");
    assert_eq!(batch.from_cursor(), expected_from);
    assert!(
        batch.to_cursor() > expected_from,
        "Forge delivery batch must advance its cursor"
    );
    assert!(
        batch.to_cursor() <= final_cursor,
        "Forge delivery batch must remain within the final durable cursor"
    );
    for patch in batch.patches() {
        let expected_sequence = expected_cursor
            .checked_next_sequence()
            .expect("fixture replay sequence should not overflow");
        assert_eq!(
            patch.sequence(),
            expected_sequence,
            "Forge delivery must not duplicate or skip a sequence"
        );
        *expected_cursor = ConversationCursor::from(patch.sequence());
    }
    assert_eq!(*expected_cursor, batch.to_cursor());
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FixtureStreamObservation {
    cursor: ConversationCursor,
    assistant_item_id: ItemId,
    turn_id: TurnId,
}

fn fixture_stream_observation(patches: &[ConversationPatch]) -> FixtureStreamObservation {
    let mut streaming_items = patches.iter().filter_map(|patch| match patch {
        ConversationPatch::ItemUpsert {
            sequence,
            item: ConversationItem::AssistantMessage(item),
            ..
        } if item.lifecycle == ConversationLifecycle::Streaming => Some(FixtureStreamObservation {
            cursor: ConversationCursor::from(*sequence),
            assistant_item_id: item.item_id.clone(),
            turn_id: item.turn_id.clone(),
        }),
        _ => None,
    });
    let observation = streaming_items
        .next()
        .expect("delivered Forge patches should contain the assistant stream item");
    assert!(
        streaming_items.next().is_none(),
        "delivered Forge patches should contain one assistant stream item"
    );
    observation
}

fn assert_fixture_terminal_patches(
    patches: &[ConversationPatch],
    stream: &FixtureStreamObservation,
    final_cursor: ConversationCursor,
) {
    let item_completions: Vec<_> = patches
        .iter()
        .filter(|patch| {
            matches!(
                patch,
                ConversationPatch::ItemLifecycle {
                    lifecycle: ConversationLifecycle::Completed,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        item_completions.len(),
        1,
        "delivered Forge patches should contain one assistant completion"
    );
    let item_completion = item_completions[0];
    let ConversationPatch::ItemLifecycle {
        sequence, item_id, ..
    } = item_completion
    else {
        unreachable!("assistant completion was filtered as an item lifecycle")
    };
    assert_eq!(item_id, &stream.assistant_item_id);
    assert!(
        sequence.get() > stream.cursor.get() && sequence.get() <= final_cursor.get(),
        "assistant completion must follow the streaming item before the final cursor"
    );

    let turn_completions: Vec<_> = patches
        .iter()
        .filter(|patch| {
            matches!(
                patch,
                ConversationPatch::TurnLifecycle {
                    lifecycle: ConversationLifecycle::Completed,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        turn_completions.len(),
        1,
        "delivered Forge patches should contain one turn completion"
    );
    let turn_completion = turn_completions[0];
    let ConversationPatch::TurnLifecycle {
        sequence, turn_id, ..
    } = turn_completion
    else {
        unreachable!("turn completion was filtered as a turn lifecycle")
    };
    assert_eq!(turn_id, &stream.turn_id);
    assert!(
        sequence.get() > stream.cursor.get() && sequence.get() <= final_cursor.get(),
        "turn completion must follow the streaming item before the final cursor"
    );
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FixtureIdentitySnapshot {
    project: String,
    thread: String,
    request: String,
    message: String,
    run: String,
    turn: String,
    assistant_item: String,
}

async fn fixture_project_identity(database: &DatabaseConnection) -> String {
    let projects = entities::attached_project::Entity::find()
        .all(database)
        .await
        .expect("attached project rows should be readable");
    assert_eq!(projects.len(), 1);
    let project = projects.into_iter().next().expect("attached project row");
    assert!(
        project.project_id == "project-1",
        "the one attached project identity must remain stable"
    );
    project.project_id
}

async fn fixture_thread_identity(database: &DatabaseConnection) -> String {
    let threads = entities::thread::Entity::find()
        .all(database)
        .await
        .expect("thread rows should be readable");
    assert_eq!(threads.len(), 1);
    let thread = threads.into_iter().next().expect("thread row");
    assert!(
        thread.thread_id == "fixture-thread" && thread.project_id == "project-1",
        "the one thread must retain its project identity"
    );
    thread.thread_id
}

async fn fixture_request_identity(database: &DatabaseConnection) -> String {
    let request = entities::command_receipt::Entity::find_by_id("fixture-request")
        .one(database)
        .await
        .expect("queue request receipt should be readable")
        .expect("queue request receipt should exist");
    assert!(
        request.command_kind == entities::CommandKind::QueueFirstMessage
            && request.thread_id.as_deref() == Some("fixture-thread")
            && request.message_id.as_deref() == Some("fixture-message")
            && request.body.as_deref() == Some("hello world"),
        "the queued request must retain its durable message identity"
    );
    request.request_id
}

async fn fixture_message_identity(database: &DatabaseConnection) -> String {
    let messages = entities::message::Entity::find()
        .all(database)
        .await
        .expect("message rows should be readable");
    assert_eq!(messages.len(), 1);
    let message = messages.into_iter().next().expect("message row");
    assert!(
        message.message_id == "fixture-message"
            && message.thread_id == "fixture-thread"
            && message.body == "hello world",
        "the one durable prompt message must retain its identity"
    );
    message.message_id
}

async fn assert_fixture_dispatch_identity(database: &DatabaseConnection) {
    let dispatches = entities::message_dispatch::Entity::find()
        .all(database)
        .await
        .expect("dispatch rows should be readable");
    assert_eq!(dispatches.len(), 1);
    let dispatch = dispatches.into_iter().next().expect("dispatch row");
    assert!(
        dispatch.message_id == "fixture-message" && dispatch.correlation_id == "fixture-request",
        "the one dispatch must retain request and message correlation"
    );
}

async fn fixture_run_identity(database: &DatabaseConnection) -> String {
    let runs = entities::assistant_run::Entity::find()
        .all(database)
        .await
        .expect("run rows should be readable");
    assert_eq!(runs.len(), 1);
    let run = runs.into_iter().next().expect("run row");
    assert!(
        run.run_id == "fixture-run"
            && run.thread_id == "fixture-thread"
            && run.origin_message_id == "fixture-message"
            && run.origin_turn_id == "fixture-turn"
            && run.generation == 1,
        "the one run must retain its durable prompt and generation"
    );
    run.run_id
}

async fn fixture_turn_identity(database: &DatabaseConnection) -> String {
    let turns = entities::conversation_turn::Entity::find()
        .all(database)
        .await
        .expect("turn rows should be readable");
    assert_eq!(turns.len(), 1);
    let turn = turns.into_iter().next().expect("turn row");
    assert!(
        turn.turn_id == "fixture-turn" && turn.thread_id == "fixture-thread",
        "the one turn must retain its thread identity"
    );
    turn.turn_id
}

async fn fixture_assistant_item_identity(database: &DatabaseConnection) -> String {
    let items = entities::conversation_item::Entity::find()
        .all(database)
        .await
        .expect("conversation item rows should be readable");
    assert_eq!(items.len(), 2);
    let assistant_items: Vec<_> = items
        .iter()
        .filter(|item| item.item_kind == ConversationItemKind::AssistantMessage)
        .collect();
    assert_eq!(assistant_items.len(), 1);
    let user_items: Vec<_> = items
        .iter()
        .filter(|item| item.item_kind == ConversationItemKind::UserMessage)
        .collect();
    assert_eq!(user_items.len(), 1);
    assert!(
        user_items[0].source_message_id.as_deref() == Some("fixture-message"),
        "the prompt item must retain the durable message identity"
    );
    let assistant = assistant_items[0];
    assert!(
        assistant.item_id != "fixture-user-item"
            && assistant.turn_id == "fixture-turn"
            && assistant.run_id.as_deref() == Some("fixture-run"),
        "the one assistant item must retain its run and turn identity"
    );
    assistant.item_id.clone()
}

async fn fixture_identity_snapshot(database: &DatabaseConnection) -> FixtureIdentitySnapshot {
    let project = fixture_project_identity(database).await;
    let thread = fixture_thread_identity(database).await;
    let request = fixture_request_identity(database).await;
    let message = fixture_message_identity(database).await;
    assert_fixture_dispatch_identity(database).await;
    let run = fixture_run_identity(database).await;
    let turn = fixture_turn_identity(database).await;
    let assistant_item = fixture_assistant_item_identity(database).await;
    FixtureIdentitySnapshot {
        project,
        thread,
        request,
        message,
        run,
        turn,
        assistant_item,
    }
}

async fn reopen_fixture_repository(path: &Path) -> (DatabaseConnection, Repository) {
    let database = connect(
        SqliteConfig::file(path)
            .min_connections(1)
            .max_connections(4)
            .sqlx_logging(false),
    )
    .await
    .expect("same file-backed repository should reopen");
    migrate_to_current(&database)
        .await
        .expect("reopened repository should be current");
    (database.clone(), Repository::new(database))
}

fn assert_fixture_single_engine_pass() {
    let counts = crate::engine_owner::witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    assert_eq!(counts.kills_requested, 0);
    assert_eq!(counts.watchdog_failures_seen, 0);
    // The configured fixture has one successful health driver, one successful
    // session-creation driver, and one successful prompt driver. A reconnect never reaches the engine owner.
    assert_eq!(counts.control_driver_joined, 3);
}

struct FixtureDispatchState {
    pki: FixtureForgePki,
    notifier: ConversationCommitNotifier,
    commit_subscription: ConversationCommitSubscription,
    process_cancel: Arc<CancelHandle>,
    dispatcher: NativeRunDispatcher,
    listener_cancel: Arc<CancelHandle>,
    listener_task: tokio::task::JoinHandle<()>,
    first_client: FixtureForgeClient,
    delivery_stream: quinn::RecvStream,
}

struct FixtureRestartContext {
    pki: FixtureForgePki,
    notifier: ConversationCommitNotifier,
    commit_subscription: ConversationCommitSubscription,
}

struct FixtureRestartProof {
    context: FixtureRestartContext,
    database_path: PathBuf,
    thread_id: ThreadId,
    message_id: MessageId,
    stream: FixtureStreamObservation,
    final_cursor: ConversationCursor,
    before_restart: AllRows,
    identity_before: FixtureIdentitySnapshot,
}

async fn start_fixture_dispatch(
    repository: &Repository,
    database_path: &Path,
    thread_id: &ThreadId,
    fixture: PathBuf,
) -> FixtureDispatchState {
    let pki = fixture_forge_pki();
    let notifier = ConversationCommitNotifier::new();
    let commit_subscription = notifier
        .subscribe(thread_id.clone())
        .expect("fixture commit subscription");
    let (listener, handler, listener_cancel, listener_address) =
        bind_fixture_forge_listener(repository.clone(), notifier.clone(), &pki);
    let listener_task = tokio::spawn(serve_fixture_forge_listener(
        listener,
        handler,
        Arc::clone(&listener_cancel),
    ));
    let first_client = connect_fixture_forge_client(&pki, listener_address).await;
    let started = subscribe_fixture_forge_client(
        &first_client.connection,
        thread_id.clone(),
        ConversationCursor::default(),
    )
    .await;
    assert_fixture_resumed_subscription(started, thread_id, ConversationCursor::default());

    crate::engine_owner::reset_witnesses();
    let config = config_for_fixture_dispatch(notifier.clone()).expect("fixture dispatch policy");
    let process_cancel = Arc::new(CancelHandle::new());
    let dispatcher = NativeRunDispatcher::start_with_fixture_for_tests(
        repository.clone(),
        database_path.to_owned(),
        config,
        Arc::clone(&process_cancel),
        ActivityGateImpl::new(),
        &tokio::runtime::Handle::current(),
        fixture,
    );
    let delivery_stream = accept_fixture_delivery(&first_client.connection).await;

    FixtureDispatchState {
        pki,
        notifier,
        commit_subscription,
        process_cancel,
        dispatcher,
        listener_cancel,
        listener_task,
        first_client,
        delivery_stream,
    }
}

async fn collect_fixture_initial_delivery(
    repository: &Repository,
    thread_id: &ThreadId,
    state: &mut FixtureDispatchState,
) -> (ConversationCursor, FixtureStreamObservation) {
    let final_replay =
        wait_for_fixture_final_replay(repository, thread_id, &mut state.commit_subscription).await;
    let final_cursor = final_replay.to_cursor();
    assert!(
        final_cursor > ConversationCursor::default(),
        "fixture final replay should advance past cursor zero"
    );

    let mut next_cursor = ConversationCursor::default();
    let mut delivered_patches = Vec::new();
    while next_cursor < final_cursor {
        let batch = receive_fixture_patch_batch(&mut state.delivery_stream).await;
        let expected_from = next_cursor;
        assert_fixture_delivery_batch(&batch, expected_from, final_cursor, &mut next_cursor);
        let expected = fixture_durable_replay_prefix(
            repository,
            thread_id,
            batch.from_cursor(),
            batch.to_cursor(),
        )
        .await;
        assert_eq!(
            batch, expected,
            "each Forge batch must equal the durable replay for its cursor range"
        );
        delivered_patches.extend(batch.patches().iter().cloned());
    }
    assert_eq!(next_cursor, final_cursor);
    assert_eq!(
        delivered_patches.as_slice(),
        final_replay.patches(),
        "Forge batches must cover the exact final durable replay without gaps"
    );
    let stream = fixture_stream_observation(&delivered_patches);
    assert_fixture_terminal_patches(&delivered_patches, &stream, final_cursor);
    (final_cursor, stream)
}

async fn shutdown_fixture_initial_dispatch(
    state: FixtureDispatchState,
    database: &DatabaseConnection,
    thread_id: &ThreadId,
    message_id: &MessageId,
) -> (FixtureRestartContext, AllRows, FixtureIdentitySnapshot) {
    let FixtureDispatchState {
        pki,
        notifier,
        commit_subscription,
        process_cancel,
        mut dispatcher,
        listener_cancel,
        listener_task,
        first_client,
        delivery_stream,
    } = state;

    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    assert_fixture_single_engine_pass();
    listener_cancel.cancel();
    listener_task
        .await
        .expect("first Forge listener should shut down cleanly");
    drop(delivery_stream);
    shutdown_fixture_forge_client(first_client, b"fixture delivery client complete").await;

    let before_restart = fetch_all(database).await;
    assert_fixture_dispatch(&before_restart, message_id);
    assert_fixture_run(&before_restart);
    assert_fixture_transcript(&before_restart, thread_id);
    assert_fixture_batch_durability(database).await;
    let identity_before = fixture_identity_snapshot(database).await;
    (
        FixtureRestartContext {
            pki,
            notifier,
            commit_subscription,
        },
        before_restart,
        identity_before,
    )
}

async fn restart_fixture_delivery(proof: FixtureRestartProof) {
    let FixtureRestartProof {
        context,
        database_path,
        thread_id,
        message_id,
        stream,
        final_cursor,
        before_restart,
        identity_before,
    } = proof;
    let FixtureRestartContext {
        pki,
        notifier,
        commit_subscription: _commit_subscription,
    } = context;
    let (reopened_database, reopened_repository) = reopen_fixture_repository(&database_path).await;
    let (restarted_listener, restarted_handler, restarted_cancel, restarted_address) =
        bind_fixture_forge_listener(reopened_repository.clone(), notifier, &pki);
    let restarted_listener_task = tokio::spawn(serve_fixture_forge_listener(
        restarted_listener,
        restarted_handler,
        Arc::clone(&restarted_cancel),
    ));
    let resumed_client = connect_fixture_forge_client(&pki, restarted_address).await;
    let restarted = subscribe_fixture_forge_client(
        &resumed_client.connection,
        thread_id.clone(),
        stream.cursor,
    )
    .await;
    assert_fixture_resumed_subscription(restarted, &thread_id, stream.cursor);
    let mut resumed_stream = accept_fixture_delivery(&resumed_client.connection).await;
    let resumed_batch = receive_fixture_patch_batch(&mut resumed_stream).await;
    let restarted_final_replay =
        fixture_durable_replay(&reopened_repository, &thread_id, stream.cursor).await;
    assert_eq!(restarted_final_replay.to_cursor(), final_cursor);
    assert_eq!(resumed_batch.from_cursor(), stream.cursor);
    assert_eq!(resumed_batch.to_cursor(), final_cursor);
    assert!(
        resumed_batch == restarted_final_replay,
        "restart delivery must be the exact final replay from the stream cursor"
    );

    restarted_cancel.cancel();
    restarted_listener_task
        .await
        .expect("restarted Forge listener should shut down cleanly");
    drop(resumed_stream);
    shutdown_fixture_forge_client(resumed_client, b"fixture restart client complete").await;

    let after_restart = fetch_all(&reopened_database).await;
    assert!(
        after_restart == before_restart,
        "listener restart must not mutate or requeue completed durable work"
    );
    assert_fixture_dispatch(&after_restart, &message_id);
    assert_fixture_run(&after_restart);
    assert_fixture_transcript(&after_restart, &thread_id);
    assert_fixture_batch_durability(&reopened_database).await;
    let identity_after = fixture_identity_snapshot(&reopened_database).await;
    assert!(
        identity_after == identity_before,
        "restart must preserve project, thread, request, message, run, turn, and assistant identity"
    );

    drop(reopened_repository);
    reopened_database
        .close()
        .await
        .expect("restarted repository should close cleanly");
    assert_fixture_single_engine_pass();
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_fixture_streams_over_forge_delivery_and_resumes_after_restart() {
    let fixture = registered_fixture_program();
    let (database, repository, temp) = temp_repository("dispatch-forge-restart").await;
    let thread_id = ThreadId::parse("fixture-thread").expect("thread id");
    let message_id = MessageId::parse("fixture-message").expect("message id");
    seed_project_and_thread_with_profile(
        &database,
        &repository,
        thread_id.as_str(),
        "fixture-test",
        30_000,
        5_000,
        2_000,
    )
    .await;
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("fixture-request").expect("request id"),
            message_id: message_id.clone(),
            thread_id: thread_id.clone(),
            body: MessageBody::parse("hello world").expect("message body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("one fixture message should queue");

    let mut dispatch_state =
        start_fixture_dispatch(&repository, temp.path(), &thread_id, fixture).await;
    let (final_cursor, stream) =
        collect_fixture_initial_delivery(&repository, &thread_id, &mut dispatch_state).await;
    let (restart_context, before_restart, identity_before) =
        shutdown_fixture_initial_dispatch(dispatch_state, &database, &thread_id, &message_id).await;
    drop(repository);
    database
        .close()
        .await
        .expect("original repository should close before restart");

    restart_fixture_delivery(FixtureRestartProof {
        context: restart_context,
        database_path: temp.path().to_owned(),
        thread_id,
        message_id,
        stream,
        final_cursor,
        before_restart,
        identity_before,
    })
    .await;
}

// ---------------------------------------------------------------------------
// Configured-engine mid-turn loss and recovery proof
// ---------------------------------------------------------------------------

struct MidturnLossPatchSource {
    notifier: ConversationCommitNotifier,
}

impl crate::startup_reconciliation_sweep::StartupReconciliationPatchSource
    for MidturnLossPatchSource
{
    fn patch_ids_for(
        &mut self,
        candidate: &artisan_database::StartupReconciliationCandidate,
    ) -> Result<
        crate::startup_reconciliation_sweep::StartupReconciliationPatches,
        crate::startup_reconciliation_sweep::PatchSourceError,
    > {
        use artisan_domain::PatchId;
        let turn_patch_id = PatchId::parse(candidate.run_id.as_str())
            .map_err(|_| crate::startup_reconciliation_sweep::PatchSourceError)?;
        let item_patch_id = candidate
            .assistant_item_id
            .as_ref()
            .map(|item_id| {
                PatchId::parse(item_id.as_str())
                    .map_err(|_| crate::startup_reconciliation_sweep::PatchSourceError)
            })
            .transpose()?;
        Ok(
            crate::startup_reconciliation_sweep::StartupReconciliationPatches::new(
                turn_patch_id,
                item_patch_id,
            ),
        )
    }

    fn on_durable_disposition(
        &mut self,
        candidate: &artisan_database::StartupReconciliationCandidate,
    ) {
        let _ = self.notifier.publish(&candidate.thread_id);
    }
}

const MIDTURN_LOSS_SCENARIO: &str = "prompt_text_then_hold_after_first_delta";
const MIDTURN_LOSS_DEADLINE: Duration = Duration::from_secs(20);
const MIDTURN_RESTART_QUIESCE: Duration = Duration::from_millis(500);

struct MidturnLossSeed {
    database: DatabaseConnection,
    repository: Repository,
    temp: TempDatabase,
    thread_id: ThreadId,
    message_id: MessageId,
    fixture: PathBuf,
}

async fn seed_midturn_loss() -> MidturnLossSeed {
    let fixture = registered_fixture_program();
    let (database, repository, temp) = temp_repository("dispatch-midturn-loss").await;
    let thread_id = ThreadId::parse("fixture-midturn-thread").expect("thread id");
    let message_id = MessageId::parse("fixture-midturn-message").expect("message id");
    seed_project_and_thread_with_profile(
        &database,
        &repository,
        thread_id.as_str(),
        "fixture-test",
        125_000,
        30_000,
        5_000,
    )
    .await;
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("fixture-midturn-request").expect("request id"),
            message_id: message_id.clone(),
            thread_id: thread_id.clone(),
            body: MessageBody::parse("hello world").expect("message body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("one fixture message should queue");
    assert_queued_fixture_message(&fetch_all(&database).await, &message_id);
    MidturnLossSeed {
        database,
        repository,
        temp,
        thread_id,
        message_id,
        fixture,
    }
}

async fn await_first_durable_delta(
    database: &DatabaseConnection,
    subscription: &mut ConversationCommitSubscription,
) {
    // Deterministic hold witness: the first assistant delta must become
    // durable through the run-dispatch/repository path. Event-driven commit
    // wakes drive this wait; no timing sleep stands in for the hold.
    tokio::time::timeout(MIDTURN_LOSS_DEADLINE, async {
        loop {
            let items = entities::conversation_item::Entity::find()
                .all(database)
                .await
                .expect("conversation items should be readable");
            let assistant_delta = items.iter().any(|item| {
                item.item_kind == ConversationItemKind::AssistantMessage
                    && item.body == "hello world"
            });
            let receipts = entities::run_batch_receipt::Entity::find()
                .all(database)
                .await
                .expect("batch receipts should be readable");
            if assistant_delta && !receipts.is_empty() {
                return;
            }
            subscription
                .wait()
                .await
                .expect("mid-turn commit notifier should remain open");
        }
    })
    .await
    .expect("first assistant delta should become durable before the deadline");
}

fn assert_midturn_hold_state(held: &AllRows, message_id: &MessageId) -> u64 {
    assert_eq!(held.dispatches.len(), 1);
    assert_eq!(held.runs.len(), 1);
    let held_dispatch = held
        .dispatches
        .iter()
        .find(|dispatch| dispatch.message_id == message_id.as_str())
        .expect("mid-turn dispatch");
    assert_eq!(held_dispatch.state, DispatchState::Running);
    let held_run = held
        .runs
        .iter()
        .find(|run| run.run_id == "fixture-run")
        .expect("mid-turn run");
    assert_eq!(held_run.lifecycle, AssistantRunLifecycle::Running);
    assert_eq!(held_run.generation, 1);
    assert!(held_run.terminal_at_ms.is_none());
    assert!(held_run.error_code.is_none());
    assert_eq!(held_run.provider_binding_version, Some(1));
    let held_binding: serde_json::Value = serde_json::from_slice(
        held_run
            .provider_binding
            .as_ref()
            .expect("mid-turn provider binding")
            .as_slice(),
    )
    .expect("mid-turn provider binding JSON");
    assert_eq!(
        held_binding,
        serde_json::json!({
            "engine": "opencode2",
            "profile_id": "fixture-test",
            "session_id": "test-session",
        })
    );
    let held_assistant: Vec<_> = held
        .items
        .iter()
        .filter(|item| item.item_kind == ConversationItemKind::AssistantMessage)
        .collect();
    assert_eq!(held_assistant.len(), 1);
    assert_eq!(held_assistant[0].body, "hello world");
    assert_eq!(held_assistant[0].run_id.as_deref(), Some("fixture-run"));
    let held_counts = crate::engine_owner::witness_counts();
    assert_eq!(held_counts.spawned, 1);
    assert_eq!(held_counts.reaps_observed, 0);
    assert_eq!(held_counts.watchdog_failures_seen, 0);
    held_counts.control_driver_joined
}

#[tokio::test(flavor = "current_thread")]
async fn dispatch_fixture_midturn_engine_loss_recovers_without_second_spawn() {
    let MidturnLossSeed {
        database,
        repository,
        temp,
        thread_id,
        message_id,
        fixture,
    } = seed_midturn_loss().await;

    let notifier = ConversationCommitNotifier::new();
    let mut subscription = notifier
        .subscribe(thread_id.clone())
        .expect("mid-turn thread subscription");
    let config = config_for_fixture_dispatch(notifier.clone()).expect("fixture dispatch policy");
    crate::engine_owner::reset_witnesses();
    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher = NativeRunDispatcher::start_with_fixture_scenario_for_tests(
        FixtureScenarioLaunch {
            repository: repository.clone(),
            database_path: temp.path().to_owned(),
            config,
            process_cancel: Arc::clone(&process_cancel),
            activity: ActivityGateImpl::new(),
            runtime: &tokio::runtime::Handle::current(),
            fixture_program: fixture.clone(),
            scenario: MIDTURN_LOSS_SCENARIO,
        },
    );
    await_first_durable_delta(&database, &mut subscription).await;

    // Exactly one run was admitted and the owning engine is deterministically
    // alive in its hold: one child spawned, none reaped yet.
    let held = fetch_all(&database).await;
    let drivers_at_hold = assert_midturn_hold_state(&held, &message_id);

    // Stop the owning engine process through the existing custody boundary.
    // The fixture holds its log connection open with no terminal event, so
    // this cancellation is the deterministic engine-loss witness.
    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    let loss_counts = crate::engine_owner::witness_counts();
    assert_eq!(loss_counts.spawned, 1);
    assert_eq!(loss_counts.reaps_observed, 1);
    // Mid-turn abort custody: `cleanup_after_abort` closes the lifeline and
    // then requests whole-job termination before its bounded reap wait for
    // configured launches (`engine_owner/process.rs`), so one kill with one
    // reap is the designed stop for the held log connection.
    assert_eq!(loss_counts.kills_requested, 1);
    assert_eq!(loss_counts.watchdog_failures_seen, 0);
    assert!(format!("{dispatcher:?}").contains("payload-free"));

    // The interrupted run, turn, and assistant item reach the existing
    // terminal/interrupted disposition with the durable first delta intact:
    // no second origin row, no invented success.
    let after = fetch_all(&database).await;
    assert_midturn_interrupted_dispatch_run(&after, &message_id);
    assert_midturn_interrupted_items(&after);
    assert_midturn_replay_contiguity(&after, &database, &thread_id).await;

    // Run the existing startup-reconciliation path against the same file-backed
    // repository: nothing remains to interrupt, nothing is requeued, and the
    // durable rows are byte-stable.
    assert_midturn_quiescent_sweep(&repository, &notifier, &database, &after, &message_id).await;

    // Restart against the reopened file-backed repository: no second fixture
    // child spawn, session creation, or prompt, and no durable mutation.
    let settled_counts = crate::engine_owner::witness_counts();
    let (reopened_database, reopened_repository) =
        restart_midturn_dispatcher(repository, database, temp, fixture).await;
    let restart_counts = crate::engine_owner::witness_counts();
    assert_eq!(restart_counts.spawned, settled_counts.spawned);
    assert_eq!(restart_counts.reaps_observed, settled_counts.reaps_observed);
    assert_eq!(
        restart_counts.control_driver_joined,
        settled_counts.control_driver_joined
    );
    assert_eq!(restart_counts.watchdog_failures_seen, 0);
    assert!(drivers_at_hold <= settled_counts.control_driver_joined);
    let after_restart = fetch_all(&reopened_database).await;
    assert_eq!(after_restart.dispatches.len(), 1);
    assert_eq!(after_restart.runs.len(), 1);
    assert_eq!(after_restart.turns.len(), 1);
    assert_eq!(after_restart.patches, after.patches);
    assert_midturn_restart_replay(&reopened_repository, &after, &thread_id).await;
    drop(reopened_repository);
    reopened_database
        .close()
        .await
        .expect("restarted repository should close cleanly");
}

fn assert_midturn_interrupted_dispatch_run(after: &AllRows, message_id: &MessageId) {
    // The interrupted run, turn, and assistant item reach the existing
    // terminal/interrupted disposition with the durable first delta intact:
    // no second origin row, no invented success.
    assert_eq!(after.dispatches.len(), 1);
    assert_eq!(after.runs.len(), 1);
    assert_eq!(after.turns.len(), 1);
    let dispatch = after
        .dispatches
        .iter()
        .find(|dispatch| dispatch.message_id == message_id.as_str())
        .expect("interrupted dispatch");
    assert_eq!(dispatch.state, DispatchState::Failed);
    assert_eq!(dispatch.attempt_count, 1);
    assert!(dispatch.lease_owner.is_none());
    assert!(dispatch.lease_expires_at_ms.is_none());
    assert_eq!(
        dispatch.last_error.as_deref(),
        Some("OpenCode2 provider turn interrupted")
    );
    let run = after
        .runs
        .iter()
        .find(|run| run.run_id == "fixture-run")
        .expect("interrupted run");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
    assert_eq!(run.generation, 1);
    assert_eq!(run.error_code.as_deref(), Some("provider_interrupted"));
    assert_eq!(
        run.error_message.as_deref(),
        Some("OpenCode2 provider turn interrupted")
    );
    assert!(run.terminal_at_ms.is_none());
    assert!(run.owner.is_none());
    assert!(run.lease.is_none());
    assert!(run.claim_token.is_none());
    assert_eq!(run.provider_binding_version, Some(1));
    let binding: serde_json::Value = serde_json::from_slice(
        run.provider_binding
            .as_ref()
            .expect("provider binding")
            .as_slice(),
    )
    .expect("provider binding JSON");
    assert_eq!(
        binding,
        serde_json::json!({
            "engine": "opencode2",
            "profile_id": "fixture-test",
            "session_id": "test-session",
        })
    );
}

fn assert_midturn_interrupted_items(after: &AllRows) {
    let turn = after
        .turns
        .iter()
        .find(|turn| turn.turn_id == "fixture-turn")
        .expect("interrupted turn");
    assert_eq!(turn.lifecycle, EntityLifecycle::Interrupted);
    let assistant_items: Vec<_> = after
        .items
        .iter()
        .filter(|item| item.item_kind == ConversationItemKind::AssistantMessage)
        .collect();
    assert_eq!(assistant_items.len(), 1);
    assert_eq!(assistant_items[0].lifecycle, EntityLifecycle::Interrupted);
    assert_eq!(assistant_items[0].body, "hello world");
    assert_eq!(assistant_items[0].run_id.as_deref(), Some("fixture-run"));
    let user_items: Vec<_> = after
        .items
        .iter()
        .filter(|item| item.item_kind == ConversationItemKind::UserMessage)
        .collect();
    assert_eq!(user_items.len(), 1);
    assert_eq!(user_items[0].body, "hello world");
}

async fn assert_midturn_replay_contiguity(
    after: &AllRows,
    database: &DatabaseConnection,
    thread_id: &ThreadId,
) {
    // Replay stays bounded and contiguous with no duplicated origin rows.
    assert!(!after.patches.is_empty());
    assert!(after.patches.len() <= 16);
    let sequences: Vec<i64> = after.patches.iter().map(|patch| patch.sequence).collect();
    let patch_total = i64::try_from(sequences.len()).expect("patch count fits i64");
    let expected: Vec<i64> = (1..=patch_total).collect();
    assert_eq!(sequences, expected);
    assert!(
        after
            .patches
            .iter()
            .any(|patch| patch.body.as_deref() == Some("hello world")
                && patch.item_kind == Some(ConversationItemKind::AssistantMessage))
    );
    assert!(
        after
            .patches
            .iter()
            .any(|patch| matches!(patch.kind, ConversationPatchKind::ItemLifecycle))
    );
    let state = after
        .states
        .iter()
        .find(|state| state.thread_id == thread_id.as_str())
        .expect("conversation state");
    assert_eq!(state.last_patch_sequence, patch_total);
    let checkpoints = entities::run_checkpoint::Entity::find()
        .all(database)
        .await
        .expect("checkpoint rows");
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].run_id, "fixture-run");
    assert_eq!(checkpoints[0].generation, 1);
    let receipts = entities::run_batch_receipt::Entity::find()
        .all(database)
        .await
        .expect("batch receipt rows");
    assert!(!receipts.is_empty());
    for receipt in &receipts {
        assert_eq!(receipt.run_id, "fixture-run");
        assert_eq!(receipt.generation, 1);
        assert!(receipt.committed);
    }
}

async fn assert_midturn_quiescent_sweep(
    repository: &Repository,
    notifier: &ConversationCommitNotifier,
    database: &DatabaseConnection,
    after: &AllRows,
    message_id: &MessageId,
) {
    let operated_at = crate::SystemCommandOrigin
        .acceptance_instant()
        .expect("recovery clock should succeed");
    let mut patch_source = MidturnLossPatchSource {
        notifier: notifier.clone(),
    };
    let report = crate::startup_reconciliation_sweep::sweep_startup_reconciliation(
        repository,
        crate::startup_reconciliation_sweep::StartupReconciliationSweepInput::new(operated_at, 64)
            .expect("bounded sweep input"),
        &mut patch_source,
    )
    .await
    .expect("startup reconciliation sweep should succeed");
    assert_eq!(report.discovered, 0);
    assert_eq!(report.attempted, 0);
    assert_eq!(report.interrupted, 0);
    assert_eq!(report.already_interrupted, 0);
    let after_sweep = fetch_all(database).await;
    assert_eq!(after_sweep, *after);
    let redispatch = after_sweep
        .dispatches
        .iter()
        .find(|dispatch| dispatch.message_id == message_id.as_str())
        .expect("dispatch after sweep");
    assert_eq!(redispatch.state, DispatchState::Failed);
}

async fn restart_midturn_dispatcher(
    repository: Repository,
    database: DatabaseConnection,
    temp: TempDatabase,
    fixture: PathBuf,
) -> (DatabaseConnection, Repository) {
    drop(repository);
    database
        .close()
        .await
        .expect("original repository should close before restart");
    let (reopened_database, reopened_repository) = reopen_fixture_repository(temp.path()).await;
    let restart_notifier = ConversationCommitNotifier::new();
    let restart_config =
        config_for_fixture_dispatch(restart_notifier).expect("restart dispatch policy");
    let restart_cancel = Arc::new(CancelHandle::new());
    let mut restarted = NativeRunDispatcher::start_with_fixture_scenario_for_tests(
        FixtureScenarioLaunch {
            repository: reopened_repository.clone(),
            database_path: temp.path().to_owned(),
            config: restart_config,
            process_cancel: Arc::clone(&restart_cancel),
            activity: ActivityGateImpl::new(),
            runtime: &tokio::runtime::Handle::current(),
            fixture_program: fixture,
            scenario: MIDTURN_LOSS_SCENARIO,
        },
    );
    tokio::time::sleep(MIDTURN_RESTART_QUIESCE).await;
    restart_cancel.cancel();
    assert_eq!(
        restarted.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    assert_eq!(
        restarted.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    (reopened_database, reopened_repository)
}

async fn assert_midturn_restart_replay(
    reopened_repository: &Repository,
    after: &AllRows,
    thread_id: &ThreadId,
) {
    let restarted_run = after
        .runs
        .iter()
        .find(|run| run.run_id == "fixture-run")
        .expect("restarted run");
    assert_eq!(restarted_run.lifecycle, AssistantRunLifecycle::Interrupted);
    assert_eq!(restarted_run.generation, 1);
    let replay = reopened_repository
        .read_conversation_patch_replay(thread_id, ConversationCursor::default())
        .await
        .expect("restart replay should be readable");
    match replay {
        ConversationPatchReplay::Batch(batch) => {
            assert_eq!(batch.from_cursor(), ConversationCursor::default());
            assert_eq!(batch.patches().len(), after.patches.len());
            let mut cursor = ConversationCursor::default();
            for patch in batch.patches() {
                let expected_sequence = cursor
                    .checked_next_sequence()
                    .expect("restart replay sequence should not overflow");
                assert_eq!(patch.sequence(), expected_sequence);
                cursor = ConversationCursor::from(patch.sequence());
            }
            assert_eq!(
                usize::try_from(cursor.get()).expect("replay cursor fits usize"),
                batch.patches().len()
            );
        }
        ConversationPatchReplay::Current { .. }
        | ConversationPatchReplay::ResnapshotRequired { .. } => {
            panic!("restart replay should contain the interrupted patches");
        }
    }
}
