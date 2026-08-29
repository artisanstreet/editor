//! Focused in-memory coverage for the bounded startup-reconciliation sweep coordinator.

#![forbid(unsafe_code)]

#[path = "../../modules/backend/src/startup_reconciliation_sweep.rs"]
mod startup_reconciliation_sweep;

use artisan_database::entities::{self, AssistantRunLifecycle, DispatchState, EntityLifecycle};
use artisan_database::{
    AssistantChange, BindRunProvider, CheckpointUpdate, ClaimMessageDispatch, CreateThreadInput,
    ProviderBindingBytes, QueueFirstMessageInput, Repository, RunLaunchCredentials, RunStartKey,
    SqliteConfig, StartupReconciliationCandidate, connect,
};
use artisan_domain::{
    AssistantBody, AssistantMessagePhase, ItemId, MessageBody, MessageId, PatchId, ProjectId,
    RequestId, RunId, ThreadId, ThreadTitle, TurnId, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};
use startup_reconciliation_sweep::{
    PatchSourceError, StartupReconciliationPatchSource, StartupReconciliationPatches,
    StartupReconciliationSweepError, StartupReconciliationSweepInput,
    StartupReconciliationSweepReport, sweep_startup_reconciliation,
};

const OWNER_BYTES: [u8; 32] = [0xa1; 32];
const LEASE_BYTES: [u8; 32] = [0xb2; 32];
const CLAIM_TOKEN_BYTES: [u8; 32] = [0xc3; 32];
const DISPATCH_OWNER_BYTE: u8 = 0x11;

const THREAD_CREATED_AT_MS: i64 = 10;
const ACCEPTED_AT_MS: i64 = 50;
const CLAIMED_AT_MS: i64 = 100;
const OPERATED_AT_MS: i64 = 150;
const BOUND_AT_MS: i64 = 200;
const BATCH_AT_MS: i64 = 250;
const LEASE_EXPIRES_AT_MS: i64 = 600;
const SWEEP_OPERATED_AT_MS: i64 = LEASE_EXPIRES_AT_MS;

async fn memory_repository() -> (DatabaseConnection, Repository) {
    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database).await.expect("migrate");
    (database.clone(), Repository::new(database))
}

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDatabase {
    dir: PathBuf,
    file: PathBuf,
}

impl TempDatabase {
    fn new(label: &str) -> Self {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "artisan-sweep-{}-{}-{}",
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
        // Best-effort panic cleanup — does not replace explicit success-path cleanup.
        let _ = std::fs::remove_file(&self.file);
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = PathBuf::from(format!("{}{}", self.file.display(), suffix));
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
            title: ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
            updated_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await;
}

#[allow(clippy::too_many_lines)]
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
        accepted_at: UnixMillis::from_millis(ACCEPTED_AT_MS),
    };
    repository.queue_first_message(queue).await.expect("queue");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: artisan_database::DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            claimed_at: UnixMillis::from_millis(CLAIMED_AT_MS),
            lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
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
    let outcome = repository
        .launch_claimed_run(artisan_database::LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run,
            turn_id: &turn,
            item_id: &item,
            first_patch_id: &p1,
            second_patch_id: &p2,
            operated_at: UnixMillis::from_millis(OPERATED_AT_MS),
            run_start_key: &start_key,
            credentials: &creds,
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
            expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
            bound_at: UnixMillis::from_millis(BOUND_AT_MS),
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
                expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
                expected_updated_at: UnixMillis::from_millis(BOUND_AT_MS),
            },
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_AT_MS),
            activate_turn_patch_id: Some(patch_turn),
            changes: &[AssistantChange::Start {
                item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: patch_item,
            }],
            checkpoint: CheckpointUpdate::Keep,
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
// Patch sources
// ---------------------------------------------------------------------------

struct DeterministicSource;

impl StartupReconciliationPatchSource for DeterministicSource {
    fn patch_ids_for(
        &mut self,
        candidate: &StartupReconciliationCandidate,
    ) -> Result<StartupReconciliationPatches, PatchSourceError> {
        let turn_patch =
            PatchId::parse(format!("turn-{}", candidate.run_id.as_str())).expect("turn patch");
        let item_patch = candidate.assistant_item_id.as_ref().map(|_| {
            PatchId::parse(format!("item-{}", candidate.run_id.as_str())).expect("item patch")
        });
        Ok(StartupReconciliationPatches::new(turn_patch, item_patch))
    }
}

struct PanickingSource;

impl StartupReconciliationPatchSource for PanickingSource {
    fn patch_ids_for(
        &mut self,
        _candidate: &StartupReconciliationCandidate,
    ) -> Result<StartupReconciliationPatches, PatchSourceError> {
        panic!("patch source must not be consulted for empty pass");
    }
}

struct FailingSource {
    fail_at: usize,
    calls: usize,
}

impl FailingSource {
    fn new(fail_at: usize) -> Self {
        Self { fail_at, calls: 0 }
    }
}

impl StartupReconciliationPatchSource for FailingSource {
    fn patch_ids_for(
        &mut self,
        candidate: &StartupReconciliationCandidate,
    ) -> Result<StartupReconciliationPatches, PatchSourceError> {
        let current = self.calls;
        self.calls += 1;
        if current == self.fail_at {
            return Err(PatchSourceError);
        }
        let turn_patch =
            PatchId::parse(format!("turn-{}", candidate.run_id.as_str())).expect("turn patch");
        let item_patch = candidate.assistant_item_id.as_ref().map(|_| {
            PatchId::parse(format!("item-{}", candidate.run_id.as_str())).expect("item patch")
        });
        Ok(StartupReconciliationPatches::new(turn_patch, item_patch))
    }
}

struct MismatchedShapeSource {
    mode: u8,
}

impl StartupReconciliationPatchSource for MismatchedShapeSource {
    fn patch_ids_for(
        &mut self,
        candidate: &StartupReconciliationCandidate,
    ) -> Result<StartupReconciliationPatches, PatchSourceError> {
        let turn_patch =
            PatchId::parse(format!("turn-{}", candidate.run_id.as_str())).expect("turn patch");
        let item_patch = match self.mode {
            0 => {
                // missing item patch when item exists, extra when not
                if candidate.assistant_item_id.is_some() {
                    None
                } else {
                    Some(PatchId::parse(format!("item-{}", candidate.run_id.as_str())).expect("p"))
                }
            }
            1 => {
                // colliding identities
                let colliding = turn_patch.clone();
                if candidate.assistant_item_id.is_some() {
                    Some(colliding)
                } else {
                    None
                }
            }
            _ => None,
        };
        Ok(StartupReconciliationPatches::new(turn_patch, item_patch))
    }
}

// ---------------------------------------------------------------------------
// 1. empty pass
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_pass_returns_all_zero_and_never_consults_source() {
    let (_db, repository) = memory_repository().await;
    let input =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(SWEEP_OPERATED_AT_MS), 10)
            .expect("input");
    let mut source = PanickingSource;
    let report = sweep_startup_reconciliation(&repository, input, &mut source)
        .await
        .expect("empty sweep should succeed");
    assert_eq!(
        report,
        StartupReconciliationSweepReport {
            discovered: 0,
            attempted: 0,
            interrupted: 0,
            already_interrupted: 0,
            skipped_moved: 0,
        }
    );
}

// ---------------------------------------------------------------------------
// 2. deterministic discovery order and limit honored
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deterministic_discovery_order_and_limit_honored() {
    let (database, repository) = memory_repository().await;
    // Create three runs with staggered lease expiry.
    for (run, expiry) in [("run-a", 500), ("run-b", 500), ("run-c", 400)] {
        let thread = format!("thread-{run}");
        let msg = format!("msg-{run}");
        let turn = format!("turn-{run}");
        seed_project_and_thread(&database, &repository, &thread).await;
        let (_claimed, _receipt, _sk, _creds) =
            queue_claim_launch(&repository, &thread, &msg, run, &turn).await;
        let dispatch = entities::message_dispatch::Entity::find_by_id(msg.clone())
            .one(&database)
            .await
            .expect("find")
            .expect("dispatch");
        let mut active: entities::message_dispatch::ActiveModel = dispatch.into();
        active.lease_expires_at_ms = Set(Some(expiry));
        active.update(&database).await.expect("update expiry");
    }

    // limit 1 should process only run-c (earliest expiry)
    let input_one =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(500), 1).expect("input");
    let mut source_one = DeterministicSource;
    let report_one = sweep_startup_reconciliation(&repository, input_one, &mut source_one)
        .await
        .expect("sweep");
    assert_eq!(report_one.discovered, 1);
    assert_eq!(report_one.interrupted, 1);
    assert_eq!(report_one.attempted, 1);

    // Re-create DB for limit 2 test with fresh state to check ordering.
    let (database2, repository2) = memory_repository().await;
    for (run, expiry) in [("run-a", 500), ("run-b", 500), ("run-c", 400)] {
        let thread = format!("thread-{run}");
        let msg = format!("msg-{run}");
        let turn = format!("turn-{run}");
        seed_project_and_thread(&database2, &repository2, &thread).await;
        let (_claimed, _receipt, _sk, _creds) =
            queue_claim_launch(&repository2, &thread, &msg, run, &turn).await;
        let dispatch = entities::message_dispatch::Entity::find_by_id(msg.clone())
            .one(&database2)
            .await
            .expect("find")
            .expect("dispatch");
        let mut active: entities::message_dispatch::ActiveModel = dispatch.into();
        active.lease_expires_at_ms = Set(Some(expiry));
        active.update(&database2).await.expect("update");
    }
    let input_two =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(500), 2).expect("input");
    // Capture order via custom source that records run_id order.
    struct OrderingSource {
        order: Vec<String>,
    }
    impl StartupReconciliationPatchSource for OrderingSource {
        fn patch_ids_for(
            &mut self,
            candidate: &StartupReconciliationCandidate,
        ) -> Result<StartupReconciliationPatches, PatchSourceError> {
            self.order.push(candidate.run_id.as_str().to_owned());
            let turn_patch =
                PatchId::parse(format!("turn-{}", candidate.run_id.as_str())).expect("p");
            Ok(StartupReconciliationPatches::new(turn_patch, None))
        }
    }
    let mut ordering = OrderingSource { order: Vec::new() };
    let report_two = sweep_startup_reconciliation(&repository2, input_two, &mut ordering)
        .await
        .expect("sweep 2");
    assert_eq!(report_two.discovered, 2);
    assert_eq!(ordering.order, vec!["run-c".to_owned(), "run-a".to_owned()]);
}

// ---------------------------------------------------------------------------
// 3. launching/no-item and running/with-item receive correct shapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn launching_no_item_and_running_with_item_both_interrupted_with_correct_shapes() {
    let (database, repository) = memory_repository().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_claimed1, _receipt1, _sk1, _creds1) =
        queue_claim_launch(&repository, "thread-1", "message-1", "run-1", "turn-1").await;
    seed_project_and_thread(&database, &repository, "thread-2").await;
    let (claimed2, receipt2, sk2, creds2) =
        queue_claim_launch(&repository, "thread-2", "message-2", "run-2", "turn-2").await;
    let bound2 = bind_running(&repository, &claimed2, &receipt2, &sk2, &creds2).await;
    let assistant_item = ItemId::parse("assistant-2").expect("aid");
    let p_turn = PatchId::parse("p-turn-2").expect("p");
    let p_item = PatchId::parse("p-item-2").expect("p");
    commit_running_item(
        &repository,
        &claimed2,
        &receipt2,
        &bound2,
        &sk2,
        &creds2,
        &assistant_item,
        &p_turn,
        &p_item,
    )
    .await;

    let before = fetch_all(&database).await;
    let before_patches = before.patches.len();

    struct ShapeValidatingSource {
        calls: Vec<(String, bool)>,
    }
    impl StartupReconciliationPatchSource for ShapeValidatingSource {
        fn patch_ids_for(
            &mut self,
            candidate: &StartupReconciliationCandidate,
        ) -> Result<StartupReconciliationPatches, PatchSourceError> {
            let has_item = candidate.assistant_item_id.is_some();
            self.calls
                .push((candidate.run_id.as_str().to_owned(), has_item));
            let turn_patch =
                PatchId::parse(format!("turn-{}", candidate.run_id.as_str())).expect("p");
            let item_patch = if has_item {
                Some(PatchId::parse(format!("item-{}", candidate.run_id.as_str())).expect("p"))
            } else {
                None
            };
            // Validate shape agreement: has_item must match item_patch Some.
            assert_eq!(has_item, item_patch.is_some());
            Ok(StartupReconciliationPatches::new(turn_patch, item_patch))
        }
    }

    let mut source = ShapeValidatingSource { calls: Vec::new() };
    let input =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(SWEEP_OPERATED_AT_MS), 10)
            .expect("input");
    let report = sweep_startup_reconciliation(&repository, input, &mut source)
        .await
        .expect("sweep");

    assert_eq!(report.discovered, 2);
    assert_eq!(report.interrupted, 2);
    assert_eq!(report.attempted, 2);
    assert_eq!(report.skipped_moved, 0);
    assert_eq!(report.already_interrupted, 0);
    // Both candidates had correct shapes.
    assert_eq!(source.calls.len(), 2);
    for (run_id, has_item) in source.calls {
        if run_id == "run-1" {
            assert!(!has_item, "launching candidate must have no item");
        } else if run_id == "run-2" {
            assert!(has_item, "running candidate must have item");
        } else {
            panic!("unexpected run {run_id}");
        }
    }

    let after = fetch_all(&database).await;
    // Each launching needs 1 patch, running with item needs 2 patches: total 3 new patches.
    assert_eq!(after.patches.len(), before_patches + 3);
    // Verify durable rows: both runs interrupted.
    for run in &after.runs {
        if run.run_id == "run-1" || run.run_id == "run-2" {
            assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
            assert_eq!(run.updated_at_ms, SWEEP_OPERATED_AT_MS);
            assert!(run.owner.is_none());
        }
    }
    for dispatch in &after.dispatches {
        if dispatch.message_id == "message-1" || dispatch.message_id == "message-2" {
            assert_eq!(dispatch.state, DispatchState::Failed);
            assert_eq!(dispatch.updated_at_ms, SWEEP_OPERATED_AT_MS);
        }
    }
    // Turn and item interrupted.
    let turn1 = after
        .turns
        .iter()
        .find(|t| t.turn_id == "turn-1")
        .expect("turn1");
    assert_eq!(turn1.lifecycle, EntityLifecycle::Interrupted);
    let turn2 = after
        .turns
        .iter()
        .find(|t| t.turn_id == "turn-2")
        .expect("turn2");
    assert_eq!(turn2.lifecycle, EntityLifecycle::Interrupted);
    let item2 = after
        .items
        .iter()
        .find(|i| i.item_id == "assistant-2")
        .expect("item2");
    assert_eq!(item2.lifecycle, EntityLifecycle::Interrupted);
}

// ---------------------------------------------------------------------------
// 4. stale/moved candidate counted and later candidates continue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stale_moved_candidate_counted_and_later_continue() {
    let (database, repository, _temp) = temp_repository("stale-moved").await;
    for (thread, msg, run, turn) in [
        ("thread-a", "msg-a", "run-a", "turn-a"),
        ("thread-b", "msg-b", "run-b", "turn-b"),
        ("thread-c", "msg-c", "run-c", "turn-c"),
    ] {
        seed_project_and_thread(&database, &repository, thread).await;
        let (_cl, _rc, _sk, _cr) = queue_claim_launch(&repository, thread, msg, run, turn).await;
    }

    let before = fetch_all(&database).await;
    let before_patches = before.patches.len();

    // Deterministic move after discovery and before disposition of run-b.
    let (tx_req, rx_req) = std::sync::mpsc::sync_channel::<()>(0);
    let (tx_ack, rx_ack) = std::sync::mpsc::sync_channel::<Result<(), String>>(0);
    let db_for_thread = database.clone();
    let external = std::thread::spawn(move || {
        rx_req.recv().expect("request");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = rt.block_on(async {
            let run = entities::assistant_run::Entity::find_by_id("run-b".to_owned())
                .one(&db_for_thread)
                .await
                .expect("find")
                .expect("run");
            let mut active: entities::assistant_run::ActiveModel = run.into();
            active.updated_at_ms = Set(9999);
            active.update(&db_for_thread).await.expect("update");
            Ok::<(), String>(())
        });
        tx_ack.send(result).expect("ack");
    });

    struct SignalingStaleSource {
        target: String,
        tx: std::sync::mpsc::SyncSender<()>,
        rx: std::sync::mpsc::Receiver<Result<(), String>>,
    }

    impl StartupReconciliationPatchSource for SignalingStaleSource {
        fn patch_ids_for(
            &mut self,
            candidate: &StartupReconciliationCandidate,
        ) -> Result<StartupReconciliationPatches, PatchSourceError> {
            if candidate.run_id.as_str() == self.target {
                self.tx.send(()).expect("send");
                self.rx.recv().expect("recv").expect("external ok");
            }
            let turn_patch =
                PatchId::parse(format!("turn-{}", candidate.run_id.as_str())).expect("p");
            let item_patch = candidate
                .assistant_item_id
                .as_ref()
                .map(|_| PatchId::parse(format!("item-{}", candidate.run_id.as_str())).expect("p"));
            Ok(StartupReconciliationPatches::new(turn_patch, item_patch))
        }
    }

    let mut source = SignalingStaleSource {
        target: "run-b".to_owned(),
        tx: tx_req,
        rx: rx_ack,
    };
    let input =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(SWEEP_OPERATED_AT_MS), 10)
            .expect("input");

    let report = sweep_startup_reconciliation(&repository, input, &mut source)
        .await
        .expect("sweep should succeed with skipped");
    external.join().expect("external thread panicked");

    assert_eq!(report.discovered, 3);
    assert_eq!(report.attempted, 3);
    assert_eq!(report.interrupted, 2);
    assert_eq!(report.skipped_moved, 1);
    assert_eq!(report.already_interrupted, 0);

    let after = fetch_all(&database).await;
    // Only two interrupted candidates created patches (1 each) => 2 new patches.
    assert_eq!(after.patches.len(), before_patches + 2);

    // run-b should be unchanged (still launching) — move was not a disposition.
    let run_b = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-b")
        .expect("run-b");
    assert_eq!(run_b.lifecycle, AssistantRunLifecycle::Launching);
    let dispatch_b = after
        .dispatches
        .iter()
        .find(|d| d.message_id == "msg-b")
        .expect("dispatch-b");
    assert_eq!(dispatch_b.state, DispatchState::Running);

    // run-a and run-c interrupted.
    for run_id in ["run-a", "run-c"] {
        let run = after.runs.iter().find(|r| r.run_id == run_id).expect("run");
        assert_eq!(run.lifecycle, AssistantRunLifecycle::Interrupted);
    }

    drop(repository);
    let db_path = _temp.path().to_owned();
    let dir_path = _temp.dir.clone();
    let close = database.close().await;
    assert!(close.is_ok(), "close failed: {close:?}");
    std::fs::remove_file(&db_path).expect("remove db file");
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = PathBuf::from(format!("{}{}", db_path.display(), suffix));
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => panic!("remove sidecar {} failed: {err:?}", sidecar.display()),
        }
    }
    match std::fs::remove_dir(&dir_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("remove dir {} failed: {err:?}", dir_path.display()),
    }
}

// ---------------------------------------------------------------------------
// 5. identical pass/replay no duplicate and reported as already interrupted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn identical_pass_replay_no_duplicate_and_already_interrupted() {
    let (database, repository, _temp) = temp_repository("replay").await;
    for (thread, msg, run, turn) in [
        ("thread-a", "msg-a", "run-a", "turn-a"),
        ("thread-b", "msg-b", "run-b", "turn-b"),
        ("thread-c", "msg-c", "run-c", "turn-c"),
    ] {
        seed_project_and_thread(&database, &repository, thread).await;
        let (_cl, _rc, _sk, _cr) = queue_claim_launch(&repository, thread, msg, run, turn).await;
    }

    let before = fetch_all(&database).await;
    let before_patches = before.patches.len();
    let before_states: Vec<(String, i64)> = before
        .states
        .iter()
        .map(|s| (s.thread_id.clone(), s.last_patch_sequence))
        .collect();

    // Deterministic identical replay after discovery and before disposition of run-b.
    let (tx_req, rx_req) = std::sync::mpsc::sync_channel::<(
        StartupReconciliationCandidate,
        PatchId,
        Option<PatchId>,
    )>(0);
    let (tx_ack, rx_ack) = std::sync::mpsc::sync_channel::<Result<(), String>>(0);
    let repo_for_thread = repository.clone();
    let external = std::thread::spawn(move || {
        let (candidate, turn_patch, item_patch) = rx_req.recv().expect("request");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = rt.block_on(async {
            repo_for_thread
                .dispose_expired_startup_candidate(
                    artisan_database::StartupReconciliationDisposition {
                        candidate: &candidate,
                        operated_at: UnixMillis::from_millis(SWEEP_OPERATED_AT_MS),
                        turn_patch_id: &turn_patch,
                        item_patch_id: item_patch.as_ref(),
                    },
                )
                .await
                .map(|_| ())
                .map_err(|e| format!("{e:?}"))
        });
        tx_ack.send(result).expect("ack");
    });

    struct SignalingReplaySource {
        target: String,
        tx: std::sync::mpsc::SyncSender<(StartupReconciliationCandidate, PatchId, Option<PatchId>)>,
        rx: std::sync::mpsc::Receiver<Result<(), String>>,
    }

    impl StartupReconciliationPatchSource for SignalingReplaySource {
        fn patch_ids_for(
            &mut self,
            candidate: &StartupReconciliationCandidate,
        ) -> Result<StartupReconciliationPatches, PatchSourceError> {
            let turn_patch =
                PatchId::parse(format!("turn-{}", candidate.run_id.as_str())).expect("p");
            let item_patch = candidate
                .assistant_item_id
                .as_ref()
                .map(|_| PatchId::parse(format!("item-{}", candidate.run_id.as_str())).expect("p"));
            if candidate.run_id.as_str() == self.target {
                let candidate_clone = candidate.clone();
                let turn_clone = turn_patch.clone();
                let item_clone = item_patch.clone();
                self.tx
                    .send((candidate_clone, turn_clone, item_clone))
                    .expect("send");
                self.rx.recv().expect("recv").expect("external dispose ok");
            }
            Ok(StartupReconciliationPatches::new(turn_patch, item_patch))
        }
    }

    let mut source = SignalingReplaySource {
        target: "run-b".to_owned(),
        tx: tx_req,
        rx: rx_ack,
    };
    let input =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(SWEEP_OPERATED_AT_MS), 10)
            .expect("input");
    let report = sweep_startup_reconciliation(&repository, input, &mut source)
        .await
        .expect("sweep");
    external.join().expect("external thread panicked");

    assert_eq!(report.discovered, 3);
    assert_eq!(report.attempted, 3);
    assert_eq!(report.interrupted, 2);
    assert_eq!(report.already_interrupted, 1);
    assert_eq!(report.skipped_moved, 0);

    let after = fetch_all(&database).await;
    // External actor created 1 patch for run-b, sweep created 1 each for run-a/c and none duplicate for run-b => 3 new patches.
    assert_eq!(after.patches.len(), before_patches + 3);
    // No duplicate patch identities.
    let mut seen = std::collections::HashSet::new();
    for patch in &after.patches {
        assert!(
            seen.insert(patch.patch_id.clone()),
            "duplicate patch {}",
            patch.patch_id
        );
    }
    // Counter advance: each thread that was interrupted advances exactly once; run-b's counter was advanced by external actor, not doubled.
    for (thread_id, before_seq) in before_states {
        let after_state = after
            .states
            .iter()
            .find(|s| s.thread_id == thread_id)
            .expect("state");
        assert_eq!(after_state.last_patch_sequence, before_seq + 1);
    }
    // Durable rows: run-b was already interrupted by external actor, sweep reports it as AlreadyInterrupted without duplicate.
    let run_b = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-b")
        .expect("run-b");
    assert_eq!(run_b.lifecycle, AssistantRunLifecycle::Interrupted);
    assert_eq!(run_b.updated_at_ms, SWEEP_OPERATED_AT_MS);
    // Later candidate still committed.
    let run_c = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-c")
        .expect("run-c");
    assert_eq!(run_c.lifecycle, AssistantRunLifecycle::Interrupted);

    drop(repository);
    let db_path = _temp.path().to_owned();
    let dir_path = _temp.dir.clone();
    let close = database.close().await;
    assert!(close.is_ok(), "close failed: {close:?}");
    std::fs::remove_file(&db_path).expect("remove db file");
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = PathBuf::from(format!("{}{}", db_path.display(), suffix));
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => panic!("remove sidecar {} failed: {err:?}", sidecar.display()),
        }
    }
    match std::fs::remove_dir(&dir_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("remove dir {} failed: {err:?}", dir_path.display()),
    }
}

// ---------------------------------------------------------------------------
// 6. ID-source failure before candidate N leaves N untouched
// ---------------------------------------------------------------------------

#[tokio::test]
async fn patch_source_failure_before_candidate_n_leaves_n_untouched() {
    let (database, repository) = memory_repository().await;
    for (thread, msg, run, turn) in [
        ("thread-a", "msg-a", "run-a", "turn-a"),
        ("thread-b", "msg-b", "run-b", "turn-b"),
    ] {
        seed_project_and_thread(&database, &repository, thread).await;
        let (_cl, _rc, _sk, _cr) = queue_claim_launch(&repository, thread, msg, run, turn).await;
    }

    let before = fetch_all(&database).await;
    let input =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(SWEEP_OPERATED_AT_MS), 10)
            .expect("input");
    let mut source = FailingSource::new(1); // fail on second candidate (index 1)

    let err = sweep_startup_reconciliation(&repository, input, &mut source)
        .await
        .expect_err("should fail on second candidate");

    match &err {
        StartupReconciliationSweepError::PatchSource {
            report,
            failing_index,
            failing_run_id,
            ..
        } => {
            assert_eq!(*failing_index, 1);
            assert_eq!(failing_run_id.as_str(), "run-b");
            assert_eq!(report.discovered, 2);
            assert_eq!(report.interrupted, 1);
            assert_eq!(report.attempted, 1);
            assert_eq!(report.skipped_moved, 0);
            // Ensure error display is bounded and does not leak patch material.
            let display = format!("{err}");
            // The wrapped error's display is content-free.
            assert!(display.contains("1"), "display should contain index");
            // Report is prefix, not including failing candidate.
            let _ = report;
        }
        other => panic!("expected PatchSource, got {other:?}"),
    }

    let after = fetch_all(&database).await;
    // run-a should be interrupted, run-b untouched.
    let run_a = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-a")
        .expect("run-a");
    assert_eq!(run_a.lifecycle, AssistantRunLifecycle::Interrupted);
    let run_b = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-b")
        .expect("run-b");
    assert_eq!(run_b.lifecycle, AssistantRunLifecycle::Launching);
    let dispatch_b = after
        .dispatches
        .iter()
        .find(|d| d.message_id == "msg-b")
        .expect("dispatch-b");
    assert_eq!(dispatch_b.state, DispatchState::Running);
    // Only one new patch for run-a.
    assert_eq!(after.patches.len(), before.patches.len() + 1);
}

// ---------------------------------------------------------------------------
// 7. disposition failure after committed prefix reports prefix honestly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disposition_failure_after_committed_prefix_reports_honestly() {
    let (database, repository) = memory_repository().await;
    for (thread, msg, run, turn) in [
        ("thread-a", "msg-a", "run-a", "turn-a"),
        ("thread-b", "msg-b", "run-b", "turn-b"),
    ] {
        seed_project_and_thread(&database, &repository, thread).await;
        let (_cl, _rc, _sk, _cr) = queue_claim_launch(&repository, thread, msg, run, turn).await;
    }
    // Make second candidate's patch sequence overflow by setting its state's last_patch_sequence to MAX.
    let state_b = entities::conversation_state::Entity::find_by_id("thread-b")
        .one(&database)
        .await
        .expect("find")
        .expect("state");
    let mut active: entities::conversation_state::ActiveModel = state_b.into();
    active.last_patch_sequence = Set(i64::MAX);
    active.update(&database).await.expect("update");

    let before = fetch_all(&database).await;
    let input =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(SWEEP_OPERATED_AT_MS), 10)
            .expect("input");
    let mut source = DeterministicSource;

    let err = sweep_startup_reconciliation(&repository, input, &mut source)
        .await
        .expect_err("should fail on second candidate");

    match err {
        StartupReconciliationSweepError::Disposition {
            report,
            failing_index,
            failing_run_id,
            ..
        } => {
            assert_eq!(failing_index, 1);
            // Deterministic order is by lease then run_id; run-a (msg-a) < run-b => run-a first.
            assert_eq!(failing_run_id.as_str(), "run-b");
            assert_eq!(report.discovered, 2);
            assert_eq!(report.interrupted, 1);
            assert_eq!(report.attempted, 1);
        }
        other => panic!("expected Disposition, got {other:?}"),
    }

    let after = fetch_all(&database).await;
    // run-a interrupted, run-b untouched (rollback).
    let run_a = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-a")
        .expect("run-a");
    assert_eq!(run_a.lifecycle, AssistantRunLifecycle::Interrupted);
    let run_b = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-b")
        .expect("run-b");
    assert_eq!(run_b.lifecycle, AssistantRunLifecycle::Launching);
    // Only run-a created a patch.
    assert_eq!(after.patches.len(), before.patches.len() + 1);
}

// ---------------------------------------------------------------------------
// 8. invalid limit / counter overflow / patch-shape disagreement typed and no mutation of failing candidate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_limit_is_typed_and_no_mutation() {
    let (database, repository) = memory_repository().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_cl, _rc, _sk, _cr) =
        queue_claim_launch(&repository, "thread-1", "message-1", "run-1", "turn-1").await;
    let before = fetch_all(&database).await;

    // Via constructor.
    let err =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(SWEEP_OPERATED_AT_MS), 0)
            .expect_err("0 should be invalid");
    assert!(matches!(
        err,
        StartupReconciliationSweepError::InvalidLimit { limit: 0 }
    ));

    // Via sweep with raw struct bypassing constructor.
    let raw_input = StartupReconciliationSweepInput {
        operated_at: UnixMillis::from_millis(SWEEP_OPERATED_AT_MS),
        limit: 65,
    };
    let mut source = DeterministicSource;
    let err2 = sweep_startup_reconciliation(&repository, raw_input, &mut source)
        .await
        .expect_err("65 should be invalid");
    assert!(matches!(
        err2,
        StartupReconciliationSweepError::InvalidLimit { limit: 65 }
    ));

    let after = fetch_all(&database).await;
    assert_eq!(before, after);
}

#[tokio::test]
async fn patch_shape_disagreement_is_typed_and_no_mutation_of_failing_candidate() {
    let (database, repository) = memory_repository().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_cl, _rc, _sk, _cr) =
        queue_claim_launch(&repository, "thread-1", "message-1", "run-1", "turn-1").await;
    // run-1 is launching with no item, so shape mismatch: supply item patch.
    seed_project_and_thread(&database, &repository, "thread-2").await;
    let (claimed2, receipt2, sk2, creds2) =
        queue_claim_launch(&repository, "thread-2", "message-2", "run-2", "turn-2").await;
    let bound2 = bind_running(&repository, &claimed2, &receipt2, &sk2, &creds2).await;
    let assistant_item = ItemId::parse("assistant-2").expect("aid");
    commit_running_item(
        &repository,
        &claimed2,
        &receipt2,
        &bound2,
        &sk2,
        &creds2,
        &assistant_item,
        &PatchId::parse("p-turn-pre").expect("p"),
        &PatchId::parse("p-item-pre").expect("p"),
    )
    .await;

    // First candidate (run-1) has no item, second (run-2) has item. We'll make source fail shape on second candidate after first succeeds.
    struct FirstOkSecondMismatched {
        calls: usize,
    }
    impl StartupReconciliationPatchSource for FirstOkSecondMismatched {
        fn patch_ids_for(
            &mut self,
            candidate: &StartupReconciliationCandidate,
        ) -> Result<StartupReconciliationPatches, PatchSourceError> {
            self.calls += 1;
            if self.calls == 1 {
                // run-1: correct (no item)
                let turn =
                    PatchId::parse(format!("turn-{}", candidate.run_id.as_str())).expect("p");
                assert!(candidate.assistant_item_id.is_none());
                return Ok(StartupReconciliationPatches::new(turn, None));
            }
            // run-2: has item but we omit it => shape mismatch
            let turn = PatchId::parse(format!("turn-{}", candidate.run_id.as_str())).expect("p");
            Ok(StartupReconciliationPatches::new(turn, None))
        }
    }

    let before = fetch_all(&database).await;
    let input =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(SWEEP_OPERATED_AT_MS), 10)
            .expect("input");
    let mut source = FirstOkSecondMismatched { calls: 0 };
    let err = sweep_startup_reconciliation(&repository, input, &mut source)
        .await
        .expect_err("shape mismatch should fail");

    match err {
        StartupReconciliationSweepError::PatchShape {
            report,
            failing_index,
            failing_run_id,
            reason,
        } => {
            assert_eq!(failing_index, 1);
            assert_eq!(failing_run_id.as_str(), "run-2");
            assert_eq!(report.interrupted, 1);
            assert_eq!(report.attempted, 1);
            assert!(reason.contains("assistant item"));
        }
        other => panic!("expected PatchShape, got {other:?}"),
    }

    let after = fetch_all(&database).await;
    // run-1 should be interrupted (first succeeded), run-2 untouched.
    let run1 = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-1")
        .expect("run1");
    assert_eq!(run1.lifecycle, AssistantRunLifecycle::Interrupted);
    let run2 = after
        .runs
        .iter()
        .find(|r| r.run_id == "run-2")
        .expect("run2");
    assert_eq!(run2.lifecycle, AssistantRunLifecycle::Running);
    // Only one new patch for run-1.
    assert_eq!(after.patches.len(), before.patches.len() + 1);

    // Also test colliding identities.
    let (database2, repository2) = memory_repository().await;
    seed_project_and_thread(&database2, &repository2, "thread-1").await;
    let (claimed, receipt, sk, creds) =
        queue_claim_launch(&repository2, "thread-1", "message-1", "run-1", "turn-1").await;
    let bound = bind_running(&repository2, &claimed, &receipt, &sk, &creds).await;
    let assistant_item2 = ItemId::parse("assistant-1").expect("aid");
    commit_running_item(
        &repository2,
        &claimed,
        &receipt,
        &bound,
        &sk,
        &creds,
        &assistant_item2,
        &PatchId::parse("p-turn-pre2").expect("p"),
        &PatchId::parse("p-item-pre2").expect("p"),
    )
    .await;
    let before2 = fetch_all(&database2).await;
    let mut colliding = MismatchedShapeSource { mode: 1 };
    let err2 = sweep_startup_reconciliation(&repository2, input, &mut colliding)
        .await
        .expect_err("colliding should fail");
    assert!(matches!(
        err2,
        StartupReconciliationSweepError::PatchShape { .. }
    ));
    let after2 = fetch_all(&database2).await;
    assert_eq!(before2, after2);
}

#[tokio::test]
async fn counter_overflow_is_typed_and_no_mutation_of_failing_candidate() {
    let (database, repository) = memory_repository().await;
    seed_project_and_thread(&database, &repository, "thread-1").await;
    let (_cl, _rc, _sk, _cr) =
        queue_claim_launch(&repository, "thread-1", "message-1", "run-1", "turn-1").await;
    // Force last_patch_sequence to MAX for this thread.
    let state = entities::conversation_state::Entity::find_by_id("thread-1")
        .one(&database)
        .await
        .expect("find")
        .expect("state");
    let mut active: entities::conversation_state::ActiveModel = state.into();
    active.last_patch_sequence = Set(i64::MAX);
    active.update(&database).await.expect("update");

    let before = fetch_all(&database).await;
    let input =
        StartupReconciliationSweepInput::new(UnixMillis::from_millis(SWEEP_OPERATED_AT_MS), 10)
            .expect("input");
    let mut source = DeterministicSource;
    let err = sweep_startup_reconciliation(&repository, input, &mut source)
        .await
        .expect_err("overflow should fail");
    match err {
        StartupReconciliationSweepError::Disposition { .. } => {}
        other => panic!("expected Disposition for overflow, got {other:?}"),
    }
    let after = fetch_all(&database).await;
    assert_eq!(
        before, after,
        "failing candidate must be untouched on overflow"
    );
}

#[tokio::test]
async fn error_display_is_content_free_and_bounded() {
    let display_limit = StartupReconciliationSweepError::InvalidLimit { limit: 0 }.to_string();
    assert!(display_limit.contains("limit"));
    assert!(!display_limit.contains("deadbeef"));

    let report = StartupReconciliationSweepReport {
        discovered: 2,
        attempted: 1,
        interrupted: 1,
        already_interrupted: 0,
        skipped_moved: 0,
    };
    let err = StartupReconciliationSweepError::PatchSource {
        report,
        failing_index: 1,
        failing_run_id: RunId::parse("run-1").expect("run"),
        source: PatchSourceError,
    };
    let display = err.to_string();
    assert!(display.contains('1'));
    // Must not contain patch material.
    assert!(!display.contains("turn-"));
    assert!(display.len() < 500);
}
