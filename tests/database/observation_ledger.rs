//! Observation-ledger coverage: durable append-only activity history.
//!
//! The `run_checkpoints` row keeps only the latest batch per run, so the
//! ledger preserves every committed observation instead: one immutable row
//! per observation, appended atomically inside the same `commit_run_batch`
//! transaction that persists the checkpoint and receipt. These tests prove
//! that contract end to end through real migrated SQLite and the public
//! repository APIs: multi-batch history reads in delivery order with
//! attribution, cross-run delivery continuation with reset run-local
//! sequences, receipt replays that append nothing, insertion faults that
//! roll back the checkpoint with the ledger, bounded cursor pagination
//! (including the oversized-limit cap), and checkpoints that carry no
//! observation tag (`Keep` and opaque `Replace`) appending nothing while
//! preserving Replace semantics.

use artisan_database::{
    AssistantChange, BindRunProvider, BindRunProviderOutcome, BoundRunReceipt, CheckpointUpdate,
    ClaimMessageDispatch, ClaimedMessageDispatch, CommitRunBatch, CommitRunBatchOutcome,
    CreateThreadInput, DispatchLeaseOwner, EngineCheckpoint, LaunchClaimedRun, LaunchClaimedRunOutcome,
    LaunchedRunReceipt, OBSERVATION_BATCH_MAX_OBSERVATIONS, OBSERVATION_CHECKPOINT_VERSION,
    QueueFirstMessageInput, QueueMessageInput, Repository, RepositoryError, RunBatchScope, RunLaunchCredentials,
    RunObservationError, RunStartKey, SetThreadEngineConfigInput, SqliteConfig, ThreadEngineSettings,
    connect, encode_observation_bytes, encode_observation_checkpoint, entities,
};
use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantMessagePhase, ByteLimit, CountLimit, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineId, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FilesystemAccess, FiniteMillis, ItemId, MessageBody,
    MessageId, NetworkAccess, Observation, ObservationId, ObservationSequence, OpenCode2Selection,
    PatchId, PermissionId, ProjectId, QueueMessagePayload, RequestId, RunId, RunState,
    RunStateObservation, ThreadId, ThreadTitle, ToolAction, ToolObservation, TurnId, TurnState,
    TurnStateObservation, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

const OWNER_BYTES: [u8; 32] = [0xa1; 32];
const LEASE_BYTES: [u8; 32] = [0xb2; 32];
const CLAIM_TOKEN_BYTES: [u8; 32] = [0xc3; 32];
const CLAIM_TOKEN_BYTES_2: [u8; 32] = [0xc4; 32];
const START_KEY_BYTES: [u8; 32] = [0xd4; 32];
const START_KEY_BYTES_2: [u8; 32] = [0xd5; 32];
const DISPATCH_OWNER_BYTE: u8 = 0x11;

const THREAD_ID: &str = "ledger-thread-1";
const MESSAGE_ID_1: &str = "ledger-message-1";
const MESSAGE_ID_2: &str = "ledger-message-2";
const RUN_ID_1: &str = "ledger-run-1";
const RUN_ID_2: &str = "ledger-run-2";
const TURN_ID_1: &str = "ledger-turn-1";
const TURN_ID_2: &str = "ledger-turn-2";

const THREAD_CREATED_AT_MS: i64 = 10;
const ACCEPTED_1_MS: i64 = 50;
const CLAIMED_1_MS: i64 = 100;
const LEASE_1_EXPIRES_MS: i64 = 600;
const LAUNCH_1_MS: i64 = 150;
const BOUND_1_MS: i64 = 200;
const BATCH_1_MS: i64 = 250;
const BATCH_2_MS: i64 = 260;
const ACCEPTED_2_MS: i64 = 300;
const CLAIMED_2_MS: i64 = 310;
const LEASE_2_EXPIRES_MS: i64 = 900;
const LAUNCH_2_MS: i64 = 320;
const BOUND_2_MS: i64 = 330;
const BATCH_3_MS: i64 = 340;

fn oid(value: impl Into<String>) -> ObservationId {
    ObservationId::parse(value.into()).expect("fixture identity is valid")
}

fn seq(value: u64) -> ObservationSequence {
    ObservationSequence::new(value).expect("fixture sequence is valid")
}

fn run_state(index: u64, state: RunState) -> Observation {
    let id = oid(format!("ledger-obs-{index}"));
    Observation::RunState(RunStateObservation::new(id, seq(index), state))
}

fn turn_state(index: u64, state: TurnState) -> Observation {
    Observation::TurnState(TurnStateObservation::new(
        oid(format!("ledger-obs-{index}")),
        seq(index),
        oid("ledger-provider-turn"),
        state,
    ))
}

fn tool(index: u64, action: ToolAction) -> Observation {
    Observation::Tool(
        ToolObservation::new(
            oid(format!("ledger-obs-{index}")),
            seq(index),
            oid(format!("ledger-tool-{index}")),
            "bash".to_owned(),
            action,
            None,
        )
        .expect("tool row is valid"),
    )
}

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
        PermissionId::parse("permission-ledger").expect("permission id is valid"),
        EngineAgentId::parse("agent-ledger").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-ledger").expect("profile id is valid"),
            EngineModelId::parse("model-ledger").expect("model id is valid"),
            EngineRouteId::parse("route-ledger").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

struct Fixture {
    database: DatabaseConnection,
    repository: Repository,
    engine_settings: ThreadEngineSettings,
}

struct SeededRun {
    claimed: ClaimedMessageDispatch,
    launched: LaunchedRunReceipt,
    bound: BoundRunReceipt,
    start_key: RunStartKey,
    credentials: RunLaunchCredentials,
    expected_launch_at: UnixMillis,
}

impl SeededRun {
    fn scope(&self, expected_updated_at: UnixMillis) -> RunBatchScope<'_> {
        RunBatchScope {
            claimed: &self.claimed,
            launched: &self.launched,
            bound: &self.bound,
            run_start_key: &self.start_key,
            credentials: &self.credentials,
            expected_launch_at: self.expected_launch_at,
            expected_updated_at,
        }
    }
}

async fn fixture() -> Fixture {
    let (database, repository) = memory_database().await;
    entities::attached_project::ActiveModel {
        project_id: Set("ledger-project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    }
    .insert(&database)
    .await
    .expect("project");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse("ledger-seed-thread").expect("request id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            project_id: ProjectId::parse("ledger-project-1").expect("project id"),
            title: ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
            updated_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await
        .expect("thread");
    let thread_id = ThreadId::parse(THREAD_ID).expect("thread id");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("ledger-seed-config").expect("request id"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: launch_config(),
            accepted_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await
        .expect("engine configuration should create");
    let engine_settings = repository
        .read_thread_engine_settings(&thread_id)
        .await
        .expect("engine configuration should read")
        .expect("engine configuration should be present");
    Fixture {
        database,
        repository,
        engine_settings,
    }
}

async fn queue_first_message(fixture: &Fixture) {
    fixture
        .repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("ledger-request-1").expect("request id"),
            message_id: MessageId::parse(MESSAGE_ID_1).expect("message id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            body: MessageBody::parse("first durable body").expect("body"),
            accepted_at: UnixMillis::from_millis(ACCEPTED_1_MS),
        })
        .await
        .expect("queue");
}

async fn queue_second_message(fixture: &Fixture) {
    fixture
        .repository
        .queue_message(QueueMessageInput {
            request_id: RequestId::parse("ledger-request-2").expect("request id"),
            message_id: MessageId::parse(MESSAGE_ID_2).expect("message id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            payload: QueueMessagePayload::text_only("second durable body")
                .expect("payload should be valid"),
            accepted_at: UnixMillis::from_millis(ACCEPTED_2_MS),
        })
        .await
        .expect("queue");
}

#[allow(clippy::too_many_arguments)]
async fn claim_launch_bind(
    fixture: &Fixture,
    run_id: &str,
    turn_id: &str,
    item_id: &str,
    first_patch_id: &str,
    second_patch_id: &str,
    launch_operated_at_ms: i64,
    start_key_bytes: [u8; 32],
    claim_token_bytes: [u8; 32],
    claimed_at_ms: i64,
    lease_expires_at_ms: i64,
    bound_at_ms: i64,
) -> SeededRun {
    let claimed = fixture
        .repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            claimed_at: UnixMillis::from_millis(claimed_at_ms),
            lease_expires_at: UnixMillis::from_millis(lease_expires_at_ms),
        })
        .await
        .expect("claim")
        .expect("claimed");
    let run = RunId::parse(run_id).expect("run id");
    let turn = TurnId::parse(turn_id).expect("turn id");
    let item = ItemId::parse(item_id).expect("item id");
    let first_patch = PatchId::parse(first_patch_id).expect("patch id");
    let second_patch = PatchId::parse(second_patch_id).expect("patch id");
    let start_key = RunStartKey::new(start_key_bytes);
    let credentials = RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, claim_token_bytes);
    let LaunchClaimedRunOutcome::Started(launched) = fixture
        .repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run,
            turn_id: &turn,
            item_id: &item,
            first_patch_id: &first_patch,
            second_patch_id: &second_patch,
            operated_at: UnixMillis::from_millis(launch_operated_at_ms),
            run_start_key: &start_key,
            credentials: &credentials,
            engine_settings: &fixture.engine_settings,
        })
        .await
        .expect("launch")
    else {
        panic!("launch should start the run")
    };
    let binding =
        artisan_database::ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let bound = match fixture
        .repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(launch_operated_at_ms),
            bound_at: UnixMillis::from_millis(bound_at_ms),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await
        .expect("bind")
    {
        BindRunProviderOutcome::Bound(receipt) => receipt,
        BindRunProviderOutcome::AlreadyBound(_) => panic!("bind should be fresh"),
    };
    SeededRun {
        claimed,
        launched,
        bound,
        start_key,
        credentials,
        expected_launch_at: UnixMillis::from_millis(launch_operated_at_ms),
    }
}

async fn launch_first_run(fixture: &Fixture) -> SeededRun {
    queue_first_message(fixture).await;
    claim_launch_bind(
        fixture,
        RUN_ID_1,
        TURN_ID_1,
        "ledger-launch-item-1",
        "ledger-launch-patch-1",
        "ledger-launch-patch-2",
        LAUNCH_1_MS,
        START_KEY_BYTES,
        CLAIM_TOKEN_BYTES,
        CLAIMED_1_MS,
        LEASE_1_EXPIRES_MS,
        BOUND_1_MS,
    )
    .await
}

async fn launch_second_run(fixture: &Fixture) -> SeededRun {
    queue_second_message(fixture).await;
    claim_launch_bind(
        fixture,
        RUN_ID_2,
        TURN_ID_2,
        "ledger-launch-item-2",
        "ledger-launch-patch-3",
        "ledger-launch-patch-4",
        LAUNCH_2_MS,
        START_KEY_BYTES_2,
        CLAIM_TOKEN_BYTES_2,
        CLAIMED_2_MS,
        LEASE_2_EXPIRES_MS,
        BOUND_2_MS,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn commit_observations(
    fixture: &Fixture,
    run: &SeededRun,
    expected_updated_at_ms: i64,
    batch_sequence: i64,
    operated_at_ms: i64,
    activate_turn_patch_id: Option<&PatchId>,
    changes: &[AssistantChange<'_>],
    checkpoint: CheckpointUpdate<'_>,
) -> Result<CommitRunBatchOutcome, RunObservationError> {
    fixture
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: run.scope(UnixMillis::from_millis(expected_updated_at_ms)),
            batch_sequence,
            operated_at: UnixMillis::from_millis(operated_at_ms),
            activate_turn_patch_id,
            changes,
            checkpoint,
        })
        .await
}

fn observation_checkpoint(
    base_sequence: Option<u64>,
    observations: &[Observation],
) -> EngineCheckpoint {
    encode_observation_checkpoint(EngineId::OpenCode2, 1, base_sequence, observations)
        .expect("fixture batch should encode")
}

async fn ledger_rows(database: &DatabaseConnection) -> Vec<entities::ObservationLedger> {
    let mut rows = entities::observation_ledger::Entity::find()
        .all(database)
        .await
        .expect("ledger rows should read");
    rows.sort_by(|a, b| {
        (a.thread_id.clone(), a.delivery_sequence).cmp(&(b.thread_id.clone(), b.delivery_sequence))
    });
    rows
}

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD_ID).expect("thread id")
}

#[tokio::test]
async fn two_batches_before_read_return_ordered_attributed_history() {
    let fixture = fixture().await;
    let run = launch_first_run(&fixture).await;

    let first = vec![
        run_state(1, RunState::Running),
        turn_state(2, TurnState::Started),
    ];
    let second = vec![tool(3, ToolAction::Completed)];

    let body = AssistantBody::parse("ledger progress one").expect("body");
    let item = ItemId::parse("ledger-item-1").expect("item id");
    let activation = PatchId::parse("ledger-activate-1").expect("patch id");
    let patch_item = PatchId::parse("ledger-patch-1").expect("patch id");
    let checkpoint = observation_checkpoint(None, &first);
    let outcome = commit_observations(
        &fixture,
        &run,
        BOUND_1_MS,
        1,
        BATCH_1_MS,
        Some(&activation),
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("first observation batch should commit");
    assert!(matches!(outcome, CommitRunBatchOutcome::Committed(_)));

    let body = AssistantBody::parse("ledger progress two").expect("body");
    let item = ItemId::parse("ledger-item-2").expect("item id");
    let patch_item = PatchId::parse("ledger-patch-2").expect("patch id");
    let checkpoint = observation_checkpoint(Some(2), &second);
    let outcome = commit_observations(
        &fixture,
        &run,
        BATCH_1_MS,
        2,
        BATCH_2_MS,
        None,
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Commentary,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("second observation batch should commit");
    assert!(matches!(outcome, CommitRunBatchOutcome::Committed(_)));

    // The checkpoint keeps only the latest batch while the ledger keeps all.
    assert_eq!(ledger_rows(&fixture.database).await.len(), 3);

    let history = fixture
        .repository
        .read_observation_history(&thread_id(), 0, 64)
        .await
        .expect("history should read");
    assert_eq!(history.len(), 3);
    let expected: Vec<Observation> = first.into_iter().chain(second).collect();
    for (index, event) in history.iter().enumerate() {
        let delivery = u64::try_from(index).expect("index fits") + 1;
        assert_eq!(event.thread_id.as_str(), THREAD_ID);
        assert_eq!(event.observation, expected[index]);
        let attribution = event
            .attribution
            .clone()
            .expect("ledger events always carry attribution");
        assert_eq!(attribution.run_id.as_str(), RUN_ID_1);
        assert_eq!(attribution.turn_id.as_str(), TURN_ID_1);
        assert_eq!(attribution.delivery_sequence, delivery);
        let committed_at = if delivery <= 2 { BATCH_1_MS } else { BATCH_2_MS };
        assert_eq!(attribution.committed_at.as_millis(), committed_at);
    }
}

#[tokio::test]
async fn two_runs_same_thread_continue_delivery_with_reset_source_sequence() {
    let fixture = fixture().await;
    let first_run = launch_first_run(&fixture).await;

    let first = vec![run_state(1, RunState::Running)];
    let body = AssistantBody::parse("ledger run one").expect("body");
    let item = ItemId::parse("ledger-item-1").expect("item id");
    let activation = PatchId::parse("ledger-activate-1").expect("patch id");
    let patch_item = PatchId::parse("ledger-patch-1").expect("patch id");
    let checkpoint = observation_checkpoint(None, &first);
    commit_observations(
        &fixture,
        &first_run,
        BOUND_1_MS,
        1,
        BATCH_1_MS,
        Some(&activation),
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("first run batch should commit");

    // The second run restarts its run-local observation sequence at one.
    let second_run = launch_second_run(&fixture).await;
    let second = vec![
        run_state(1, RunState::Running),
        tool(2, ToolAction::Started),
    ];
    let body = AssistantBody::parse("ledger run two").expect("body");
    let item = ItemId::parse("ledger-item-2").expect("item id");
    let activation = PatchId::parse("ledger-activate-2").expect("patch id");
    let patch_item = PatchId::parse("ledger-patch-2").expect("patch id");
    let checkpoint = observation_checkpoint(None, &second);
    commit_observations(
        &fixture,
        &second_run,
        BOUND_2_MS,
        1,
        BATCH_3_MS,
        Some(&activation),
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("second run batch should commit");

    let history = fixture
        .repository
        .read_observation_history(&thread_id(), 0, 64)
        .await
        .expect("history should read");
    assert_eq!(history.len(), 3);
    // Delivery continues across runs while run-local sequences reset.
    assert_eq!(history[0].observation, first[0]);
    assert_eq!(history[1].observation, second[0]);
    assert_eq!(history[2].observation, second[1]);
    let first_attribution = history[0].attribution.clone().expect("attribution");
    assert_eq!(first_attribution.run_id.as_str(), RUN_ID_1);
    assert_eq!(first_attribution.turn_id.as_str(), TURN_ID_1);
    assert_eq!(first_attribution.delivery_sequence, 1);
    assert_eq!(first_attribution.committed_at.as_millis(), BATCH_1_MS);
    for event in history.iter().skip(1) {
        let attribution = event.attribution.clone().expect("attribution");
        assert_eq!(attribution.run_id.as_str(), RUN_ID_2);
        assert_eq!(attribution.turn_id.as_str(), TURN_ID_2);
        assert_eq!(attribution.committed_at.as_millis(), BATCH_3_MS);
    }
    assert_eq!(
        history[1]
            .attribution
            .clone()
            .expect("attribution")
            .delivery_sequence,
        2
    );
    assert_eq!(
        history[2]
            .attribution
            .clone()
            .expect("attribution")
            .delivery_sequence,
        3
    );
}

#[tokio::test]
async fn receipt_replay_creates_no_ledger_rows() {
    let fixture = fixture().await;
    let run = launch_first_run(&fixture).await;

    let observations = vec![
        run_state(1, RunState::Running),
        tool(2, ToolAction::Completed),
    ];
    let body = AssistantBody::parse("ledger replay body").expect("body");
    let item = ItemId::parse("ledger-item-1").expect("item id");
    let activation = PatchId::parse("ledger-activate-1").expect("patch id");
    let patch_item = PatchId::parse("ledger-patch-1").expect("patch id");
    let checkpoint = observation_checkpoint(None, &observations);
    let changes = [AssistantChange::Start {
        item_id: &item,
        phase: AssistantMessagePhase::Final,
        body: &body,
        patch_id: &patch_item,
    }];
    let outcome = commit_observations(
        &fixture,
        &run,
        BOUND_1_MS,
        1,
        BATCH_1_MS,
        Some(&activation),
        &changes,
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("first commit should succeed");
    assert!(matches!(outcome, CommitRunBatchOutcome::Committed(_)));
    assert_eq!(ledger_rows(&fixture.database).await.len(), 2);

    // The byte-exact replay classifies through the receipt and appends nothing.
    let replay = commit_observations(
        &fixture,
        &run,
        BOUND_1_MS,
        1,
        BATCH_1_MS,
        Some(&activation),
        &changes,
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("replay should classify");
    assert!(matches!(
        replay,
        CommitRunBatchOutcome::AlreadyCommitted(_)
    ));
    assert_eq!(ledger_rows(&fixture.database).await.len(), 2);
    let history = fixture
        .repository
        .read_observation_history(&thread_id(), 0, 64)
        .await
        .expect("history should read");
    assert_eq!(history.len(), 2);
}

#[tokio::test]
async fn ledger_insertion_fault_rolls_back_checkpoint_and_ledger() {
    let fixture = fixture().await;
    let run = launch_first_run(&fixture).await;

    let first = vec![tool(1, ToolAction::Started)];
    let body = AssistantBody::parse("ledger first").expect("body");
    let item = ItemId::parse("ledger-item-1").expect("item id");
    let activation = PatchId::parse("ledger-activate-1").expect("patch id");
    let patch_item = PatchId::parse("ledger-patch-1").expect("patch id");
    let checkpoint = observation_checkpoint(None, &first);
    commit_observations(
        &fixture,
        &run,
        BOUND_1_MS,
        1,
        BATCH_1_MS,
        Some(&activation),
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("first batch should commit");

    // The second batch reuses the run-local sequence, so its ledger insert
    // collides on `(run_id, observation_sequence)`.
    let conflicting = vec![tool(1, ToolAction::Completed)];
    let body = AssistantBody::parse("ledger second").expect("body");
    let item = ItemId::parse("ledger-item-2").expect("item id");
    let patch_item = PatchId::parse("ledger-patch-2").expect("patch id");
    let checkpoint = observation_checkpoint(None, &conflicting);
    let error = commit_observations(
        &fixture,
        &run,
        BATCH_1_MS,
        2,
        BATCH_2_MS,
        None,
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect_err("sequence reuse must fail the batch");
    assert!(
        matches!(
            error,
            RunObservationError::Repository(RepositoryError::Database { .. })
        ),
        "insertion fault should surface as a database rejection, got {error:?}"
    );

    // The failed batch rolled back both effects: one ledger row, the first
    // checkpoint, and no receipt for the second batch sequence.
    assert_eq!(ledger_rows(&fixture.database).await.len(), 1);
    let checkpoint_row = entities::run_checkpoint::Entity::find_by_id(RUN_ID_1)
        .one(&fixture.database)
        .await
        .expect("checkpoint should read")
        .expect("checkpoint row should exist");
    assert_eq!(checkpoint_row.last_batch_sequence, 1);
    let receipt = entities::run_batch_receipt::Entity::find_by_id((RUN_ID_1.to_owned(), 2))
        .one(&fixture.database)
        .await
        .expect("receipt should read");
    assert!(receipt.is_none());
    let history = fixture
        .repository
        .read_observation_history(&thread_id(), 0, 64)
        .await
        .expect("history should read");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].observation, first[0]);
}

#[tokio::test]
async fn bounded_pagination_returns_cursor_pages() {
    let fixture = fixture().await;
    let run = launch_first_run(&fixture).await;

    let first = vec![
        run_state(1, RunState::Running),
        turn_state(2, TurnState::Started),
    ];
    let second = vec![
        tool(3, ToolAction::Started),
        tool(4, ToolAction::Completed),
    ];
    let body = AssistantBody::parse("ledger page one").expect("body");
    let item = ItemId::parse("ledger-item-1").expect("item id");
    let activation = PatchId::parse("ledger-activate-1").expect("patch id");
    let patch_item = PatchId::parse("ledger-patch-1").expect("patch id");
    let checkpoint = observation_checkpoint(None, &first);
    commit_observations(
        &fixture,
        &run,
        BOUND_1_MS,
        1,
        BATCH_1_MS,
        Some(&activation),
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("first batch should commit");
    let body = AssistantBody::parse("ledger page two").expect("body");
    let item = ItemId::parse("ledger-item-2").expect("item id");
    let patch_item = PatchId::parse("ledger-patch-2").expect("patch id");
    let checkpoint = observation_checkpoint(Some(2), &second);
    commit_observations(
        &fixture,
        &run,
        BATCH_1_MS,
        2,
        BATCH_2_MS,
        None,
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Commentary,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("second batch should commit");

    let repository = &fixture.repository;
    let page = repository
        .read_observation_history(&thread_id(), 0, 2)
        .await
        .expect("first page should read");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].observation, first[0]);
    assert_eq!(page[1].observation, first[1]);

    let page = repository
        .read_observation_history(&thread_id(), 2, 2)
        .await
        .expect("second page should read");
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].observation, second[0]);
    assert_eq!(page[1].observation, second[1]);
    for event in &page {
        let attribution = event.attribution.clone().expect("attribution");
        assert_eq!(attribution.run_id.as_str(), RUN_ID_1);
        assert_eq!(attribution.turn_id.as_str(), TURN_ID_1);
        assert_eq!(attribution.committed_at.as_millis(), BATCH_2_MS);
    }

    let page = repository
        .read_observation_history(&thread_id(), 4, 2)
        .await
        .expect("past-the-end page should read");
    assert!(page.is_empty());

    let page = repository
        .read_observation_history(&thread_id(), 0, 0)
        .await
        .expect("zero limit should read");
    assert!(page.is_empty());

    let page = repository
        .read_observation_history(&thread_id(), 1, 64)
        .await
        .expect("tail page should read");
    assert_eq!(page.len(), 3);
    assert_eq!(
        page[0]
            .attribution
            .clone()
            .expect("attribution")
            .delivery_sequence,
        2
    );
}

#[tokio::test]
async fn oversized_limit_is_capped_at_batch_maximum() {
    let fixture = fixture().await;
    let run = launch_first_run(&fixture).await;

    let per_batch = usize::try_from(40).expect("forty fits");
    let mut committed = Vec::new();
    let mut expected_updated_at_ms = BOUND_1_MS;
    let mut operated_at_ms = BATCH_1_MS;
    let mut base_sequence = None;
    for batch_index in 0..2 {
        let start = batch_index * per_batch + 1;
        let observations: Vec<Observation> = (0..per_batch)
            .map(|offset| {
                let index = u64::try_from(start + offset).expect("index fits");
                run_state(index, RunState::Running)
            })
            .collect();
        let body = AssistantBody::parse(format!("ledger bulk {batch_index}")).expect("body");
        let item = ItemId::parse(format!("ledger-bulk-item-{batch_index}")).expect("item id");
        let patch_item =
            PatchId::parse(format!("ledger-bulk-patch-{batch_index}")).expect("patch id");
        let checkpoint = observation_checkpoint(base_sequence, &observations);
        let activation = (batch_index == 0).then(|| {
            PatchId::parse("ledger-bulk-activate").expect("patch id")
        });
        commit_observations(
            &fixture,
            &run,
            expected_updated_at_ms,
            i64::try_from(batch_index).expect("batch fits") + 1,
            operated_at_ms,
            activation.as_ref(),
            &[AssistantChange::Start {
                item_id: &item,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: &patch_item,
            }],
            CheckpointUpdate::Replace(&checkpoint),
        )
        .await
        .expect("bulk batch should commit");
        base_sequence = Some(u64::try_from(start + per_batch).expect("sequence fits") - 1);
        expected_updated_at_ms = operated_at_ms;
        operated_at_ms += 10;
        committed.extend(observations);
    }
    assert_eq!(committed.len(), 80);

    // An unbounded request returns one capped page, never all history.
    let page = fixture
        .repository
        .read_observation_history(&thread_id(), 0, usize::MAX)
        .await
        .expect("capped page should read");
    assert_eq!(page.len(), OBSERVATION_BATCH_MAX_OBSERVATIONS);
    assert_eq!(page[0].observation, committed[0]);
    assert_eq!(
        page[OBSERVATION_BATCH_MAX_OBSERVATIONS - 1].observation,
        committed[OBSERVATION_BATCH_MAX_OBSERVATIONS - 1]
    );

    let remainder = fixture
        .repository
        .read_observation_history(
            &thread_id(),
            u64::try_from(OBSERVATION_BATCH_MAX_OBSERVATIONS).expect("cap fits"),
            usize::MAX,
        )
        .await
        .expect("remainder page should read");
    assert_eq!(
        remainder.len(),
        committed.len() - OBSERVATION_BATCH_MAX_OBSERVATIONS
    );
    assert_eq!(
        remainder[0].observation,
        committed[OBSERVATION_BATCH_MAX_OBSERVATIONS]
    );
}

#[tokio::test]
async fn wrong_version_claimed_envelope_rolls_back() {
    let fixture = fixture().await;
    let run = launch_first_run(&fixture).await;

    let first = vec![tool(1, ToolAction::Started)];
    let body = AssistantBody::parse("ledger version body").expect("body");
    let item = ItemId::parse("ledger-item-1").expect("item id");
    let activation = PatchId::parse("ledger-activate-1").expect("patch id");
    let patch_item = PatchId::parse("ledger-patch-1").expect("patch id");
    let checkpoint = observation_checkpoint(None, &first);
    commit_observations(
        &fixture,
        &run,
        BOUND_1_MS,
        1,
        BATCH_1_MS,
        Some(&activation),
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("first batch should commit");

    // The same canonical envelope bytes wrapped in the wrong outer checkpoint
    // version: the format tag claims an observation batch, so strict decoding
    // rejects the version mismatch instead of treating it as opaque.
    let second = vec![tool(2, ToolAction::Started)];
    let envelope = encode_observation_bytes(EngineId::OpenCode2, 1, None, &second)
        .expect("fixture envelope should encode");
    let mismatched =
        EngineCheckpoint::new(OBSERVATION_CHECKPOINT_VERSION + 1, envelope)
            .expect("outer version is still a valid checkpoint version");
    let body = AssistantBody::parse("ledger version second").expect("body");
    let item = ItemId::parse("ledger-item-2").expect("item id");
    let patch_item = PatchId::parse("ledger-patch-2").expect("patch id");
    let error = commit_observations(
        &fixture,
        &run,
        BATCH_1_MS,
        2,
        BATCH_2_MS,
        None,
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&mismatched),
    )
    .await
    .expect_err("version-mismatched claim must fail the batch");
    assert!(
        matches!(error, RunObservationError::InvalidCheckpoint { .. }),
        "claimed envelope with wrong version should reject typed, got {error:?}"
    );

    assert_eq!(ledger_rows(&fixture.database).await.len(), 1);
    let checkpoint_row = entities::run_checkpoint::Entity::find_by_id(RUN_ID_1)
        .one(&fixture.database)
        .await
        .expect("checkpoint should read")
        .expect("checkpoint row should exist");
    assert_eq!(checkpoint_row.last_batch_sequence, 1);
    assert_eq!(
        checkpoint_row.engine_checkpoint_version,
        Some(OBSERVATION_CHECKPOINT_VERSION)
    );
    let receipt = entities::run_batch_receipt::Entity::find_by_id((RUN_ID_1.to_owned(), 2))
        .one(&fixture.database)
        .await
        .expect("receipt should read");
    assert!(receipt.is_none());
    let history = fixture
        .repository
        .read_observation_history(&thread_id(), 0, 64)
        .await
        .expect("history should read");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].observation, first[0]);
}

#[tokio::test]
async fn keep_and_opaque_checkpoints_append_nothing() {
    let fixture = fixture().await;
    let run = launch_first_run(&fixture).await;

    let body = AssistantBody::parse("ledger keep body").expect("body");
    let item = ItemId::parse("ledger-item-1").expect("item id");
    let activation = PatchId::parse("ledger-activate-1").expect("patch id");
    let patch_item = PatchId::parse("ledger-patch-1").expect("patch id");
    commit_observations(
        &fixture,
        &run,
        BOUND_1_MS,
        1,
        BATCH_1_MS,
        Some(&activation),
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Keep,
    )
    .await
    .expect("keep batch should commit");
    let row = entities::run_checkpoint::Entity::find_by_id(RUN_ID_1)
        .one(&fixture.database)
        .await
        .expect("checkpoint should read")
        .expect("checkpoint row should exist");
    assert!(row.engine_checkpoint_version.is_none());
    assert!(row.engine_checkpoint_blob.is_none());
    assert!(ledger_rows(&fixture.database).await.is_empty());

    // An opaque Replace still wins the checkpoint tuple but stages no rows.
    let opaque = EngineCheckpoint::new(7, vec![0xaa; 8]).expect("opaque checkpoint is valid");
    let body = AssistantBody::parse("ledger opaque body").expect("body");
    let item = ItemId::parse("ledger-item-2").expect("item id");
    let patch_item = PatchId::parse("ledger-patch-2").expect("patch id");
    commit_observations(
        &fixture,
        &run,
        BATCH_1_MS,
        2,
        BATCH_2_MS,
        None,
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Commentary,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&opaque),
    )
    .await
    .expect("opaque batch should commit");
    let row = entities::run_checkpoint::Entity::find_by_id(RUN_ID_1)
        .one(&fixture.database)
        .await
        .expect("checkpoint should read")
        .expect("checkpoint row should exist");
    assert_eq!(row.engine_checkpoint_version, Some(7));
    assert!(ledger_rows(&fixture.database).await.is_empty());
    let history = fixture
        .repository
        .read_observation_history(&thread_id(), 0, 64)
        .await
        .expect("history should read");
    assert!(history.is_empty());

    // A later observation batch still Replace-commits and starts delivery at one.
    let observations = vec![tool(1, ToolAction::Started)];
    let body = AssistantBody::parse("ledger late body").expect("body");
    let item = ItemId::parse("ledger-item-3").expect("item id");
    let patch_item = PatchId::parse("ledger-patch-3").expect("patch id");
    let checkpoint = observation_checkpoint(None, &observations);
    commit_observations(
        &fixture,
        &run,
        BATCH_2_MS,
        3,
        BATCH_2_MS + 10,
        None,
        &[AssistantChange::Start {
            item_id: &item,
            phase: AssistantMessagePhase::Final,
            body: &body,
            patch_id: &patch_item,
        }],
        CheckpointUpdate::Replace(&checkpoint),
    )
    .await
    .expect("observation batch should commit");
    let history = fixture
        .repository
        .read_observation_history(&thread_id(), 0, 64)
        .await
        .expect("history should read");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].observation, observations[0]);
    let attribution = history[0].attribution.clone().expect("attribution");
    assert_eq!(attribution.delivery_sequence, 1);
    assert_eq!(attribution.committed_at.as_millis(), BATCH_2_MS + 10);
}
