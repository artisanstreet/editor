//! Provider-binding coverage through real migrated SQLite and the public
//! repository APIs. Mirrors `run_launch` seeding and adds the binding matrix.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{self, AssistantRunLifecycle, DispatchState};
use artisan_database::{
    BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch, ClaimedMessageDispatch,
    CreateThreadInput, ProviderBindingBytes, QueueFirstMessageInput, Repository, RepositoryError,
    RunBindingError, RunLaunchCredentials, RunStartKey, SqliteConfig, connect,
};
use artisan_domain::{
    ItemId, MessageBody, MessageId, PatchId, ProjectId, RequestId, RunId, ThreadId, ThreadTitle,
    TurnId, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

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

const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: ?Sized + std::fmt::Debug> Ambiguous<Marker> for T {}
    let _ = <ProviderBindingBytes as Ambiguous<_>>::marker;
};
const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: ?Sized + std::fmt::Display> Ambiguous<Marker> for T {}
    let _ = <ProviderBindingBytes as Ambiguous<_>>::marker;
};
const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: Clone> Ambiguous<Marker> for T {}
    let _ = <ProviderBindingBytes as Ambiguous<_>>::marker;
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

async fn seed_project_and_thread(database: &DatabaseConnection, repository: &Repository) {
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
        checkpoints: all::<entities::run_checkpoint::Entity>(database).await,
        batch_receipts: all::<entities::run_batch_receipt::Entity>(database).await,
    }
}

async fn seeded_repository() -> (
    DatabaseConnection,
    Repository,
    ClaimedMessageDispatch,
    artisan_database::LaunchedRunReceipt,
    LaunchContext,
    LaunchIdentityFixture,
) {
    let (database, repository) = memory_database().await;
    seed_project_and_thread(&database, &repository).await;
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
    let context = LaunchContext::fixture();
    let artisan_database::LaunchClaimedRunOutcome::Started(receipt) = repository
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("launch")
    else {
        panic!("started")
    };
    (database, repository, claimed, receipt, context, identity)
}

fn bind_command<'a>(
    claimed: &'a ClaimedMessageDispatch,
    receipt: &'a artisan_database::LaunchedRunReceipt,
    context: &'a LaunchContext,
    bytes: &'a ProviderBindingBytes,
    version: i64,
    expected_launch_at: UnixMillis,
    bound_at: UnixMillis,
) -> BindRunProvider<'a> {
    BindRunProvider {
        claimed,
        receipt,
        run_start_key: &context.start_key,
        credentials: &context.credentials,
        expected_launch_at,
        bound_at,
        binding_version: version,
        binding_bytes: bytes,
    }
}

async fn assert_generation_rejections(
    repository: &Repository,
    claimed: &ClaimedMessageDispatch,
    receipt: &artisan_database::LaunchedRunReceipt,
    context: &LaunchContext,
    before: &PersistedRows,
    database: &DatabaseConnection,
) {
    let binding = ProviderBindingBytes::new(vec![9; 8]).expect("binding");
    let mut bad_receipt = receipt.clone();
    bad_receipt.generation = 2;
    let err = repository
        .bind_run_provider(bind_command(
            claimed,
            &bad_receipt,
            context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("wrong generation");
    assert!(matches!(err, RunBindingError::CredentialMismatch { .. }));
    assert_eq!(before, &persisted_rows(database).await);
    let bad_ctx = LaunchContext {
        start_key: RunStartKey::new([0x99; 32]),
        credentials: RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES),
    };
    let err = repository
        .bind_run_provider(bind_command(
            claimed,
            receipt,
            &bad_ctx,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("wrong start key");
    assert!(matches!(err, RunBindingError::CredentialMismatch { .. }));
    assert_eq!(before, &persisted_rows(database).await);
    let bad_cred = RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, [0x77; 32]);
    let bad_ctx2 = LaunchContext {
        start_key: RunStartKey::new(START_KEY_BYTES),
        credentials: bad_cred,
    };
    let err = repository
        .bind_run_provider(bind_command(
            claimed,
            receipt,
            &bad_ctx2,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("wrong claim token");
    assert!(matches!(err, RunBindingError::CredentialMismatch { .. }));
    assert_eq!(before, &persisted_rows(database).await);
}

async fn assert_snapshot_rejections(
    repository: &Repository,
    claimed: &ClaimedMessageDispatch,
    receipt: &artisan_database::LaunchedRunReceipt,
    context: &LaunchContext,
    before: &PersistedRows,
    database: &DatabaseConnection,
) {
    let binding = ProviderBindingBytes::new(vec![9; 8]).expect("binding");
    let mut stale = replayable_claim(claimed);
    stale.queued_at = UnixMillis::from_millis(9999);
    let err = repository
        .bind_run_provider(BindRunProvider {
            claimed: &stale,
            ..bind_command(
                claimed,
                receipt,
                context,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            )
        })
        .await
        .expect_err("stale queued");
    assert!(matches!(err, RunBindingError::SnapshotMismatch { .. }));
    assert_eq!(before, &persisted_rows(database).await);
    let err = repository
        .bind_run_provider(bind_command(
            claimed,
            receipt,
            context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        ))
        .await
        .expect_err("expiry equality");
    assert!(matches!(
        err,
        RunBindingError::Repository(RepositoryError::DispatchLeaseExpired { .. })
    ));
    assert_eq!(before, &persisted_rows(database).await);
    let err = repository
        .bind_run_provider(bind_command(
            claimed,
            receipt,
            context,
            &binding,
            2,
            UnixMillis::from_millis(CLAIMED_AT_MS - 10),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("chronology");
    assert!(matches!(
        err,
        RunBindingError::Repository(RepositoryError::InvalidChronology { .. })
    ));
    assert_eq!(before, &persisted_rows(database).await);
    let err = repository
        .bind_run_provider(bind_command(
            claimed,
            receipt,
            context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(OPERATED_AT_MS - 1),
        ))
        .await
        .expect_err("chronology");
    assert!(matches!(
        err,
        RunBindingError::Repository(RepositoryError::InvalidChronology { .. })
    ));
    assert_eq!(before, &persisted_rows(database).await);
}

async fn check_version_rejections(
    repository: &Repository,
    claimed: &ClaimedMessageDispatch,
    receipt: &artisan_database::LaunchedRunReceipt,
    context: &LaunchContext,
    database: &DatabaseConnection,
    before: &PersistedRows,
) {
    let binding = ProviderBindingBytes::new(vec![1; 1]).expect("1 byte");
    let err = repository
        .bind_run_provider(bind_command(
            claimed,
            receipt,
            context,
            &binding,
            0,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("version 0");
    assert!(matches!(
        err,
        RunBindingError::InvalidBindingVersion { version: 0 }
    ));
    assert!(!err.to_string().contains("ab"));
    assert_eq!(before, &persisted_rows(database).await);
    let err = repository
        .bind_run_provider(bind_command(
            claimed,
            receipt,
            context,
            &binding,
            -1,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("negative version");
    assert!(matches!(
        err,
        RunBindingError::InvalidBindingVersion { version: -1 }
    ));
    assert_eq!(before, &persisted_rows(database).await);
}

async fn verify_max_version_persists(
    database: &DatabaseConnection,
    repository: &Repository,
    claimed: &ClaimedMessageDispatch,
    receipt: &artisan_database::LaunchedRunReceipt,
    context: &LaunchContext,
) {
    let large_version = i64::MAX;
    let binding_max = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let outcome = repository
        .bind_run_provider(bind_command(
            claimed,
            receipt,
            context,
            &binding_max,
            large_version,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("max i64 version must bind");
    let BindRunProviderOutcome::Bound(bound) = outcome else {
        panic!("fresh bind must be Bound")
    };
    assert_eq!(bound.binding_version, i64::MAX);
    assert_eq!(bound.bound_at, UnixMillis::from_millis(BOUND_AT_MS));
    let persisted = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(database)
        .await
        .expect("q")
        .expect("exists");
    assert_eq!(persisted.provider_binding_version, Some(i64::MAX));
    assert_eq!(persisted.provider_bound_at_ms, Some(BOUND_AT_MS));
    assert_eq!(
        persisted
            .provider_binding
            .as_ref()
            .map(entities::OpaqueBytes::as_slice),
        Some(&[0xab; 16][..])
    );
}

async fn verify_large_payload_persists() {
    let (database2, repository2, claimed2, receipt2, context2, _) = seeded_repository().await;
    let large_bytes = vec![0x5a; 262_144];
    let large_binding = ProviderBindingBytes::new(large_bytes.clone()).expect("262144");
    let outcome2 = repository2
        .bind_run_provider(bind_command(
            &claimed2,
            &receipt2,
            &context2,
            &large_binding,
            7,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("262144-byte bind should succeed");
    let BindRunProviderOutcome::Bound(bound2) = outcome2 else {
        panic!("large payload must be Bound")
    };
    assert_eq!(bound2.binding_version, 7);
    assert_eq!(bound2.bound_at, UnixMillis::from_millis(BOUND_AT_MS));
    let run2 = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database2)
        .await
        .expect("q")
        .expect("exists");
    assert_eq!(run2.provider_binding_version, Some(7));
    assert_eq!(
        run2.provider_binding
            .as_ref()
            .map(entities::OpaqueBytes::as_slice),
        Some(large_bytes.as_slice())
    );
    assert_eq!(run2.provider_bound_at_ms, Some(BOUND_AT_MS));
    assert_eq!(run2.updated_at_ms, BOUND_AT_MS);
}

async fn check_changed_payload_time(
    repository: &Repository,
    replay_claimed: &ClaimedMessageDispatch,
    receipt: &artisan_database::LaunchedRunReceipt,
    context: &LaunchContext,
    binding: &ProviderBindingBytes,
    before: &PersistedRows,
    database: &DatabaseConnection,
) {
    let other = ProviderBindingBytes::new(vec![0xcd; 16]).expect("other");
    let err = repository
        .bind_run_provider(bind_command(
            replay_claimed,
            receipt,
            context,
            &other,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("changed payload");
    assert!(matches!(err, RunBindingError::CredentialMismatch { .. }));
    assert_eq!(before, &persisted_rows(database).await);
    let err = repository
        .bind_run_provider(bind_command(
            replay_claimed,
            receipt,
            context,
            binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS + 1),
        ))
        .await
        .expect_err("changed time");
    assert!(matches!(err, RunBindingError::SnapshotMismatch { .. }));
    assert_eq!(before, &persisted_rows(database).await);
}

async fn check_remaining_owner_erased(
    repository: &Repository,
    replay_claimed: &ClaimedMessageDispatch,
    receipt: &artisan_database::LaunchedRunReceipt,
    binding: &ProviderBindingBytes,
    before: &PersistedRows,
    database: &DatabaseConnection,
) {
    let different_owner_cred =
        RunLaunchCredentials::new([0x01; 32], LEASE_BYTES, CLAIM_TOKEN_BYTES);
    let different_ctx = LaunchContext {
        start_key: RunStartKey::new(START_KEY_BYTES),
        credentials: different_owner_cred,
    };
    let err = repository
        .bind_run_provider(bind_command(
            replay_claimed,
            receipt,
            &different_ctx,
            binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("changed owner");
    assert!(matches!(err, RunBindingError::CredentialMismatch { .. }));
    assert_eq!(before, &persisted_rows(database).await);
    let different_claim = RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, [0xff; 32]);
    let different_claim_ctx = LaunchContext {
        start_key: RunStartKey::new(START_KEY_BYTES),
        credentials: different_claim,
    };
    let replay_with_different_claim = repository
        .bind_run_provider(bind_command(
            replay_claimed,
            receipt,
            &different_claim_ctx,
            binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("different claim token after erasure is unverifiable and returns AlreadyBound");
    assert!(matches!(
        replay_with_different_claim,
        BindRunProviderOutcome::AlreadyBound(_)
    ));
    assert_eq!(before, &persisted_rows(database).await);
}

fn assert_race_receipts(
    out_a: &Result<BindRunProviderOutcome, RunBindingError>,
    out_b: &Result<BindRunProviderOutcome, RunBindingError>,
) {
    let bound_count = [out_a, out_b]
        .iter()
        .filter(|r| matches!(r, Ok(BindRunProviderOutcome::Bound(_))))
        .count();
    let already_count = [out_a, out_b]
        .iter()
        .filter(|r| matches!(r, Ok(BindRunProviderOutcome::AlreadyBound(_))))
        .count();
    assert_eq!(
        bound_count, 1,
        "exactly one racer may commit Bound; outcomes were {out_a:?} and {out_b:?}"
    );
    assert_eq!(
        already_count, 1,
        "exactly one racer must be AlreadyBound; outcomes were {out_a:?} and {out_b:?}"
    );
    for outcome in [out_a, out_b] {
        match outcome {
            Ok(BindRunProviderOutcome::Bound(_) | BindRunProviderOutcome::AlreadyBound(_)) => {}
            Err(other) => panic!("race must reject every Database error, got {other:?}"),
        }
    }
    let ((
        Ok(BindRunProviderOutcome::Bound(bound_receipt)),
        Ok(BindRunProviderOutcome::AlreadyBound(already_receipt)),
    )
    | (
        Ok(BindRunProviderOutcome::AlreadyBound(already_receipt)),
        Ok(BindRunProviderOutcome::Bound(bound_receipt)),
    )) = (out_a, out_b)
    else {
        panic!("race must yield one Bound and one AlreadyBound, got {out_a:?} and {out_b:?}")
    };
    assert_eq!(bound_receipt.run_id, already_receipt.run_id);
    assert_eq!(bound_receipt.thread_id, already_receipt.thread_id);
    assert_eq!(bound_receipt.message_id, already_receipt.message_id);
    assert_eq!(bound_receipt.generation, already_receipt.generation);
    assert_eq!(
        bound_receipt.binding_version,
        already_receipt.binding_version
    );
    assert_eq!(bound_receipt.bound_at, already_receipt.bound_at);
    assert_eq!(bound_receipt, already_receipt);
}

#[tokio::test]
async fn successful_bind_writes_exact_blob_version_time() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let outcome = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("bind should succeed");
    let BindRunProviderOutcome::Bound(bound) = outcome else {
        panic!("bound")
    };
    assert_eq!(bound.run_id.as_str(), RUN_ID);
    assert_eq!(bound.thread_id.as_str(), THREAD_ID);
    assert_eq!(bound.message_id.as_str(), MESSAGE_ID);
    assert_eq!(bound.generation, 1);
    assert_eq!(bound.binding_version, 2);
    assert_eq!(bound.bound_at, UnixMillis::from_millis(BOUND_AT_MS));
    let dispatch = entities::message_dispatch::Entity::find_by_id(MESSAGE_ID)
        .one(&database)
        .await
        .expect("q")
        .expect("exists");
    assert_eq!(dispatch.state, DispatchState::Running);
    assert_eq!(dispatch.updated_at_ms, BOUND_AT_MS);
    assert_eq!(
        dispatch.lease_owner.as_deref(),
        Some("1111111111111111111111111111111111111111111111111111111111111111")
    );
    assert_eq!(dispatch.lease_expires_at_ms, Some(LEASE_EXPIRES_AT_MS));
    let run = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("q")
        .expect("exists");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Running);
    assert_eq!(run.generation, 1);
    assert_eq!(run.provider_binding_version, Some(2));
    assert_eq!(
        run.provider_binding
            .as_ref()
            .map(entities::OpaqueBytes::as_slice),
        Some(&[0xab; 16][..])
    );
    assert_eq!(run.provider_bound_at_ms, Some(BOUND_AT_MS));
    assert_eq!(run.created_at_ms, OPERATED_AT_MS);
    assert_eq!(run.updated_at_ms, BOUND_AT_MS);
    assert!(run.claim_token.is_none());
    assert_eq!(
        run.owner.as_ref().map(entities::OpaqueBytes::as_slice),
        Some(&OWNER_BYTES[..])
    );
    assert_eq!(
        run.lease.as_ref().map(entities::OpaqueBytes::as_slice),
        Some(&LEASE_BYTES[..])
    );
    assert!(run.error_code.is_none() && run.terminal_at_ms.is_none());
    let after = persisted_rows(&database).await;
    assert_eq!(before.projects, after.projects);
    assert_eq!(before.threads, after.threads);
    assert_eq!(before.messages, after.messages);
    assert_eq!(before.receipts, after.receipts);
    assert_eq!(before.states, after.states);
    assert_eq!(before.ordinals, after.ordinals);
    assert_eq!(before.turns, after.turns);
    assert_eq!(before.items, after.items);
    assert_eq!(before.patches, after.patches);
    assert_eq!(before.checkpoints, after.checkpoints);
    assert_eq!(before.batch_receipts, after.batch_receipts);
    let mut expected_dispatches = before.dispatches.clone();
    let dispatch_entry = expected_dispatches
        .iter_mut()
        .find(|row| row.message_id == MESSAGE_ID)
        .expect("dispatch");
    dispatch_entry.updated_at_ms = BOUND_AT_MS;
    assert_eq!(expected_dispatches, after.dispatches);
    let mut expected_runs = before.runs.clone();
    let run_entry = expected_runs
        .iter_mut()
        .find(|row| row.run_id == RUN_ID)
        .expect("run");
    run_entry.lifecycle = AssistantRunLifecycle::Running;
    run_entry.claim_token = None;
    run_entry.provider_binding_version = Some(2);
    run_entry.provider_binding = Some(entities::OpaqueBytes::new(vec![0xab; 16]));
    run_entry.provider_bound_at_ms = Some(BOUND_AT_MS);
    run_entry.updated_at_ms = BOUND_AT_MS;
    assert_eq!(expected_runs, after.runs);
    let unrelated_blob = ProviderBindingBytes::new(vec![7; 8]);
    assert!(unrelated_blob.is_ok());
}

#[tokio::test]
async fn post_dispatch_fence_failure_rolls_back_dispatch_stamp() {
    let (database, repository, claimed, receipt, _context, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    let wrong_context = LaunchContext {
        start_key: RunStartKey::new([0xff; 32]),
        credentials: RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES),
    };
    let binding = ProviderBindingBytes::new(vec![1, 2, 3]).expect("binding");
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &wrong_context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("wrong start key should fail");
    assert!(matches!(err, RunBindingError::CredentialMismatch { .. }));
    let after = persisted_rows(&database).await;
    let dispatch = after
        .dispatches
        .iter()
        .find(|row| row.message_id == MESSAGE_ID)
        .expect("dispatch");
    assert_eq!(dispatch.updated_at_ms, OPERATED_AT_MS);
    assert_eq!(
        before, after,
        "post-dispatch fence must rollback entire transaction including tentative dispatch stamp"
    );
}

#[tokio::test]
async fn rejection_of_wrong_credential_and_snapshot_fields() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    let binding = ProviderBindingBytes::new(vec![9; 8]).expect("binding");
    let mut wrong_owner = replayable_claim(&claimed);
    wrong_owner.owner = artisan_database::DispatchLeaseOwner::new([0x22; 32]);
    let err = repository
        .bind_run_provider(BindRunProvider {
            claimed: &wrong_owner,
            ..bind_command(
                &claimed,
                &receipt,
                &context,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            )
        })
        .await
        .expect_err("wrong owner");
    assert!(matches!(
        err,
        RunBindingError::Repository(RepositoryError::DispatchOwnerMismatch { .. })
    ));
    assert_eq!(before, persisted_rows(&database).await);
    assert_generation_rejections(
        &repository,
        &claimed,
        &receipt,
        &context,
        &before,
        &database,
    )
    .await;
    let mut bad_receipt2 = receipt.clone();
    bad_receipt2.turn_id = TurnId::parse("turn-zzz").expect("turn");
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &bad_receipt2,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("wrong origin");
    assert!(matches!(err, RunBindingError::IdentityConflict { .. }));
    assert_eq!(before, persisted_rows(&database).await);
    assert_snapshot_rejections(
        &repository,
        &claimed,
        &receipt,
        &context,
        &before,
        &database,
    )
    .await;
}

#[tokio::test]
async fn payload_and_version_boundaries() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    assert!(matches!(
        ProviderBindingBytes::new(vec![]),
        Err(RunBindingError::InvalidBindingLength { length: 0 })
    ));
    assert!(ProviderBindingBytes::new(vec![1]).is_ok());
    assert!(ProviderBindingBytes::new(vec![7; 262_144]).is_ok());
    assert!(matches!(
        ProviderBindingBytes::new(vec![7; 262_145]),
        Err(RunBindingError::InvalidBindingLength { length: 262_145 })
    ));
    let oversized = std::iter::repeat_n([0xde, 0xad, 0xbe, 0xef], 262_145_usize.div_ceil(4))
        .flatten()
        .take(262_145)
        .collect::<Vec<u8>>();
    let Err(err) = ProviderBindingBytes::new(oversized) else {
        panic!("oversized payload must be rejected")
    };
    assert!(matches!(
        err,
        RunBindingError::InvalidBindingLength { length: 262_145 }
    ));
    assert!(!err.to_string().contains("deadbeef") && !format!("{err:?}").contains("deadbeef"));
    check_version_rejections(
        &repository,
        &claimed,
        &receipt,
        &context,
        &database,
        &before,
    )
    .await;
    verify_max_version_persists(&database, &repository, &claimed, &receipt, &context).await;
    verify_large_payload_persists().await;
}

#[tokio::test]
async fn exact_replay_preserves_all_rows() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let first = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("first bind");
    let BindRunProviderOutcome::Bound(expected) = first else {
        panic!("bound")
    };
    let before = persisted_rows(&database).await;
    let replay_claimed = replayable_claim(&claimed);
    let replay = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("replay");
    let BindRunProviderOutcome::AlreadyBound(replayed) = replay else {
        panic!("already bound")
    };
    assert_eq!(replayed, expected);
    let after = persisted_rows(&database).await;
    assert_eq!(before, after, "exact replay must mutate nothing");
}

#[tokio::test]
async fn changed_version_payload_time_credential_rejection_and_erased_token_limitation() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("first bind");
    let before = persisted_rows(&database).await;
    let replay_claimed = replayable_claim(&claimed);
    let err = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &receipt,
            &context,
            &binding,
            3,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("changed version");
    assert!(matches!(err, RunBindingError::CredentialMismatch { .. }));
    assert_eq!(&before, &persisted_rows(&database).await);
    check_changed_payload_time(
        &repository,
        &replay_claimed,
        &receipt,
        &context,
        &binding,
        &before,
        &database,
    )
    .await;
    check_remaining_owner_erased(
        &repository,
        &replay_claimed,
        &receipt,
        &binding,
        &before,
        &database,
    )
    .await;
}

#[tokio::test]
async fn bound_at_equals_expected_launch_at_replay_boundary() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let binding = ProviderBindingBytes::new(vec![0xab; 8]).expect("binding");
    let outcome = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            5,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(OPERATED_AT_MS),
        ))
        .await
        .expect("bound_at == expected_launch_at should succeed");
    assert!(matches!(outcome, BindRunProviderOutcome::Bound(_)));
    let before = persisted_rows(&database).await;
    assert_eq!(before.dispatches[0].updated_at_ms, OPERATED_AT_MS);
    let replay_claimed = replayable_claim(&claimed);
    let replay = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &receipt,
            &context,
            &binding,
            5,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(OPERATED_AT_MS),
        ))
        .await
        .expect("replay boundary should be AlreadyBound");
    assert!(matches!(replay, BindRunProviderOutcome::AlreadyBound(_)));
    let after = persisted_rows(&database).await;
    assert_eq!(
        before, after,
        "boundary replay must rollback tentative dispatch write and mutate nothing"
    );
    assert_eq!(after.dispatches[0].updated_at_ms, OPERATED_AT_MS);
    assert_eq!(after.runs[0].updated_at_ms, OPERATED_AT_MS);
}

#[tokio::test]
async fn file_backed_two_connection_race_proves_first_write_ownership() {
    let temp = TempDatabase::new("run-binding-race");
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
    seed_project_and_thread(&setup_db, &setup_repo).await;
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
    let context = LaunchContext::fixture();
    let artisan_database::LaunchClaimedRunOutcome::Started(receipt) = setup_repo
        .launch_claimed_run(launch_command(&claimed, &identity, &context))
        .await
        .expect("launch")
    else {
        panic!("started")
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
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let claimed_a = replayable_claim(&claimed);
    let claimed_b = replayable_claim(&claimed);
    let race = async {
        tokio::join!(
            repo_a.bind_run_provider(BindRunProvider {
                claimed: &claimed_a,
                receipt: &receipt,
                run_start_key: &context.start_key,
                credentials: &context.credentials,
                expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
                bound_at: UnixMillis::from_millis(BOUND_AT_MS),
                binding_version: 2,
                binding_bytes: &binding
            }),
            repo_b.bind_run_provider(BindRunProvider {
                claimed: &claimed_b,
                receipt: &receipt,
                run_start_key: &context.start_key,
                credentials: &context.credentials,
                expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
                bound_at: UnixMillis::from_millis(BOUND_AT_MS),
                binding_version: 2,
                binding_bytes: &binding
            })
        )
    };
    let (out_a, out_b) = tokio::time::timeout(std::time::Duration::from_secs(10), race)
        .await
        .expect("race must complete within bounded timeout");
    assert_race_receipts(&out_a, &out_b);
    let runs = entities::assistant_run::Entity::find()
        .all(&db_a)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].lifecycle, AssistantRunLifecycle::Running);
    assert_eq!(runs[0].provider_binding_version, Some(2));
}

#[tokio::test]
async fn bound_replay_claim_snapshot_fields() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("first bind");
    let replay_claimed_base = replayable_claim(&claimed);
    for (label, bad) in [
        ("correlation", {
            let mut c = replayable_claim(&claimed);
            c.correlation_id = RequestId::parse("request-zzz").expect("req");
            c
        }),
        ("attempt2", {
            let mut c = replayable_claim(&claimed);
            c.attempt_count = 2;
            c
        }),
        ("queued_at", {
            let mut c = replayable_claim(&claimed);
            c.queued_at = UnixMillis::from_millis(9999);
            c
        }),
        ("available_at", {
            let mut c = replayable_claim(&claimed);
            c.available_at = UnixMillis::from_millis(9999);
            c
        }),
        ("expiry601", {
            let mut c = replayable_claim(&claimed);
            c.lease_expires_at = UnixMillis::from_millis(601);
            c
        }),
    ] {
        let before = persisted_rows(&database).await;
        let err = repository
            .bind_run_provider(BindRunProvider {
                claimed: &bad,
                ..bind_command(
                    &replay_claimed_base,
                    &receipt,
                    &context,
                    &binding,
                    2,
                    UnixMillis::from_millis(OPERATED_AT_MS),
                    UnixMillis::from_millis(BOUND_AT_MS),
                )
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, RunBindingError::SnapshotMismatch { ref message_id } if message_id.as_str() == MESSAGE_ID),
            "{label} should be SnapshotMismatch with message_id {MESSAGE_ID}, got {err:?}"
        );
        assert_eq!(
            before,
            persisted_rows(&database).await,
            "{label} must not mutate 13 tables"
        );
    }
}

#[tokio::test]
async fn bound_replay_expected_launch_149_is_credential_mismatch() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("first bind");
    let before = persisted_rows(&database).await;
    let replay_claimed = replayable_claim(&claimed);
    let err = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(149),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("launch 149 on bound replay");
    assert!(
        matches!(err, RunBindingError::CredentialMismatch { ref run_id } if run_id.as_str() == RUN_ID),
        "expected CredentialMismatch with run_id {RUN_ID}, got {err:?}"
    );
    assert_eq!(before, persisted_rows(&database).await);
}

fn assert_two_row_delta(
    before: &PersistedRows,
    after: &PersistedRows,
    expected_version: i64,
    expected_bound_at: i64,
) {
    assert_eq!(before.projects, after.projects);
    assert_eq!(before.threads, after.threads);
    assert_eq!(before.messages, after.messages);
    assert_eq!(before.receipts, after.receipts);
    assert_eq!(before.states, after.states);
    assert_eq!(before.ordinals, after.ordinals);
    assert_eq!(before.turns, after.turns);
    assert_eq!(before.items, after.items);
    assert_eq!(before.patches, after.patches);
    assert_eq!(before.checkpoints, after.checkpoints);
    assert_eq!(before.batch_receipts, after.batch_receipts);
    let mut expected_dispatches = before.dispatches.clone();
    let dispatch = expected_dispatches
        .iter_mut()
        .find(|row| row.message_id == MESSAGE_ID)
        .expect("dispatch");
    dispatch.updated_at_ms = expected_bound_at;
    assert_eq!(expected_dispatches, after.dispatches);
    let mut expected_runs = before.runs.clone();
    let run = expected_runs
        .iter_mut()
        .find(|row| row.run_id == RUN_ID)
        .expect("run");
    run.lifecycle = AssistantRunLifecycle::Running;
    run.claim_token = None;
    run.provider_binding_version = Some(expected_version);
    run.provider_binding = Some(entities::OpaqueBytes::new(vec![0xab; 16]));
    run.provider_bound_at_ms = Some(expected_bound_at);
    run.updated_at_ms = expected_bound_at;
    assert_eq!(expected_runs, after.runs);
}

#[tokio::test]
async fn invalid_generations_rejected() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    let mut bad = receipt.clone();
    bad.generation = 0;
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &bad,
            &context,
            &ProviderBindingBytes::new(vec![9; 8]).expect("binding"),
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("generation 0");
    assert!(matches!(
        err,
        RunBindingError::InvalidGeneration { generation: 0 }
    ));
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = receipt.clone();
    bad.generation = -1;
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &bad,
            &context,
            &ProviderBindingBytes::new(vec![9; 8]).expect("binding"),
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("generation -1");
    assert!(matches!(
        err,
        RunBindingError::InvalidGeneration { generation: -1 }
    ));
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = receipt.clone();
    bad.generation = 2;
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &bad,
            &context,
            &ProviderBindingBytes::new(vec![9; 8]).expect("binding"),
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("mismatch 2");
    assert!(
        matches!(err, RunBindingError::CredentialMismatch { ref run_id } if run_id.as_str() == RUN_ID)
    );
    assert_eq!(before, persisted_rows(&database).await);
}

#[tokio::test]
async fn max_generation_binds_independent_of_attempt() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let row = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("find")
        .expect("run");
    let mut active = entities::assistant_run::ActiveModel::from(row);
    active.generation = Set(i64::MAX);
    active.update(&database).await.expect("update generation");
    let mut max_receipt = receipt.clone();
    max_receipt.generation = i64::MAX;
    let before = persisted_rows(&database).await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let outcome = repository
        .bind_run_provider(bind_command(
            &claimed,
            &max_receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("max generation should bind");
    let BindRunProviderOutcome::Bound(bound) = outcome else {
        panic!("bound")
    };
    assert_eq!(bound.generation, i64::MAX);
    let after = persisted_rows(&database).await;
    let dispatch = after
        .dispatches
        .iter()
        .find(|d| d.message_id == MESSAGE_ID)
        .unwrap();
    assert_eq!(dispatch.attempt_count, 1);
    let run = after.runs.iter().find(|r| r.run_id == RUN_ID).unwrap();
    assert_eq!(run.generation, i64::MAX);
    assert_eq!(run.provider_binding_version, Some(2));
    assert_two_row_delta(&before, &after, 2, BOUND_AT_MS);
}

#[tokio::test]
async fn invalid_attempts_rejected() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    let binding = ProviderBindingBytes::new(vec![9; 8]).expect("binding");
    for attempt in [0u32, 2_147_483_648u32, u32::MAX] {
        let mut bad_claim = replayable_claim(&claimed);
        bad_claim.attempt_count = attempt;
        let err = repository
            .bind_run_provider(BindRunProvider {
                claimed: &bad_claim,
                ..bind_command(
                    &claimed,
                    &receipt,
                    &context,
                    &binding,
                    2,
                    UnixMillis::from_millis(OPERATED_AT_MS),
                    UnixMillis::from_millis(BOUND_AT_MS),
                )
            })
            .await
            .expect_err("invalid attempt");
        assert!(
            matches!(err, RunBindingError::SnapshotMismatch { ref message_id } if message_id.as_str() == MESSAGE_ID)
        );
        assert_eq!(before, persisted_rows(&database).await);
    }
}

#[tokio::test]
async fn max_attempt_binds_independent_of_generation() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let dispatch_row = entities::message_dispatch::Entity::find_by_id(MESSAGE_ID)
        .one(&database)
        .await
        .expect("find")
        .expect("dispatch");
    let mut active = entities::message_dispatch::ActiveModel::from(dispatch_row);
    active.attempt_count = Set(i32::MAX);
    active.update(&database).await.expect("update attempt");
    let mut max_claim = replayable_claim(&claimed);
    max_claim.attempt_count = 2_147_483_647u32;
    let before = persisted_rows(&database).await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let outcome = repository
        .bind_run_provider(BindRunProvider {
            claimed: &max_claim,
            ..bind_command(
                &claimed,
                &receipt,
                &context,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            )
        })
        .await
        .expect("max attempt should bind");
    let BindRunProviderOutcome::Bound(bound) = outcome else {
        panic!("bound")
    };
    assert_eq!(bound.generation, 1);
    let after = persisted_rows(&database).await;
    let dispatch = after
        .dispatches
        .iter()
        .find(|d| d.message_id == MESSAGE_ID)
        .unwrap();
    assert_eq!(dispatch.attempt_count, i32::MAX);
    assert_two_row_delta(&before, &after, 2, BOUND_AT_MS);
}

#[tokio::test]
async fn fresh_run_owner_lease_rejected() {
    let (database, repository, claimed, receipt, _, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    let binding = ProviderBindingBytes::new(vec![9; 8]).expect("binding");
    let ctx_owner = LaunchContext {
        start_key: RunStartKey::new(START_KEY_BYTES),
        credentials: RunLaunchCredentials::new([0x99; 32], LEASE_BYTES, CLAIM_TOKEN_BYTES),
    };
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &ctx_owner,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("fresh owner");
    assert!(
        matches!(err, RunBindingError::CredentialMismatch { ref run_id } if run_id.as_str() == RUN_ID)
    );
    assert_eq!(before, persisted_rows(&database).await);
    let ctx_lease = LaunchContext {
        start_key: RunStartKey::new(START_KEY_BYTES),
        credentials: RunLaunchCredentials::new(OWNER_BYTES, [0x88; 32], CLAIM_TOKEN_BYTES),
    };
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &ctx_lease,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("fresh lease");
    assert!(
        matches!(err, RunBindingError::CredentialMismatch { ref run_id } if run_id.as_str() == RUN_ID)
    );
    assert_eq!(before, persisted_rows(&database).await);
}

#[tokio::test]
async fn replay_remaining_capabilities_rejected() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("first bind");
    let before = persisted_rows(&database).await;
    let replay_claimed = replayable_claim(&claimed);
    for (owner, lease) in [([0x01; 32], LEASE_BYTES), (OWNER_BYTES, [0x02; 32])] {
        let cred = RunLaunchCredentials::new(owner, lease, CLAIM_TOKEN_BYTES);
        let ctx = LaunchContext {
            start_key: RunStartKey::new(START_KEY_BYTES),
            credentials: cred,
        };
        let err = repository
            .bind_run_provider(bind_command(
                &replay_claimed,
                &receipt,
                &ctx,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            ))
            .await
            .expect_err("replay owner/lease");
        assert!(
            matches!(err, RunBindingError::CredentialMismatch { ref run_id } if run_id.as_str() == RUN_ID)
        );
        assert_eq!(before, persisted_rows(&database).await);
    }
    let bad_key_ctx = LaunchContext {
        start_key: RunStartKey::new([0xff; 32]),
        credentials: RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES),
    };
    let err = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &receipt,
            &bad_key_ctx,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("replay startkey");
    assert!(
        matches!(err, RunBindingError::CredentialMismatch { ref run_id } if run_id.as_str() == RUN_ID)
    );
    assert_eq!(before, persisted_rows(&database).await);
    let mut wrong_owner = replayable_claim(&claimed);
    wrong_owner.owner = artisan_database::DispatchLeaseOwner::new([0x22; 32]);
    let err = repository
        .bind_run_provider(BindRunProvider {
            claimed: &wrong_owner,
            ..bind_command(
                &replay_claimed,
                &receipt,
                &context,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            )
        })
        .await
        .expect_err("replay dispatch owner");
    assert!(matches!(
        err,
        RunBindingError::Repository(RepositoryError::DispatchOwnerMismatch { ref message_id }) if message_id.as_str() == MESSAGE_ID
    ));
    assert_eq!(before, persisted_rows(&database).await);
}

#[tokio::test]
async fn erased_claim_exact_receipt_and_rows() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let first = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("first");
    let BindRunProviderOutcome::Bound(first_receipt) = first else {
        panic!("bound")
    };
    let before = persisted_rows(&database).await;
    let replay_claimed = replayable_claim(&claimed);
    let different_claim = RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, [0xff; 32]);
    let different_ctx = LaunchContext {
        start_key: RunStartKey::new(START_KEY_BYTES),
        credentials: different_claim,
    };
    let replay = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &receipt,
            &different_ctx,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("erased claim replay");
    let BindRunProviderOutcome::AlreadyBound(second) = replay else {
        panic!("already")
    };
    assert_eq!(first_receipt, second);
    assert_eq!(before, persisted_rows(&database).await);
}

#[tokio::test]
async fn input_identity_tuples_rejected() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    let binding = ProviderBindingBytes::new(vec![9; 8]).expect("binding");
    let mut bad = receipt.clone();
    bad.message_id = MessageId::parse("message-zzz").expect("mid");
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &bad,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("message_id");
    assert!(
        matches!(err, RunBindingError::SnapshotMismatch { ref message_id } if message_id.as_str() == MESSAGE_ID)
    );
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = receipt.clone();
    bad.thread_id = ThreadId::parse("thread-zzz").expect("tid");
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &bad,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("thread_id");
    assert!(
        matches!(err, RunBindingError::IdentityConflict { reason } if reason == "stored run originates from another message or turn")
    );
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = receipt.clone();
    bad.turn_id = TurnId::parse("turn-zzz").expect("tid");
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &bad,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("turn_id");
    assert!(
        matches!(err, RunBindingError::IdentityConflict { reason } if reason == "stored run originates from another message or turn")
    );
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = receipt.clone();
    bad.run_id = RunId::parse("run-zzz").expect("rid");
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &bad,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("absent run");
    assert!(
        matches!(err, RunBindingError::RunNotFound { ref run_id } if run_id.as_str() == "run-zzz")
    );
    assert_eq!(before, persisted_rows(&database).await);
}

fn assert_snapshot_mismatch(err: &RunBindingError) {
    assert!(
        matches!(err, RunBindingError::SnapshotMismatch { message_id } if message_id.as_str() == MESSAGE_ID)
    );
}

fn assert_credential_mismatch(err: &RunBindingError) {
    assert!(
        matches!(err, RunBindingError::CredentialMismatch { run_id } if run_id.as_str() == RUN_ID)
    );
}

#[tokio::test]
async fn replay_identity_and_generation_rejected() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect("first");
    let before = persisted_rows(&database).await;
    let replay_claimed = replayable_claim(&claimed);
    let mut bad = receipt.clone();
    bad.message_id = MessageId::parse("message-zzz").expect("mid");
    let err = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &bad,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("replay message");
    assert_snapshot_mismatch(&err);
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = receipt.clone();
    bad.thread_id = ThreadId::parse("thread-zzz").expect("tid");
    let err = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &bad,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("replay thread");
    assert_credential_mismatch(&err);
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = receipt.clone();
    bad.turn_id = TurnId::parse("turn-zzz").expect("tid");
    let err = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &bad,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("replay turn");
    assert_credential_mismatch(&err);
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = receipt.clone();
    bad.generation = 2;
    let err = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &bad,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("replay generation");
    assert_credential_mismatch(&err);
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = receipt.clone();
    bad.run_id = RunId::parse("run-zzz").expect("rid");
    let err = repository
        .bind_run_provider(bind_command(
            &replay_claimed,
            &bad,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("replay absent");
    assert!(
        matches!(err, RunBindingError::RunNotFound { ref run_id } if run_id.as_str() == "run-zzz")
    );
    assert_eq!(before, persisted_rows(&database).await);
}

#[tokio::test]
async fn claim_snapshot_fields_rejected() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    let binding = ProviderBindingBytes::new(vec![9; 8]).expect("binding");
    let mut bad = replayable_claim(&claimed);
    bad.correlation_id = RequestId::parse("request-zzz").expect("req");
    let err = repository
        .bind_run_provider(BindRunProvider {
            claimed: &bad,
            ..bind_command(
                &claimed,
                &receipt,
                &context,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            )
        })
        .await
        .expect_err("correlation");
    assert_snapshot_mismatch(&err);
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = replayable_claim(&claimed);
    bad.attempt_count = 2;
    let err = repository
        .bind_run_provider(BindRunProvider {
            claimed: &bad,
            ..bind_command(
                &claimed,
                &receipt,
                &context,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            )
        })
        .await
        .expect_err("attempt2");
    assert_snapshot_mismatch(&err);
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = replayable_claim(&claimed);
    bad.queued_at = UnixMillis::from_millis(9999);
    let err = repository
        .bind_run_provider(BindRunProvider {
            claimed: &bad,
            ..bind_command(
                &claimed,
                &receipt,
                &context,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            )
        })
        .await
        .expect_err("queued");
    assert_snapshot_mismatch(&err);
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = replayable_claim(&claimed);
    bad.available_at = UnixMillis::from_millis(9999);
    let err = repository
        .bind_run_provider(BindRunProvider {
            claimed: &bad,
            ..bind_command(
                &claimed,
                &receipt,
                &context,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            )
        })
        .await
        .expect_err("available");
    assert_snapshot_mismatch(&err);
    assert_eq!(before, persisted_rows(&database).await);
    let mut bad = replayable_claim(&claimed);
    bad.lease_expires_at = UnixMillis::from_millis(601);
    let err = repository
        .bind_run_provider(BindRunProvider {
            claimed: &bad,
            ..bind_command(
                &claimed,
                &receipt,
                &context,
                &binding,
                2,
                UnixMillis::from_millis(OPERATED_AT_MS),
                UnixMillis::from_millis(BOUND_AT_MS),
            )
        })
        .await
        .expect_err("expiry 601");
    assert_snapshot_mismatch(&err);
    assert_eq!(before, persisted_rows(&database).await);
}

#[tokio::test]
async fn exact_launch_stamp_rejected() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let before = persisted_rows(&database).await;
    let binding = ProviderBindingBytes::new(vec![9; 8]).expect("binding");
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(149),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("launch 149");
    assert!(
        matches!(err, RunBindingError::SnapshotMismatch { ref message_id } if message_id.as_str() == MESSAGE_ID)
    );
    assert_eq!(before, persisted_rows(&database).await);
    let dispatch_row = entities::message_dispatch::Entity::find_by_id(MESSAGE_ID)
        .one(&database)
        .await
        .expect("find")
        .expect("dispatch");
    let mut active = entities::message_dispatch::ActiveModel::from(dispatch_row);
    active.updated_at_ms = Set(151);
    active.update(&database).await.expect("update");
    let before2 = persisted_rows(&database).await;
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("dispatch 151");
    assert!(
        matches!(err, RunBindingError::SnapshotMismatch { ref message_id } if message_id.as_str() == MESSAGE_ID)
    );
    assert_eq!(before2, persisted_rows(&database).await);
}

#[tokio::test]
async fn run_stamp_second_fence_rollback() {
    let (database, repository, claimed, receipt, context, _) = seeded_repository().await;
    let binding = ProviderBindingBytes::new(vec![9; 8]).expect("binding");
    let run_row = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("find")
        .expect("run");
    let mut active = entities::assistant_run::ActiveModel::from(run_row);
    active.created_at_ms = Set(149);
    active.update(&database).await.expect("update");
    let before = persisted_rows(&database).await;
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("created 149");
    assert!(
        matches!(err, RunBindingError::SnapshotMismatch { ref message_id } if message_id.as_str() == MESSAGE_ID)
    );
    assert_eq!(before, persisted_rows(&database).await);
    let run_row = entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("find")
        .expect("run");
    let mut active = entities::assistant_run::ActiveModel::from(run_row);
    active.created_at_ms = Set(OPERATED_AT_MS);
    active.updated_at_ms = Set(151);
    active.update(&database).await.expect("update");
    let before = persisted_rows(&database).await;
    let err = repository
        .bind_run_provider(bind_command(
            &claimed,
            &receipt,
            &context,
            &binding,
            2,
            UnixMillis::from_millis(OPERATED_AT_MS),
            UnixMillis::from_millis(BOUND_AT_MS),
        ))
        .await
        .expect_err("updated 151");
    assert!(
        matches!(err, RunBindingError::SnapshotMismatch { ref message_id } if message_id.as_str() == MESSAGE_ID)
    );
    assert_eq!(before, persisted_rows(&database).await);
    let dispatch = persisted_rows(&database).await.dispatches[0].clone();
    assert_eq!(dispatch.updated_at_ms, OPERATED_AT_MS);
}
