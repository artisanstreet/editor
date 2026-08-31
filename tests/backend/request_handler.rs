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

use artisan_backend::conversation_subscription_registry::{
    ActivateError, ApplyBatchError, SubscriptionLease, SubscriptionState, SubscriptionView,
};
use artisan_backend::request_handler::{
    ActivatedConversationSubscription, ConversationSubscriptionRegistrar, RequestHandlerReceipt,
};
use artisan_backend::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, ForgeStorage, RequestHandler,
};
use artisan_database::{
    AttachProjectInput, BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch,
    CreateThreadInput, DispatchLeaseOwner, LaunchClaimedRun, LaunchClaimedRunOutcome,
    MessageDispatchPayload, ProviderBindingBytes, QueueFirstMessageInput, Repository,
    RunLaunchCredentials, RunStartKey, SetThreadEngineConfigInput, SqliteConfig,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, Command, ConversationCursor, ConversationPatch, ConversationQuery,
    ConversationQueryBounds, ConversationRequest, ConversationSubscribe, ConversationUnsubscribe,
    CountLimit, DirectoryId, DisplayName, EngineAgentId, EngineConfigRevision,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, IncrementalText, ItemId, ListAttachedProjects,
    ListDirectories, ListProjectThreads, MessageBody, MessageId, NetworkAccess, OpenCode2Selection,
    PatchBatch, PatchId, PatchSequence, PermissionId, ProjectId, Query, QueryTurnCount,
    ReceiptDisposition, RequestId, Revision, RootPath, SetThreadEngineConfig, ThreadId,
    ThreadSummary, ThreadTitle, UnixMillis, WebSearchAccess,
};
use artisan_protocol::{
    ClientRequest, ConversationSubscriptionStarted, ErrorCode, FirstMessageReceipt, FrameId,
    LifecycleRequest, ProtocolFailure, ProtocolValueError, ProtocolVersion, ResponsePayload,
    ServerResponse, SetThreadEngineConfigResult, WireEnvelope, WireEnvelopeBody,
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

fn engine_config_command(
    request_id: &str,
    thread_id: &str,
    precondition: EngineConfigUpdatePrecondition,
    label: &str,
) -> ClientRequest {
    ClientRequest::Command(Command::SetThreadEngineConfig(Box::new(
        SetThreadEngineConfig::new(
            request(request_id),
            ThreadId::parse(thread_id).expect("valid thread id"),
            precondition,
            engine_config(label),
        ),
    )))
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

fn engine_config(label: &str) -> EngineRunConfig {
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
        PermissionId::parse(format!("permission-{label}")).expect("permission id is valid"),
        EngineAgentId::parse(format!("agent-{label}")).expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse(format!("profile-{label}")).expect("profile id is valid"),
            EngineModelId::parse(format!("model-{label}")).expect("model id is valid"),
            EngineRouteId::parse(format!("route-{label}")).expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
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

/// Seeds one conversation with two durable patch sequences through the public
/// repository workflow used by the real dispatch path.
async fn seed_conversation(repository: &Repository, thread_id: &str, label: &str) {
    let project_id = format!("project-{label}");
    repository
        .attach_project(attach_input(
            &format!("request-project-{label}"),
            &format!("directory-project-{label}"),
            &project_id,
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input(
            &format!("request-thread-{label}"),
            &project_id,
            thread_id,
        ))
        .await
        .expect("seed thread should persist");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request(&format!("request-engine-{label}")),
            thread_id: ThreadId::parse(thread_id).expect("valid thread id"),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: engine_config(label),
            accepted_at: UnixMillis::from_millis(250),
        })
        .await
        .expect("seed engine configuration should persist");
    repository
        .queue_first_message(message_input(
            &format!("request-message-{label}"),
            thread_id,
            &format!("message-{label}"),
            "durable conversation seed",
        ))
        .await
        .expect("seed message should persist");

    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x11; 32]),
            claimed_at: UnixMillis::from_millis(400),
            lease_expires_at: UnixMillis::from_millis(900),
        })
        .await
        .expect("seed dispatch claim should persist")
        .expect("seed dispatch should be claimable");
    let run_id = artisan_domain::RunId::parse(format!("run-{label}")).expect("valid run id");
    let turn_id = artisan_domain::TurnId::parse(format!("turn-{label}")).expect("valid turn id");
    let item_id = artisan_domain::ItemId::parse(format!("item-{label}")).expect("valid item id");
    let first_patch_id =
        artisan_domain::PatchId::parse(format!("patch-{label}-first")).expect("valid patch id");
    let second_patch_id =
        artisan_domain::PatchId::parse(format!("patch-{label}-second")).expect("valid patch id");
    let run_start_key = run_start_key(label);
    let credentials = RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]);
    let engine_settings = repository
        .read_thread_engine_settings(&ThreadId::parse(thread_id).expect("valid thread id"))
        .await
        .expect("seed engine configuration should read")
        .expect("seed engine configuration should be present");
    let launched = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &item_id,
            first_patch_id: &first_patch_id,
            second_patch_id: &second_patch_id,
            operated_at: UnixMillis::from_millis(500),
            run_start_key: &run_start_key,
            credentials: &credentials,
            engine_settings: &engine_settings,
        })
        .await
        .expect("seed run launch should persist");
    let launched = match launched {
        LaunchClaimedRunOutcome::Started(receipt)
        | LaunchClaimedRunOutcome::AlreadyStarted(receipt) => receipt,
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("valid provider binding");
    let bound = repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &run_start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(500),
            bound_at: UnixMillis::from_millis(600),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await
        .expect("seed provider binding should persist");
    assert!(matches!(
        bound,
        BindRunProviderOutcome::Bound(_) | BindRunProviderOutcome::AlreadyBound(_)
    ));
}

fn run_start_key(label: &str) -> RunStartKey {
    let mut bytes = [0x44; 32];
    for (index, byte) in label.as_bytes().iter().take(32).enumerate() {
        bytes[index] = *byte;
    }
    RunStartKey::new(bytes)
}

fn single_patch_batch(thread_id: ThreadId, from: u64, to: u64, patch_id: &str) -> PatchBatch {
    PatchBatch::new(
        thread_id,
        ConversationCursor::new(from),
        ConversationCursor::new(to),
        vec![ConversationPatch::ItemAppend {
            patch_id: PatchId::parse(patch_id).expect("fixture patch id should be valid"),
            sequence: PatchSequence::new(to).expect("fixture sequence should be positive"),
            item_id: ItemId::parse("item-registrar").expect("fixture item id should be valid"),
            revision: Revision::new(to),
            text: IncrementalText::parse("x").expect("fixture fragment should be valid"),
            updated_at: UnixMillis::from_millis(1),
        }],
    )
    .expect("fixture batch should be valid")
}

async fn subscription_request(
    handler: &RequestHandler,
    frame_id: &str,
    subscription: ConversationSubscribe,
) -> (
    Result<ServerResponse, ProtocolFailure>,
    RequestHandlerReceipt,
) {
    handler
        .respond_with_receipt(
            &request(frame_id),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(subscription)),
        )
        .await
        .into_parts()
}

async fn unsubscribe_request(
    handler: &RequestHandler,
    frame_id: &str,
    thread_id: ThreadId,
) -> (
    Result<ServerResponse, ProtocolFailure>,
    RequestHandlerReceipt,
) {
    handler
        .respond_with_receipt(
            &request(frame_id),
            &ClientRequest::Conversation(ConversationRequest::Unsubscribe(
                ConversationUnsubscribe { thread_id },
            )),
        )
        .await
        .into_parts()
}

async fn activate_subscription(
    handler: &RequestHandler,
    receipt: RequestHandlerReceipt,
) -> ActivatedConversationSubscription {
    handler
        .activate_after_response(receipt)
        .await
        .expect("subscription receipt should activate")
        .expect("subscription receipt should carry activation work")
}

async fn registrar_view(
    registrar: &ConversationSubscriptionRegistrar,
    thread_id: &ThreadId,
) -> SubscriptionView {
    registrar
        .subscription_view(thread_id)
        .await
        .expect("subscription should be visible")
}

async fn assert_batch_error(
    registrar: &ConversationSubscriptionRegistrar,
    lease: &SubscriptionLease,
    batch: &PatchBatch,
    expected: ApplyBatchError,
    thread_id: &ThreadId,
    view: &SubscriptionView,
) {
    assert_eq!(
        registrar.record_published_batch(lease, batch).await,
        Err(expected)
    );
    assert_eq!(registrar_view(registrar, thread_id).await, view.clone());
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

#[test]
fn engine_config_response_correlation_observes_nested_request_mismatch() {
    fn envelope(nested_request_id: &str) -> WireEnvelope {
        WireEnvelope {
            protocol_version: ProtocolVersion::V1,
            frame_id: FrameId::parse("request-engine-correlation").expect("valid frame id"),
            sent_at: UnixMillis::from_millis(1),
            body: WireEnvelopeBody::Response(ServerResponse {
                request_id: request("request-engine-correlation"),
                payload: ResponsePayload::ThreadEngineConfigSet(SetThreadEngineConfigResult {
                    request_id: request(nested_request_id),
                    thread_id: ThreadId::parse("thread-engine-correlation")
                        .expect("valid thread id"),
                    revision: EngineConfigRevision::new(1).expect("valid revision"),
                    disposition: ReceiptDisposition::Accepted,
                }),
            }),
        }
    }

    assert_eq!(
        envelope("request-engine-correlation").validate_correlation(),
        Ok(())
    );
    assert_eq!(
        envelope("request-engine-other").validate_correlation(),
        Err(ProtocolValueError::ResponseCorrelationMismatch)
    );
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
async fn lifecycle_requests_use_the_defensive_unsupported_feature_fallback() {
    let (_temporary, storage) = opened_storage("lifecycle-fallback").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let (response, receipt) = handler
        .respond_with_receipt(
            &request("frame-lifecycle-fallback"),
            &ClientRequest::Lifecycle(LifecycleRequest::Status),
        )
        .await
        .into_parts();
    assert!(receipt.is_no_work());
    let failure = failure_of(response);

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::UnsupportedFeature);
    assert!(!failure.retryable);
    assert_eq!(
        failure.request_id,
        Some(request("frame-lifecycle-fallback"))
    );
    assert!(failure.detail.as_str().len() <= artisan_protocol::ERROR_DETAIL_MAX_BYTES);
    assert!(!failure.detail.as_str().contains("frame-lifecycle-fallback"));
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

#[tokio::test]
async fn receipt_handler_fresh_subscribe_returns_real_snapshot_pending_then_active() {
    let (_temporary, storage) = opened_storage("receipt-fresh").await;
    let repository = storage.repository();
    let thread_id = ThreadId::parse("thread-receipt-fresh").expect("valid thread id");
    seed_conversation(repository, thread_id.as_str(), "receipt-fresh").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());

    let (wire, receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-fresh"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(thread_id.clone()),
            )),
        )
        .await
        .into_parts();
    let response = wire.expect("fresh subscription should answer");
    assert_eq!(response.request_id, request("frame-receipt-fresh"));
    let ResponsePayload::ConversationSubscriptionStarted(ConversationSubscriptionStarted::Fresh(
        start,
    )) = response.payload
    else {
        panic!("expected a fresh conversation subscription response");
    };
    assert_eq!(start.snapshot().thread_id(), &thread_id);
    assert_eq!(start.snapshot().cursor().get(), 2);
    assert!(!receipt.is_no_work());

    let pending = handler
        .subscription_view(&thread_id)
        .await
        .expect("fresh subscription should be visible");
    assert_eq!(pending.state(), SubscriptionState::Pending);
    assert_eq!(pending.cursor().get(), 2);

    let activated = handler
        .activate_after_response(receipt)
        .await
        .expect("fresh receipt should activate")
        .expect("fresh receipt should carry activation work");
    assert_eq!(activated.lease(), pending.lease());
    assert_eq!(activated.cursor().get(), 2);

    let active = handler
        .subscription_view(&thread_id)
        .await
        .expect("activated subscription should remain visible");
    assert_eq!(active.state(), SubscriptionState::Active);
    assert_eq!(active.cursor().get(), 2);
    assert_eq!(active.lease(), activated.lease());
    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn engine_config_command_is_durable_idempotent_and_revision_fenced() {
    let (_temporary, storage) = opened_storage("engine-config-command").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "engine-project-request",
            "engine-project-directory",
            "engine-project",
        ))
        .await
        .expect("engine project should attach");
    repository
        .create_thread(create_input(
            "engine-thread-create",
            "engine-project",
            "engine-thread",
        ))
        .await
        .expect("engine thread should create");

    let origin = ScriptedOriginHandle::scripted(
        Vec::new(),
        vec![
            Ok(UnixMillis::from_millis(400)),
            Ok(UnixMillis::from_millis(401)),
            Ok(UnixMillis::from_millis(402)),
        ],
    );
    let handler = scripted_handler(&storage, &origin);
    let first = engine_config_command(
        "engine-first",
        "engine-thread",
        EngineConfigUpdatePrecondition::Unconfigured,
        "handler-first",
    );
    let response = handler
        .respond(&request("engine-first"), &first)
        .await
        .expect("first engine configuration should succeed");
    let first_revision = match response.payload {
        ResponsePayload::ThreadEngineConfigSet(result) => {
            assert_eq!(result.request_id, request("engine-first"));
            assert_eq!(result.thread_id, ThreadId::parse("engine-thread").unwrap());
            assert_eq!(result.disposition, ReceiptDisposition::Accepted);
            result.revision
        }
        payload => panic!("unexpected engine configuration payload: {payload:?}"),
    };
    assert_eq!(origin.instant_calls(), 1);

    let replay = handler
        .respond(&request("engine-first"), &first)
        .await
        .expect("exact engine configuration replay should succeed");
    match replay.payload {
        ResponsePayload::ThreadEngineConfigSet(result) => {
            assert_eq!(result.revision, first_revision);
            assert_eq!(result.disposition, ReceiptDisposition::Duplicate);
        }
        payload => panic!("unexpected replay payload: {payload:?}"),
    }
    assert_eq!(origin.instant_calls(), 1);

    let update = engine_config_command(
        "engine-update",
        "engine-thread",
        EngineConfigUpdatePrecondition::Exact(first_revision),
        "handler-updated",
    );
    let update_response = handler
        .respond(&request("engine-update"), &update)
        .await
        .expect("engine configuration update should succeed");
    let second_revision = match update_response.payload {
        ResponsePayload::ThreadEngineConfigSet(result) => {
            assert_eq!(result.revision.get(), 2);
            assert_eq!(result.disposition, ReceiptDisposition::Accepted);
            result.revision
        }
        payload => panic!("unexpected update payload: {payload:?}"),
    };
    assert_ne!(second_revision, first_revision);

    let stale = engine_config_command(
        "engine-stale",
        "engine-thread",
        EngineConfigUpdatePrecondition::Exact(first_revision),
        "handler-stale",
    );
    let failure = failure_of(handler.respond(&request("engine-stale"), &stale).await);
    assert_eq!(failure.code, ErrorCode::EngineConfigConflict);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("engine-stale")));
    assert_eq!(origin.instant_calls(), 3);

    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn engine_config_command_rejects_correlation_mismatch_before_admission() {
    let (_temporary, storage) = opened_storage("engine-config-correlation").await;
    let origin = ScriptedOriginHandle::scripted(Vec::new(), Vec::new());
    let handler = scripted_handler(&storage, &origin);
    let command = engine_config_command(
        "engine-command",
        "engine-thread",
        EngineConfigUpdatePrecondition::Unconfigured,
        "handler-correlation",
    );
    let failure = failure_of(handler.respond(&request("engine-frame"), &command).await);
    assert_eq!(failure.code, ErrorCode::InvalidInput);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("engine-frame")));
    assert_eq!(origin.instant_calls(), 0);

    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn engine_config_command_maps_missing_thread_to_typed_failure() {
    let (_temporary, storage) = opened_storage("engine-config-missing").await;
    let origin = ScriptedOriginHandle::scripted(Vec::new(), vec![Ok(UnixMillis::from_millis(500))]);
    let handler = scripted_handler(&storage, &origin);
    let command = engine_config_command(
        "engine-missing",
        "engine-thread-missing",
        EngineConfigUpdatePrecondition::Unconfigured,
        "handler-missing",
    );
    let failure = failure_of(handler.respond(&request("engine-missing"), &command).await);
    assert_eq!(failure.code, ErrorCode::ThreadUnknown);
    assert!(!failure.retryable);
    assert_eq!(origin.instant_calls(), 1);

    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn receipt_handler_resume_current_and_batch_register_at_requested_cursor() {
    let (_temporary, storage) = opened_storage("receipt-resume").await;
    let repository = storage.repository();
    let current_thread = ThreadId::parse("thread-receipt-current").expect("valid thread id");
    let batch_thread = ThreadId::parse("thread-receipt-batch").expect("valid thread id");
    seed_conversation(repository, current_thread.as_str(), "receipt-current").await;
    seed_conversation(repository, batch_thread.as_str(), "receipt-batch").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());

    let (current_wire, current_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-current"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::resume(current_thread.clone(), ConversationCursor::new(2)),
            )),
        )
        .await
        .into_parts();
    let current = current_wire.expect("current resume should answer");
    assert!(!current_receipt.is_no_work());
    match current.payload {
        ResponsePayload::ConversationSubscriptionStarted(
            ConversationSubscriptionStarted::Resumed { thread_id, cursor },
        ) => {
            assert_eq!(thread_id, current_thread);
            assert_eq!(cursor, ConversationCursor::new(2));
        }
        other => panic!("expected a current resume response: {other:?}"),
    }
    let current_view = handler
        .subscription_view(&current_thread)
        .await
        .expect("current resume should be visible");
    assert_eq!(current_view.state(), SubscriptionState::Pending);
    assert_eq!(current_view.cursor(), ConversationCursor::new(2));

    let (batch_wire, batch_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-batch"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::resume(batch_thread.clone(), ConversationCursor::new(0)),
            )),
        )
        .await
        .into_parts();
    let batch = batch_wire.expect("batch resume should answer");
    assert!(!batch_receipt.is_no_work());
    match batch.payload {
        ResponsePayload::ConversationSubscriptionStarted(
            ConversationSubscriptionStarted::Resumed { thread_id, cursor },
        ) => {
            assert_eq!(thread_id, batch_thread);
            assert_eq!(cursor, ConversationCursor::new(0));
        }
        other => panic!("expected a batch resume response: {other:?}"),
    }
    let batch_view = handler
        .subscription_view(&batch_thread)
        .await
        .expect("batch resume should be visible");
    assert_eq!(batch_view.state(), SubscriptionState::Pending);
    assert_eq!(batch_view.cursor(), ConversationCursor::new(0));
    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn receipt_handler_beyond_tail_is_stable_and_preserves_existing_entry() {
    let (_temporary, storage) = opened_storage("receipt-beyond-tail").await;
    let repository = storage.repository();
    let thread_id = ThreadId::parse("thread-receipt-beyond").expect("valid thread id");
    seed_conversation(repository, thread_id.as_str(), "receipt-beyond").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());

    let (fresh_wire, old_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-existing"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(thread_id.clone()),
            )),
        )
        .await
        .into_parts();
    fresh_wire.expect("initial subscription should answer");
    let before = handler
        .subscription_view(&thread_id)
        .await
        .expect("initial subscription should be visible");

    let (wire, receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-beyond"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::resume(thread_id.clone(), ConversationCursor::new(999)),
            )),
        )
        .await
        .into_parts();
    let failure = wire.expect_err("beyond-tail resume should fail");
    assert_eq!(failure.code, ErrorCode::InvalidInput);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("frame-receipt-beyond")));
    assert_eq!(
        failure.detail.as_str(),
        "a fresh conversation resnapshot is required"
    );
    assert!(!failure.detail.as_str().contains("999"));
    assert!(!failure.detail.as_str().contains(thread_id.as_str()));
    assert!(receipt.is_no_work());
    assert_eq!(
        handler
            .subscription_view(&thread_id)
            .await
            .expect("existing subscription should remain visible"),
        before
    );
    drop(old_receipt);
    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn receipt_handler_repository_failure_has_existing_classification_and_no_mutation() {
    let (_temporary, storage) = opened_storage("receipt-repository-failure").await;
    let repository = storage.repository();
    let existing_thread = ThreadId::parse("thread-receipt-existing").expect("valid thread id");
    seed_conversation(repository, existing_thread.as_str(), "receipt-existing").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());

    let (existing_wire, existing_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-existing-seed"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(existing_thread.clone()),
            )),
        )
        .await
        .into_parts();
    existing_wire.expect("existing subscription should answer");
    let before = handler
        .subscription_view(&existing_thread)
        .await
        .expect("existing subscription should be visible");

    let missing_thread = ThreadId::parse("thread-receipt-missing").expect("valid thread id");
    let (wire, receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-repository-failure"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(missing_thread),
            )),
        )
        .await
        .into_parts();
    let failure = wire.expect_err("unknown thread should fail through the repository classifier");
    assert_eq!(failure.code, ErrorCode::ThreadUnknown);
    assert!(!failure.retryable);
    assert_eq!(
        failure.request_id,
        Some(request("frame-receipt-repository-failure"))
    );
    assert!(receipt.is_no_work());
    assert_eq!(
        handler
            .subscription_view(&existing_thread)
            .await
            .expect("existing entry should remain visible"),
        before
    );
    drop(existing_receipt);
    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn receipt_handler_replacement_stales_old_receipt_and_activates_only_new_lease() {
    let (_temporary, storage) = opened_storage("receipt-replacement").await;
    let repository = storage.repository();
    let thread_id = ThreadId::parse("thread-receipt-replacement").expect("valid thread id");
    seed_conversation(repository, thread_id.as_str(), "receipt-replacement").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());

    let (first_wire, first_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-first"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(thread_id.clone()),
            )),
        )
        .await
        .into_parts();
    first_wire.expect("first subscription should answer");
    let first_view = handler
        .subscription_view(&thread_id)
        .await
        .expect("first subscription should be visible");

    let (second_wire, second_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-second"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::resume(thread_id.clone(), ConversationCursor::new(0)),
            )),
        )
        .await
        .into_parts();
    let second = second_wire.expect("replacement subscription should answer");
    assert!(matches!(
        second.payload,
        ResponsePayload::ConversationSubscriptionStarted(
            ConversationSubscriptionStarted::Resumed { .. }
        )
    ));
    let second_view = handler
        .subscription_view(&thread_id)
        .await
        .expect("replacement subscription should be visible");
    assert_ne!(first_view.lease(), second_view.lease());
    assert_eq!(second_view.cursor(), ConversationCursor::new(0));
    assert_eq!(second_view.state(), SubscriptionState::Pending);

    assert_eq!(
        handler.activate_after_response(first_receipt).await,
        Err(ActivateError::StaleLease)
    );
    let activated = handler
        .activate_after_response(second_receipt)
        .await
        .expect("replacement receipt should activate")
        .expect("replacement receipt should carry activation work");
    assert_eq!(activated.lease(), second_view.lease());
    assert_eq!(activated.cursor(), ConversationCursor::new(0));
    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn receipt_handler_rejects_cross_handler_receipts_before_activation() {
    let (_temporary, storage) = opened_storage("receipt-cross-handler").await;
    let repository = storage.repository();
    let thread_id = ThreadId::parse("thread-receipt-cross-handler").expect("valid thread id");
    seed_conversation(repository, thread_id.as_str(), "receipt-cross-handler").await;
    let handler_a = RequestHandler::with_subscriptions(repository.clone());
    let handler_b = RequestHandler::with_subscriptions(repository.clone());

    let (a_wire, a_receipt) = handler_a
        .respond_with_receipt(
            &request("frame-receipt-cross-a"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(thread_id.clone()),
            )),
        )
        .await
        .into_parts();
    a_wire.expect("handler A subscription should answer");
    let (b_wire, b_receipt) = handler_b
        .respond_with_receipt(
            &request("frame-receipt-cross-b"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(thread_id.clone()),
            )),
        )
        .await
        .into_parts();
    b_wire.expect("handler B subscription should answer");

    let a_before = handler_a
        .subscription_view(&thread_id)
        .await
        .expect("handler A subscription should be visible");
    let b_before = handler_b
        .subscription_view(&thread_id)
        .await
        .expect("handler B subscription should be visible");
    assert_eq!(a_before.lease(), b_before.lease());
    assert_eq!(a_before.state(), SubscriptionState::Pending);
    assert_eq!(b_before.state(), SubscriptionState::Pending);

    assert_eq!(
        handler_b.activate_after_response(a_receipt).await,
        Err(ActivateError::StaleLease)
    );
    assert_eq!(
        handler_b.subscription_view(&thread_id).await,
        Some(b_before.clone())
    );

    let b_activated = handler_b
        .activate_after_response(b_receipt)
        .await
        .expect("handler B receipt should activate")
        .expect("handler B receipt should carry activation work");
    assert_eq!(b_activated.lease(), b_before.lease());
    assert_eq!(b_activated.cursor(), b_before.cursor());
    assert_eq!(
        handler_b
            .subscription_view(&thread_id)
            .await
            .expect("handler B active subscription should remain visible")
            .state(),
        SubscriptionState::Active
    );

    let (a_replacement_wire, a_replacement_receipt) = handler_a
        .respond_with_receipt(
            &request("frame-receipt-cross-a-replacement"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::resume(thread_id.clone(), ConversationCursor::new(2)),
            )),
        )
        .await
        .into_parts();
    a_replacement_wire.expect("handler A replacement subscription should answer");
    let a_replacement = handler_a
        .subscription_view(&thread_id)
        .await
        .expect("handler A replacement should be visible");
    assert_eq!(a_replacement.state(), SubscriptionState::Pending);
    let a_activated = handler_a
        .activate_after_response(a_replacement_receipt)
        .await
        .expect("handler A receipt should activate")
        .expect("handler A receipt should carry activation work");
    assert_eq!(a_activated.lease(), a_replacement.lease());
    assert_eq!(a_activated.cursor(), a_replacement.cursor());
    assert_eq!(
        handler_a
            .subscription_view(&thread_id)
            .await
            .expect("handler A active subscription should remain visible")
            .state(),
        SubscriptionState::Active
    );
    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn receipt_handler_unsubscribe_is_immediate_and_idempotent_without_receipt() {
    let (_temporary, storage) = opened_storage("receipt-unsubscribe").await;
    let repository = storage.repository();
    let thread_id = ThreadId::parse("thread-receipt-stop").expect("valid thread id");
    seed_conversation(repository, thread_id.as_str(), "receipt-stop").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());

    let (subscribe_wire, subscribe_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-stop-subscribe"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(thread_id.clone()),
            )),
        )
        .await
        .into_parts();
    subscribe_wire.expect("subscription should answer");

    let (stop_wire, stop_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-stop"),
            &ClientRequest::Conversation(ConversationRequest::Unsubscribe(
                ConversationUnsubscribe {
                    thread_id: thread_id.clone(),
                },
            )),
        )
        .await
        .into_parts();
    let stop = stop_wire.expect("stop should answer");
    assert!(stop_receipt.is_no_work());
    assert_eq!(
        stop.payload,
        ResponsePayload::ConversationSubscriptionStopped(
            artisan_protocol::ConversationSubscriptionStopped {
                thread_id: thread_id.clone(),
            }
        )
    );
    assert!(handler.subscription_view(&thread_id).await.is_none());
    assert_eq!(
        handler.activate_after_response(subscribe_receipt).await,
        Err(ActivateError::StaleLease)
    );

    let (repeat_wire, repeat_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-stop-repeat"),
            &ClientRequest::Conversation(ConversationRequest::Unsubscribe(
                ConversationUnsubscribe {
                    thread_id: thread_id.clone(),
                },
            )),
        )
        .await
        .into_parts();
    let repeat = repeat_wire.expect("repeated stop should answer");
    assert!(repeat_receipt.is_no_work());
    assert_eq!(repeat.payload, stop.payload);
    assert!(handler.subscription_view(&thread_id).await.is_none());

    let unknown_thread = ThreadId::parse("thread-receipt-stop-unknown").expect("valid thread id");
    let (unknown_wire, unknown_receipt) = handler
        .respond_with_receipt(
            &request("frame-receipt-stop-unknown"),
            &ClientRequest::Conversation(ConversationRequest::Unsubscribe(
                ConversationUnsubscribe {
                    thread_id: unknown_thread.clone(),
                },
            )),
        )
        .await
        .into_parts();
    let unknown = unknown_wire.expect("unknown stop should answer");
    assert!(unknown_receipt.is_no_work());
    assert_eq!(
        unknown.payload,
        ResponsePayload::ConversationSubscriptionStopped(
            artisan_protocol::ConversationSubscriptionStopped {
                thread_id: unknown_thread,
            }
        )
    );
    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn receipt_handler_non_subscription_query_and_command_match_respond_without_work() {
    let (_temporary, storage) = opened_storage("receipt-delegation").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "request-receipt-project",
            "directory-receipt-project",
            "project-receipt",
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input(
            "request-receipt-thread",
            "project-receipt",
            "thread-receipt-query",
        ))
        .await
        .expect("seed thread should persist");
    let handler = RequestHandler::with_subscriptions(repository.clone());

    let query = ClientRequest::Conversation(ConversationRequest::Query(ConversationQuery {
        thread_id: ThreadId::parse("thread-receipt-query").expect("valid thread id"),
        bounds: ConversationQueryBounds::Window {
            maximum_turn_count: QueryTurnCount::new(1).expect("valid count"),
        },
    }));
    let ordinary_query = handler
        .respond(&request("frame-receipt-query"), &query)
        .await;
    let (receipt_query, query_receipt) = handler
        .respond_with_receipt(&request("frame-receipt-query"), &query)
        .await
        .into_parts();
    assert_eq!(receipt_query, ordinary_query);
    assert!(query_receipt.is_no_work());

    let command = ClientRequest::Command(Command::AttachProject(artisan_domain::AttachProject {
        request_id: request("request-receipt-project"),
        directory_id: DirectoryId::parse("directory-receipt-project").expect("valid directory id"),
    }));
    let ordinary_command = handler
        .respond(&request("request-receipt-project"), &command)
        .await;
    let (receipt_command, command_receipt) = handler
        .respond_with_receipt(&request("request-receipt-project"), &command)
        .await
        .into_parts();
    assert_eq!(receipt_command, ordinary_command);
    assert!(command_receipt.is_no_work());
    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn supplied_registrar_observes_fresh_and_resume_after_activation() {
    let (_temporary, storage) = opened_storage("registrar-observes").await;
    let repository = storage.repository();
    let fresh_thread = ThreadId::parse("thread-registrar-fresh").expect("valid thread id");
    let resume_thread = ThreadId::parse("thread-registrar-resume").expect("valid thread id");
    seed_conversation(repository, fresh_thread.as_str(), "registrar-fresh").await;
    seed_conversation(repository, resume_thread.as_str(), "registrar-resume").await;

    let registrar = ConversationSubscriptionRegistrar::new();
    let retained_registrar = registrar.clone();
    let handler = RequestHandler::with_subscription_registrar(repository.clone(), registrar);

    let (fresh_wire, fresh_receipt) = handler
        .respond_with_receipt(
            &request("frame-registrar-fresh"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(fresh_thread.clone()),
            )),
        )
        .await
        .into_parts();
    let fresh_response = fresh_wire.expect("fresh subscription should answer");
    let ResponsePayload::ConversationSubscriptionStarted(ConversationSubscriptionStarted::Fresh(
        start,
    )) = fresh_response.payload
    else {
        panic!("expected a fresh subscription response");
    };
    let fresh_cursor = start.snapshot().cursor();
    let fresh_pending = retained_registrar
        .subscription_view(&fresh_thread)
        .await
        .expect("retained registrar should see fresh registration");
    assert_eq!(fresh_pending.state(), SubscriptionState::Pending);
    assert_eq!(fresh_pending.cursor(), fresh_cursor);

    let fresh_activated = handler
        .activate_after_response(fresh_receipt)
        .await
        .expect("fresh receipt should activate")
        .expect("fresh receipt should carry activation work");
    assert_eq!(fresh_activated.lease(), fresh_pending.lease());
    assert_eq!(fresh_activated.cursor(), fresh_cursor);
    let fresh_active = retained_registrar
        .subscription_view(&fresh_thread)
        .await
        .expect("retained registrar should see active fresh registration");
    assert_eq!(fresh_active.state(), SubscriptionState::Active);
    assert_eq!(fresh_active.cursor(), fresh_cursor);

    let resume_cursor = ConversationCursor::new(1);
    let (resume_wire, resume_receipt) = handler
        .respond_with_receipt(
            &request("frame-registrar-resume"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::resume(resume_thread.clone(), resume_cursor),
            )),
        )
        .await
        .into_parts();
    let resume_response = resume_wire.expect("resume subscription should answer");
    let ResponsePayload::ConversationSubscriptionStarted(
        ConversationSubscriptionStarted::Resumed { thread_id, cursor },
    ) = resume_response.payload
    else {
        panic!("expected a resumed subscription response");
    };
    assert_eq!(thread_id, resume_thread);
    assert_eq!(cursor, resume_cursor);
    let resume_pending = retained_registrar
        .subscription_view(&resume_thread)
        .await
        .expect("retained registrar should see resume registration");
    assert_eq!(resume_pending.state(), SubscriptionState::Pending);
    assert_eq!(resume_pending.cursor(), resume_cursor);

    let resume_activated = handler
        .activate_after_response(resume_receipt)
        .await
        .expect("resume receipt should activate")
        .expect("resume receipt should carry activation work");
    assert_eq!(resume_activated.lease(), resume_pending.lease());
    assert_eq!(resume_activated.cursor(), resume_cursor);
    let resume_active = retained_registrar
        .subscription_view(&resume_thread)
        .await
        .expect("retained registrar should see active resume registration");
    assert_eq!(resume_active.state(), SubscriptionState::Active);
    assert_eq!(resume_active.cursor(), resume_cursor);

    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn retained_registrar_records_publication_only_after_explicit_call() {
    let (_temporary, storage) = opened_storage("registrar-publication").await;
    let repository = storage.repository();
    let thread_id = ThreadId::parse("thread-registrar-publication").expect("valid thread id");
    seed_conversation(repository, thread_id.as_str(), "registrar-publication").await;

    let registrar = ConversationSubscriptionRegistrar::new();
    let retained_registrar = registrar.clone();
    let handler = RequestHandler::with_subscription_registrar(repository.clone(), registrar);
    let (wire, receipt) = handler
        .respond_with_receipt(
            &request("frame-registrar-publication"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(thread_id.clone()),
            )),
        )
        .await
        .into_parts();
    wire.expect("subscription should answer");
    let pending = retained_registrar
        .subscription_view(&thread_id)
        .await
        .expect("subscription should be visible");
    let lease = pending.lease().clone();
    let activated = handler
        .activate_after_response(receipt)
        .await
        .expect("subscription should activate")
        .expect("subscription should carry activation work");
    assert_eq!(activated.cursor(), ConversationCursor::new(2));

    let before_publication = handler
        .subscription_view(&thread_id)
        .await
        .expect("active subscription should be visible");
    assert_eq!(before_publication.state(), SubscriptionState::Active);
    assert_eq!(before_publication.cursor(), ConversationCursor::new(2));

    let batch = single_patch_batch(thread_id.clone(), 2, 3, "patch-registrar-publication");
    assert_eq!(
        handler
            .subscription_view(&thread_id)
            .await
            .expect("subscription should remain visible")
            .cursor(),
        ConversationCursor::new(2)
    );
    assert_eq!(
        retained_registrar
            .record_published_batch(&lease, &batch)
            .await,
        Ok(ConversationCursor::new(3))
    );
    assert_eq!(
        handler
            .subscription_view(&thread_id)
            .await
            .expect("published subscription should remain visible")
            .cursor(),
        ConversationCursor::new(3)
    );

    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn registrar_publication_preserves_pending_stale_and_thread_fences() {
    let (_temporary, storage) = opened_storage("registrar-publication-fences").await;
    let repository = storage.repository();
    let thread_a = ThreadId::parse("thread-registrar-fence-a").expect("valid thread id");
    let thread_b = ThreadId::parse("thread-registrar-fence-b").expect("valid thread id");
    seed_conversation(repository, thread_a.as_str(), "registrar-fence-a").await;
    seed_conversation(repository, thread_b.as_str(), "registrar-fence-b").await;

    let registrar = ConversationSubscriptionRegistrar::new();
    let handler =
        RequestHandler::with_subscription_registrar(repository.clone(), registrar.clone());

    let (first_wire, first_receipt) = subscription_request(
        &handler,
        "frame-registrar-fence-first",
        ConversationSubscribe::fresh(thread_a.clone()),
    )
    .await;
    first_wire.expect("first subscription should answer");
    let first_pending = registrar
        .subscription_view(&thread_a)
        .await
        .expect("first subscription should be visible");
    let first_lease = first_pending.lease().clone();
    let pending_batch = single_patch_batch(thread_a.clone(), 2, 3, "patch-registrar-pending");
    assert_batch_error(
        &registrar,
        &first_lease,
        &pending_batch,
        ApplyBatchError::NotActive,
        &thread_a,
        &first_pending,
    )
    .await;
    drop(first_receipt);

    let (replacement_wire, replacement_receipt) = subscription_request(
        &handler,
        "frame-registrar-fence-replacement",
        ConversationSubscribe::resume(thread_a.clone(), ConversationCursor::new(2)),
    )
    .await;
    replacement_wire.expect("replacement subscription should answer");
    let replacement_pending = registrar_view(&registrar, &thread_a).await;
    assert_ne!(first_pending.lease(), replacement_pending.lease());
    assert_batch_error(
        &registrar,
        &first_lease,
        &pending_batch,
        ApplyBatchError::StaleLease,
        &thread_a,
        &replacement_pending,
    )
    .await;
    let replacement_lease = replacement_pending.lease().clone();
    let replacement_activated = activate_subscription(&handler, replacement_receipt).await;
    assert_eq!(replacement_activated.cursor(), ConversationCursor::new(2));

    let (other_wire, other_receipt) = subscription_request(
        &handler,
        "frame-registrar-fence-other",
        ConversationSubscribe::fresh(thread_b.clone()),
    )
    .await;
    other_wire.expect("other subscription should answer");
    let other_activated = activate_subscription(&handler, other_receipt).await;
    assert_eq!(other_activated.cursor(), ConversationCursor::new(2));

    let before_a = registrar_view(&registrar, &thread_a).await;
    let before_b = registrar_view(&registrar, &thread_b).await;
    let thread_mismatch = single_patch_batch(thread_b.clone(), 2, 3, "patch-registrar-mismatch");
    assert_batch_error(
        &registrar,
        &replacement_lease,
        &thread_mismatch,
        ApplyBatchError::ThreadMismatch,
        &thread_a,
        &before_a,
    )
    .await;
    assert_eq!(registrar_view(&registrar, &thread_b).await, before_b);

    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn registrar_publication_preserves_duplicate_regression_and_gap_cursor_fences() {
    let (_temporary, storage) = opened_storage("registrar-cursor-fences").await;
    let repository = storage.repository();
    let thread_id = ThreadId::parse("thread-registrar-cursor-fence").expect("valid thread id");
    seed_conversation(repository, thread_id.as_str(), "registrar-cursor-fence").await;

    let registrar = ConversationSubscriptionRegistrar::new();
    let handler =
        RequestHandler::with_subscription_registrar(repository.clone(), registrar.clone());
    let (wire, receipt) = subscription_request(
        &handler,
        "frame-registrar-cursor-fence",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    wire.expect("subscription should answer");
    let activated = activate_subscription(&handler, receipt).await;
    assert_eq!(activated.cursor(), ConversationCursor::new(2));
    let lease = registrar_view(&registrar, &thread_id).await.lease().clone();

    let advance = single_patch_batch(thread_id.clone(), 2, 3, "patch-registrar-advance");
    assert_eq!(
        registrar.record_published_batch(&lease, &advance).await,
        Ok(ConversationCursor::new(3))
    );
    let after_advance = registrar_view(&registrar, &thread_id).await;
    assert_eq!(after_advance.state(), SubscriptionState::Active);
    assert_eq!(after_advance.cursor(), ConversationCursor::new(3));

    let duplicate = single_patch_batch(thread_id.clone(), 2, 3, "patch-registrar-duplicate");
    assert_batch_error(
        &registrar,
        &lease,
        &duplicate,
        ApplyBatchError::CursorMismatch {
            expected: ConversationCursor::new(3),
            actual: ConversationCursor::new(2),
        },
        &thread_id,
        &after_advance,
    )
    .await;

    let regression = single_patch_batch(thread_id.clone(), 1, 2, "patch-registrar-regression");
    assert_batch_error(
        &registrar,
        &lease,
        &regression,
        ApplyBatchError::CursorMismatch {
            expected: ConversationCursor::new(3),
            actual: ConversationCursor::new(1),
        },
        &thread_id,
        &after_advance,
    )
    .await;

    let gap = single_patch_batch(thread_id.clone(), 4, 5, "patch-registrar-gap");
    assert_batch_error(
        &registrar,
        &lease,
        &gap,
        ApplyBatchError::CursorMismatch {
            expected: ConversationCursor::new(3),
            actual: ConversationCursor::new(4),
        },
        &thread_id,
        &after_advance,
    )
    .await;

    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn shared_registrar_keeps_receipt_identity_private() {
    let (_temporary, storage) = opened_storage("registrar-shared-identity").await;
    let repository = storage.repository();
    let thread_a = ThreadId::parse("thread-registrar-shared-a").expect("valid thread id");
    let thread_b = ThreadId::parse("thread-registrar-shared-b").expect("valid thread id");
    seed_conversation(repository, thread_a.as_str(), "registrar-shared-a").await;
    seed_conversation(repository, thread_b.as_str(), "registrar-shared-b").await;

    let registrar = ConversationSubscriptionRegistrar::new();
    let handler_a =
        RequestHandler::with_subscription_registrar(repository.clone(), registrar.clone());
    let handler_b =
        RequestHandler::with_subscription_registrar(repository.clone(), registrar.clone());

    let (a_wire, a_receipt) = handler_a
        .respond_with_receipt(
            &request("frame-registrar-shared-a"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(thread_a.clone()),
            )),
        )
        .await
        .into_parts();
    a_wire.expect("handler A subscription should answer");
    let (a_other_wire, a_other_receipt) = handler_a
        .respond_with_receipt(
            &request("frame-registrar-shared-other"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(
                ConversationSubscribe::fresh(thread_b.clone()),
            )),
        )
        .await
        .into_parts();
    a_other_wire.expect("handler A second subscription should answer");

    let a_pending = registrar
        .subscription_view(&thread_a)
        .await
        .expect("shared registrar should see handler A subscription");
    assert_eq!(a_pending.state(), SubscriptionState::Pending);
    assert_eq!(
        handler_b.activate_after_response(a_receipt).await,
        Err(ActivateError::StaleLease)
    );
    assert_eq!(
        registrar.subscription_view(&thread_a).await,
        Some(a_pending.clone())
    );

    let a_other_activated = handler_a
        .activate_after_response(a_other_receipt)
        .await
        .expect("handler A should activate its own receipt")
        .expect("handler A receipt should carry activation work");
    assert_eq!(a_other_activated.cursor(), ConversationCursor::new(2));
    assert_eq!(
        handler_b
            .subscription_view(&thread_b)
            .await
            .expect("handler B should observe the shared active entry")
            .state(),
        SubscriptionState::Active
    );

    storage.close().await.expect("storage should close");
}

#[tokio::test]
async fn handler_unsubscribe_and_replacement_stale_retained_publication_leases() {
    let (_temporary, storage) = opened_storage("registrar-stale-retained").await;
    let repository = storage.repository();
    let thread_id = ThreadId::parse("thread-registrar-stale-retained").expect("valid thread id");
    seed_conversation(repository, thread_id.as_str(), "registrar-stale-retained").await;

    let registrar = ConversationSubscriptionRegistrar::new();
    let retained_registrar = registrar.clone();
    let handler = RequestHandler::with_subscription_registrar(repository.clone(), registrar);
    let (wire, receipt) = subscription_request(
        &handler,
        "frame-registrar-stale-initial",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    wire.expect("initial subscription should answer");
    let initial_activated = activate_subscription(&handler, receipt).await;
    assert_eq!(initial_activated.cursor(), ConversationCursor::new(2));
    let initial_lease = registrar_view(&retained_registrar, &thread_id)
        .await
        .lease()
        .clone();
    let initial_batch =
        single_patch_batch(thread_id.clone(), 2, 3, "patch-registrar-stale-unsubscribe");

    let (stop_wire, stop_receipt) = unsubscribe_request(
        &handler,
        "frame-registrar-stale-unsubscribe",
        thread_id.clone(),
    )
    .await;
    stop_wire.expect("unsubscribe should answer");
    assert!(stop_receipt.is_no_work());
    assert!(
        retained_registrar
            .subscription_view(&thread_id)
            .await
            .is_none()
    );
    assert_eq!(
        retained_registrar
            .record_published_batch(&initial_lease, &initial_batch)
            .await,
        Err(ApplyBatchError::StaleLease)
    );

    let (second_wire, second_receipt) = subscription_request(
        &handler,
        "frame-registrar-stale-second",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    second_wire.expect("second subscription should answer");
    let second_activated = activate_subscription(&handler, second_receipt).await;
    assert_eq!(second_activated.cursor(), ConversationCursor::new(2));
    let replaced_lease = registrar_view(&retained_registrar, &thread_id)
        .await
        .lease()
        .clone();

    let (replacement_wire, replacement_receipt) = subscription_request(
        &handler,
        "frame-registrar-stale-replacement",
        ConversationSubscribe::resume(thread_id.clone(), ConversationCursor::new(2)),
    )
    .await;
    replacement_wire.expect("replacement subscription should answer");
    let replacement = registrar_view(&retained_registrar, &thread_id).await;
    assert_eq!(replacement.state(), SubscriptionState::Pending);
    assert_ne!(replacement.lease(), &replaced_lease);
    let replacement_batch =
        single_patch_batch(thread_id.clone(), 2, 3, "patch-registrar-stale-replacement");
    assert_eq!(
        retained_registrar
            .record_published_batch(&replaced_lease, &replacement_batch)
            .await,
        Err(ApplyBatchError::StaleLease)
    );
    assert_eq!(
        retained_registrar.subscription_view(&thread_id).await,
        Some(replacement)
    );
    drop(replacement_receipt);

    storage.close().await.expect("storage should close");
}

fn read_thread_engine_settings_request(thread_id: &str) -> ClientRequest {
    ClientRequest::Query(artisan_domain::Query::ReadThreadEngineSettings(
        artisan_domain::commands::ReadThreadEngineSettings::new(
            ThreadId::parse(thread_id).expect("valid thread id"),
        ),
    ))
}

#[tokio::test]
async fn read_thread_engine_settings_for_unconfigured_thread_returns_unconfigured() {
    let (_temporary, storage) = opened_storage("read-unconfigured").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "read-project-unconfigured",
            "directory-read-unconfigured",
            "project-read-unconfigured",
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input(
            "read-thread-unconfigured",
            "project-read-unconfigured",
            "thread-read-unconfigured",
        ))
        .await
        .expect("seed thread should persist");
    let handler = RequestHandler::new(repository.clone());

    let response = handler
        .respond(
            &request("frame-read-unconfigured"),
            &read_thread_engine_settings_request("thread-read-unconfigured"),
        )
        .await
        .expect("unconfigured read should succeed");

    storage.close().await.expect("storage should close");

    assert_eq!(response.request_id, request("frame-read-unconfigured"));
    match response.payload {
        ResponsePayload::ThreadEngineSettings(
            artisan_protocol::ThreadEngineSettingsResult::Unconfigured { thread_id },
        ) => {
            assert_eq!(thread_id.as_str(), "thread-read-unconfigured");
        }
        other => panic!("expected unconfigured thread engine settings, got {other:?}"),
    }
}

#[tokio::test]
async fn read_thread_engine_settings_for_configured_thread_returns_exact_revision_and_config() {
    let (_temporary, storage) = opened_storage("read-configured").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "read-project-configured",
            "directory-read-configured",
            "project-read-configured",
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input(
            "read-thread-configured",
            "project-read-configured",
            "thread-read-configured",
        ))
        .await
        .expect("seed thread should persist");
    let expected_config = engine_config("read-configured");
    let stored = repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request("request-engine-configured"),
            thread_id: ThreadId::parse("thread-read-configured").expect("valid thread id"),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: expected_config.clone(),
            accepted_at: UnixMillis::from_millis(400),
        })
        .await
        .expect("seed engine config should persist");
    let handler = RequestHandler::new(repository.clone());

    let response = handler
        .respond(
            &request("frame-read-configured"),
            &read_thread_engine_settings_request("thread-read-configured"),
        )
        .await
        .expect("configured read should succeed");

    storage.close().await.expect("storage should close");

    assert_eq!(response.request_id, request("frame-read-configured"));
    match response.payload {
        ResponsePayload::ThreadEngineSettings(
            artisan_protocol::ThreadEngineSettingsResult::Configured {
                thread_id,
                revision,
                config,
            },
        ) => {
            assert_eq!(thread_id.as_str(), "thread-read-configured");
            assert_eq!(revision, stored.revision());
            assert_eq!(*config, expected_config);
        }
        other => panic!("expected configured thread engine settings, got {other:?}"),
    }
}

#[tokio::test]
async fn read_thread_engine_settings_for_missing_thread_fails_thread_unknown() {
    let (_temporary, storage) = opened_storage("read-missing").await;
    let handler = RequestHandler::new(storage.repository().clone());

    let failure = failure_of(
        handler
            .respond(
                &request("frame-read-missing"),
                &read_thread_engine_settings_request("thread-missing"),
            )
            .await,
    );

    storage.close().await.expect("storage should close");

    assert_eq!(failure.code, ErrorCode::ThreadUnknown);
    assert!(!failure.retryable);
    assert_eq!(failure.request_id, Some(request("frame-read-missing")));
}

#[tokio::test]
async fn read_thread_engine_settings_response_request_id_equals_triggering_frame_and_no_origin_consult()
 {
    let (_temporary, storage) = opened_storage("read-correlation").await;
    let repository = storage.repository();
    repository
        .attach_project(attach_input(
            "read-project-correlation",
            "directory-read-correlation",
            "project-read-correlation",
        ))
        .await
        .expect("seed attach should persist");
    repository
        .create_thread(create_input(
            "read-thread-correlation",
            "project-read-correlation",
            "thread-read-correlation",
        ))
        .await
        .expect("seed thread should persist");
    let origin = ScriptedOriginHandle::scripted(Vec::new(), Vec::new());
    let handler = scripted_handler(&storage, &origin);

    let response = handler
        .respond(
            &request("frame-read-correlation"),
            &read_thread_engine_settings_request("thread-read-correlation"),
        )
        .await
        .expect("pure read should succeed without origin");

    storage.close().await.expect("storage should close");

    assert_eq!(response.request_id, request("frame-read-correlation"));
    assert_eq!(origin.identity_calls(), 0);
    assert_eq!(origin.instant_calls(), 0);
    assert!(matches!(
        response.payload,
        ResponsePayload::ThreadEngineSettings(_)
    ));
}
