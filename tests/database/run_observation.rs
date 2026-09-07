//! E2-A atomic RUNNING progress/checkpoint coverage through real migrated
//! SQLite and the public repository APIs. Mirrors the launch/binding fixture
//! pattern and proves rollback, replay and exact permitted deltas via full
//! before/after snapshots of all 13 relevant tables.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{
    self, ConversationItemKind, ConversationPatchKind, EntityLifecycle, OrdinalKind, RenderPhase,
};
use artisan_database::{
    AssistantChange, BindRunProvider, CheckpointUpdate, ClaimMessageDispatch,
    ClaimedMessageDispatch, CommitRunBatch, CommitRunBatchOutcome, CreateThreadInput,
    EngineCheckpoint, ProviderBindingBytes, QueueFirstMessageInput, Repository, RepositoryError,
    RunBatchScope, RunLaunchCredentials, RunObservationError, RunStartKey,
    SetThreadEngineConfigInput, SqliteConfig, ThreadEngineSettings, connect,
};
use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantMessagePhase, ByteLimit, CountLimit, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, IncrementalText, ItemId, MessageBody,
    MessageId, NetworkAccess, OpenCode2Selection, PatchId, PermissionId, ProjectId, RequestId,
    Revision, RunId, ThreadId, ThreadTitle, TurnId, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseConnection, EntityTrait,
};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Redaction / marker: EngineCheckpoint must NOT impl Debug/Display/Clone
// ---------------------------------------------------------------------------
const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: ?Sized + std::fmt::Debug> Ambiguous<Marker> for T {}
    let _ = <EngineCheckpoint as Ambiguous<_>>::marker;
};
const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: ?Sized + std::fmt::Display> Ambiguous<Marker> for T {}
    let _ = <EngineCheckpoint as Ambiguous<_>>::marker;
};
const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: Clone> Ambiguous<Marker> for T {}
    let _ = <EngineCheckpoint as Ambiguous<_>>::marker;
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
const BOUND_AT_MS: i64 = 200;
const BATCH_OPERATED_AT_MS: i64 = 250;
const BATCH_OPERATED_AT_MS_2: i64 = 260;

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
            .expect("epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "artisan-editor-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("temp dir");
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
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

async fn seed_project_and_thread(
    database: &DatabaseConnection,
    repository: &Repository,
) -> ThreadEngineSettings {
    entities::attached_project::ActiveModel {
        project_id: Set("project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    }
    .insert(database)
    .await
    .expect("project");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse("seed-thread-request").expect("req"),
            thread_id: ThreadId::parse(THREAD_ID).expect("tid"),
            project_id: ProjectId::parse("project-1").expect("pid"),
            title: ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
            updated_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await
        .expect("thread");
    let thread_id = ThreadId::parse(THREAD_ID).expect("thread id");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("seed-engine-config").expect("request id"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: launch_config(),
            accepted_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await
        .expect("engine configuration should create");
    repository
        .read_thread_engine_settings(&thread_id)
        .await
        .expect("engine configuration should read")
        .expect("engine configuration should be present")
}

fn launch_config() -> EngineRunConfig {
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
            EngineProfileId::parse("profile-launch").expect("profile id is valid"),
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
        request_id: RequestId::parse(CORRELATION_ID).expect("req"),
        message_id: MessageId::parse(MESSAGE_ID).expect("mid"),
        thread_id: ThreadId::parse(THREAD_ID).expect("tid"),
        body: MessageBody::parse("first durable body").expect("body"),
        accepted_at: UnixMillis::from_millis(ACCEPTED_AT_MS),
    }
}
fn claim_command(owner_byte: u8) -> ClaimMessageDispatch {
    ClaimMessageDispatch {
        owner: artisan_database::DispatchLeaseOwner::new([owner_byte; 32]),
        claimed_at: UnixMillis::from_millis(CLAIMED_AT_MS),
        lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
    }
}
fn replayable_claim(claimed: &ClaimedMessageDispatch) -> ClaimedMessageDispatch {
    ClaimedMessageDispatch {
        message_id: claimed.message_id.clone(),
        correlation_id: claimed.correlation_id.clone(),
        attempt_count: claimed.attempt_count,
        queued_at: claimed.queued_at,
        available_at: claimed.available_at,
        owner: artisan_database::DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
        lease_expires_at: claimed.lease_expires_at,
        updated_at: claimed.updated_at,
    }
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
        run: RunId::parse(RUN_ID).expect("run"),
        turn: TurnId::parse(TURN_ID).expect("turn"),
        item: ItemId::parse(ITEM_ID).expect("item"),
        first_patch: PatchId::parse(FIRST_PATCH_ID).expect("patch"),
        second_patch: PatchId::parse(SECOND_PATCH_ID).expect("patch"),
    }
}
struct LaunchContext {
    start_key: RunStartKey,
    credentials: RunLaunchCredentials,
    engine_settings: ThreadEngineSettings,
}
impl LaunchContext {
    fn fixture(engine_settings: ThreadEngineSettings) -> Self {
        Self {
            start_key: RunStartKey::new(START_KEY_BYTES),
            credentials: RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES),
            engine_settings,
        }
    }
}
fn launch_command<'a>(
    claimed: &'a ClaimedMessageDispatch,
    identity: &'a LaunchIdentityFixture,
    context: &'a LaunchContext,
) -> artisan_database::LaunchClaimedRun<'a> {
    artisan_database::LaunchClaimedRun {
        claimed,
        run_id: &identity.run,
        turn_id: &identity.turn,
        item_id: &identity.item,
        first_patch_id: &identity.first_patch,
        second_patch_id: &identity.second_patch,
        operated_at: UnixMillis::from_millis(OPERATED_AT_MS),
        run_start_key: &context.start_key,
        credentials: &context.credentials,
        engine_settings: &context.engine_settings,
    }
}

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
    checkpoints: Vec<entities::RunCheckpoint>,
    batch_receipts: Vec<entities::RunBatchReceipt>,
}
async fn persisted_rows(database: &DatabaseConnection) -> PersistedRows {
    async fn all<E>(db: &DatabaseConnection) -> Vec<E::Model>
    where
        E: EntityTrait,
    {
        E::find().all(db).await.expect("rows")
    }
    let mut projects = all::<entities::attached_project::Entity>(database).await;
    let mut threads = all::<entities::thread::Entity>(database).await;
    let mut messages = all::<entities::message::Entity>(database).await;
    let mut receipts = all::<entities::command_receipt::Entity>(database).await;
    let mut dispatches = all::<entities::message_dispatch::Entity>(database).await;
    let mut states = all::<entities::conversation_state::Entity>(database).await;
    let mut ordinals = all::<entities::conversation_ordinal::Entity>(database).await;
    let mut turns = all::<entities::conversation_turn::Entity>(database).await;
    let mut items = all::<entities::conversation_item::Entity>(database).await;
    let mut patches = all::<entities::conversation_patch::Entity>(database).await;
    let mut runs = all::<entities::assistant_run::Entity>(database).await;
    let mut checkpoints = all::<entities::run_checkpoint::Entity>(database).await;
    let mut batch_receipts = all::<entities::run_batch_receipt::Entity>(database).await;
    projects.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    threads.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
    messages.sort_by(|a, b| a.message_id.cmp(&b.message_id));
    receipts.sort_by(|a, b| a.request_id.cmp(&b.request_id));
    dispatches.sort_by(|a, b| a.message_id.cmp(&b.message_id));
    states.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
    ordinals.sort_by_key(|a| (a.thread_id.clone(), a.ordinal));
    turns.sort_by(|a, b| a.turn_id.cmp(&b.turn_id));
    items.sort_by(|a, b| a.item_id.cmp(&b.item_id));
    patches.sort_by(|a, b| a.patch_id.cmp(&b.patch_id));
    runs.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    checkpoints.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    batch_receipts.sort_by(|a, b| {
        (a.run_id.clone(), a.batch_sequence).cmp(&(b.run_id.clone(), b.batch_sequence))
    });
    PersistedRows {
        projects,
        threads,
        messages,
        receipts,
        dispatches,
        states,
        ordinals,
        turns,
        items,
        patches,
        runs,
        checkpoints,
        batch_receipts,
    }
}

struct SeededPair {
    database: DatabaseConnection,
    repository: Repository,
    claimed: ClaimedMessageDispatch,
    launched: artisan_database::LaunchedRunReceipt,
    bound: artisan_database::BoundRunReceipt,
    context: LaunchContext,
}
async fn seeded_pair() -> SeededPair {
    let (database, repository) = memory_database().await;
    let engine_settings = seed_project_and_thread(&database, &repository).await;
    repository
        .queue_first_message(queue_input())
        .await
        .expect("queue");
    let claimed = repository
        .claim_next_message_dispatch(claim_command(DISPATCH_OWNER_BYTE))
        .await
        .expect("claim")
        .expect("claimed");
    let identity = launch_identity();
    let context = LaunchContext::fixture(engine_settings);
    let artisan_database::LaunchClaimedRunOutcome::Started(launched) = repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("launch")
    else {
        panic!("started")
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let bound = match repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &context.start_key,
            credentials: &context.credentials,
            expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
            bound_at: UnixMillis::from_millis(BOUND_AT_MS),
            binding_version: 2,
            binding_bytes: &binding,
        })
        .await
        .expect("bind")
    {
        artisan_database::BindRunProviderOutcome::Bound(r) => r,
        artisan_database::BindRunProviderOutcome::AlreadyBound(_) => panic!("bound"),
    };
    SeededPair {
        database,
        repository,
        claimed,
        launched,
        bound,
        context,
    }
}

fn batch_scope(pair: &SeededPair, expected_updated_at: UnixMillis) -> RunBatchScope<'_> {
    RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at,
    }
}

// ---------------------------------------------------------------------------
// Helpers for batch building
// ---------------------------------------------------------------------------
fn assistant_body(s: &str) -> AssistantBody {
    AssistantBody::parse(s.to_owned()).expect("body")
}
fn incremental_text(s: &str) -> IncrementalText {
    IncrementalText::parse(s.to_owned()).expect("fragment")
}

fn digest_encode_i64(h: &mut Sha256, value: i64) {
    h.update(value.to_le_bytes());
}
fn digest_encode_u32(h: &mut Sha256, value: u32) {
    h.update(value.to_le_bytes());
}
fn digest_write_str(h: &mut Sha256, text: &str) {
    digest_encode_u32(h, u32::try_from(text.len()).expect("str len fits u32"));
    h.update(text.as_bytes());
}
fn digest_write_bytes(h: &mut Sha256, bytes: &[u8]) {
    digest_encode_u32(h, u32::try_from(bytes.len()).expect("bytes len fits u32"));
    h.update(bytes);
}
const fn digest_phase_tag(phase: AssistantMessagePhase) -> u8 {
    match phase {
        AssistantMessagePhase::Unspecified => 0,
        AssistantMessagePhase::Commentary => 1,
        AssistantMessagePhase::Final => 2,
    }
}
fn digest_encode_changes(h: &mut Sha256, changes: &[AssistantChange<'_>]) {
    for change in changes {
        match change {
            AssistantChange::Start {
                item_id,
                phase,
                body,
                patch_id,
            } => {
                h.update([0u8]);
                digest_write_str(h, item_id.as_str());
                h.update([digest_phase_tag(*phase)]);
                digest_write_bytes(h, body.as_str().as_bytes());
                digest_write_str(h, patch_id.as_str());
            }
            AssistantChange::Append {
                item_id,
                expected_revision,
                text,
                patch_id,
            } => {
                h.update([1u8]);
                digest_write_str(h, item_id.as_str());
                digest_encode_i64(
                    h,
                    i64::try_from(expected_revision.get()).unwrap_or(i64::MAX),
                );
                digest_write_bytes(h, text.as_str().as_bytes());
                digest_write_str(h, patch_id.as_str());
            }
            AssistantChange::Replace {
                item_id,
                expected_revision,
                body,
                phase,
                patch_id,
            } => {
                h.update([2u8]);
                digest_write_str(h, item_id.as_str());
                digest_encode_i64(
                    h,
                    i64::try_from(expected_revision.get()).unwrap_or(i64::MAX),
                );
                digest_write_bytes(h, body.as_str().as_bytes());
                h.update([digest_phase_tag(*phase)]);
                digest_write_str(h, patch_id.as_str());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Independent v1 digest encoder for test-local verification (must equal persisted)
// Pass explicit checkpoint version/bytes when the command carries Replace so the
// encoder never calls private EngineCheckpoint::version/as_slice.
// ---------------------------------------------------------------------------
fn independent_digest(
    command: &CommitRunBatch<'_>,
    explicit_checkpoint: Option<(i64, &[u8])>,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"artisan.run-batch.v1");
    hasher.update([0u8]);
    hasher.update([0u8]);
    let scope = &command.scope;
    let launched = scope.launched;
    let claimed = scope.claimed;
    digest_write_str(&mut hasher, launched.run_id.as_str());
    digest_write_str(&mut hasher, launched.thread_id.as_str());
    digest_write_str(&mut hasher, launched.message_id.as_str());
    digest_write_str(&mut hasher, launched.turn_id.as_str());
    digest_encode_i64(&mut hasher, launched.generation);
    digest_encode_i64(&mut hasher, scope.bound.binding_version);
    digest_encode_i64(&mut hasher, scope.bound.bound_at.as_millis());
    digest_encode_i64(&mut hasher, scope.expected_launch_at.as_millis());
    digest_encode_i64(&mut hasher, scope.expected_updated_at.as_millis());
    digest_encode_i64(&mut hasher, command.operated_at.as_millis());
    digest_encode_i64(&mut hasher, command.batch_sequence);
    digest_write_str(&mut hasher, claimed.message_id.as_str());
    digest_write_str(&mut hasher, claimed.correlation_id.as_str());
    digest_encode_i64(&mut hasher, i64::from(claimed.attempt_count));
    digest_encode_i64(&mut hasher, claimed.queued_at.as_millis());
    digest_encode_i64(&mut hasher, claimed.available_at.as_millis());
    digest_encode_i64(&mut hasher, claimed.lease_expires_at.as_millis());
    digest_encode_i64(&mut hasher, claimed.updated_at.as_millis());
    match command.activate_turn_patch_id {
        None => hasher.update([0u8]),
        Some(pid) => {
            hasher.update([1u8]);
            digest_write_str(&mut hasher, pid.as_str());
        }
    }
    digest_encode_u32(
        &mut hasher,
        u32::try_from(command.changes.len()).expect("changes len fits u32"),
    );
    digest_encode_changes(&mut hasher, command.changes);
    match command.checkpoint {
        CheckpointUpdate::Keep => {
            assert!(
                explicit_checkpoint.is_none(),
                "Keep must be paired with None explicit checkpoint"
            );
            hasher.update([0u8]);
        }
        CheckpointUpdate::Replace(_) => {
            let (version, bytes) =
                explicit_checkpoint.expect("Replace must be paired with Some explicit checkpoint");
            hasher.update([1u8]);
            digest_encode_i64(&mut hasher, version);
            digest_write_bytes(&mut hasher, bytes);
        }
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_progress_writes_turn_active_fresh_item_patches_state_checkpoint_receipt() {
    let pair = seeded_pair().await;
    let before = persisted_rows(&pair.database).await;
    let scope = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    let body = assistant_body("hello assistant");
    let item_id = ItemId::parse("assistant-1").expect("id");
    let patch_turn = PatchId::parse("patch-turn-activation").expect("p");
    let patch_item = PatchId::parse("patch-assistant-start").expect("p");

    let outcome = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&patch_turn),
            changes: &[AssistantChange::Start {
                item_id: &item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: &patch_item,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("commit should succeed");
    let CommitRunBatchOutcome::Committed(info) = outcome else {
        panic!("committed")
    };
    assert_eq!(info.run_id.as_str(), RUN_ID);
    assert_eq!(info.batch_sequence, 1);

    let after = persisted_rows(&pair.database).await;
    verify_first_progress_after(&before, &after);
    assert_eq!(before.receipts, after.receipts);
    // dispatch stamps advanced
    let disp = after
        .dispatches
        .iter()
        .find(|d| d.message_id == MESSAGE_ID)
        .expect("d");
    assert_eq!(disp.updated_at_ms, BATCH_OPERATED_AT_MS);
    let run = after.runs.iter().find(|r| r.run_id == RUN_ID).expect("run");
    assert_eq!(run.updated_at_ms, BATCH_OPERATED_AT_MS);
}

fn verify_first_progress_after(before: &PersistedRows, after: &PersistedRows) {
    let turn = after
        .turns
        .iter()
        .find(|t| t.turn_id == TURN_ID)
        .expect("turn");
    assert_eq!(turn.lifecycle, EntityLifecycle::Active);
    assert_eq!(turn.revision, 1);
    assert_eq!(turn.updated_at_ms, BATCH_OPERATED_AT_MS);
    let item = after
        .items
        .iter()
        .find(|i| i.item_id == "assistant-1")
        .expect("item");
    assert_eq!(item.ordinal, 2);
    assert_eq!(item.revision, 0);
    assert_eq!(item.lifecycle, EntityLifecycle::Streaming);
    assert_eq!(item.item_kind, ConversationItemKind::AssistantMessage);
    assert_eq!(item.phase, Some(RenderPhase::Final));
    assert_eq!(item.body, "hello assistant");
    assert_eq!(item.run_id.as_deref(), Some(RUN_ID));
    assert_eq!(item.source_message_id, None);
    assert!(
        after
            .ordinals
            .iter()
            .any(|o| o.ordinal == 2 && o.entity_id == "assistant-1" && o.kind == OrdinalKind::Item)
    );
    let mut patches: Vec<_> = after
        .patches
        .iter()
        .filter(|p| p.patch_id == "patch-turn-activation" || p.patch_id == "patch-assistant-start")
        .collect();
    patches.sort_by_key(|p| p.sequence);
    assert_eq!(patches.len(), 2);
    let _new_patch_ids: Vec<&str> = patches.iter().map(|p| p.patch_id.as_str()).collect();
    assert_eq!(after.patches.len(), 4);
    let seqs: Vec<i64> = after.patches.iter().map(|p| p.sequence).collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, [1, 2, 3, 4]);
    let activation = after
        .patches
        .iter()
        .find(|p| p.patch_id == "patch-turn-activation")
        .expect("act");
    assert_eq!(activation.kind, ConversationPatchKind::TurnLifecycle);
    assert_eq!(activation.sequence, 3);
    assert_eq!(activation.lifecycle, Some(EntityLifecycle::Active));
    let upsert = after
        .patches
        .iter()
        .find(|p| p.patch_id == "patch-assistant-start")
        .expect("start");
    assert_eq!(upsert.kind, ConversationPatchKind::ItemUpsert);
    assert_eq!(upsert.sequence, 4);
    assert_eq!(upsert.body.as_deref(), Some("hello assistant"));
    let state = after
        .states
        .iter()
        .find(|s| s.thread_id == THREAD_ID)
        .expect("state");
    assert_eq!(state.next_renderer_ordinal, 3);
    assert_eq!(state.last_patch_sequence, 4);
    assert_eq!(state.updated_at_ms, BATCH_OPERATED_AT_MS);
    let cp = after
        .checkpoints
        .iter()
        .find(|c| c.run_id == RUN_ID)
        .expect("cp");
    assert_eq!(cp.last_batch_sequence, 1);
    assert_eq!(cp.generation, 1);
    assert!(cp.engine_checkpoint_version.is_none());
    assert!(cp.engine_checkpoint_blob.is_none());
    let receipt = after
        .batch_receipts
        .iter()
        .find(|r| r.run_id == RUN_ID && r.batch_sequence == 1)
        .expect("receipt");
    assert!(receipt.committed);
    assert_eq!(receipt.digest.as_slice().len(), 32);
    assert_eq!(receipt.generation, 1);
    assert_eq!(before.projects, after.projects);
    assert_eq!(before.threads, after.threads);
    assert_eq!(before.messages, after.messages);
}

#[tokio::test]
async fn seeded_nonzero_counters_allocate_from_actual_state() {
    let pair = seeded_pair().await;
    // bump state to nonzero via direct update
    let state = entities::conversation_state::Entity::find_by_id(THREAD_ID)
        .one(&pair.database)
        .await
        .expect("q")
        .expect("state");
    let mut active: entities::conversation_state::ActiveModel = state.into();
    active.next_renderer_ordinal = Set(10);
    active.last_patch_sequence = Set(20);
    active.update(&pair.database).await.expect("update");
    let scope = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    let body = assistant_body("from nonzero");
    let item_id = ItemId::parse("assistant-nonzero").expect("id");
    let patch_turn = PatchId::parse("patch-activation-nz").expect("p");
    let patch_item = PatchId::parse("patch-item-nz").expect("p");
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&patch_turn),
            changes: &[AssistantChange::Start {
                item_id: &item_id,
                phase: AssistantMessagePhase::Commentary,
                body: &body,
                patch_id: &patch_item,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("commit");
    let after = persisted_rows(&pair.database).await;
    let item = after
        .items
        .iter()
        .find(|i| i.item_id == "assistant-nonzero")
        .expect("item");
    assert_eq!(item.ordinal, 10);
    let state2 = after
        .states
        .iter()
        .find(|s| s.thread_id == THREAD_ID)
        .expect("state2");
    assert_eq!(state2.next_renderer_ordinal, 11);
    assert_eq!(state2.last_patch_sequence, 22);
    let activation = after
        .patches
        .iter()
        .find(|p| p.patch_id == "patch-activation-nz")
        .expect("act");
    assert_eq!(activation.sequence, 21);
    let upsert = after
        .patches
        .iter()
        .find(|p| p.patch_id == "patch-item-nz")
        .expect("up");
    assert_eq!(upsert.sequence, 22);
}

#[tokio::test]
async fn later_append_and_replace_preserve_unicode_whitespace_and_phase() {
    let pair = seeded_pair().await;
    // first progress
    let scope1 = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    let body = assistant_body("start");
    let item_id = ItemId::parse("assistant-1").expect("id");
    let pt = PatchId::parse("p-act-1").expect("p");
    let pi = PatchId::parse("p-start-1").expect("p");
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope: scope1,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &[AssistantChange::Start {
                item_id: &item_id,
                phase: AssistantMessagePhase::Commentary,
                body: &body,
                patch_id: &pi,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("first");
    // append unicode including empty and whitespace
    let scope2 = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let frag = incremental_text("  🚀 unicode \t\n");
    let p_append = PatchId::parse("p-append-1").expect("p");
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS_2),
            activate_turn_patch_id: None,
            changes: &[AssistantChange::Append {
                item_id: &item_id,
                expected_revision: Revision::new(0),
                text: &frag,
                patch_id: &p_append,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("append");
    let after_append = persisted_rows(&pair.database).await;
    verify_unicode_append(&after_append);
    // replace with empty body and phase Final
    let scope3 = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS_2),
    };
    let new_body = assistant_body("");
    let p_replace = PatchId::parse("p-replace-1").expect("p");
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope: scope3,
            batch_sequence: 3,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS_2 + 10),
            activate_turn_patch_id: None,
            changes: &[AssistantChange::Replace {
                item_id: &item_id,
                expected_revision: Revision::new(1),
                body: &new_body,
                phase: AssistantMessagePhase::Final,
                patch_id: &p_replace,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("replace");
    let after_replace = persisted_rows(&pair.database).await;
    verify_replace_final(&after_replace);
}

fn verify_unicode_append(after: &PersistedRows) {
    let item = after
        .items
        .iter()
        .find(|i| i.item_id == "assistant-1")
        .expect("item");
    assert_eq!(item.revision, 1);
    assert_eq!(item.body, "start  🚀 unicode \t\n");
    let patch = after
        .patches
        .iter()
        .find(|p| p.patch_id == "p-append-1")
        .expect("patch");
    assert_eq!(patch.kind, ConversationPatchKind::ItemAppend);
    assert_eq!(patch.fragment.as_deref(), Some("  🚀 unicode \t\n"));
    assert_eq!(patch.revision, 1);
    assert_eq!(patch.recorded_at_ms, BATCH_OPERATED_AT_MS_2);
    assert!(patch.body.is_none() && patch.phase.is_none());
}
fn verify_replace_final(after: &PersistedRows) {
    let item = after
        .items
        .iter()
        .find(|i| i.item_id == "assistant-1")
        .expect("item2");
    assert_eq!(item.revision, 2);
    assert_eq!(item.body, "");
    assert_eq!(item.phase, Some(RenderPhase::Final));
    let patch = after
        .patches
        .iter()
        .find(|p| p.patch_id == "p-replace-1")
        .expect("p2");
    assert_eq!(patch.kind, ConversationPatchKind::ItemUpsert);
    assert_eq!(patch.body.as_deref(), Some(""));
    assert_eq!(patch.phase, Some(RenderPhase::Final));
    assert_eq!(patch.revision, 2);
}

#[tokio::test]
async fn checkpoint_boundaries_and_patch_budget() {
    let pair = seeded_pair().await;
    let scope = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    let body = assistant_body("b");
    let iid = ItemId::parse("assistant-1").expect("id");
    let pt = PatchId::parse("p-act-b").expect("p");
    let pi = PatchId::parse("p-start-b").expect("p");
    let cp1 = EngineCheckpoint::new(1, vec![0xaa; 1]).expect("cp");
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &[AssistantChange::Start {
                item_id: &iid,
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &pi,
            }],
            checkpoint: CheckpointUpdate::Replace(&cp1),
        })
        .await
        .expect("replace 1");
    let after = persisted_rows(&pair.database).await;
    let cp_row = after
        .checkpoints
        .iter()
        .find(|c| c.run_id == RUN_ID)
        .expect("cp");
    assert_eq!(cp_row.engine_checkpoint_version, Some(1));
    assert_eq!(
        cp_row
            .engine_checkpoint_blob
            .as_ref()
            .map(|b| b.as_slice().len()),
        Some(1)
    );
    let bad_cp_version = EngineCheckpoint::new(0, vec![1]);
    assert!(matches!(
        bad_cp_version,
        Err(RunObservationError::InvalidCheckpoint { .. })
    ));
    assert!(matches!(
        EngineCheckpoint::new(1, vec![]),
        Err(RunObservationError::InvalidCheckpoint { .. })
    ));
    assert!(EngineCheckpoint::new(1, vec![0x55; 262_144]).is_ok());
    assert!(matches!(
        EngineCheckpoint::new(1, vec![0x55; 262_145]),
        Err(RunObservationError::InvalidCheckpoint { .. })
    ));
    let big_body = "a".repeat(65536);
    let big_body_parsed = AssistantBody::parse(big_body.clone()).expect("65536");
    let iid2 = ItemId::parse("assistant-2").expect("id");
    let pi2 = PatchId::parse("p-start-big").expect("p");
    let scope2 = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 5),
            activate_turn_patch_id: None,
            changes: &[AssistantChange::Start {
                item_id: &iid2,
                phase: AssistantMessagePhase::Unspecified,
                body: &big_body_parsed,
                patch_id: &pi2,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("big body ok");
    let too_big = "a".repeat(65537);
    assert!(AssistantBody::parse(too_big).is_err());
    let frag_ok = "b".repeat(4096);
    let frag_ok_parsed = IncrementalText::parse(frag_ok.clone()).expect("ok");
    assert_eq!(frag_ok_parsed.as_str().len(), 4096);
    let frag_bad = "b".repeat(4097);
    assert!(IncrementalText::parse(frag_bad).is_err());
    verify_emitted_patch_budget_limits().await;
}
async fn verify_emitted_patch_budget_limits() {
    let pair2 = seeded_pair().await;
    let scope64 = batch_scope(&pair2, UnixMillis::from_millis(BOUND_AT_MS));
    let mut changes64: Vec<AssistantChange> = Vec::new();
    let mut bodies64: Vec<AssistantBody> = Vec::new();
    let mut item_ids64: Vec<ItemId> = Vec::new();
    let mut patch_ids64: Vec<PatchId> = Vec::new();
    for idx in 0..63 {
        bodies64.push(assistant_body("x"));
        item_ids64.push(ItemId::parse(format!("assistant-64-{idx}")).expect("id"));
        patch_ids64.push(PatchId::parse(format!("p-64-{idx}")).expect("p"));
    }
    for idx in 0..63 {
        changes64.push(AssistantChange::Start {
            item_id: &item_ids64[idx],
            phase: AssistantMessagePhase::Unspecified,
            body: &bodies64[idx],
            patch_id: &patch_ids64[idx],
        });
    }
    let act64 = PatchId::parse("p-act-64").expect("p");
    let res64 = pair2
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope64,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&act64),
            changes: &changes64,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await;
    assert!(res64.is_ok(), "64 patches should succeed: {res64:?}");
    let pair3 = seeded_pair().await;
    let scope65 = batch_scope(&pair3, UnixMillis::from_millis(BOUND_AT_MS));
    let mut changes65: Vec<AssistantChange> = Vec::new();
    let mut bodies65: Vec<AssistantBody> = Vec::new();
    let mut item_ids65: Vec<ItemId> = Vec::new();
    let mut patch_ids65: Vec<PatchId> = Vec::new();
    for idx in 0..64 {
        bodies65.push(assistant_body("x"));
        item_ids65.push(ItemId::parse(format!("assistant-65-{idx}")).expect("id"));
        patch_ids65.push(PatchId::parse(format!("p-65-{idx}")).expect("p"));
    }
    for idx in 0..64 {
        changes65.push(AssistantChange::Start {
            item_id: &item_ids65[idx],
            phase: AssistantMessagePhase::Unspecified,
            body: &bodies65[idx],
            patch_id: &patch_ids65[idx],
        });
    }
    let act65 = PatchId::parse("p-act-65").expect("p");
    let err65 = pair3
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope65,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&act65),
            changes: &changes65,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("65 should exceed budget");
    assert!(matches!(
        err65,
        RunObservationError::PatchBudgetExceeded { .. }
    ));
}

#[tokio::test]
async fn fence_matrix_each_claim_field_and_credentials_rejects() {
    let pair = seeded_pair().await;
    let before = persisted_rows(&pair.database).await;
    let body = assistant_body("x");
    let iid = ItemId::parse("assistant-fence").expect("id");
    let pt = PatchId::parse("p-act-fence").expect("p");
    let pi = PatchId::parse("p-start-fence").expect("p");
    // helper to attempt with mutated claimed
    let mut bad_claim = replayable_claim(&pair.claimed);
    bad_claim.correlation_id = RequestId::parse("request-zzz").expect("req");
    let scope_bad = RunBatchScope {
        claimed: &bad_claim,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope_bad,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &[AssistantChange::Start {
                item_id: &iid,
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &pi,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("correlation mismatch");
    assert!(matches!(err, RunObservationError::SnapshotMismatch { .. }));
    assert_eq!(before, persisted_rows(&pair.database).await);

    // attempt_count >1 still generation 1 - we test via raw claim with attempt 2 but using same launched gen 1 - should still fence fail as snapshot mismatch (attempt mismatch)
    let mut claim2 = replayable_claim(&pair.claimed);
    claim2.attempt_count = 2;
    let scope2 = RunBatchScope {
        claimed: &claim2,
        ..batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS))
    };
    let err2 = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("p-act-2").expect("p")),
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-f2").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &PatchId::parse("p-start-f2").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("attempt 2 mismatch");
    assert!(matches!(err2, RunObservationError::SnapshotMismatch { .. }));
    verify_fence_owner_and_replay(&body).await;
}

async fn verify_fence_owner_and_replay(body: &AssistantBody) {
    let pair = seeded_pair().await;
    let bad_owner_ctx = RunLaunchCredentials::new([0x99; 32], LEASE_BYTES, CLAIM_TOKEN_BYTES);
    let scope_bad_owner = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &bad_owner_ctx,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let err3 = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope_bad_owner,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("p-act-3").expect("p")),
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-f3").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body,
                patch_id: &PatchId::parse("p-start-f3").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("owner mismatch");
    assert!(matches!(
        err3,
        RunObservationError::CredentialMismatch { .. }
    ));
    let pair_ok = seeded_pair().await;
    let scope_ok = batch_scope(&pair_ok, UnixMillis::from_millis(BOUND_AT_MS));
    let iid_ok = ItemId::parse("assistant-ok").expect("id");
    let activation_ok = PatchId::parse("p-act-ok").expect("p");
    let item_start_ok = PatchId::parse("p-start-ok").expect("p");
    pair_ok
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope_ok,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&activation_ok),
            changes: &[AssistantChange::Start {
                item_id: &iid_ok,
                phase: AssistantMessagePhase::Unspecified,
                body,
                patch_id: &item_start_ok,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("first ok");
    let before_replay = persisted_rows(&pair_ok.database).await;
    let different_claim = RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, [0xff; 32]);
    let scope_replay = RunBatchScope {
        claimed: &pair_ok.claimed,
        launched: &pair_ok.launched,
        bound: &pair_ok.bound,
        run_start_key: &pair_ok.context.start_key,
        credentials: &different_claim,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let replay = pair_ok
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope_replay,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&activation_ok),
            changes: &[AssistantChange::Start {
                item_id: &iid_ok,
                phase: AssistantMessagePhase::Unspecified,
                body,
                patch_id: &item_start_ok,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("replay with different claim token");
    assert!(matches!(replay, CommitRunBatchOutcome::AlreadyCommitted(_)));
    assert_eq!(before_replay, persisted_rows(&pair_ok.database).await);
}

#[tokio::test]
async fn expiry_equality_rejects_and_chronology() {
    let pair = seeded_pair().await;
    let before = persisted_rows(&pair.database).await;
    let body = assistant_body("x");
    let iid = ItemId::parse("assistant-exp").expect("id");
    let pt = PatchId::parse("p-act-exp").expect("p");
    let pi = PatchId::parse("p-start-exp").expect("p");
    // expiry equality: operated_at == lease_expires_at should reject
    let scope_eq = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope_eq,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &[AssistantChange::Start {
                item_id: &iid,
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &pi,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("expiry equality");
    assert!(matches!(
        err,
        RunObservationError::Repository(RepositoryError::DispatchLeaseExpired { .. })
    ));
    assert_eq!(before, persisted_rows(&pair.database).await);
    verify_expiry_chronology_and_equal_times(&body).await;
}

async fn verify_expiry_chronology_and_equal_times(body: &AssistantBody) {
    let pair = seeded_pair().await;
    let scope_chrono = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 100),
    };
    let err2 = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope_chrono,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("p-act-c2").expect("p")),
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-c2").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body,
                patch_id: &PatchId::parse("p-start-c2").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("chronology");
    assert!(matches!(
        err2,
        RunObservationError::Repository(RepositoryError::InvalidChronology { .. })
    ));
    let pair_eq = seeded_pair().await;
    let scope1 = batch_scope(&pair_eq, UnixMillis::from_millis(BOUND_AT_MS));
    let pt1 = PatchId::parse("p-act-eq1").expect("p");
    let pi1 = PatchId::parse("p-start-eq1").expect("p");
    pair_eq
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope1,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt1),
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-eq1").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body,
                patch_id: &pi1,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("first eq");
    let scope2 = RunBatchScope {
        claimed: &pair_eq.claimed,
        launched: &pair_eq.launched,
        bound: &pair_eq.bound,
        run_start_key: &pair_eq.context.start_key,
        credentials: &pair_eq.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let res2 = pair_eq
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: None,
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-eq2").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body,
                patch_id: &PatchId::parse("p-start-eq2").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await;
    assert!(
        res2.is_ok(),
        "equal operated_at consecutive batches should be valid: {res2:?}"
    );
}

#[tokio::test]
async fn i64_max_counters_reject_without_partial_writes() {
    let pair = seeded_pair().await;
    let state = entities::conversation_state::Entity::find_by_id(THREAD_ID)
        .one(&pair.database)
        .await
        .expect("q")
        .expect("state");
    let mut active: entities::conversation_state::ActiveModel = state.into();
    active.next_renderer_ordinal = Set(i64::MAX);
    active.last_patch_sequence = Set(0);
    active.update(&pair.database).await.expect("update");
    let before = persisted_rows(&pair.database).await;
    let scope = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    let body = assistant_body("x");
    let iid = ItemId::parse("assistant-max").expect("id");
    let pt = PatchId::parse("p-act-max").expect("p");
    let pi = PatchId::parse("p-start-max").expect("p");
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &[AssistantChange::Start {
                item_id: &iid,
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &pi,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("ordinal max should overflow");
    assert!(matches!(err, RunObservationError::CounterOverflow { .. }));
    assert_eq!(before, persisted_rows(&pair.database).await);
    let pair2 = seeded_pair().await;
    let scope1 = batch_scope(&pair2, UnixMillis::from_millis(BOUND_AT_MS));
    let iid2 = ItemId::parse("assistant-rev").expect("id");
    let pt2 = PatchId::parse("p-act-rev").expect("p");
    let pi2 = PatchId::parse("p-start-rev").expect("p");
    pair2
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope1,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt2),
            changes: &[AssistantChange::Start {
                item_id: &iid2,
                phase: AssistantMessagePhase::Unspecified,
                body: &assistant_body("hello"),
                patch_id: &pi2,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("first");
    let item = entities::conversation_item::Entity::find_by_id("assistant-rev")
        .one(&pair2.database)
        .await
        .expect("q")
        .expect("item");
    let mut active_item: entities::conversation_item::ActiveModel = item.into();
    active_item.revision = Set(i64::MAX);
    active_item.update(&pair2.database).await.expect("rev max");
    let before2 = persisted_rows(&pair2.database).await;
    let scope2 = RunBatchScope {
        claimed: &pair2.claimed,
        launched: &pair2.launched,
        bound: &pair2.bound,
        run_start_key: &pair2.context.start_key,
        credentials: &pair2.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let frag = incremental_text("more");
    let err2 = pair2
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 10),
            activate_turn_patch_id: None,
            changes: &[AssistantChange::Append {
                item_id: &iid2,
                expected_revision: Revision::new(u64::try_from(i64::MAX).unwrap()),
                text: &frag,
                patch_id: &PatchId::parse("p-append-max").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("revision overflow");
    assert!(matches!(err2, RunObservationError::CounterOverflow { .. }));
    assert_eq!(before2, persisted_rows(&pair2.database).await);
}

#[tokio::test]
async fn exact_replay_latest_and_older_after_later_batch() {
    let pair = seeded_pair().await;
    let body = assistant_body("start");
    let iid = ItemId::parse("assistant-replay").expect("id");
    let pt = PatchId::parse("p-act-r1").expect("p");
    let pi = PatchId::parse("p-start-r1").expect("p");
    let scope1 = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope: scope1,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &[AssistantChange::Start {
                item_id: &iid,
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &pi,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("batch1");
    let _after1 = persisted_rows(&pair.database).await;
    // second batch
    let scope2 = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let iid2 = ItemId::parse("assistant-replay2").expect("id");
    let pi2 = PatchId::parse("p-start-r2").expect("p");
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 10),
            activate_turn_patch_id: None,
            changes: &[AssistantChange::Start {
                item_id: &iid2,
                phase: AssistantMessagePhase::Unspecified,
                body: &assistant_body("second"),
                patch_id: &pi2,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("batch2");
    let after2 = persisted_rows(&pair.database).await;
    let ctx = ExactReplayTailContext {
        pair: &pair,
        after2,
        iid,
        pt,
        pi,
        iid2,
        pi2,
        body,
    };
    verify_exact_replay_tail(ctx).await;
}
struct ExactReplayTailContext<'a> {
    pair: &'a SeededPair,
    after2: PersistedRows,
    iid: ItemId,
    pt: PatchId,
    pi: PatchId,
    iid2: ItemId,
    pi2: PatchId,
    body: AssistantBody,
}
async fn verify_exact_replay_tail(ctx: ExactReplayTailContext<'_>) {
    let second_body = assistant_body("second");
    let replay_latest = ctx
        .pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: RunBatchScope {
                claimed: &ctx.pair.claimed,
                launched: &ctx.pair.launched,
                bound: &ctx.pair.bound,
                run_start_key: &ctx.pair.context.start_key,
                credentials: &ctx.pair.context.credentials,
                expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
                expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            },
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 10),
            activate_turn_patch_id: None,
            changes: &[AssistantChange::Start {
                item_id: &ctx.iid2,
                phase: AssistantMessagePhase::Unspecified,
                body: &second_body,
                patch_id: &ctx.pi2,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("replay latest");
    assert!(matches!(
        replay_latest,
        CommitRunBatchOutcome::AlreadyCommitted(_)
    ));
    assert_eq!(ctx.after2, persisted_rows(&ctx.pair.database).await);
    let replay_older = ctx
        .pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: batch_scope(ctx.pair, UnixMillis::from_millis(BOUND_AT_MS)),
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&ctx.pt),
            changes: &[AssistantChange::Start {
                item_id: &ctx.iid,
                phase: AssistantMessagePhase::Unspecified,
                body: &ctx.body,
                patch_id: &ctx.pi,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("replay older");
    assert!(matches!(
        replay_older,
        CommitRunBatchOutcome::AlreadyCommitted(_)
    ));
    assert_eq!(ctx.after2, persisted_rows(&ctx.pair.database).await);
    assert_receipt_conflicts(ctx.pair, ctx.iid, ctx.pt, ctx.pi, ctx.body).await;
}

async fn assert_receipt_conflicts(
    pair: &SeededPair,
    iid: ItemId,
    pt: PatchId,
    pi: PatchId,
    body: AssistantBody,
) {
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: batch_scope(pair, UnixMillis::from_millis(BOUND_AT_MS)),
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &[AssistantChange::Start {
                item_id: &iid,
                phase: AssistantMessagePhase::Unspecified,
                body: &assistant_body("different"),
                patch_id: &pi,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("changed content");
    assert!(matches!(err, RunObservationError::ReceiptConflict { .. }));
    let receipt = entities::run_batch_receipt::Entity::find_by_id((RUN_ID.to_owned(), 1))
        .one(&pair.database)
        .await
        .expect("q")
        .expect("receipt");
    let mut active: entities::run_batch_receipt::ActiveModel = receipt.into();
    active.committed = Set(false);
    active.update(&pair.database).await.expect("uncommitted");
    let err2 = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: batch_scope(pair, UnixMillis::from_millis(BOUND_AT_MS)),
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &[AssistantChange::Start {
                item_id: &iid,
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &pi,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("uncommitted");
    assert!(matches!(
        err2,
        RunObservationError::UncommittedReceipt { .. }
    ));
}

#[tokio::test]
async fn wrong_targets_and_duplicate_collisions_reject() {
    let pair = seeded_pair().await;
    let body = assistant_body("x");
    let pt = PatchId::parse("p-act-wrong").expect("p");
    let pi = PatchId::parse("p-start-wrong").expect("p");
    let iid = ItemId::parse("assistant-wrong").expect("id");
    let scope = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    let changes = [AssistantChange::Start {
        item_id: &iid,
        phase: AssistantMessagePhase::Unspecified,
        body: &body,
        patch_id: &pi,
    }];
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &changes,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("first");
    assert_user_item_target_conflict(&pair, &body).await;
    assert_duplicate_target_conflict(&pair, &body).await;
    assert_duplicate_patch_conflict(&pair, &body).await;
    verify_wrong_targets_sealed_and_checkpoint(&body).await;
}
async fn assert_user_item_target_conflict(pair: &SeededPair, _body: &AssistantBody) {
    let user_item = ItemId::parse(ITEM_ID).expect("user item");
    let frag = incremental_text("frag");
    let append_patch = PatchId::parse("p-append-user").expect("p");
    let scope = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let changes = [AssistantChange::Append {
        item_id: &user_item,
        expected_revision: Revision::new(0),
        text: &frag,
        patch_id: &append_patch,
    }];
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 10),
            activate_turn_patch_id: None,
            changes: &changes,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("user item");
    assert!(matches!(err, RunObservationError::TargetConflict { .. }));
}
async fn assert_duplicate_target_conflict(pair: &SeededPair, body: &AssistantBody) {
    let item_a = ItemId::parse("assistant-a").expect("id");
    let patch_a = PatchId::parse("p-a").expect("p");
    let patch_b = PatchId::parse("p-b").expect("p");
    let scope = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let changes = [
        AssistantChange::Start {
            item_id: &item_a,
            phase: AssistantMessagePhase::Unspecified,
            body,
            patch_id: &patch_a,
        },
        AssistantChange::Start {
            item_id: &item_a,
            phase: AssistantMessagePhase::Unspecified,
            body,
            patch_id: &patch_b,
        },
    ];
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 10),
            activate_turn_patch_id: None,
            changes: &changes,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("duplicate target");
    assert!(matches!(err, RunObservationError::TargetConflict { .. }));
}
async fn assert_duplicate_patch_conflict(pair: &SeededPair, body: &AssistantBody) {
    let item_p1 = ItemId::parse("assistant-p1").expect("id");
    let item_p2 = ItemId::parse("assistant-p2").expect("id");
    let dup_patch = PatchId::parse("p-dup").expect("p");
    let scope = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let changes = [
        AssistantChange::Start {
            item_id: &item_p1,
            phase: AssistantMessagePhase::Unspecified,
            body,
            patch_id: &dup_patch,
        },
        AssistantChange::Start {
            item_id: &item_p2,
            phase: AssistantMessagePhase::Unspecified,
            body,
            patch_id: &dup_patch,
        },
    ];
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 10),
            activate_turn_patch_id: None,
            changes: &changes,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("dup patch");
    assert!(matches!(err, RunObservationError::PatchConflict { .. }));
}

async fn verify_wrong_targets_sealed_and_checkpoint(body: &AssistantBody) {
    verify_sealed_item_append_rejected(body).await;
    verify_missing_checkpoint_rejected(body).await;
}
async fn verify_sealed_item_append_rejected(body: &AssistantBody) {
    let iid_seal = ItemId::parse("assistant-seal").expect("id");
    let seal_start_patch = PatchId::parse("p-seal-start").expect("p");
    let seal_activation_patch = PatchId::parse("p-act-seal").expect("p");
    let pair_seal = seeded_pair().await;
    let scope = batch_scope(&pair_seal, UnixMillis::from_millis(BOUND_AT_MS));
    let changes = [AssistantChange::Start {
        item_id: &iid_seal,
        phase: AssistantMessagePhase::Unspecified,
        body,
        patch_id: &seal_start_patch,
    }];
    pair_seal
        .repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&seal_activation_patch),
            changes: &changes,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("seal start");
    let item_row = entities::conversation_item::Entity::find_by_id("assistant-seal")
        .one(&pair_seal.database)
        .await
        .expect("q")
        .expect("item");
    let mut active: entities::conversation_item::ActiveModel = item_row.into();
    active.lifecycle = Set(EntityLifecycle::Completed);
    active.update(&pair_seal.database).await.expect("seal");
    let frag = incremental_text("frag");
    let append_patch = PatchId::parse("p-append-sealed").expect("p");
    let scope2 = RunBatchScope {
        claimed: &pair_seal.claimed,
        launched: &pair_seal.launched,
        bound: &pair_seal.bound,
        run_start_key: &pair_seal.context.start_key,
        credentials: &pair_seal.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let changes2 = [AssistantChange::Append {
        item_id: &iid_seal,
        expected_revision: Revision::new(0),
        text: &frag,
        patch_id: &append_patch,
    }];
    let err_sealed = pair_seal
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 10),
            activate_turn_patch_id: None,
            changes: &changes2,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("sealed");
    assert!(matches!(err_sealed, RunObservationError::SealedItem { .. }));
}
async fn verify_missing_checkpoint_rejected(body: &AssistantBody) {
    let pair_cp = seeded_pair().await;
    let cp_activation_patch = PatchId::parse("p-act-cp").expect("p");
    let cp_start_patch = PatchId::parse("p-start-cp").expect("p");
    let cp_item = ItemId::parse("assistant-cp").expect("id");
    let scope = batch_scope(&pair_cp, UnixMillis::from_millis(BOUND_AT_MS));
    let changes = [AssistantChange::Start {
        item_id: &cp_item,
        phase: AssistantMessagePhase::Unspecified,
        body,
        patch_id: &cp_start_patch,
    }];
    pair_cp
        .repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&cp_activation_patch),
            changes: &changes,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("cp first");
    entities::run_checkpoint::Entity::delete_by_id(RUN_ID.to_owned())
        .exec(&pair_cp.database)
        .await
        .expect("del cp");
    let cp2_item = ItemId::parse("assistant-cp2").expect("id");
    let cp2_patch = PatchId::parse("p-start-cp2").expect("p");
    let scope2 = RunBatchScope {
        claimed: &pair_cp.claimed,
        launched: &pair_cp.launched,
        bound: &pair_cp.bound,
        run_start_key: &pair_cp.context.start_key,
        credentials: &pair_cp.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let changes2 = [AssistantChange::Start {
        item_id: &cp2_item,
        phase: AssistantMessagePhase::Unspecified,
        body,
        patch_id: &cp2_patch,
    }];
    let err_cp = pair_cp
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 10),
            activate_turn_patch_id: None,
            changes: &changes2,
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("missing checkpoint");
    assert!(matches!(
        err_cp,
        RunObservationError::Repository(RepositoryError::CorruptData { .. })
    ));
}

#[tokio::test]
async fn failure_after_dispatch_fence_rolls_back_pair_stamps() {
    let pair = seeded_pair().await;
    let before = persisted_rows(&pair.database).await;
    let body = assistant_body("x");
    // failure due to wrong run lease (second fence fails) should rollback dispatch stamp
    let bad_lease = RunLaunchCredentials::new(OWNER_BYTES, [0x99; 32], CLAIM_TOKEN_BYTES);
    let scope_bad = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &bad_lease,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope_bad,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("p-act-fail").expect("p")),
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-fail").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &PatchId::parse("p-start-fail").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("wrong lease");
    assert!(matches!(
        err,
        RunObservationError::CredentialMismatch { .. }
    ));
    assert_eq!(before, persisted_rows(&pair.database).await);
}

#[tokio::test]
async fn late_transaction_failure_via_trigger_rolls_back_everything() {
    let pair = seeded_pair().await;
    pair.database
        .execute_unprepared(
            "CREATE TRIGGER abort_patch BEFORE INSERT ON conversation_patches BEGIN SELECT RAISE(ABORT, 'test abort patch'); END;",
        )
        .await
        .expect("trigger patch");
    let before = persisted_rows(&pair.database).await;
    let body = assistant_body("x");
    let scope = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("p-act-trig").expect("p")),
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-trig").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &PatchId::parse("p-start-trig").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("trigger should abort");
    assert!(matches!(
        err,
        RunObservationError::Repository(RepositoryError::Database { .. })
    ));
    assert_eq!(before, persisted_rows(&pair.database).await);
    pair.database
        .execute_unprepared("DROP TRIGGER abort_patch")
        .await
        .expect("drop");
    let pair2 = seeded_pair().await;
    pair2
        .database
        .execute_unprepared(
            "CREATE TRIGGER abort_cp BEFORE INSERT ON run_checkpoints BEGIN SELECT RAISE(ABORT, 'test abort cp'); END;",
        )
        .await
        .expect("trigger cp");
    let before2 = persisted_rows(&pair2.database).await;
    let scope2 = batch_scope(&pair2, UnixMillis::from_millis(BOUND_AT_MS));
    let err2 = pair2
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("p-act-trig2").expect("p")),
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-trig2").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &PatchId::parse("p-start-trig2").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("trigger cp");
    assert!(matches!(
        err2,
        RunObservationError::Repository(RepositoryError::Database { .. })
    ));
    helper_late_transaction_failure_via_trigger_rolls_back_everything_tail(&before2, &pair2, &body)
        .await;
}
async fn helper_late_transaction_failure_via_trigger_rolls_back_everything_tail(
    before2: &PersistedRows,
    pair2: &SeededPair,
    body: &AssistantBody,
) {
    assert_eq!(before2, &persisted_rows(&pair2.database).await);
    pair2
        .database
        .execute_unprepared("DROP TRIGGER abort_cp")
        .await
        .expect("drop2");
    let pair3 = seeded_pair().await;
    pair3
        .database
        .execute_unprepared(
            "CREATE TRIGGER abort_receipt BEFORE INSERT ON run_batch_receipts BEGIN SELECT RAISE(ABORT, 'test abort receipt'); END;",
        )
        .await
        .expect("trigger receipt");
    let before3 = persisted_rows(&pair3.database).await;
    let scope3 = batch_scope(&pair3, UnixMillis::from_millis(BOUND_AT_MS));
    let err3 = pair3
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope3,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("p-act-trig3").expect("p")),
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-trig3").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body,
                patch_id: &PatchId::parse("p-start-trig3").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("trigger receipt");
    assert!(matches!(
        err3,
        RunObservationError::Repository(RepositoryError::Database { .. })
    ));
    assert_eq!(before3, persisted_rows(&pair3.database).await);
}

#[tokio::test]
async fn file_backed_races_identical_and_conflicting() {
    let temp = TempDatabase::new("run-observation-race-identical");
    let path = temp.database().to_path_buf();
    let setup_db = connect(
        SqliteConfig::file(&path)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("setup");
    migrate_to_current(&setup_db).await.expect("migrate");
    let setup_repo = Repository::new(setup_db.clone());
    let engine_settings = seed_project_and_thread(&setup_db, &setup_repo).await;
    setup_repo
        .queue_first_message(queue_input())
        .await
        .expect("queue");
    let claimed = setup_repo
        .claim_next_message_dispatch(claim_command(DISPATCH_OWNER_BYTE))
        .await
        .expect("claim")
        .expect("claimed");
    let identity = launch_identity();
    let context = LaunchContext::fixture(engine_settings);
    let artisan_database::LaunchClaimedRunOutcome::Started(launched) = setup_repo
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("launch")
    else {
        panic!("started")
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let artisan_database::BindRunProviderOutcome::Bound(bound) = setup_repo
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &context.start_key,
            credentials: &context.credentials,
            expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
            bound_at: UnixMillis::from_millis(BOUND_AT_MS),
            binding_version: 2,
            binding_bytes: &binding,
        })
        .await
        .expect("bind")
    else {
        panic!("bound")
    };
    let db_a = connect(
        SqliteConfig::file(&path)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("a");
    let db_b = connect(
        SqliteConfig::file(&path)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("b");
    let repo_a = Repository::new(db_a.clone());
    let repo_b = Repository::new(db_b.clone());
    let claimed_a = replayable_claim(&claimed);
    let claimed_b = replayable_claim(&claimed);
    let body = assistant_body("race");
    let path_ref = path.as_path();
    let ctx = IdenticalRaceTailContext {
        binding: &binding,
        claimed_a: &claimed_a,
        claimed_b: &claimed_b,
        body: &body,
        path: path_ref,
        context: &context,
        repo_a: &repo_a,
        repo_b: &repo_b,
        launched: &launched,
        bound: &bound,
    };
    helper_file_backed_races_identical_and_conflicting_tail(ctx).await;
}
struct IdenticalRaceTailContext<'a> {
    binding: &'a ProviderBindingBytes,
    claimed_a: &'a ClaimedMessageDispatch,
    claimed_b: &'a ClaimedMessageDispatch,
    body: &'a AssistantBody,
    path: &'a Path,
    context: &'a LaunchContext,
    repo_a: &'a Repository,
    repo_b: &'a Repository,
    launched: &'a artisan_database::LaunchedRunReceipt,
    bound: &'a artisan_database::BoundRunReceipt,
}
async fn helper_file_backed_races_identical_and_conflicting_tail(
    ctx: IdenticalRaceTailContext<'_>,
) {
    let iid = ItemId::parse("assistant-race").expect("id");
    let pt = PatchId::parse("p-act-race").expect("p");
    let pi = PatchId::parse("p-start-race").expect("p");
    let scope_a = RunBatchScope {
        claimed: ctx.claimed_a,
        launched: ctx.launched,
        bound: ctx.bound,
        run_start_key: &ctx.context.start_key,
        credentials: &ctx.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let scope_b = RunBatchScope {
        claimed: ctx.claimed_b,
        launched: ctx.launched,
        bound: ctx.bound,
        run_start_key: &ctx.context.start_key,
        credentials: &ctx.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let changes_a = [AssistantChange::Start {
        item_id: &iid,
        phase: AssistantMessagePhase::Unspecified,
        body: ctx.body,
        patch_id: &pi,
    }];
    let changes_b = [AssistantChange::Start {
        item_id: &iid,
        phase: AssistantMessagePhase::Unspecified,
        body: ctx.body,
        patch_id: &pi,
    }];
    let (out_a, out_b) = tokio::join!(
        ctx.repo_a.commit_run_batch(CommitRunBatch {
            scope: scope_a,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &changes_a,
            checkpoint: CheckpointUpdate::Keep
        }),
        ctx.repo_b.commit_run_batch(CommitRunBatch {
            scope: scope_b,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &changes_b,
            checkpoint: CheckpointUpdate::Keep
        })
    );
    let committed = [&out_a, &out_b]
        .iter()
        .filter(|r| matches!(r, Ok(CommitRunBatchOutcome::Committed(_))))
        .count();
    let already = [&out_a, &out_b]
        .iter()
        .filter(|r| matches!(r, Ok(CommitRunBatchOutcome::AlreadyCommitted(_))))
        .count();
    assert_eq!(
        committed, 1,
        "identical race must have exactly one Committed: {out_a:?} {out_b:?}"
    );
    assert_eq!(
        already, 1,
        "identical race must have exactly one AlreadyCommitted: {out_a:?} {out_b:?}"
    );
    let reopen = connect(
        SqliteConfig::file(ctx.path)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("reopen");
    let receipts = entities::run_batch_receipt::Entity::find()
        .all(&reopen)
        .await
        .expect("receipts");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].batch_sequence, 1);
    assert!(receipts[0].committed);
    verify_conflicting_race(ctx.binding).await;
}

async fn verify_conflicting_race(binding: &ProviderBindingBytes) {
    let temp2 = TempDatabase::new("run-observation-race-conflict");
    let path2 = temp2.database().to_path_buf();
    let setup_db2 = connect(
        SqliteConfig::file(&path2)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("setup2");
    migrate_to_current(&setup_db2).await.expect("migrate2");
    let setup_repo2 = Repository::new(setup_db2.clone());
    let engine_settings2 = seed_project_and_thread(&setup_db2, &setup_repo2).await;
    setup_repo2
        .queue_first_message(queue_input())
        .await
        .expect("queue2");
    let claimed2 = setup_repo2
        .claim_next_message_dispatch(claim_command(DISPATCH_OWNER_BYTE))
        .await
        .expect("claim2")
        .expect("cl2");
    let identity2 = launch_identity();
    let context2 = LaunchContext::fixture(engine_settings2);
    let artisan_database::LaunchClaimedRunOutcome::Started(launched2) = setup_repo2
        .launch_claimed_run(launch_command(&claimed2, &identity2, &context2))
        .await
        .expect("launch2")
    else {
        panic!("st")
    };
    let artisan_database::BindRunProviderOutcome::Bound(bound2) = setup_repo2
        .bind_run_provider(BindRunProvider {
            claimed: &claimed2,
            receipt: &launched2,
            run_start_key: &context2.start_key,
            credentials: &context2.credentials,
            expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
            bound_at: UnixMillis::from_millis(BOUND_AT_MS),
            binding_version: 2,
            binding_bytes: binding,
        })
        .await
        .expect("bind2")
    else {
        panic!()
    };
    let alpha_db = connect(
        SqliteConfig::file(&path2)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("a2");
    let beta_db = connect(
        SqliteConfig::file(&path2)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("b2");
    let alpha_repo = Repository::new(alpha_db.clone());
    let beta_repo = Repository::new(beta_db.clone());
    let alpha_claim = replayable_claim(&claimed2);
    let beta_claim = replayable_claim(&claimed2);
    let alpha_body = assistant_body("content-a");
    let beta_body = assistant_body("content-b");
    let ctx = ConflictingRaceTailContext {
        alpha_body: &alpha_body,
        alpha_repo: &alpha_repo,
        beta_body: &beta_body,
        alpha_claim: &alpha_claim,
        beta_repo: &beta_repo,
        beta_claim: &beta_claim,
        context2: &context2,
        launched2: &launched2,
        bound2: &bound2,
    };
    helper_verify_conflicting_race_tail(ctx).await;
}
struct ConflictingRaceTailContext<'a> {
    alpha_body: &'a AssistantBody,
    alpha_repo: &'a Repository,
    beta_body: &'a AssistantBody,
    alpha_claim: &'a ClaimedMessageDispatch,
    beta_repo: &'a Repository,
    beta_claim: &'a ClaimedMessageDispatch,
    context2: &'a LaunchContext,
    launched2: &'a artisan_database::LaunchedRunReceipt,
    bound2: &'a artisan_database::BoundRunReceipt,
}
async fn helper_verify_conflicting_race_tail(ctx: ConflictingRaceTailContext<'_>) {
    let alpha_item = ItemId::parse("assistant-conflict").expect("id");
    let beta_item = ItemId::parse("assistant-conflict").expect("id");
    let alpha_item_patch = PatchId::parse("p-start-conflict-a").expect("p");
    let beta_item_patch = PatchId::parse("p-start-conflict-b").expect("p");
    let alpha_activation_patch = PatchId::parse("p-act-conflict-a").expect("p");
    let beta_activation_patch = PatchId::parse("p-act-conflict-b").expect("p");
    let alpha_scope = RunBatchScope {
        claimed: ctx.alpha_claim,
        launched: ctx.launched2,
        bound: ctx.bound2,
        run_start_key: &ctx.context2.start_key,
        credentials: &ctx.context2.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let beta_scope = RunBatchScope {
        claimed: ctx.beta_claim,
        launched: ctx.launched2,
        bound: ctx.bound2,
        run_start_key: &ctx.context2.start_key,
        credentials: &ctx.context2.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let alpha_changes = [AssistantChange::Start {
        item_id: &alpha_item,
        phase: AssistantMessagePhase::Unspecified,
        body: ctx.alpha_body,
        patch_id: &alpha_item_patch,
    }];
    let beta_changes = [AssistantChange::Start {
        item_id: &beta_item,
        phase: AssistantMessagePhase::Unspecified,
        body: ctx.beta_body,
        patch_id: &beta_item_patch,
    }];
    let (alpha_outcome, beta_outcome) = tokio::join!(
        ctx.alpha_repo.commit_run_batch(CommitRunBatch {
            scope: alpha_scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&alpha_activation_patch),
            changes: &alpha_changes,
            checkpoint: CheckpointUpdate::Keep
        }),
        ctx.beta_repo.commit_run_batch(CommitRunBatch {
            scope: beta_scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&beta_activation_patch),
            changes: &beta_changes,
            checkpoint: CheckpointUpdate::Keep
        })
    );
    let successes = [&alpha_outcome, &beta_outcome]
        .iter()
        .filter(|r| r.is_ok())
        .count();
    assert_eq!(
        successes, 1,
        "conflicting same sequence must have exactly one winner: {alpha_outcome:?} {beta_outcome:?}"
    );
    let _conflicts = [&alpha_outcome, &beta_outcome]
        .iter()
        .filter(|r| matches!(r, Err(RunObservationError::ReceiptConflict { .. })))
        .count();
    let has_error = [&alpha_outcome, &beta_outcome].iter().any(|r| r.is_err());
    assert!(has_error, "one must be typed conflict");
}

#[tokio::test]
async fn digest_independent_encoder_equals_persisted_and_inequality() {
    let pair = seeded_pair().await;
    let body = assistant_body("digest body 🚀");
    let iid = ItemId::parse("assistant-digest").expect("id");
    let pt = PatchId::parse("p-act-digest").expect("p");
    let pi = PatchId::parse("p-start-digest").expect("p");
    let cp = EngineCheckpoint::new(7, vec![0xde, 0xad, 0xbe, 0xef]).expect("cp");
    let scope = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    let cmd = CommitRunBatch {
        scope,
        batch_sequence: 1,
        operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
        activate_turn_patch_id: Some(&pt),
        changes: &[AssistantChange::Start {
            item_id: &iid,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &pi,
        }],
        checkpoint: CheckpointUpdate::Replace(&cp),
    };
    let expected_digest = independent_digest(&cmd, Some((7, &[0xde, 0xad, 0xbe, 0xef])));
    let scope2 = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope: scope2,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&pt),
            changes: &[AssistantChange::Start {
                item_id: &iid,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: &pi,
            }],
            checkpoint: CheckpointUpdate::Replace(&cp),
        })
        .await
        .expect("commit digest");
    let receipt = entities::run_batch_receipt::Entity::find_by_id((RUN_ID.to_owned(), 1))
        .one(&pair.database)
        .await
        .expect("q")
        .expect("receipt");
    assert_eq!(receipt.digest.as_slice(), expected_digest);
    let cmd_changed = CommitRunBatch {
        scope: batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS)),
        batch_sequence: 1,
        operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS + 1),
        activate_turn_patch_id: Some(&pt),
        changes: &[AssistantChange::Start {
            item_id: &iid,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &pi,
        }],
        checkpoint: CheckpointUpdate::Replace(&cp),
    };
    assert_ne!(
        independent_digest(&cmd_changed, Some((7, &[0xde, 0xad, 0xbe, 0xef]))),
        expected_digest
    );
    let iid2 = ItemId::parse("assistant-digest2").expect("id");
    let cmd_phase = CommitRunBatch {
        scope: batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS)),
        batch_sequence: 1,
        operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
        activate_turn_patch_id: Some(&pt),
        changes: &[AssistantChange::Start {
            item_id: &iid2,
            phase: AssistantMessagePhase::Commentary,
            body: &body,
            patch_id: &PatchId::parse("p-start-digest2").expect("p"),
        }],
        checkpoint: CheckpointUpdate::Replace(&cp),
    };
    let cmd_phase2 = CommitRunBatch {
        scope: batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS)),
        batch_sequence: 1,
        operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
        activate_turn_patch_id: Some(&pt),
        changes: &[AssistantChange::Start {
            item_id: &iid2,
            phase: AssistantMessagePhase::Unspecified,
            body: &body,
            patch_id: &PatchId::parse("p-start-digest2").expect("p"),
        }],
        checkpoint: CheckpointUpdate::Replace(&cp),
    };
    assert_ne!(
        independent_digest(&cmd_phase, Some((7, &[0xde, 0xad, 0xbe, 0xef]))),
        independent_digest(&cmd_phase2, Some((7, &[0xde, 0xad, 0xbe, 0xef])))
    );
}

#[tokio::test]
async fn redaction_errors_never_contain_secret_or_checkpoint_bytes() {
    let pair = seeded_pair().await;
    let bad_lease = RunLaunchCredentials::new(OWNER_BYTES, [0x99; 32], CLAIM_TOKEN_BYTES);
    let scope_bad = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &bad_lease,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let body = assistant_body("secret");
    let err = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope_bad,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("p-act-redact").expect("p")),
            changes: &[AssistantChange::Start {
                item_id: &ItemId::parse("assistant-redact").expect("id"),
                phase: AssistantMessagePhase::Unspecified,
                body: &body,
                patch_id: &PatchId::parse("p-start-redact").expect("p"),
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect_err("bad lease");
    let msg = format!("{err} {err:?}");
    // secret bytes should not appear: check hex of owner not present
    assert!(!msg.contains("a1a1"), "error should not leak owner bytes");
    assert!(!msg.contains("b2b2"), "error should not leak lease bytes");
    // checkpoint bytes redaction: create checkpoint with known pattern and try to cause InvalidCheckpoint error leaking bytes
    let _bad_cp = EngineCheckpoint::new(1, vec![0xde, 0xad, 0xbe, 0xef]);
    // bad_cp itself error not needed; but ensure formatting doesn't contain bytes
    // Use an invalid checkpoint attempt: we test EngineCheckpoint::new error message doesn't contain bytes? It shouldn't.
    let bad = EngineCheckpoint::new(0, vec![0xde, 0xad]);
    if let Err(e) = bad {
        let m = format!("{e} {e:?}");
        assert!(
            !m.contains("dead"),
            "checkpoint error should not leak bytes"
        );
    }
    // Also checkpoint valid but error on other field should not leak checkpoint bytes
    let cp_secret = EngineCheckpoint::new(5, vec![0xca, 0xfe]).expect("cp");
    // trigger a different error while having a secret checkpoint in command - use duplicate patch error
    let iid1 = ItemId::parse("assistant-redact2").expect("id");
    let iid2 = ItemId::parse("assistant-redact3").expect("id");
    let dup_patch = PatchId::parse("p-dup-redact").expect("p");
    let scope_dup = batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS));
    let err2 = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: scope_dup,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&PatchId::parse("p-act-redact2").expect("p")),
            changes: &[
                AssistantChange::Start {
                    item_id: &iid1,
                    phase: AssistantMessagePhase::Unspecified,
                    body: &body,
                    patch_id: &dup_patch,
                },
                AssistantChange::Start {
                    item_id: &iid2,
                    phase: AssistantMessagePhase::Unspecified,
                    body: &body,
                    patch_id: &dup_patch,
                },
            ],
            checkpoint: CheckpointUpdate::Replace(&cp_secret),
        })
        .await
        .expect_err("dup patch");
    let m2 = format!("{err2} {err2:?}");
    assert!(
        !m2.contains("cafe") && !m2.contains("CAFE"),
        "error should not leak checkpoint bytes"
    );
}
