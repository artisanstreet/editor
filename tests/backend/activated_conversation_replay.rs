use artisan_backend::RequestHandler;
use artisan_backend::activated_conversation_replay::read_activated_conversation_replay;
use artisan_database::{
    AttachProjectInput, BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch,
    ConversationPatchReplay, CreateThreadInput, DispatchLeaseOwner, LaunchClaimedRun,
    LaunchClaimedRunOutcome, ProviderBindingBytes, QueueFirstMessageInput, Repository,
    RepositoryError, RunLaunchCredentials, RunStartKey, SqliteConfig, connect,
};
use artisan_domain::{
    ConversationCursor, ConversationRequest, ConversationSubscribe, DirectoryId, DisplayName,
    MessageBody, MessageId, ProjectId, RequestId, RootPath, ThreadId, ThreadTitle, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use artisan_protocol::{ClientRequest, ConversationSubscriptionStarted, ResponsePayload};
use sea_orm::{DatabaseConnection, EntityTrait};

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
        .expect("database should migrate");
    (database.clone(), Repository::new(database))
}

async fn seed_thread(repository: &Repository, thread_id: &str, label: &str) -> ThreadId {
    let project_id = format!("project-{label}");
    repository
        .attach_project(AttachProjectInput {
            request_id: RequestId::parse(format!("request-project-{label}"))
                .expect("request id should parse"),
            directory_id: DirectoryId::parse(format!("directory-{label}"))
                .expect("directory id should parse"),
            project_id: ProjectId::parse(project_id.clone()).expect("project id should parse"),
            root_path: RootPath::parse(format!("C:/repos/{label}"))
                .expect("root path should parse"),
            display_name: DisplayName::parse(format!("Project {label}"))
                .expect("display name should parse"),
            attached_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("project should attach");

    let thread_id = ThreadId::parse(thread_id).expect("thread id should parse");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse(format!("request-thread-{label}"))
                .expect("request id should parse"),
            thread_id: thread_id.clone(),
            project_id: ProjectId::parse(project_id).expect("project id should parse"),
            title: ThreadTitle::parse(format!("Thread {label}")).expect("title should parse"),
            created_at: UnixMillis::from_millis(20),
            updated_at: UnixMillis::from_millis(20),
        })
        .await
        .expect("thread should create");
    thread_id
}

async fn commit_two_patches(repository: &Repository, thread_id: &ThreadId, label: &str) {
    let message_id = MessageId::parse(format!("message-{label}")).expect("message id should parse");
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse(format!("request-message-{label}"))
                .expect("request id should parse"),
            message_id,
            thread_id: thread_id.clone(),
            body: MessageBody::parse("durable message").expect("message body should parse"),
            accepted_at: UnixMillis::from_millis(30),
        })
        .await
        .expect("message should queue");

    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x11; 32]),
            claimed_at: UnixMillis::from_millis(40),
            lease_expires_at: UnixMillis::from_millis(90),
        })
        .await
        .expect("message should claim")
        .expect("message dispatch should be present");
    let run_id = artisan_domain::RunId::parse(format!("run-{label}")).expect("run id should parse");
    let turn_id =
        artisan_domain::TurnId::parse(format!("turn-{label}")).expect("turn id should parse");
    let item_id =
        artisan_domain::ItemId::parse(format!("item-{label}")).expect("item id should parse");
    let first_patch_id = artisan_domain::PatchId::parse(format!("patch-{label}-first"))
        .expect("patch id should parse");
    let second_patch_id = artisan_domain::PatchId::parse(format!("patch-{label}-second"))
        .expect("patch id should parse");
    let run_start_key = RunStartKey::new([0x44; 32]);
    let credentials = RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]);
    let launched = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &item_id,
            first_patch_id: &first_patch_id,
            second_patch_id: &second_patch_id,
            operated_at: UnixMillis::from_millis(50),
            run_start_key: &run_start_key,
            credentials: &credentials,
        })
        .await
        .expect("run should launch");
    let LaunchClaimedRunOutcome::Started(launched) = launched else {
        panic!("fresh dispatch should launch");
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding should be valid");
    let BindRunProviderOutcome::Bound(_) = repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &run_start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(50),
            bound_at: UnixMillis::from_millis(60),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await
        .expect("run provider should bind")
    else {
        panic!("fresh run should bind");
    };
}

async fn activate(
    handler: &RequestHandler,
    request_id: &str,
    subscribe: ConversationSubscribe,
) -> artisan_backend::request_handler::ActivatedConversationSubscription {
    let (wire, receipt) = handler
        .respond_with_receipt(
            &RequestId::parse(request_id).expect("request id should parse"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe)),
        )
        .await
        .into_parts();
    let response = wire.expect("subscription response should succeed");
    assert!(matches!(
        response.payload,
        ResponsePayload::ConversationSubscriptionStarted(
            ConversationSubscriptionStarted::Fresh(_)
                | ConversationSubscriptionStarted::Resumed { .. }
        )
    ));
    handler
        .activate_after_response(receipt)
        .await
        .expect("subscription receipt should activate")
        .expect("subscription should carry activation work")
}

#[tokio::test]
async fn fresh_activation_at_tail_reads_current_and_preserves_activation() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "thread-fresh-tail", "fresh-tail").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());
    let subscription = activate(
        &handler,
        "request-fresh-tail",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    let expected_lease = subscription.lease().clone();
    let expected_cursor = subscription.cursor();

    let owner = read_activated_conversation_replay(&repository, subscription)
        .await
        .expect("fresh tail replay should succeed");
    assert_eq!(owner.subscription().lease(), &expected_lease);
    assert_eq!(owner.subscription().cursor(), expected_cursor);
    assert!(matches!(
        owner.replay(),
        ConversationPatchReplay::Current { cursor } if *cursor == expected_cursor
    ));

    let (subscription, replay) = owner.into_parts();
    assert_eq!(subscription.lease(), &expected_lease);
    assert_eq!(subscription.cursor(), expected_cursor);
    assert!(matches!(
        replay,
        ConversationPatchReplay::Current { cursor } if cursor == expected_cursor
    ));
}

#[tokio::test]
async fn commit_after_fresh_activation_is_returned_as_batch_from_activation_cursor() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "thread-after-activation", "after-activation").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());
    let subscription = activate(
        &handler,
        "request-after-activation",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    assert_eq!(subscription.cursor(), ConversationCursor::default());
    let expected_lease = subscription.lease().clone();
    let expected_cursor = subscription.cursor();

    commit_two_patches(&repository, &thread_id, "after-activation").await;
    let owner = read_activated_conversation_replay(&repository, subscription)
        .await
        .expect("post-activation replay should succeed");
    assert_eq!(owner.subscription().lease(), &expected_lease);
    assert_eq!(owner.subscription().cursor(), expected_cursor);
    let ConversationPatchReplay::Batch(batch) = owner.replay() else {
        panic!("the post-activation commit should be a batch");
    };
    assert_eq!(batch.from_cursor(), expected_cursor);
    assert_eq!(batch.to_cursor(), ConversationCursor::new(2));
    assert_eq!(batch.patches().len(), 2);
}

#[tokio::test]
async fn resume_activation_replays_from_exact_requested_cursor() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "thread-resume-exact", "resume-exact").await;
    commit_two_patches(&repository, &thread_id, "resume-exact").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());
    let requested_cursor = ConversationCursor::new(1);
    let subscription = activate(
        &handler,
        "request-resume-exact",
        ConversationSubscribe::resume(thread_id, requested_cursor),
    )
    .await;
    assert_eq!(subscription.cursor(), requested_cursor);
    let expected_lease = subscription.lease().clone();

    let owner = read_activated_conversation_replay(&repository, subscription)
        .await
        .expect("resume replay should succeed");
    assert_eq!(owner.subscription().lease(), &expected_lease);
    assert_eq!(owner.subscription().cursor(), requested_cursor);
    let ConversationPatchReplay::Batch(batch) = owner.replay() else {
        panic!("replay must use the requested cursor rather than preparation endpoint");
    };
    assert_eq!(batch.from_cursor(), requested_cursor);
    assert_eq!(batch.to_cursor(), ConversationCursor::new(2));
    assert_eq!(batch.patches().len(), 1);
    assert_eq!(batch.patches()[0].sequence().get(), 2);
}

#[tokio::test]
async fn repository_not_found_after_valid_activation_is_preserved() {
    let (database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "thread-deleted", "deleted").await;
    let handler = RequestHandler::with_subscriptions(repository.clone());
    let subscription = activate(
        &handler,
        "request-deleted",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;

    // The migrated schema restricts the thread's command receipt, so remove
    // that fixture-only dependent row before deleting the otherwise empty
    // thread. No production deletion capability is introduced.
    artisan_database::entities::command_receipt::Entity::delete_by_id("request-thread-deleted")
        .exec(&database)
        .await
        .expect("thread receipt should be deletable in fixture");
    artisan_database::entities::thread::Entity::delete_by_id(thread_id.as_str())
        .exec(&database)
        .await
        .expect("empty thread should be deletable in fixture");
    let error = read_activated_conversation_replay(&repository, subscription)
        .await
        .expect_err("deleted thread should fail replay");
    match error {
        RepositoryError::ThreadNotFound { thread_id: actual } => assert_eq!(actual, thread_id),
        other => panic!("expected exact thread-not-found error, got {other:?}"),
    }
}
