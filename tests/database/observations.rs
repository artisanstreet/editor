//! S1a observation persistence coverage: the shared observation vocabulary
//! round-trips through the existing batch payload with no migration.
//!
//! Typed observations pack into an [`EngineCheckpoint`] with the explicit
//! `artisan.observation.v1` format tag and commit through the existing
//! [`Repository::commit_run_batch`] path. These tests prove no migration was
//! needed by committing fixture observations end to end (claim, launch, bind,
//! batch) and decoding the persisted `run_checkpoints` row with
//! [`decode_observation_checkpoint`]. Only pre-existing tables and repository
//! APIs are touched: no new entity, no new migration, no protocol or backend
//! involvement.
//!
//! Coverage: round-trip of every observation variant, bind-agreement
//! rejection, engine-continuity rejection, monotonic sequencing within and
//! across batches, oversize rejection, malformed/format/canonicality
//! rejection, and typed rejection of unknown engines, tags, and provider
//! values.

use artisan_database::{
    AssistantChange, BindRunProvider, BindRunProviderOutcome, BoundRunReceipt, CheckpointUpdate,
    ClaimMessageDispatch, ClaimedMessageDispatch, CommitRunBatch, CommitRunBatchOutcome,
    CreateThreadInput, DecodedObservationBatch, DispatchLeaseOwner, LaunchClaimedRun,
    LaunchClaimedRunOutcome, LaunchedRunReceipt, OBSERVATION_BATCH_MAX_OBSERVATIONS,
    OBSERVATION_CHECKPOINT_VERSION, OBSERVATION_FORMAT_TAG, ObservationCommitError,
    ProviderBindingBytes, QueueFirstMessageInput, Repository, RunBatchScope, RunLaunchCredentials,
    RunStartKey, SetThreadEngineConfigInput, SqliteConfig, ThreadEngineSettings, connect,
    decode_observation_checkpoint, encode_observation_bytes, encode_observation_checkpoint,
    entities, validate_observation_bind, validate_observation_engine,
};
use artisan_domain::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation, ApprovalMode,
    ApprovalObservation, ApprovalRequest, ArtisanCode, AssistantBody, AssistantMessagePhase,
    ByteLimit, CompactionObservation, CompactionState, CountLimit, DiagnosticLevel, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineErrorRef, EngineErrorRefInput, EngineId, EngineModelId,
    EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FileAction, FileObservation, FilesystemAccess,
    FiniteMillis, IncrementalText, ItemId, LimitScope, MessageBody, MessageId, MessagePhase,
    NativeActionObservation, NetworkAccess, Observation, ObservationError, ObservationId,
    ObservationSequence, OpenCode2Selection, PatchId, PermissionId, PlanEntry, PlanEntryStatus,
    PlanObservation, ProcessDiagnosticObservation, ProjectId, ProtocolDiagnosticObservation,
    QuestionInput, QuestionObservation, QuestionOption, ReasoningSummaryCompletedObservation,
    ReasoningSummaryDeltaObservation, RequestId, RetryAttemptState, RetryObservation, Revision,
    RunId, RunState, RunStateObservation, RunTerminalObservation, RunTerminalState,
    SearchObservation, SearchScope, SearchState, SubagentInput, SubagentObservation, SubagentState,
    SubagentTranscriptObservation, TerminalActivityInput, TerminalActivityObservation,
    TerminalActivityState, TerminalChannel, ThreadId, ThreadTitle, ToolAction, ToolObservation,
    TranscriptContent, TurnId, TurnState, TurnStateObservation, UnixMillis, UsageBasis, UsageInput,
    UsageObservation, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

const OWNER_BYTES: [u8; 32] = [0xa1; 32];
const LEASE_BYTES: [u8; 32] = [0xb2; 32];
const CLAIM_TOKEN_BYTES: [u8; 32] = [0xc3; 32];
const START_KEY_BYTES: [u8; 32] = [0xd4; 32];
const DISPATCH_OWNER_BYTE: u8 = 0x11;

const RUN_ID: &str = "s1a-run-1";
const TURN_ID: &str = "s1a-turn-1";
const THREAD_ID: &str = "s1a-thread-1";
const MESSAGE_ID: &str = "s1a-message-1";
const CORRELATION_ID: &str = "s1a-request-1";

const THREAD_CREATED_AT_MS: i64 = 10;
const ACCEPTED_AT_MS: i64 = 50;
const CLAIMED_AT_MS: i64 = 100;
const LEASE_EXPIRES_AT_MS: i64 = 600;
const OPERATED_AT_MS: i64 = 150;
const BOUND_AT_MS: i64 = 200;
const BATCH_OPERATED_AT_MS: i64 = 250;
const BATCH_OPERATED_AT_MS_2: i64 = 260;

fn oid(value: &str) -> ObservationId {
    ObservationId::parse(value.to_owned()).expect("fixture identity is valid")
}

fn seq(value: u64) -> ObservationSequence {
    ObservationSequence::new(value).expect("fixture sequence is valid")
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
        PermissionId::parse("permission-s1a").expect("permission id is valid"),
        EngineAgentId::parse("agent-s1a").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-s1a").expect("profile id is valid"),
            EngineModelId::parse("model-s1a").expect("model id is valid"),
            EngineRouteId::parse("route-s1a").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

struct SeededPair {
    database: DatabaseConnection,
    repository: Repository,
    claimed: ClaimedMessageDispatch,
    launched: LaunchedRunReceipt,
    bound: BoundRunReceipt,
    start_key: RunStartKey,
    credentials: RunLaunchCredentials,
}

async fn seeded_pair() -> SeededPair {
    let (database, repository) = memory_database().await;
    entities::attached_project::ActiveModel {
        project_id: Set("s1a-project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    }
    .insert(&database)
    .await
    .expect("project");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse("s1a-seed-thread").expect("request id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            project_id: ProjectId::parse("s1a-project-1").expect("project id"),
            title: ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
            updated_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await
        .expect("thread");
    let thread_id = ThreadId::parse(THREAD_ID).expect("thread id");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("s1a-seed-config").expect("request id"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: launch_config(),
            accepted_at: UnixMillis::from_millis(THREAD_CREATED_AT_MS),
        })
        .await
        .expect("engine configuration should create");
    let engine_settings: ThreadEngineSettings = repository
        .read_thread_engine_settings(&thread_id)
        .await
        .expect("engine configuration should read")
        .expect("engine configuration should be present");
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse(CORRELATION_ID).expect("request id"),
            message_id: MessageId::parse(MESSAGE_ID).expect("message id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            body: MessageBody::parse("first durable body").expect("body"),
            accepted_at: UnixMillis::from_millis(ACCEPTED_AT_MS),
        })
        .await
        .expect("queue");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            claimed_at: UnixMillis::from_millis(CLAIMED_AT_MS),
            lease_expires_at: UnixMillis::from_millis(LEASE_EXPIRES_AT_MS),
        })
        .await
        .expect("claim")
        .expect("claimed");
    let run_id = RunId::parse(RUN_ID).expect("run id");
    let turn_id = TurnId::parse(TURN_ID).expect("turn id");
    let item_id = ItemId::parse("s1a-launch-item-1").expect("item id");
    let first_patch_id = PatchId::parse("s1a-launch-patch-1").expect("patch id");
    let second_patch_id = PatchId::parse("s1a-launch-patch-2").expect("patch id");
    let start_key = RunStartKey::new(START_KEY_BYTES);
    let credentials = RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES);
    let LaunchClaimedRunOutcome::Started(launched) = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &item_id,
            first_patch_id: &first_patch_id,
            second_patch_id: &second_patch_id,
            operated_at: UnixMillis::from_millis(OPERATED_AT_MS),
            run_start_key: &start_key,
            credentials: &credentials,
            engine_settings: &engine_settings,
        })
        .await
        .expect("launch")
    else {
        panic!("launch should start the run")
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let bound = match repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
            bound_at: UnixMillis::from_millis(BOUND_AT_MS),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await
        .expect("bind")
    {
        BindRunProviderOutcome::Bound(receipt) => receipt,
        BindRunProviderOutcome::AlreadyBound(_) => panic!("bind should be fresh"),
    };
    SeededPair {
        database,
        repository,
        claimed,
        launched,
        bound,
        start_key,
        credentials,
    }
}

fn error_ref() -> EngineErrorRef {
    EngineErrorRef::new(EngineErrorRefInput {
        artisan_code: ArtisanCode::parse("AE-PROVIDER-206".to_owned())
            .expect("fixture artisan code is valid"),
        provider_code: Some("quota_exhausted".to_owned()),
        detail: Some("model allowance depleted".to_owned()),
        affected_model_id: None,
        limit_id: Some("weekly".to_owned()),
        limit_label: None,
        limit_scope: Some(LimitScope::Shared),
        resets_at: Some("2026-09-06T00:00:00Z".to_owned()),
    })
    .expect("fixture error reference is valid")
}

/// One fixture observation per durable shape: all twenty-two variants with
/// strictly increasing sequences, covering requested and resolved approval
/// and question states.
fn all_variant_observations() -> Vec<Observation> {
    let mut observations = message_observations();
    observations.extend(activity_observations());
    observations.extend(interaction_observations());
    observations.extend(lifecycle_observations());
    observations.extend(closing_observations());
    observations
}

fn message_observations() -> Vec<Observation> {
    vec![
        Observation::AgentMessageDelta(
            AgentMessageDeltaObservation::new(
                oid("s1a-obs-1"),
                seq(1),
                oid("s1a-item-1"),
                MessagePhase::Commentary,
                "partial text".to_owned(),
                oid("s1a-turn-1"),
            )
            .expect("delta is valid"),
        ),
        Observation::AgentMessageCompleted(
            AgentMessageCompletedObservation::new(
                oid("s1a-obs-2"),
                seq(2),
                oid("s1a-item-1"),
                MessagePhase::Final,
                "settled text".to_owned(),
                oid("s1a-turn-1"),
            )
            .expect("completed message is valid"),
        ),
        Observation::ReasoningSummaryDelta(
            ReasoningSummaryDeltaObservation::new(
                oid("s1a-obs-3"),
                seq(3),
                oid("s1a-reasoning-1"),
                0,
                "considering options".to_owned(),
                None,
                oid("s1a-turn-1"),
            )
            .expect("reasoning delta is valid"),
        ),
        Observation::ReasoningSummaryCompleted(
            ReasoningSummaryCompletedObservation::new(
                oid("s1a-obs-4"),
                seq(4),
                oid("s1a-reasoning-1"),
                None,
                oid("s1a-turn-1"),
            )
            .expect("complete-without-delta settles"),
        ),
    ]
}

fn activity_observations() -> Vec<Observation> {
    vec![
        Observation::Tool(
            ToolObservation::new(
                oid("s1a-obs-5"),
                seq(5),
                oid("s1a-tool-1"),
                "bash".to_owned(),
                ToolAction::Completed,
                Some("ran tests".to_owned()),
            )
            .expect("tool row is valid"),
        ),
        Observation::File(
            FileObservation::new(
                oid("s1a-obs-6"),
                seq(6),
                "src/main.rs".to_owned(),
                FileAction::Modified,
                Some(12),
                Some(3),
            )
            .expect("file row is valid"),
        ),
        Observation::Search(
            SearchObservation::new(
                oid("s1a-obs-7"),
                seq(7),
                "observation codec".to_owned(),
                Some(SearchScope::Workspace),
                Some(oid("s1a-search-1")),
                SearchState::Completed,
                Some(4),
            )
            .expect("search row is valid"),
        ),
        Observation::TerminalActivity(
            TerminalActivityObservation::new(
                oid("s1a-obs-8"),
                seq(8),
                TerminalActivityInput {
                    activity_id: oid("s1a-activity-1"),
                    channel: Some(TerminalChannel::Stdout),
                    command: Some("cargo test".to_owned()),
                    shell: Some("bash".to_owned()),
                    output: Some("ok".to_owned()),
                    exit_code: Some(0),
                    state: TerminalActivityState::Completed,
                },
            )
            .expect("terminal row is valid"),
        ),
    ]
}

fn command_request() -> ApprovalRequest {
    ApprovalRequest::command(
        "rm -rf /tmp/work".to_owned(),
        Some("/tmp/work".to_owned()),
        None,
    )
    .expect("command request is valid")
}

fn question_input() -> QuestionInput {
    QuestionInput {
        question_id: oid("s1a-question-1"),
        text: "Which runtime first?".to_owned(),
        header: Some("Runtime".to_owned()),
        multi_select: false,
        options: Some(vec![
            QuestionOption::new("OpenCode".to_owned(), None).expect("option is valid"),
        ]),
    }
}

fn interaction_observations() -> Vec<Observation> {
    vec![
        Observation::Approval(
            ApprovalObservation::requested(
                oid("s1a-obs-9"),
                seq(9),
                oid("s1a-approval-1"),
                "Remove the scratch directory?".to_owned(),
                command_request(),
            )
            .expect("approval request is valid"),
        ),
        Observation::Approval(
            ApprovalObservation::resolved(
                oid("s1a-obs-10"),
                seq(10),
                oid("s1a-approval-1"),
                "Remove the scratch directory?".to_owned(),
                command_request(),
                true,
            )
            .expect("approval resolution is valid"),
        ),
        Observation::Question(
            QuestionObservation::requested(oid("s1a-obs-11"), seq(11), question_input())
                .expect("question request is valid"),
        ),
        Observation::Question(
            QuestionObservation::resolved(
                oid("s1a-obs-12"),
                seq(12),
                question_input(),
                vec!["OpenCode".to_owned()],
            )
            .expect("question resolution is valid"),
        ),
    ]
}

fn lifecycle_observations() -> Vec<Observation> {
    vec![
        Observation::Plan(
            PlanObservation::new(
                oid("s1a-obs-13"),
                seq(13),
                vec![
                    PlanEntry::new(
                        oid("s1a-step-1"),
                        PlanEntryStatus::Completed,
                        "Inventory".to_owned(),
                    )
                    .expect("entry is valid"),
                    PlanEntry::new(
                        oid("s1a-step-2"),
                        PlanEntryStatus::InProgress,
                        "Implement".to_owned(),
                    )
                    .expect("entry is valid"),
                ],
                Some(oid("s1a-turn-1")),
            )
            .expect("plan is valid"),
        ),
        Observation::Compaction(
            CompactionObservation::new(
                oid("s1a-obs-14"),
                seq(14),
                CompactionState::Completed,
                None,
                Some(1_250),
                Some("summarized".to_owned()),
            )
            .expect("compaction is valid"),
        ),
        Observation::Retry(
            RetryObservation::new(
                oid("s1a-obs-15"),
                seq(15),
                oid("s1a-turn-1"),
                RetryAttemptState::Retrying,
                true,
                "upstream hiccup".to_owned(),
            )
            .expect("retry is valid"),
        ),
        Observation::RunState(RunStateObservation::new(
            oid("s1a-obs-16"),
            seq(16),
            RunState::Running,
        )),
        Observation::TurnState(TurnStateObservation::new(
            oid("s1a-obs-17"),
            seq(17),
            oid("s1a-turn-1"),
            TurnState::Started,
        )),
        Observation::Subagent(
            SubagentObservation::new(
                oid("s1a-obs-18"),
                seq(18),
                SubagentInput {
                    agent_native_thread_id: oid("s1a-child-1"),
                    parent_native_thread_id: oid("s1a-parent-1"),
                    state: SubagentState::Running,
                    activity: Some("researching".to_owned()),
                    agent_path: None,
                    turn_id: None,
                },
            )
            .expect("subagent row is valid"),
        ),
        Observation::SubagentTranscript(SubagentTranscriptObservation::new(
            oid("s1a-obs-19"),
            seq(19),
            oid("s1a-child-1"),
            oid("s1a-parent-1"),
            TranscriptContent::project(&Observation::AgentMessageDelta(
                AgentMessageDeltaObservation::new(
                    oid("s1a-root-delta"),
                    seq(19),
                    oid("s1a-child-item-1"),
                    MessagePhase::Commentary,
                    "child progress".to_owned(),
                    oid("s1a-child-turn-1"),
                )
                .expect("root delta is valid"),
            ))
            .expect("delta projects into transcripts"),
        )),
    ]
}

fn closing_observations() -> Vec<Observation> {
    vec![
        Observation::Usage(
            UsageObservation::new(
                oid("s1a-obs-20"),
                seq(20),
                UsageInput {
                    basis: UsageBasis::Cumulative,
                    input_tokens: Some(120),
                    cached_input_tokens: Some(40),
                    output_tokens: Some(60),
                    context_tokens: Some(9_000),
                    context_window_tokens: Some(200_000),
                    cost_usd: Some(0.02),
                    provider_route_id: Some(oid("s1a-route-1")),
                    turn_id: Some(oid("s1a-turn-1")),
                },
            )
            .expect("usage is valid"),
        ),
        Observation::NativeAction(
            NativeActionObservation::new(
                oid("s1a-obs-21"),
                seq(21),
                "provider.deploy".to_owned(),
                Some("rolled out".to_owned()),
                false,
                None,
            )
            .expect("native action is valid"),
        ),
        Observation::ProcessDiagnostic(
            ProcessDiagnosticObservation::new(
                oid("s1a-obs-22"),
                seq(22),
                DiagnosticLevel::Error,
                "child exited".to_owned(),
                Some(error_ref()),
            )
            .expect("process diagnostic is valid"),
        ),
        Observation::ProtocolDiagnostic(
            ProtocolDiagnosticObservation::new(
                oid("s1a-obs-23"),
                seq(23),
                DiagnosticLevel::Warning,
                "unknown frame ignored".to_owned(),
            )
            .expect("protocol diagnostic is valid"),
        ),
        Observation::RunTerminal(
            RunTerminalObservation::new(
                oid("s1a-obs-24"),
                seq(24),
                RunTerminalState::Completed,
                None,
                Some("Session title".to_owned()),
            )
            .expect("terminal outcome is valid"),
        ),
    ]
}

async fn committed_checkpoint_row(
    database: &DatabaseConnection,
) -> entities::run_checkpoint::Model {
    entities::run_checkpoint::Entity::find_by_id(RUN_ID)
        .one(database)
        .await
        .expect("checkpoint query should succeed")
        .expect("a checkpoint row should exist")
}

fn batch_scope(pair: &SeededPair, expected_updated_at: UnixMillis) -> RunBatchScope<'_> {
    RunBatchScope {
        claimed: &pair.claimed,
        launched: &pair.launched,
        bound: &pair.bound,
        run_start_key: &pair.start_key,
        credentials: &pair.credentials,
        expected_launch_at: UnixMillis::from_millis(OPERATED_AT_MS),
        expected_updated_at,
    }
}

#[tokio::test]
async fn all_variants_commit_through_existing_batch_path_without_migration() {
    let pair = seeded_pair().await;
    let observations = all_variant_observations();
    validate_observation_bind(1, &pair.bound).expect("fixture bind should agree");
    let checkpoint = encode_observation_checkpoint(EngineId::OpenCode2, 1, None, &observations)
        .expect("fixture batch should encode");

    let body = AssistantBody::parse("s1a progress").expect("body");
    let item_id = ItemId::parse("s1a-batch-item-1").expect("item id");
    let activation = PatchId::parse("s1a-activate-1").expect("patch id");
    let patch_item = PatchId::parse("s1a-batch-patch-1").expect("patch id");
    let outcome = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS)),
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&activation),
            changes: &[AssistantChange::Start {
                item_id: &item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: &patch_item,
            }],
            checkpoint: CheckpointUpdate::Replace(&checkpoint),
        })
        .await
        .expect("observation batch should commit through the existing path");
    let CommitRunBatchOutcome::Committed(info) = outcome else {
        panic!("first observation batch should commit")
    };
    assert_eq!(info.batch_sequence, 1);

    // No-migration proof: the observations persist in the pre-existing
    // `run_checkpoints` row written by the unmodified batch path.
    let row = committed_checkpoint_row(&pair.database).await;
    assert_eq!(row.last_batch_sequence, 1);
    assert_eq!(
        row.engine_checkpoint_version,
        Some(OBSERVATION_CHECKPOINT_VERSION)
    );
    let blob = row
        .engine_checkpoint_blob
        .as_ref()
        .expect("checkpoint blob should exist");
    let decoded = decode_observation_checkpoint(
        row.engine_checkpoint_version.expect("version should exist"),
        blob.as_slice(),
    )
    .expect("persisted bytes should decode");
    assert_eq!(decoded.engine(), EngineId::OpenCode2);
    assert_eq!(decoded.binding_version(), 1);
    assert_eq!(decoded.max_sequence(), Some(24));
    assert_eq!(decoded.observations(), &observations);
    validate_observation_engine(EngineId::OpenCode2, &decoded)
        .expect("engine history should continue");
}

#[tokio::test]
async fn continuation_batch_enforces_monotonic_sequences() {
    let pair = seeded_pair().await;
    let first = all_variant_observations();
    let checkpoint = encode_observation_checkpoint(EngineId::OpenCode2, 1, None, &first)
        .expect("first batch should encode");
    let body = AssistantBody::parse("s1a progress").expect("body");
    let item_id = ItemId::parse("s1a-batch-item-1").expect("item id");
    let activation = PatchId::parse("s1a-activate-1").expect("patch id");
    let patch_item = PatchId::parse("s1a-batch-patch-1").expect("patch id");
    pair.repository
        .commit_run_batch(CommitRunBatch {
            scope: batch_scope(&pair, UnixMillis::from_millis(BOUND_AT_MS)),
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS),
            activate_turn_patch_id: Some(&activation),
            changes: &[AssistantChange::Start {
                item_id: &item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: &patch_item,
            }],
            checkpoint: CheckpointUpdate::Replace(&checkpoint),
        })
        .await
        .expect("first batch should commit");

    let row = committed_checkpoint_row(&pair.database).await;
    let blob = row.engine_checkpoint_blob.as_ref().expect("blob");
    let previous: DecodedObservationBatch = decode_observation_checkpoint(
        row.engine_checkpoint_version.expect("version"),
        blob.as_slice(),
    )
    .expect("first batch should decode");
    let base = previous.max_sequence().expect("first batch has a maximum");

    let next = vec![
        Observation::RunState(RunStateObservation::new(
            oid("s1a-obs-25"),
            seq(25),
            RunState::Waiting,
        )),
        Observation::TurnState(TurnStateObservation::new(
            oid("s1a-obs-26"),
            seq(26),
            oid("s1a-turn-1"),
            TurnState::Waiting,
        )),
    ];
    let checkpoint = encode_observation_checkpoint(EngineId::OpenCode2, 1, Some(base), &next)
        .expect("continuation should encode");
    let fragment = IncrementalText::parse(" more").expect("fragment");
    let patch_append = PatchId::parse("s1a-batch-patch-2").expect("patch id");
    let outcome = pair
        .repository
        .commit_run_batch(CommitRunBatch {
            scope: batch_scope(&pair, UnixMillis::from_millis(BATCH_OPERATED_AT_MS)),
            batch_sequence: 2,
            operated_at: UnixMillis::from_millis(BATCH_OPERATED_AT_MS_2),
            activate_turn_patch_id: None,
            changes: &[AssistantChange::Append {
                item_id: &item_id,
                expected_revision: Revision::new(0),
                text: &fragment,
                patch_id: &patch_append,
            }],
            checkpoint: CheckpointUpdate::Replace(&checkpoint),
        })
        .await
        .expect("continuation batch should commit");
    let CommitRunBatchOutcome::Committed(info) = outcome else {
        panic!("continuation batch should commit")
    };
    assert_eq!(info.batch_sequence, 2);

    let row = committed_checkpoint_row(&pair.database).await;
    assert_eq!(row.last_batch_sequence, 2);
    let blob = row.engine_checkpoint_blob.as_ref().expect("blob");
    let decoded = decode_observation_checkpoint(
        row.engine_checkpoint_version.expect("version"),
        blob.as_slice(),
    )
    .expect("continuation should decode");
    assert_eq!(decoded.observations(), &next);
    assert_eq!(decoded.max_sequence(), Some(26));

    // Stale and unordered sequences never encode, so they can never commit.
    assert!(
        matches!(
            encode_observation_checkpoint(EngineId::OpenCode2, 1, Some(26), &next),
            Err(ObservationCommitError::SequenceNotMonotonic)
        ),
        "replaying the same base is not a continuation"
    );
    let unordered = vec![
        Observation::RunState(RunStateObservation::new(
            oid("s1a-obs-27"),
            seq(28),
            RunState::Running,
        )),
        Observation::RunState(RunStateObservation::new(
            oid("s1a-obs-28"),
            seq(27),
            RunState::Running,
        )),
    ];
    assert!(
        matches!(
            encode_observation_checkpoint(EngineId::OpenCode2, 1, Some(26), &unordered),
            Err(ObservationCommitError::SequenceNotMonotonic)
        ),
        "sequences must increase within a batch"
    );
}

#[tokio::test]
async fn bind_mismatch_rejection() {
    let pair = seeded_pair().await;
    validate_observation_bind(1, &pair.bound).expect("matching version should agree");
    assert_eq!(
        validate_observation_bind(2, &pair.bound)
            .expect_err("a rebound version must not carry old observations"),
        ObservationCommitError::BindMismatch
    );
    assert!(
        matches!(
            encode_observation_checkpoint(
                EngineId::OpenCode2,
                0,
                None,
                &all_variant_observations()
            ),
            Err(ObservationCommitError::InvalidObservation(
                ObservationError::OutOfRange {
                    field: "binding_version"
                }
            ))
        ),
        "binding versions are positive"
    );
}

#[test]
fn engine_continuity_rejection() {
    let observations = all_variant_observations();
    let bytes = encode_observation_bytes(EngineId::OpenCode2, 1, None, &observations)
        .expect("fixture batch should encode");
    let decoded = decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, &bytes)
        .expect("fixture batch should decode");
    validate_observation_engine(EngineId::OpenCode2, &decoded)
        .expect("matching engine should continue");
    assert_eq!(
        validate_observation_engine(EngineId::Codex, &decoded)
            .expect_err("engine histories never mix"),
        ObservationCommitError::EngineMismatch
    );

    let foreign = String::from_utf8(bytes).expect("canonical bytes are UTF-8");
    let foreign = foreign.replace("\"engine\":\"opencode2\"", "\"engine\":\"nope\"");
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, foreign.as_bytes())
            .expect_err("unknown engine tags reject"),
        ObservationCommitError::UnknownEngine
    );
}

fn single_tool_bytes() -> Vec<u8> {
    let observations = vec![Observation::Tool(
        ToolObservation::new(
            oid("s1a-tool-obs"),
            seq(1),
            oid("s1a-tool-1"),
            "bash".to_owned(),
            ToolAction::Started,
            None,
        )
        .expect("tool row is valid"),
    )];
    encode_observation_bytes(EngineId::OpenCode2, 1, None, &observations)
        .expect("single tool batch should encode")
}

fn single_delta_bytes() -> Vec<u8> {
    let observations = vec![Observation::AgentMessageDelta(
        AgentMessageDeltaObservation::new(
            oid("s1a-delta-obs"),
            seq(1),
            oid("s1a-item-9"),
            MessagePhase::Commentary,
            "partial".to_owned(),
            oid("s1a-turn-9"),
        )
        .expect("delta is valid"),
    )];
    encode_observation_bytes(EngineId::OpenCode2, 1, None, &observations)
        .expect("single delta batch should encode")
}

#[test]
fn unknown_tags_and_provider_values_reject_typed() {
    let bytes = single_tool_bytes();
    let text = String::from_utf8(bytes).expect("canonical bytes are UTF-8");
    let unknown_tag = text.replace("\"tag\":\"tool\"", "\"tag\":\"teleport\"");
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, unknown_tag.as_bytes())
            .expect_err("unknown tags reject"),
        ObservationCommitError::UnknownObservation
    );

    let bytes = single_delta_bytes();
    let text = String::from_utf8(bytes).expect("canonical bytes are UTF-8");
    let unknown_phase = text.replace("\"phase\":\"commentary\"", "\"phase\":\"shouting\"");
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, unknown_phase.as_bytes())
            .expect_err("unknown provider values reject typed"),
        ObservationCommitError::InvalidObservation(ObservationError::UnknownValue {
            field: "phase"
        })
    );
}

#[test]
fn malformed_format_and_canonicality_rejection() {
    let bytes = single_tool_bytes();
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, &bytes[..bytes.len() / 2],)
            .expect_err("truncated bytes are malformed"),
        ObservationCommitError::Malformed
    );
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, b"not json")
            .expect_err("non-JSON bytes are malformed"),
        ObservationCommitError::Malformed
    );
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION + 1, &bytes)
            .expect_err("checkpoint versions must match"),
        ObservationCommitError::VersionMismatch
    );

    let text = String::from_utf8(bytes).expect("canonical bytes are UTF-8");
    let wrong_format = text.replace(OBSERVATION_FORMAT_TAG, "other.format");
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, wrong_format.as_bytes())
            .expect_err("format tags must match"),
        ObservationCommitError::FormatMismatch
    );
    let wrong_inner_version = text.replace("\"version\":1", "\"version\":2");
    assert_eq!(
        decode_observation_checkpoint(
            OBSERVATION_CHECKPOINT_VERSION,
            wrong_inner_version.as_bytes(),
        )
        .expect_err("inner versions must match"),
        ObservationCommitError::VersionMismatch
    );
    let spaced = text.replace("{\"format\"", "{ \"format\"");
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, spaced.as_bytes())
            .expect_err("only canonical bytes decode"),
        ObservationCommitError::NonCanonical
    );

    let empty = format!(
        "{{\"format\":\"{OBSERVATION_FORMAT_TAG}\",\"version\":1,\"engine\":\"opencode2\",\
         \"binding_version\":1,\"observations\":[]}}"
    );
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, empty.as_bytes())
            .expect_err("empty batches reject"),
        ObservationCommitError::EmptyBatch
    );
    assert_eq!(
        encode_observation_bytes(EngineId::OpenCode2, 1, None, &[])
            .expect_err("empty batches never encode"),
        ObservationCommitError::EmptyBatch
    );

    let mut crowded = String::from("{\"format\":\"");
    crowded.push_str(OBSERVATION_FORMAT_TAG);
    crowded.push_str(
        "\",\"version\":1,\"engine\":\"opencode2\",\"binding_version\":1,\"observations\":[",
    );
    for index in 0..OBSERVATION_BATCH_MAX_OBSERVATIONS + 1 {
        if index > 0 {
            crowded.push(',');
        }
        crowded.push_str(&format!(
            "{{\"tag\":\"tool\",\"id\":\"o-{index}\",\"sequence\":{},\"tool_id\":\"t-{index}\",\
             \"tool_name\":\"bash\",\"action\":\"started\",\"detail\":null}}",
            index + 1
        ));
    }
    crowded.push_str("]}");
    assert_eq!(
        decode_observation_checkpoint(OBSERVATION_CHECKPOINT_VERSION, crowded.as_bytes())
            .expect_err("oversized batches reject on decode"),
        ObservationCommitError::TooMany {
            count: OBSERVATION_BATCH_MAX_OBSERVATIONS + 1,
            maximum: OBSERVATION_BATCH_MAX_OBSERVATIONS,
        }
    );
}

#[test]
fn oversize_rejection() {
    let heavy: Vec<Observation> = (0..OBSERVATION_BATCH_MAX_OBSERVATIONS)
        .map(|index| {
            Observation::AgentMessageCompleted(
                AgentMessageCompletedObservation::new(
                    oid(&format!("s1a-heavy-{index}")),
                    seq(u64::try_from(index).expect("index fits") + 1),
                    oid(&format!("s1a-heavy-item-{index}")),
                    MessagePhase::Final,
                    "x".repeat(65_536),
                    oid(&format!("s1a-heavy-turn-{index}")),
                )
                .expect("maximal bodies are valid"),
            )
        })
        .collect();
    match encode_observation_bytes(EngineId::OpenCode2, 1, None, &heavy)
        .expect_err("a full batch of maximal bodies exceeds the checkpoint ceiling")
    {
        ObservationCommitError::TooLarge { maximum, .. } => {
            assert_eq!(maximum, 262_144);
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
}
