//! Focused checks for the explicit configured-run scheduler boundary.

use std::{num::NonZeroUsize, path::PathBuf, sync::Arc, time::Duration};

use artisan_database::{RepositoryError, RunLaunchError, SqliteConfig, connect};
use artisan_domain::MessageId;
use artisan_migrations::migrate_to_current;
use artisan_native_engine::NativeOpenCode2Authority;
use artisan_transport::CancelHandle;

use super::{
    NativeRunDispatcherConfig, NativeRunDispatcherConfigError, NativeRunDispatcherConfigInput,
    conversation_commit_notifier::ConversationCommitNotifier,
};
use crate::native_run_dispatch::{
    LaunchAuthority, NativeRunDispatcher, NativeRunDispatcherShutdown, PromptAuthorization,
    SettingsLoadDecision, classify_launch_result, classify_settings_load, notify_after_commit,
    prompt_authorization_after_binding,
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
        config("queue").expect("complete dispatcher policy"),
        Arc::clone(&process_cancel),
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
use std::sync::atomic::{AtomicU64, Ordering};

use artisan_database::entities::{self, AssistantRunLifecycle, DispatchState, EntityLifecycle};
use artisan_database::{
    AssistantChange, BindRunProvider, ClaimMessageDispatch, CreateThreadInput,
    ProviderBindingBytes, QueueFirstMessageInput, Repository, RunLaunchCredentials, RunStartKey,
    SetThreadEngineConfigInput,
};
use artisan_domain::ThreadId as DomainThreadId;
use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantMessagePhase, ByteLimit, CountLimit, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, ItemId, MessageBody, NetworkAccess,
    OpenCode2Selection, PatchId, ProjectId, RequestId, RunId, ThreadId, TurnId, UnixMillis,
    WebSearchAccess,
};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

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
        PermissionId::parse("permission-reconcile").expect("permission id is valid"),
        EngineAgentId::parse("agent-reconcile").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-reconcile").expect("profile id is valid"),
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
                config: fixture_engine_config(),
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

async fn commit_running_item(
    repository: &Repository,
    claimed: &artisan_database::ClaimedMessageDispatch,
    receipt: &artisan_database::LaunchedRunReceipt,
    bound: &artisan_database::BoundRunReceipt,
    start_key: &RunStartKey,
    creds: &RunLaunchCredentials,
    item_id: &ItemId,
    patch_turn: &PatchId,
    patch_item: &PatchId,
) {
    let body = AssistantBody::parse("hello assistant").expect("body");
    repository
        .commit_run_batch(artisan_database::CommitRunBatch {
            scope: artisan_database::RunBatchScope {
                claimed,
                launched: receipt,
                bound,
                run_start_key: start_key,
                credentials: creds,
                expected_launch_at: UnixMillis::from_millis(150),
                expected_updated_at: UnixMillis::from_millis(200),
            },
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(250),
            activate_turn_patch_id: Some(patch_turn),
            changes: &[AssistantChange::Start {
                item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: patch_item,
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
    let mut dispatcher = NativeRunDispatcher::start(
        repository.clone(),
        StdPathBuf::from("C:/forge/database.sqlite3"),
        config,
        Arc::clone(&process_cancel),
        &tokio::runtime::Handle::current(),
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    let mid = fetch_all(&database).await;
    assert_eq!(before.dispatches, mid.dispatches);
    assert_eq!(before.runs, mid.runs);
    assert_eq!(before.patches, mid.patches);

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
    // launching candidate (no item, no binding)
    seed_project_and_thread(&database, &repository, "thread-launch").await;
    let (_cl1, _rc1, _sk1, _cr1) = queue_claim_launch(
        &repository,
        "thread-launch",
        "msg-launch",
        "run-launch",
        "turn-launch",
    )
    .await;
    // running candidate with item and binding
    seed_project_and_thread(&database, &repository, "thread-run").await;
    let (claimed2, receipt2, sk2, cr2) =
        queue_claim_launch(&repository, "thread-run", "msg-run", "run-run", "turn-run").await;
    let bound2 = bind_running(&repository, &claimed2, &receipt2, &sk2, &cr2).await;
    let before_binding = {
        let run = entities::assistant_run::Entity::find_by_id("run-run")
            .one(&database)
            .await
            .expect("find")
            .expect("run");
        run.provider_binding.clone()
    };
    assert!(
        before_binding.is_some(),
        "running candidate must have binding before sweep"
    );
    let item_id = ItemId::parse("assistant-run").expect("item");
    commit_running_item(
        &repository,
        &claimed2,
        &receipt2,
        &bound2,
        &sk2,
        &cr2,
        &item_id,
        &PatchId::parse("p-turn-run").expect("p"),
        &PatchId::parse("p-item-run").expect("p"),
    )
    .await;

    let notifier = ConversationCommitNotifier::new();
    let config = config_with_notifier(notifier, Duration::from_millis(15)).expect("config");
    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher = NativeRunDispatcher::start(
        repository.clone(),
        StdPathBuf::from("C:/forge/database.sqlite3"),
        config,
        Arc::clone(&process_cancel),
        &tokio::runtime::Handle::current(),
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );

    let after = fetch_all(&database).await;
    // Both dispatches failed
    for msg in ["msg-launch", "msg-run"] {
        let d = after
            .dispatches
            .iter()
            .find(|x| x.message_id == msg)
            .expect("dispatch");
        assert_eq!(d.state, DispatchState::Failed);
        assert_eq!(
            d.last_error.as_deref(),
            Some("startup reconciliation: unknown outcome after lease expiry")
        );
    }
    // Both runs interrupted
    for run_id in ["run-launch", "run-run"] {
        let r = after.runs.iter().find(|x| x.run_id == run_id).expect("run");
        assert_eq!(r.lifecycle, AssistantRunLifecycle::Interrupted);
        assert_eq!(
            r.error_code.as_deref(),
            Some("startup_reconciliation_unknown_outcome")
        );
    }
    // Binding retained for running candidate, still None for launching
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
    assert_eq!(
        run_run.provider_binding, before_binding,
        "binding must be retained"
    );
    assert!(run_run.provider_binding_version.is_some());
    // Conversation lifecycle interrupted
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
        &tokio::runtime::Handle::current(),
    );

    let notified = tokio::time::timeout(Duration::from_millis(800), subscription.wait()).await;
    assert!(
        notified.is_ok(),
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
