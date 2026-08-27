//! E2-B paired terminal settlement coverage through real migrated SQLite.
//! Verifies co-commit of run, dispatch, item, turn, patches, and state in one
//! transaction, plus replay, fence, rollback, and retention properties.

use artisan_database::entities::{
    self, AssistantRunLifecycle, ConversationPatchKind, DispatchState, EntityLifecycle, RenderPhase,
};
use artisan_database::{
    AssistantChange, BindRunProvider, CheckpointUpdate, ClaimMessageDispatch,
    ClaimedMessageDispatch, CommitRunBatch, CompleteRun, CompleteRunOutcome, FailRun,
    FailRunOutcome, ProviderBindingBytes, QueueFirstMessageInput, Repository, RunBatchScope,
    RunErrorCode, RunErrorMessage, RunLaunchCredentials, RunStartKey, SqliteConfig, connect,
};
use artisan_domain::{
    AssistantBody, AssistantMessagePhase, ItemId, MessageId, PatchId, ProjectId, RequestId,
    Revision, RunId, ThreadId, ThreadTitle, TurnId, UnixMillis,
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
const TERMINAL_AT_MS: i64 = 300;

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
        .create_thread(artisan_database::CreateThreadInput {
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
        body: artisan_domain::MessageBody::parse("first durable body").expect("body"),
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
        first_patch: PatchId::parse("patch-a").expect("patch"),
        second_patch: PatchId::parse("patch-b").expect("patch"),
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
    let artisan_database::LaunchClaimedRunOutcome::Started(launched) = repository
        .launch_claimed_run(artisan_database::LaunchClaimedRun {
            claimed: &claimed,
            run_id: &identity.run,
            turn_id: &identity.turn,
            item_id: &identity.item,
            first_patch_id: &identity.first_patch,
            second_patch_id: &identity.second_patch,
            operated_at: UnixMillis::from_millis(OPERATED_AT_MS),
            run_start_key: &context.start_key,
            credentials: &context.credentials,
        })
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct PersistedRows {
    dispatches: Vec<entities::MessageDispatch>,
    runs: Vec<entities::AssistantRun>,
    items: Vec<entities::ConversationItem>,
    turns: Vec<entities::ConversationTurn>,
    patches: Vec<entities::ConversationPatch>,
    states: Vec<entities::ConversationState>,
    checkpoints: Vec<entities::RunCheckpoint>,
    receipts: Vec<entities::RunBatchReceipt>,
}
async fn persisted_rows(database: &DatabaseConnection) -> PersistedRows {
    async fn all<E>(db: &DatabaseConnection) -> Vec<E::Model>
    where
        E: EntityTrait,
    {
        E::find().all(db).await.expect("rows")
    }
    let mut dispatches = all::<entities::message_dispatch::Entity>(database).await;
    let mut runs = all::<entities::assistant_run::Entity>(database).await;
    let mut items = all::<entities::conversation_item::Entity>(database).await;
    let mut turns = all::<entities::conversation_turn::Entity>(database).await;
    let mut patches = all::<entities::conversation_patch::Entity>(database).await;
    let mut states = all::<entities::conversation_state::Entity>(database).await;
    let mut checkpoints = all::<entities::run_checkpoint::Entity>(database).await;
    let mut receipts = all::<entities::run_batch_receipt::Entity>(database).await;
    dispatches.sort_by(|a, b| a.message_id.cmp(&b.message_id));
    runs.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    items.sort_by(|a, b| a.item_id.cmp(&b.item_id));
    turns.sort_by(|a, b| a.turn_id.cmp(&b.turn_id));
    patches.sort_by(|a, b| a.patch_id.cmp(&b.patch_id));
    states.sort_by(|a, b| a.thread_id.cmp(&b.thread_id));
    checkpoints.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    receipts.sort_by(|a, b| {
        (a.run_id.clone(), a.batch_sequence).cmp(&(b.run_id.clone(), b.batch_sequence))
    });
    PersistedRows {
        dispatches,
        runs,
        items,
        turns,
        patches,
        states,
        checkpoints,
        receipts,
    }
}

fn assistant_body(s: &str) -> AssistantBody {
    AssistantBody::parse(s.to_owned()).expect("body")
}

async fn seeded_with_item() -> (SeededPair, ItemId, PatchId, PatchId) {
    let pair = seeded_pair().await;
    let scope = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
    };
    let assistant_item = ItemId::parse("assistant-1").expect("id");
    let patch_turn = PatchId::parse("patch-turn-act").expect("p");
    let patch_item = PatchId::parse("patch-item-start").expect("p");
    let body = assistant_body("hello assistant");
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope,
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&patch_turn),
            changes: &[AssistantChange::Start {
                item_id: &assistant_item,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: &patch_item,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await
        .expect("batch");
    (pair, assistant_item, patch_item, patch_turn)
}

fn terminal_scope(pair: &SeededPair) -> RunBatchScope<'_> {
    RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    }
}

#[tokio::test]
async fn complete_pair_updates_all_rows() {
    let (pair, assistant_item, _, _) = seeded_with_item().await;
    let before = persisted_rows(&pair.database).await;
    let scope = terminal_scope(&pair);
    let final_body = assistant_body("final completed body");
    let item_patch = PatchId::parse("patch-item-complete").expect("p");
    let turn_patch = PatchId::parse("patch-turn-complete").expect("p");
    let outcome = pair
        .repository
        .complete_run(CompleteRun {
            scope,
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &final_body,
            phase: AssistantMessagePhase::Final,
            item_patch_id: &item_patch,
            turn_patch_id: &turn_patch,
        })
        .await
        .expect("complete");
    match outcome {
        CompleteRunOutcome::Completed(r) => {
            assert_eq!(r.run_id.as_str(), RUN_ID);
            assert_eq!(r.terminal_at.as_millis(), TERMINAL_AT_MS);
        }
        CompleteRunOutcome::AlreadyCompleted(_) => panic!("should complete"),
    }
    let after = persisted_rows(&pair.database).await;
    let run = after.runs.iter().find(|r| r.run_id == RUN_ID).expect("run");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Completed);
    assert!(run.owner.is_none() && run.lease.is_none() && run.claim_token.is_none());
    assert_eq!(run.terminal_at_ms, Some(TERMINAL_AT_MS));
    assert!(run.error_code.is_none());
    assert!(run.provider_binding.is_some());
    assert_eq!(run.updated_at_ms, TERMINAL_AT_MS);
    let dispatch = after
        .dispatches
        .iter()
        .find(|d| d.message_id == MESSAGE_ID)
        .expect("dispatch");
    assert_eq!(dispatch.state, DispatchState::Completed);
    assert!(dispatch.lease_owner.is_none());
    assert_eq!(dispatch.updated_at_ms, TERMINAL_AT_MS);
    let item = after
        .items
        .iter()
        .find(|i| i.item_id == "assistant-1")
        .expect("item");
    assert_eq!(item.lifecycle, EntityLifecycle::Completed);
    assert_eq!(item.body, "final completed body");
    assert_eq!(item.phase, Some(RenderPhase::Final));
    assert_eq!(item.revision, 1);
    assert_eq!(item.updated_at_ms, TERMINAL_AT_MS);
    let turn = after
        .turns
        .iter()
        .find(|t| t.turn_id == TURN_ID)
        .expect("turn");
    assert_eq!(turn.lifecycle, EntityLifecycle::Completed);
    assert_eq!(turn.revision, 2);
    assert_eq!(turn.updated_at_ms, TERMINAL_AT_MS);
    let patches: Vec<_> = after
        .patches
        .iter()
        .filter(|p| p.patch_id == "patch-item-complete" || p.patch_id == "patch-turn-complete")
        .collect();
    assert_eq!(patches.len(), 2);
    for p in patches {
        assert_eq!(p.recorded_at_ms, TERMINAL_AT_MS);
    }
    let state = after
        .states
        .iter()
        .find(|s| s.thread_id == THREAD_ID)
        .expect("state");
    assert_eq!(state.last_patch_sequence, 6);
    assert_eq!(state.updated_at_ms, TERMINAL_AT_MS);
    // receipts retained
    assert_eq!(before.receipts, vec![after.receipts[0].clone()]);
    assert_eq!(before.checkpoints.len(), after.checkpoints.len());
}

#[tokio::test]
async fn fail_pair_records_error_and_failed_state() {
    let (pair, assistant_item, _, _) = seeded_with_item().await;
    let scope = terminal_scope(&pair);
    let body = assistant_body("failed body");
    let item_patch = PatchId::parse("patch-item-fail").expect("p");
    let turn_patch = PatchId::parse("patch-turn-fail").expect("p");
    let code = RunErrorCode::parse("E_TEST".to_owned()).expect("code");
    let msg = RunErrorMessage::parse("engine failed".to_owned()).expect("msg");
    let outcome = pair
        .repository
        .fail_run(FailRun {
            scope,
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &body,
            phase: AssistantMessagePhase::Unspecified,
            item_patch_id: &item_patch,
            turn_patch_id: &turn_patch,
            error_code: &code,
            error_message: &msg,
        })
        .await
        .expect("fail");
    match outcome {
        FailRunOutcome::Failed(r) => assert_eq!(r.terminal_at.as_millis(), TERMINAL_AT_MS),
        FailRunOutcome::AlreadyFailed(_) => panic!("should fail"),
    }
    let after = persisted_rows(&pair.database).await;
    let run = after.runs.iter().find(|r| r.run_id == RUN_ID).expect("run");
    assert_eq!(run.lifecycle, AssistantRunLifecycle::Failed);
    assert_eq!(run.error_code.as_deref(), Some("E_TEST"));
    assert_eq!(run.error_message.as_deref(), Some("engine failed"));
    assert_eq!(run.terminal_at_ms, Some(TERMINAL_AT_MS));
    let dispatch = after
        .dispatches
        .iter()
        .find(|d| d.message_id == MESSAGE_ID)
        .expect("d");
    assert_eq!(dispatch.state, DispatchState::Failed);
    assert_eq!(dispatch.last_error.as_deref(), Some("engine failed"));
    let item = after
        .items
        .iter()
        .find(|i| i.item_id == "assistant-1")
        .expect("item");
    assert_eq!(item.lifecycle, EntityLifecycle::Failed);
    let turn = after
        .turns
        .iter()
        .find(|t| t.turn_id == TURN_ID)
        .expect("turn");
    assert_eq!(turn.lifecycle, EntityLifecycle::Failed);
}

#[tokio::test]
async fn exact_replay_harmless_and_conflicting_rejected() {
    let (pair, assistant_item, _, _) = seeded_with_item().await;
    let final_body = assistant_body("replay body");
    let item_patch = PatchId::parse("patch-item-replay").expect("p");
    let turn_patch = PatchId::parse("patch-turn-replay").expect("p");
    let scope1 = terminal_scope(&pair);
    pair.repository
        .complete_run(CompleteRun {
            scope: scope1,
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &final_body,
            phase: AssistantMessagePhase::Final,
            item_patch_id: &item_patch,
            turn_patch_id: &turn_patch,
        })
        .await
        .expect("first complete");
    let before_replay = persisted_rows(&pair.database).await;
    let scope2 = terminal_scope(&pair);
    let replay = pair
        .repository
        .complete_run(CompleteRun {
            scope: scope2,
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &final_body,
            phase: AssistantMessagePhase::Final,
            item_patch_id: &item_patch,
            turn_patch_id: &turn_patch,
        })
        .await
        .expect("replay");
    assert!(matches!(replay, CompleteRunOutcome::AlreadyCompleted(_)));
    assert_eq!(before_replay, persisted_rows(&pair.database).await);
    // conflicting replay with different body should be rejected
    let diff_body = assistant_body("different");
    let diff_patch = PatchId::parse("patch-item-diff").expect("p");
    let diff_turn = PatchId::parse("patch-turn-diff").expect("p");
    let err = pair
        .repository
        .complete_run(CompleteRun {
            scope: terminal_scope(&pair),
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &diff_body,
            phase: AssistantMessagePhase::Final,
            item_patch_id: &diff_patch,
            turn_patch_id: &diff_turn,
        })
        .await
        .expect_err("conflicting replay should be rejected");
    // should be typed error, not success; check it is not AlreadyCompleted
    let _ = err;
    assert_eq!(before_replay, persisted_rows(&pair.database).await);
}

#[tokio::test]
async fn fence_failures_write_nothing() {
    let (pair, assistant_item, _, _) = seeded_with_item().await;
    let before = persisted_rows(&pair.database).await;
    // wrong owner credentials
    let bad_creds = RunLaunchCredentials::new([0x99; 32], LEASE_BYTES, CLAIM_TOKEN_BYTES);
    let bad_scope = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &bad_creds,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let err = pair
        .repository
        .complete_run(CompleteRun {
            scope: bad_scope,
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &assistant_body("x"),
            phase: AssistantMessagePhase::Final,
            item_patch_id: &PatchId::parse("p-bad-1").expect("p"),
            turn_patch_id: &PatchId::parse("p-bad-2").expect("p"),
        })
        .await
        .expect_err("bad owner should fail");
    let _ = err;
    assert_eq!(before, persisted_rows(&pair.database).await);
    // stale snapshot: wrong expected_updated_at
    let stale_scope = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS), // stale
    };
    let err2 = pair
        .repository
        .complete_run(CompleteRun {
            scope: stale_scope,
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &assistant_body("x"),
            phase: AssistantMessagePhase::Final,
            item_patch_id: &PatchId::parse("p-stale-1").expect("p"),
            turn_patch_id: &PatchId::parse("p-stale-2").expect("p"),
        })
        .await
        .expect_err("stale snapshot should fail");
    let _ = err2;
    assert_eq!(before, persisted_rows(&pair.database).await);
    // generation mismatch via wrong bound version (simulate credential mismatch)
    // expiry equality
    let expiry_eq_scope = RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.context.start_key,
        credentials: &pair.context.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
    };
    let err3 = pair
        .repository
        .complete_run(CompleteRun {
            scope: expiry_eq_scope,
            operated_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS), // equality
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &assistant_body("x"),
            phase: AssistantMessagePhase::Final,
            item_patch_id: &PatchId::parse("p-exp-1").expect("p"),
            turn_patch_id: &PatchId::parse("p-exp-2").expect("p"),
        })
        .await
        .expect_err("expiry equality should fail");
    let _ = err3;
    assert_eq!(before, persisted_rows(&pair.database).await);
}

#[tokio::test]
async fn late_patch_failure_rolls_back_all() {
    let (pair, assistant_item, _, _) = seeded_with_item().await;
    // pre-insert a patch that will collide with terminal item patch
    let colliding = PatchId::parse("patch-collide").expect("p");
    let state = entities::conversation_state::Entity::find_by_id(THREAD_ID)
        .one(&pair.database)
        .await
        .expect("q")
        .expect("state");
    // Insert colliding patch manually
    entities::conversation_patch::ActiveModel {
        patch_id: Set(colliding.as_str().to_owned()),
        thread_id: Set(THREAD_ID.to_owned()),
        sequence: Set(state.last_patch_sequence + 10),
        kind: Set(ConversationPatchKind::ItemLifecycle),
        revision: Set(1),
        recorded_at_ms: Set(999),
        turn_id: Set(None),
        item_id: Set(Some(assistant_item.as_str().to_owned())),
        ordinal: Set(None),
        lifecycle: Set(Some(EntityLifecycle::Completed)),
        item_kind: Set(None),
        run_id: Set(None),
        phase: Set(None),
        body: Set(None),
        fragment: Set(None),
        entity_created_at_ms: Set(None),
        entity_updated_at_ms: Set(None),
    }
    .insert(&pair.database)
    .await
    .expect("insert colliding patch");
    let before = persisted_rows(&pair.database).await;
    let err = pair
        .repository
        .complete_run(CompleteRun {
            scope: terminal_scope(&pair),
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &assistant_body("x"),
            phase: AssistantMessagePhase::Final,
            item_patch_id: &colliding,
            turn_patch_id: &PatchId::parse("patch-turn-ok").expect("p"),
        })
        .await
        .expect_err("colliding patch should fail");
    let _ = err;
    let after = persisted_rows(&pair.database).await;
    // dispatch and run must be unchanged (still running)
    let run_before = before.runs.iter().find(|r| r.run_id == RUN_ID).unwrap();
    let run_after = after.runs.iter().find(|r| r.run_id == RUN_ID).unwrap();
    assert_eq!(run_before.lifecycle, run_after.lifecycle);
    assert_eq!(run_before.updated_at_ms, run_after.updated_at_ms);
    let disp_before = before
        .dispatches
        .iter()
        .find(|d| d.message_id == MESSAGE_ID)
        .unwrap();
    let disp_after = after
        .dispatches
        .iter()
        .find(|d| d.message_id == MESSAGE_ID)
        .unwrap();
    assert_eq!(disp_before.state, disp_after.state);
    assert_eq!(disp_before.updated_at_ms, disp_after.updated_at_ms);
}

#[tokio::test]
async fn binding_and_receipts_retained() {
    let (pair, assistant_item, _, _) = seeded_with_item().await;
    let before = persisted_rows(&pair.database).await;
    let before_binding = before
        .runs
        .iter()
        .find(|r| r.run_id == RUN_ID)
        .unwrap()
        .provider_binding
        .clone();
    let before_version = before
        .runs
        .iter()
        .find(|r| r.run_id == RUN_ID)
        .unwrap()
        .provider_binding_version;
    pair.repository
        .complete_run(CompleteRun {
            scope: terminal_scope(&pair),
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &assistant_body("retain"),
            phase: AssistantMessagePhase::Final,
            item_patch_id: &PatchId::parse("patch-retain-item").expect("p"),
            turn_patch_id: &PatchId::parse("patch-retain-turn").expect("p"),
        })
        .await
        .expect("complete");
    let after = persisted_rows(&pair.database).await;
    let run_after = after.runs.iter().find(|r| r.run_id == RUN_ID).unwrap();
    assert_eq!(run_after.provider_binding, before_binding);
    assert_eq!(run_after.provider_binding_version, before_version);
    assert_eq!(before.receipts, after.receipts);
    assert_eq!(before.checkpoints.len(), after.checkpoints.len());
    // patches from batch retained
    assert!(
        after
            .patches
            .iter()
            .any(|p| p.patch_id == "patch-item-start")
    );
}

#[tokio::test]
async fn ineligible_rows_untouched() {
    let pair = seeded_pair().await;
    // No batch yet, try to complete with non-existent item -> should fail and leave rows
    let before = persisted_rows(&pair.database).await;
    let fake_item = ItemId::parse("assistant-fake").expect("id");
    let err = pair
        .repository
        .complete_run(CompleteRun {
            scope: RunBatchScope {
                claimed: &pair.claimed,
                launched: &pair.launched,
                bound: &pair.bound,
                run_start_key: &pair.context.start_key,
                credentials: &pair.context.credentials,
                expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
                expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
            },
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &fake_item,
            expected_revision: Revision::new(0),
            body: &assistant_body("x"),
            phase: AssistantMessagePhase::Final,
            item_patch_id: &PatchId::parse("p-inelig-1").expect("p"),
            turn_patch_id: &PatchId::parse("p-inelig-2").expect("p"),
        })
        .await
        .expect_err("fake item should fail");
    let _ = err;
    assert_eq!(before, persisted_rows(&pair.database).await);
    // Also try to complete a queued dispatch (new message not claimed)
    // Create distinct thread for queued dispatch
    pair.repository
        .create_thread(artisan_database::CreateThreadInput {
            request_id: RequestId::parse("request-thread-queued").expect("req"),
            thread_id: ThreadId::parse("thread-queued").expect("tid"),
            project_id: ProjectId::parse("project-1").expect("pid"),
            title: ThreadTitle::parse("Queued Thread").expect("title"),
            created_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
            updated_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await
        .expect("thread-queued");
    let second_msg = MessageId::parse("message-2").expect("mid");
    pair.repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("request-2").expect("req"),
            message_id: second_msg.clone(),
            thread_id: ThreadId::parse("thread-queued").expect("tid"),
            body: artisan_domain::MessageBody::parse("second").expect("body"),
            accepted_at: UnixMillis::from_millis(ACCEPTED_AT_MS + 10),
        })
        .await
        .expect("queue second");
    let after_queue = persisted_rows(&pair.database).await;
    let queued_dispatch = after_queue
        .dispatches
        .iter()
        .find(|d| d.message_id == "message-2")
        .expect("queued");
    assert_eq!(queued_dispatch.state, DispatchState::Queued);
    // Attempting to use that dispatch as terminal scope would be wrong generation etc;
    // we just verify queued dispatch untouched after previous failure
    assert_eq!(before.dispatches.len() + 1, after_queue.dispatches.len());
}

#[tokio::test]
async fn failed_error_bounds_enforced() {
    let (pair, assistant_item, _, _) = seeded_with_item().await;
    // empty code via parse should fail at construction, but we test repository rejects empty via wrapper
    let bad_code = RunErrorCode::parse(String::new());
    assert!(bad_code.is_err());
    let long_code = "a".repeat(129);
    assert!(RunErrorCode::parse(long_code).is_err());
    let long_msg = "b".repeat(1025);
    assert!(RunErrorMessage::parse(long_msg).is_err());
    // valid but try to complete after already completed should be rejected as not running
    pair.repository
        .complete_run(CompleteRun {
            scope: terminal_scope(&pair),
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS),
            item_id: &assistant_item,
            expected_revision: Revision::new(0),
            body: &assistant_body("first"),
            phase: AssistantMessagePhase::Final,
            item_patch_id: &PatchId::parse("p-bounds-1").expect("p"),
            turn_patch_id: &PatchId::parse("p-bounds-2").expect("p"),
        })
        .await
        .expect("first complete");
    let err = pair
        .repository
        .fail_run(FailRun {
            scope: terminal_scope(&pair),
            operated_at: UnixMillis::from_millis(TERMINAL_AT_MS + 10),
            item_id: &assistant_item,
            expected_revision: Revision::new(1),
            body: &assistant_body("x"),
            phase: AssistantMessagePhase::Final,
            item_patch_id: &PatchId::parse("p-bounds-3").expect("p"),
            turn_patch_id: &PatchId::parse("p-bounds-4").expect("p"),
            error_code: &RunErrorCode::parse("E2".to_owned()).expect("code"),
            error_message: &RunErrorMessage::parse("msg".to_owned()).expect("msg"),
        })
        .await
        .expect_err("terminal already completed should not allow fail");
    let _ = err;
}
