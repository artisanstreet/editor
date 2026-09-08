//! A-approve request-handler routing: receipt replay, live-run routing, and
//! acknowledgement mapping.
//!
//! Every test drives [`RequestHandler::respond`] directly with a decoded
//! protocol request and a correlated domain request id against real migrated
//! storage. A scripted owning loop stands in for the dispatch loop: it
//! drains one registered inbox and resolves through the same repository
//! transaction, so these tests prove routing, replay, and acknowledgement
//! mapping while the dispatch-level tests prove the owning loop itself.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use artisan_backend::run_interaction::{
    OwnedInteractionCommand, RunInteractionAck, RunInteractionRegistry,
};
use artisan_backend::{ForgeStorage, RequestHandler};
use artisan_database::{
    AttachProjectInput, BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch,
    CreateThreadInput, DispatchLeaseOwner, LaunchClaimedRun, LaunchClaimedRunOutcome,
    ProviderBindingBytes, QueueFirstMessageInput, RecordApprovalRequest, Repository, ResolveScope,
    RunLaunchCredentials, RunStartKey, SetThreadEngineConfigInput, SqliteConfig,
};
use artisan_domain::{
    ApprovalMode, ApprovalRequest, ByteLimit, Command, CountLimit, DirectoryId, DisplayName,
    EngineAgentId, EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FilesystemAccess, FiniteMillis, ItemId,
    MessageBody, MessageId, NetworkAccess, ObservationId, OpenCode2Selection, PatchId,
    PermissionId, ProjectId, QuestionInput, QuestionOption, RequestId, RespondApproval,
    RespondQuestion, RootPath, RunId, ThreadId, ThreadTitle, TurnId, UnixMillis, WebSearchAccess,
};
use artisan_protocol::{
    ClientRequest, ErrorCode, ProtocolFailure, ResponsePayload, RunInteractionOutcome,
    ServerResponse,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TemporaryDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "artisan-forge-run-interaction-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("temporary database directory should be created");
        let database = directory.join("forge.sqlite3");
        Self {
            directory,
            database,
        }
    }

    fn path(&self) -> &Path {
        &self.database
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.directory);
    }
}

async fn opened_storage(label: &str) -> (TemporaryDatabase, ForgeStorage) {
    let temporary = TemporaryDatabase::new(label);
    let storage = ForgeStorage::open(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("Forge storage should open and migrate");
    (temporary, storage)
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).expect("test request id should be valid")
}

fn thread(value: &str) -> ThreadId {
    ThreadId::parse(value).expect("test thread id should be valid")
}

fn run(value: &str) -> RunId {
    RunId::parse(value).expect("test run id should be valid")
}

fn target(value: &str) -> ObservationId {
    ObservationId::parse(value).expect("test target id should be valid")
}

fn approval_command(request_id: &str, approved: bool) -> Command {
    Command::RespondApproval(RespondApproval::new(
        request(request_id),
        thread("approve-thread"),
        run("approve-run"),
        target("approval-1"),
        approved,
    ))
}

fn question_command(request_id: &str, answers: Vec<String>) -> Command {
    Command::RespondQuestion(
        RespondQuestion::new(
            request(request_id),
            thread("approve-thread"),
            run("approve-run"),
            target("question-1"),
            answers,
        )
        .expect("test answers should be valid"),
    )
}

/// Answers one command through the handler, binding the correlated request
/// id before the command moves into its frame.
async fn respond(
    handler: &RequestHandler,
    command: Command,
) -> Result<ServerResponse, ProtocolFailure> {
    let request_id = command.request_id().clone();
    handler
        .respond(&request_id, &ClientRequest::Command(command))
        .await
}

/// Scripted stand-in for the owning dispatch loop.
///
/// Registers one live inbox, resolves every envelope through the same
/// repository transaction the dispatch loop uses, and acknowledges the
/// stored outcome. Observation commits and ledger delivery belong to the
/// dispatch-level tests; routing and acknowledgement mapping are proven
/// here. The returned task owns the registration lease: dropping it (via
/// abort) unregisters the run.
fn spawn_owner_drainer(
    repository: Repository,
    registry: RunInteractionRegistry,
    binding_version: i64,
) -> tokio::task::JoinHandle<()> {
    let (_lease, mut inbox) = registry
        .register(thread("approve-thread"), run("approve-run"))
        .expect("scripted owner should register");
    tokio::spawn(async move {
        let _lease = _lease;
        let scope = ResolveScope {
            binding_version,
            responded_at: UnixMillis::from_millis(1_100),
        };
        while let Some(envelope) = inbox.recv().await {
            let ack = match &envelope.command {
                OwnedInteractionCommand::RespondApproval {
                    approval_id,
                    approved,
                    ..
                } => {
                    let command = RespondApproval::new(
                        envelope.command.request_id().clone(),
                        envelope.thread_id.clone(),
                        envelope.run_id.clone(),
                        approval_id.clone(),
                        *approved,
                    );
                    match repository.resolve_approval_response(&command, &scope).await {
                        Ok(artisan_database::ResolveInteractionOutcome::Applied(applied)) => {
                            RunInteractionAck::Settled(applied.receipt)
                        }
                        Ok(artisan_database::ResolveInteractionOutcome::Duplicate(stored))
                        | Ok(artisan_database::ResolveInteractionOutcome::UnknownTarget(stored))
                        | Ok(artisan_database::ResolveInteractionOutcome::AlreadyResolved(
                            stored,
                        )) => RunInteractionAck::Settled(stored),
                        Ok(artisan_database::ResolveInteractionOutcome::Conflict(_)) => {
                            RunInteractionAck::Conflict
                        }
                        Ok(artisan_database::ResolveInteractionOutcome::WrongRun) => {
                            RunInteractionAck::WrongRun
                        }
                        Err(_) => RunInteractionAck::Unavailable,
                    }
                }
                OwnedInteractionCommand::RespondQuestion {
                    question_id,
                    answers,
                    ..
                } => {
                    let command = match RespondQuestion::new(
                        envelope.command.request_id().clone(),
                        envelope.thread_id.clone(),
                        envelope.run_id.clone(),
                        question_id.clone(),
                        answers.clone(),
                    ) {
                        Ok(command) => command,
                        Err(_) => {
                            let _ = envelope.respond.send(RunInteractionAck::Unavailable);
                            continue;
                        }
                    };
                    match repository.resolve_question_response(&command, &scope).await {
                        Ok(artisan_database::ResolveInteractionOutcome::Applied(applied)) => {
                            RunInteractionAck::Settled(applied.receipt)
                        }
                        Ok(artisan_database::ResolveInteractionOutcome::Duplicate(stored))
                        | Ok(artisan_database::ResolveInteractionOutcome::UnknownTarget(stored))
                        | Ok(artisan_database::ResolveInteractionOutcome::AlreadyResolved(
                            stored,
                        )) => RunInteractionAck::Settled(stored),
                        Ok(artisan_database::ResolveInteractionOutcome::Conflict(_)) => {
                            RunInteractionAck::Conflict
                        }
                        Ok(artisan_database::ResolveInteractionOutcome::WrongRun) => {
                            RunInteractionAck::WrongRun
                        }
                        Err(_) => RunInteractionAck::Unavailable,
                    }
                }
            };
            let _ = envelope.respond.send(ack);
        }
    })
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
        PermissionId::parse("permission-route").expect("permission id is valid"),
        EngineAgentId::parse("agent-route").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-route").expect("profile id is valid"),
            EngineModelId::parse("model-route").expect("model id is valid"),
            EngineRouteId::parse("route-route").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

/// Seeds the `approve-thread` thread with engine configuration, then claims,
/// launches, and binds the `approve-run` running run responses fence against.
async fn seed_running_run(repository: &Repository) {
    repository
        .attach_project(AttachProjectInput {
            request_id: request("route-seed-attach"),
            directory_id: DirectoryId::parse("approve-directory").expect("directory id"),
            project_id: ProjectId::parse("approve-project").expect("project id"),
            root_path: RootPath::parse("C:/repos/artisan").expect("root path"),
            display_name: DisplayName::parse("Artisan").expect("display name"),
            attached_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("project should attach");
    repository
        .create_thread(CreateThreadInput {
            request_id: request("route-seed-thread"),
            thread_id: thread("approve-thread"),
            project_id: ProjectId::parse("approve-project").expect("project id"),
            title: ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(10),
            updated_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("thread should create");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request("route-seed-config"),
            thread_id: thread("approve-thread"),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: launch_config(),
            accepted_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("engine configuration should create");
    let engine_settings = repository
        .read_thread_engine_settings(&thread("approve-thread"))
        .await
        .expect("engine configuration should read")
        .expect("engine configuration should be present");
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: request("route-seed-message"),
            message_id: MessageId::parse("approve-message").expect("message id"),
            thread_id: thread("approve-thread"),
            body: MessageBody::parse("first durable body").expect("body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("message should queue");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x11; 32]),
            claimed_at: UnixMillis::from_millis(100),
            lease_expires_at: UnixMillis::from_millis(600),
        })
        .await
        .expect("claim should succeed")
        .expect("dispatch should be claimed");
    let start_key = RunStartKey::new([0xd4; 32]);
    let credentials = RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]);
    let LaunchClaimedRunOutcome::Started(launched) = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run("approve-run"),
            turn_id: &TurnId::parse("approve-turn").expect("turn id"),
            item_id: &ItemId::parse("approve-item").expect("item id"),
            first_patch_id: &PatchId::parse("approve-patch-a").expect("patch id"),
            second_patch_id: &PatchId::parse("approve-patch-b").expect("patch id"),
            operated_at: UnixMillis::from_millis(150),
            run_start_key: &start_key,
            credentials: &credentials,
            engine_settings: &engine_settings,
        })
        .await
        .expect("launch should succeed")
    else {
        panic!("launch should start the run")
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let BindRunProviderOutcome::Bound(_) = repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(150),
            bound_at: UnixMillis::from_millis(200),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await
        .expect("bind should succeed")
    else {
        panic!("bind should be fresh")
    };
}

#[tokio::test]
async fn route_without_registry_is_unsupported() {
    let (_temporary, storage) = opened_storage("unsupported").await;
    let handler = RequestHandler::new(storage.repository().clone());

    for command in [
        approval_command("respond-no-registry", true),
        question_command("respond-no-registry-q", vec!["yes".to_owned()]),
    ] {
        let failure = respond(&handler, command)
            .await
            .expect_err("missing registry must fail");
        assert_eq!(failure.code, ErrorCode::UnsupportedFeature);
        assert!(!failure.retryable);
    }
}

#[tokio::test]
async fn route_without_live_run_reports_wrong_run_without_storage() {
    let (_temporary, storage) = opened_storage("wrong-run").await;
    let repository = storage.repository().clone();
    let registry = RunInteractionRegistry::new(1).expect("registry capacity is valid");
    let handler = RequestHandler::new(repository.clone()).with_run_interaction_registry(registry);

    let command = approval_command("respond-no-live-run", true);
    let response = respond(&handler, command)
        .await
        .expect("wrong run should answer, not fail");
    let ResponsePayload::ApprovalResponse(receipt) = response.payload else {
        panic!("wrong run should carry an approval receipt")
    };
    assert_eq!(receipt.outcome, RunInteractionOutcome::WrongRun);
    assert!(
        repository
            .lookup_interaction_receipt(&request("respond-no-live-run"))
            .await
            .expect("receipt lookup should succeed")
            .is_none(),
        "wrong_run outcomes are never stored"
    );
}

#[tokio::test]
async fn mismatched_command_correlation_is_rejected() {
    let (_temporary, storage) = opened_storage("correlation").await;
    let registry = RunInteractionRegistry::new(1).expect("registry capacity is valid");
    let handler =
        RequestHandler::new(storage.repository().clone()).with_run_interaction_registry(registry);

    let command = approval_command("respond-correlation", true);
    let failure = handler
        .respond(&request("another-frame"), &ClientRequest::Command(command))
        .await
        .expect_err("correlation mismatch must fail");
    assert_eq!(failure.code, ErrorCode::InvalidInput);
    assert!(!failure.retryable);
}

fn approval_request_fixture() -> ApprovalRequest {
    ApprovalRequest::command(
        "cargo test".to_owned(),
        Some("C:/repos/artisan".to_owned()),
        Some("run the suite".to_owned()),
    )
    .expect("fixture approval request should be valid")
}

#[tokio::test]
async fn unknown_target_stores_receipt_and_replays_duplicate() {
    let (_temporary, storage) = opened_storage("unknown-target").await;
    let repository = storage.repository().clone();
    let registry = RunInteractionRegistry::new(1).expect("registry capacity is valid");
    let handler =
        RequestHandler::new(repository.clone()).with_run_interaction_registry(registry.clone());
    let _drainer = spawn_owner_drainer(repository.clone(), registry, 1);

    let command = approval_command("respond-ghost-route", true);
    let response = respond(&handler, command)
        .await
        .expect("unknown target should answer, not fail");
    let ResponsePayload::ApprovalResponse(receipt) = response.payload else {
        panic!("unknown target should carry an approval receipt")
    };
    assert_eq!(receipt.outcome, RunInteractionOutcome::UnknownTarget);
    assert_eq!(
        receipt.disposition,
        artisan_domain::ReceiptDisposition::Accepted
    );

    let command = approval_command("respond-ghost-route", true);
    let response = respond(&handler, command)
        .await
        .expect("replay should answer, not fail");
    let ResponsePayload::ApprovalResponse(receipt) = response.payload else {
        panic!("replay should carry an approval receipt")
    };
    assert_eq!(receipt.outcome, RunInteractionOutcome::UnknownTarget);
    assert_eq!(
        receipt.disposition,
        artisan_domain::ReceiptDisposition::Duplicate
    );
}

#[tokio::test]
async fn reused_request_id_with_changed_intent_conflicts() {
    let (_temporary, storage) = opened_storage("route-conflict").await;
    let repository = storage.repository().clone();
    let registry = RunInteractionRegistry::new(1).expect("registry capacity is valid");
    let handler =
        RequestHandler::new(repository.clone()).with_run_interaction_registry(registry.clone());
    let _drainer = spawn_owner_drainer(repository.clone(), registry, 1);

    let command = approval_command("respond-clash-route", true);
    respond(&handler, command)
        .await
        .expect("first response should settle");

    let command = approval_command("respond-clash-route", false);
    let failure = respond(&handler, command)
        .await
        .expect_err("changed intent must conflict");
    assert_eq!(failure.code, ErrorCode::IdempotencyConflict);
    assert!(!failure.retryable);
}

#[tokio::test]
async fn approval_applies_through_the_owning_loop_without_side_effects() {
    let (_temporary, storage) = opened_storage("route-applied").await;
    let repository = storage.repository().clone();
    seed_running_run(&repository).await;
    let registry = RunInteractionRegistry::new(1).expect("registry capacity is valid");
    let handler =
        RequestHandler::new(repository.clone()).with_run_interaction_registry(registry.clone());

    static REQUEST: std::sync::OnceLock<ApprovalRequest> = std::sync::OnceLock::new();
    let asked = REQUEST.get_or_init(approval_request_fixture);
    repository
        .record_approval_request(RecordApprovalRequest {
            thread_id: &thread("approve-thread"),
            run_id: &run("approve-run"),
            approval_id: &target("approval-1"),
            description: "Run the suite?".to_owned(),
            request: asked,
            requested_at: UnixMillis::from_millis(1_000),
            binding_version: 1,
        })
        .await
        .expect("request should store");
    let _drainer = spawn_owner_drainer(repository.clone(), registry, 1);

    let command = approval_command("respond-apply-route", false);
    let response = respond(&handler, command)
        .await
        .expect("deny should settle");
    let ResponsePayload::ApprovalResponse(receipt) = response.payload else {
        panic!("deny should carry an approval receipt")
    };
    assert_eq!(receipt.outcome, RunInteractionOutcome::Applied);
    assert_eq!(
        receipt.disposition,
        artisan_domain::ReceiptDisposition::Accepted
    );
    assert!(!receipt.approved);

    // The stored receipt proves the decision landed with no second effect
    // available to a replay; run-continuation itself is proven by the
    // dispatch-level tests with a live turn.
    let stored = repository
        .lookup_interaction_receipt(&request("respond-apply-route"))
        .await
        .expect("receipt should read")
        .expect("deny receipt should exist");
    assert_eq!(
        stored.disposition,
        artisan_domain::ReceiptDisposition::Accepted
    );
    assert_eq!(stored.approved, Some(false));
}

#[tokio::test]
async fn question_applies_and_replays_through_the_owning_loop() {
    let (_temporary, storage) = opened_storage("route-question").await;
    let repository = storage.repository().clone();
    seed_running_run(&repository).await;
    let registry = RunInteractionRegistry::new(1).expect("registry capacity is valid");
    let handler =
        RequestHandler::new(repository.clone()).with_run_interaction_registry(registry.clone());

    let input = QuestionInput {
        question_id: target("question-1"),
        text: "Which profile should run?".to_owned(),
        header: None,
        multi_select: false,
        options: Some(vec![
            QuestionOption::new("fast".to_owned(), None).expect("fixture option should be valid"),
        ]),
    };
    repository
        .record_question_request(artisan_database::RecordQuestionRequest {
            thread_id: &thread("approve-thread"),
            run_id: &run("approve-run"),
            question_id: &target("question-1"),
            input: &input,
            requested_at: UnixMillis::from_millis(1_000),
            binding_version: 1,
        })
        .await
        .expect("question should store");
    let _drainer = spawn_owner_drainer(repository.clone(), registry, 1);

    let command = question_command("respond-answer-route", vec!["fast".to_owned()]);
    let response = respond(&handler, command)
        .await
        .expect("answer should settle");
    let ResponsePayload::QuestionResponse(receipt) = response.payload else {
        panic!("answer should carry a question receipt")
    };
    assert_eq!(receipt.outcome, RunInteractionOutcome::Applied);
    assert_eq!(receipt.answers, vec!["fast".to_owned()]);

    let command = question_command("respond-answer-route", vec!["fast".to_owned()]);
    let response = respond(&handler, command)
        .await
        .expect("replay should settle");
    let ResponsePayload::QuestionResponse(receipt) = response.payload else {
        panic!("replay should carry a question receipt")
    };
    assert_eq!(receipt.outcome, RunInteractionOutcome::Applied);
    assert_eq!(
        receipt.disposition,
        artisan_domain::ReceiptDisposition::Duplicate
    );
}
