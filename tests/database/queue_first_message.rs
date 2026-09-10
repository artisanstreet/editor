use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{self, CommandKind, DispatchState};
use artisan_database::{
    AttachProjectInput, CreateThreadInput, QueueFirstMessageInput, QueueMessageInput, Repository,
    RepositoryError, SetThreadEngineConfigInput, SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, AuthoredText, ByteLimit, CountLimit, DirectoryId, DisplayName, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, ImageAttachment, MessageBody, MessageId,
    NetworkAccess, OpenCode2Selection, PermissionId, ProjectId, QueueMessagePayload,
    ReceiptDisposition, RequestId, RootPath, ThreadId, ThreadTitle, UnixMillis, WebSearchAccess,
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
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request(&format!("{request_id}-engine")),
            thread_id: self::thread_id(thread_id),
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
        PermissionId::parse("permission-first").expect("permission id is valid"),
        EngineAgentId::parse("agent-first").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-first").expect("profile id is valid"),
            EngineModelId::parse("model-first").expect("model id is valid"),
            EngineRouteId::parse("route-first").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
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
        steer_run_id: Set(None),
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

fn image_only_payload(seed: u8) -> QueueMessagePayload {
    let mut attachments = Vec::new();
    for index in 0_u8..3 {
        attachments.push(
            ImageAttachment::new(
                "image/png",
                vec![seed.saturating_add(index); 4 * 1024 * 1024],
                format!("capture-{index}.png"),
            )
            .expect("fixture image should satisfy the per-image bound"),
        );
    }
    QueueMessagePayload::new(None, attachments).expect("three images fit the aggregate bound")
}

fn payload_with_text(text: Option<&str>, name: &str) -> QueueMessagePayload {
    QueueMessagePayload::new(
        text.map(|value| AuthoredText::parse(value).expect("test text should parse")),
        vec![
            ImageAttachment::new("image/png", vec![7, 8, 9], name)
                .expect("test image should satisfy the image bound"),
        ],
    )
    .expect("text/image payload should be valid")
}

#[tokio::test]
async fn general_message_preserves_order_replay_and_owned_image_reads_after_reopen() {
    let temporary = TemporaryDatabase::new("queue-message-media");
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

    let payload = image_only_payload(0);
    let accepted = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-media-request"),
            message_id: message_id("queue-media-message"),
            thread_id: thread_id("thread-1"),
            payload: payload.clone(),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(301),
        })
        .await
        .expect("image-only general message should queue");
    assert_eq!(accepted.receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(accepted.payload, payload);

    let dispatch = repository
        .read_queue_message_dispatch_payload(&accepted.message_id)
        .await
        .expect("dispatch payload should load")
        .expect("dispatch payload should exist");
    assert_eq!(dispatch.payload, payload);
    assert_eq!(dispatch.payload.attachments()[0].name(), "capture-0.png");
    assert_eq!(dispatch.payload.attachments()[2].bytes()[0], 2);

    let second_payload = image_only_payload(10);
    let second = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-media-request-2"),
            message_id: message_id("queue-media-message-2"),
            thread_id: thread_id("thread-1"),
            payload: second_payload.clone(),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(302),
        })
        .await
        .expect("a subsequent 12 MiB image message should queue");
    assert_eq!(second.payload, second_payload);

    let duplicate = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-media-request"),
            message_id: message_id("discarded-media-message"),
            thread_id: thread_id("thread-1"),
            payload: payload.clone(),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(302),
        })
        .await
        .expect("exact general-message retry should replay");
    assert_eq!(duplicate.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(duplicate.message_id, accepted.message_id);

    let changed = QueueMessagePayload::new(
        Some(AuthoredText::parse("changed").expect("changed text should parse")),
        Vec::new(),
    )
    .expect("changed text should be a valid payload");
    assert!(matches!(
        repository
            .queue_message(QueueMessageInput {
                request_id: request("queue-media-request"),
                message_id: message_id("conflicting-media-message"),
                thread_id: thread_id("thread-1"),
                payload: changed,
                steer_run_id: None,
                accepted_at: UnixMillis::from_millis(303),
            })
            .await
            .expect_err("same request id cannot change payload"),
        RepositoryError::IdempotencyConflict { .. }
    ));

    let first_image = repository
        .read_message_image(
            &thread_id("thread-1"),
            &accepted.message_id,
            0,
        )
        .await
        .expect("owned image read should work")
        .expect("first image should exist");
    assert_eq!(first_image.reference.thread_id, thread_id("thread-1"));
    assert_eq!(first_image.reference.message_id, accepted.message_id);
    assert_eq!(first_image.reference.index, 0);
    assert_eq!(first_image.reference.name, "capture-0.png");
    assert_eq!(first_image.bytes.len(), 4 * 1024 * 1024);
    assert_eq!(first_image.bytes[0], 0);
    assert_ne!(first_image.reference.digest, [0; 32]);

    assert!(repository
        .read_message_image(
            &thread_id("thread-2"),
            &accepted.message_id,
            0,
        )
        .await
        .expect("wrong-thread image read should be handled")
        .is_none());

    database.close().await.expect("database should close");
    let reopened = connect(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("database should reopen");
    migrate_to_current(&reopened)
        .await
        .expect("reopen migration should be idempotent");
    let reopened_repository = Repository::new(reopened.clone());
    let replay = reopened_repository
        .lookup_queue_message(
            &request("queue-media-request"),
            &thread_id("thread-1"),
            &payload,
            None,
        )
        .await
        .expect("reopened receipt lookup should work")
        .expect("reopened receipt should exist");
    assert_eq!(replay.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(replay.message_id, accepted.message_id);
    let reopened_dispatch = reopened_repository
        .read_queue_message_dispatch_payload(&accepted.message_id)
        .await
        .expect("reopened dispatch payload should load")
        .expect("reopened dispatch payload should exist");
    assert_eq!(reopened_dispatch.payload, payload);
    let third_image = reopened_repository
        .read_message_image(
            &thread_id("thread-1"),
            &accepted.message_id,
            2,
        )
        .await
        .expect("reopened image read should work")
        .expect("third image should exist");
    assert_eq!(third_image.reference.index, 2);
    assert_eq!(third_image.reference.name, "capture-2.png");
    assert_eq!(third_image.bytes[0], 2);
    let second_history_image = reopened_repository
        .read_message_image(
            &thread_id("thread-1"),
            &second.message_id,
            1,
        )
        .await
        .expect("second history image read should work")
        .expect("second history image should exist");
    assert_eq!(second_history_image.reference.message_id, second.message_id);
    assert_eq!(second_history_image.bytes[0], 11);
    reopened.close().await.expect("reopened database should close");
}

#[tokio::test]
async fn general_message_reloads_absent_and_empty_text_exactly_after_reopen() {
    let temporary = TemporaryDatabase::new("queue-message-text-presence");
    let database = connect(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("file database should open");
    migrate_to_current(&database)
        .await
        .expect("file database should migrate");
    let repository = Repository::new(database.clone());
    setup_thread(&repository).await;

    let absent = payload_with_text(None, "absent.png");
    let present_empty = payload_with_text(Some(""), "present-empty.png");
    let absent_result = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-text-absent"),
            message_id: message_id("queue-text-absent-message"),
            thread_id: thread_id("thread-1"),
            payload: absent.clone(),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(301),
        })
        .await
        .expect("absent-text message should queue");
    let present_empty_result = repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-text-empty"),
            message_id: message_id("queue-text-empty-message"),
            thread_id: thread_id("thread-1"),
            payload: present_empty.clone(),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(302),
        })
        .await
        .expect("present-empty-text message should queue");

    database.close().await.expect("database should close");
    let reopened = connect(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("database should reopen");
    migrate_to_current(&reopened)
        .await
        .expect("reopen migration should be idempotent");
    let reopened_repository = Repository::new(reopened.clone());

    let absent_dispatch = reopened_repository
        .read_queue_message_dispatch_payload(&absent_result.message_id)
        .await
        .expect("absent-text dispatch should reload")
        .expect("absent-text dispatch should exist");
    assert!(absent_dispatch.payload.text().is_none());
    let present_empty_dispatch = reopened_repository
        .read_queue_message_dispatch_payload(&present_empty_result.message_id)
        .await
        .expect("present-empty-text dispatch should reload")
        .expect("present-empty-text dispatch should exist");
    assert_eq!(
        present_empty_dispatch
            .payload
            .text()
            .expect("present empty text should remain present")
            .as_str(),
        ""
    );

    let replay = reopened_repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-text-empty"),
            message_id: message_id("discarded-text-empty-replay"),
            thread_id: thread_id("thread-1"),
            payload: present_empty.clone(),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(303),
        })
        .await
        .expect("exact present-empty retry should replay");
    assert_eq!(replay.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(replay.message_id, present_empty_result.message_id);
    assert!(matches!(
        reopened_repository
            .queue_message(QueueMessageInput {
                request_id: request("queue-text-empty"),
                message_id: message_id("conflicting-text-empty-replay"),
                thread_id: thread_id("thread-1"),
                payload: absent,
                steer_run_id: None,
                accepted_at: UnixMillis::from_millis(304),
            })
            .await
            .expect_err("same request with absent text must conflict"),
        RepositoryError::IdempotencyConflict { .. }
    ));

    reopened.close().await.expect("reopened database should close");
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
