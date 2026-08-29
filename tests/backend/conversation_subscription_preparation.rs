use artisan_backend::conversation_subscription_preparation::{
    PrepareSubscriptionError, prepare_conversation_subscription, stop_conversation_subscription,
};
use artisan_backend::conversation_subscription_registry::{
    ConversationSubscriptionRegistry, SubscriptionState, UnsubscribeOutcome,
};
use artisan_database::{
    BindRunProvider, ClaimMessageDispatch, CreateThreadInput, ProviderBindingBytes,
    QueueFirstMessageInput, Repository, RepositoryError, RunLaunchCredentials, RunStartKey,
    SqliteConfig, connect,
};
use artisan_domain::{
    ConversationCursor, ConversationSubscribe, ConversationUnsubscribe, ItemId, MessageBody,
    MessageId, PatchId, ProjectId, RequestId, ThreadId, ThreadTitle, TurnId, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use artisan_protocol::{ConversationSubscriptionStarted, ConversationSubscriptionStopped};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

const OWNER_BYTES: [u8; 32] = [0xa1; 32];
const LEASE_BYTES: [u8; 32] = [0xb2; 32];
const CLAIM_TOKEN_BYTES: [u8; 32] = [0xc3; 32];

async fn memory_repository() -> (DatabaseConnection, Repository) {
    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database).await.expect("migrate");
    (database.clone(), Repository::new(database))
}

async fn seed_thread(
    database: &DatabaseConnection,
    repository: &Repository,
    thread_id: &str,
) -> ThreadId {
    let tid = ThreadId::parse(thread_id).expect("tid");
    let exists = artisan_database::entities::attached_project::Entity::find_by_id("project-1")
        .one(database)
        .await
        .expect("query");
    if exists.is_none() {
        artisan_database::entities::attached_project::ActiveModel {
            project_id: Set("project-1".to_owned()),
            root_path: Set("C:/repos/artisan".to_owned()),
            display_name: Set("Artisan".to_owned()),
            attached_at_ms: Set(1),
        }
        .insert(database)
        .await
        .expect("project insert");
    }
    let _ = repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse(format!("req-{thread_id}")).expect("req"),
            thread_id: tid.clone(),
            project_id: ProjectId::parse("project-1").expect("pid"),
            title: ThreadTitle::parse(format!("Thread {thread_id}")).expect("title"),
            created_at: UnixMillis::from_millis(10),
            updated_at: UnixMillis::from_millis(10),
        })
        .await;
    tid
}

struct LaunchFixture {
    run_id: artisan_domain::RunId,
    turn_id: TurnId,
    item_id: ItemId,
    first_patch: PatchId,
    second_patch: PatchId,
    start_key: RunStartKey,
    credentials: RunLaunchCredentials,
}

fn launch_fixture(run: &str, turn: &str, item: &str, p1: &str, p2: &str) -> LaunchFixture {
    let mut key_bytes = [0u8; 32];
    let run_bytes = run.as_bytes();
    for (idx, byte) in key_bytes.iter_mut().enumerate() {
        let idx_u8 = u8::try_from(idx).expect("fit");
        *byte = run_bytes[idx % run_bytes.len()].wrapping_add(idx_u8);
    }
    if key_bytes == [0u8; 32] {
        key_bytes[0] = 0x01;
    }
    LaunchFixture {
        run_id: artisan_domain::RunId::parse(run).expect("run"),
        turn_id: TurnId::parse(turn).expect("turn"),
        item_id: ItemId::parse(item).expect("item"),
        first_patch: PatchId::parse(p1).expect("p1"),
        second_patch: PatchId::parse(p2).expect("p2"),
        start_key: RunStartKey::new(key_bytes),
        credentials: RunLaunchCredentials::new(OWNER_BYTES, LEASE_BYTES, CLAIM_TOKEN_BYTES),
    }
}

async fn queue_claim_launch_bind(
    repository: &Repository,
    thread_id: &ThreadId,
    message_id: &str,
    correlation: &str,
    launch: &LaunchFixture,
) {
    let mid = MessageId::parse(message_id).expect("mid");
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse(correlation).expect("req"),
            message_id: mid,
            thread_id: thread_id.clone(),
            body: MessageBody::parse("hello").expect("body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("queue");
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: artisan_database::DispatchLeaseOwner::new([0x11; 32]),
            claimed_at: UnixMillis::from_millis(100),
            lease_expires_at: UnixMillis::from_millis(600),
        })
        .await
        .expect("claim")
        .expect("claimed");
    let outcome = repository
        .launch_claimed_run(artisan_database::LaunchClaimedRun {
            claimed: &claimed,
            run_id: &launch.run_id,
            turn_id: &launch.turn_id,
            item_id: &launch.item_id,
            first_patch_id: &launch.first_patch,
            second_patch_id: &launch.second_patch,
            operated_at: UnixMillis::from_millis(150),
            run_start_key: &launch.start_key,
            credentials: &launch.credentials,
        })
        .await
        .expect("launch");
    let artisan_database::LaunchClaimedRunOutcome::Started(launched) = outcome else {
        panic!("started");
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("binding");
    let bind_outcome = repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &launch.start_key,
            credentials: &launch.credentials,
            expected_launch_at: UnixMillis::from_millis(150),
            bound_at: UnixMillis::from_millis(200),
            binding_version: 2,
            binding_bytes: &binding,
        })
        .await
        .expect("bind");
    assert!(matches!(
        bind_outcome,
        artisan_database::BindRunProviderOutcome::Bound(_)
    ));
}

#[tokio::test]
async fn fresh_preparation_returns_snapshot_pending_at_exact_cursor_and_preserves_thread() {
    let (database, repo) = memory_repository().await;
    let tid = seed_thread(&database, &repo, "thread-fresh-1").await;
    let launch = launch_fixture(
        "run-fresh-1",
        "turn-fresh-1",
        "item-fresh-1",
        "patch-fresh-a",
        "patch-fresh-b",
    );
    queue_claim_launch_bind(&repo, &tid, "msg-fresh-1", "req-fresh-1", &launch).await;
    let mut registry = ConversationSubscriptionRegistry::new();
    let subscribe = ConversationSubscribe::fresh(tid.clone());
    let prepared = prepare_conversation_subscription(&repo, &mut registry, &subscribe)
        .await
        .expect("fresh should succeed");
    let started = prepared.started();
    let lease = prepared.lease();
    assert_eq!(lease.thread_id(), &tid);
    assert_eq!(lease.generation().get(), 1);
    match started {
        ConversationSubscriptionStarted::Fresh(start) => {
            assert_eq!(start.snapshot().thread_id(), &tid);
            assert_eq!(start.snapshot().cursor().get(), 2);
            let view = registry.view(&tid).expect("view");
            assert_eq!(view.state(), SubscriptionState::Pending);
            assert_eq!(view.cursor(), start.snapshot().cursor());
            assert_eq!(view.cursor().get(), 2);
            assert_eq!(view.lease(), lease);
        }
        ConversationSubscriptionStarted::Resumed { .. } => {
            panic!("expected Fresh, got Resumed")
        }
    }
    let (started2, lease2) = prepared.into_parts();
    assert_eq!(lease2.thread_id(), &tid);
    assert_eq!(lease2.generation().get(), 1);
    match started2 {
        ConversationSubscriptionStarted::Fresh(s) => assert_eq!(s.snapshot().cursor().get(), 2),
        ConversationSubscriptionStarted::Resumed { .. } => panic!("expected Fresh in into_parts"),
    }
}

#[tokio::test]
async fn unknown_thread_fresh_failure_leaves_existing_registry_unchanged() {
    let (database, repo) = memory_repository().await;
    let tid_existing = seed_thread(&database, &repo, "thread-existing").await;
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_a = registry
        .register_pending(tid_existing.clone(), ConversationCursor::new(5))
        .expect("register a");
    let tid_other = ThreadId::parse("thread-other").expect("tid");
    let lease_other = registry
        .register_pending(tid_other.clone(), ConversationCursor::new(9))
        .expect("register other");
    let before_a = registry.view(&tid_existing).unwrap();
    let before_other = registry.view(&tid_other).unwrap();
    let len_before = registry.len();

    let unknown = ThreadId::parse("thread-unknown").expect("unknown");
    let err = prepare_conversation_subscription(
        &repo,
        &mut registry,
        &ConversationSubscribe::fresh(unknown.clone()),
    )
    .await
    .expect_err("unknown thread should fail");
    assert!(matches!(
        err,
        PrepareSubscriptionError::Repository(RepositoryError::ThreadNotFound { .. })
    ));
    if let PrepareSubscriptionError::Repository(RepositoryError::ThreadNotFound { thread_id }) = err
    {
        assert_eq!(thread_id, unknown);
    }
    assert_eq!(registry.view(&tid_existing).unwrap(), before_a);
    assert_eq!(registry.view(&tid_other).unwrap(), before_other);
    assert_eq!(registry.len(), len_before);
    assert_eq!(registry.view(&tid_existing).unwrap().lease(), &lease_a);
    assert_eq!(registry.view(&tid_other).unwrap().lease(), &lease_other);
}

#[tokio::test]
async fn resume_at_current_tail_returns_resumed_and_pending_at_requested_cursor() {
    let (database, repo) = memory_repository().await;
    let tid = seed_thread(&database, &repo, "thread-resume-current").await;
    let launch = launch_fixture(
        "run-resume-cur",
        "turn-resume-cur",
        "item-resume-cur",
        "patch-cur-a",
        "patch-cur-b",
    );
    queue_claim_launch_bind(&repo, &tid, "msg-cur", "req-cur", &launch).await;
    let mut registry = ConversationSubscriptionRegistry::new();
    let after = ConversationCursor::new(2);
    let prepared = prepare_conversation_subscription(
        &repo,
        &mut registry,
        &ConversationSubscribe::resume(tid.clone(), after),
    )
    .await
    .expect("resume at tail should succeed");
    match prepared.started() {
        ConversationSubscriptionStarted::Resumed { thread_id, cursor } => {
            assert_eq!(thread_id, &tid);
            assert_eq!(*cursor, after);
        }
        ConversationSubscriptionStarted::Fresh(_) => {
            panic!("expected Resumed, got Fresh")
        }
    }
    let view = registry.view(&tid).expect("view");
    assert_eq!(view.state(), SubscriptionState::Pending);
    assert_eq!(view.cursor(), after);
    assert_eq!(view.lease(), prepared.lease());
}

#[tokio::test]
async fn resume_behind_tail_with_batch_still_registers_at_requested_cursor_not_batch_endpoint() {
    let (database, repo) = memory_repository().await;
    let tid = seed_thread(&database, &repo, "thread-resume-batch").await;
    let launch = launch_fixture(
        "run-batch",
        "turn-batch",
        "item-batch",
        "patch-batch-a",
        "patch-batch-b",
    );
    queue_claim_launch_bind(&repo, &tid, "msg-batch", "req-batch", &launch).await;
    let mut registry = ConversationSubscriptionRegistry::new();
    let after = ConversationCursor::new(0);
    let prepared = prepare_conversation_subscription(
        &repo,
        &mut registry,
        &ConversationSubscribe::resume(tid.clone(), after),
    )
    .await
    .expect("resume behind tail should succeed");
    match prepared.started() {
        ConversationSubscriptionStarted::Resumed { thread_id, cursor } => {
            assert_eq!(thread_id, &tid);
            assert_eq!(*cursor, ConversationCursor::new(0));
        }
        ConversationSubscriptionStarted::Fresh(_) => {
            panic!("expected Resumed, got Fresh")
        }
    }
    let view = registry.view(&tid).expect("view");
    assert_eq!(view.cursor(), ConversationCursor::new(0));
    assert_eq!(view.state(), SubscriptionState::Pending);
}

#[tokio::test]
async fn resume_beyond_tail_returns_both_cursors_and_leaves_existing_unchanged() {
    let (database, repo) = memory_repository().await;
    let tid = seed_thread(&database, &repo, "thread-beyond").await;
    let launch = launch_fixture(
        "run-beyond",
        "turn-beyond",
        "item-beyond",
        "patch-beyond-a",
        "patch-beyond-b",
    );
    queue_claim_launch_bind(&repo, &tid, "msg-beyond", "req-beyond", &launch).await;
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_before = registry
        .register_pending(tid.clone(), ConversationCursor::new(7))
        .expect("before");
    let view_before = registry.view(&tid).unwrap();
    let beyond = ConversationCursor::new(999);
    let err = prepare_conversation_subscription(
        &repo,
        &mut registry,
        &ConversationSubscribe::resume(tid.clone(), beyond),
    )
    .await
    .expect_err("beyond tail should be ResnapshotRequired");
    match err {
        PrepareSubscriptionError::ResnapshotRequired {
            requested_cursor,
            current_cursor,
        } => {
            assert_eq!(requested_cursor, beyond);
            assert_eq!(current_cursor.get(), 2);
        }
        PrepareSubscriptionError::Repository(_) | PrepareSubscriptionError::Register(_) => {
            panic!("expected ResnapshotRequired, got Repository or Register")
        }
    }
    let view_after = registry.view(&tid).expect("view after");
    assert_eq!(view_after, view_before);
    assert_eq!(view_after.lease(), &lease_before);
    assert_eq!(view_after.cursor(), ConversationCursor::new(7));
    assert_eq!(registry.len(), 1);
}

#[tokio::test]
async fn second_successful_preparation_replaces_first_lease_and_pending_at_new_cursor() {
    let (database, repo) = memory_repository().await;
    let tid = seed_thread(&database, &repo, "thread-replace").await;
    let launch = launch_fixture(
        "run-replace",
        "turn-replace",
        "item-replace",
        "patch-rep-a",
        "patch-rep-b",
    );
    queue_claim_launch_bind(&repo, &tid, "msg-rep", "req-rep", &launch).await;
    let mut registry = ConversationSubscriptionRegistry::new();
    let first = prepare_conversation_subscription(
        &repo,
        &mut registry,
        &ConversationSubscribe::fresh(tid.clone()),
    )
    .await
    .expect("first");
    let lease1 = first.lease().clone();
    assert_eq!(registry.view(&tid).unwrap().lease(), &lease1);
    assert_eq!(registry.view(&tid).unwrap().cursor().get(), 2);
    let second = prepare_conversation_subscription(
        &repo,
        &mut registry,
        &ConversationSubscribe::resume(tid.clone(), ConversationCursor::new(0)),
    )
    .await
    .expect("second should replace");
    let lease2 = second.lease().clone();
    assert_ne!(lease1.generation(), lease2.generation());
    assert!(lease2.generation().get() > lease1.generation().get());
    let view = registry.view(&tid).unwrap();
    assert_eq!(view.lease(), &lease2);
    assert_eq!(view.cursor(), ConversationCursor::new(0));
    assert_eq!(view.state(), SubscriptionState::Pending);
    assert_eq!(
        registry.activate(&lease1),
        Err(artisan_backend::conversation_subscription_registry::ActivateError::StaleLease)
    );
    assert!(registry.activate(&lease2).is_ok());
}

#[tokio::test]
async fn stop_returns_removed_and_absent_idempotently() {
    let mut registry = ConversationSubscriptionRegistry::new();
    let tid = ThreadId::parse("thread-stop").expect("tid");
    let lease = registry
        .register_pending(tid.clone(), ConversationCursor::new(3))
        .expect("register");
    registry.activate(&lease).expect("activate");
    let view_before = registry.view(&tid).unwrap();
    let stopped = stop_conversation_subscription(
        &mut registry,
        &ConversationUnsubscribe {
            thread_id: tid.clone(),
        },
    );
    assert_eq!(stopped.response().thread_id, tid);
    assert_eq!(
        stopped.response(),
        &ConversationSubscriptionStopped {
            thread_id: tid.clone()
        }
    );
    match stopped.outcome() {
        UnsubscribeOutcome::Removed(removed) => {
            assert_eq!(removed.lease(), &lease);
            assert_eq!(removed.state(), SubscriptionState::Active);
            assert_eq!(removed.cursor(), ConversationCursor::new(3));
            assert_eq!(removed.lease(), view_before.lease());
        }
        UnsubscribeOutcome::Absent => {
            panic!("expected Removed, got Absent")
        }
    }
    let (resp, outcome) = stopped.into_parts();
    assert_eq!(resp.thread_id, tid);
    assert!(matches!(outcome, UnsubscribeOutcome::Removed(_)));
    assert!(registry.view(&tid).is_none());

    let second = stop_conversation_subscription(
        &mut registry,
        &ConversationUnsubscribe {
            thread_id: tid.clone(),
        },
    );
    assert_eq!(second.response().thread_id, tid);
    assert_eq!(second.outcome(), &UnsubscribeOutcome::Absent);
    let (resp2, out2) = second.into_parts();
    assert_eq!(resp2.thread_id, tid);
    assert!(matches!(out2, UnsubscribeOutcome::Absent));

    let unknown = ThreadId::parse("thread-unknown-stop").expect("tid");
    let third = stop_conversation_subscription(
        &mut registry,
        &ConversationUnsubscribe {
            thread_id: unknown.clone(),
        },
    );
    assert_eq!(third.response().thread_id, unknown);
    assert_eq!(third.outcome(), &UnsubscribeOutcome::Absent);
}

#[tokio::test]
async fn prepared_and_stopped_accessors_preserve_exact_values() {
    let (database, repo) = memory_repository().await;
    let tid = seed_thread(&database, &repo, "thread-accessor").await;
    let mut registry = ConversationSubscriptionRegistry::new();
    let prepared = prepare_conversation_subscription(
        &repo,
        &mut registry,
        &ConversationSubscribe::fresh(tid.clone()),
    )
    .await
    .expect("fresh accessor");
    let started_ref = prepared.started().clone();
    let lease_ref = prepared.lease().clone();
    let (started_owned, lease_owned) = prepared.into_parts();
    assert_eq!(started_ref, started_owned);
    assert_eq!(lease_ref, lease_owned);

    let mut reg2 = ConversationSubscriptionRegistry::new();
    let pending_lease = reg2
        .register_pending(tid.clone(), ConversationCursor::new(0))
        .expect("reg");
    let stopped = stop_conversation_subscription(
        &mut reg2,
        &ConversationUnsubscribe {
            thread_id: tid.clone(),
        },
    );
    match stopped.outcome() {
        UnsubscribeOutcome::Removed(removed) => assert_eq!(removed.lease(), &pending_lease),
        other => panic!("expected Removed, got {other:?}"),
    }
    let resp_ref = stopped.response().clone();
    let out_ref = stopped.outcome().clone();
    let (resp_owned, out_owned) = stopped.into_parts();
    assert_eq!(resp_ref, resp_owned);
    assert_eq!(out_ref, out_owned);
}

#[tokio::test]
async fn repository_failures_preserve_existing_registry_state() {
    let (database, repo) = memory_repository().await;
    let tid = seed_thread(&database, &repo, "thread-gen").await;
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease_before = registry
        .register_pending(tid.clone(), ConversationCursor::new(1))
        .expect("initial");
    let view_before = registry.view(&tid).unwrap();
    let unknown = ThreadId::parse("unknown-gen-thread").expect("tid");
    let err = prepare_conversation_subscription(
        &repo,
        &mut registry,
        &ConversationSubscribe::fresh(unknown.clone()),
    )
    .await
    .expect_err("should be repo error");
    assert!(matches!(err, PrepareSubscriptionError::Repository(_)));
    assert_eq!(registry.view(&tid).unwrap(), view_before);
    assert_eq!(registry.view(&tid).unwrap().lease(), &lease_before);

    let err2 = prepare_conversation_subscription(
        &repo,
        &mut registry,
        &ConversationSubscribe::resume(unknown.clone(), ConversationCursor::new(0)),
    )
    .await
    .expect_err("resume unknown should be repo error");
    assert!(matches!(err2, PrepareSubscriptionError::Repository(_)));
    assert_eq!(registry.view(&tid).unwrap(), view_before);
}
