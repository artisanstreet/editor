//! A-approve fixture proof: the full answer round-trip against a live turn.
//!
//! The deterministic hold scenario keeps one fixture turn live after its
//! first durable delta, so this test records provider requests, answers
//! them through the authenticated request route, and proves the run
//! continues: deny lands with no side effect, allow continues, replays
//! duplicate without double effect, unknown ids miss, foreign runs are
//! rejected without storage, resolutions commit through the S1b checkpoint
//! path, and settle wipes pending rows while receipts survive.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use artisan_database::{
    RecordApprovalRequest, Repository, SqliteConfig, StoredInteractionReceipt, connect,
    decode_observation_checkpoint,
};
use artisan_domain::{
    ApprovalMode, ApprovalRequest, ByteLimit, Command, CountLimit, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, MessageBody, MessageId, NetworkAccess,
    Observation, ObservationId, OpenCode2Selection, PermissionId, ProjectId, ReceiptDisposition,
    RequestId, RespondApproval, RunId, ThreadId, ThreadTitle, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use artisan_native_engine::NativeOpenCode2Authority;
use artisan_protocol::{ClientRequest, ResponsePayload, RunInteractionOutcome};
use artisan_transport::CancelHandle;
use sea_orm::EntityTrait;

use super::{
    conversation_commit_notifier::ConversationCommitNotifier, lifecycle_control::ActivityGateImpl,
    run_cancellation::RunCancellationRegistry,
};
use crate::RequestHandler;
use crate::native_run_dispatch::{
    FixtureScenarioLaunch, NativeRunDispatcher, NativeRunDispatcherConfig,
    NativeRunDispatcherConfigInput, NativeRunDispatcherShutdown,
};

const HOLD_SCENARIO: &str = "prompt_text_then_hold_after_first_delta";
const PROOF_DEADLINE: Duration = Duration::from_secs(20);

const THREAD_ID: &str = "approve-hold-thread";
const RUN_ID: &str = "fixture-run";
const APPROVAL_ONE: &str = "approval-one";
const APPROVAL_TWO: &str = "approval-two";
const REQUESTED_AT_ONE_MS: i64 = 2_000_000;
const REQUESTED_AT_TWO_MS: i64 = 2_001_000;

fn registered_fixture_program() -> PathBuf {
    let path = std::env::var_os("ARTISAN_ENGINE_OWNER_FIXTURE").map_or_else(
        || {
            let test = std::env::current_exe().expect("test executable path");
            test.parent()
                .expect("test output directory")
                .parent()
                .expect("Cargo profile directory")
                .join("examples")
                .join(format!(
                    "engine-owner-fixture{}",
                    std::env::consts::EXE_SUFFIX
                ))
        },
        std::path::PathBuf::from,
    );
    assert!(
        path.is_absolute() && path.is_file(),
        "build fixture with cargo build -p artisan-backend --examples or set ARTISAN_ENGINE_OWNER_FIXTURE to an absolute file: {}",
        path.display()
    );
    path
}

fn temp_database(label: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "artisan-approve-hold-{label}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temporary database directory should be created");
    let file = dir.join("forge.sqlite3");
    (dir, file)
}

fn config() -> NativeRunDispatcherConfig {
    NativeRunDispatcherConfig::new(
        NativeOpenCode2Authority::new(),
        ConversationCommitNotifier::new(),
        NativeRunDispatcherConfigInput {
            claim_lease: Duration::from_secs(30),
            launch_deadline: Duration::from_secs(120),
            poll_interval: Duration::from_millis(10),
            retry_backoff: Duration::from_millis(10),
            shutdown_budget: Duration::from_secs(5),
            queue_capacity: NonZeroUsize::new(1).expect("one queue slot is nonzero"),
            max_command_retries: NonZeroUsize::new(3).expect("three retries are nonzero"),
            prompt_delivery: "immediate".to_owned(),
            stream_after: 0,
        },
    )
    .expect("fixture dispatch policy")
}

fn engine_config() -> EngineRunConfig {
    let phase = FiniteMillis::new(5_000).expect("phase budget is valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(125_000).expect("attempt budget is valid"),
        readiness_budget: phase,
        health_budget: phase,
        prompt_budget: phase,
        stream_budget: phase,
        close_budget: FiniteMillis::new(5_000).expect("close budget is valid"),
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
        PermissionId::parse("permission-hold").expect("permission id is valid"),
        EngineAgentId::parse("agent-hold").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("fixture-test").expect("profile id is valid"),
            EngineModelId::parse("model-hold").expect("model id is valid"),
            EngineRouteId::parse("route-hold").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

async fn seed_hold_thread(repository: &Repository) {
    repository
        .attach_project(artisan_database::AttachProjectInput {
            request_id: RequestId::parse("hold-seed-attach").expect("request id"),
            directory_id: artisan_domain::DirectoryId::parse("approve-hold-directory")
                .expect("directory id"),
            project_id: ProjectId::parse("approve-hold-project").expect("project id"),
            root_path: artisan_domain::RootPath::parse("C:/repos/artisan").expect("root path"),
            display_name: artisan_domain::DisplayName::parse("Artisan").expect("display name"),
            attached_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("project should attach");
    repository
        .create_thread(artisan_database::CreateThreadInput {
            request_id: RequestId::parse("hold-seed-thread").expect("request id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            project_id: ProjectId::parse("approve-hold-project").expect("project id"),
            title: ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(10),
            updated_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("thread should create");
    repository
        .set_thread_engine_config(artisan_database::SetThreadEngineConfigInput {
            request_id: RequestId::parse("hold-seed-config").expect("request id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: engine_config(),
            accepted_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("engine configuration should create");
    repository
        .queue_first_message(artisan_database::QueueFirstMessageInput {
            request_id: RequestId::parse("hold-seed-message").expect("request id"),
            message_id: MessageId::parse("approve-hold-message").expect("message id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            body: MessageBody::parse("hello world").expect("message body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("one fixture message should queue");
}

fn approval_request_fixture() -> ApprovalRequest {
    ApprovalRequest::command(
        "cargo test".to_owned(),
        Some("C:/repos/artisan".to_owned()),
        Some("run the suite".to_owned()),
    )
    .expect("fixture approval request should be valid")
}

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD_ID).expect("thread id")
}

fn run_id() -> RunId {
    RunId::parse(RUN_ID).expect("run id")
}

fn deny(request: &str, approval: &str) -> Command {
    Command::RespondApproval(RespondApproval::new(
        RequestId::parse(request).expect("request id"),
        thread_id(),
        run_id(),
        ObservationId::parse(approval).expect("target id"),
        false,
    ))
}

fn allow(request: &str, approval: &str) -> Command {
    Command::RespondApproval(RespondApproval::new(
        RequestId::parse(request).expect("request id"),
        thread_id(),
        run_id(),
        ObservationId::parse(approval).expect("target id"),
        true,
    ))
}

async fn record_approval(
    repository: &Repository,
    approval: &str,
    description: &str,
    requested_at_ms: i64,
) {
    static REQUEST: std::sync::OnceLock<ApprovalRequest> = std::sync::OnceLock::new();
    let asked = REQUEST.get_or_init(approval_request_fixture);
    repository
        .record_approval_request(RecordApprovalRequest {
            thread_id: &thread_id(),
            run_id: &run_id(),
            approval_id: &ObservationId::parse(approval).expect("target id"),
            description: description.to_owned(),
            request: asked,
            requested_at: UnixMillis::from_millis(requested_at_ms),
            binding_version: 1,
        })
        .await
        .expect("request should store");
}

async fn respond(
    handler: &RequestHandler,
    command: Command,
) -> artisan_protocol::RespondApprovalReceipt {
    let request_id = command.request_id().clone();
    let response = handler
        .respond(&request_id, &ClientRequest::Command(command))
        .await
        .expect("response should settle");
    let ResponsePayload::ApprovalResponse(receipt) = response.payload else {
        panic!("approval responses should carry approval receipts")
    };
    receipt
}

async fn await_live_turn(database: &sea_orm::DatabaseConnection, dispatcher: &NativeRunDispatcher) {
    // The hold scenario commits its first durable delta while the turn stays
    // live; that plus a routed inbox proves the consuming window is open.
    tokio::time::timeout(PROOF_DEADLINE, async {
        loop {
            let items = artisan_database::entities::conversation_item::Entity::find()
                .all(database)
                .await
                .expect("conversation items should be readable");
            let delta = items.iter().any(|item| {
                item.item_kind == artisan_database::entities::ConversationItemKind::AssistantMessage
                    && item.body == "hello world"
            });
            let routed = dispatcher
                .interaction_registry()
                .route(&thread_id(), &run_id())
                .is_ok_and(|route| route.is_some());
            if delta && routed {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("turn should go live before the deadline");
}

async fn await_settlement(database: &sea_orm::DatabaseConnection) {
    tokio::time::timeout(PROOF_DEADLINE, async {
        loop {
            let run = artisan_database::entities::assistant_run::Entity::find_by_id(RUN_ID)
                .one(database)
                .await
                .expect("run should be readable")
                .expect("run should exist");
            if run.terminal_at_ms.is_some() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("run should settle before the deadline");
}

fn resolved_approval(decoded: &artisan_database::DecodedObservationBatch) -> (bool, String) {
    let mut found = None;
    for observation in decoded.observations() {
        if let Observation::Approval(resolved) = observation
            && resolved.state() == artisan_domain::ApprovalState::Resolved
        {
            found = Some((
                resolved.approved() == Some(true),
                resolved.description().to_owned(),
            ));
        }
    }
    found.expect("a resolved approval observation should be committed")
}

#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
#[tokio::test(flavor = "current_thread")]
async fn hold_turn_answers_deny_then_allow_and_settles_clean() {
    let fixture = registered_fixture_program();
    let (dir, file) = temp_database("hold-approve");
    let database = connect(
        SqliteConfig::file(&file)
            .min_connections(1)
            .max_connections(4)
            .sqlx_logging(false),
    )
    .await
    .expect("temp database should open");
    migrate_to_current(&database)
        .await
        .expect("temp database should migrate");
    let repository = Repository::new(database.clone());
    seed_hold_thread(&repository).await;

    let process_cancel = Arc::new(CancelHandle::new());
    let mut dispatcher =
        NativeRunDispatcher::start_with_fixture_scenario_for_tests(FixtureScenarioLaunch {
            repository: repository.clone(),
            database_path: file.clone(),
            config: config(),
            process_cancel: Arc::clone(&process_cancel),
            cancellation: RunCancellationRegistry::new(1).expect("registry capacity is valid"),
            activity: ActivityGateImpl::new(),
            runtime: &tokio::runtime::Handle::current(),
            fixture_program: fixture,
            scenario: HOLD_SCENARIO,
        });
    await_live_turn(&database, &dispatcher).await;
    let handler = RequestHandler::new(repository.clone())
        .with_run_interaction_registry(dispatcher.interaction_registry());

    // Request goes requested: the wake signal names its instant.
    record_approval(
        &repository,
        APPROVAL_ONE,
        "Run the suite?",
        REQUESTED_AT_ONE_MS,
    )
    .await;
    let instants = repository
        .pending_request_instants()
        .await
        .expect("instants should read");
    assert_eq!(instants, vec![REQUESTED_AT_ONE_MS]);
    // The durable signal feeds the wake-lock policy directly: with no other
    // progressing work, the human-blocked run still holds the lock inside
    // its approval grace window instead of reading as stalled.
    let assessment = crate::wake_lock_policy::assess_unsettled_work(
        crate::wake_lock_policy::UnsettledWorkSnapshot::new(0, &instants),
        REQUESTED_AT_ONE_MS + 1_000,
        5 * 60_000,
    );
    assert!(assessment.hold);
    assert_eq!(assessment.held_count, 1);

    // Deny has no side effect and the run continues.
    let denied = respond(&handler, deny("respond-deny-hold", APPROVAL_ONE)).await;
    assert_eq!(denied.outcome, RunInteractionOutcome::Applied);
    assert_eq!(denied.disposition, ReceiptDisposition::Accepted);
    assert!(!denied.approved);
    let live = artisan_database::entities::assistant_run::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("run should be readable")
        .expect("run should exist");
    assert_eq!(
        live.lifecycle,
        artisan_database::entities::AssistantRunLifecycle::Running
    );
    assert!(live.terminal_at_ms.is_none());

    // The denial committed as a resolved observation through the S1b path.
    let checkpoint = artisan_database::entities::run_checkpoint::Entity::find_by_id(RUN_ID)
        .one(&database)
        .await
        .expect("checkpoint should be readable")
        .expect("checkpoint should exist");
    let decoded = decode_observation_checkpoint(
        checkpoint.engine_checkpoint_version.expect("version"),
        checkpoint
            .engine_checkpoint_blob
            .as_ref()
            .expect("blob")
            .as_slice(),
    )
    .expect("resolution checkpoint should decode");
    assert_eq!(
        resolved_approval(&decoded),
        (false, "Run the suite?".to_owned())
    );

    // Re-request, then allow: the run continues.
    record_approval(
        &repository,
        APPROVAL_TWO,
        "Run the suite again?",
        REQUESTED_AT_TWO_MS,
    )
    .await;
    let allowed = respond(&handler, allow("respond-allow-hold", APPROVAL_TWO)).await;
    assert_eq!(allowed.outcome, RunInteractionOutcome::Applied);
    assert!(allowed.approved);

    // Replayed client request ids return duplicate receipts, no double effect.
    let replayed = respond(&handler, deny("respond-deny-hold", APPROVAL_ONE)).await;
    assert_eq!(replayed.outcome, RunInteractionOutcome::Applied);
    assert_eq!(replayed.disposition, ReceiptDisposition::Duplicate);

    // Unknown ids miss their target.
    let unknown = respond(&handler, deny("respond-ghost-hold", "approval-ghost")).await;
    assert_eq!(unknown.outcome, RunInteractionOutcome::UnknownTarget);

    // Foreign runs are rejected without storage.
    let foreign = Command::RespondApproval(RespondApproval::new(
        RequestId::parse("respond-foreign-hold").expect("request id"),
        thread_id(),
        RunId::parse("ghost-run").expect("run id"),
        ObservationId::parse(APPROVAL_ONE).expect("target id"),
        true,
    ));
    let request_id = foreign.request_id().clone();
    let response = handler
        .respond(&request_id, &ClientRequest::Command(foreign))
        .await
        .expect("foreign run should answer, not fail");
    let ResponsePayload::ApprovalResponse(foreign) = response.payload else {
        panic!("foreign run should carry an approval receipt")
    };
    assert_eq!(foreign.outcome, RunInteractionOutcome::WrongRun);
    assert!(
        repository
            .lookup_interaction_receipt(&RequestId::parse("respond-foreign-hold").expect("id"))
            .await
            .expect("receipt lookup should succeed")
            .is_none()
    );

    // Both decisions resolved: nothing human-blocked remains.
    assert!(
        repository
            .pending_request_instants()
            .await
            .expect("instants should read")
            .is_empty()
    );

    // Settle wipes pending rows while receipts survive; the run settled.
    process_cancel.cancel();
    assert_eq!(
        dispatcher.shutdown().await,
        NativeRunDispatcherShutdown::Joined
    );
    await_settlement(&database).await;
    assert!(
        repository
            .pending_interactions(&run_id())
            .await
            .expect("pending should read")
            .is_empty(),
        "settle must wipe pending rows"
    );
    let stored: Option<StoredInteractionReceipt> = repository
        .lookup_interaction_receipt(&RequestId::parse("respond-deny-hold").expect("id"))
        .await
        .expect("receipt should read");
    assert!(stored.is_some(), "receipts survive settle");

    drop(repository);
    drop(database);
    let _cleanup = std::fs::remove_dir_all(&dir);
}
