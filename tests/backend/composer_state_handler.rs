//! Focused private tests for the composer-state request-handler leaf.
//!
//! This file is path-linked from `request_handler.rs`, so it can call the
//! child helper methods without widening the shipping handler API or adding
//! parent dispatch registrations before the root integration packet is ready.

use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use artisan_database::{
    AttachProjectInput, BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch,
    CreateThreadInput, DispatchLeaseOwner, LaunchClaimedRun, ProviderBindingBytes,
    QueueMessageInput, Repository, RunLaunchCredentials, RunStartKey, SetThreadEngineConfigInput,
    SqliteConfig,
};
use artisan_domain::{
    ApprovalMode, AuthoredText, ByteLimit, CountLimit, DirectoryId, DisplayName, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, ImageAttachment, ItemId, ListQueuedMessages,
    MessageId, NetworkAccess, OpenCode2Selection, PatchId, QueueMessagePayload,
    QueuedMessageListOrder, QueuedMessageWithdrawalOutcome, ReadRecalledMessage, ReadRunUsage,
    ReceiptDisposition, RequestId, RootPath, RunId, ThreadId, ThreadTitle, TurnId, UnixMillis,
    WebSearchAccess, WithdrawQueuedMessageCommand, WithdrawQueuedMessageResult,
};
use artisan_protocol::{ErrorCode, ProtocolFailure, ResponsePayload, ServerResponse};

use crate::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, ForgeStorage, RequestHandler,
};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TemporaryDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "artisan-forge-composer-state-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("temporary database directory should be created");
        let database = directory.join("forge.sqlite3");
        Self {
            directory,
            database,
        }
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.directory);
    }
}

async fn opened_storage(label: &str) -> (TemporaryDatabase, ForgeStorage) {
    let temporary = TemporaryDatabase::new(label);
    let storage = ForgeStorage::open(SqliteConfig::file(&temporary.database).sqlx_logging(false))
        .await
        .expect("Forge storage should open and migrate");
    (temporary, storage)
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).expect("test request id should be valid")
}

fn thread(value: &str) -> ThreadId {
    ThreadId::parse(value).expect("test thread id should be valid")
}

fn message(value: &str) -> MessageId {
    MessageId::parse(value).expect("test message id should be valid")
}

fn queue_payload() -> QueueMessagePayload {
    QueueMessagePayload::text_only("queued composer text").expect("test payload should be valid")
}

async fn seed_thread(repository: &Repository, label: &str) -> ThreadId {
    let project_id = format!("project-{label}");
    let thread_id = thread(&format!("thread-{label}"));
    repository
        .attach_project(AttachProjectInput {
            request_id: request(&format!("attach-{label}")),
            directory_id: DirectoryId::parse(format!("directory-{label}"))
                .expect("directory id should be valid"),
            project_id: artisan_domain::ProjectId::parse(&project_id)
                .expect("project id should be valid"),
            root_path: RootPath::parse(format!("C:/repos/{project_id}"))
                .expect("root path should be valid"),
            display_name: DisplayName::parse("Composer test project")
                .expect("display name should be valid"),
            attached_at: UnixMillis::from_millis(100),
        })
        .await
        .expect("test project should persist");
    repository
        .create_thread(CreateThreadInput {
            request_id: request(&format!("create-thread-{label}")),
            thread_id: thread_id.clone(),
            project_id: artisan_domain::ProjectId::parse(project_id)
                .expect("project id should be valid"),
            title: ThreadTitle::parse("Composer test thread").expect("title should be valid"),
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await
        .expect("test thread should persist");
    thread_id
}

async fn seed_message(
    repository: &Repository,
    label: &str,
    payload: QueueMessagePayload,
) -> (ThreadId, MessageId, RequestId) {
    let thread_id = seed_thread(repository, label).await;
    let message_id = message(&format!("message-{label}"));
    let original_request_id = request(&format!("queue-{label}"));
    repository
        .queue_message(QueueMessageInput {
            request_id: original_request_id.clone(),
            message_id: message_id.clone(),
            thread_id: thread_id.clone(),
            payload,
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(300),
        })
        .await
        .expect("test queued message should persist");
    (thread_id, message_id, original_request_id)
}

#[derive(Debug)]
struct ScriptedOrigin {
    instants: Mutex<VecDeque<Result<UnixMillis, CommandOriginClockError>>>,
    instant_calls: AtomicUsize,
}

impl ScriptedOrigin {
    fn new(
        instants: impl IntoIterator<Item = Result<UnixMillis, CommandOriginClockError>>,
    ) -> Self {
        Self {
            instants: Mutex::new(instants.into_iter().collect()),
            instant_calls: AtomicUsize::new(0),
        }
    }

    fn instant_calls(&self) -> usize {
        self.instant_calls.load(Ordering::Relaxed)
    }
}

impl CommandOrigin for ScriptedOrigin {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        panic!("composer-state withdrawal helper must not mint an identity")
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        self.instant_calls.fetch_add(1, Ordering::Relaxed);
        self.instants
            .lock()
            .expect("origin script mutex should not be poisoned")
            .pop_front()
            .expect("origin instant script should cover every fresh withdrawal")
    }
}

#[derive(Clone, Debug)]
struct ScriptedOriginHandle(Arc<ScriptedOrigin>);

impl ScriptedOriginHandle {
    fn new(
        instants: impl IntoIterator<Item = Result<UnixMillis, CommandOriginClockError>>,
    ) -> Self {
        Self(Arc::new(ScriptedOrigin::new(instants)))
    }

    fn instant_calls(&self) -> usize {
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

fn handler(storage: &ForgeStorage, origin: &ScriptedOriginHandle) -> RequestHandler {
    RequestHandler::with_origin(storage.repository().clone(), Box::new(origin.clone()))
}

fn withdrawal_of(response: ServerResponse) -> WithdrawQueuedMessageResult {
    match response.payload {
        ResponsePayload::MessageWithdrawn(result) => result,
        other => panic!("expected withdrawal response, got {other:?}"),
    }
}

fn recalled_of(response: ServerResponse) -> artisan_domain::RecalledMessageResult {
    match response.payload {
        ResponsePayload::RecalledMessage(result) => result,
        other => panic!("expected recalled-message response, got {other:?}"),
    }
}

fn failure_of(result: Result<ServerResponse, ProtocolFailure>) -> ProtocolFailure {
    result.expect_err("expected a typed protocol failure")
}

fn withdrawal_command(
    request_id: &str,
    thread_id: &ThreadId,
    message_id: &MessageId,
    original_request_id: &RequestId,
) -> WithdrawQueuedMessageCommand {
    WithdrawQueuedMessageCommand::new(
        request(request_id),
        thread_id.clone(),
        message_id.clone(),
        original_request_id.clone(),
    )
}

fn image(byte: u8, name: &str) -> ImageAttachment {
    ImageAttachment::new("image/png", vec![byte], name).expect("test image should be valid")
}

#[tokio::test]
async fn queue_read_is_repository_backed_and_does_not_consult_admission() {
    let (_temporary, storage) = opened_storage("queue-read").await;
    let repository = storage.repository();
    let (thread_id, message_id, original_request_id) =
        seed_message(repository, "queue-read", queue_payload()).await;
    let origin = ScriptedOriginHandle::new([]);
    let handler = handler(&storage, &origin);

    let response = handler
        .read_composer_queue(
            &request("read-queue"),
            &ListQueuedMessages::new(thread_id.clone(), QueuedMessageListOrder::OldestFirst, 1)
                .expect("bounded queue query should be valid"),
        )
        .await
        .expect("repository-backed queue read should succeed");
    match response.payload {
        ResponsePayload::QueuedMessages(listing) => {
            assert_eq!(listing.thread_id(), &thread_id);
            assert_eq!(listing.messages().len(), 1);
            assert_eq!(listing.messages()[0].message_id, message_id);
            assert_eq!(
                listing.messages()[0].original_request_id,
                original_request_id
            );
        }
        other => panic!("expected queued-message listing, got {other:?}"),
    }
    assert_eq!(origin.instant_calls(), 0);
}

#[tokio::test]
async fn withdrawal_replay_precedes_clock_and_replays_forge_timestamp() {
    let (_temporary, storage) = opened_storage("withdrawal-replay").await;
    let repository = storage.repository();
    let (thread_id, message_id, original_request_id) =
        seed_message(repository, "withdrawal-replay", queue_payload()).await;
    let withdrawal = withdrawal_command(
        "withdrawal-replay-request",
        &thread_id,
        &message_id,
        &original_request_id,
    );
    let origin = ScriptedOriginHandle::new([
        Ok(UnixMillis::from_millis(400)),
        Err(CommandOriginClockError),
    ]);
    let handler = handler(&storage, &origin);

    let first = withdrawal_of(
        handler
            .withdraw_composer_message(&withdrawal.request_id, &withdrawal)
            .await
            .expect("fresh withdrawal should succeed"),
    );
    let replay = withdrawal_of(
        handler
            .withdraw_composer_message(&withdrawal.request_id, &withdrawal)
            .await
            .expect("exact replay should not consult the failing clock"),
    );

    assert_eq!(first.receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(replay.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(first.accepted_at, UnixMillis::from_millis(400));
    assert_eq!(replay.accepted_at, first.accepted_at);
    assert_eq!(replay.outcome, QueuedMessageWithdrawalOutcome::Withdrawn);
    assert_eq!(origin.instant_calls(), 1);
}

#[tokio::test]
async fn changed_withdrawal_identities_are_conflicts_before_clock_access() {
    let (_temporary, storage) = opened_storage("withdrawal-conflict").await;
    let repository = storage.repository();
    let (thread_id, message_id, original_request_id) =
        seed_message(repository, "withdrawal-conflict", queue_payload()).await;
    let withdrawal = withdrawal_command(
        "withdrawal-conflict-request",
        &thread_id,
        &message_id,
        &original_request_id,
    );
    let origin = ScriptedOriginHandle::new([Ok(UnixMillis::from_millis(400))]);
    let handler = handler(&storage, &origin);
    withdrawal_of(
        handler
            .withdraw_composer_message(&withdrawal.request_id, &withdrawal)
            .await
            .expect("initial withdrawal should succeed"),
    );

    let mut changed_message = withdrawal.clone();
    changed_message.message_id = message("message-withdrawal-conflict-other");
    let mut changed_thread = withdrawal.clone();
    changed_thread.thread_id = thread("thread-withdrawal-conflict-other");
    let mut changed_original = withdrawal.clone();
    changed_original.original_request_id = request("queue-withdrawal-conflict-other");

    for changed in [changed_message, changed_thread, changed_original] {
        let failure = failure_of(
            handler
                .withdraw_composer_message(&withdrawal.request_id, &changed)
                .await,
        );
        assert_eq!(failure.code, ErrorCode::IdempotencyConflict);
        assert!(!failure.retryable);
    }
    assert_eq!(origin.instant_calls(), 1);
}

#[tokio::test]
async fn successful_withdrawal_commits_before_read_and_preserves_image_payload_presence() {
    let (_temporary, storage) = opened_storage("withdrawal-payload").await;
    let repository = storage.repository();
    let image_only = QueueMessagePayload::new(None, vec![image(1, "only.png")])
        .expect("image-only payload should be valid");
    let present_empty = QueueMessagePayload::new(
        Some(AuthoredText::empty()),
        vec![image(2, "empty-text.png")],
    )
    .expect("present-empty payload should be valid");
    let first = seed_message(repository, "withdrawal-image-only", image_only.clone()).await;
    let second = seed_message(
        repository,
        "withdrawal-present-empty",
        present_empty.clone(),
    )
    .await;
    let origin = ScriptedOriginHandle::new([
        Ok(UnixMillis::from_millis(400)),
        Ok(UnixMillis::from_millis(401)),
    ]);
    let handler = handler(&storage, &origin);

    for (label, (thread_id, message_id, original_request_id), expected) in [
        ("image-only", first, image_only),
        ("present-empty", second, present_empty),
    ] {
        let withdrawal = withdrawal_command(
            &format!("withdrawal-payload-{label}"),
            &thread_id,
            &message_id,
            &original_request_id,
        );
        let receipt = withdrawal_of(
            handler
                .withdraw_composer_message(&withdrawal.request_id, &withdrawal)
                .await
                .expect("withdrawal should commit"),
        );
        assert_eq!(receipt.outcome, QueuedMessageWithdrawalOutcome::Withdrawn);

        let recalled = recalled_of(
            handler
                .read_recalled_composer_message(
                    &request(&format!("read-payload-{label}")),
                    &ReadRecalledMessage::new(thread_id, message_id, original_request_id),
                )
                .await
                .expect("committed withdrawal payload should be readable"),
        );
        assert_eq!(recalled.payload, Some(expected.clone()));
        if label == "present-empty" {
            let payload = recalled
                .payload
                .expect("present-empty payload should be present");
            assert_eq!(
                payload
                    .text()
                    .expect("empty text presence should survive")
                    .as_str(),
                ""
            );
            assert_eq!(payload.attachments().len(), 1);
        } else {
            let payload = recalled
                .payload
                .expect("image-only payload should be present");
            assert!(payload.text().is_none());
            assert!(payload.is_image_only());
        }
    }
}

#[tokio::test]
async fn wrong_thread_recalled_reads_return_no_payload() {
    let (_temporary, storage) = opened_storage("recalled-wrong-thread").await;
    let repository = storage.repository();
    let (owner_thread, message_id, original_request_id) =
        seed_message(repository, "recalled-owner", queue_payload()).await;
    let wrong_thread = seed_thread(repository, "recalled-wrong-thread").await;
    let origin = ScriptedOriginHandle::new([Ok(UnixMillis::from_millis(400))]);
    let handler = handler(&storage, &origin);
    let withdrawal = withdrawal_command(
        "withdrawal-wrong-thread",
        &owner_thread,
        &message_id,
        &original_request_id,
    );
    withdrawal_of(
        handler
            .withdraw_composer_message(&withdrawal.request_id, &withdrawal)
            .await
            .expect("owner withdrawal should succeed"),
    );

    let recalled = recalled_of(
        handler
            .read_recalled_composer_message(
                &request("read-wrong-thread"),
                &ReadRecalledMessage::new(wrong_thread.clone(), message_id, original_request_id),
            )
            .await
            .expect("wrong-thread read should answer without exposing bytes"),
    );
    assert_eq!(recalled.thread_id, wrong_thread);
    assert!(recalled.payload.is_none());
}

#[tokio::test]
async fn too_late_and_not_queued_are_durable_typed_outcomes() {
    let (_temporary, storage) = opened_storage("withdrawal-outcomes").await;
    let repository = storage.repository();
    let (late_thread, late_message, late_original) =
        seed_message(repository, "withdrawal-too-late", queue_payload()).await;
    repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x11; 32]),
            claimed_at: UnixMillis::from_millis(400),
            lease_expires_at: UnixMillis::from_millis(900),
        })
        .await
        .expect("dispatch claim should persist");

    let (not_queued_thread, not_queued_message, not_queued_original) =
        seed_message(repository, "withdrawal-not-queued", queue_payload()).await;
    let origin = ScriptedOriginHandle::new([
        Ok(UnixMillis::from_millis(500)),
        Ok(UnixMillis::from_millis(501)),
        Ok(UnixMillis::from_millis(502)),
    ]);
    let handler = handler(&storage, &origin);

    let late = withdrawal_command(
        "withdrawal-too-late-request",
        &late_thread,
        &late_message,
        &late_original,
    );
    let late_result = withdrawal_of(
        handler
            .withdraw_composer_message(&late.request_id, &late)
            .await
            .expect("too-late outcome should be a successful durable response"),
    );
    assert_eq!(
        late_result.receipt.disposition,
        ReceiptDisposition::Accepted
    );
    assert_eq!(late_result.outcome, QueuedMessageWithdrawalOutcome::TooLate);

    let first_not_queued = withdrawal_command(
        "withdrawal-not-queued-first",
        &not_queued_thread,
        &not_queued_message,
        &not_queued_original,
    );
    let first_result = withdrawal_of(
        handler
            .withdraw_composer_message(&first_not_queued.request_id, &first_not_queued)
            .await
            .expect("first not-queued fixture withdrawal should succeed"),
    );
    assert_eq!(
        first_result.outcome,
        QueuedMessageWithdrawalOutcome::Withdrawn
    );

    let second_not_queued = withdrawal_command(
        "withdrawal-not-queued-second",
        &not_queued_thread,
        &MessageId::parse("message-never-queued").expect("missing message id"),
        &not_queued_original,
    );
    let second_result = withdrawal_of(
        handler
            .withdraw_composer_message(&second_not_queued.request_id, &second_not_queued)
            .await
            .expect("not-queued outcome should be a successful durable response"),
    );
    assert_eq!(
        second_result.receipt.disposition,
        ReceiptDisposition::Accepted
    );
    assert_eq!(
        second_result.outcome,
        QueuedMessageWithdrawalOutcome::NotQueued
    );
}

#[tokio::test]
async fn clock_failure_before_fresh_withdrawal_leaves_queue_intact() {
    let (_temporary, storage) = opened_storage("withdrawal-clock-failure").await;
    let repository = storage.repository();
    let (thread_id, message_id, original_request_id) =
        seed_message(repository, "withdrawal-clock-failure", queue_payload()).await;
    let origin = ScriptedOriginHandle::new([Err(CommandOriginClockError)]);
    let handler = handler(&storage, &origin);
    let withdrawal = withdrawal_command(
        "withdrawal-clock-failure-request",
        &thread_id,
        &message_id,
        &original_request_id,
    );

    let failure = failure_of(
        handler
            .withdraw_composer_message(&withdrawal.request_id, &withdrawal)
            .await,
    );
    assert_eq!(failure.code, ErrorCode::Internal);
    assert!(failure.retryable);
    assert_eq!(origin.instant_calls(), 1);

    let listing = repository
        .read_queued_messages(
            ListQueuedMessages::new(thread_id.clone(), QueuedMessageListOrder::OldestFirst, 1)
                .expect("bounded queue query should be valid"),
        )
        .await
        .expect("queue should remain readable after clock failure");
    assert_eq!(listing.messages().len(), 1);
    assert_eq!(listing.messages()[0].message_id, message_id);
    assert_eq!(
        listing.messages()[0].original_request_id,
        original_request_id
    );
}

fn engine_config(label: &str) -> EngineRunConfig {
    let one = FiniteMillis::new(1).expect("one millisecond should be valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget should be valid"),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).expect("body bound should be valid"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("line bound should be valid"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("event bound should be valid"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness bound should be valid"),
        max_header_count: CountLimit::new(8).expect("header bound should be valid"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("HTTP bound should be valid"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr bound should be valid"),
        observation_capacity: CountLimit::new(16).expect("observation bound should be valid"),
    })
    .expect("runtime controls should be valid");
    let permission = EnginePermissionPolicy::new(
        artisan_domain::PermissionId::parse(format!("permission-{label}"))
            .expect("permission id should be valid"),
        EngineAgentId::parse(format!("agent-{label}")).expect("agent id should be valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse(format!("profile-{label}")).expect("profile id should be valid"),
            EngineModelId::parse(format!("model-{label}")).expect("model id should be valid"),
            EngineRouteId::parse(format!("route-{label}")).expect("route id should be valid"),
            None,
            permission,
        )),
        runtime,
    )
}

async fn seed_bound_run(repository: &Repository, label: &str) -> (ThreadId, RunId) {
    let thread_id = seed_thread(repository, &format!("usage-{label}")).await;
    let settings = repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request(&format!("engine-usage-{label}")),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: engine_config(label),
            accepted_at: UnixMillis::from_millis(250),
        })
        .await
        .expect("engine settings should persist");
    let payload = queue_payload();
    let message_id = message(&format!("usage-message-{label}"));
    let original_request_id = request(&format!("usage-queue-{label}"));
    repository
        .queue_message(QueueMessageInput {
            request_id: original_request_id,
            message_id,
            thread_id: thread_id.clone(),
            payload,
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(300),
        })
        .await
        .expect("usage origin message should persist");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x21; 32]),
            claimed_at: UnixMillis::from_millis(400),
            lease_expires_at: UnixMillis::from_millis(900),
        })
        .await
        .expect("usage dispatch claim should persist")
        .expect("usage dispatch should be claimable");
    let run_id = RunId::parse(format!("usage-run-{label}")).expect("run id should be valid");
    let turn_id = TurnId::parse(format!("usage-turn-{label}")).expect("turn id should be valid");
    let item_id = ItemId::parse(format!("usage-item-{label}")).expect("item id should be valid");
    let first_patch_id =
        PatchId::parse(format!("usage-patch-first-{label}")).expect("patch id should be valid");
    let second_patch_id =
        PatchId::parse(format!("usage-patch-second-{label}")).expect("patch id should be valid");
    let run_start_key = RunStartKey::new([0x31; 32]);
    let credentials = RunLaunchCredentials::new([0x32; 32], [0x33; 32], [0x34; 32]);
    let engine_settings = repository
        .read_thread_engine_settings(&thread_id)
        .await
        .expect("engine settings should be readable")
        .expect("engine settings should be configured");
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
        .expect("assistant run should launch");
    let launched = match launched {
        artisan_database::LaunchClaimedRunOutcome::Started(receipt)
        | artisan_database::LaunchClaimedRunOutcome::AlreadyStarted(receipt) => receipt,
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding should be valid");
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
        .expect("provider binding should persist");
    assert!(matches!(
        bound,
        BindRunProviderOutcome::Bound(_) | BindRunProviderOutcome::AlreadyBound(_)
    ));
    let _ = settings;
    (thread_id, run_id)
}

#[tokio::test]
async fn run_usage_is_exactly_scoped_and_absence_stays_none() {
    let (_temporary, storage) = opened_storage("usage-scope").await;
    let repository = storage.repository();
    let (thread_id, run_id) = seed_bound_run(repository, "scope").await;
    let origin = ScriptedOriginHandle::new([]);
    let handler = handler(&storage, &origin);

    let response = handler
        .read_composer_usage(
            &request("read-usage-scope"),
            &ReadRunUsage::new(thread_id.clone(), run_id.clone()),
        )
        .await
        .expect("exact usage read should succeed");
    match response.payload {
        ResponsePayload::RunUsage(result) => {
            assert_eq!(result.thread_id, thread_id);
            assert_eq!(result.run_id, run_id);
            assert!(result.report.is_none(), "absence must not fabricate usage");
        }
        other => panic!("expected usage response, got {other:?}"),
    }

    let wrong_thread = thread("thread-usage-scope-other");
    let failure = failure_of(
        handler
            .read_composer_usage(
                &request("read-usage-wrong-thread"),
                &ReadRunUsage::new(wrong_thread, run_id),
            )
            .await,
    );
    assert_eq!(failure.code, ErrorCode::InvalidInput);
    assert!(!failure.retryable);
}
