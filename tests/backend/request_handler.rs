//! External tests for the Forge application request-handler seam.
//!
//! Every test drives [`RequestHandler::respond`] directly with a decoded
//! protocol request and a correlated domain request id against real migrated
//! storage, proving repository-backed mapping and typed failure behavior
//! without any network stack.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use artisan_backend::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, ForgeStorage, RequestHandler,
};
use artisan_database::{
    AttachProjectInput, CreateThreadInput, MessageDispatchPayload, QueueFirstMessageInput,
    Repository, SqliteConfig,
};
use artisan_domain::{
    Command, ConversationQuery, ConversationQueryBounds, ConversationRequest,
    ConversationSubscribe, ConversationUnsubscribe, DirectoryId, DisplayName, ListAttachedProjects,
    ListDirectories, ListProjectThreads, MessageBody, MessageId, ProjectId, Query, QueryTurnCount,
    ReceiptDisposition, RequestId, RootPath, ThreadId, ThreadSummary, ThreadTitle, UnixMillis,
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

/// Deterministic origin scripting identities and instants while counting
/// every consultation, standing in for the real OS-entropy boundary.
///
/// The scripts are consulted front to back; a missing entry panics so a test
/// can never silently admit without its planned nondeterminism. Failure
/// entries exercise typed acquisition faults with the real pinned
/// `getrandom::Error` API value.
#[derive(Debug)]
struct ScriptedOrigin {
    identities: Mutex<VecDeque<Result<String, CommandOriginEntropyError>>>,
    identity_calls: AtomicU64,
    instants: Mutex<VecDeque<Result<UnixMillis, CommandOriginClockError>>>,
    instant_calls: AtomicU64,
}

impl ScriptedOrigin {
    fn identity_calls(&self) -> u64 {
        self.identity_calls.load(Ordering::Relaxed)
    }

    fn instant_calls(&self) -> u64 {
        self.instant_calls.load(Ordering::Relaxed)
    }
}

impl CommandOrigin for ScriptedOrigin {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        self.identity_calls.fetch_add(1, Ordering::Relaxed);
        self.identities
            .lock()
            .expect("identity script mutex should not be poisoned")
            .pop_front()
            .expect("identity script should cover every consult in this test")
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        self.instant_calls.fetch_add(1, Ordering::Relaxed);
        self.instants
            .lock()
            .expect("instant script mutex should not be poisoned")
            .pop_front()
            .expect("instant script should cover every consult in this test")
    }
}

/// Shareable handle owning the scripted state.
///
/// The external test crate may implement the foreign backend trait only for
/// a local type, so shared state lives behind this newtype instead of a bare
/// `Arc<ScriptedOrigin>`.
#[derive(Clone, Debug)]
struct ScriptedOriginHandle(Arc<ScriptedOrigin>);

impl ScriptedOriginHandle {
    /// Origin answering each consultation from explicit failure-capable
    /// scripts.
    fn scripted(
        identities: Vec<Result<String, CommandOriginEntropyError>>,
        instants: Vec<Result<UnixMillis, CommandOriginClockError>>,
    ) -> Self {
        Self(Arc::new(ScriptedOrigin {
            identities: Mutex::new(identities.into_iter().collect()),
            identity_calls: AtomicU64::new(0),
            instants: Mutex::new(instants.into_iter().collect()),
            instant_calls: AtomicU64::new(0),
        }))
    }

    /// Origin answering one fresh admission with fixed deterministic values.
    fn deterministic(identities: &[&str], instant_ms: i64) -> Self {
        Self::scripted(
            identities
                .iter()
                .map(|identity| Ok((*identity).to_owned()))
                .collect(),
            vec![Ok(UnixMillis::from_millis(instant_ms)); identities.len()],
        )
    }

    /// Origin whose next identity acquisition fails with a real typed
    /// platform-entropy error value.
    fn failing_entropy() -> Self {
        Self::scripted(
            vec![Err(CommandOriginEntropyError::from(
                getrandom::Error::UNEXPECTED,
            ))],
            Vec::new(),
        )
    }

    /// Origin whose instant acquisition fails after identity succeeds.
    fn failing_clock() -> Self {
        Self::scripted(
            vec![Ok("message-stalled".to_owned())],
            vec![Err(CommandOriginClockError)],
        )
    }

    fn identity_calls(&self) -> u64 {
        self.0.identity_calls()
    }

    fn instant_calls(&self) -> u64 {
        self.0.instant_calls()
    }
}

impl CommandOrigin for ScriptedOriginHandle {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        self.0.mint_identity()
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        self.0.acceptance_instant()
    }
}

/// Handler over `storage` whose fresh admissions answer from `origin`.
fn scripted_handler(storage: &ForgeStorage, origin: &ScriptedOriginHandle) -> RequestHandler {
    RequestHandler::with_origin(storage.repository().clone(), Box::new(origin.clone()))
}

/// Creates a fresh-thread command correlated to `request_id`.
fn create_command(request_id: &str, project: &str, title: &str) -> ClientRequest {
    ClientRequest::Command(Command::CreateThread(artisan_domain::CreateThread {
        request_id: request(request_id),
        project_id: ProjectId::parse(project).expect("valid project id"),
        title: ThreadTitle::parse(title).expect("valid title"),
    }))
}

/// Queues a first-message command correlated to `request_id`.
fn queue_command(request_id: &str, thread: &str, body: &str) -> ClientRequest {
    ClientRequest::Command(Command::QueueFirstMessage(
        artisan_domain::QueueFirstMessage {
            request_id: request(request_id),
            thread_id: ThreadId::parse(thread).expect("valid thread id"),
            body: MessageBody::parse(body).expect("valid body"),
        },
    ))
}

/// Extracts the created-thread payload from a successful response.
fn created_thread_of(response: ServerResponse) -> (ThreadSummary, ReceiptDisposition) {
    let ResponsePayload::CreatedThread {
        thread,
        disposition,
    } = response.payload
    else {
        panic!("expected a created-thread payload");
    };
    (thread, disposition)
}

/// Extracts the queued-receipt payload from a successful response.
fn queued_receipt_of(response: ServerResponse) -> FirstMessageReceipt {
    let ResponsePayload::FirstMessageQueued(receipt) = response.payload else {
        panic!("expected a first-message receipt payload");
    };
    receipt
}

/// Reads back one durable dispatch payload through the repository facade.
async fn read_dispatch(
    repository: &Repository,
    message_id: &str,
) -> Option<MessageDispatchPayload> {
    repository
        .read_message_dispatch_payload(&MessageId::parse(message_id).expect("valid message id"))
        .await
        .expect("dispatch readback should work")
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
async fn conversation_subscriptions_remain_unbacked_without_a_registrar() {
    let (_temporary, storage) = opened_storage("conversation").await;
    let handler = RequestHandler::new(storage.repository().clone());

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
    let unsubscribe_failure = failure_of(
        handler
            .respond(
                &request("frame-unsubscribe"),
                &ClientRequest::Conversation(ConversationRequest::Unsubscribe(
                    ConversationUnsubscribe {
                        thread_id: ThreadId::parse("thread-1").expect("valid id"),
                    },
                )),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(subscribe_failure.code, ErrorCode::Internal);
    assert!(!subscribe_failure.retryable);
    assert_eq!(
        subscribe_failure.request_id,
        Some(request("frame-subscribe"))
    );
    assert!(
        subscribe_failure.detail.as_str().contains("subscription"),
        "subscription failure should name the missing subscription capability"
    );
    assert_eq!(unsubscribe_failure.code, ErrorCode::Internal);
    assert!(!unsubscribe_failure.retryable);
    assert_eq!(
        unsubscribe_failure.request_id,
        Some(request("frame-unsubscribe"))
    );
    assert!(
        unsubscribe_failure.detail.as_str().contains("subscription"),
        "unsubscription failure should name the missing subscription capability"
    );
}

#[tokio::test]
async fn window_conversation_query_answers_from_durable_snapshot() {
    let (_temporary, storage) = opened_storage("conversation-snapshot").await;
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
            &request("frame-conversation-query"),
            &ClientRequest::Conversation(ConversationRequest::Query(ConversationQuery {
                thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
                bounds: ConversationQueryBounds::Window {
                    maximum_turn_count: QueryTurnCount::new(4)
                        .expect("bounded count should be valid"),
                },
            })),
        )
        .await
        .expect("known thread should answer with a conversation snapshot");

    storage.close().await.expect("storage should close");

    assert_eq!(response.request_id, request("frame-conversation-query"));
    let ResponsePayload::ConversationSnapshot(snapshot) = response.payload else {
        panic!("expected a conversation snapshot payload");
    };
    assert_eq!(snapshot.thread_id().as_str(), "thread-1");
    assert_eq!(snapshot.cursor().get(), 0);
    assert!(snapshot.turns().is_empty());
    assert!(snapshot.items().is_empty());
    assert_eq!(snapshot.updated_at(), UnixMillis::from_millis(200));
}

#[tokio::test]
async fn unknown_thread_conversation_query_fails_thread_unknown() {
    let (_temporary, storage) = opened_storage("conversation-unknown").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let failure = failure_of(
        handler
            .respond(
                &request("frame-unknown-query"),
                &ClientRequest::Conversation(ConversationRequest::Query(ConversationQuery {
                    thread_id: ThreadId::parse("thread-missing").expect("valid thread id"),
                    bounds: ConversationQueryBounds::Window {
                        maximum_turn_count: QueryTurnCount::new(1)
                            .expect("bounded count should be valid"),
                    },
                })),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::ThreadUnknown);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("frame-unknown-query")));
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

#[tokio::test]
async fn pick_directory_fails_correlated_nonretryable_without_a_picker_process() {
    let (_temporary, storage) = opened_storage("pick-directory").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let failure = failure_of(
        handler
            .respond(&request("frame-pick"), &ClientRequest::PickDirectory)
            .await,
    );

    storage.close().await.expect("storage should close");

    // The explicit host-interaction request stays correlated to its frame,
    // answers through the established unbacked-capability path, and never
    // claims that a picker ran: repeating it deterministically fails again.
    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("frame-pick")));
}

#[tokio::test]
async fn fresh_create_thread_admits_with_a_forged_identity_and_reopens_durable() {
    let (temporary, storage) = opened_storage("fresh-create").await;
    storage
        .repository()
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    let origin = ScriptedOriginHandle::deterministic(&["thread-forged-1"], 1000);
    let handler = scripted_handler(&storage, &origin);

    let response = handler
        .respond(
            &request("request-create"),
            &create_command("request-create", "project-1", "New thread"),
        )
        .await
        .expect("fresh create should admit");
    assert_eq!(response.request_id, request("request-create"));
    let (thread, disposition) = created_thread_of(response);

    let listing = storage
        .repository()
        .list_threads(&ProjectId::parse("project-1").expect("valid project id"))
        .await
        .expect("persisted threads should list");
    storage.close().await.expect("storage should close");

    assert_eq!(disposition, ReceiptDisposition::Accepted);
    assert_eq!(thread.thread_id.as_str(), "thread-forged-1");
    assert_eq!(thread.project_id.as_str(), "project-1");
    assert_eq!(thread.title.as_str(), "New thread");
    assert_eq!(thread.created_at, UnixMillis::from_millis(1000));
    assert_eq!(thread.updated_at, UnixMillis::from_millis(1000));
    assert_eq!(listing.threads().len(), 1);
    assert_eq!(listing.threads()[0], thread);

    let reopened = ForgeStorage::open(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("storage should reopen");
    let replay = reopened
        .repository()
        .lookup_create_thread(
            &request("request-create"),
            &ProjectId::parse("project-1").expect("valid project id"),
            &ThreadTitle::parse("New thread").expect("valid title"),
        )
        .await
        .expect("reopened receipt lookup should work")
        .expect("accepted receipt should survive reopen");
    reopened.close().await.expect("storage should close");

    assert_eq!(replay.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(replay.thread, thread);
}

#[tokio::test]
async fn fresh_queue_first_message_accepts_atomically_and_reopens_durable() {
    let (temporary, storage) = opened_storage("fresh-queue").await;
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
    let origin = ScriptedOriginHandle::deterministic(&["message-forged-1"], 1200);
    let handler = scripted_handler(&storage, &origin);

    let response = handler
        .respond(
            &request("request-message"),
            &queue_command("request-message", "thread-1", "first body"),
        )
        .await
        .expect("fresh queue should accept");
    let receipt = queued_receipt_of(response);

    let payload = read_dispatch(repository, "message-forged-1").await;
    let listing = repository
        .list_threads(&ProjectId::parse("project-1").expect("valid project id"))
        .await
        .expect("threads should list after acceptance");
    storage.close().await.expect("storage should close");

    assert_eq!(
        receipt,
        FirstMessageReceipt {
            request_id: request("request-message"),
            message_id: MessageId::parse("message-forged-1").expect("valid message id"),
            thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
            disposition: ReceiptDisposition::Accepted,
        }
    );
    let payload =
        payload.expect("queued acceptance should atomically persist the durable dispatch");
    assert_eq!(payload.thread_id.as_str(), "thread-1");
    assert_eq!(payload.correlation_id.as_str(), "request-message");
    assert_eq!(payload.body.as_str(), "first body");
    assert_eq!(
        listing.threads()[0].updated_at,
        UnixMillis::from_millis(1200)
    );

    let reopened = ForgeStorage::open(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("storage should reopen");
    let replay = reopened
        .repository()
        .lookup_queue_first_message(
            &request("request-message"),
            &ThreadId::parse("thread-1").expect("valid thread id"),
            &MessageBody::parse("first body").expect("valid body"),
        )
        .await
        .expect("reopened receipt lookup should work")
        .expect("accepted receipt should survive reopen");
    reopened.close().await.expect("storage should close");

    assert_eq!(replay.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(
        replay.message.message_id,
        MessageId::parse("message-forged-1").expect("valid message id")
    );
}

#[tokio::test]
async fn exact_replay_answers_duplicate_without_consulting_the_origin() {
    let (_temporary, storage) = opened_storage("replay-counts").await;
    storage
        .repository()
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    let origin = ScriptedOriginHandle::deterministic(&["thread-forged"], 500);
    let handler = scripted_handler(&storage, &origin);

    let accepted = handler
        .respond(
            &request("request-create"),
            &create_command("request-create", "project-1", "Same thread"),
        )
        .await
        .expect("fresh create should admit");
    let replay = handler
        .respond(
            &request("request-create"),
            &create_command("request-create", "project-1", "Same thread"),
        )
        .await
        .expect("exact replay should answer from the durable receipt");

    storage.close().await.expect("storage should close");

    let (accepted_thread, accepted_disposition) = created_thread_of(accepted);
    let (replayed_thread, replayed_disposition) = created_thread_of(replay);
    assert_eq!(accepted_disposition, ReceiptDisposition::Accepted);
    assert_eq!(replayed_disposition, ReceiptDisposition::Duplicate);
    assert_eq!(replayed_thread, accepted_thread);
    assert_eq!(origin.identity_calls(), 1);
    assert_eq!(origin.instant_calls(), 1);
}

#[tokio::test]
async fn changed_payload_conflict_preserves_the_original_outcome_without_origin_consults() {
    let (_temporary, storage) = opened_storage("conflict-counts").await;
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
    let origin = ScriptedOriginHandle::scripted(Vec::new(), Vec::new());
    let handler = scripted_handler(&storage, &origin);

    let conflict = failure_of(
        handler
            .respond(
                &request("request-thread-1"),
                &create_command("request-thread-1", "project-1", "Other thread"),
            )
            .await,
    );
    let original = handler
        .respond(
            &request("request-thread-1"),
            &create_command("request-thread-1", "project-1", "First thread"),
        )
        .await
        .expect("the original outcome must stay replayable");

    storage.close().await.expect("storage should close");

    assert_eq!(conflict.code, ErrorCode::IdempotencyConflict);
    assert!(!conflict.retryable);
    assert_eq!(conflict.request_id, Some(request("request-thread-1")));
    assert!(!conflict.detail.as_str().contains("First thread"));
    assert!(!conflict.detail.as_str().contains("Other thread"));
    let (thread, disposition) = created_thread_of(original);
    assert_eq!(disposition, ReceiptDisposition::Duplicate);
    assert_eq!(thread.thread_id.as_str(), "thread-1");
    assert_eq!(origin.identity_calls(), 0);
    assert_eq!(origin.instant_calls(), 0);
}

#[tokio::test]
async fn reads_unsupported_requests_and_bad_correlation_never_consult_the_origin() {
    let (_temporary, storage) = opened_storage("no-consult").await;
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
    let origin = ScriptedOriginHandle::scripted(Vec::new(), Vec::new());
    let handler = scripted_handler(&storage, &origin);

    let listing = handler
        .respond(
            &request("frame-threads"),
            &ClientRequest::Query(Query::ListProjectThreads(ListProjectThreads {
                project_id: ProjectId::parse("project-1").expect("valid project id"),
            })),
        )
        .await
        .expect("reads answer without the admission origin");
    let conversation_success = handler
        .respond(
            &request("frame-conversation"),
            &ClientRequest::Conversation(ConversationRequest::Query(ConversationQuery {
                thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
                bounds: ConversationQueryBounds::Window {
                    maximum_turn_count: QueryTurnCount::new(1)
                        .expect("bounded count should be valid"),
                },
            })),
        )
        .await
        .expect("successful conversation query should not consult the origin");
    let conversation_failure = failure_of(
        handler
            .respond(
                &request("frame-unknown-conversation"),
                &ClientRequest::Conversation(ConversationRequest::Query(ConversationQuery {
                    thread_id: ThreadId::parse("thread-missing").expect("valid thread id"),
                    bounds: ConversationQueryBounds::Window {
                        maximum_turn_count: QueryTurnCount::new(1)
                            .expect("bounded count should be valid"),
                    },
                })),
            )
            .await,
    );
    let correlation = failure_of(
        handler
            .respond(
                &request("frame-other"),
                &create_command("request-create", "project-1", "New thread"),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert!(matches!(listing.payload, ResponsePayload::ThreadListing(_)));
    assert!(
        matches!(
            conversation_success.payload,
            ResponsePayload::ConversationSnapshot(_)
        ),
        "known-thread query should answer with a snapshot without consulting origin"
    );
    assert_eq!(conversation_failure.code, ErrorCode::ThreadUnknown);
    assert!(!conversation_failure.retryable);
    assert_eq!(
        conversation_failure.request_id,
        Some(request("frame-unknown-conversation"))
    );
    assert_eq!(correlation.code, ErrorCode::InvalidInput);
    assert!(!correlation.retryable);
    assert_eq!(correlation.request_id, Some(request("frame-other")));
    assert_eq!(origin.identity_calls(), 0);
    assert_eq!(origin.instant_calls(), 0);
}

#[tokio::test]
async fn conversation_queries_never_consult_the_origin() {
    let (_temporary, storage) = opened_storage("conversation-no-origin").await;
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
    let origin = ScriptedOriginHandle::scripted(Vec::new(), Vec::new());
    let handler = scripted_handler(&storage, &origin);

    let success = handler
        .respond(
            &request("frame-success"),
            &ClientRequest::Conversation(ConversationRequest::Query(ConversationQuery {
                thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
                bounds: ConversationQueryBounds::Window {
                    maximum_turn_count: QueryTurnCount::new(2)
                        .expect("bounded count should be valid"),
                },
            })),
        )
        .await
        .expect("successful query should answer without origin");
    let failure = failure_of(
        handler
            .respond(
                &request("frame-failure"),
                &ClientRequest::Conversation(ConversationRequest::Query(ConversationQuery {
                    thread_id: ThreadId::parse("thread-unknown").expect("valid thread id"),
                    bounds: ConversationQueryBounds::Window {
                        maximum_turn_count: QueryTurnCount::new(2)
                            .expect("bounded count should be valid"),
                    },
                })),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert!(matches!(
        success.payload,
        ResponsePayload::ConversationSnapshot(_)
    ));
    assert_eq!(success.request_id, request("frame-success"));
    assert_eq!(failure.code, ErrorCode::ThreadUnknown);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("frame-failure")));
    assert_eq!(origin.identity_calls(), 0);
    assert_eq!(origin.instant_calls(), 0);
}

#[tokio::test]
async fn fresh_create_for_unknown_project_fails_project_unknown_without_rows() {
    let (_temporary, storage) = opened_storage("unknown-project-create").await;
    storage
        .repository()
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    let origin = ScriptedOriginHandle::deterministic(&["thread-orphan"], 100);
    let handler = scripted_handler(&storage, &origin);

    let failure = failure_of(
        handler
            .respond(
                &request("request-orphan"),
                &create_command("request-orphan", "project-missing", "Orphan"),
            )
            .await,
    );

    let receipt = storage
        .repository()
        .lookup_create_thread(
            &request("request-orphan"),
            &ProjectId::parse("project-missing").expect("valid project id"),
            &ThreadTitle::parse("Orphan").expect("valid title"),
        )
        .await
        .expect("receipt lookup should work");
    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::ProjectUnknown);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-orphan")));
    assert!(receipt.is_none());
    assert_eq!(origin.identity_calls(), 1);
    assert_eq!(origin.instant_calls(), 1);
}

#[tokio::test]
async fn fresh_queue_for_unknown_thread_fails_thread_unknown_without_rows() {
    let (_temporary, storage) = opened_storage("unknown-thread-queue").await;
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
    let origin = ScriptedOriginHandle::deterministic(&["message-orphan"], 300);
    let handler = scripted_handler(&storage, &origin);

    let failure = failure_of(
        handler
            .respond(
                &request("request-orphan"),
                &queue_command("request-orphan", "thread-missing", "first body"),
            )
            .await,
    );

    let receipt = repository
        .lookup_queue_first_message(
            &request("request-orphan"),
            &ThreadId::parse("thread-missing").expect("valid thread id"),
            &MessageBody::parse("first body").expect("valid body"),
        )
        .await
        .expect("receipt lookup should work");
    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::ThreadUnknown);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-orphan")));
    assert!(receipt.is_none());
    assert_eq!(origin.identity_calls(), 1);
    assert_eq!(origin.instant_calls(), 1);
}

#[tokio::test]
async fn second_first_message_from_another_request_stays_invalid_input() {
    let (_temporary, storage) = opened_storage("second-first-message").await;
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
            "request-first",
            "thread-1",
            "message-1",
            "original body",
        ))
        .await
        .expect("seed queue should persist");
    // The fresh request misses its receipt lookup first, so the origin must
    // supply identity and instant before the authoritative repository
    // rejects the ordinal-zero collision.
    let origin = ScriptedOriginHandle::deterministic(&["message-restricted"], 300);
    let handler = scripted_handler(&storage, &origin);

    let restricted = failure_of(
        handler
            .respond(
                &request("request-second"),
                &queue_command("request-second", "thread-1", "second attempt body"),
            )
            .await,
    );
    let preserved = handler
        .respond(
            &request("request-first"),
            &queue_command("request-first", "thread-1", "original body"),
        )
        .await
        .expect("the original first message must stay replayable");

    storage.close().await.expect("storage should close");

    assert_eq!(restricted.code, ErrorCode::InvalidInput);
    assert!(!restricted.retryable);
    assert_eq!(restricted.request_id, Some(request("request-second")));
    assert!(!restricted.detail.as_str().contains("original body"));
    assert!(!restricted.detail.as_str().contains("second attempt body"));
    assert_eq!(
        queued_receipt_of(preserved),
        FirstMessageReceipt {
            request_id: request("request-first"),
            message_id: MessageId::parse("message-1").expect("valid message id"),
            thread_id: ThreadId::parse("thread-1").expect("valid thread id"),
            disposition: ReceiptDisposition::Duplicate,
        }
    );
    assert_eq!(origin.identity_calls(), 1);
    assert_eq!(origin.instant_calls(), 1);
}

#[tokio::test]
async fn deterministic_thread_collision_fails_typed_without_partial_writes() {
    let (_temporary, storage) = opened_storage("thread-collision").await;
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
        .create_thread(create_input("request-original", "project-1", "thread-a"))
        .await
        .expect("seed create should persist");
    let origin = ScriptedOriginHandle::deterministic(&["thread-a"], 900);
    let handler = scripted_handler(&storage, &origin);

    let failure = failure_of(
        handler
            .respond(
                &request("request-collision"),
                &create_command("request-collision", "project-1", "Collision title"),
            )
            .await,
    );

    let receipt = repository
        .lookup_create_thread(
            &request("request-collision"),
            &ProjectId::parse("project-1").expect("valid project id"),
            &ThreadTitle::parse("Collision title").expect("valid title"),
        )
        .await
        .expect("receipt lookup should work");
    let listing = repository
        .list_threads(&ProjectId::parse("project-1").expect("valid project id"))
        .await
        .expect("threads should list");
    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-collision")));
    assert!(!failure.detail.as_str().contains("Collision title"));
    assert!(receipt.is_none());
    assert_eq!(listing.threads().len(), 1);
}

#[tokio::test]
async fn deterministic_message_collision_fails_internal_without_partial_writes() {
    let (_temporary, storage) = opened_storage("message-collision").await;
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
        .create_thread(create_input("request-thread-2", "project-1", "thread-2"))
        .await
        .expect("second seed create should persist");
    repository
        .queue_first_message(message_input(
            "request-first",
            "thread-1",
            "message-1",
            "Hello",
        ))
        .await
        .expect("seed queue should persist");
    let origin = ScriptedOriginHandle::deterministic(&["message-1"], 400);
    let handler = scripted_handler(&storage, &origin);

    let failure = failure_of(
        handler
            .respond(
                &request("request-collision"),
                &queue_command("request-collision", "thread-2", "World"),
            )
            .await,
    );

    let receipt = repository
        .lookup_queue_first_message(
            &request("request-collision"),
            &ThreadId::parse("thread-2").expect("valid thread id"),
            &MessageBody::parse("World").expect("valid body"),
        )
        .await
        .expect("receipt lookup should work");
    let original_payload = read_dispatch(repository, "message-1").await;
    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-collision")));
    assert!(receipt.is_none());
    let original_payload =
        original_payload.expect("the seeded original dispatch should stay intact");
    assert_eq!(original_payload.correlation_id.as_str(), "request-first");
}

#[tokio::test]
async fn instant_before_thread_creation_fails_chronology_without_partial_writes() {
    let (_temporary, storage) = opened_storage("chronology").await;
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
    let origin = ScriptedOriginHandle::deterministic(&["message-early"], 199);
    let handler = scripted_handler(&storage, &origin);

    let failure = failure_of(
        handler
            .respond(
                &request("request-early"),
                &queue_command("request-early", "thread-1", "too early"),
            )
            .await,
    );

    let receipt = repository
        .lookup_queue_first_message(
            &request("request-early"),
            &ThreadId::parse("thread-1").expect("valid thread id"),
            &MessageBody::parse("too early").expect("valid body"),
        )
        .await
        .expect("receipt lookup should work");
    let payload = read_dispatch(repository, "message-early").await;
    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-early")));
    assert!(receipt.is_none());
    assert!(payload.is_none());
    assert_eq!(origin.identity_calls(), 1);
    assert_eq!(origin.instant_calls(), 1);
}

#[tokio::test]
async fn entropy_failure_answers_retryable_and_leaves_the_request_retryable() {
    let (_temporary, storage) = opened_storage("entropy-failure").await;
    storage
        .repository()
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    let failing = ScriptedOriginHandle::failing_entropy();
    let handler = scripted_handler(&storage, &failing);

    let failure = failure_of(
        handler
            .respond(
                &request("request-retry"),
                &create_command("request-retry", "project-1", "Recovered thread"),
            )
            .await,
    );

    let receipt = storage
        .repository()
        .lookup_create_thread(
            &request("request-retry"),
            &ProjectId::parse("project-1").expect("valid project id"),
            &ThreadTitle::parse("Recovered thread").expect("valid title"),
        )
        .await
        .expect("receipt lookup should work");
    let recovered = scripted_handler(
        &storage,
        &ScriptedOriginHandle::deterministic(&["thread-recovered"], 700),
    );
    let retry = recovered
        .respond(
            &request("request-retry"),
            &create_command("request-retry", "project-1", "Recovered thread"),
        )
        .await
        .expect("identical retry may succeed once entropy recovers");

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-retry")));
    assert!(failure.detail.as_str().contains("entropy"));
    assert!(receipt.is_none());
    assert_eq!(failing.identity_calls(), 1);
    assert_eq!(failing.instant_calls(), 0);
    let (thread, disposition) = created_thread_of(retry);
    assert_eq!(disposition, ReceiptDisposition::Accepted);
    assert_eq!(thread.thread_id.as_str(), "thread-recovered");
}

#[tokio::test]
async fn clock_failure_answers_retryable_and_leaves_the_request_retryable() {
    let (_temporary, storage) = opened_storage("clock-failure").await;
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
    let failing = ScriptedOriginHandle::failing_clock();
    let handler = scripted_handler(&storage, &failing);

    let failure = failure_of(
        handler
            .respond(
                &request("request-retry"),
                &queue_command("request-retry", "thread-1", "retry me"),
            )
            .await,
    );

    let receipt = repository
        .lookup_queue_first_message(
            &request("request-retry"),
            &ThreadId::parse("thread-1").expect("valid thread id"),
            &MessageBody::parse("retry me").expect("valid body"),
        )
        .await
        .expect("receipt lookup should work");
    let stalled = read_dispatch(repository, "message-stalled").await;
    let recovered = scripted_handler(
        &storage,
        &ScriptedOriginHandle::deterministic(&["message-recovered"], 800),
    );
    let retry = recovered
        .respond(
            &request("request-retry"),
            &queue_command("request-retry", "thread-1", "retry me"),
        )
        .await
        .expect("identical retry may succeed once the clock recovers");

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(failure.retryable);
    assert_eq!(failure.request_id, Some(request("request-retry")));
    assert!(failure.detail.as_str().contains("acceptance instant"));
    assert!(receipt.is_none());
    assert!(stalled.is_none());
    assert_eq!(failing.identity_calls(), 1);
    assert_eq!(failing.instant_calls(), 1);
    let receipt = queued_receipt_of(retry);
    assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(
        receipt.message_id,
        MessageId::parse("message-recovered").expect("valid message id")
    );
}

#[tokio::test]
async fn concurrent_same_request_create_converges_on_one_durable_identity() {
    let (temporary, storage) = opened_storage("concurrent-create").await;
    let repository = storage.repository().clone();
    repository
        .attach_project(attach_input(
            "request-project-1",
            "directory-project-1",
            "project-1",
        ))
        .await
        .expect("seed attach should persist");
    let first_handler = RequestHandler::new(repository.clone());
    let second_handler = RequestHandler::new(repository.clone());

    let command = create_command("request-race", "project-1", "Concurrent");
    let first = tokio::spawn(async move {
        first_handler
            .respond(&request("request-race"), &command)
            .await
    });
    let second_command = create_command("request-race", "project-1", "Concurrent");
    let second = tokio::spawn(async move {
        second_handler
            .respond(&request("request-race"), &second_command)
            .await
    });
    let outcomes = [
        first.await.expect("first task should finish"),
        second.await.expect("second task should finish"),
    ];
    let responses = [
        outcomes[0].as_ref().expect("both races should succeed"),
        outcomes[1].as_ref().expect("both races should succeed"),
    ];
    let dispositions = responses.map(|response| match &response.payload {
        ResponsePayload::CreatedThread { disposition, .. } => *disposition,
        other => panic!("expected a created-thread payload: {other:?}"),
    });
    let threads = responses.map(|response| match &response.payload {
        ResponsePayload::CreatedThread { thread, .. } => thread.clone(),
        other => panic!("expected a created-thread payload: {other:?}"),
    });

    storage.close().await.expect("storage should close");

    assert_eq!(
        dispositions
            .iter()
            .filter(|disposition| **disposition == ReceiptDisposition::Accepted)
            .count(),
        1
    );
    assert_eq!(
        dispositions
            .iter()
            .filter(|disposition| **disposition == ReceiptDisposition::Duplicate)
            .count(),
        1
    );
    assert_eq!(threads[0], threads[1]);

    let reopened = ForgeStorage::open(SqliteConfig::file(temporary.path()).sqlx_logging(false))
        .await
        .expect("storage should reopen");
    let replay = reopened
        .repository()
        .lookup_create_thread(
            &request("request-race"),
            &ProjectId::parse("project-1").expect("valid project id"),
            &ThreadTitle::parse("Concurrent").expect("valid title"),
        )
        .await
        .expect("reopened receipt lookup should work")
        .expect("raced receipt should survive reopen");
    let listing = reopened
        .repository()
        .list_threads(&ProjectId::parse("project-1").expect("valid project id"))
        .await
        .expect("threads should list after reopen");
    reopened.close().await.expect("storage should close");

    assert_eq!(replay.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(replay.thread, threads[0]);
    assert_eq!(listing.threads().len(), 1);
}

#[tokio::test]
async fn concurrent_same_request_queue_converges_on_one_durable_identity() {
    let (_temporary, storage) = opened_storage("concurrent-queue").await;
    let repository = storage.repository().clone();
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
    let first_handler = RequestHandler::new(repository.clone());
    let second_handler = RequestHandler::new(repository.clone());

    let first_command = queue_command("queue-request", "thread-1", "Hello");
    let first = tokio::spawn(async move {
        first_handler
            .respond(&request("queue-request"), &first_command)
            .await
    });
    let second_command = queue_command("queue-request", "thread-1", "Hello");
    let second = tokio::spawn(async move {
        second_handler
            .respond(&request("queue-request"), &second_command)
            .await
    });
    let outcomes = [
        first.await.expect("first task should finish"),
        second.await.expect("second task should finish"),
    ];
    let receipts = outcomes.map(|outcome| queued_receipt_of(outcome.expect("both should answer")));

    let winner = receipts
        .iter()
        .find(|receipt| receipt.disposition == ReceiptDisposition::Accepted)
        .expect("exactly one race winner should exist")
        .clone();
    let payload = read_dispatch(&repository, winner.message_id.as_str()).await;
    storage.close().await.expect("storage should close");

    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.disposition == ReceiptDisposition::Accepted)
            .count(),
        1
    );
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| receipt.disposition == ReceiptDisposition::Duplicate)
            .count(),
        1
    );
    for receipt in &receipts {
        assert_eq!(receipt.message_id, winner.message_id);
        assert_eq!(receipt.thread_id, winner.thread_id);
        assert_eq!(receipt.request_id, winner.request_id);
    }

    let payload = payload.expect("the winning acceptance should keep its durable dispatch");
    assert_eq!(payload.correlation_id.as_str(), "queue-request");
}
