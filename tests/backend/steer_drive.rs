//! Production steer-drain proof against the Codex wire fixture.
//!
//! These tests drive the real dispatch steer arm
//! (`super::handle_steer`) against a live Codex owner turn backed by the
//! `codex-wire-steer_burst` / `codex-wire-steer_reject` fixture scenarios:
//!
//! - the burst turn emits 64 valid same-item deltas through an observation
//!   channel of capacity 4 before its correlated steer ack, so reading the
//!   ack causally requires the production inline drain;
//! - exactly one provider write is recorded per steer
//!   (`steer-requests.jsonl` in the project cwd); redelivery must never add
//!   a second;
//! - the steered message completes while the original turn is still live
//!   (no terminal observed before the `Steered` ack);
//! - the reject turn answers a typed provider error with the payload
//!   preserved;
//! - a pre-ack cancellation records nothing and leaves the row open, so a
//!   retry performs exactly one provider write;
//! - a known-acked redelivery against an open row replays the idempotent
//!   projection through the real `Duplicate` branch without provider
//!   contact.
//!
//! No live provider is involved: the fixture executable stands in for the
//! Codex CLI over stdio, exactly like the wire tests. The ledger
//! preflight unit tests at the end pin the nonmutating contract the arm
//! relies on.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use artisan_database::{
    AttachProjectInput, BindRunProvider, BindRunProviderOutcome, BoundRunReceipt,
    ClaimMessageDispatch, ClaimedMessageDispatch, CreateThreadInput, DispatchLeaseOwner,
    LaunchClaimedRun, LaunchClaimedRunOutcome, LaunchedRunReceipt, ProviderBindingBytes,
    QueueFirstMessageInput, QueueMessageInput, Repository, RunBatchScope, RunLaunchCredentials,
    RunStartKey, SetThreadEngineConfigInput, SqliteConfig, ThreadEngineSettings, connect,
    entities as database_entities,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CodexModelContextWindow, CodexReasoningEffort, CodexSelection,
    CodexServiceTier, CountLimit, DirectoryId, DisplayName, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineId, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, ItemId, MessageBody, MessageId, NetworkAccess,
    ObservationId, PatchId, PermissionId, ProjectId, QueueMessagePayload, RequestId, RootPath,
    RunId, ThreadId, ThreadTitle, TurnId, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use artisan_native_engine::{NativeCodexAuthority, NativeOpenCode2Authority};
use artisan_transport::CancelHandle;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use super::{
    NativeRunDispatcherConfig, NativeRunDispatcherConfigInput, TurnConsumptionContext,
    TurnConsumptionState, handle_observation, handle_steer,
};
use crate::{
    SystemCommandOrigin,
    conversation_commit_notifier::ConversationCommitNotifier,
    engine_owner::observation::{EngineObservation, TerminalState},
    engine_owner::{EngineCodexTurnInput, EngineOwner, EngineOwnerShutdown},
    run_interaction::RunInteractionAck,
};

const STEER_DEADLINE: Duration = Duration::from_secs(90);
const OBSERVE_DEADLINE: Duration = Duration::from_secs(20);

/// Scratch project root (also the fixture cwd) plus database path.
struct SteerTempRoot {
    dir: PathBuf,
    root: RootPath,
    db_path: PathBuf,
}

impl SteerTempRoot {
    fn new(label: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "artisan-steer-drive-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("steer temp root");
        let root = RootPath::parse(dir.to_str().expect("temp path utf8")).expect("root");
        let db_path = dir.join("steer.sqlite3");
        Self { dir, root, db_path }
    }
}

impl Drop for SteerTempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Resolves the built wire-fixture executable, mirroring the wire tests.
fn steer_fixture_program() -> PathBuf {
    if let Ok(path) = std::env::var("ARTISAN_CODEX_WIRE_FIXTURE") {
        let mapping = PathBuf::from(&path);
        let path = if mapping.is_absolute() {
            mapping
        } else {
            let runfiles = runfiles::Runfiles::create().expect("runfiles discovery");
            runfiles::rlocation!(runfiles, path.as_str()).expect("wire fixture runfile")
        };
        assert!(
            path.is_file(),
            "declared wire fixture must be a regular file"
        );
        return path;
    }
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codex_wire_fixture") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return path;
        }
    }
    panic!("wire fixture binary not found; set ARTISAN_CODEX_WIRE_FIXTURE");
}

/// Copies the built fixture to a per-test executable whose basename names
/// the frozen scenario (`steer_burst`, `steer_reject`).
fn steer_scenario_program(fixture: &PathBuf, dir: &PathBuf, scenario: &str) -> PathBuf {
    let named = dir.join(format!(
        "codex-wire-{scenario}{}",
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::copy(fixture, &named).expect("wire fixture copies per test");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = std::fs::metadata(&named).expect("meta").permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&named, permissions).expect("chmod");
    }
    named
}

fn steer_runtime() -> EngineRuntimeControls {
    let budget = |ms: u64| FiniteMillis::new(ms).expect("finite millis valid");
    EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: budget(45_000),
        readiness_budget: budget(5_000),
        health_budget: budget(5_000),
        prompt_budget: budget(15_000),
        stream_budget: budget(15_000),
        close_budget: budget(5_000),
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness limit"),
        max_header_count: CountLimit::new(32).expect("header count"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr"),
        // Four slots: the 64-frame burst causally requires the production
        // inline drain before its correlated ack can be read.
        observation_capacity: CountLimit::new(4).expect("observation cap"),
    })
    .expect("runtime valid")
}

fn steer_codex_config() -> EngineRunConfig {
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-steer").expect("permission id"),
        EngineAgentId::parse("agent-steer").expect("agent id"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::Codex(
            CodexSelection::new(
                EngineProfileId::parse("codex-fixture").expect("profile id"),
                Some(EngineModelId::parse("codex-model").expect("model id")),
                permission,
                Some(CodexReasoningEffort::High),
                Some(CodexServiceTier::Fast),
                Some(CodexModelContextWindow::new(1_000).expect("window")),
            )
            .expect("codex steer selection valid"),
        ),
        steer_runtime(),
    )
}

/// Real-clock base for seed chronology: observation commits fence the
/// dispatch lease against the real origin clock, so fixed millisecond
/// seeds would expire before the test runs.
fn seed_base_ms() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time after epoch")
        .as_millis();
    i64::try_from(now).expect("millis fit") - 60_000
}

/// Real durable launch/bind receipts backing one live Codex run.
struct SteerLaunch {
    run_id: RunId,
    turn_id: TurnId,
    claimed: ClaimedMessageDispatch,
    launched: LaunchedRunReceipt,
    bound: BoundRunReceipt,
    run_start_key: RunStartKey,
    credentials: RunLaunchCredentials,
    settings: ThreadEngineSettings,
    launch_op_ms: i64,
    bound_op_ms: i64,
}

async fn seed_steer_run(
    repository: &Repository,
    thread_id: &ThreadId,
    label: &str,
    run_id: &str,
    turn_id: &str,
) -> SteerLaunch {
    let project_id = ProjectId::parse(format!("project-{label}")).expect("project id");
    // All seed times anchor to the real clock: observation commits fence
    // the dispatch lease against the real origin clock, and the launch and
    // bind fences order creation, launch, and binding.
    let base = seed_base_ms();
    let at = |offset: i64| UnixMillis::from_millis(base + offset);
    let launch_op_ms = base + 490;
    let bound_op_ms = base + 590;
    repository
        .attach_project(AttachProjectInput {
            request_id: RequestId::parse(format!("request-project-{label}")).expect("request id"),
            directory_id: DirectoryId::parse(format!("directory-{label}")).expect("directory id"),
            project_id: project_id.clone(),
            root_path: RootPath::parse(format!("C:/repos/project-{label}")).expect("root path"),
            display_name: DisplayName::parse("Project").expect("display name"),
            attached_at: at(0),
        })
        .await
        .expect("project should attach");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse(format!("request-thread-{label}")).expect("request id"),
            thread_id: thread_id.clone(),
            project_id,
            title: ThreadTitle::parse("Steer thread").expect("title"),
            created_at: at(0),
            updated_at: at(0),
        })
        .await
        .expect("thread should create");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse(format!("request-config-{label}")).expect("request id"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: steer_codex_config(),
            accepted_at: at(0),
        })
        .await
        .expect("engine configuration should persist");
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse(format!("request-message-{label}")).expect("request id"),
            message_id: MessageId::parse(format!("message-{label}")).expect("message id"),
            thread_id: thread_id.clone(),
            body: MessageBody::parse("hello steer").expect("message body"),
            accepted_at: at(40),
        })
        .await
        .expect("first message should queue");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x11; 32]),
            claimed_at: at(390),
            // The lease must outlive every real-clock commit below.
            lease_expires_at: UnixMillis::from_millis(base + 3_600_000),
        })
        .await
        .expect("claim should persist")
        .expect("dispatch should be claimable");
    let run_id = RunId::parse(run_id).expect("run id");
    let turn_id = TurnId::parse(turn_id).expect("turn id");
    let settings = repository
        .read_thread_engine_settings(thread_id)
        .await
        .expect("settings should read")
        .expect("settings should exist");
    let run_start_key = RunStartKey::new([0x44; 32]);
    let credentials = RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]);
    let launched = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &ItemId::parse(format!("item-{label}")).expect("item id"),
            first_patch_id: &PatchId::parse(format!("patch-{label}-first")).expect("patch id"),
            second_patch_id: &PatchId::parse(format!("patch-{label}-second")).expect("patch id"),
            operated_at: UnixMillis::from_millis(launch_op_ms),
            run_start_key: &run_start_key,
            credentials: &credentials,
            engine_settings: &settings,
        })
        .await
        .expect("launch should persist");
    let launched = match launched {
        LaunchClaimedRunOutcome::Started(receipt)
        | LaunchClaimedRunOutcome::AlreadyStarted(receipt) => receipt,
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding bytes");
    let bound = repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &run_start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(launch_op_ms),
            bound_at: UnixMillis::from_millis(bound_op_ms),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await
        .expect("bind should persist");
    let bound = match bound {
        BindRunProviderOutcome::Bound(receipt) | BindRunProviderOutcome::AlreadyBound(receipt) => {
            receipt
        }
    };
    SteerLaunch {
        run_id,
        turn_id,
        claimed,
        launched,
        bound,
        run_start_key,
        credentials,
        settings,
        launch_op_ms,
        bound_op_ms,
    }
}

fn test_dispatcher_config() -> NativeRunDispatcherConfig {
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
            prompt_delivery: "queue".to_owned(),
            stream_after: 0,
        },
    )
    .expect("test scheduler policy should validate")
}

/// Counts appended `steer-requests.jsonl` lines in the fixture cwd.
fn steer_request_lines(dir: &PathBuf) -> Vec<String> {
    let path = dir.join("steer-requests.jsonl");
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    body.lines().map(str::to_owned).collect()
}

async fn patch_count(database: &sea_orm::DatabaseConnection, thread_id: &ThreadId) -> u64 {
    use database_entities::conversation_patch;
    conversation_patch::Entity::find()
        .filter(conversation_patch::Column::ThreadId.eq(thread_id.as_str()))
        .all(database)
        .await
        .expect("patches should read")
        .len() as u64
}

#[tokio::test]
async fn codex_steer_burst_drains_sixty_four_through_production_handle_steer() {
    tokio::time::timeout(STEER_DEADLINE, async {
        let fixture = steer_fixture_program();
        let temp = SteerTempRoot::new("burst");
        let program = steer_scenario_program(&fixture, &temp.dir, "steer_burst");
        let database = connect(SqliteConfig::file(&temp.db_path).sqlx_logging(false))
            .await
            .expect("file database should open");
        migrate_to_current(&database)
            .await
            .expect("migrations should apply");
        let repository = Repository::new(database.clone());
        let thread_id = ThreadId::parse("thread-fixture-1").expect("thread id");
        let seed = seed_steer_run(
            &repository,
            &thread_id,
            "burst",
            "run-steer-burst",
            "turn-steer-burst",
        )
        .await;

        let launch = NativeCodexAuthority::new()
            .resolve_launch_with_executable(
                &temp.db_path,
                &EngineProfileId::parse("codex-fixture").expect("profile id"),
                &program,
                "codex-cli 0.145.0",
            )
            .expect("wire launch resolves");
        let mut owner = EngineOwner::start_configured(
            NonZeroUsize::new(1).expect("one slot"),
            &tokio::runtime::Handle::current(),
        );
        let mut turn = owner
            .admit_codex_turn(
                EngineCodexTurnInput {
                    run_id: seed.run_id.clone(),
                    thread_id: thread_id.clone(),
                    project_root: temp.root.clone(),
                    prompt_id: "prompt-steer-1".to_owned(),
                    prompt: QueueMessagePayload::text_only("hello steer").expect("payload"),
                    settings: seed.settings.clone(),
                    launch,
                    continuation: None,
                    prompt_delivery: "immediate".to_owned(),
                    stream_after: 0,
                    control_capacity: 1,
                },
                Duration::from_secs(50),
            )
            .expect("wire turn admits");
        turn.prepare().await.expect("wire turn prepares");
        turn.authorize().expect("wire turn authorizes once");

        // The steered message is accepted after the Codex configuration, so
        // its receipt snapshot names the live engine.
        let command_id = RequestId::parse("request-steer-1").expect("request id");
        let message_id = MessageId::parse("message-steer-1").expect("message id");
        repository
            .queue_message(QueueMessageInput {
                request_id: command_id.clone(),
                message_id: message_id.clone(),
                thread_id: thread_id.clone(),
                payload: QueueMessagePayload::text_only("follow up").expect("payload"),
                steer_run_id: Some(seed.run_id.clone()),
                accepted_at: UnixMillis::from_millis(100),
            })
            .await
            .expect("steered message should queue");

        let config = test_dispatcher_config();
        let origin = SystemCommandOrigin;
        let stop = CancelHandle::new();
        let process_cancel = CancelHandle::new();
        let run_cancel = CancelHandle::new();
        let context = TurnConsumptionContext {
            repository: &repository,
            config: &config,
            origin: &origin,
            stop: &stop,
            process_cancel: &process_cancel,
            run_cancel: &run_cancel,
        };
        let mut state = TurnConsumptionState::new(
            RunBatchScope {
                claimed: &seed.claimed,
                launched: &seed.launched,
                bound: &seed.bound,
                run_start_key: &seed.run_start_key,
                credentials: &seed.credentials,
                expected_launch_at: UnixMillis::from_millis(seed.launch_op_ms),
                expected_updated_at: UnixMillis::from_millis(seed.bound_op_ms),
            },
            EngineId::Codex,
        );
        // The opening delta goes through the real observation handler like
        // every later frame, keeping state and durable counters honest
        // before the steer is sent.
        let initial = tokio::time::timeout(OBSERVE_DEADLINE, turn.next_observation())
            .await
            .expect("initial observation settles")
            .expect("initial observation exists");
        assert!(matches!(initial, EngineObservation::TextDelta(_)));
        handle_observation(&context, &mut state, &mut turn, initial).await;
        assert!(
            !state.forced_interrupted,
            "initial observation must commit cleanly against the live lease"
        );
        let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
        handle_steer(
            &context,
            &mut state,
            &mut turn,
            thread_id.clone(),
            seed.run_id.clone(),
            command_id.clone(),
            message_id.clone(),
            "follow up".to_owned(),
            respond_tx,
        )
        .await;
        let ack = tokio::time::timeout(OBSERVE_DEADLINE, respond_rx)
            .await
            .expect("steer ack settles")
            .expect("steer ack sends");
        assert!(
            matches!(ack, RunInteractionAck::Steered),
            "burst ack must steer, got {ack:?}"
        );
        // The Steered ack only leaves the Acked(Ok) arm, which never saw a
        // terminal: the message completes while the original turn is live.
        assert!(
            state.terminal.is_none(),
            "steered message must complete before the original turn terminal"
        );

        let (dispatch_state, _, _) = repository
            .read_steered_dispatch_state(&message_id)
            .await
            .expect("dispatch state should read");
        assert!(
            matches!(
                dispatch_state,
                artisan_database::entities::DispatchState::Completed
            ),
            "steered dispatch must complete"
        );
        let recorded = steer_request_lines(&temp.dir);
        assert_eq!(recorded.len(), 1, "exactly one provider steer write");
        assert!(
            recorded[0].contains("follow up"),
            "recorded steer must carry the verbatim input text"
        );

        // All 64 burst deltas plus the initial one commit as patches: 64
        // frames through a 4-slot channel proves the production drain.
        let patches = patch_count(&database, &thread_id).await;
        assert!(
            patches >= 64,
            "burst deltas must persist, saw {patches} patches"
        );

        // The projection links the same run: no second spawn happened.
        use database_entities::assistant_run;
        let runs = assistant_run::Entity::find()
            .filter(assistant_run::Column::ThreadId.eq(thread_id.as_str()))
            .all(&database)
            .await
            .expect("runs should read");
        assert_eq!(runs.len(), 1, "steer must not spawn a second run");
        use database_entities::conversation_item;
        let item = conversation_item::Entity::find()
            .filter(conversation_item::Column::SourceMessageId.eq(message_id.as_str()))
            .one(&database)
            .await
            .expect("projected item should read")
            .expect("projected item should exist");
        assert_eq!(
            item.turn_id.as_str(),
            seed.turn_id.as_str(),
            "projected item must link the live turn"
        );

        // Replaying the same envelope executes the Duplicate branch against
        // the completed row with no extra provider write.
        let (replay_tx, replay_rx) = tokio::sync::oneshot::channel();
        handle_steer(
            &context,
            &mut state,
            &mut turn,
            thread_id.clone(),
            seed.run_id.clone(),
            command_id.clone(),
            message_id.clone(),
            "follow up".to_owned(),
            replay_tx,
        )
        .await;
        let replay = tokio::time::timeout(OBSERVE_DEADLINE, replay_rx)
            .await
            .expect("replay ack settles")
            .expect("replay ack sends");
        assert!(
            matches!(replay, RunInteractionAck::Steered),
            "completed replay must steer without resending, got {replay:?}"
        );
        assert_eq!(
            steer_request_lines(&temp.dir).len(),
            1,
            "replay must not add a provider write"
        );

        // Ending the turn surfaces the fixture's cancelled terminal, then
        // the owner shuts down cleanly.
        turn.cancel();
        let mut saw_terminal = None;
        for _ in 0..128 {
            let next = tokio::time::timeout(OBSERVE_DEADLINE, turn.next_observation())
                .await
                .expect("terminal observation settles");
            let Some(observation) = next else { break };
            if let EngineObservation::Terminal(terminal) = observation {
                saw_terminal = Some(terminal.state());
                break;
            }
        }
        assert_eq!(
            saw_terminal,
            Some(TerminalState::Cancelled),
            "cancel must surface the interrupted terminal"
        );
        let finished = turn.finish().await.expect("turn finishes");
        assert_eq!(finished.terminal(), TerminalState::Cancelled);
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("burst steer settles inside budget");
}

#[tokio::test]
async fn codex_steer_reject_fails_typed_with_payload_preserved() {
    tokio::time::timeout(STEER_DEADLINE, async {
        let fixture = steer_fixture_program();
        let temp = SteerTempRoot::new("reject");
        let program = steer_scenario_program(&fixture, &temp.dir, "steer_reject");
        let database = connect(SqliteConfig::file(&temp.db_path).sqlx_logging(false))
            .await
            .expect("file database should open");
        migrate_to_current(&database)
            .await
            .expect("migrations should apply");
        let repository = Repository::new(database.clone());
        let thread_id = ThreadId::parse("thread-fixture-1").expect("thread id");
        let seed = seed_steer_run(
            &repository,
            &thread_id,
            "reject",
            "run-steer-reject",
            "turn-steer-reject",
        )
        .await;

        let launch = NativeCodexAuthority::new()
            .resolve_launch_with_executable(
                &temp.db_path,
                &EngineProfileId::parse("codex-fixture").expect("profile id"),
                &program,
                "codex-cli 0.145.0",
            )
            .expect("wire launch resolves");
        let mut owner = EngineOwner::start_configured(
            NonZeroUsize::new(1).expect("one slot"),
            &tokio::runtime::Handle::current(),
        );
        let mut turn = owner
            .admit_codex_turn(
                EngineCodexTurnInput {
                    run_id: seed.run_id.clone(),
                    thread_id: thread_id.clone(),
                    project_root: temp.root.clone(),
                    prompt_id: "prompt-steer-1".to_owned(),
                    prompt: QueueMessagePayload::text_only("hello steer").expect("payload"),
                    settings: seed.settings.clone(),
                    launch,
                    continuation: None,
                    prompt_delivery: "immediate".to_owned(),
                    stream_after: 0,
                    control_capacity: 1,
                },
                Duration::from_secs(50),
            )
            .expect("wire turn admits");
        turn.prepare().await.expect("wire turn prepares");
        turn.authorize().expect("wire turn authorizes once");

        let command_id = RequestId::parse("request-steer-1").expect("request id");
        let message_id = MessageId::parse("message-steer-1").expect("message id");
        repository
            .queue_message(QueueMessageInput {
                request_id: command_id.clone(),
                message_id: message_id.clone(),
                thread_id: thread_id.clone(),
                payload: QueueMessagePayload::text_only("follow up").expect("payload"),
                steer_run_id: Some(seed.run_id.clone()),
                accepted_at: UnixMillis::from_millis(100),
            })
            .await
            .expect("steered message should queue");

        let config = test_dispatcher_config();
        let origin = SystemCommandOrigin;
        let stop = CancelHandle::new();
        let process_cancel = CancelHandle::new();
        let run_cancel = CancelHandle::new();
        let context = TurnConsumptionContext {
            repository: &repository,
            config: &config,
            origin: &origin,
            stop: &stop,
            process_cancel: &process_cancel,
            run_cancel: &run_cancel,
        };
        let mut state = TurnConsumptionState::new(
            RunBatchScope {
                claimed: &seed.claimed,
                launched: &seed.launched,
                bound: &seed.bound,
                run_start_key: &seed.run_start_key,
                credentials: &seed.credentials,
                expected_launch_at: UnixMillis::from_millis(seed.launch_op_ms),
                expected_updated_at: UnixMillis::from_millis(seed.bound_op_ms),
            },
            EngineId::Codex,
        );
        // The opening delta goes through the real observation handler like
        // every later frame, keeping state and durable counters honest
        // before the steer is sent.
        let initial = tokio::time::timeout(OBSERVE_DEADLINE, turn.next_observation())
            .await
            .expect("initial observation settles")
            .expect("initial observation exists");
        assert!(matches!(initial, EngineObservation::TextDelta(_)));
        handle_observation(&context, &mut state, &mut turn, initial).await;
        assert!(
            !state.forced_interrupted,
            "initial observation must commit cleanly against the live lease"
        );
        let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
        handle_steer(
            &context,
            &mut state,
            &mut turn,
            thread_id.clone(),
            seed.run_id.clone(),
            command_id.clone(),
            message_id.clone(),
            "follow up".to_owned(),
            respond_tx,
        )
        .await;
        let ack = tokio::time::timeout(OBSERVE_DEADLINE, respond_rx)
            .await
            .expect("steer ack settles")
            .expect("steer ack sends");
        assert!(
            matches!(
                ack,
                RunInteractionAck::Refused {
                    reason: "steer write to the provider failed",
                }
            ),
            "provider rejection must refuse typed, got {ack:?}"
        );

        let (dispatch_state, reason, _) = repository
            .read_steered_dispatch_state(&message_id)
            .await
            .expect("dispatch state should read");
        assert!(
            matches!(
                dispatch_state,
                artisan_database::entities::DispatchState::Failed
            ),
            "rejected steer must fail its row"
        );
        assert_eq!(
            reason.as_deref(),
            Some("steer write to the provider failed")
        );
        let retained = repository
            .read_queue_message_dispatch_payload(&message_id)
            .await
            .expect("dispatch payload should read")
            .expect("rejected steer must keep its payload");
        assert_eq!(
            retained
                .payload
                .text()
                .expect("payload text should survive")
                .as_str(),
            "follow up"
        );
        // The rejected attempt still performed exactly one provider write.
        let recorded = steer_request_lines(&temp.dir);
        assert_eq!(recorded.len(), 1, "exactly one recorded provider write");
        assert!(recorded[0].contains("follow up"));

        turn.cancel();
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("reject steer settles inside budget");
}

#[tokio::test]
async fn codex_steer_cancel_before_ack_records_nothing_and_retry_steers_once() {
    tokio::time::timeout(STEER_DEADLINE, async {
        let fixture = steer_fixture_program();
        let temp = SteerTempRoot::new("cancel");
        let program = steer_scenario_program(&fixture, &temp.dir, "steer_burst");
        let database = connect(SqliteConfig::file(&temp.db_path).sqlx_logging(false))
            .await
            .expect("file database should open");
        migrate_to_current(&database)
            .await
            .expect("migrations should apply");
        let repository = Repository::new(database.clone());
        let thread_id = ThreadId::parse("thread-fixture-1").expect("thread id");
        let seed = seed_steer_run(
            &repository,
            &thread_id,
            "cancel",
            "run-steer-cancel",
            "turn-steer-cancel",
        )
        .await;

        let launch = NativeCodexAuthority::new()
            .resolve_launch_with_executable(
                &temp.db_path,
                &EngineProfileId::parse("codex-fixture").expect("profile id"),
                &program,
                "codex-cli 0.145.0",
            )
            .expect("wire launch resolves");
        let mut owner = EngineOwner::start_configured(
            NonZeroUsize::new(1).expect("one slot"),
            &tokio::runtime::Handle::current(),
        );
        let mut turn = owner
            .admit_codex_turn(
                EngineCodexTurnInput {
                    run_id: seed.run_id.clone(),
                    thread_id: thread_id.clone(),
                    project_root: temp.root.clone(),
                    prompt_id: "prompt-steer-1".to_owned(),
                    prompt: QueueMessagePayload::text_only("hello steer").expect("payload"),
                    settings: seed.settings.clone(),
                    launch,
                    continuation: None,
                    prompt_delivery: "immediate".to_owned(),
                    stream_after: 0,
                    control_capacity: 1,
                },
                Duration::from_secs(50),
            )
            .expect("wire turn admits");
        turn.prepare().await.expect("wire turn prepares");
        turn.authorize().expect("wire turn authorizes once");

        let command_id = RequestId::parse("request-steer-1").expect("request id");
        let message_id = MessageId::parse("message-steer-1").expect("message id");
        repository
            .queue_message(QueueMessageInput {
                request_id: command_id.clone(),
                message_id: message_id.clone(),
                thread_id: thread_id.clone(),
                payload: QueueMessagePayload::text_only("follow up").expect("payload"),
                steer_run_id: Some(seed.run_id.clone()),
                accepted_at: UnixMillis::from_millis(100),
            })
            .await
            .expect("steered message should queue");

        // The cancellation fires before the arm runs: the biased drive
        // observes it without polling the provider future, so nothing is
        // recorded, the row stays open, and the provider sees nothing. The
        // lazy provider future is never polled, so no write can race this.
        let config = test_dispatcher_config();
        let origin = SystemCommandOrigin;
        let stop = CancelHandle::new();
        let process_cancel = CancelHandle::new();
        let run_cancel = CancelHandle::new();
        run_cancel.cancel();
        let context = TurnConsumptionContext {
            repository: &repository,
            config: &config,
            origin: &origin,
            stop: &stop,
            process_cancel: &process_cancel,
            run_cancel: &run_cancel,
        };
        let mut state = TurnConsumptionState::new(
            RunBatchScope {
                claimed: &seed.claimed,
                launched: &seed.launched,
                bound: &seed.bound,
                run_start_key: &seed.run_start_key,
                credentials: &seed.credentials,
                expected_launch_at: UnixMillis::from_millis(seed.launch_op_ms),
                expected_updated_at: UnixMillis::from_millis(seed.bound_op_ms),
            },
            EngineId::Codex,
        );
        // The opening delta goes through the real observation handler like
        // every later frame, keeping state and durable counters honest
        // before the steer is sent.
        let initial = tokio::time::timeout(OBSERVE_DEADLINE, turn.next_observation())
            .await
            .expect("initial observation settles")
            .expect("initial observation exists");
        assert!(matches!(initial, EngineObservation::TextDelta(_)));
        handle_observation(&context, &mut state, &mut turn, initial).await;
        assert!(
            !state.forced_interrupted,
            "initial observation must commit cleanly against the live lease"
        );
        let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
        handle_steer(
            &context,
            &mut state,
            &mut turn,
            thread_id.clone(),
            seed.run_id.clone(),
            command_id.clone(),
            message_id.clone(),
            "follow up".to_owned(),
            respond_tx,
        )
        .await;
        let ack = tokio::time::timeout(OBSERVE_DEADLINE, respond_rx)
            .await
            .expect("cancel ack settles")
            .expect("cancel ack sends");
        assert!(
            matches!(ack, RunInteractionAck::Unavailable),
            "pre-ack cancellation must answer transiently, got {ack:?}"
        );
        let (dispatch_state, _, _) = repository
            .read_steered_dispatch_state(&message_id)
            .await
            .expect("dispatch state should read");
        assert!(
            matches!(
                dispatch_state,
                artisan_database::entities::DispatchState::Queued
            ),
            "cancelled steer must leave its row open"
        );
        assert!(
            steer_request_lines(&temp.dir).is_empty(),
            "cancelled steer must not reach the provider"
        );

        // The retry runs the full path on the same untouched turn and
        // performs exactly one provider write.
        let live_cancel = CancelHandle::new();
        let context = TurnConsumptionContext {
            repository: &repository,
            config: &config,
            origin: &origin,
            stop: &stop,
            process_cancel: &process_cancel,
            run_cancel: &live_cancel,
        };
        let (retry_tx, retry_rx) = tokio::sync::oneshot::channel();
        handle_steer(
            &context,
            &mut state,
            &mut turn,
            thread_id.clone(),
            seed.run_id.clone(),
            command_id.clone(),
            message_id.clone(),
            "follow up".to_owned(),
            retry_tx,
        )
        .await;
        let ack = tokio::time::timeout(OBSERVE_DEADLINE, retry_rx)
            .await
            .expect("retry ack settles")
            .expect("retry ack sends");
        assert!(
            matches!(ack, RunInteractionAck::Steered),
            "retry after cancel must steer, got {ack:?}"
        );
        assert_eq!(
            steer_request_lines(&temp.dir).len(),
            1,
            "exactly one provider write across cancel and retry"
        );

        turn.cancel();
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("cancel steer settles inside budget");
}

#[tokio::test]
async fn codex_steer_known_acked_retry_replays_projection_without_provider() {
    tokio::time::timeout(STEER_DEADLINE, async {
        let fixture = steer_fixture_program();
        let temp = SteerTempRoot::new("dupopen");
        let program = steer_scenario_program(&fixture, &temp.dir, "steer_burst");
        let database = connect(SqliteConfig::file(&temp.db_path).sqlx_logging(false))
            .await
            .expect("file database should open");
        migrate_to_current(&database)
            .await
            .expect("migrations should apply");
        let repository = Repository::new(database.clone());
        let thread_id = ThreadId::parse("thread-fixture-1").expect("thread id");
        let seed = seed_steer_run(
            &repository,
            &thread_id,
            "dupopen",
            "run-steer-dupopen",
            "turn-steer-dupopen",
        )
        .await;

        let launch = NativeCodexAuthority::new()
            .resolve_launch_with_executable(
                &temp.db_path,
                &EngineProfileId::parse("codex-fixture").expect("profile id"),
                &program,
                "codex-cli 0.145.0",
            )
            .expect("wire launch resolves");
        let mut owner = EngineOwner::start_configured(
            NonZeroUsize::new(1).expect("one slot"),
            &tokio::runtime::Handle::current(),
        );
        let mut turn = owner
            .admit_codex_turn(
                EngineCodexTurnInput {
                    run_id: seed.run_id.clone(),
                    thread_id: thread_id.clone(),
                    project_root: temp.root.clone(),
                    prompt_id: "prompt-steer-1".to_owned(),
                    prompt: QueueMessagePayload::text_only("hello steer").expect("payload"),
                    settings: seed.settings.clone(),
                    launch,
                    continuation: None,
                    prompt_delivery: "immediate".to_owned(),
                    stream_after: 0,
                    control_capacity: 1,
                },
                Duration::from_secs(50),
            )
            .expect("wire turn admits");
        turn.prepare().await.expect("wire turn prepares");
        turn.authorize().expect("wire turn authorizes once");

        let command_id = RequestId::parse("request-steer-1").expect("request id");
        let message_id = MessageId::parse("message-steer-1").expect("message id");
        repository
            .queue_message(QueueMessageInput {
                request_id: command_id.clone(),
                message_id: message_id.clone(),
                thread_id: thread_id.clone(),
                payload: QueueMessagePayload::text_only("follow up").expect("payload"),
                steer_run_id: Some(seed.run_id.clone()),
                accepted_at: UnixMillis::from_millis(100),
            })
            .await
            .expect("steered message should queue");

        // Record the post-ack ledger resolution exactly as the production
        // arm does after a real provider ack (same method, same intent):
        // the redelivery below must therefore take the Duplicate branch
        // against the still-open row and replay the projection with no
        // provider contact.
        let target_id = ObservationId::parse("message-steer-1").expect("target id should parse");
        let intent = format!("steer:{}", message_id.as_str());
        turn.note_interaction_requested(
            &target_id,
            crate::engine_owner::interaction::InteractionTarget::Steer,
        );
        turn.deliver_interaction_response(
            command_id.as_str(),
            &target_id,
            crate::engine_owner::interaction::InteractionTarget::Steer,
            &intent,
        )
        .expect("post-ack ledger record should apply");
        assert!(
            steer_request_lines(&temp.dir).is_empty(),
            "ledger record alone must not touch the provider"
        );

        let config = test_dispatcher_config();
        let origin = SystemCommandOrigin;
        let stop = CancelHandle::new();
        let process_cancel = CancelHandle::new();
        let run_cancel = CancelHandle::new();
        let context = TurnConsumptionContext {
            repository: &repository,
            config: &config,
            origin: &origin,
            stop: &stop,
            process_cancel: &process_cancel,
            run_cancel: &run_cancel,
        };
        let mut state = TurnConsumptionState::new(
            RunBatchScope {
                claimed: &seed.claimed,
                launched: &seed.launched,
                bound: &seed.bound,
                run_start_key: &seed.run_start_key,
                credentials: &seed.credentials,
                expected_launch_at: UnixMillis::from_millis(seed.launch_op_ms),
                expected_updated_at: UnixMillis::from_millis(seed.bound_op_ms),
            },
            EngineId::Codex,
        );
        // The opening delta goes through the real observation handler like
        // every later frame, keeping state and durable counters honest
        // before the steer is sent.
        let initial = tokio::time::timeout(OBSERVE_DEADLINE, turn.next_observation())
            .await
            .expect("initial observation settles")
            .expect("initial observation exists");
        assert!(matches!(initial, EngineObservation::TextDelta(_)));
        handle_observation(&context, &mut state, &mut turn, initial).await;
        assert!(
            !state.forced_interrupted,
            "initial observation must commit cleanly against the live lease"
        );
        let (respond_tx, respond_rx) = tokio::sync::oneshot::channel();
        handle_steer(
            &context,
            &mut state,
            &mut turn,
            thread_id.clone(),
            seed.run_id.clone(),
            command_id.clone(),
            message_id.clone(),
            "follow up".to_owned(),
            respond_tx,
        )
        .await;
        let ack = tokio::time::timeout(OBSERVE_DEADLINE, respond_rx)
            .await
            .expect("retry ack settles")
            .expect("retry ack sends");
        assert!(
            matches!(ack, RunInteractionAck::Steered),
            "known-acked retry must steer from the open row, got {ack:?}"
        );
        let (dispatch_state, _, _) = repository
            .read_steered_dispatch_state(&message_id)
            .await
            .expect("dispatch state should read");
        assert!(
            matches!(
                dispatch_state,
                artisan_database::entities::DispatchState::Completed
            ),
            "retried projection must complete the row"
        );
        assert!(
            steer_request_lines(&temp.dir).is_empty(),
            "known-acked retry must not contact the provider"
        );

        turn.cancel();
        assert_eq!(owner.shutdown().await, EngineOwnerShutdown::Joined);
    })
    .await
    .expect("duplicate-open retry settles inside budget");
}

#[test]
fn steer_ledger_preflight_records_only_post_ack() {
    use crate::engine_owner::interaction::{
        InteractionTarget, TurnInteractionLedger, TurnInteractionOutcome,
    };

    let run_id = RunId::parse("run-ledger").expect("run id");
    let target = ObservationId::parse("message-ledger").expect("target id");
    let mut ledger = TurnInteractionLedger::new(run_id);
    assert!(ledger.note_requested(&target, InteractionTarget::Steer));
    // Preflight is nonmutating: repeated preflights stay eligible, so a
    // pre-ack interruption never writes a resolution the durable row
    // cannot see.
    for _ in 0..2 {
        assert_eq!(
            ledger.preflight(
                "request-ledger",
                &target,
                InteractionTarget::Steer,
                "steer:message-ledger"
            ),
            Ok(TurnInteractionOutcome::Applied)
        );
    }
    // Recording happens only through deliver, after the actual ack.
    assert_eq!(
        ledger.deliver(
            "request-ledger",
            &target,
            InteractionTarget::Steer,
            "steer:message-ledger"
        ),
        Ok(TurnInteractionOutcome::Applied)
    );
    // Now the identical command is a known-acked duplicate in both paths.
    assert_eq!(
        ledger.preflight(
            "request-ledger",
            &target,
            InteractionTarget::Steer,
            "steer:message-ledger"
        ),
        Ok(TurnInteractionOutcome::Duplicate)
    );
    assert_eq!(
        ledger.deliver(
            "request-ledger",
            &target,
            InteractionTarget::Steer,
            "steer:message-ledger"
        ),
        Ok(TurnInteractionOutcome::Duplicate)
    );
    // Same command id with a changed intent conflicts in both paths.
    assert!(matches!(
        ledger.preflight(
            "request-ledger",
            &target,
            InteractionTarget::Steer,
            "steer:other"
        ),
        Err(_)
    ));
}
