//! Atomic first-run launch coverage through real migrated SQLite and the
//! public repository APIs.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{self, AssistantRunLifecycle, DispatchState};
use artisan_database::{
    ClaimMessageDispatch, ClaimedMessageDispatch, CompleteMessageDispatch, CreateThreadInput,
    DispatchFailureReason, DispatchLeaseOwner, FailMessageDispatch, LaunchClaimedRun,
    LaunchClaimedRunOutcome, QueueFirstMessageInput, Repository, RepositoryError,
    RequeueMessageDispatch, RunLaunchCredentials, RunLaunchError, RunStartKey,
    SetThreadEngineConfigInput, SqliteConfig, ThreadEngineSettings, connect,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, EngineAgentId, EngineConfigUpdatePrecondition,
    EngineModelId, EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig,
    EngineRuntimeControls, EngineRuntimeControlsInput, EngineSelection, FilesystemAccess,
    FiniteMillis, ItemId, MessageBody, MessageId, NetworkAccess, OpenCode2Selection, PatchId,
    PermissionId, ProjectId, RequestId, RunId, ThreadId, ThreadTitle, TurnId, UnixMillis,
    WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel,
};

const OWNER_BYTES: [u8; 32] = [0xa1; 32];
const LEASE_BYTES: [u8; 32] = [0xb2; 32];
const CLAIM_TOKEN_BYTES: [u8; 32] = [0xc3; 32];
const START_KEY_BYTES: [u8; 32] = [0xd4; 32];
const DISPATCH_OWNER_BYTE: u8 = 0x11;

const RUN_ID: &str = "run-1";
const TURN_ID: &str = "turn-1";
const ITEM_ID: &str = "item-1";
const FIRST_PATCH_ID: &str = "patch-a";
const SECOND_PATCH_ID: &str = "patch-b";
const MESSAGE_ID: &str = "message-1";
const CORRELATION_ID: &str = "request-1";
const THREAD_ID: &str = "thread-1";

const THREAD_CREATED_AT_MS: i64 = 10;
const ACCEPTED_AT_MS: i64 = 50;
const CLAIMED_AT_MS: i64 = 100;
const LEASE_EXPIRES_AT_MS: i64 = 600;
const OPERATED_AT_MS: i64 = 150;

static ENGINE_SETTINGS: OnceLock<ThreadEngineSettings> = OnceLock::new();

const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: ?Sized + std::fmt::Debug> Ambiguous<Marker> for T {}
    let _ = <RunStartKey as Ambiguous<_>>::marker;
    let _ = <RunLaunchCredentials as Ambiguous<_>>::marker;
};

const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: ?Sized + std::fmt::Display> Ambiguous<Marker> for T {}
    let _ = <RunStartKey as Ambiguous<_>>::marker;
    let _ = <RunLaunchCredentials as Ambiguous<_>>::marker;
};

const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: Clone> Ambiguous<Marker> for T {}
    let _ = <RunStartKey as Ambiguous<_>>::marker;
    let _ = <RunLaunchCredentials as Ambiguous<_>>::marker;
};

const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: ?Sized + PartialEq> Ambiguous<Marker> for T {}
    let _ = <RunStartKey as Ambiguous<_>>::marker;
    let _ = <RunLaunchCredentials as Ambiguous<_>>::marker;
};

async fn memory_database() -> (DatabaseConnection, Repository) {
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
    (database.clone(), Repository::new(database))
}

struct TempDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TempDatabase {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after the epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "artisan-editor-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("temporary directory should create");
        let database = directory.join("forge.sqlite3");
        Self {
            directory,
            database,
        }
    }

    fn database(&self) -> &Path {
        &self.database
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _cleanup_result = std::fs::remove_dir_all(&self.directory);
    }
}

async fn seed_project_and_thread(database: &DatabaseConnection, repository: &Repository) {
    entities::attached_project::ActiveModel {
        project_id: Set("project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    }
    .insert(database)
    .await
    .expect("fixture project should insert");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse("seed-thread-request").expect("fixture request id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("fixture thread id"),
            project_id: ProjectId::parse("project-1").expect("fixture project id"),
            title: ThreadTitle::parse("Thread").expect("fixture title"),
            created_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
            updated_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await
        .expect("fixture thread should create");
}

async fn seeded_repository() -> (DatabaseConnection, Repository) {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository).await;
    let thread_id = ThreadId::parse(THREAD_ID).expect("fixture thread id");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("seed-engine-config").expect("fixture request id"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: launch_config(),
            accepted_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await
        .expect("fixture engine configuration should create");
    let settings = repository
        .read_thread_engine_settings(&thread_id)
        .await
        .expect("fixture engine configuration should read")
        .expect("fixture engine configuration should be present");
    let _ = ENGINE_SETTINGS.set(settings);
    (database, repository)
}

fn launch_engine_settings() -> &'static ThreadEngineSettings {
    ENGINE_SETTINGS
        .get()
        .expect("seeded launch settings should be initialized")
}

fn launch_config() -> EngineRunConfig {
    launch_config_with_profile("profile-launch")
}

fn launch_config_with_profile(profile: &str) -> EngineRunConfig {
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
        PermissionId::parse("permission-launch").expect("permission id is valid"),
        EngineAgentId::parse("agent-launch").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse(profile).expect("profile id is valid"),
            EngineModelId::parse("model-launch").expect("model id is valid"),
            EngineRouteId::parse("route-launch").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

fn queue_input() -> QueueFirstMessageInput {
    QueueFirstMessageInput {
        request_id: RequestId::parse(CORRELATION_ID).expect("fixture request id"),
        message_id: MessageId::parse(MESSAGE_ID).expect("fixture message id"),
        thread_id: ThreadId::parse(THREAD_ID).expect("fixture thread id"),
        body: MessageBody::parse("first durable body").expect("fixture body"),
        accepted_at: UnixMillis::from_millis(ACCEPTED_AT_MS),
    }
}

fn claim_command(owner_byte: u8) -> ClaimMessageDispatch {
    ClaimMessageDispatch {
        owner: DispatchLeaseOwner::new([owner_byte; 32]),
        claimed_at: UnixMillis::from_millis(CLAIMED_AT_MS),
        lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
    }
}

/// Rebuilds an equal claim snapshot; the owner bytes are fixed by
/// [`claim_command`] fixtures, so equality is exact without cloning secrets.
fn replayable_claim(claimed: &ClaimedMessageDispatch) -> ClaimedMessageDispatch {
    ClaimedMessageDispatch {
        message_id: claimed.message_id.clone(),
        correlation_id: claimed.correlation_id.clone(),
        attempt_count: claimed.attempt_count,
        queued_at: claimed.queued_at,
        available_at: claimed.available_at,
        owner: DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
        lease_expires_at: claimed.lease_expires_at,
        updated_at: claimed.updated_at,
    }
}

async fn seed_accepted_message(repository: &Repository) {
    repository
        .queue_first_message(queue_input())
        .await
        .expect("fixture first message should queue");
}

async fn claim_live_dispatch(repository: &Repository) -> ClaimedMessageDispatch {
    seed_accepted_message(repository).await;
    let claimed = repository
        .claim_next_message_dispatch(claim_command(DISPATCH_OWNER_BYTE))
        .await
        .expect("claim should succeed")
        .expect("queued dispatch should be claimable");
    assert_eq!(claimed.attempt_count, 1);
    assert_eq!(claimed.updated_at, UnixMillis::from_millis(CLAIMED_AT_MS));
    claimed
}

struct LaunchIdentityFixture {
    run: RunId,
    turn: TurnId,
    item: ItemId,
    first_patch: PatchId,
    second_patch: PatchId,
}

fn launch_identity() -> LaunchIdentityFixture {
    LaunchIdentityFixture {
        run: RunId::parse(RUN_ID).expect("fixture run id"),
        turn: TurnId::parse(TURN_ID).expect("fixture turn id"),
        item: ItemId::parse(ITEM_ID).expect("fixture item id"),
        first_patch: PatchId::parse(FIRST_PATCH_ID).expect("fixture patch id"),
        second_patch: PatchId::parse(SECOND_PATCH_ID).expect("fixture patch id"),
    }
}

/// Lifetime-owned start key and capabilities so every borrowed launch
/// command outlives its call.
struct LaunchContext {
    start_key: RunStartKey,
    credentials: RunLaunchCredentials,
}

impl LaunchContext {
    fn fixture() -> Self {
        Self {
            start_key: RunStartKey::new(START_KEY_BYTES),
            credentials: RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES),
        }
    }
}

fn launch_command<'a>(
    claimed: &'a ClaimedMessageDispatch,
    identity: &'a LaunchIdentityFixture,
    context: &'a LaunchContext,
) -> LaunchClaimedRun<'a> {
    LaunchClaimedRun {
        claimed,
        run_id: &identity.run,
        turn_id: &identity.turn,
        item_id: &identity.item,
        first_patch_id: &identity.first_patch,
        second_patch_id: &identity.second_patch,
        operated_at: UnixMillis::from_millis(OPERATED_AT_MS),
        run_start_key: &context.start_key,
        credentials: &context.credentials,
        engine_settings: launch_engine_settings(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GraphCounts {
    runs: i64,
    turns: i64,
    items: i64,
    patches: i64,
    ordinals: i64,
    next_renderer_ordinal: Option<i64>,
    last_patch_sequence: Option<i64>,
    conversation_state_updated_at: Option<i64>,
}

async fn graph_counts(database: &DatabaseConnection) -> GraphCounts {
    async fn count<E>(database: &DatabaseConnection) -> i64
    where
        E: EntityTrait,
    {
        let rows = E::find()
            .all(database)
            .await
            .expect("rows should query")
            .len();
        i64::try_from(rows).expect("row count fits i64")
    }

    let state = entities::conversation_state::Entity::find_by_id(THREAD_ID)
        .one(database)
        .await
        .expect("conversation state should query");
    GraphCounts {
        runs: count::<entities::assistant_run::Entity>(database).await,
        turns: count::<entities::conversation_turn::Entity>(database).await,
        items: count::<entities::conversation_item::Entity>(database).await,
        patches: count::<entities::conversation_patch::Entity>(database).await,
        ordinals: count::<entities::conversation_ordinal::Entity>(database).await,
        next_renderer_ordinal: state.as_ref().map(|row| row.next_renderer_ordinal),
        last_patch_sequence: state.as_ref().map(|row| row.last_patch_sequence),
        conversation_state_updated_at: state.as_ref().map(|row| row.updated_at_ms),
    }
}

/// Complete relevant persisted rows for exact before/after equality.
///
/// Every captured model is compared wholesale — including the dispatch's
/// `last_error`, the immutable accepted message, its original queue receipt,
/// thread recency, project attachment, conversation counters, any existing
/// graph/ordinal/patch rows, and the launching run itself — so a rejected
/// launch must preserve all of them byte-for-byte.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PersistedRows {
    projects: Vec<entities::AttachedProject>,
    threads: Vec<entities::Thread>,
    messages: Vec<entities::Message>,
    receipts: Vec<entities::CommandReceipt>,
    dispatches: Vec<entities::MessageDispatch>,
    states: Vec<entities::ConversationState>,
    ordinals: Vec<entities::ConversationOrdinal>,
    turns: Vec<entities::ConversationTurn>,
    items: Vec<entities::ConversationItem>,
    patches: Vec<entities::ConversationPatch>,
    runs: Vec<entities::AssistantRun>,
}

async fn persisted_rows(database: &DatabaseConnection) -> PersistedRows {
    async fn all<E>(database: &DatabaseConnection) -> Vec<E::Model>
    where
        E: EntityTrait,
    {
        E::find()
            .all(database)
            .await
            .expect("persisted rows should query")
    }

    PersistedRows {
        projects: all::<entities::attached_project::Entity>(database).await,
        threads: all::<entities::thread::Entity>(database).await,
        messages: all::<entities::message::Entity>(database).await,
        receipts: all::<entities::command_receipt::Entity>(database).await,
        dispatches: all::<entities::message_dispatch::Entity>(database).await,
        states: all::<entities::conversation_state::Entity>(database).await,
        ordinals: all::<entities::conversation_ordinal::Entity>(database).await,
        turns: all::<entities::conversation_turn::Entity>(database).await,
        items: all::<entities::conversation_item::Entity>(database).await,
        patches: all::<entities::conversation_patch::Entity>(database).await,
        runs: all::<entities::assistant_run::Entity>(database).await,
    }
}

fn dispatch_row_of<'a>(rows: &'a PersistedRows, message_id: &str) -> &'a entities::MessageDispatch {
    rows.dispatches
        .iter()
        .find(|row| row.message_id == message_id)
        .expect("dispatch row should exist")
}

fn state_counters_of(rows: &PersistedRows) -> Option<(i64, i64)> {
    rows.states
        .first()
        .map(|row| (row.next_renderer_ordinal, row.last_patch_sequence))
}

/// Asserts the fenced first write rolled back: the dispatch is still leased
/// and every persisted row equals its pre-launch snapshot.
async fn assert_rollback_preserved(database: &DatabaseConnection, before: &PersistedRows) {
    let after = persisted_rows(database).await;
    let dispatch = dispatch_row_of(&after, MESSAGE_ID);
    assert_eq!(
        dispatch.state,
        DispatchState::Leased,
        "the fenced first write must roll back"
    );
    assert_eq!(before, &after, "a rejected launch must mutate nothing");
}

#[tokio::test]
async fn started_launch_writes_the_exact_graph_once() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();

    let outcome = repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("fresh launch should succeed");
    let receipt = match outcome {
        LaunchClaimedRunOutcome::Started(receipt) => receipt,
        LaunchClaimedRunOutcome::AlreadyStarted(_) => {
            panic!("a fresh transaction must answer Started")
        }
    };
    assert_eq!(receipt.run_id.as_str(), RUN_ID);
    assert_eq!(receipt.thread_id.as_str(), THREAD_ID);
    assert_eq!(receipt.message_id.as_str(), MESSAGE_ID);
    assert_eq!(receipt.turn_id.as_str(), TURN_ID);
    assert_eq!(receipt.item_id.as_str(), ITEM_ID);
    assert_eq!(receipt.generation, 1);
    assert_eq!(receipt.resulting_cursor.get(), 2);

    assert_launched_dispatch_row(&database).await;
    assert_launched_run_row(&database).await;
    let immutable = database
        .execute_unprepared(
            "UPDATE assistant_runs SET engine_run_config_revision = 2 WHERE run_id = 'run-1'",
        )
        .await;
    assert!(immutable.is_err(), "run engine snapshots must be immutable");
    assert_launched_run_row(&database).await;
    assert_launched_turn_item_patch_rows(&database).await;
    assert_ledger_and_state_advanced(&database).await;
    assert_originals_unchanged(&database).await;
}

#[tokio::test]
async fn launch_fence_rejects_a_changed_thread_configuration_without_writes() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("update-before-launch").expect("request id is valid"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id is valid"),
            precondition: EngineConfigUpdatePrecondition::Exact(
                launch_engine_settings().revision(),
            ),
            config: launch_config_with_profile("profile-launch-updated"),
            accepted_at: UnixMillis::from_millis(OPERATED_AT_MS + 1),
        })
        .await
        .expect("configuration revision should update");
    let before = persisted_rows(&database).await;

    let error = repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect_err("a stale engine snapshot must not launch");
    assert!(matches!(error, RunLaunchError::SnapshotMismatch { .. }));
    assert_rollback_preserved(&database, &before).await;
}

#[tokio::test]
async fn thread_configuration_changes_do_not_mutate_an_existing_run_snapshot() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();
    repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("fresh launch should succeed");
    let original_run = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("run should query")
        .expect("run should exist");
    let original_revision = original_run.engine_run_config_revision;
    let original_blob = original_run.engine_run_config.clone();

    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("update-engine-config").expect("request id is valid"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id is valid"),
            precondition: EngineConfigUpdatePrecondition::Exact(
                launch_engine_settings().revision(),
            ),
            config: launch_config_with_profile("profile-launch-updated"),
            accepted_at: UnixMillis::from_millis(OPERATED_AT_MS + 1),
        })
        .await
        .expect("configuration revision should update");

    let updated_run = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("run should query after thread update")
        .expect("run should remain present after thread update");
    assert_eq!(updated_run.engine_run_config_revision, original_revision);
    assert_eq!(updated_run.engine_run_config, original_blob);

    let replay_claimed = replayable_claim(&claimed);
    let replay = repository
        .launch_claimed_run(launch_command(&replay_claimed, &identity, &context))
        .await
        .expect("the immutable original snapshot should still diagnose an exact replay");
    assert!(matches!(replay, LaunchClaimedRunOutcome::AlreadyStarted(_)));
}

async fn assert_launched_dispatch_row(database: &DatabaseConnection) {
    let dispatch = entities::message_dispatch::Entity::find_by_id(MESSAGE_ID)
        .one(database)
        .await
        .expect("dispatch should query")
        .expect("dispatch should exist");
    assert_eq!(dispatch.state, DispatchState::Running);
    assert_eq!(dispatch.attempt_count, 1);
    assert_eq!(
        dispatch.lease_owner.as_deref(),
        Some("1111111111111111111111111111111111111111111111111111111111111111")
    );
    assert_eq!(dispatch.lease_expires_at_ms, Some(LEASE_EXPIRES_AT_MS));
    assert_eq!(dispatch.updated_at_ms, OPERATED_AT_MS);
}

async fn assert_launched_run_row(database: &DatabaseConnection) {
    let run = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(database)
        .await
        .expect("run should query")
        .expect("run should exist");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Launching);
    assert_eq!(run.generation, 1);
    assert_eq!(run.thread_id, THREAD_ID);
    assert_eq!(run.origin_message_id, MESSAGE_ID);
    assert_eq!(run.origin_turn_id, TURN_ID);
    assert_eq!(run.run_start_key.as_slice(), START_KEY_BYTES);
    assert_eq!(
        run.owner.as_ref().map(entities::OpaqueBytes::as_slice),
        Some(&OWNER_BYTES[..])
    );
    assert_eq!(
        run.lease.as_ref().map(entities::OpaqueBytes::as_slice),
        Some(&LEASE_BYTES[..])
    );
    assert_eq!(
        run.claim_token
            .as_ref()
            .map(entities::OpaqueBytes::as_slice),
        Some(&CLAIM_TOKEN_BYTES[..])
    );
    assert!(run.provider_binding_version.is_none());
    assert!(run.provider_binding.is_none());
    assert!(run.provider_bound_at_ms.is_none());
    assert!(run.error_code.is_none());
    assert!(run.error_message.is_none());
    assert!(run.terminal_at_ms.is_none());
    assert_eq!(run.engine_run_config_version, Some(1));
    assert_eq!(run.engine_run_config_revision, Some(1));
    let thread = entities::thread::Entity::find_by_id(THREAD_ID)
        .one(database)
        .await
        .expect("thread should query")
        .expect("thread should exist");
    assert_eq!(
        run.engine_run_config
            .as_ref()
            .map(entities::OpaqueBytes::as_slice),
        thread
            .engine_run_config
            .as_ref()
            .map(entities::OpaqueBytes::as_slice)
    );
}

async fn assert_launched_turn_item_patch_rows(database: &DatabaseConnection) {
    let turn = entities::conversation_turn::Entity::find_by_id(TURN_ID)
        .one(database)
        .await
        .expect("turn should query")
        .expect("turn should exist");
    assert_eq!(turn.ordinal, 0);
    assert_eq!(turn.revision, 0);
    assert_eq!(turn.lifecycle, entities::EntityLifecycle::Pending);
    assert_eq!(turn.created_at_ms, OPERATED_AT_MS);
    assert_eq!(turn.updated_at_ms, OPERATED_AT_MS);

    let item = entities::conversation_item::Entity::find_by_id(ITEM_ID)
        .one(database)
        .await
        .expect("item should query")
        .expect("item should exist");
    assert_eq!(item.ordinal, 1);
    assert_eq!(item.source_message_id.as_deref(), Some(MESSAGE_ID));
    assert_eq!(item.body, "first durable body");

    let first_patch = entities::conversation_patch::Entity::find_by_id(FIRST_PATCH_ID)
        .one(database)
        .await
        .expect("patch should query")
        .expect("patch should exist");
    assert_eq!(first_patch.sequence, 1);
    assert_eq!(first_patch.ordinal, Some(0));
    let second_patch = entities::conversation_patch::Entity::find_by_id(SECOND_PATCH_ID)
        .one(database)
        .await
        .expect("patch should query")
        .expect("patch should exist");
    assert_eq!(second_patch.sequence, 2);
    assert_eq!(second_patch.body.as_deref(), Some("first durable body"));
}

async fn assert_ledger_and_state_advanced(database: &DatabaseConnection) {
    let turn_ledger = entities::conversation_ordinal::Entity::find_by_id((THREAD_ID.to_owned(), 0))
        .one(database)
        .await
        .expect("ordinal ledger should query")
        .expect("turn ordinal should exist");
    assert_eq!(turn_ledger.entity_id, TURN_ID);
    let item_ledger = entities::conversation_ordinal::Entity::find_by_id((THREAD_ID.to_owned(), 1))
        .one(database)
        .await
        .expect("ordinal ledger should query")
        .expect("item ordinal should exist");
    assert_eq!(item_ledger.entity_id, ITEM_ID);

    let state = entities::conversation_state::Entity::find_by_id(THREAD_ID)
        .one(database)
        .await
        .expect("state should query")
        .expect("state should exist");
    assert_eq!(state.next_renderer_ordinal, 2);
    assert_eq!(state.last_patch_sequence, 2);
    assert_eq!(state.updated_at_ms, OPERATED_AT_MS);
}

async fn assert_originals_unchanged(database: &DatabaseConnection) {
    let message = entities::message::Entity::find_by_id(MESSAGE_ID)
        .one(database)
        .await
        .expect("message should query")
        .expect("message should exist");
    assert_eq!(message.body, "first durable body");
    assert_eq!(message.ordinal, 0);
    assert_eq!(message.accepted_at_ms, ACCEPTED_AT_MS);
    let thread = entities::thread::Entity::find_by_id(THREAD_ID)
        .one(database)
        .await
        .expect("thread should query")
        .expect("thread should exist");
    assert_eq!(thread.updated_at_ms, ACCEPTED_AT_MS);
    let receipt_row = entities::command_receipt::Entity::find_by_id(CORRELATION_ID)
        .one(database)
        .await
        .expect("receipt should query")
        .expect("original receipt should persist unchanged");
    assert_eq!(receipt_row.message_id.as_deref(), Some(MESSAGE_ID));
}

#[tokio::test]
async fn exact_replay_answers_already_started_without_mutations() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();
    let started = repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("fresh launch should succeed");
    let expected_receipt = match started {
        LaunchClaimedRunOutcome::Started(receipt) => receipt,
        LaunchClaimedRunOutcome::AlreadyStarted(_) => panic!("unexpected duplicate start"),
    };

    let replay_claimed = replayable_claim(&claimed);
    let before = persisted_rows(&database).await;
    let replay = repository
        .launch_claimed_run(launch_command(&replay_claimed, &identity, &context))
        .await
        .expect("exact replay should diagnose cleanly");
    match replay {
        LaunchClaimedRunOutcome::AlreadyStarted(receipt) => {
            assert_eq!(receipt, expected_receipt);
        }
        LaunchClaimedRunOutcome::Started(_) => {
            panic!("AlreadyStarted must never carry provider authority")
        }
    }
    let after = persisted_rows(&database).await;
    assert_eq!(
        before, after,
        "an exact replay must mutate no persisted row"
    );
}

#[tokio::test]
async fn replay_cursor_derives_from_original_patches_not_current_counters() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();
    repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("fresh launch should succeed");

    // Unrelated later work legitimately advances the shared counters.
    let mut state = entities::conversation_state::Entity::find_by_id(THREAD_ID)
        .one(&database)
        .await
        .expect("state should query")
        .expect("state should exist")
        .into_active_model();
    state.next_renderer_ordinal = Set(9);
    state.last_patch_sequence = Set(9);
    state.updated_at_ms = Set(OPERATED_AT_MS + 5);
    state
        .update(&database)
        .await
        .expect("counter drift fixture");

    let replay_claimed = replayable_claim(&claimed);
    let replay = repository
        .launch_claimed_run(launch_command(&replay_claimed, &identity, &context))
        .await
        .expect("exact replay should diagnose cleanly");
    match replay {
        LaunchClaimedRunOutcome::AlreadyStarted(receipt) => {
            assert_eq!(
                receipt.resulting_cursor.get(),
                2,
                "the initial cursor must come from the original second patch sequence"
            );
        }
        LaunchClaimedRunOutcome::Started(_) => panic!("unexpected duplicate start"),
    }
}

#[tokio::test]
async fn replay_patch_sequence_at_the_i64_boundary_is_a_typed_conflict() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();
    repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("fresh launch should succeed");

    // A first patch at i64::MAX has no successor: checked adjacency must
    // reject rather than saturate or wrap into acceptance.
    let mut first_patch = entities::conversation_patch::Entity::find_by_id(FIRST_PATCH_ID)
        .one(&database)
        .await
        .expect("first patch should query")
        .expect("first patch should exist")
        .into_active_model();
    first_patch.sequence = Set(i64::MAX);
    first_patch
        .update(&database)
        .await
        .expect("sequence boundary fixture");

    let replay_claimed = replayable_claim(&claimed);
    let before = persisted_rows(&database).await;
    let error = repository
        .launch_claimed_run(launch_command(&replay_claimed, &identity, &context))
        .await
        .expect_err("a boundary sequence contradicts the recorded launch");
    assert!(matches!(error, RunLaunchError::IdentityConflict { .. }));
    let after = persisted_rows(&database).await;
    assert_eq!(
        before, after,
        "a rejected replay must mutate no persisted row"
    );
}

#[tokio::test]
async fn replay_with_a_different_operated_time_is_a_typed_mismatch() {
    let (_database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();
    repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("fresh launch should succeed");

    let replay_claimed = replayable_claim(&claimed);
    let drifted_launch = LaunchClaimedRun {
        operated_at: UnixMillis::from_millis(OPERATED_AT_MS + 1),
        ..launch_command(&replay_claimed, &identity, &context)
    };
    let error = repository
        .launch_claimed_run(drifted_launch)
        .await
        .expect_err("a different operation time cannot be an exact replay");
    assert!(matches!(error, RunLaunchError::SnapshotMismatch { .. }));
}

#[tokio::test]
async fn conflicting_replay_credentials_and_ids_are_typed_conflicts() {
    let (_database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();
    repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("fresh launch should succeed");
    let replay_claimed = replayable_claim(&claimed);

    let stranger_run = RunId::parse("run-2").expect("alternate run id");
    let other_run_error = repository
        .launch_claimed_run(LaunchClaimedRun {
            run_id: &stranger_run,
            ..launch_command(&replay_claimed, &identity, &context)
        })
        .await
        .expect_err("a different run id conflicts with the stored origin");
    assert!(matches!(
        other_run_error,
        RunLaunchError::IdentityConflict { .. }
    ));

    let foreign_context = LaunchContext {
        start_key: RunStartKey::new([0xe5; 32]),
        ..LaunchContext::fixture()
    };
    let wrong_key_error = repository
        .launch_claimed_run(launch_command(&replay_claimed, &identity, &foreign_context))
        .await
        .expect_err("a different start key cannot authenticate a replay");
    assert!(matches!(
        wrong_key_error,
        RunLaunchError::CredentialMismatch { .. }
    ));

    for tokens in [
        RunLaunchCredentials::new([0x01; 32], LEASE_BYTES, CLAIM_TOKEN_BYTES),
        RunLaunchCredentials::new(OWNER_BYTES, [0x02; 32], CLAIM_TOKEN_BYTES),
        RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, [0x03; 32]),
    ] {
        let token_context = LaunchContext {
            credentials: tokens,
            ..LaunchContext::fixture()
        };
        let token_error = repository
            .launch_claimed_run(launch_command(&replay_claimed, &identity, &token_context))
            .await
            .expect_err("any mismatching capability rejects the replay");
        assert!(matches!(
            token_error,
            RunLaunchError::CredentialMismatch { .. }
        ));
    }
}

#[tokio::test]
async fn advanced_runs_are_never_launchable_again() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();
    repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("fresh launch should succeed");

    let mut run = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("run should query")
        .expect("run should exist")
        .into_active_model();
    run.lifecycle = Set(AssistantRunLifecycle::Running);
    run.claim_token = Set(None);
    run.provider_binding_version = Set(Some(1));
    run.provider_binding = Set(Some(entities::OpaqueBytes::new(vec![7; 8])));
    run.provider_bound_at_ms = Set(Some(OPERATED_AT_MS + 10));
    run.updated_at_ms = Set(OPERATED_AT_MS + 10);
    run.update(&database)
        .await
        .expect("advanced run fixture should update");

    let error = repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect_err("an advanced run must not be relaunchable");
    assert!(matches!(error, RunLaunchError::RunNotLaunchable { .. }));
}

#[tokio::test]
async fn stale_or_foreign_claims_reject_without_writes() {
    let (database, repository) = seeded_repository().await;
    let live = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();

    let mut wrong_attempt = replayable_claim(&live);
    wrong_attempt.attempt_count = 2;
    let attempt_error = repository
        .launch_claimed_run(launch_command(&wrong_attempt, &identity, &context))
        .await
        .expect_err("a stale attempt count cannot launch");
    assert!(matches!(
        attempt_error,
        RunLaunchError::SnapshotMismatch { .. }
    ));

    let mut stale_stamp = replayable_claim(&live);
    stale_stamp.updated_at = UnixMillis::from_millis(CLAIMED_AT_MS - 1);
    let stamp_error = repository
        .launch_claimed_run(launch_command(&stale_stamp, &identity, &context))
        .await
        .expect_err("a stale update stamp cannot launch");
    assert!(matches!(
        stamp_error,
        RunLaunchError::SnapshotMismatch { .. }
    ));

    let foreign = ClaimMessageDispatch {
        owner: DispatchLeaseOwner::new([0x22; 32]),
        claimed_at: UnixMillis::from_millis(CLAIMED_AT_MS),
        lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
    };
    let mut foreign_claim = replayable_claim(&live);
    foreign_claim.owner = foreign.owner;
    let foreign_error = repository
        .launch_claimed_run(launch_command(&foreign_claim, &identity, &context))
        .await
        .expect_err("a different lease owner cannot launch someone else's claim");
    assert!(matches!(
        foreign_error,
        RunLaunchError::Repository(RepositoryError::DispatchOwnerMismatch { .. })
    ));

    let mut wrong_expiry = replayable_claim(&live);
    wrong_expiry.lease_expires_at = UnixMillis::from_millis(LEASE_EXPIRES_AT_MS + 1);
    let expiry_error = repository
        .launch_claimed_run(launch_command(&wrong_expiry, &identity, &context))
        .await
        .expect_err("a mismatching lease expiry cannot launch");
    assert!(matches!(
        expiry_error,
        RunLaunchError::SnapshotMismatch { .. }
    ));

    let after = persisted_rows(&database).await;
    let dispatch = dispatch_row_of(&after, MESSAGE_ID);
    assert_eq!(dispatch.state, DispatchState::Leased);
    assert_eq!(dispatch.attempt_count, 1);
    assert_eq!(dispatch.last_error.as_deref(), None);
    assert_eq!(after.runs.len(), 0);
    assert_eq!(after.turns.len(), 0);
    assert_eq!(after.items.len(), 0);
    assert_eq!(after.patches.len(), 0);
    assert_eq!(after.ordinals.len(), 0);
    assert_eq!(after.states.len(), 0);
}

#[tokio::test]
async fn expired_equality_is_dead_for_launch() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();

    let before = persisted_rows(&database).await;
    let at_expiry = LaunchClaimedRun {
        operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        ..launch_command(&claimed, &identity, &context)
    };
    let error = repository
        .launch_claimed_run(at_expiry)
        .await
        .expect_err("expiry equality leaves no live lease");
    assert!(matches!(
        error,
        RunLaunchError::Repository(RepositoryError::DispatchLeaseExpired { .. })
    ));
    assert_rollback_preserved(&database, &before).await;
}

#[tokio::test]
async fn chronology_violation_rolls_back_the_fenced_write() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();

    let before = persisted_rows(&database).await;
    let early = LaunchClaimedRun {
        operated_at: UnixMillis::from_millis(ACCEPTED_AT_MS - 1),
        ..launch_command(&claimed, &identity, &context)
    };
    let error = repository
        .launch_claimed_run(early)
        .await
        .expect_err("operating before the accepted message is invalid");
    assert!(matches!(
        error,
        RunLaunchError::Repository(RepositoryError::InvalidChronology { .. })
    ));
    assert_rollback_preserved(&database, &before).await;
}

#[tokio::test]
async fn conversation_state_chronology_rolls_back_the_fenced_write() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    entities::conversation_state::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        next_renderer_ordinal: Set(0),
        last_patch_sequence: Set(0),
        updated_at_ms: Set(OPERATED_AT_MS + 350),
    }
    .insert(&database)
    .await
    .expect("future-stamped conversation state fixture should insert");
    let before = persisted_rows(&database).await;

    // This rejection fires AFTER the fenced write, proving the fence itself
    // rolls back when a later prerequisite fails.
    let error = repository
        .launch_claimed_run(launch_command(
            &claimed,
            &launch_identity(),
            &LaunchContext::fixture(),
        ))
        .await
        .expect_err("operating before the conversation-state stamp is invalid");
    assert!(matches!(
        error,
        RunLaunchError::Repository(RepositoryError::InvalidChronology { .. })
    ));
    assert_rollback_preserved(&database, &before).await;
}

#[tokio::test]
async fn unknown_dispatch_is_not_found() {
    let (_database, repository) = seeded_repository().await;
    let context = LaunchContext::fixture();

    let unknown = ClaimedMessageDispatch {
        message_id: MessageId::parse("message-missing").expect("unknown message id"),
        correlation_id: RequestId::parse("request-missing").expect("unknown request id"),
        attempt_count: 1,
        queued_at: UnixMillis::from_millis(ACCEPTED_AT_MS),
        available_at: UnixMillis::from_millis(ACCEPTED_AT_MS),
        owner: DispatchLeaseOwner::new([0x33; 32]),
        lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        updated_at: UnixMillis::from_millis(CLAIMED_AT_MS),
    };
    let error = repository
        .launch_claimed_run(launch_command(&unknown, &launch_identity(), &context))
        .await
        .expect_err("an unknown dispatch cannot launch");
    assert!(matches!(
        error,
        RunLaunchError::Repository(RepositoryError::DispatchNotFound { .. })
    ));
}

#[tokio::test]
async fn counter_overflow_boundaries_roll_back_everything() {
    for (column, next_value, last_value) in [
        ("next_renderer_ordinal", i64::MAX - 1, 0),
        ("last_patch_sequence", 0, i64::MAX),
    ] {
        let (database, repository) = seeded_repository().await;
        let claimed = claim_live_dispatch(&repository).await;
        entities::conversation_state::ActiveModel {
            thread_id: Set(THREAD_ID.to_owned()),
            next_renderer_ordinal: Set(next_value),
            last_patch_sequence: Set(last_value),
            updated_at_ms: Set(CLAIMED_AT_MS - 10),
        }
        .insert(&database)
        .await
        .expect("boundary conversation state should insert");
        let before = persisted_rows(&database).await;

        let error = repository
            .launch_claimed_run(launch_command(
                &claimed,
                &launch_identity(),
                &LaunchContext::fixture(),
            ))
            .await
            .expect_err("insufficient counter range must refuse the launch");
        assert!(
            matches!(error, RunLaunchError::CounterOverflow { .. }),
            "{column} boundary should overflow"
        );

        let after = persisted_rows(&database).await;
        let dispatch = dispatch_row_of(&after, MESSAGE_ID);
        assert_eq!(dispatch.state, DispatchState::Leased);
        assert_eq!(
            state_counters_of(&after),
            Some((next_value, last_value)),
            "{column} boundary must leave counters untouched"
        );
        assert_eq!(before, after);
    }
}

#[tokio::test]
async fn fresh_identity_collisions_roll_back_the_fence() {
    for (label, colliding_entity) in [
        ("turn ordinal entity", TURN_ID),
        ("item ordinal entity", ITEM_ID),
    ] {
        let (database, repository) = seeded_repository().await;
        let claimed = claim_live_dispatch(&repository).await;
        entities::conversation_ordinal::ActiveModel {
            thread_id: Set(THREAD_ID.to_owned()),
            ordinal: Set(9),
            kind: Set(entities::OrdinalKind::Item),
            entity_id: Set(colliding_entity.to_owned()),
        }
        .insert(&database)
        .await
        .expect("colliding ordinal fixture should insert");

        // Full-row snapshot AFTER the fixture so before/after equality
        // preserves the seeded collision plus every original row.
        let before = persisted_rows(&database).await;
        let error = repository
            .launch_claimed_run(launch_command(
                &claimed,
                &launch_identity(),
                &LaunchContext::fixture(),
            ))
            .await
            .expect_err("an occupied ordinal entity identity must refuse the launch");
        assert!(
            matches!(error, RunLaunchError::IdentityConflict { .. }),
            "{label} collision should be typed"
        );
        let after = persisted_rows(&database).await;
        let dispatch = dispatch_row_of(&after, MESSAGE_ID);
        assert_eq!(dispatch.state, DispatchState::Leased);
        assert_eq!(before, after, "{label} collision must mutate nothing");
    }
}

#[tokio::test]
async fn same_call_identity_collisions_reject_before_any_write() {
    let (database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let context = LaunchContext::fixture();

    // One launch whose turn and item identities collide with each other.
    let colliding_turn_item = LaunchIdentityFixture {
        item: ItemId::parse(TURN_ID).expect("colliding item identity"),
        ..launch_identity()
    };
    let before = persisted_rows(&database).await;
    let turn_item_error = repository
        .launch_claimed_run(LaunchClaimedRun {
            item_id: &colliding_turn_item.item,
            ..launch_command(&claimed, &colliding_turn_item, &context)
        })
        .await
        .expect_err("same-call turn/item identity collision must be typed");
    assert!(matches!(
        turn_item_error,
        RunLaunchError::IdentityConflict { .. }
    ));
    assert_rollback_preserved(&database, &before).await;

    // One launch whose two initial patch identities collide with each other.
    let colliding_patches = LaunchIdentityFixture {
        second_patch: PatchId::parse(FIRST_PATCH_ID).expect("colliding patch identity"),
        ..launch_identity()
    };
    let patch_error = repository
        .launch_claimed_run(launch_command(&claimed, &colliding_patches, &context))
        .await
        .expect_err("same-call patch identity collision must be typed");
    assert!(matches!(
        patch_error,
        RunLaunchError::IdentityConflict { .. }
    ));
    assert_rollback_preserved(&database, &before).await;
}

#[tokio::test]
async fn first_generation_is_one_even_when_the_attempt_exceeds_one() {
    let (database, repository) = seeded_repository().await;
    seed_accepted_message(&repository).await;

    // Real multi-attempt history through the public claim/requeue flow:
    // claim (attempt 1) -> requeue under the live lease -> claim again.
    let first_claim = repository
        .claim_next_message_dispatch(claim_command(DISPATCH_OWNER_BYTE))
        .await
        .expect("first claim should succeed")
        .expect("dispatch should be claimable");
    assert_eq!(first_claim.attempt_count, 1);

    repository
        .requeue_message_dispatch(RequeueMessageDispatch {
            message_id: MessageId::parse(MESSAGE_ID).expect("message id"),
            owner: first_claim.owner,
            operated_at: UnixMillis::from_millis(120),
            available_at: UnixMillis::from_millis(120),
            reason: DispatchFailureReason::parse("fixture retry").expect("bounded reason"),
        })
        .await
        .expect("fixture requeue should succeed");

    let second_claim = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            claimed_at: UnixMillis::from_millis(130),
            lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        })
        .await
        .expect("second claim should succeed")
        .expect("requeued dispatch should be claimable");
    assert_eq!(second_claim.attempt_count, 2);
    assert_eq!(second_claim.updated_at, UnixMillis::from_millis(130));

    let outcome = repository
        .launch_claimed_run(launch_command(
            &second_claim,
            &launch_identity(),
            &LaunchContext::fixture(),
        ))
        .await
        .expect("launch should succeed over a genuine attempt-2 lease");
    match outcome {
        LaunchClaimedRunOutcome::Started(receipt) => {
            assert_eq!(receipt.generation, 1);
        }
        LaunchClaimedRunOutcome::AlreadyStarted(_) => panic!("unexpected duplicate start"),
    }
    let run = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("run should query")
        .expect("run should exist");
    assert_eq!(run.generation, 1);
    let dispatch = entities::message_dispatch::Entity::find_by_id(MESSAGE_ID)
        .one(&database)
        .await
        .expect("dispatch should query")
        .expect("dispatch should exist");
    assert_eq!(dispatch.state, DispatchState::Running);
    assert_eq!(dispatch.attempt_count, 2);
}

/// One isolated file-backed fixture per race: its own temporary database,
/// seeded project/thread/queued first message with a live claim, and two
/// independent contender pools. Nothing is shared between races.
struct RaceEnvironment {
    _temp: TempDatabase,
    claimed: ClaimedMessageDispatch,
    database_a: DatabaseConnection,
    repository_a: Repository,
    repository_b: Repository,
}

async fn race_environment(label: &str) -> RaceEnvironment {
    let temp = TempDatabase::new(label);
    let database_path = temp.database().to_path_buf();
    let setup = connect(
        SqliteConfig::file(&database_path)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("setup database should open");
    migrate_to_current(&setup)
        .await
        .expect("setup database should migrate");
    let setup_repository = Repository::new(setup.clone());
    seed_project_and_thread(&setup, &setup_repository).await;
    setup_repository
        .queue_first_message(queue_input())
        .await
        .expect("fixture first message should queue");
    let claimed = setup_repository
        .claim_next_message_dispatch(claim_command(DISPATCH_OWNER_BYTE))
        .await
        .expect("claim should succeed")
        .expect("dispatch should be claimable");

    let database_a = connect(
        SqliteConfig::file(&database_path)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("contender a should open");
    let database_b = connect(
        SqliteConfig::file(&database_path)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("contender b should open");
    RaceEnvironment {
        _temp: temp,
        claimed,
        database_a: database_a.clone(),
        repository_a: Repository::new(database_a),
        repository_b: Repository::new(database_b),
    }
}

#[tokio::test]
async fn concurrent_same_and_different_starts_commit_one_graph_each() {
    // SAME-identity race in its own fresh file-backed fixture: two identical
    // launches contend; exactly one commits its graph and the other answers
    // AlreadyStarted or a typed busy failure.
    let env_same = race_environment("run-launch-race-same").await;
    let snapshot_same_a = replayable_claim(&env_same.claimed);
    let snapshot_same_b = replayable_claim(&env_same.claimed);
    let identity_same_a = launch_identity();
    let identity_same_b = launch_identity();
    let context_same_a = LaunchContext::fixture();
    let context_same_b = LaunchContext::fixture();
    let (same_outcome_a, same_outcome_b) = tokio::join!(
        env_same.repository_a.launch_claimed_run(launch_command(
            &snapshot_same_a,
            &identity_same_a,
            &context_same_a
        )),
        env_same.repository_b.launch_claimed_run(launch_command(
            &snapshot_same_b,
            &identity_same_b,
            &context_same_b
        ))
    );

    let same_outcomes = [&same_outcome_a, &same_outcome_b];
    let same_started = same_outcomes
        .iter()
        .filter(|result| matches!(result, Ok(LaunchClaimedRunOutcome::Started(_))))
        .count();
    assert_eq!(
        same_started, 1,
        "exactly one same-start racer may commit; outcomes were {same_outcome_a:?} and {same_outcome_b:?}"
    );
    for result in same_outcomes {
        match result {
            Ok(
                LaunchClaimedRunOutcome::AlreadyStarted(_) | LaunchClaimedRunOutcome::Started(_),
            )
            | Err(RunLaunchError::Repository(RepositoryError::Database { .. })) => {}
            Err(other) => panic!("unexpected same-start failure: {other:?}"),
        }
    }

    let counts = graph_counts(&env_same.database_a).await;
    assert_eq!(counts.runs, 1);
    assert_eq!(counts.turns, 1);
    assert_eq!(counts.items, 1);
    assert_eq!(counts.patches, 2);
    assert_eq!(counts.ordinals, 2);
    assert_eq!(counts.next_renderer_ordinal, Some(2));
    assert_eq!(counts.last_patch_sequence, Some(2));
}

#[tokio::test]
async fn concurrent_different_starts_commit_only_one_graph() {
    // DIFFERENT-identity race in its own fresh file-backed fixture: distinct
    // run, turn, item, patch identities plus distinct start key and
    // credentials contend for one writer position. At most one commits.
    let env = race_environment("run-launch-race-different").await;

    let snapshot_a = replayable_claim(&env.claimed);
    let snapshot_b = replayable_claim(&env.claimed);
    let identity_a = launch_identity();
    let identity_b = LaunchIdentityFixture {
        run: RunId::parse("run-2").expect("alternate run id"),
        turn: TurnId::parse("turn-2").expect("alternate turn id"),
        item: ItemId::parse("item-2").expect("alternate item id"),
        first_patch: PatchId::parse("patch-c").expect("alternate patch id"),
        second_patch: PatchId::parse("patch-d").expect("alternate patch id"),
    };
    let context_a = LaunchContext::fixture();
    let context_b = LaunchContext {
        start_key: RunStartKey::new([0xe5; 32]),
        credentials: RunLaunchCredentials::new([0x01; 32], [0x02; 32], [0x03; 32]),
    };

    let (outcome_a, outcome_b) = tokio::join!(
        env.repository_a
            .launch_claimed_run(launch_command(&snapshot_a, &identity_a, &context_a)),
        env.repository_b
            .launch_claimed_run(launch_command(&snapshot_b, &identity_b, &context_b))
    );

    let outcomes = [&outcome_a, &outcome_b];
    let started_count = outcomes
        .iter()
        .filter(|result| matches!(result, Ok(LaunchClaimedRunOutcome::Started(_))))
        .count();
    assert_eq!(
        started_count, 1,
        "two different-start racers may never both commit; outcomes were {outcome_a:?} and {outcome_b:?}"
    );
    for result in outcomes {
        match result {
            Ok(_)
            | Err(
                RunLaunchError::IdentityConflict { .. }
                | RunLaunchError::Repository(RepositoryError::Database { .. }),
            ) => {}
            Err(other) => panic!("unexpected different-start failure: {other:?}"),
        }
    }

    let counts = graph_counts(&env.database_a).await;
    assert_eq!(counts.runs, 1);
    assert_eq!(counts.turns, 1);
    assert_eq!(counts.items, 1);
    assert_eq!(counts.patches, 2);
    assert_eq!(counts.ordinals, 2);
    assert_eq!(counts.next_renderer_ordinal, Some(2));
    assert_eq!(counts.last_patch_sequence, Some(2));
    let dispatch = entities::message_dispatch::Entity::find_by_id(MESSAGE_ID)
        .one(&env.database_a)
        .await
        .expect("dispatch should query")
        .expect("dispatch should exist");
    assert_eq!(dispatch.state, DispatchState::Running);
}

async fn open_migrated_file_database(path: &Path) -> DatabaseConnection {
    let database = connect(
        SqliteConfig::file(path)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("file database should open");
    migrate_to_current(&database)
        .await
        .expect("file database should migrate");
    database
}

/// Seeds a project/thread/first message and drives it through a real claim
/// and launch, returning the exact claimed snapshot for later replays.
async fn seed_running_launch(
    database: &DatabaseConnection,
    repository: &Repository,
) -> ClaimedMessageDispatch {
    seed_project_and_thread(database, repository).await;
    repository
        .queue_first_message(queue_input())
        .await
        .expect("fixture first message should queue");
    let claimed = repository
        .claim_next_message_dispatch(claim_command(DISPATCH_OWNER_BYTE))
        .await
        .expect("claim should succeed")
        .expect("dispatch should be claimable");
    let context = LaunchContext::fixture();
    repository
        .launch_claimed_run(launch_command(&claimed, &launch_identity(), &context))
        .await
        .expect("launch should succeed");
    claimed
}

async fn assert_running_dispatch_refuses_legacy_transitions(
    repository: &Repository,
    operated_at_ms: i64,
) {
    let completion = repository
        .complete_message_dispatch(CompleteMessageDispatch {
            message_id: MessageId::parse(MESSAGE_ID).expect("message id"),
            owner: DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            operated_at: UnixMillis::from_millis(operated_at_ms),
        })
        .await
        .expect_err("existing completion refuses RUNNING dispatches");
    assert!(matches!(
        completion,
        RepositoryError::InvalidDispatchState {
            state: "running",
            ..
        }
    ));

    let failure = repository
        .fail_message_dispatch(FailMessageDispatch {
            message_id: MessageId::parse(MESSAGE_ID).expect("message id"),
            owner: DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            operated_at: UnixMillis::from_millis(operated_at_ms),
            reason: DispatchFailureReason::parse("unused reason").expect("bounded reason"),
        })
        .await
        .expect_err("existing failure refuses RUNNING dispatches");
    assert!(matches!(
        failure,
        RepositoryError::InvalidDispatchState {
            state: "running",
            ..
        }
    ));

    let requeue = repository
        .requeue_message_dispatch(RequeueMessageDispatch {
            message_id: MessageId::parse(MESSAGE_ID).expect("message id"),
            owner: DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            operated_at: UnixMillis::from_millis(operated_at_ms),
            available_at: UnixMillis::from_millis(operated_at_ms),
            reason: DispatchFailureReason::parse("unused reason").expect("bounded reason"),
        })
        .await
        .expect_err("existing requeue refuses RUNNING dispatches");
    assert!(matches!(
        requeue,
        RepositoryError::InvalidDispatchState {
            state: "running",
            ..
        }
    ));
}

#[tokio::test]
async fn reopen_persists_running_and_claim_stays_excluded() {
    let temp = TempDatabase::new("run-launch-reopen");
    let database = open_migrated_file_database(temp.database()).await;
    let repository = Repository::new(database.clone());
    let claimed = seed_running_launch(&database, &repository).await;
    database.close().await.expect("database should close");

    let reopened = open_migrated_file_database(temp.database()).await;
    let repository = Repository::new(reopened.clone());

    let run = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&reopened)
        .await
        .expect("run should query after reopen")
        .expect("launched run should persist across reopen");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Launching);
    assert_eq!(run.generation, 1);

    let identity = launch_identity();
    let context = LaunchContext::fixture();
    let replay_claimed = replayable_claim(&claimed);
    let replay_after_reopen = repository
        .launch_claimed_run(launch_command(&replay_claimed, &identity, &context))
        .await
        .expect("exact replay should diagnose cleanly after reopen");
    match replay_after_reopen {
        LaunchClaimedRunOutcome::AlreadyStarted(receipt) => {
            assert_eq!(receipt.resulting_cursor.get(), 2);
        }
        LaunchClaimedRunOutcome::Started(_) => panic!("unexpected duplicate start"),
    }

    let dispatch = entities::message_dispatch::Entity::find_by_id(MESSAGE_ID)
        .one(&reopened)
        .await
        .expect("dispatch should query after reopen")
        .expect("dispatch should exist");
    assert_eq!(dispatch.state, DispatchState::Running);

    let far_future = ClaimMessageDispatch {
        owner: DispatchLeaseOwner::new([0x44; 32]),
        claimed_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS * 10),
        lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS * 20),
    };
    let reclaimed = repository
        .claim_next_message_dispatch(far_future)
        .await
        .expect("future claim should succeed");
    assert!(
        reclaimed.is_none(),
        "RUNNING dispatches are never reclaimable by expiry"
    );

    assert_running_dispatch_refuses_legacy_transitions(&repository, LEASE_EXPIRES_AT_MS * 10).await;
}

#[tokio::test]
async fn rendered_launch_errors_never_disclose_capabilities() {
    let (_database, repository) = seeded_repository().await;
    let claimed = claim_live_dispatch(&repository).await;
    let identity = launch_identity();
    let context = LaunchContext::fixture();
    repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("fresh launch should succeed");

    let foreign_context = LaunchContext {
        start_key: RunStartKey::new([0xe5; 32]),
        ..LaunchContext::fixture()
    };
    let replay_claimed = replayable_claim(&claimed);
    let key_error = repository
        .launch_claimed_run(launch_command(&replay_claimed, &identity, &foreign_context))
        .await
        .expect_err("mismatched keys are credential failures");

    let rendered_display = key_error.to_string();
    let rendered_debug = format!("{key_error:?}");
    for secret_pattern in ["b2b2b2", "c3c3c3", "d4d4d4", "a1a1a1"] {
        assert!(!rendered_display.contains(secret_pattern));
        assert!(!rendered_debug.contains(secret_pattern));
    }
}
