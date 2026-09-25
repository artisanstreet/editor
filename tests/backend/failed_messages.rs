//! Private handler tests for Forge-owned submissions: sending a composer
//! draft by revision, retrying a failed message by identity, moving it into
//! a new thread, and the edit withdrawal that recalls a queued payload into
//! the Forge draft.
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
    ApprovalMode, AuthoredText, ByteLimit, ComposerDraftRevision, ComposerDraftScope, CountLimit,
    DirectoryId, DisplayName, DraftSubmissionOutcome, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FailedMessageRecovered, FailedMessageRetryOutcome, FailedMessageTarget,
    FilesystemAccess, FiniteMillis, ImageAttachment, ListFailedMessages, ListQueuedMessages,
    MessageId, NetworkAccess, OpenCode2Selection, PermissionId, ProjectId, QueueMessagePayload,
    QueuedMessageListOrder, ReceiptDisposition, RecoverFailedMessage, RequestId,
    RetryFailedMessage, RootPath, SaveComposerDraft, SubmitComposerDraft, ThreadId, ThreadTitle,
    UnixMillis, WebSearchAccess, WithdrawQueuedMessageCommand,
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

#[tokio::test]
async fn a_draft_resubmitted_after_a_lost_answer_is_queued_once() {
    let (_temporary, storage) = storage("submit").await;
    let repository = storage.repository();
    seed(repository).await;
    let handler = handler(&storage);
    let save = SaveComposerDraft::new(
        request("save-draft"),
        ComposerDraftScope::Thread(thread()),
        AuthoredText::parse("send me once").unwrap(),
        Vec::new(),
    )
    .unwrap();
    let saved = handler
        .save_composer_draft(save.request_id(), &save)
        .await
        .unwrap();
    let ResponsePayload::ComposerDraftSaved(saved) = saved.payload else {
        panic!("expected a save answer");
    };

    let submit = |request_id: &str, draft_revision| SubmitComposerDraft {
        request_id: request(request_id),
        thread_id: thread(),
        draft_revision,
        selection: None,
    };
    let mut answers = Vec::new();
    // The first answer is lost; the Editor sends the same revision again
    // under a new request id after reconnecting.
    for request_id in ["submit-1", "submit-2"] {
        let command = submit(request_id, saved.revision);
        let response = handler
            .submit_composer_draft_outcome(&command.request_id, &command)
            .await
            .unwrap();
        let ResponsePayload::ComposerDraftSubmitted(answer) = response.payload else {
            panic!("expected a submission answer");
        };
        assert_eq!(answer.request_id, command.request_id);
        answers.push(answer.outcome);
    }
    let [
        DraftSubmissionOutcome::Queued {
            message_id: first,
            disposition: ReceiptDisposition::Accepted,
            cleared_revision,
            ..
        },
        DraftSubmissionOutcome::Queued {
            message_id: second,
            disposition: ReceiptDisposition::Duplicate,
            ..
        },
    ] = answers.as_slice()
    else {
        panic!("one accepted submission and one duplicate: {answers:?}");
    };
    assert_eq!(first, second);
    let queued = repository
        .read_queued_messages(
            ListQueuedMessages::new(thread(), QueuedMessageListOrder::OldestFirst, 32).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        queued.messages().len(),
        2,
        "the seeded message and one send"
    );

    // A revision the Forge never gave the draft is refused with the current one.
    let stale = submit("submit-3", ComposerDraftRevision::new(7).unwrap());
    let response = handler
        .submit_composer_draft_outcome(&stale.request_id, &stale)
        .await
        .unwrap();
    let ResponsePayload::ComposerDraftSubmitted(answer) = response.payload else {
        panic!("expected a submission answer");
    };
    assert_eq!(
        answer.outcome,
        DraftSubmissionOutcome::Stale {
            current_revision: Some(*cleared_revision)
        }
    );
}

#[tokio::test]
async fn a_send_without_a_model_on_an_unconfigured_thread_is_refused_as_data() {
    let (_temporary, storage) = storage("refused").await;
    let repository = storage.repository();
    seed(repository).await;
    let unconfigured = ThreadId::parse("thread-unconfigured").unwrap();
    repository
        .create_thread(CreateThreadInput {
            request_id: request("create-unconfigured"),
            thread_id: unconfigured.clone(),
            project_id: ProjectId::parse("project-failed").unwrap(),
            title: ThreadTitle::parse("New thread").unwrap(),
            created_at: UnixMillis::from_millis(600),
            updated_at: UnixMillis::from_millis(600),
        })
        .await
        .unwrap();
    let handler = handler(&storage);
    let save = SaveComposerDraft::new(
        request("save-unconfigured"),
        ComposerDraftScope::Thread(unconfigured.clone()),
        AuthoredText::parse("which model?").unwrap(),
        Vec::new(),
    )
    .unwrap();
    let ResponsePayload::ComposerDraftSaved(saved) = handler
        .save_composer_draft(save.request_id(), &save)
        .await
        .unwrap()
        .payload
    else {
        panic!("expected a save answer");
    };
    let submit = SubmitComposerDraft {
        request_id: request("submit-unconfigured"),
        thread_id: unconfigured.clone(),
        draft_revision: saved.revision,
        selection: None,
    };
    let ResponsePayload::ComposerDraftSubmitted(answer) = handler
        .submit_composer_draft_outcome(&submit.request_id, &submit)
        .await
        .unwrap()
        .payload
    else {
        panic!("expected a submission answer");
    };
    let DraftSubmissionOutcome::Refused(refusal) = answer.outcome else {
        panic!("the send is refused as data, got {:?}", answer.outcome);
    };
    assert_eq!(
        refusal.kind(),
        artisan_domain::SubmissionRefusalKind::NoSelection
    );
    assert_eq!(
        refusal.message(),
        "Select a model before sending. Your draft is preserved."
    );
    // Nothing was queued and the draft is untouched, so the same revision
    // can be sent again once a model is chosen.
    let queued = repository
        .read_queued_messages(
            ListQueuedMessages::new(
                unconfigured.clone(),
                QueuedMessageListOrder::OldestFirst,
                32,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(queued.messages().is_empty());
    assert!(
        !repository
            .composer_draft_submitted(&unconfigured, saved.revision)
            .await
            .unwrap()
    );
}

/// Uploads one picked image and saves it as the seeded thread's draft.
async fn draft_with_picked_image(
    handler: &RequestHandler,
    mime_type: &str,
    bytes: Vec<u8>,
    name: &str,
) -> ComposerDraftRevision {
    let upload = artisan_domain::UploadComposerAttachment {
        request_id: request("upload-picked"),
        image: artisan_domain::ComposerImage::new(mime_type, bytes, name).unwrap(),
    };
    let ResponsePayload::ComposerAttachmentUploaded(uploaded) = handler
        .upload_composer_attachment(&upload.request_id, &upload)
        .await
        .unwrap()
        .payload
    else {
        panic!("expected an upload answer");
    };
    let save = SaveComposerDraft::new(
        request("save-picked"),
        ComposerDraftScope::Thread(thread()),
        AuthoredText::empty(),
        vec![uploaded.reference],
    )
    .unwrap();
    let ResponsePayload::ComposerDraftSaved(saved) = handler
        .save_composer_draft(save.request_id(), &save)
        .await
        .unwrap()
        .payload
    else {
        panic!("expected a save answer");
    };
    saved.revision
}

async fn submit_draft(
    handler: &RequestHandler,
    revision: ComposerDraftRevision,
) -> DraftSubmissionOutcome {
    let submit = SubmitComposerDraft {
        request_id: request("submit-picked"),
        thread_id: thread(),
        draft_revision: revision,
        selection: None,
    };
    let ResponsePayload::ComposerDraftSubmitted(answer) = handler
        .submit_composer_draft_outcome(&submit.request_id, &submit)
        .await
        .unwrap()
        .payload
    else {
        panic!("expected a submission answer");
    };
    answer.outcome
}

#[tokio::test]
async fn a_picked_image_is_fitted_to_the_threads_engine_when_the_draft_is_sent() {
    let (_temporary, storage) = storage("fitted").await;
    let repository = storage.repository();
    seed(repository).await;
    let handler = handler(&storage);
    // Wider than the long-edge cap: the Forge rescales it for the engine.
    let mut picked = Vec::new();
    image::DynamicImage::new_rgba8(3000, 10)
        .write_to(
            &mut std::io::Cursor::new(&mut picked),
            image::ImageFormat::Png,
        )
        .unwrap();
    let revision = draft_with_picked_image(&handler, "image/png", picked.clone(), "wide.png").await;
    let DraftSubmissionOutcome::Queued { message_id, .. } = submit_draft(&handler, revision).await
    else {
        panic!("the draft is queued");
    };
    let queued = repository
        .read_queued_messages(
            ListQueuedMessages::new(thread(), QueuedMessageListOrder::OldestFirst, 32).unwrap(),
        )
        .await
        .unwrap();
    let sent = queued
        .messages()
        .iter()
        .find(|message| message.message_id == message_id)
        .expect("the sent message is queued");
    // The seeded thread runs `OpenCode`, which takes PNG: the rescaled image
    // stays PNG but is not the picked bytes.
    assert_eq!(sent.attachments.len(), 1);
    assert_eq!(sent.attachments[0].mime_type.as_str(), "image/png");
    assert_ne!(
        usize::try_from(sent.attachments[0].size_bytes).unwrap(),
        picked.len()
    );
}

#[tokio::test]
async fn an_image_the_engine_cannot_take_refuses_the_send_and_keeps_the_draft() {
    let (_temporary, storage) = storage("unfittable").await;
    let repository = storage.repository();
    seed(repository).await;
    let handler = handler(&storage);
    // A GIF passes through untouched, so one over the message bound cannot
    // be sent even though the store keeps it.
    let revision = draft_with_picked_image(
        &handler,
        "image/gif",
        vec![0; artisan_domain::MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES + 1],
        "huge.gif",
    )
    .await;
    let DraftSubmissionOutcome::Refused(refusal) = submit_draft(&handler, revision).await else {
        panic!("the send is refused");
    };
    assert_eq!(
        refusal.kind(),
        artisan_domain::SubmissionRefusalKind::AttachmentRejected
    );
    assert_eq!(
        refusal.message(),
        "huge.gif: That image exceeds the 5 MiB limit. Your draft is preserved; remove or replace the image and send again."
    );
    assert!(
        !repository
            .composer_draft_submitted(&thread(), revision)
            .await
            .unwrap()
    );
}
