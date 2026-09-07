//! External coverage for reading persisted message-dispatch payloads.

use artisan_database::entities::{self, DispatchState};
use artisan_database::{
    AttachProjectInput, ClaimMessageDispatch, CreateThreadInput, DispatchLeaseOwner,
    QueueFirstMessageInput, Repository, RepositoryError, SqliteConfig, connect,
};
use artisan_domain::{
    DirectoryId, DisplayName, MessageBody, MessageId, ProjectId, ReceiptDisposition, RequestId,
    RootPath, ThreadId, ThreadTitle, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

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
    message_id_value: &str,
    thread_id_value: &str,
    body_value: &str,
) -> QueueFirstMessageInput {
    QueueFirstMessageInput {
        request_id: request(request_id),
        message_id: self::message_id(message_id_value),
        thread_id: self::thread_id(thread_id_value),
        body: self::body(body_value),
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

async fn create_thread(repository: &Repository, request_id: &str, thread_id_value: &str) {
    repository
        .create_thread(CreateThreadInput {
            request_id: request(request_id),
            thread_id: self::thread_id(thread_id_value),
            project_id: project_id("project-1"),
            title: ThreadTitle::parse(format!("Thread {thread_id_value}"))
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

async fn dispatch_row(
    database: &DatabaseConnection,
    message_id_value: &str,
) -> entities::MessageDispatch {
    entities::message_dispatch::Entity::find_by_id(message_id_value)
        .one(database)
        .await
        .expect("dispatch should query")
        .expect("dispatch should exist")
}

#[tokio::test]
async fn payload_read_returns_the_accepted_execution_fields() {
    let (_database, repository) = memory_repository().await;
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

    let payload = repository
        .read_message_dispatch_payload(&message_id("message-1"))
        .await
        .expect("payload read should work")
        .expect("queued message should expose its dispatch payload");
    assert_eq!(payload.message_id, message_id("message-1"));
    assert_eq!(payload.thread_id, thread_id("thread-1"));
    assert_eq!(payload.correlation_id, request("queue-request-1"));
    assert_eq!(payload.body, body("Hello"));
}

#[tokio::test]
async fn unknown_messages_have_no_dispatch_payload() {
    let (_database, repository) = memory_repository().await;

    let absent = repository
        .read_message_dispatch_payload(&message_id("missing-message"))
        .await
        .expect("empty-database read should work");
    assert!(absent.is_none());

    setup_thread(&repository).await;
    let still_absent = repository
        .read_message_dispatch_payload(&message_id("missing-message"))
        .await
        .expect("thread-only read should work");
    assert!(still_absent.is_none());
}

#[tokio::test]
async fn malformed_persisted_text_maps_to_corrupt_data_without_panicking() {
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

    let dispatch = dispatch_row(&database, "message-1").await;
    let mut corrupted = entities::message_dispatch::ActiveModel::from(dispatch);
    corrupted.correlation_id = Set("broken request".to_owned());
    corrupted
        .update(&database)
        .await
        .expect("dispatch fixture should update");
    assert!(matches!(
        repository
            .read_message_dispatch_payload(&message_id("message-1"))
            .await
            .expect_err("malformed correlation text must stay typed"),
        RepositoryError::CorruptData {
            table: "message_dispatches",
            field: "correlation_id",
            ..
        }
    ));

    let dispatch = dispatch_row(&database, "message-1").await;
    let mut restored = entities::message_dispatch::ActiveModel::from(dispatch);
    restored.correlation_id = Set("queue-request-1".to_owned());
    restored
        .update(&database)
        .await
        .expect("dispatch fixture should restore");
    let message = entities::message::Entity::find_by_id("message-1")
        .one(&database)
        .await
        .expect("message should query")
        .expect("message should exist");
    let mut corrupted_body = entities::message::ActiveModel::from(message);
    corrupted_body.body = Set(String::new());
    corrupted_body
        .update(&database)
        .await
        .expect("message fixture should update");
    assert!(matches!(
        repository
            .read_message_dispatch_payload(&message_id("message-1"))
            .await
            .expect_err("malformed body text must stay typed"),
        RepositoryError::CorruptData {
            table: "messages",
            field: "body",
            ..
        }
    ));
}

#[tokio::test]
async fn payload_read_survives_claims_without_mutating_lease_state() {
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
    let before_claim = repository
        .read_message_dispatch_payload(&message_id("message-1"))
        .await
        .expect("pre-claim read should work")
        .expect("queued dispatch should expose its payload");

    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([9_u8; 32]),
            claimed_at: UnixMillis::from_millis(1_000),
            lease_expires_at: UnixMillis::from_millis(90_000),
        })
        .await
        .expect("claim should work")
        .expect("queued dispatch should claim");
    assert_eq!(claimed.message_id, message_id("message-1"));
    let leased = dispatch_row(&database, "message-1").await;
    assert_eq!(leased.state, DispatchState::Leased);
    assert_eq!(leased.attempt_count, 1);

    let after_claim = repository
        .read_message_dispatch_payload(&message_id("message-1"))
        .await
        .expect("leased read should work")
        .expect("leased dispatch should keep exposing its payload");
    let after_reread = repository
        .read_message_dispatch_payload(&message_id("message-1"))
        .await
        .expect("leased re-read should work")
        .expect("leased dispatch should keep exposing its payload");

    assert_eq!(after_claim, before_claim);
    assert_eq!(after_reread, after_claim);
    assert_eq!(after_claim.message_id, message_id("message-1"));
    assert_eq!(after_claim.thread_id, thread_id("thread-1"));
    assert_eq!(after_claim.correlation_id, request("queue-request-1"));
    assert_eq!(after_claim.body, body("Hello"));

    let after_read = dispatch_row(&database, "message-1").await;
    assert_eq!(
        after_read, leased,
        "payload reads must not touch claim or lease state"
    );
    assert_eq!(after_read.state, DispatchState::Leased);
    assert_eq!(after_read.updated_at_ms, 1_000);
}
