//! SQLite coverage for the Forge-owned message outbox: delivery state of
//! undelivered messages, retry and recovery of failed messages by identity,
//! and the fingerprint that decides when subscribers receive a new outbox.

use artisan_database::entities::{self, DispatchState};
use artisan_database::{
    AttachProjectInput, ClaimMessageDispatch, CreateThreadInput, DispatchFailureReason,
    DispatchLeaseOwner, FailMessageDispatch, LaunchClaimedRun, QueueMessageInput, Repository,
    RunLaunchCredentials, RunStartKey, SetThreadEngineConfigInput, SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, DirectoryId, DisplayName, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineId, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FailedMessageListing, FailedMessageRetryOutcome,
    FailedMessageTarget, FilesystemAccess, FiniteMillis, ItemId, ListFailedMessages,
    ListQueuedMessages, MessageId, NetworkAccess, OpenCode2Selection, PatchId, PermissionId,
    ProjectId, QUEUED_MESSAGE_LIST_MAX, QueueMessagePayload, QueuedMessageListOrder,
    QueuedMessageListing, QueuedMessageState, RequestId, RootPath, RunId, ThreadId, ThreadTitle,
    TurnId, UnixMillis, WebSearchAccess, WithdrawQueuedMessage,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait, IntoActiveModel,
};

const FAILURE: &str = "engine profile unavailable";

async fn repository() -> (DatabaseConnection, Repository) {
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
    let repository = Repository::new(database.clone());
    repository
        .attach_project(AttachProjectInput {
            request_id: request("attach-project"),
            directory_id: DirectoryId::parse("directory-1").unwrap(),
            project_id: ProjectId::parse("project-1").unwrap(),
            root_path: RootPath::parse("C:/repos/artisan").unwrap(),
            display_name: DisplayName::parse("Artisan").unwrap(),
            attached_at: UnixMillis::from_millis(100),
        })
        .await
        .expect("project should attach");
    create_thread(&repository, "thread-1").await;
    (database, repository)
}

async fn create_thread(repository: &Repository, value: &str) {
    repository
        .create_thread(CreateThreadInput {
            request_id: request(&format!("create-{value}")),
            thread_id: thread(value),
            project_id: ProjectId::parse("project-1").unwrap(),
            title: ThreadTitle::parse("Thread").unwrap(),
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await
        .expect("thread should create");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request(&format!("config-{value}")),
            thread_id: thread(value),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: engine_config(),
            accepted_at: UnixMillis::from_millis(250),
        })
        .await
        .expect("thread configuration should persist");
}

fn engine_config() -> EngineRunConfig {
    let one = FiniteMillis::new(1).unwrap();
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).unwrap(),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).unwrap(),
        max_sse_line_bytes: ByteLimit::new(4_096).unwrap(),
        max_sse_event_bytes: ByteLimit::new(8_192).unwrap(),
        max_readiness_line_bytes: ByteLimit::new(4_096).unwrap(),
        max_header_count: CountLimit::new(8).unwrap(),
        max_http_buffer_bytes: ByteLimit::new(8_192).unwrap(),
        max_stderr_bytes: ByteLimit::new(4_096).unwrap(),
        observation_capacity: CountLimit::new(16).unwrap(),
    })
    .unwrap();
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-outbox").unwrap(),
        EngineAgentId::parse("agent-outbox").unwrap(),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-outbox").unwrap(),
            EngineModelId::parse("model-outbox").unwrap(),
            EngineRouteId::parse("route-outbox").unwrap(),
            None,
            permission,
        )),
        runtime,
    )
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).unwrap()
}

fn thread(value: &str) -> ThreadId {
    ThreadId::parse(value).unwrap()
}

fn message(value: &str) -> MessageId {
    MessageId::parse(value).unwrap()
}

fn target(value: &str) -> FailedMessageTarget {
    FailedMessageTarget {
        thread_id: thread("thread-1"),
        message_id: message(value),
        original_request_id: request(&format!("queue-{value}")),
    }
}

fn owner(byte: u8) -> DispatchLeaseOwner {
    DispatchLeaseOwner::new([byte; 32])
}

async fn queue(repository: &Repository, value: &str, accepted_at_ms: i64) {
    repository
        .queue_message(QueueMessageInput {
            request_id: request(&format!("queue-{value}")),
            message_id: message(value),
            thread_id: thread("thread-1"),
            payload: QueueMessagePayload::text_only(value).unwrap(),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(accepted_at_ms),
        })
        .await
        .expect("message should queue");
}

async fn claim(repository: &Repository, byte: u8) -> artisan_database::ClaimedMessageDispatch {
    claim_at(repository, byte, 400).await
}

async fn claim_at(
    repository: &Repository,
    byte: u8,
    claimed_at_ms: i64,
) -> artisan_database::ClaimedMessageDispatch {
    repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: owner(byte),
            claimed_at: UnixMillis::from_millis(claimed_at_ms),
            lease_expires_at: UnixMillis::from_millis(claimed_at_ms + 500),
        })
        .await
        .expect("claim should work")
        .expect("a dispatch should be claimed")
}

async fn fail(repository: &Repository, value: &str, byte: u8) {
    repository
        .fail_message_dispatch(FailMessageDispatch {
            message_id: message(value),
            owner: owner(byte),
            operated_at: UnixMillis::from_millis(500),
            reason: DispatchFailureReason::parse(FAILURE).unwrap(),
        })
        .await
        .expect("terminal failure should commit");
}

async fn launch(repository: &Repository, claimed: &artisan_database::ClaimedMessageDispatch) {
    let tag = claimed.message_id.as_str();
    let settings = repository
        .read_receipt_engine_settings(&request(&format!("queue-{tag}")))
        .await
        .unwrap()
        .unwrap();
    let outcome = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed,
            run_id: &RunId::parse(format!("run-{tag}")).unwrap(),
            turn_id: &TurnId::parse(format!("turn-{tag}")).unwrap(),
            item_id: &ItemId::parse(format!("item-{tag}")).unwrap(),
            first_patch_id: &PatchId::parse(format!("patch-{tag}-a")).unwrap(),
            second_patch_id: &PatchId::parse(format!("patch-{tag}-b")).unwrap(),
            operated_at: UnixMillis::from_millis(450),
            run_start_key: &RunStartKey::new([0x61; 32]),
            credentials: &RunLaunchCredentials::new([0x62; 32], [0x63; 32], [0x64; 32]),
            engine_settings: &settings,
        })
        .await
        .expect("launch should commit");
    assert!(matches!(
        outcome,
        artisan_database::LaunchClaimedRunOutcome::Started(_)
    ));
}

async fn queued(repository: &Repository) -> QueuedMessageListing {
    repository
        .read_queued_messages(
            ListQueuedMessages::new(
                thread("thread-1"),
                QueuedMessageListOrder::OldestFirst,
                QUEUED_MESSAGE_LIST_MAX,
            )
            .unwrap(),
        )
        .await
        .expect("queued listing should read")
}

async fn failed(repository: &Repository) -> FailedMessageListing {
    repository
        .read_failed_messages(ListFailedMessages::new(thread("thread-1"), 32).unwrap())
        .await
        .expect("failed listing should read")
}

#[tokio::test]
async fn queued_listing_reports_forge_delivery_state_and_engine() {
    let (_database, repository) = repository().await;
    queue(&repository, "first", 300).await;
    queue(&repository, "second", 301).await;
    let claimed = claim(&repository, 0x11).await;
    assert_eq!(claimed.message_id, message("first"));

    let listing = queued(&repository).await;
    let states: Vec<_> = listing
        .messages()
        .iter()
        .map(|row| (row.message_id.as_str().to_owned(), row.state, row.engine))
        .collect();
    assert_eq!(
        states,
        vec![
            (
                "first".to_owned(),
                QueuedMessageState::Dispatching,
                Some(EngineId::OpenCode2)
            ),
            (
                "second".to_owned(),
                QueuedMessageState::Queued,
                Some(EngineId::OpenCode2)
            ),
        ]
    );
}

#[tokio::test]
async fn launched_message_leaves_the_outbox_in_its_launch_commit() {
    let (_database, repository) = repository().await;
    queue(&repository, "first", 300).await;
    let claimed = claim(&repository, 0x11).await;
    let before = repository
        .message_outbox_fingerprint(&thread("thread-1"))
        .await
        .unwrap();
    launch(&repository, &claimed).await;
    assert!(queued(&repository).await.messages().is_empty());
    assert!(failed(&repository).await.messages().is_empty());
    assert_ne!(
        repository
            .message_outbox_fingerprint(&thread("thread-1"))
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn retry_requeues_the_stored_payload_once() {
    let (_database, repository) = repository().await;
    queue(&repository, "first", 300).await;
    claim(&repository, 0x11).await;
    fail(&repository, "first", 0x11).await;
    let failure = failed(&repository).await;
    let [row] = failure.messages() else {
        panic!("one failed row");
    };
    assert!(row.retryable, "a message that never launched is retryable");
    let before = repository
        .message_outbox_fingerprint(&thread("thread-1"))
        .await
        .unwrap();

    let outcome = repository
        .retry_failed_message(&target("first"), UnixMillis::from_millis(600))
        .await
        .unwrap();
    assert_eq!(outcome, FailedMessageRetryOutcome::Requeued);
    assert!(failed(&repository).await.messages().is_empty());
    let listing = queued(&repository).await;
    let [row] = listing.messages() else {
        panic!("the retried message is queued again");
    };
    assert_eq!(row.state, QueuedMessageState::Queued);
    assert_eq!(row.last_error, None);
    assert_ne!(
        repository
            .message_outbox_fingerprint(&thread("thread-1"))
            .await
            .unwrap(),
        before
    );

    let again = repository
        .retry_failed_message(&target("first"), UnixMillis::from_millis(601))
        .await
        .unwrap();
    assert_eq!(again, FailedMessageRetryOutcome::NotRetryable);
    let claimed = claim_at(&repository, 0x12, 700).await;
    assert_eq!(
        claimed.message_id,
        message("first"),
        "the dispatcher claims it"
    );
}

#[tokio::test]
async fn retry_refuses_a_mismatched_identity() {
    let (_database, repository) = repository().await;
    queue(&repository, "first", 300).await;
    claim(&repository, 0x11).await;
    fail(&repository, "first", 0x11).await;
    let mut wrong = target("first");
    wrong.original_request_id = request("queue-other");
    assert_eq!(
        repository
            .retry_failed_message(&wrong, UnixMillis::from_millis(600))
            .await
            .unwrap(),
        FailedMessageRetryOutcome::NotRetryable
    );
    assert_eq!(failed(&repository).await.messages().len(), 1);
}

#[tokio::test]
async fn a_failure_after_launch_is_offered_but_not_retryable() {
    let (database, repository) = repository().await;
    queue(&repository, "first", 300).await;
    let claimed = claim(&repository, 0x11).await;
    launch(&repository, &claimed).await;
    let mut row = entities::message_dispatch::Entity::find_by_id("first")
        .one(&database)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    row.state = Set(DispatchState::Failed);
    row.lease_owner = Set(None);
    row.lease_expires_at_ms = Set(None);
    row.last_error = Set(Some(FAILURE.to_owned()));
    row.update(&database).await.unwrap();

    let failure = failed(&repository).await;
    let [row] = failure.messages() else {
        panic!("one failed row");
    };
    assert!(!row.retryable, "its transcript item already exists");
    assert_eq!(
        repository
            .retry_failed_message(&target("first"), UnixMillis::from_millis(600))
            .await
            .unwrap(),
        FailedMessageRetryOutcome::NotRetryable
    );
}

#[tokio::test]
async fn a_recovered_failure_leaves_the_failed_listing() {
    let (_database, repository) = repository().await;
    queue(&repository, "first", 300).await;
    claim(&repository, 0x11).await;
    fail(&repository, "first", 0x11).await;
    create_thread(&repository, "thread-2").await;
    repository
        .record_failed_message_recovery(
            &request("recover-1"),
            &target("first"),
            &thread("thread-2"),
            UnixMillis::from_millis(700),
        )
        .await
        .unwrap();
    assert!(failed(&repository).await.messages().is_empty());
    let recovery = repository
        .failed_message_recovery(&message("first"))
        .await
        .unwrap()
        .expect("recovery is recorded");
    assert_eq!(recovery.new_thread_id, thread("thread-2"));
    assert_eq!(recovery.request_id, request("recover-1"));
    create_thread(&repository, "thread-3").await;
    repository
        .record_failed_message_recovery(
            &request("recover-2"),
            &target("first"),
            &thread("thread-3"),
            UnixMillis::from_millis(701),
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .failed_message_recovery(&message("first"))
            .await
            .unwrap()
            .unwrap()
            .new_thread_id,
        thread("thread-2"),
        "the first recovery stands"
    );
    assert_eq!(
        repository
            .retry_failed_message(&target("first"), UnixMillis::from_millis(702))
            .await
            .unwrap(),
        FailedMessageRetryOutcome::NotRetryable
    );
}

#[tokio::test]
async fn a_failure_superseded_by_a_later_delivered_message_is_not_offered() {
    let (_database, repository) = repository().await;
    queue(&repository, "first", 300).await;
    claim(&repository, 0x11).await;
    fail(&repository, "first", 0x11).await;
    assert_eq!(failed(&repository).await.messages().len(), 1);
    queue(&repository, "second", 301).await;
    let claimed = claim(&repository, 0x12).await;
    assert_eq!(claimed.message_id, message("second"));
    launch(&repository, &claimed).await;
    assert!(failed(&repository).await.messages().is_empty());
}

#[tokio::test]
async fn fingerprint_is_stable_until_the_outbox_moves() {
    let (_database, repository) = repository().await;
    let empty = repository
        .message_outbox_fingerprint(&thread("thread-1"))
        .await
        .unwrap();
    queue(&repository, "first", 300).await;
    let queued_print = repository
        .message_outbox_fingerprint(&thread("thread-1"))
        .await
        .unwrap();
    assert_ne!(queued_print, empty);
    assert_eq!(
        repository
            .message_outbox_fingerprint(&thread("thread-1"))
            .await
            .unwrap(),
        queued_print
    );
    repository
        .withdraw_queued_message(WithdrawQueuedMessage::new(
            thread("thread-1"),
            message("first"),
            request("queue-first"),
            request("withdraw-first"),
            UnixMillis::from_millis(310),
        ))
        .await
        .unwrap();
    assert_ne!(
        repository
            .message_outbox_fingerprint(&thread("thread-1"))
            .await
            .unwrap(),
        queued_print
    );
}
