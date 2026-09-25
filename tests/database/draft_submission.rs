//! Sending a composer draft as a message: one transaction queues the draft
//! at exactly the submitted revision, records the submission, and empties
//! the draft; a repeat of the revision answers the first message.

use artisan_database::{
    AttachProjectInput, CreateThreadInput, DraftSubmission, Repository, SaveComposerDraftInput,
    SetThreadEngineConfigInput, SqliteConfig, SubmitComposerDraftInput, connect,
};
use artisan_domain::{
    ApprovalMode, AuthoredText, ByteLimit, ComposerAttachmentRef, ComposerDraftRevision,
    ComposerDraftScope, CountLimit, DirectoryId, DisplayName, EngineAgentId,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, ImageAttachment, MessageId, NetworkAccess,
    OpenCode2Selection, PermissionId, ProjectId, ReceiptDisposition, RequestId, RootPath, ThreadId,
    ThreadTitle, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

async fn repository() -> (DatabaseConnection, Repository) {
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
    let repository = Repository::new(database.clone());
    repository
        .attach_project(AttachProjectInput {
            request_id: request("attach-project"),
            directory_id: DirectoryId::parse("directory-1").unwrap(),
            project_id: ProjectId::parse("project-1").unwrap(),
            root_path: RootPath::parse("C:/repos/artisan").unwrap(),
            display_name: DisplayName::parse("Artisan").unwrap(),
            attached_at: UnixMillis::from_millis(100),
        })
        .await
        .expect("project should attach");
    repository
        .create_thread(CreateThreadInput {
            request_id: request("create-thread"),
            thread_id: thread(),
            project_id: ProjectId::parse("project-1").unwrap(),
            title: ThreadTitle::parse("Thread").unwrap(),
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await
        .expect("thread should create");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: request("config-thread"),
            thread_id: thread(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: engine_config(),
            accepted_at: UnixMillis::from_millis(250),
        })
        .await
        .expect("thread configuration should persist");
    (database, repository)
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
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-draft").unwrap(),
        EngineAgentId::parse("agent-draft").unwrap(),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-draft").unwrap(),
            EngineModelId::parse("model-draft").unwrap(),
            EngineRouteId::parse("route-draft").unwrap(),
            None,
            permission,
        )),
        runtime,
    )
}

fn request(value: &str) -> RequestId {
    RequestId::parse(value).unwrap()
}

fn thread() -> ThreadId {
    ThreadId::parse("thread-1").unwrap()
}

fn scope() -> ComposerDraftScope {
    ComposerDraftScope::Thread(thread())
}

fn revision(value: u64) -> ComposerDraftRevision {
    ComposerDraftRevision::new(value).unwrap()
}

async fn save(
    repository: &Repository,
    request_id: &str,
    text: &str,
    attachments: Vec<ComposerAttachmentRef>,
) -> ComposerDraftRevision {
    repository
        .save_composer_draft(SaveComposerDraftInput {
            request_id: request(request_id),
            scope: scope(),
            text: AuthoredText::parse(text).unwrap(),
            attachments,
            saved_at: UnixMillis::from_millis(300),
        })
        .await
        .expect("draft should save")
        .revision
}

async fn submit(
    repository: &Repository,
    request_id: &str,
    draft_revision: u64,
    message_id: &str,
) -> DraftSubmission {
    repository
        .submit_composer_draft(SubmitComposerDraftInput {
            request_id: request(request_id),
            thread_id: thread(),
            draft_revision: revision(draft_revision),
            message_id: MessageId::parse(message_id).unwrap(),
            steer_run_id: None,
            submitted_at: UnixMillis::from_millis(400),
        })
        .await
        .expect("submission should not fail")
}

async fn message_count(database: &DatabaseConnection) -> i64 {
    database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT count(*) FROM messages",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<i64>(0)
        .unwrap()
}

async fn stored_draft(repository: &Repository) -> (u64, String, usize) {
    let draft = repository
        .read_composer_draft(&scope())
        .await
        .unwrap()
        .expect("draft row kept");
    (
        draft.revision().get(),
        draft.text().as_str().to_owned(),
        draft.attachments().len(),
    )
}

#[tokio::test]
async fn a_revision_is_queued_once_whatever_request_repeats_it() {
    let (database, repository) = repository().await;
    assert_eq!(
        save(&repository, "save-1", "hello", Vec::new()).await.get(),
        1
    );

    let DraftSubmission::Queued {
        result,
        cleared_revision,
    } = submit(&repository, "send-a", 1, "message-a").await
    else {
        panic!("the saved revision is queued");
    };
    assert_eq!(result.receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(result.message_id.as_str(), "message-a");
    assert_eq!(
        result.payload.text().map(AuthoredText::as_str),
        Some("hello")
    );
    assert_eq!(cleared_revision.get(), 2);
    assert_eq!(stored_draft(&repository).await, (2, String::new(), 0));

    // The answer was lost: the same request, and a new request after a
    // reconnect, both answer the first message and change nothing.
    for (request_id, message_id) in [("send-a", "message-b"), ("send-c", "message-c")] {
        let DraftSubmission::Queued {
            result,
            cleared_revision,
        } = submit(&repository, request_id, 1, message_id).await
        else {
            panic!("a repeated revision answers its message");
        };
        assert_eq!(result.receipt.disposition, ReceiptDisposition::Duplicate);
        assert_eq!(result.message_id.as_str(), "message-a");
        assert_eq!(result.receipt.request_id.as_str(), "send-a");
        assert_eq!(cleared_revision.get(), 2);
    }
    assert_eq!(message_count(&database).await, 1);
    assert_eq!(stored_draft(&repository).await, (2, String::new(), 0));
}

#[tokio::test]
async fn a_revision_other_than_the_stored_one_is_refused_with_the_current_revision() {
    let (database, repository) = repository().await;
    assert!(matches!(
        submit(&repository, "send-none", 1, "message-none").await,
        DraftSubmission::Stale {
            current_revision: None
        }
    ));
    save(&repository, "save-1", "first", Vec::new()).await;
    save(&repository, "save-2", "second", Vec::new()).await;
    for stale in [1, 7] {
        assert_eq!(
            submit(&repository, &format!("send-{stale}"), stale, "message-x").await,
            DraftSubmission::Stale {
                current_revision: Some(revision(2))
            }
        );
    }
    assert_eq!(message_count(&database).await, 0);
    assert_eq!(stored_draft(&repository).await, (2, "second".to_owned(), 0));

    // An emptied draft cannot be sent.
    save(&repository, "save-3", "", Vec::new()).await;
    assert_eq!(
        submit(&repository, "send-empty", 3, "message-empty").await,
        DraftSubmission::Empty
    );
    assert_eq!(message_count(&database).await, 0);
}

#[tokio::test]
async fn a_late_save_cannot_bring_back_a_sent_draft() {
    let (_database, repository) = repository().await;
    save(&repository, "save-hello", "hello", Vec::new()).await;
    submit(&repository, "send-hello", 1, "message-hello").await;
    assert_eq!(stored_draft(&repository).await, (2, String::new(), 0));

    // The save request arrives again (its answer was lost): it answers its
    // first revision and writes nothing.
    assert_eq!(
        save(&repository, "save-hello", "hello", Vec::new())
            .await
            .get(),
        1
    );
    assert_eq!(stored_draft(&repository).await, (2, String::new(), 0));

    // A new save is the user's next draft.
    assert_eq!(
        save(&repository, "save-next", "next", Vec::new())
            .await
            .get(),
        3
    );
    assert_eq!(stored_draft(&repository).await, (3, "next".to_owned(), 0));
}

#[tokio::test]
async fn stored_images_are_sent_with_their_bytes_and_leave_the_draft() {
    let (_database, repository) = repository().await;
    let image = ImageAttachment::new("image/png", vec![7; 5], "shot.png").unwrap();
    let reference = repository
        .store_composer_attachment(&image, UnixMillis::from_millis(300))
        .await
        .unwrap();
    save(&repository, "save-image", "", vec![reference]).await;
    let DraftSubmission::Queued { result, .. } =
        submit(&repository, "send-image", 1, "message-image").await
    else {
        panic!("an image-only draft is queued");
    };
    assert_eq!(result.payload.text(), None);
    assert_eq!(result.payload.attachments(), [image]);
    assert_eq!(stored_draft(&repository).await, (2, String::new(), 0));
}
