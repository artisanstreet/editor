//! Focused SQLite coverage for bounded queued-message edit/discard storage.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{self, DispatchState};
use artisan_database::{
    AttachProjectInput, ClaimMessageDispatch, CreateThreadInput, DispatchFailureReason,
    DispatchLeaseOwner, FailMessageDispatch, LaunchClaimedRun, QueueMessageInput,
    QueuedMessageRepositoryError, Repository, RepositoryError, RequeueMessageDispatch,
    RunLaunchCredentials, RunStartKey, SetThreadEngineConfigInput, SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, AuthoredText, ByteLimit, CountLimit, DirectoryId, DisplayName, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, ImageAttachment, ListFailedMessages,
    ListQueuedMessages, MessageId, NetworkAccess, OpenCode2Selection, PermissionId, ProjectId,
    QUEUED_MESSAGE_LIST_MAX, QueueMessagePayload, QueuedMessageListError, QueuedMessageListOrder,
    QueuedMessageWithdrawalOutcome, ReceiptDisposition, RequestId, RootPath, RunId, ThreadId,
    ThreadTitle, TurnId, ItemId, PatchId, UnixMillis, WebSearchAccess, WithdrawQueuedMessage,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
        .expect("memory database should migrate through withdrawal migration");
    (database.clone(), Repository::new(database))
}

async fn file_repository(path: &Path) -> (DatabaseConnection, Repository) {
    let database = connect(
        SqliteConfig::file(path)
            .min_connections(1)
            .max_connections(4)
            .sqlx_logging(false),
    )
    .await
    .expect("file database should open");
    migrate_to_current(&database)
        .await
        .expect("file database should migrate through withdrawal migration");
    (database.clone(), Repository::new(database))
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).expect("request id fixture should parse")
}

fn message_id(value: &str) -> MessageId {
    MessageId::parse(value).expect("message id fixture should parse")
}

fn thread_id(value: &str) -> ThreadId {
    ThreadId::parse(value).expect("thread id fixture should parse")
}

fn project_id(value: &str) -> ProjectId {
    ProjectId::parse(value).expect("project id fixture should parse")
}

fn image(name: &str, bytes: &[u8]) -> ImageAttachment {
    ImageAttachment::new("image/png", bytes.to_vec(), name)
        .expect("image fixture should satisfy native bounds")
}

fn text_payload(value: &str) -> QueueMessagePayload {
    QueueMessagePayload::text_only(value).expect("text payload fixture should be valid")
}

fn mixed_payload(text: Option<&str>) -> QueueMessagePayload {
    QueueMessagePayload::new(
        text.map(|value| AuthoredText::parse(value).expect("fixture text should parse")),
        vec![
            image("first.png", &[1, 2, 3]),
            image("second.webp", &[4, 5, 6, 7]),
        ],
    )
    .expect("mixed payload fixture should be valid")
}

async fn setup_thread(repository: &Repository) {
    repository
        .attach_project(AttachProjectInput {
            request_id: request("attach-project"),
            directory_id: DirectoryId::parse("directory-1").expect("directory should parse"),
            project_id: project_id("project-1"),
            root_path: RootPath::parse("C:/repos/artisan").expect("root should parse"),
            display_name: DisplayName::parse("Artisan").expect("display name should parse"),
            attached_at: UnixMillis::from_millis(100),
        })
        .await
        .expect("project fixture should attach");
    create_thread(repository, "thread-1", "create-thread-1").await;
}

async fn create_thread(repository: &Repository, value: &str, request_value: &str) {
    repository
        .create_thread(CreateThreadInput {
            request_id: request(request_value),
            thread_id: thread_id(value),
            project_id: project_id("project-1"),
            title: ThreadTitle::parse(format!("Thread {value}"))
                .expect("thread title should parse"),
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await
        .expect("thread fixture should create");
    // Queue admission captures authoritative settings in-transaction and
    // refuses unconfigured threads, so every fixture thread that receives
    // a queued message is configured exactly once here.
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request(&format!("{request_value}-engine")),
            thread_id: thread_id(value),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: fixture_engine_config(),
            accepted_at: UnixMillis::from_millis(250),
        })
        .await
        .expect("thread engine configuration should persist");
}

fn fixture_engine_config() -> EngineRunConfig {
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
        PermissionId::parse("permission-queue").expect("permission id is valid"),
        EngineAgentId::parse("agent-queue").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-queue").expect("profile id is valid"),
            EngineModelId::parse("model-queue").expect("model id is valid"),
            EngineRouteId::parse("route-queue").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

fn second_fixture_engine_config() -> EngineRunConfig {
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
        PermissionId::parse("permission-queue-2").expect("permission id is valid"),
        EngineAgentId::parse("agent-queue-2").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-queue-2").expect("profile id is valid"),
            EngineModelId::parse("model-queue-2").expect("model id is valid"),
            EngineRouteId::parse("route-queue-2").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

async fn queue(
    repository: &Repository,
    request_value: &str,
    message_value: &str,
    thread_value: &str,
    payload: QueueMessagePayload,
    accepted_at_ms: i64,
) {
    repository
        .queue_message(QueueMessageInput {
            request_id: request(request_value),
            message_id: message_id(message_value),
            thread_id: thread_id(thread_value),
            payload,
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(accepted_at_ms),
        })
        .await
        .expect("general message fixture should queue");
}

fn run_id(value: &str) -> RunId {
    RunId::parse(value).expect("test run id should be valid")
}

async fn queue_named(
    repository: &Repository,
    request_value: &str,
    message_value: &str,
    thread_value: &str,
    payload: QueueMessagePayload,
    steer_run_id: Option<RunId>,
    accepted_at_ms: i64,
) {
    repository
        .queue_message(QueueMessageInput {
            request_id: request(request_value),
            message_id: message_id(message_value),
            thread_id: thread_id(thread_value),
            payload,
            steer_run_id,
            accepted_at: UnixMillis::from_millis(accepted_at_ms),
        })
        .await
        .expect("named message fixture should queue");
}

fn withdrawal(
    thread_value: &str,
    message_value: &str,
    original_request_value: &str,
    withdrawal_request_value: &str,
    accepted_at_ms: i64,
) -> WithdrawQueuedMessage {
    WithdrawQueuedMessage::new(
        thread_id(thread_value),
        message_id(message_value),
        request(original_request_value),
        request(withdrawal_request_value),
        UnixMillis::from_millis(accepted_at_ms),
    )
}

fn claim(owner_byte: u8, claimed_at_ms: i64, lease_expires_at_ms: i64) -> ClaimMessageDispatch {
    ClaimMessageDispatch {
        owner: DispatchLeaseOwner::new([owner_byte; 32]),
        claimed_at: UnixMillis::from_millis(claimed_at_ms),
        lease_expires_at: UnixMillis::from_millis(lease_expires_at_ms),
    }
}

async fn dispatch(database: &DatabaseConnection, message_value: &str) -> entities::MessageDispatch {
    entities::message_dispatch::Entity::find_by_id(message_value)
        .one(database)
        .await
        .expect("dispatch query should work")
        .expect("dispatch fixture should exist")
}

#[tokio::test]
async fn exact_withdrawal_replays_and_reused_identity_conflicts() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue(
        &repository,
        "queue-1",
        "message-1",
        "thread-1",
        text_payload("keep this exact text"),
        300,
    )
    .await;

    let input = withdrawal("thread-1", "message-1", "queue-1", "withdraw-1", 301);
    let accepted = repository
        .withdraw_queued_message(input.clone())
        .await
        .expect("withdrawal should commit");
    assert_eq!(accepted.receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(accepted.outcome, QueuedMessageWithdrawalOutcome::Withdrawn);

    let duplicate = repository
        .withdraw_queued_message(input.clone())
        .await
        .expect("exact withdrawal retry should replay");
    assert_eq!(duplicate.receipt.disposition, ReceiptDisposition::Duplicate);
    let mut expected_duplicate = accepted.clone();
    expected_duplicate.receipt.disposition = ReceiptDisposition::Duplicate;
    assert_eq!(duplicate, expected_duplicate);
    let later_clock_retry = repository.withdraw_queued_message(WithdrawQueuedMessage {
        accepted_at: UnixMillis::from_millis(500), ..input.clone()
    }).await.expect("server clock advancement must replay the receipt");
    assert_eq!(later_clock_retry, expected_duplicate);
    let lookup = repository.lookup_queued_message_withdrawal(
        &input.thread_id, &input.message_id, &input.original_request_id,
        &input.withdrawal_request_id,
    ).await.expect("receipt lookup").expect("durable receipt");
    assert_eq!(lookup, expected_duplicate);


    let reused = repository
        .withdraw_queued_message(WithdrawQueuedMessage {
            original_request_id: request("different-original-request"),
            ..input
        })
        .await
        .expect_err("changed original queue identity must conflict");
    assert!(matches!(
        reused,
        QueuedMessageRepositoryError::IdempotencyConflict { .. }
    ));

    let persisted_dispatch = dispatch(&database, "message-1").await;
    assert_eq!(persisted_dispatch.state, DispatchState::Failed);
    assert_eq!(persisted_dispatch.attempt_count, 0);
    assert!(
        entities::message::Entity::find_by_id("message-1")
            .one(&database)
            .await
            .expect("message query should work")
            .is_some(),
        "withdrawal must retain immutable user history"
    );
    assert!(
        entities::command_receipt::Entity::find_by_id("queue-1")
            .one(&database)
            .await
            .expect("receipt query should work")
            .is_some(),
        "withdrawal must retain the original queue receipt"
    );
}

#[tokio::test]
async fn queued_withdrawal_is_removed_from_listing_and_retry_cannot_resurrect_it() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    let payload = mixed_payload(Some("edit me"));
    queue(
        &repository,
        "queue-retry",
        "message-retry",
        "thread-1",
        payload.clone(),
        300,
    )
    .await;

    let before = repository
        .read_queued_messages(
            ListQueuedMessages::new(
                thread_id("thread-1"),
                QueuedMessageListOrder::OldestFirst,
                QUEUED_MESSAGE_LIST_MAX,
            )
            .expect("bounded list query should build"),
        )
        .await
        .expect("queued list should read");
    assert_eq!(before.total_count(), 1);
    assert_eq!(before.messages()[0].message_id, message_id("message-retry"));

    let result = repository
        .withdraw_queued_message(withdrawal(
            "thread-1",
            "message-retry",
            "queue-retry",
            "withdraw-retry",
            301,
        ))
        .await
        .expect("queued withdrawal should commit");
    assert_eq!(result.outcome, QueuedMessageWithdrawalOutcome::Withdrawn);

    let retry = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-retry"),
            message_id: message_id("message-retry-allocated-again"),
            thread_id: thread_id("thread-1"),
            payload,
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(302),
        })
        .await
        .expect("the original queue retry should return its durable duplicate");
    assert_eq!(retry.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(retry.message_id, message_id("message-retry"));

    let after = repository
        .list_queued_messages(
            ListQueuedMessages::new(
                thread_id("thread-1"),
                QueuedMessageListOrder::LatestFirst,
                QUEUED_MESSAGE_LIST_MAX,
            )
            .expect("bounded list query should build"),
        )
        .await
        .expect("queued list should read after withdrawal");
    assert_eq!(after.total_count(), 0);
    assert!(after.messages().is_empty());
    assert!(
        repository
            .claim_next_message_dispatch(claim(0x44, 400, 500))
            .await
            .expect("claim after withdrawal should succeed")
            .is_none(),
        "the existing claim path must not resurrect failed-fenced withdrawal rows"
    );
    assert_eq!(
        dispatch(&database, "message-retry").await.state,
        DispatchState::Failed
    );
}

#[tokio::test]
async fn withdrawal_fence_races_claim_with_one_deterministic_winner() {
    let temporary = TemporaryDatabase::new("queued-message-race");
    let (database, repository) = file_repository(temporary.path()).await;
    setup_thread(&repository).await;
    queue(
        &repository,
        "queue-race",
        "message-race",
        "thread-1",
        text_payload("race"),
        300,
    )
    .await;

    let claim_repository = Repository::new(database.clone());
    let withdraw_repository = Repository::new(database.clone());
    let (claim_result, withdraw_result) = tokio::join!(
        claim_repository.claim_next_message_dispatch(claim(0x11, 400, 500)),
        withdraw_repository.withdraw_queued_message(withdrawal(
            "thread-1",
            "message-race",
            "queue-race",
            "withdraw-race",
            401,
        )),
    );
    let claimed = claim_result.expect("claim race should not fail");
    let withdrawn = withdraw_result.expect("withdrawal race should not fail");
    match (claimed, withdrawn.outcome) {
        (Some(claimed), QueuedMessageWithdrawalOutcome::TooLate) => {
            assert_eq!(claimed.message_id, message_id("message-race"));
            assert_eq!(
                dispatch(&database, "message-race").await.state,
                DispatchState::Leased
            );
        }
        (None, QueuedMessageWithdrawalOutcome::Withdrawn) => {
            assert_eq!(
                dispatch(&database, "message-race").await.state,
                DispatchState::Failed
            );
        }
        _ => panic!("claim and withdrawal must have one winner"),
    }
    drop(claim_repository);
    drop(withdraw_repository);
    drop(repository);
    database.close().await.expect("close SQLite before removing fixture");
}

#[tokio::test]
async fn leased_and_started_messages_are_too_late_and_left_untouched() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue(
        &repository,
        "queue-leased",
        "message-leased",
        "thread-1",
        text_payload("leased"),
        300,
    )
    .await;
    queue(
        &repository,
        "queue-running",
        "message-running",
        "thread-1",
        text_payload("running"),
        301,
    )
    .await;

    let leased = repository
        .claim_next_message_dispatch(claim(0x21, 400, 500))
        .await
        .expect("leased claim should work")
        .expect("leased fixture should be claimed");
    assert_eq!(leased.message_id, message_id("message-leased"));
    let leased_before = dispatch(&database, "message-leased").await;
    let leased_result = repository
        .withdraw_queued_message(withdrawal(
            "thread-1",
            "message-leased",
            "queue-leased",
            "withdraw-leased",
            401,
        ))
        .await
        .expect("leased withdrawal should produce an outcome");
    assert_eq!(
        leased_result.outcome,
        QueuedMessageWithdrawalOutcome::TooLate
    );
    assert_eq!(dispatch(&database, "message-leased").await, leased_before);

    let running = repository
        .claim_next_message_dispatch(claim(0x22, 400, 500))
        .await
        .expect("running claim should work")
        .expect("second fixture should be claimed");
    assert_eq!(running.message_id, message_id("message-running"));
    let mut running_model = dispatch(&database, "message-running")
        .await
        .into_active_model();
    running_model.state = Set(DispatchState::Running);
    running_model
        .update(&database)
        .await
        .expect("running fixture should advance");
    let running_before = dispatch(&database, "message-running").await;
    let running_result = repository
        .withdraw_queued_message(withdrawal(
            "thread-1",
            "message-running",
            "queue-running",
            "withdraw-running",
            402,
        ))
        .await
        .expect("started withdrawal should produce an outcome");
    assert_eq!(
        running_result.outcome,
        QueuedMessageWithdrawalOutcome::TooLate
    );
    assert_eq!(dispatch(&database, "message-running").await, running_before);
}

async fn claim_and_requeue_unconfigured(
    repository: &Repository,
    message_value: &str,
    owner_byte: u8,
    claimed_at_ms: i64,
    lease_expires_at_ms: i64,
    operated_at_ms: i64,
    available_at_ms: i64,
) {
    let claimed = repository
        .claim_next_message_dispatch(claim(owner_byte, claimed_at_ms, lease_expires_at_ms))
        .await
        .expect("claim should work")
        .expect("dispatch should be claimed");
    assert_eq!(claimed.message_id, message_id(message_value));
    let owner = claimed.owner;
    repository
        .requeue_message_dispatch(RequeueMessageDispatch {
            message_id: message_id(message_value),
            owner,
            operated_at: UnixMillis::from_millis(operated_at_ms),
            available_at: UnixMillis::from_millis(available_at_ms),
            reason: DispatchFailureReason::parse("engine unconfigured")
                .expect("requeue reason should be bounded"),
        })
        .await
        .expect("requeue should commit");
}

async fn queued_listing(
    repository: &Repository,
    thread_value: &str,
) -> artisan_domain::QueuedMessageListing {
    repository
        .read_queued_messages(
            ListQueuedMessages::new(
                thread_id(thread_value),
                QueuedMessageListOrder::OldestFirst,
                QUEUED_MESSAGE_LIST_MAX,
            )
            .expect("bounded list query should build"),
        )
        .await
        .expect("queued listing should read")
}

#[tokio::test]
async fn requeued_dispatch_stays_listed_with_its_last_error() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue(
        &repository,
        "queue-retry-visible",
        "message-retry-visible",
        "thread-1",
        text_payload("still waiting"),
        300,
    )
    .await;

    // A never-attempted row carries no diagnostic.
    let fresh = queued_listing(&repository, "thread-1").await;
    assert_eq!(fresh.total_count(), 1);
    assert_eq!(fresh.messages()[0].last_error, None);

    // One dispatcher claim plus a requeue with the production reason.
    claim_and_requeue_unconfigured(&repository, "message-retry-visible", 0x55, 400, 500, 410, 460)
        .await;

    // The retrying row stays in the composer projection with its error.
    let listed = queued_listing(&repository, "thread-1").await;
    assert_eq!(listed.total_count(), 1);
    assert_eq!(
        listed.messages()[0].message_id,
        message_id("message-retry-visible")
    );
    assert_eq!(
        listed.messages()[0]
            .last_error
            .as_ref()
            .expect("dispatch diagnostic should project")
            .as_str(),
        "engine unconfigured"
    );
    assert_eq!(
        dispatch(&database, "message-retry-visible").await.last_error.as_deref(),
        Some("engine unconfigured")
    );
}

#[tokio::test]
async fn requeued_dispatch_withdrawal_is_too_late_but_claimable_after_backoff() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue(
        &repository,
        "queue-retry-visible",
        "message-retry-visible",
        "thread-1",
        text_payload("still waiting"),
        300,
    )
    .await;
    claim_and_requeue_unconfigured(&repository, "message-retry-visible", 0x55, 400, 500, 410, 460)
        .await;

    // Withdrawal stays honest for claimed rows: too late, dispatch untouched.
    let late = repository
        .withdraw_queued_message(withdrawal(
            "thread-1",
            "message-retry-visible",
            "queue-retry-visible",
            "withdraw-retry-visible",
            411,
        ))
        .await
        .expect("claimed withdrawal should produce an outcome");
    assert_eq!(late.outcome, QueuedMessageWithdrawalOutcome::TooLate);
    assert_eq!(
        dispatch(&database, "message-retry-visible").await.state,
        DispatchState::Queued
    );

    // The dispatcher can claim the retry once its backoff elapses.
    let retry = repository
        .claim_next_message_dispatch(claim(0x56, 500, 600))
        .await
        .expect("retry claim should work")
        .expect("requeued dispatch should be claimable after backoff");
    assert_eq!(retry.message_id, message_id("message-retry-visible"));
    assert_eq!(retry.attempt_count, 2);
}

#[tokio::test]
async fn cross_thread_withdrawal_is_rejected_without_mutating_the_owner_thread() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    create_thread(&repository, "thread-2", "create-thread-2").await;
    queue(
        &repository,
        "queue-owner",
        "message-owner",
        "thread-1",
        text_payload("owner"),
        300,
    )
    .await;

    let error = repository
        .withdraw_queued_message(withdrawal(
            "thread-2",
            "message-owner",
            "queue-owner",
            "withdraw-cross-thread",
            301,
        ))
        .await
        .expect_err("cross-thread withdrawal must be rejected");
    assert!(matches!(
        error,
        QueuedMessageRepositoryError::CrossThread { .. }
    ));
    assert_eq!(
        dispatch(&database, "message-owner").await.state,
        DispatchState::Queued
    );
    let receipt = database
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Sqlite,
            "SELECT 1 FROM queued_message_withdrawals WHERE withdrawal_request_id = ?",
            [sea_orm::Value::String(Some(
                "withdraw-cross-thread".to_owned(),
            ))],
        ))
        .await
        .expect("withdrawal receipt query should work");
    assert!(
        receipt.is_none(),
        "rejected cross-thread input must not mint a receipt"
    );
}

#[tokio::test]
async fn image_only_and_present_empty_text_restore_exact_ordered_payload() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    let absent_text = mixed_payload(None);
    let present_empty_text = mixed_payload(Some(""));
    queue(
        &repository,
        "queue-image-only",
        "message-image-only",
        "thread-1",
        absent_text.clone(),
        300,
    )
    .await;
    queue(
        &repository,
        "queue-empty-text",
        "message-empty-text",
        "thread-1",
        present_empty_text.clone(),
        301,
    )
    .await;

    let listing = repository
        .read_queued_messages(
            ListQueuedMessages::new(
                thread_id("thread-1"),
                QueuedMessageListOrder::OldestFirst,
                2,
            )
            .expect("bounded list query should build"),
        )
        .await
        .expect("image listing should read");
    assert_eq!(listing.messages().len(), 2);
    assert!(listing.messages()[0].text.is_none());
    assert_eq!(listing.messages()[0].attachments[0].name, "first.png");
    assert_eq!(listing.messages()[0].attachments[1].name, "second.webp");
    assert_eq!(listing.messages()[0].attachments[0].size_bytes, 3);
    assert_eq!(listing.messages()[0].attachments[1].size_bytes, 4);
    assert_eq!(
        listing.messages()[1]
            .text
            .as_ref()
            .expect("present empty text should remain present")
            .as_str(),
        ""
    );

    for (message_value, queue_value, payload, withdraw_value) in [
        (
            "message-image-only",
            "queue-image-only",
            absent_text,
            "withdraw-image-only",
        ),
        (
            "message-empty-text",
            "queue-empty-text",
            present_empty_text,
            "withdraw-empty-text",
        ),
    ] {
        let result = repository
            .withdraw_queued_message(withdrawal(
                "thread-1",
                message_value,
                queue_value,
                withdraw_value,
                400,
            ))
            .await
            .expect("image withdrawal should commit");
        assert_eq!(result.outcome, QueuedMessageWithdrawalOutcome::Withdrawn);
        let restored = repository
            .read_withdrawn_message_payload(
                &thread_id("thread-1"),
                &message_id(message_value),
                &request(queue_value),
            )
            .await
            .expect("owned payload read should work")
            .expect("withdrawn payload should remain readable");
        assert_eq!(restored, payload);
    }
    assert_eq!(
        dispatch(&database, "message-image-only").await.state,
        DispatchState::Failed
    );
}

#[tokio::test]
async fn listing_is_bounded_stable_and_truthfully_reports_overflow() {
    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    for index in 0..=QUEUED_MESSAGE_LIST_MAX {
        queue(
            &repository,
            &format!("queue-{index:02}"),
            &format!("message-{index:02}"),
            "thread-1",
            text_payload(&format!("body-{index}")),
            300 + i64::try_from(index).expect("fixture index should fit"),
        )
        .await;
    }

    let oldest = repository
        .read_queued_messages(
            ListQueuedMessages::new(
                thread_id("thread-1"),
                QueuedMessageListOrder::OldestFirst,
                QUEUED_MESSAGE_LIST_MAX,
            )
            .expect("maximum page should build"),
        )
        .await
        .expect("oldest page should read");
    assert_eq!(oldest.messages().len(), QUEUED_MESSAGE_LIST_MAX);
    assert_eq!(
        oldest.total_count(),
        u64::try_from(QUEUED_MESSAGE_LIST_MAX + 1).expect("fixture count should fit")
    );
    assert!(oldest.has_more());
    assert_eq!(oldest.messages()[0].message_id, message_id("message-00"));
    assert_eq!(
        oldest.messages()[QUEUED_MESSAGE_LIST_MAX - 1].message_id,
        message_id("message-31")
    );

    let latest = repository
        .list_queued_messages(
            ListQueuedMessages::new(
                thread_id("thread-1"),
                QueuedMessageListOrder::LatestFirst,
                2,
            )
            .expect("small page should build"),
        )
        .await
        .expect("latest page should read");
    assert_eq!(latest.total_count(), 33);
    assert!(latest.has_more());
    assert_eq!(latest.messages()[0].message_id, message_id("message-32"));
    assert_eq!(latest.messages()[1].message_id, message_id("message-31"));

    assert!(matches!(
        ListQueuedMessages::new(
            thread_id("thread-1"),
            QueuedMessageListOrder::OldestFirst,
            QUEUED_MESSAGE_LIST_MAX + 1,
        ),
        Err(QueuedMessageListError::TooLarge { .. })
    ));
}

#[tokio::test]
async fn not_queued_is_a_durable_distinct_outcome_for_a_missing_message() {
    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    let result = repository
        .withdraw_queued_message(withdrawal(
            "thread-1",
            "message-never-queued",
            "queue-never-queued",
            "withdraw-not-queued",
            300,
        ))
        .await
        .expect("missing target outcome should be durably recorded");
    assert_eq!(result.receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(result.outcome, QueuedMessageWithdrawalOutcome::NotQueued);

    let duplicate = repository
        .withdraw_queued_message(withdrawal(
            "thread-1",
            "message-never-queued",
            "queue-never-queued",
            "withdraw-not-queued",
            300,
        ))
        .await
        .expect("missing target retry should replay");
    assert_eq!(duplicate.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(duplicate.outcome, QueuedMessageWithdrawalOutcome::NotQueued);
}

struct TemporaryDatabase {
    path: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should follow Unix epoch")
            .as_nanos();
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self {
            path: std::env::temp_dir().join(format!(
                "artisan-queued-message-{label}-{}-{timestamp}-{sequence}.sqlite3",
                std::process::id()
            )),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-shm", "-wal"] {
            let candidate = PathBuf::from(format!("{}{suffix}", self.path.display()));
            if let Err(error) = std::fs::remove_file(&candidate) {
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::NotFound,
                    "failed to remove temporary database {}: {error}",
                    candidate.display()
                );
            }
        }
    }
}

fn fail(
    message_value: &str,
    owner_byte: u8,
    operated_at_ms: i64,
    reason: &str,
) -> FailMessageDispatch {
    FailMessageDispatch {
        message_id: message_id(message_value),
        owner: DispatchLeaseOwner::new([owner_byte; 32]),
        operated_at: UnixMillis::from_millis(operated_at_ms),
        reason: DispatchFailureReason::parse(reason).expect("failure reason should parse"),
    }
}

async fn claim_succeeds(repository: &Repository, owner_byte: u8) {
    repository
        .claim_next_message_dispatch(claim(owner_byte, 400, 900))
        .await
        .expect("claim should succeed")
        .expect("queued dispatch should be claimed");
}

const INTERRUPTED_REASON: &str = "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue";

#[tokio::test]
async fn failed_dispatch_listing_surfaces_terminal_failure_with_exact_reason() {
    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue(
        &repository,
        "queue-1",
        "message-1",
        "thread-1",
        text_payload("keep this exact text"),
        300,
    )
    .await;
    claim_succeeds(&repository, 0x11).await;
    repository
        .fail_message_dispatch(fail("message-1", 0x11, 500, INTERRUPTED_REASON))
        .await
        .expect("terminal failure should commit");

    let listing = repository
        .read_failed_messages(
            ListFailedMessages::new(thread_id("thread-1"), 32).expect("failed query"),
        )
        .await
        .expect("failed listing should read");
    assert_eq!(listing.total_count(), 1);
    assert!(!listing.has_more());
    let [summary] = listing.messages() else {
        panic!("exactly one failed row should be listed");
    };
    assert_eq!(summary.message_id, message_id("message-1"));
    assert_eq!(summary.thread_id, thread_id("thread-1"));
    assert_eq!(summary.original_request_id, request("queue-1"));
    assert_eq!(
        summary.text.as_ref().map(artisan_domain::AuthoredText::as_str),
        Some("keep this exact text")
    );
    assert!(summary.attachments.is_empty());
    assert_eq!(summary.accepted_at, UnixMillis::from_millis(300));
    assert_eq!(summary.failed_at, UnixMillis::from_millis(500));
    assert_eq!(summary.reason.as_str(), INTERRUPTED_REASON);

    let queued = repository
        .read_queued_messages(
            ListQueuedMessages::new(thread_id("thread-1"), QueuedMessageListOrder::OldestFirst, 32)
                .expect("queued query"),
        )
        .await
        .expect("queued listing should read");
    assert_eq!(queued.total_count(), 0);
    assert!(queued.messages().is_empty());
}

#[tokio::test]
async fn failed_dispatch_listing_excludes_withdrawn_queued_and_live_rows() {
    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue(
        &repository,
        "queue-withdrawn",
        "message-withdrawn",
        "thread-1",
        text_payload("withdraw me"),
        300,
    )
    .await;
    queue(
        &repository,
        "queue-live",
        "message-live",
        "thread-1",
        text_payload("still queued"),
        301,
    )
    .await;
    queue(
        &repository,
        "queue-failed",
        "message-failed",
        "thread-1",
        text_payload("terminally failed"),
        302,
    )
    .await;
    repository
        .withdraw_queued_message(withdrawal(
            "thread-1",
            "message-withdrawn",
            "queue-withdrawn",
            "withdraw-1",
            303,
        ))
        .await
        .expect("withdrawal should commit");
    // The claim takes the oldest eligible row: the withdrawn row is fenced
    // out, so the live row claims first and must be returned to `queued`
    // before the failed row can be claimed and failed terminally.
    let _live = repository
        .claim_next_message_dispatch(claim(0x11, 400, 900))
        .await
        .expect("live claim should succeed")
        .expect("live dispatch should be claimed");
    repository
        .requeue_message_dispatch(RequeueMessageDispatch {
            message_id: message_id("message-live"),
            owner: DispatchLeaseOwner::new([0x11; 32]),
            operated_at: UnixMillis::from_millis(410),
            available_at: UnixMillis::from_millis(10_000),
            reason: DispatchFailureReason::parse("engine unconfigured")
                .expect("requeue reason should parse"),
        })
        .await
        .expect("live row should requeue");
    let _failed = repository
        .claim_next_message_dispatch(claim(0x11, 420, 900))
        .await
        .expect("failed claim should succeed")
        .expect("failed dispatch should be claimed");
    repository
        .fail_message_dispatch(fail("message-failed", 0x11, 500, INTERRUPTED_REASON))
        .await
        .expect("terminal failure should commit");

    let listing = repository
        .read_failed_messages(
            ListFailedMessages::new(thread_id("thread-1"), 32).expect("failed query"),
        )
        .await
        .expect("failed listing should read");
    assert_eq!(listing.total_count(), 1);
    let [summary] = listing.messages() else {
        panic!("only the terminally failed row should be listed");
    };
    assert_eq!(summary.message_id, message_id("message-failed"));
}

#[tokio::test]
async fn failed_message_payload_read_returns_exact_bytes_for_recovery() {
    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue(
        &repository,
        "queue-1",
        "message-1",
        "thread-1",
        mixed_payload(Some("failed with images")),
        300,
    )
    .await;
    claim_succeeds(&repository, 0x11).await;
    repository
        .fail_message_dispatch(fail("message-1", 0x11, 500, INTERRUPTED_REASON))
        .await
        .expect("terminal failure should commit");

    let payload = repository
        .read_failed_message_payload(
            &thread_id("thread-1"),
            &message_id("message-1"),
            &request("queue-1"),
        )
        .await
        .expect("failed payload read should succeed")
        .expect("terminally failed row should be readable");
    assert_eq!(
        payload.text().map(artisan_domain::AuthoredText::as_str),
        Some("failed with images")
    );
    assert_eq!(payload.attachments().len(), 2);
    assert_eq!(payload.attachments()[0].bytes(), &[1, 2, 3]);
    assert_eq!(payload.attachments()[1].bytes(), &[4, 5, 6, 7]);
}

#[tokio::test]
async fn failed_message_payload_read_rejects_live_withdrawn_and_missing_rows() {
    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue(
        &repository,
        "queue-live",
        "message-live",
        "thread-1",
        text_payload("still live"),
        300,
    )
    .await;
    queue(
        &repository,
        "queue-gone",
        "message-gone",
        "thread-1",
        text_payload("withdrawn"),
        301,
    )
    .await;
    repository
        .withdraw_queued_message(withdrawal(
            "thread-1",
            "message-gone",
            "queue-gone",
            "withdraw-1",
            302,
        ))
        .await
        .expect("withdrawal should commit");

    assert!(
        repository
            .read_failed_message_payload(
                &thread_id("thread-1"),
                &message_id("message-live"),
                &request("queue-live"),
            )
            .await
            .expect("live payload read should succeed")
            .is_none(),
        "a live row must not resolve through the failed seam"
    );
    assert!(
        repository
            .read_failed_message_payload(
                &thread_id("thread-1"),
                &message_id("message-gone"),
                &request("queue-gone"),
            )
            .await
            .expect("withdrawn payload read should succeed")
            .is_none(),
        "a withdrawn row must not resolve through the failed seam"
    );
    assert!(
        repository
            .read_failed_message_payload(
                &thread_id("thread-1"),
                &message_id("message-missing"),
                &request("queue-live"),
            )
            .await
            .expect("missing payload read should succeed")
            .is_none(),
        "a missing row must not resolve through the failed seam"
    );
    assert!(
        repository
            .read_failed_message_payload(
                &thread_id("thread-1"),
                &message_id("message-live"),
                &request("queue-gone"),
            )
            .await
            .expect("mismatched payload read should succeed")
            .is_none(),
        "a mismatched original request must not resolve through the failed seam"
    );
}

#[tokio::test]
async fn unconfigured_accept_refuses_typed_and_persists_nothing() {
    let (_database, repository) = memory_repository().await;
    repository
        .attach_project(AttachProjectInput {
            request_id: request("attach-bare"),
            directory_id: DirectoryId::parse("directory-bare").expect("directory should parse"),
            project_id: project_id("project-bare"),
            root_path: RootPath::parse("C:/repos/bare").expect("root should parse"),
            display_name: DisplayName::parse("Bare").expect("display name should parse"),
            attached_at: UnixMillis::from_millis(100),
        })
        .await
        .expect("project fixture should attach");
    repository
        .create_thread(CreateThreadInput {
            request_id: request("create-bare"),
            thread_id: thread_id("thread-bare"),
            project_id: project_id("project-bare"),
            title: ThreadTitle::parse("Thread bare").expect("thread title should parse"),
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await
        .expect("thread fixture should create");
    let error = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-bare"),
            message_id: message_id("message-bare"),
            thread_id: thread_id("thread-bare"),
            payload: text_payload("no configuration"),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(300),
        })
        .await
        .expect_err("unconfigured accept must refuse");
    assert!(
        matches!(
            error,
            RepositoryError::ThreadEngineNotConfigured { .. }
        ),
        "refusal must be typed, got {error:?}"
    );
    assert!(
        repository
            .lookup_queue_message(
                &request("queue-bare"),
                &thread_id("thread-bare"),
                &text_payload("no configuration"),
                None,
            )
            .await
            .expect("lookup should work")
            .is_none(),
        "refused accept must persist nothing"
    );
}

#[tokio::test]
async fn steer_target_persists_and_claim_payload_returns_it() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue_named(
        &repository,
        "queue-steer",
        "message-steer",
        "thread-1",
        text_payload("steer me"),
        Some(run_id("run-steer-1")),
        300,
    )
    .await;
    let dispatch = dispatch(&database, "message-steer").await;
    assert_eq!(
        dispatch.steer_run_id.as_deref(),
        Some("run-steer-1"),
        "steer target must persist on the dispatch row"
    );
    let payload = repository
        .read_queue_message_dispatch_payload(&message_id("message-steer"))
        .await
        .expect("dispatch payload should load")
        .expect("dispatch payload should exist");
    assert_eq!(
        payload.steer_target.as_ref().map(|target| target.run_id()),
        Some(&run_id("run-steer-1")),
    );
    let snapshot = repository
        .read_receipt_engine_settings(&request("queue-steer"))
        .await
        .expect("receipt settings should load")
        .expect("accept must capture a settings snapshot");
    assert_eq!(
        snapshot.config().selection().engine_id(),
        artisan_domain::EngineId::OpenCode2,
    );
}

#[tokio::test]
async fn same_request_different_target_conflicts_while_identical_replays() {
    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue_named(
        &repository,
        "queue-target",
        "message-target",
        "thread-1",
        text_payload("same bytes"),
        Some(run_id("run-target-1")),
        300,
    )
    .await;
    let conflict = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-target"),
            message_id: message_id("message-target-2"),
            thread_id: thread_id("thread-1"),
            payload: text_payload("same bytes"),
            steer_run_id: Some(run_id("run-target-2")),
            accepted_at: UnixMillis::from_millis(301),
        })
        .await
        .expect_err("same request with a different target must conflict");
    assert!(
        matches!(
            conflict,
            RepositoryError::IdempotencyConflict { .. }
        ),
        "target change must conflict, got {conflict:?}"
    );
    let replay = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-target"),
            message_id: message_id("message-target-3"),
            thread_id: thread_id("thread-1"),
            payload: text_payload("same bytes"),
            steer_run_id: Some(run_id("run-target-1")),
            accepted_at: UnixMillis::from_millis(302),
        })
        .await
        .expect("identical retry must replay");
    assert_eq!(
        replay.receipt.disposition,
        ReceiptDisposition::Duplicate,
    );
    assert_eq!(replay.message_id, message_id("message-target"));
}

#[tokio::test]
async fn concurrent_duplicate_accept_has_one_winner_and_one_replay() {
    let temporary = TemporaryDatabase::new("queued-message-concurrent-accept");
    let (database, repository) = file_repository(temporary.path()).await;
    setup_thread(&repository).await;
    let first = repository.clone();
    let second = repository.clone();
    let input = || QueueMessageInput {
        request_id: request("queue-concurrent"),
        message_id: message_id("message-concurrent"),
        thread_id: thread_id("thread-1"),
        payload: text_payload("concurrent bytes"),
        steer_run_id: None,
        accepted_at: UnixMillis::from_millis(300),
    };
    let (first_result, second_result) =
        tokio::join!(first.queue_message(input()), second.queue_message(input()));
    let mut accepted = 0;
    let mut duplicate = 0;
    for result in [
        first_result.expect("first concurrent accept should settle"),
        second_result.expect("second concurrent accept should settle"),
    ] {
        match result.receipt.disposition {
            ReceiptDisposition::Accepted => accepted += 1,
            ReceiptDisposition::Duplicate => duplicate += 1,
        }
    }
    assert_eq!(
        (accepted, duplicate),
        (1, 1),
        "exactly one accept plus one replay"
    );
    drop(first);
    drop(second);
    drop(repository);
    database
        .close()
        .await
        .expect("concurrent test database should close before temp dir removal");
}

#[tokio::test]
async fn retry_after_selection_change_replays_stored_snapshot() {    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue_named(
        &repository,
        "queue-snapshot",
        "message-snapshot",
        "thread-1",
        text_payload("snapshot bytes"),
        None,
        300,
    )
    .await;
    let before = repository
        .read_receipt_engine_settings(&request("queue-snapshot"))
        .await
        .expect("snapshot should load")
        .expect("snapshot should exist");
    let revision = before.revision();
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request("queue-snapshot-engine-2"),
            thread_id: thread_id("thread-1"),
            precondition: EngineConfigUpdatePrecondition::Exact(revision),
            config: second_fixture_engine_config(),
            accepted_at: UnixMillis::from_millis(310),
        })
        .await
        .expect("selection change should persist");
    let replay = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-snapshot"),
            message_id: message_id("message-snapshot-2"),
            thread_id: thread_id("thread-1"),
            payload: text_payload("snapshot bytes"),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(320),
        })
        .await
        .expect("retry after selection change must replay");
    assert_eq!(
        replay.receipt.disposition,
        ReceiptDisposition::Duplicate,
    );
    assert_eq!(replay.message_id, message_id("message-snapshot"));
    let after = repository
        .read_receipt_engine_settings(&request("queue-snapshot"))
        .await
        .expect("snapshot should still load")
        .expect("snapshot should still exist");
    assert_eq!(
        after.config().selection().profile_id().as_str(),
        before.config().selection().profile_id().as_str(),
        "retry must replay the stored snapshot, never the current settings"
    );
}

fn launch_ids(tag: &str) -> (RunId, TurnId, ItemId, PatchId, PatchId) {
    (
        RunId::parse(format!("run-launch-{tag}")).expect("run id"),
        TurnId::parse(format!("turn-launch-{tag}")).expect("turn id"),
        ItemId::parse(format!("item-launch-{tag}")).expect("item id"),
        PatchId::parse(format!("patch-launch-{tag}-a")).expect("patch id"),
        PatchId::parse(format!("patch-launch-{tag}-b")).expect("patch id"),
    )
}

#[tokio::test]
async fn launch_uses_captured_snapshot_across_selection_change() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue_named(
        &repository,
        "queue-launch-a",
        "message-launch-a",
        "thread-1",
        text_payload("launch under A"),
        None,
        300,
    )
    .await;
    let revision = repository
        .read_thread_engine_settings(&thread_id("thread-1"))
        .await
        .expect("settings should read")
        .expect("settings should exist")
        .revision();
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request("queue-launch-config-b"),
            thread_id: thread_id("thread-1"),
            precondition: EngineConfigUpdatePrecondition::Exact(revision),
            config: second_fixture_engine_config(),
            accepted_at: UnixMillis::from_millis(310),
        })
        .await
        .expect("selection change should persist");
    let claimed = repository
        .claim_next_message_dispatch(claim(0x61, 400, 900))
        .await
        .expect("claim should work")
        .expect("dispatch should be claimed");
    assert_eq!(claimed.message_id, message_id("message-launch-a"));
    let captured = repository
        .read_receipt_engine_settings(&request("queue-launch-a"))
        .await
        .expect("snapshot should load")
        .expect("snapshot should exist");
    let (run_id, turn_id, item_id, first_patch, second_patch) = launch_ids("a");
    let outcome = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &item_id,
            first_patch_id: &first_patch,
            second_patch_id: &second_patch,
            operated_at: UnixMillis::from_millis(500),
            run_start_key: &RunStartKey::new([0x61; 32]),
            credentials: &RunLaunchCredentials::new([0x62; 32], [0x63; 32], [0x64; 32]),
            engine_settings: &captured,
        })
        .await
        .expect("launch with captured settings must succeed");
    assert!(
        matches!(
            outcome,
            artisan_database::LaunchClaimedRunOutcome::Started(_)
        ),
        "captured launch must start"
    );
    let run = entities::assistant_run::Entity::find_by_id(run_id.as_str())
        .one(&database)
        .await;
    let run = run
        .expect("launched run row should read")
        .expect("launched run row should exist");
    let receipt = entities::command_receipt::Entity::find_by_id("queue-launch-a")
        .one(&database)
        .await
        .expect("accept receipt should read")
        .expect("accept receipt should exist");
    assert_eq!(
        run.engine_run_config.as_ref().map(|bytes| bytes.as_slice()),
        receipt.engine_run_config.as_ref().map(|bytes| bytes.as_slice()),
        "launched run must store the captured snapshot, not current settings"
    );
}

#[tokio::test]
async fn launch_with_supplied_settings_mismatching_snapshot_fails() {
    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    queue_named(
        &repository,
        "queue-launch-b",
        "message-launch-b",
        "thread-1",
        text_payload("launch under B"),
        None,
        300,
    )
    .await;
    let revision = repository
        .read_thread_engine_settings(&thread_id("thread-1"))
        .await
        .expect("settings should read")
        .expect("settings should exist")
        .revision();
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request("queue-launch-config-b"),
            thread_id: thread_id("thread-1"),
            precondition: EngineConfigUpdatePrecondition::Exact(revision),
            config: second_fixture_engine_config(),
            accepted_at: UnixMillis::from_millis(310),
        })
        .await
        .expect("selection change should persist");
    let claimed = repository
        .claim_next_message_dispatch(claim(0x62, 400, 900))
        .await
        .expect("claim should work")
        .expect("dispatch should be claimed");
    let supplied = repository
        .read_thread_engine_settings(&thread_id("thread-1"))
        .await
        .expect("supplied settings should read")
        .expect("supplied settings should exist");
    let (run_id, turn_id, item_id, first_patch, second_patch) = launch_ids("b");
    let error = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &item_id,
            first_patch_id: &first_patch,
            second_patch_id: &second_patch,
            operated_at: UnixMillis::from_millis(500),
            run_start_key: &RunStartKey::new([0x65; 32]),
            credentials: &RunLaunchCredentials::new([0x66; 32], [0x67; 32], [0x68; 32]),
            engine_settings: &supplied,
        })
        .await
        .expect_err("supplied settings mismatching the snapshot must fail");
    assert!(
        matches!(
            error,
            artisan_database::RunLaunchError::SnapshotMismatch { .. }
        ),
        "mismatch must fail closed, got {error:?}"
    );
}

#[tokio::test]
async fn legacy_null_snapshot_rows_replay_and_read_as_absent() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    // A row accepted before snapshots existed: all snapshot columns
    // NULL, written directly to bypass the capturing admission path.
    entities::message::Entity::insert(entities::message::ActiveModel {
        message_id: Set("message-legacy".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(0),
        body: Set("legacy bytes".to_owned()),
        accepted_at_ms: Set(300),
    })
    .exec(&database)
    .await
    .expect("legacy message fixture should insert");
    entities::message_dispatch::Entity::insert(entities::message_dispatch::ActiveModel {
        message_id: Set("message-legacy".to_owned()),
        correlation_id: Set("queue-legacy".to_owned()),
        state: Set(DispatchState::Queued),
        attempt_count: Set(0),
        queued_at_ms: Set(300),
        available_at_ms: Set(300),
        lease_owner: Set(None),
        lease_expires_at_ms: Set(None),
        last_error: Set(None),
        steer_run_id: Set(None),
        updated_at_ms: Set(300),
    })
    .exec(&database)
    .await
    .expect("legacy dispatch fixture should insert");
    entities::command_receipt::Entity::insert(entities::command_receipt::ActiveModel {
        request_id: Set("queue-legacy".to_owned()),
        command_kind: Set(artisan_database::entities::CommandKind::QueueMessage),
        directory_id: Set(None),
        project_id: Set(None),
        thread_id: Set(Some("thread-1".to_owned())),
        title: Set(None),
        message_id: Set(Some("message-legacy".to_owned())),
        body: Set(Some("legacy bytes".to_owned())),
        accepted_at_ms: Set(300),
        engine_run_config_version: Set(None),
        engine_run_config: Set(None),
        engine_run_config_expected_revision: Set(None),
        engine_run_config_result_revision: Set(None),
    })
    .exec(&database)
    .await
    .expect("legacy receipt fixture should insert");
    let replay = repository
        .lookup_queue_message(
            &request("queue-legacy"),
            &thread_id("thread-1"),
            &text_payload("legacy bytes"),
            None,
        )
        .await
        .expect("legacy lookup should work")
        .expect("legacy receipt should replay");
    assert_eq!(
        replay.receipt.disposition,
        ReceiptDisposition::Duplicate,
    );
    assert_eq!(replay.message_id, message_id("message-legacy"));
    assert!(
        repository
            .read_receipt_engine_settings(&request("queue-legacy"))
            .await
            .expect("legacy snapshot read should work")
            .is_none(),
        "legacy rows without snapshots read as absent for the documented fallback"
    );
    let payload = repository
        .read_queue_message_dispatch_payload(&message_id("message-legacy"))
        .await
        .expect("legacy dispatch payload should load")
        .expect("legacy dispatch payload should exist");
    assert!(payload.steer_target.is_none());
}
