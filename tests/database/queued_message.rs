//! Focused SQLite coverage for bounded queued-message edit/discard storage.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{self, DispatchState};
use artisan_database::{
    AttachProjectInput, ClaimMessageDispatch, CreateThreadInput, DispatchLeaseOwner,
    QueueMessageInput, QueuedMessageRepositoryError, Repository, SqliteConfig, connect,
};
use artisan_domain::{
    AuthoredText, DirectoryId, DisplayName, ImageAttachment, ListQueuedMessages, MessageId,
    ProjectId, QUEUED_MESSAGE_LIST_MAX, QueueMessagePayload, QueuedMessageListError,
    QueuedMessageListOrder, QueuedMessageWithdrawalOutcome, ReceiptDisposition, RequestId,
    RootPath, ThreadId, ThreadTitle, UnixMillis, WithdrawQueuedMessage,
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
            accepted_at: UnixMillis::from_millis(accepted_at_ms),
        })
        .await
        .expect("general message fixture should queue");
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
