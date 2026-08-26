//! External tests for the Forge application request-handler seam.
//!
//! Every test drives [`RequestHandler::respond`] directly with a decoded
//! protocol request and a correlated domain request id against real migrated
//! storage, proving repository-backed mapping and typed failure behavior
//! without any network stack.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use artisan_backend::{ForgeStorage, RequestHandler};
use artisan_database::{
    AttachProjectInput, CreateThreadInput, QueueFirstMessageInput, SqliteConfig,
};
use artisan_domain::{
    Command, ConversationQuery, ConversationQueryBounds, ConversationRequest,
    ConversationSubscribe, DirectoryId, DisplayName, ListAttachedProjects, ListDirectories,
    ListProjectThreads, MessageBody, ProjectId, Query, QueryTurnCount, ReceiptDisposition,
    RequestId, RootPath, ThreadId, ThreadTitle, UnixMillis,
};
use artisan_protocol::{
    ClientRequest, ErrorCode, FirstMessageReceipt, ProtocolFailure, ResponsePayload, ServerResponse,
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
            "artisan-forge-request-handler-{label}-{}-{sequence}",
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

fn attach_input(request_id: &str, directory_id: &str, project_id: &str) -> AttachProjectInput {
    AttachProjectInput {
        request_id: request(request_id),
        directory_id: DirectoryId::parse(directory_id).expect("test directory id should be valid"),
        project_id: ProjectId::parse(project_id).expect("test project id should be valid"),
        root_path: RootPath::parse(format!("C:/repos/{project_id}"))
            .expect("test root path should be valid"),
        display_name: DisplayName::parse("Project").expect("test display name should be valid"),
        attached_at: UnixMillis::from_millis(100),
    }
}

fn create_input(request_id: &str, project_id: &str, thread_id: &str) -> CreateThreadInput {
    CreateThreadInput {
        request_id: request(request_id),
        thread_id: ThreadId::parse(thread_id).expect("test thread id should be valid"),
        project_id: ProjectId::parse(project_id).expect("test project id should be valid"),
        title: ThreadTitle::parse("First thread").expect("test title should be valid"),
        created_at: UnixMillis::from_millis(200),
        updated_at: UnixMillis::from_millis(200),
    }
}

fn message_input(
    request_id: &str,
    thread_id: &str,
    message_id: &str,
    body: &str,
) -> QueueFirstMessageInput {
    QueueFirstMessageInput {
        request_id: request(request_id),
        message_id: artisan_domain::MessageId::parse(message_id)
            .expect("test message id should be valid"),
        thread_id: ThreadId::parse(thread_id).expect("test thread id should be valid"),
        body: MessageBody::parse(body).expect("test body should be valid"),
        accepted_at: UnixMillis::from_millis(300),
    }
}

fn failure_of(result: Result<ServerResponse, ProtocolFailure>) -> ProtocolFailure {
    match result {
        Ok(response) => {
            let payload = response.payload;
            panic!("unexpected success payload: {payload:?}")
        }
        Err(failure) => failure,
    }
}

#[tokio::test]
async fn list_attached_projects_maps_durable_catalog_into_correlated_response() {
    let (_temporary, storage) = opened_storage("catalog").await;
    storage
        .repository()
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    let handler = RequestHandler::new(storage.repository().clone());

    let response = handler
        .respond(
            &request("frame-catalog"),
            &ClientRequest::Query(Query::ListAttachedProjects(ListAttachedProjects)),
        )
        .await
        .expect("repository-backed listing should succeed");

    storage.close().await.expect("storage should close");

    assert_eq!(response.request_id, request("frame-catalog"));
    let ResponsePayload::ProjectListing(listing) = response.payload else {
        panic!("expected a project listing payload");
    };
    assert_eq!(listing.projects().len(), 1);
    assert_eq!(listing.projects()[0].project_id.as_str(), "project-1");
    assert_eq!(
        listing.projects()[0].root_path.as_str(),
        "C:/repos/project-1"
    );
}

#[tokio::test]
async fn list_project_threads_maps_project_scoped_listing() {
    let (_temporary, storage) = opened_storage("threads").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input("request-thread-1", "project-1", "thread-1"))
        .await
        .expect("seed create should persist");
    let handler = RequestHandler::new(repository.clone());

    let response = handler
        .respond(
            &request("frame-threads"),
            &ClientRequest::Query(Query::ListProjectThreads(ListProjectThreads {
                project_id: ProjectId::parse("project-1").expect("valid project id"),
            })),
        )
        .await
        .expect("repository-backed listing should succeed");

    storage.close().await.expect("storage should close");

    assert_eq!(response.request_id, request("frame-threads"));
    let ResponsePayload::ThreadListing(listing) = response.payload else {
        panic!("expected a thread listing payload");
    };
    assert_eq!(listing.threads().len(), 1);
    assert_eq!(listing.threads()[0].thread_id.as_str(), "thread-1");
    assert_eq!(listing.threads()[0].title.as_str(), "First thread");
}

#[tokio::test]
async fn list_project_threads_for_unattached_project_fails_project_unknown() {
    let (_temporary, storage) = opened_storage("unknown-project").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let failure = failure_of(
        handler
            .respond(
                &request("frame-missing"),
                &ClientRequest::Query(Query::ListProjectThreads(ListProjectThreads {
                    project_id: ProjectId::parse("project-missing").expect("valid project id"),
                })),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::ProjectUnknown);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("frame-missing")));
}

#[tokio::test]
async fn attach_replay_answers_duplicate_from_the_durable_receipt() {
    let (_temporary, storage) = opened_storage("attach-replay").await;
    storage
        .repository()
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    let handler = RequestHandler::new(storage.repository().clone());

    let response = handler
        .respond(
            &request("request-project-1"),
            &ClientRequest::Command(Command::AttachProject(artisan_domain::AttachProject {
                request_id: request("request-project-1"),
                directory_id: DirectoryId::parse("directory-project-1")
                    .expect("valid directory id"),
            })),
        )
        .await
        .expect("replayed receipt should answer the mutation");

    storage.close().await.expect("storage should close");

    let ResponsePayload::AttachedProject {
        project,
        disposition,
    } = response.payload
    else {
        panic!("expected an attached-project payload");
    };
    assert_eq!(disposition, ReceiptDisposition::Duplicate);
    assert_eq!(project.project_id.as_str(), "project-1");
}

#[tokio::test]
async fn fresh_attach_fails_directory_unknown_without_a_directory_registry() {
    let (_temporary, storage) = opened_storage("fresh-attach").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let failure = failure_of(
        handler
            .respond(
                &request("request-fresh"),
                &ClientRequest::Command(Command::AttachProject(artisan_domain::AttachProject {
                    request_id: request("request-fresh"),
                    directory_id: DirectoryId::parse("directory-unregistered")
                        .expect("valid directory id"),
                })),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::DirectoryUnknown);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-fresh")));
}

#[tokio::test]
async fn fresh_identity_backed_mutations_fail_until_minting_exists() {
    let (_temporary, storage) = opened_storage("fresh-effects").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let creation_failure = failure_of(
        handler
            .respond(
                &request("request-create"),
                &ClientRequest::Command(Command::CreateThread(artisan_domain::CreateThread {
                    request_id: request("request-create"),
                    project_id: ProjectId::parse("project-1").expect("valid project id"),
                    title: ThreadTitle::parse("New thread").expect("valid title"),
                })),
            )
            .await,
    );
    let queue_failure = failure_of(
        handler
            .respond(
                &request("request-message"),
                &ClientRequest::Command(Command::QueueFirstMessage(
                    artisan_domain::QueueFirstMessage {
                        request_id: request("request-message"),
                        thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
                        body: MessageBody::parse("first").expect("valid body"),
                    },
                )),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    for failure in [creation_failure, queue_failure] {
        assert_eq!(failure.code, ErrorCode::Internal);
        assert!(!failure.retryable);
        assert!(
            failure
                .detail
                .as_str()
                .contains("not backed by a Forge capability")
        );
    }
}

#[tokio::test]
async fn queue_replay_answers_the_durable_receipt_with_message_identity() {
    let (_temporary, storage) = opened_storage("queue-replay").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input("request-thread-1", "project-1", "thread-1"))
        .await
        .expect("seed create should persist");
    repository
        .queue_first_message(message_input(
            "request-message-1",
            "thread-1",
            "message-1",
            "first body",
        ))
        .await
        .expect("seed queue should persist");
    let handler = RequestHandler::new(repository.clone());

    let response = handler
        .respond(
            &request("request-message-1"),
            &ClientRequest::Command(Command::QueueFirstMessage(
                artisan_domain::QueueFirstMessage {
                    request_id: request("request-message-1"),
                    thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
                    body: MessageBody::parse("first body").expect("valid body"),
                },
            )),
        )
        .await
        .expect("replayed receipt should answer the mutation");

    storage.close().await.expect("storage should close");

    assert_eq!(response.request_id, request("request-message-1"));
    let ResponsePayload::FirstMessageQueued(receipt) = response.payload else {
        panic!("expected a first-message receipt payload");
    };
    assert_eq!(
        receipt,
        FirstMessageReceipt {
            request_id: request("request-message-1"),
            message_id: artisan_domain::MessageId::parse("message-1").expect("valid message id"),
            thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
            disposition: ReceiptDisposition::Duplicate,
        }
    );
}

#[tokio::test]
async fn mismatched_command_correlation_fails_as_invalid_input() {
    let (_temporary, storage) = opened_storage("correlation").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let failure = failure_of(
        handler
            .respond(
                &request("frame-other"),
                &ClientRequest::Command(Command::CreateThread(artisan_domain::CreateThread {
                    request_id: request("request-command"),
                    project_id: ProjectId::parse("project-1").expect("valid project id"),
                    title: ThreadTitle::parse("New thread").expect("valid title"),
                })),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::InvalidInput);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("frame-other")));
}

#[tokio::test]
async fn changed_body_replay_fails_idempotency_conflict_preserving_the_original_outcome() {
    let (_temporary, storage) = opened_storage("idempotency-body").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input("request-thread-1", "project-1", "thread-1"))
        .await
        .expect("seed create should persist");
    repository
        .queue_first_message(message_input(
            "request-message-1",
            "thread-1",
            "message-1",
            "original first body",
        ))
        .await
        .expect("seed queue should persist");
    let handler = RequestHandler::new(repository.clone());

    let failure = failure_of(
        handler
            .respond(
                &request("request-message-1"),
                &ClientRequest::Command(Command::QueueFirstMessage(
                    artisan_domain::QueueFirstMessage {
                        request_id: request("request-message-1"),
                        thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
                        body: MessageBody::parse("conflicting second body").expect("valid body"),
                    },
                )),
            )
            .await,
    );
    let replay = handler
        .respond(
            &request("request-message-1"),
            &ClientRequest::Command(Command::QueueFirstMessage(
                artisan_domain::QueueFirstMessage {
                    request_id: request("request-message-1"),
                    thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
                    body: MessageBody::parse("original first body").expect("valid body"),
                },
            )),
        )
        .await
        .expect("the original outcome should still replay as its durable duplicate");

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::IdempotencyConflict);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-message-1")));
    assert!(!failure.detail.as_str().contains("original first body"));
    assert!(!failure.detail.as_str().contains("conflicting second body"));
    let ResponsePayload::FirstMessageQueued(receipt) = replay.payload else {
        panic!("expected a first-message receipt payload");
    };
    assert_eq!(
        receipt,
        FirstMessageReceipt {
            request_id: request("request-message-1"),
            message_id: artisan_domain::MessageId::parse("message-1").expect("valid message id"),
            thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
            disposition: ReceiptDisposition::Duplicate,
        }
    );
}

#[tokio::test]
async fn reused_request_identity_across_command_kinds_fails_idempotency_conflict() {
    let (_temporary, storage) = opened_storage("idempotency-kind").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input("request-shared", "project-1", "thread-1"))
        .await
        .expect("seed create should persist");
    let handler = RequestHandler::new(repository.clone());

    let failure = failure_of(
        handler
            .respond(
                &request("request-shared"),
                &ClientRequest::Command(Command::QueueFirstMessage(
                    artisan_domain::QueueFirstMessage {
                        request_id: request("request-shared"),
                        thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
                        body: MessageBody::parse("first body").expect("valid body"),
                    },
                )),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::IdempotencyConflict);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-shared")));
    assert!(!failure.detail.as_str().contains("First thread"));
    assert!(!failure.detail.as_str().contains("first body"));
}

#[tokio::test]
async fn changed_attach_directory_fails_idempotency_conflict() {
    let (_temporary, storage) = opened_storage("idempotency-directory").await;
    storage
        .repository()
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    let handler = RequestHandler::new(storage.repository().clone());

    let failure = failure_of(
        handler
            .respond(
                &request("request-project-1"),
                &ClientRequest::Command(Command::AttachProject(artisan_domain::AttachProject {
                    request_id: request("request-project-1"),
                    directory_id: DirectoryId::parse("directory-project-2")
                        .expect("valid directory id"),
                })),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::IdempotencyConflict);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-project-1")));
    assert!(!failure.detail.as_str().contains("directory-project-1"));
    assert!(!failure.detail.as_str().contains("directory-project-2"));
}

#[tokio::test]
async fn changed_create_title_fails_idempotency_conflict() {
    let (_temporary, storage) = opened_storage("idempotency-title").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input("request-thread-1", "project-1", "thread-1"))
        .await
        .expect("seed create should persist");
    let handler = RequestHandler::new(repository.clone());

    let failure = failure_of(
        handler
            .respond(
                &request("request-thread-1"),
                &ClientRequest::Command(Command::CreateThread(artisan_domain::CreateThread {
                    request_id: request("request-thread-1"),
                    project_id: ProjectId::parse("project-1").expect("valid project id"),
                    title: ThreadTitle::parse("Other thread").expect("valid title"),
                })),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::IdempotencyConflict);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-thread-1")));
    assert!(!failure.detail.as_str().contains("First thread"));
    assert!(!failure.detail.as_str().contains("Other thread"));
}

#[tokio::test]
async fn conversation_requests_fail_until_projection_exists() {
    let (_temporary, storage) = opened_storage("conversation").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let query_failure = failure_of(
        handler
            .respond(
                &request("frame-query"),
                &ClientRequest::Conversation(ConversationRequest::Query(ConversationQuery {
                    thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
                    bounds: ConversationQueryBounds::Window {
                        maximum_turn_count: QueryTurnCount::new(1)
                            .expect("bounded count should be valid"),
                    },
                })),
            )
            .await,
    );
    let subscribe_failure = failure_of(
        handler
            .respond(
                &request("frame-subscribe"),
                &ClientRequest::Conversation(ConversationRequest::Subscribe(
                    ConversationSubscribe::fresh(ThreadId::parse("thread-1").expect("valid id")),
                )),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    for failure in [query_failure, subscribe_failure] {
        assert_eq!(failure.code, ErrorCode::Internal);
        assert!(!failure.retryable);
    }
}

#[tokio::test]
async fn directory_browsing_fails_typed_until_a_registry_exists() {
    let (_temporary, storage) = opened_storage("directories").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let root_failure = failure_of(
        handler
            .respond(
                &request("frame-root"),
                &ClientRequest::Query(Query::ListDirectories(ListDirectories { parent: None })),
            )
            .await,
    );
    let nested_failure = failure_of(
        handler
            .respond(
                &request("frame-nested"),
                &ClientRequest::Query(Query::ListDirectories(ListDirectories {
                    parent: Some(DirectoryId::parse("directory-parent").expect("valid id")),
                })),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(root_failure.code, ErrorCode::Internal);
    assert!(!root_failure.retryable);
    assert_eq!(nested_failure.code, ErrorCode::DirectoryUnknown);
    assert!(!nested_failure.retryable);
}
