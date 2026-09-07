use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{self, CommandKind, DispatchState};
use artisan_database::{
    AttachProjectInput, CreateThreadInput, QueueFirstMessageInput, Repository, RepositoryError,
    SqliteConfig, connect,
};
use artisan_domain::{
    DirectoryId, DisplayName, MessageBody, MessageId, ProjectId, ReceiptDisposition, RequestId,
    RootPath, ThreadId, ThreadTitle, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

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
        .expect("memory database should migrate");
    (database.clone(), Repository::new(database))
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).expect("test request id should be valid")
}

fn project_id(value: &str) -> ProjectId {
    ProjectId::parse(value).expect("test project id should be valid")
}

fn thread_id(value: &str) -> ThreadId {
    ThreadId::parse(value).expect("test thread id should be valid")
}

fn message_id(value: &str) -> MessageId {
    MessageId::parse(value).expect("test message id should be valid")
}

fn body(value: &str) -> MessageBody {
    MessageBody::parse(value).expect("test body should be valid")
}

fn queue_input(
    request_id: &str,
    message_id: &str,
    thread_id: &str,
    body: &str,
) -> QueueFirstMessageInput {
    QueueFirstMessageInput {
        request_id: request(request_id),
        message_id: self::message_id(message_id),
        thread_id: self::thread_id(thread_id),
        body: self::body(body),
        accepted_at: UnixMillis::from_millis(300),
    }
}

async fn attach_project(repository: &Repository) {
    repository
        .attach_project(AttachProjectInput {
            request_id: request("attach-request"),
            directory_id: DirectoryId::parse("directory-1").expect("directory id should parse"),
            project_id: project_id("project-1"),
            root_path: RootPath::parse("C:/repos/artisan").expect("root should parse"),
            display_name: DisplayName::parse("Artisan").expect("display name should parse"),
            attached_at: UnixMillis::from_millis(100),
        })
        .await
        .expect("project should attach");
}

async fn create_thread(repository: &Repository, request_id: &str, thread_id: &str) {
    repository
        .create_thread(CreateThreadInput {
            request_id: request(request_id),
            thread_id: self::thread_id(thread_id),
            project_id: project_id("project-1"),
            title: ThreadTitle::parse(format!("Thread {thread_id}"))
                .expect("thread title should parse"),
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await
        .expect("thread should create");
}

async fn setup_thread(repository: &Repository) {
    attach_project(repository).await;
    create_thread(repository, "create-request-1", "thread-1").await;
}

#[tokio::test]
async fn admission_returns_original_domain_receipt_and_classifies_conflicts() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;

    let accepted = repository
        .queue_first_message(queue_input(
            "queue-request-1",
            "message-1",
            "thread-1",
            "Hello",
        ))
        .await
        .expect("first message should queue");
    assert_eq!(accepted.receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(accepted.message.message_id, message_id("message-1"));
    assert_eq!(accepted.message.body, body("Hello"));

    let preflight = repository
        .lookup_queue_first_message(
            &request("queue-request-1"),
            &thread_id("thread-1"),
            &body("Hello"),
        )
        .await
        .expect("queue receipt lookup should work")
        .expect("queue receipt should exist");
    assert_eq!(preflight.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(preflight.message, accepted.message);
    assert_eq!(preflight.queued_at, accepted.queued_at);

    let duplicate = repository
        .queue_first_message(queue_input(
            "queue-request-1",
            "discarded-message-id",
            "thread-1",
            "Hello",
        ))
        .await
        .expect("retry should return the original message");
    assert_eq!(duplicate.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(duplicate.message, accepted.message);

    assert!(matches!(
        repository
            .lookup_queue_first_message(
                &request("queue-request-1"),
                &thread_id("thread-1"),
                &body("Changed"),
            )
            .await
            .expect_err("same request cannot change body"),
        RepositoryError::IdempotencyConflict { .. }
    ));
    assert!(matches!(
        repository
            .queue_first_message(queue_input(
                "queue-request-2",
                "message-2",
                "thread-1",
                "Hello",
            ))
            .await
            .expect_err("another request cannot replace ordinal zero"),
        RepositoryError::FirstMessageAlreadyExists { .. }
    ));
    assert!(matches!(
        repository
            .lookup_queue_first_message(
                &request("attach-request"),
                &thread_id("thread-1"),
                &body("Hello"),
            )
            .await
            .expect_err("request identity must be global across commands"),
        RepositoryError::IdempotencyConflict { .. }
    ));
    assert_eq!(
        entities::message::Entity::find()
            .all(&database)
            .await
            .expect("messages should query")
            .len(),
        1
    );
    assert_eq!(
        entities::message_dispatch::Entity::find()
            .all(&database)
            .await
            .expect("dispatches should query")
            .len(),
        1
    );
}

#[tokio::test]
async fn admission_rejects_missing_threads_bad_chronology_and_message_id_collisions() {
    let (_database, repository) = memory_repository().await;
    setup_thread(&repository).await;

    assert!(matches!(
        repository
            .queue_first_message(queue_input(
                "missing-request",
                "missing-message",
                "missing-thread",
                "Hello",
            ))
            .await
            .expect_err("missing thread should be typed"),
        RepositoryError::ThreadNotFound { .. }
    ));

    let mut early = queue_input("early-request", "early-message", "thread-1", "Hello");
    early.accepted_at = UnixMillis::from_millis(199);
    assert!(matches!(
        repository
            .queue_first_message(early)
            .await
            .expect_err("message cannot predate its thread"),
        RepositoryError::InvalidChronology { .. }
    ));

    repository
        .queue_first_message(queue_input(
            "queue-request-1",
            "message-1",
            "thread-1",
            "Hello",
        ))
        .await
        .expect("first message should queue");
    create_thread(&repository, "create-request-2", "thread-2").await;
    assert!(matches!(
        repository
            .queue_first_message(queue_input(
                "queue-request-2",
                "message-1",
                "thread-2",
                "Other",
            ))
            .await
            .expect_err("one message id cannot identify two messages"),
        RepositoryError::MessageConflict { .. }
    ));
}

#[tokio::test]
async fn admission_never_regresses_thread_recency() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    let thread = entities::thread::Entity::find_by_id("thread-1")
        .one(&database)
        .await
        .expect("thread query should work")
        .expect("thread should exist");
    let mut thread = entities::thread::ActiveModel::from(thread);
    thread.updated_at_ms = Set(400);
    thread
        .update(&database)
        .await
        .expect("thread fixture should update");

    repository
        .queue_first_message(queue_input(
            "queue-request",
            "message-1",
            "thread-1",
            "Hello",
        ))
        .await
        .expect("message should queue");

    let thread = entities::thread::Entity::find_by_id("thread-1")
        .one(&database)
        .await
        .expect("thread query should work")
        .expect("thread should exist");
    assert_eq!(thread.updated_at_ms, 400);
}

#[tokio::test]
async fn receipt_lookup_requires_a_consistent_durable_dispatch() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    repository
        .queue_first_message(queue_input(
            "queue-request-1",
            "message-1",
            "thread-1",
            "Hello",
        ))
        .await
        .expect("first message should queue");

    entities::message_dispatch::Entity::delete_by_id("message-1")
        .exec(&database)
        .await
        .expect("dispatch fixture should delete");
    assert!(matches!(
        repository
            .lookup_queue_first_message(
                &request("queue-request-1"),
                &thread_id("thread-1"),
                &body("Hello"),
            )
            .await
            .expect_err("receipt without outbox must not claim recoverability"),
        RepositoryError::Invariant { .. }
    ));

    create_thread(&repository, "create-request-2", "thread-2").await;
    repository
        .queue_first_message(queue_input(
            "queue-request-2",
            "message-2",
            "thread-2",
            "World",
        ))
        .await
        .expect("second message should queue");
    let dispatch = entities::message_dispatch::Entity::find_by_id("message-2")
        .one(&database)
        .await
        .expect("dispatch query should work")
        .expect("dispatch should exist");
    let mut dispatch = entities::message_dispatch::ActiveModel::from(dispatch);
    dispatch.correlation_id = Set("wrong-request".to_owned());
    dispatch
        .update(&database)
        .await
        .expect("dispatch fixture should update");
    assert!(matches!(
        repository
            .lookup_queue_first_message(
                &request("queue-request-2"),
                &thread_id("thread-2"),
                &body("World"),
            )
            .await
            .expect_err("mismatched correlation must not claim recoverability"),
        RepositoryError::Invariant { .. }
    ));
}

#[tokio::test]
async fn upgraded_dispatch_collision_rolls_back_every_provisional_row() {
    let (database, repository) = memory_repository().await;
    setup_thread(&repository).await;
    create_thread(&repository, "create-request-2", "thread-2").await;

    entities::message::ActiveModel {
        message_id: Set("old-message".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(0),
        body: Set("Old".to_owned()),
        accepted_at_ms: Set(250),
    }
    .insert(&database)
    .await
    .expect("upgraded message fixture should insert");
    entities::message_dispatch::ActiveModel {
        message_id: Set("old-message".to_owned()),
        correlation_id: Set("orphan-request".to_owned()),
        state: Set(DispatchState::Queued),
        attempt_count: Set(0),
        queued_at_ms: Set(250),
        available_at_ms: Set(250),
        lease_owner: Set(None),
        lease_expires_at_ms: Set(None),
        last_error: Set(None),
        updated_at_ms: Set(250),
    }
    .insert(&database)
    .await
    .expect("upgraded dispatch fixture should insert");

    assert!(matches!(
        repository
            .queue_first_message(queue_input(
                "orphan-request",
                "new-message",
                "thread-2",
                "New",
            ))
            .await
            .expect_err("orphan correlation must not be silently reused"),
        RepositoryError::IdempotencyConflict { .. }
    ));
    assert!(
        entities::message::Entity::find_by_id("new-message")
            .one(&database)
            .await
            .expect("message query should work")
            .is_none(),
        "provisional message must roll back"
    );
    assert!(
        entities::command_receipt::Entity::find_by_id("orphan-request")
            .one(&database)
            .await
            .expect("receipt query should work")
            .is_none(),
        "rejected command must not leave a receipt"
    );
}

#[tokio::test]
async fn concurrent_retry_has_one_accept_and_reopens_receipt_message_and_outbox() {
    let temporary = TemporaryDatabase::new("queue-contention");
    let database = connect(
        SqliteConfig::file(temporary.path())
            .min_connections(1)
            .max_connections(4)
            .sqlx_logging(false),
    )
    .await
    .expect("file database should open");
    migrate_to_current(&database)
        .await
        .expect("file database should migrate");
    let repository = Repository::new(database.clone());
    setup_thread(&repository).await;

    let first_repository = repository.clone();
    let second_repository = repository.clone();
    let first = tokio::spawn(async move {
        first_repository
            .queue_first_message(queue_input(
                "queue-request",
                "message-first",
                "thread-1",
                "Hello",
            ))
            .await
    });
    let second = tokio::spawn(async move {
        second_repository
            .queue_first_message(queue_input(
                "queue-request",
                "message-second",
                "thread-1",
                "Hello",
            ))
            .await
    });
    let results = [
        first
            .await
            .expect("first task should finish")
            .expect("first queue should work"),
        second
            .await
            .expect("second task should finish")
            .expect("second queue should work"),
    ];
    assert_eq!(
        results
            .iter()
            .filter(|result| result.receipt.disposition == ReceiptDisposition::Accepted)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result.receipt.disposition == ReceiptDisposition::Duplicate)
            .count(),
        1
    );
    assert_eq!(results[0].message, results[1].message);
    database.close().await.expect("database should close");

    let reopened = connect(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("database should reopen");
    migrate_to_current(&reopened)
        .await
        .expect("reopen migration should be idempotent");
    let repository = Repository::new(reopened.clone());
    let duplicate = repository
        .lookup_queue_first_message(
            &request("queue-request"),
            &thread_id("thread-1"),
            &body("Hello"),
        )
        .await
        .expect("reopened receipt lookup should work")
        .expect("receipt should survive reopen");
    assert_eq!(duplicate.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(duplicate.message, results[0].message);

    let receipt = entities::command_receipt::Entity::find_by_id("queue-request")
        .one(&reopened)
        .await
        .expect("receipt should query")
        .expect("receipt should survive");
    assert_eq!(receipt.command_kind, CommandKind::QueueFirstMessage);
    let dispatch =
        entities::message_dispatch::Entity::find_by_id(duplicate.message.message_id.as_str())
            .one(&reopened)
            .await
            .expect("dispatch should query")
            .expect("dispatch should survive");
    assert_eq!(dispatch.state, DispatchState::Queued);
    assert_eq!(dispatch.correlation_id, "queue-request");
    assert_eq!(dispatch.queued_at_ms, duplicate.queued_at.as_millis());
    reopened.close().await.expect("database should close");
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
        let path = std::env::temp_dir().join(format!(
            "artisan-database-{label}-{}-{timestamp}-{sequence}.sqlite3",
            std::process::id()
        ));
        Self { path }
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
                    "failed to remove temporary database file {}: {error}",
                    candidate.display()
                );
            }
        }
    }
}
