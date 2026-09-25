//! Private handler tests for Forge-owned submissions: retrying a failed
//! message by identity, moving it into a new thread, and the edit
//! withdrawal that recalls a queued payload into the Forge draft.
//!
//! Path-linked from `request_handler.rs` so the tests call the handler
//! methods directly.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use artisan_database::{
    AttachProjectInput, ClaimMessageDispatch, CreateThreadInput, DispatchFailureReason,
    DispatchLeaseOwner, FailMessageDispatch, QueueMessageInput, Repository,
    SetThreadEngineConfigInput, SqliteConfig,
};
use artisan_domain::{
    ApprovalMode, AuthoredText, ByteLimit, ComposerDraftScope, CountLimit, DirectoryId,
    DisplayName, EngineAgentId, EngineConfigUpdatePrecondition, EngineModelId,
    EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FailedMessageRecovered, FailedMessageRetryOutcome,
    FailedMessageTarget, FilesystemAccess, FiniteMillis, ImageAttachment, ListFailedMessages,
    ListQueuedMessages, MessageId, NetworkAccess, OpenCode2Selection, PermissionId, ProjectId,
    QueueMessagePayload, QueuedMessageListOrder, ReceiptDisposition, RecoverFailedMessage,
    RequestId, RetryFailedMessage, RootPath, ThreadId, ThreadTitle, UnixMillis, WebSearchAccess,
    WithdrawQueuedMessageCommand,
};
use artisan_protocol::{ResponsePayload, ServerResponse};

use crate::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, ForgeStorage, RequestHandler,
};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TemporaryDatabase {
    directory: PathBuf,
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.directory);
    }
}

async fn storage(label: &str) -> (TemporaryDatabase, ForgeStorage) {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "artisan-forge-failed-messages-{label}-{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("temporary directory");
    let storage =
        ForgeStorage::open(SqliteConfig::file(directory.join("forge.sqlite3")).sqlx_logging(false))
            .await
            .expect("storage should open and migrate");
    (TemporaryDatabase { directory }, storage)
}

/// Mints sequential identities and strictly increasing instants.
#[derive(Debug, Default)]
struct CountingOrigin {
    identities: AtomicUsize,
    clock: AtomicI64,
}

impl CommandOrigin for CountingOrigin {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        Ok(format!(
            "minted-{}",
            self.identities.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        Ok(UnixMillis::from_millis(
            1_000 + self.clock.fetch_add(10, Ordering::Relaxed),
        ))
    }
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).unwrap()
}

fn thread() -> ThreadId {
    ThreadId::parse("thread-failed").unwrap()
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
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-failed").unwrap(),
            EngineModelId::parse("model-failed").unwrap(),
            EngineRouteId::parse("route-failed").unwrap(),
            None,
            EnginePermissionPolicy::new(
                PermissionId::parse("permission-failed").unwrap(),
                EngineAgentId::parse("agent-failed").unwrap(),
                ApprovalMode::OnRequest,
                FilesystemAccess::Workspace,
                NetworkAccess::Enabled,
                WebSearchAccess::Disabled,
            ),
        )),
        runtime,
    )
}

fn payload() -> QueueMessagePayload {
    QueueMessagePayload::new(
        Some(AuthoredText::parse("move me").unwrap()),
        vec![ImageAttachment::new("image/png", vec![1, 2, 3], "shot.png").unwrap()],
    )
    .unwrap()
}

/// A configured thread with one queued message `message-1`.
async fn seed(repository: &Repository) -> FailedMessageTarget {
    repository
        .attach_project(AttachProjectInput {
            request_id: request("attach"),
            directory_id: DirectoryId::parse("directory").unwrap(),
            project_id: ProjectId::parse("project-failed").unwrap(),
            root_path: RootPath::parse("C:/repos/failed").unwrap(),
            display_name: DisplayName::parse("Failed").unwrap(),
            attached_at: UnixMillis::from_millis(100),
        })
        .await
        .unwrap();
    repository
        .create_thread(CreateThreadInput {
            request_id: request("create"),
            thread_id: thread(),
            project_id: ProjectId::parse("project-failed").unwrap(),
            title: ThreadTitle::parse("Old").unwrap(),
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await
        .unwrap();
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request("config"),
            thread_id: thread(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: engine_config(),
            accepted_at: UnixMillis::from_millis(250),
        })
        .await
        .unwrap();
    repository
        .queue_message(QueueMessageInput {
            request_id: request("queue-1"),
            message_id: MessageId::parse("message-1").unwrap(),
            thread_id: thread(),
            payload: payload(),
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(300),
        })
        .await
        .unwrap();
    FailedMessageTarget {
        thread_id: thread(),
        message_id: MessageId::parse("message-1").unwrap(),
        original_request_id: request("queue-1"),
    }
}

async fn fail_first(repository: &Repository) {
    let owner = || DispatchLeaseOwner::new([0x31; 32]);
    repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: owner(),
            claimed_at: UnixMillis::from_millis(400),
            lease_expires_at: UnixMillis::from_millis(900),
        })
        .await
        .unwrap()
        .unwrap();
    repository
        .fail_message_dispatch(FailMessageDispatch {
            message_id: MessageId::parse("message-1").unwrap(),
            owner: owner(),
            operated_at: UnixMillis::from_millis(500),
            reason: DispatchFailureReason::parse("engine profile unavailable").unwrap(),
        })
        .await
        .unwrap();
}

fn handler(storage: &ForgeStorage) -> RequestHandler {
    RequestHandler::with_origin(
        storage.repository().clone(),
        Box::new(CountingOrigin::default()),
    )
}

fn recovered_of(response: ServerResponse) -> FailedMessageRecovered {
    match response.payload {
        ResponsePayload::FailedMessageRecovered(result) => result,
        other => panic!("expected a recovery answer, got {other:?}"),
    }
}

async fn failed_count(repository: &Repository) -> usize {
    repository
        .read_failed_messages(ListFailedMessages::new(thread(), 32).unwrap())
        .await
        .unwrap()
        .messages()
        .len()
}

#[tokio::test]
async fn retry_requeues_a_failed_message_by_identity() {
    let (_temporary, storage) = storage("retry").await;
    let repository = storage.repository();
    let target = seed(repository).await;
    fail_first(repository).await;
    let handler = handler(&storage);

    let retry = RetryFailedMessage {
        request_id: request("retry-1"),
        target: target.clone(),
    };
    let response = handler
        .retry_failed_message_outcome(&retry.request_id, &retry)
        .await
        .unwrap();
    let ResponsePayload::FailedMessageRetried(answer) = response.payload else {
        panic!("expected a retry answer");
    };
    assert_eq!(answer.outcome, FailedMessageRetryOutcome::Requeued);
    assert_eq!(answer.target, target);
    assert_eq!(failed_count(repository).await, 0);
    let queued = repository
        .read_queued_messages(
            ListQueuedMessages::new(thread(), QueuedMessageListOrder::OldestFirst, 32).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        queued.messages().len(),
        1,
        "the stored payload is queued again"
    );

    let again = RetryFailedMessage {
        request_id: request("retry-2"),
        target,
    };
    let response = handler
        .retry_failed_message_outcome(&again.request_id, &again)
        .await
        .unwrap();
    let ResponsePayload::FailedMessageRetried(answer) = response.payload else {
        panic!("expected a retry answer");
    };
    assert_eq!(answer.outcome, FailedMessageRetryOutcome::NotRetryable);
}

#[tokio::test]
async fn recovery_moves_the_prompt_and_configuration_into_a_new_thread() {
    let (_temporary, storage) = storage("recover").await;
    let repository = storage.repository();
    let target = seed(repository).await;
    fail_first(repository).await;
    let handler = handler(&storage);

    let recover = RecoverFailedMessage {
        request_id: request("recover-1"),
        target: target.clone(),
    };
    let answer = recovered_of(
        handler
            .recover_failed_message_outcome(&recover.request_id, &recover)
            .await
            .unwrap(),
    );
    assert_eq!(answer.disposition, ReceiptDisposition::Accepted);
    let new_thread = answer.new_thread_id.expect("a new thread holds the prompt");
    assert_ne!(new_thread, thread());

    let draft = repository
        .read_composer_draft(&ComposerDraftScope::Thread(new_thread.clone()))
        .await
        .unwrap()
        .expect("the prompt is the new thread's draft");
    assert_eq!(draft.text().as_str(), "move me");
    assert_eq!(draft.attachments().len(), 1);
    assert_eq!(draft.attachments()[0].name(), "shot.png");
    let settings = repository
        .read_thread_engine_settings(&new_thread)
        .await
        .unwrap()
        .expect("the failed message's configuration is copied");
    assert_eq!(settings.config(), &engine_config());
    assert_eq!(
        failed_count(repository).await,
        0,
        "the failure is no longer offered"
    );
    assert_eq!(
        repository.read_thread_project(&new_thread).await.unwrap(),
        ProjectId::parse("project-failed").unwrap()
    );

    for request_value in ["recover-1", "recover-2"] {
        let replay = RecoverFailedMessage {
            request_id: request(request_value),
            target: target.clone(),
        };
        let answer = recovered_of(
            handler
                .recover_failed_message_outcome(&replay.request_id, &replay)
                .await
                .unwrap(),
        );
        assert_eq!(answer.disposition, ReceiptDisposition::Duplicate);
        assert_eq!(answer.new_thread_id.as_ref(), Some(&new_thread));
    }
}

#[tokio::test]
async fn recovery_of_a_live_message_moves_nothing() {
    let (_temporary, storage) = storage("recover-live").await;
    let repository = storage.repository();
    let target = seed(repository).await;
    let handler = handler(&storage);
    let recover = RecoverFailedMessage {
        request_id: request("recover-live"),
        target,
    };
    let answer = recovered_of(
        handler
            .recover_failed_message_outcome(&recover.request_id, &recover)
            .await
            .unwrap(),
    );
    assert_eq!(answer.new_thread_id, None);
    assert_eq!(
        repository
            .list_threads(&ProjectId::parse("project-failed").unwrap())
            .await
            .unwrap()
            .threads()
            .len(),
        1,
        "no thread was created"
    );
}

#[tokio::test]
async fn edit_withdrawal_recalls_the_payload_into_the_forge_draft() {
    let (_temporary, storage) = storage("recall").await;
    let repository = storage.repository();
    let target = seed(repository).await;
    let handler = handler(&storage);
    let command = WithdrawQueuedMessageCommand::new(
        request("withdraw-edit"),
        target.thread_id.clone(),
        target.message_id.clone(),
        target.original_request_id.clone(),
    )
    .recalling_to_draft();
    handler
        .withdraw_composer_message(&command.request_id, &command)
        .await
        .unwrap();
    let draft = repository
        .read_composer_draft(&ComposerDraftScope::Thread(thread()))
        .await
        .unwrap()
        .expect("the withdrawn prompt is the thread's draft");
    assert_eq!(draft.text().as_str(), "move me");
    assert_eq!(draft.attachments().len(), 1);
}

#[tokio::test]
async fn discard_withdrawal_leaves_the_draft_alone() {
    let (_temporary, storage) = storage("discard").await;
    let repository = storage.repository();
    let target = seed(repository).await;
    let handler = handler(&storage);
    let command = WithdrawQueuedMessageCommand::new(
        request("withdraw-discard"),
        target.thread_id,
        target.message_id,
        target.original_request_id,
    );
    handler
        .withdraw_composer_message(&command.request_id, &command)
        .await
        .unwrap();
    assert!(
        repository
            .read_composer_draft(&ComposerDraftScope::Thread(thread()))
            .await
            .unwrap()
            .is_none()
    );
}
