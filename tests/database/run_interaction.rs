//! A-approve persistence coverage: pending approval/question rows plus
//! idempotent response receipts.
//!
//! Recording stores the provider's requested state; resolving settles one
//! client response in a single receipt-replaying, bind-fenced transaction.
//! These tests prove idempotent replays (duplicate receipt, no double
//! effect), unknown-id targets, wrong-run rejection, bind-scope rejection
//! for rebound runs, deny-then-allow flows, restart-durable requested state
//! that stays resolvable after reopening the database, settle cleanup that
//! never leaks pending rows across runs, and the wake-signal instants that
//! keep human-blocked runs from reading as stalled.

use artisan_database::{
    AppliedInteraction, BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch,
    CreateThreadInput, DispatchLeaseOwner, LaunchClaimedRun, LaunchClaimedRunOutcome,
    ProviderBindingBytes, QueueFirstMessageInput, RecordApprovalRequest, RecordInteractionOutcome,
    RecordQuestionRequest, Repository, ResolveInteractionOutcome, ResolveScope,
    RunInteractionError, RunLaunchCredentials, RunStartKey, SetThreadEngineConfigInput,
    SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, ApprovalRequest, ByteLimit, CountLimit, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, InteractionKind, InteractionOutcome,
    MessageBody, MessageId, NetworkAccess, ObservationId, OpenCode2Selection, PermissionId,
    ProjectId, QuestionInput, QuestionOption, ReceiptDisposition, RequestId, RespondApproval,
    RespondQuestion, RunId, ThreadId, ThreadTitle, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ConnectionTrait, DatabaseConnection};

const OWNER_BYTES: [u8; 32] = [0xa1; 32];
const LEASE_BYTES: [u8; 32] = [0xb2; 32];
const CLAIM_TOKEN_BYTES: [u8; 32] = [0xc3; 32];
const START_KEY_BYTES: [u8; 32] = [0xd4; 32];
const DISPATCH_OWNER_BYTE: u8 = 0x11;

const THREAD_ID: &str = "approve-thread-1";
const RUN_ID: &str = "approve-run-1";
const APPROVAL_ID: &str = "approval-1";
const QUESTION_ID: &str = "question-1";

const REQUESTED_AT_MS: i64 = 1_000;
const RESPONDED_AT_MS: i64 = 1_100;

fn oid(value: &str) -> ObservationId {
    ObservationId::parse(value.to_owned()).expect("fixture identity is valid")
}

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("fixture request id should be valid")
}

async fn memory_repository() -> (DatabaseConnection, Repository) {
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
        PermissionId::parse("permission-approve").expect("permission id is valid"),
        EngineAgentId::parse("agent-approve").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-approve").expect("profile id is valid"),
            EngineModelId::parse("model-approve").expect("model id is valid"),
            EngineRouteId::parse("route-approve").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

struct RunningPair {
    database: DatabaseConnection,
    repository: Repository,
}

/// Seeds one thread with engine configuration, then claims, launches, and
/// binds one running run the interaction rows can fence against.
#[expect(
    clippy::too_many_lines,
    reason = "the fixture seeds a complete project/thread/run aggregate before the test body; \
              extraction would just re-thread the same setup locals"
)]
async fn running_pair(database: DatabaseConnection, repository: Repository) -> RunningPair {
    use artisan_database::entities;
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};

    entities::attached_project::ActiveModel {
        project_id: Set("approve-project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    }
    .insert(&database)
    .await
    .expect("project");
    repository
        .create_thread(CreateThreadInput {
            request_id: request_id("approve-seed-thread"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            project_id: ProjectId::parse("approve-project-1").expect("project id"),
            title: ThreadTitle::parse("Thread").expect("title"),
            created_at: UnixMillis::from_millis(10),
            updated_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("thread");
    let thread_id = ThreadId::parse(THREAD_ID).expect("thread id");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request_id("approve-seed-config"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: launch_config(),
            accepted_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("engine configuration should create");
    let engine_settings = repository
        .read_thread_engine_settings(&thread_id)
        .await
        .expect("engine configuration should read")
        .expect("engine configuration should be present");
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: request_id("approve-seed-message"),
            message_id: MessageId::parse("approve-message-1").expect("message id"),
            thread_id: thread_id.clone(),
            body: MessageBody::parse("first durable body").expect("body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("queue");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([DISPATCH_OWNER_BYTE; 32]),
            claimed_at: UnixMillis::from_millis(100),
            lease_expires_at: UnixMillis::from_millis(600),
        })
        .await
        .expect("claim")
        .expect("claimed");
    let run_id = RunId::parse(RUN_ID).expect("run id");
    let start_key = RunStartKey::new(START_KEY_BYTES);
    let credentials = RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES);
    let LaunchClaimedRunOutcome::Started(launched) = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &artisan_domain::TurnId::parse("approve-turn-1").expect("turn id"),
            item_id: &artisan_domain::ItemId::parse("approve-item-1").expect("item id"),
            first_patch_id: &artisan_domain::PatchId::parse("approve-patch-1").expect("patch id"),
            second_patch_id: &artisan_domain::PatchId::parse("approve-patch-2").expect("patch id"),
            operated_at: UnixMillis::from_millis(150),
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
        .expect("bind")
    else {
        panic!("bind should be fresh")
    };
    RunningPair {
        database,
        repository,
    }
}

fn approval_request<'a>(
    thread: &'a ThreadId,
    run: &'a RunId,
    approval: &'a ObservationId,
    description: &str,
    requested_at_ms: i64,
) -> RecordApprovalRequest<'a> {
    RecordApprovalRequest {
        thread_id: thread,
        run_id: run,
        approval_id: approval,
        description: description.to_owned(),
        request: approval_command_request(),
        requested_at: UnixMillis::from_millis(requested_at_ms),
        binding_version: 1,
    }
}

fn approval_command_request() -> &'static ApprovalRequest {
    static REQUEST: std::sync::OnceLock<ApprovalRequest> = std::sync::OnceLock::new();
    REQUEST.get_or_init(|| {
        ApprovalRequest::command(
            "cargo test".to_owned(),
            Some("C:/repos/artisan".to_owned()),
            Some("run the suite".to_owned()),
        )
        .expect("fixture approval request should be valid")
    })
}

fn question_input() -> QuestionInput {
    QuestionInput {
        question_id: oid(QUESTION_ID),
        text: "Which profile should run?".to_owned(),
        header: Some("profile".to_owned()),
        multi_select: false,
        options: Some(vec![
            QuestionOption::new("fast".to_owned(), Some("optimize for speed".to_owned()))
                .expect("fixture option should be valid"),
            QuestionOption::new("smart".to_owned(), None).expect("fixture option should be valid"),
        ]),
    }
}

fn resolve_scope() -> ResolveScope {
    ResolveScope {
        binding_version: 1,
        responded_at: UnixMillis::from_millis(RESPONDED_AT_MS),
    }
}

fn deny_command(request: &str) -> RespondApproval {
    RespondApproval::new(
        request_id(request),
        ThreadId::parse(THREAD_ID).expect("thread id"),
        RunId::parse(RUN_ID).expect("run id"),
        oid(APPROVAL_ID),
        false,
    )
}

fn allow_command(request: &str, approval: &str) -> RespondApproval {
    RespondApproval::new(
        request_id(request),
        ThreadId::parse(THREAD_ID).expect("thread id"),
        RunId::parse(RUN_ID).expect("run id"),
        oid(approval),
        true,
    )
}

#[tokio::test]
async fn approval_request_records_and_replays_identically() {
    let (_database, repository) = memory_repository().await;
    let thread = ThreadId::parse(THREAD_ID).expect("thread id");
    let run = RunId::parse(RUN_ID).expect("run id");
    let approval = oid(APPROVAL_ID);

    let outcome = repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &approval,
            "Run the suite?",
            REQUESTED_AT_MS,
        ))
        .await
        .expect("record should store");
    let RecordInteractionOutcome::Recorded(first) = outcome else {
        panic!("first record should store")
    };
    assert_eq!(first.sequence, 1);

    let outcome = repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &approval,
            "Run the suite?",
            REQUESTED_AT_MS,
        ))
        .await
        .expect("replay should answer");
    let RecordInteractionOutcome::AlreadyRecorded(replay) = outcome else {
        panic!("replay should not store twice")
    };
    assert_eq!(replay.sequence, first.sequence);

    let conflict = repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &approval,
            "A different description",
            REQUESTED_AT_MS,
        ))
        .await;
    assert!(
        matches!(conflict, Err(RunInteractionError::RequestConflict { .. })),
        "same target with different content must conflict"
    );
}

#[tokio::test]
async fn deny_resolves_without_side_effect_then_replays_duplicate() {
    let (database, repository) = memory_repository().await;
    let RunningPair { repository, .. } = running_pair(database, repository).await;
    let thread = ThreadId::parse(THREAD_ID).expect("thread id");
    let run = RunId::parse(RUN_ID).expect("run id");
    let approval = oid(APPROVAL_ID);
    repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &approval,
            "Run the suite?",
            REQUESTED_AT_MS,
        ))
        .await
        .expect("record should store");

    let outcome = repository
        .resolve_approval_response(&deny_command("respond-deny-1"), &resolve_scope())
        .await
        .expect("deny should resolve");
    let ResolveInteractionOutcome::Applied(AppliedInteraction {
        receipt,
        requested,
        resolved_sequence,
    }) = outcome
    else {
        panic!("deny should apply")
    };
    assert_eq!(receipt.outcome, InteractionOutcome::Applied);
    assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(receipt.approved, Some(false));
    assert!(resolved_sequence > 0);
    let snapshot = requested.approval.expect("approval snapshot");
    assert_eq!(snapshot.description, "Run the suite?");
    assert_eq!(snapshot.requested_sequence, 1);

    // The run is untouched: still running, no terminal, no checkpoint write.
    let stored = repository
        .lookup_interaction_receipt(&request_id("respond-deny-1"))
        .await
        .expect("receipt should read")
        .expect("deny receipt should exist");
    assert_eq!(stored.outcome, InteractionOutcome::Applied);
    assert_eq!(stored.disposition, ReceiptDisposition::Accepted);

    let replay = repository
        .resolve_approval_response(&deny_command("respond-deny-1"), &resolve_scope())
        .await
        .expect("replay should answer");
    let ResolveInteractionOutcome::Duplicate(duplicate) = replay else {
        panic!("replay should duplicate")
    };
    assert_eq!(duplicate.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(duplicate.outcome, InteractionOutcome::Applied);
    assert_eq!(duplicate.approved, Some(false));
}

#[tokio::test]
async fn second_request_allows_after_deny() {
    let (database, repository) = memory_repository().await;
    let RunningPair { repository, .. } = running_pair(database, repository).await;
    let thread = ThreadId::parse(THREAD_ID).expect("thread id");
    let run = RunId::parse(RUN_ID).expect("run id");
    repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &oid(APPROVAL_ID),
            "Run the suite?",
            REQUESTED_AT_MS,
        ))
        .await
        .expect("first request should store");
    repository
        .resolve_approval_response(&deny_command("respond-deny-1"), &resolve_scope())
        .await
        .expect("deny should apply");

    repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &oid("approval-2"),
            "Run the suite again?",
            REQUESTED_AT_MS + 10,
        ))
        .await
        .expect("second request should store");
    let outcome = repository
        .resolve_approval_response(
            &allow_command("respond-allow-2", "approval-2"),
            &resolve_scope(),
        )
        .await
        .expect("allow should resolve");
    let ResolveInteractionOutcome::Applied(applied) = outcome else {
        panic!("allow should apply")
    };
    assert_eq!(applied.receipt.approved, Some(true));
    assert_eq!(
        applied.requested.approval.expect("snapshot").description,
        "Run the suite again?"
    );
}

#[tokio::test]
async fn question_resolves_with_answers_and_replays() {
    let (database, repository) = memory_repository().await;
    let RunningPair { repository, .. } = running_pair(database, repository).await;
    let thread = ThreadId::parse(THREAD_ID).expect("thread id");
    let run = RunId::parse(RUN_ID).expect("run id");
    let input = question_input();
    repository
        .record_question_request(RecordQuestionRequest {
            thread_id: &thread,
            run_id: &run,
            question_id: &oid(QUESTION_ID),
            input: &input,
            requested_at: UnixMillis::from_millis(REQUESTED_AT_MS),
            binding_version: 1,
        })
        .await
        .expect("question should store");

    let answer = RespondQuestion::new(
        request_id("respond-answer-1"),
        thread.clone(),
        run.clone(),
        oid(QUESTION_ID),
        vec!["fast".to_owned()],
    )
    .expect("fixture answers should be valid");
    let outcome = repository
        .resolve_question_response(&answer, &resolve_scope())
        .await
        .expect("answer should resolve");
    let ResolveInteractionOutcome::Applied(applied) = outcome else {
        panic!("answer should apply")
    };
    assert_eq!(applied.receipt.outcome, InteractionOutcome::Applied);
    assert_eq!(applied.receipt.answers, vec!["fast".to_owned()]);
    let snapshot = applied.requested.question.expect("question snapshot");
    assert_eq!(snapshot.input.text, "Which profile should run?");
    assert_eq!(
        snapshot.input.options.expect("options").len(),
        2,
        "stored options should round-trip"
    );

    let replay = repository
        .resolve_question_response(&answer, &resolve_scope())
        .await
        .expect("replay should answer");
    assert!(
        matches!(replay, ResolveInteractionOutcome::Duplicate(_)),
        "replayed answers must not apply twice"
    );
}

#[tokio::test]
async fn unknown_target_stores_and_replays_without_touching_runs() {
    let (database, repository) = memory_repository().await;
    let RunningPair { repository, .. } = running_pair(database, repository).await;
    let outcome = repository
        .resolve_approval_response(&deny_command("respond-ghost-1"), &resolve_scope())
        .await
        .expect("unknown target should settle");
    let ResolveInteractionOutcome::UnknownTarget(stored) = outcome else {
        panic!("ghost id should be unknown")
    };
    assert_eq!(stored.outcome, InteractionOutcome::UnknownTarget);
    assert_eq!(stored.disposition, ReceiptDisposition::Accepted);

    let replay = repository
        .resolve_approval_response(&deny_command("respond-ghost-1"), &resolve_scope())
        .await
        .expect("replay should answer");
    let ResolveInteractionOutcome::Duplicate(duplicate) = replay else {
        panic!("unknown target replay should duplicate")
    };
    assert_eq!(duplicate.outcome, InteractionOutcome::UnknownTarget);
}

#[tokio::test]
async fn second_response_to_resolved_target_reports_already_resolved() {
    let (database, repository) = memory_repository().await;
    let RunningPair { repository, .. } = running_pair(database, repository).await;
    let thread = ThreadId::parse(THREAD_ID).expect("thread id");
    let run = RunId::parse(RUN_ID).expect("run id");
    repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &oid(APPROVAL_ID),
            "Run the suite?",
            REQUESTED_AT_MS,
        ))
        .await
        .expect("record should store");
    repository
        .resolve_approval_response(&deny_command("respond-first-1"), &resolve_scope())
        .await
        .expect("first response should apply");

    let outcome = repository
        .resolve_approval_response(&deny_command("respond-second-1"), &resolve_scope())
        .await
        .expect("second response should settle");
    let ResolveInteractionOutcome::AlreadyResolved(stored) = outcome else {
        panic!("resolved target should report already-resolved")
    };
    assert_eq!(stored.disposition, ReceiptDisposition::Accepted);
}

#[tokio::test]
async fn reused_request_id_with_changed_intent_conflicts() {
    let (database, repository) = memory_repository().await;
    let RunningPair { repository, .. } = running_pair(database, repository).await;
    let thread = ThreadId::parse(THREAD_ID).expect("thread id");
    let run = RunId::parse(RUN_ID).expect("run id");
    repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &oid(APPROVAL_ID),
            "Run the suite?",
            REQUESTED_AT_MS,
        ))
        .await
        .expect("record should store");
    repository
        .resolve_approval_response(&deny_command("respond-clash-1"), &resolve_scope())
        .await
        .expect("deny should apply");

    let allow_same_id = allow_command("respond-clash-1", APPROVAL_ID);
    let outcome = repository
        .resolve_approval_response(&allow_same_id, &resolve_scope())
        .await
        .expect("conflict should settle");
    let ResolveInteractionOutcome::Conflict(stored) = outcome else {
        panic!("changed intent must conflict")
    };
    assert_eq!(stored.approved, Some(false), "the original deny must stand");
}

#[tokio::test]
async fn response_for_another_run_is_rejected_without_storage() {
    let (database, repository) = memory_repository().await;
    let RunningPair { repository, .. } = running_pair(database, repository).await;
    let foreign = RespondApproval::new(
        request_id("respond-foreign-1"),
        ThreadId::parse(THREAD_ID).expect("thread id"),
        RunId::parse("approve-run-foreign").expect("run id"),
        oid(APPROVAL_ID),
        true,
    );
    let outcome = repository
        .resolve_approval_response(&foreign, &resolve_scope())
        .await
        .expect("foreign run should settle");
    assert!(
        matches!(outcome, ResolveInteractionOutcome::WrongRun),
        "unknown run must be wrong_run"
    );
    assert!(
        repository
            .lookup_interaction_receipt(&request_id("respond-foreign-1"))
            .await
            .expect("receipt lookup should succeed")
            .is_none(),
        "wrong_run outcomes are never stored"
    );
}

#[tokio::test]
async fn response_for_a_rebound_run_is_rejected_without_storage() {
    let (database, repository) = memory_repository().await;
    let RunningPair {
        database,
        repository,
    } = running_pair(database, repository).await;
    let thread = ThreadId::parse(THREAD_ID).expect("thread id");
    let run = RunId::parse(RUN_ID).expect("run id");
    repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &oid(APPROVAL_ID),
            "Run the suite?",
            REQUESTED_AT_MS,
        ))
        .await
        .expect("record should store");

    // Simulate a provider rebind: the run now lives under a new binding
    // version, so the response pinned to the old bind must not misapply.
    database
        .execute_unprepared(
            "UPDATE assistant_runs SET provider_binding_version = 2 WHERE run_id = 'approve-run-1'",
        )
        .await
        .expect("rebind simulation should update the run row");
    let outcome = repository
        .resolve_approval_response(&deny_command("respond-rebound-1"), &resolve_scope())
        .await
        .expect("rebound response should settle");
    assert!(
        matches!(outcome, ResolveInteractionOutcome::WrongRun),
        "rebound run must be wrong_run"
    );
    assert!(
        repository
            .lookup_interaction_receipt(&request_id("respond-rebound-1"))
            .await
            .expect("receipt lookup should succeed")
            .is_none(),
        "rebound rejections are never stored"
    );
}

#[tokio::test]
async fn settle_cleans_pending_rows_but_keeps_receipts() {
    let (database, repository) = memory_repository().await;
    let RunningPair { repository, .. } = running_pair(database, repository).await;
    let thread = ThreadId::parse(THREAD_ID).expect("thread id");
    let run = RunId::parse(RUN_ID).expect("run id");
    repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &oid(APPROVAL_ID),
            "Run the suite?",
            REQUESTED_AT_MS,
        ))
        .await
        .expect("record should store");
    repository
        .resolve_approval_response(&deny_command("respond-settle-1"), &resolve_scope())
        .await
        .expect("deny should apply");
    assert_eq!(
        repository
            .pending_interactions(&run)
            .await
            .expect("pending should read")
            .len(),
        1
    );

    assert_eq!(
        repository
            .settle_run_interactions(&run)
            .await
            .expect("settle should delete"),
        1
    );
    assert!(
        repository
            .pending_interactions(&run)
            .await
            .expect("pending should read")
            .is_empty()
    );
    assert_eq!(
        repository
            .settle_run_interactions(&run)
            .await
            .expect("second settle should succeed"),
        0
    );
    assert!(
        repository
            .lookup_interaction_receipt(&request_id("respond-settle-1"))
            .await
            .expect("receipt should read")
            .is_some(),
        "receipts survive settle so replays still duplicate"
    );
}

#[tokio::test]
async fn pending_instants_signal_human_blocked_runs_until_resolved() {
    let (database, repository) = memory_repository().await;
    let RunningPair { repository, .. } = running_pair(database, repository).await;
    assert!(
        repository
            .pending_request_instants()
            .await
            .expect("instants should read")
            .is_empty()
    );

    let thread = ThreadId::parse(THREAD_ID).expect("thread id");
    let run = RunId::parse(RUN_ID).expect("run id");
    repository
        .record_approval_request(approval_request(
            &thread,
            &run,
            &oid(APPROVAL_ID),
            "Run the suite?",
            REQUESTED_AT_MS,
        ))
        .await
        .expect("record should store");
    assert_eq!(
        repository
            .pending_request_instants()
            .await
            .expect("instants should read"),
        vec![REQUESTED_AT_MS]
    );

    repository
        .resolve_approval_response(&deny_command("respond-signal-1"), &resolve_scope())
        .await
        .expect("deny should apply");
    assert!(
        repository
            .pending_request_instants()
            .await
            .expect("instants should read")
            .is_empty(),
        "resolved runs stop signalling"
    );
}

#[tokio::test]
async fn requested_state_survives_restart_and_stays_resolvable() {
    let directory =
        std::env::temp_dir().join(format!("artisan-approve-restart-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temporary directory should be created");
    let path = directory.join("forge.sqlite3");

    {
        let database = connect(
            SqliteConfig::file(&path)
                .min_connections(1)
                .max_connections(1)
                .sqlx_logging(false),
        )
        .await
        .expect("file database should open");
        migrate_to_current(&database)
            .await
            .expect("file database should migrate");
        let repository = Repository::new(database.clone());
        let RunningPair { repository, .. } = running_pair(database.clone(), repository).await;
        let thread = ThreadId::parse(THREAD_ID).expect("thread id");
        let run = RunId::parse(RUN_ID).expect("run id");
        repository
            .record_approval_request(approval_request(
                &thread,
                &run,
                &oid(APPROVAL_ID),
                "Run the suite?",
                REQUESTED_AT_MS,
            ))
            .await
            .expect("record should store");
        drop(repository);
        drop(database);
    };

    // Forge restarts: reopen the same file and resolve the requested row.
    let database = connect(
        SqliteConfig::file(&path)
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("file database should reopen");
    migrate_to_current(&database)
        .await
        .expect("reopen must be a migration no-op");
    let repository = Repository::new(database.clone());
    let run = RunId::parse(RUN_ID).expect("run id");
    let pending = repository
        .pending_interactions(&run)
        .await
        .expect("pending should survive restart");
    assert_eq!(pending.len(), 1);
    assert!(pending[0].requested);
    assert_eq!(pending[0].kind, InteractionKind::Approval);
    assert_eq!(
        repository
            .pending_request_instants()
            .await
            .expect("instants should survive restart"),
        vec![REQUESTED_AT_MS]
    );

    let outcome = repository
        .resolve_approval_response(&deny_command("respond-restart-1"), &resolve_scope())
        .await
        .expect("post-restart resolve should settle");
    assert!(
        matches!(outcome, ResolveInteractionOutcome::Applied(_)),
        "requested state stays resolvable after restart"
    );
    drop(repository);
    drop(database);
    let _cleanup = std::fs::remove_dir_all(&directory);
}
