//! External coverage for owner-fenced message-dispatch lifecycle transitions.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{self, DispatchState};
use artisan_database::{
    ClaimMessageDispatch, CompleteMessageDispatch, DispatchFailureReason,
    DispatchFailureReasonError, DispatchLeaseOwner, FailMessageDispatch, Repository,
    RepositoryError, RequeueMessageDispatch, SqliteConfig, connect,
};
use artisan_domain::{MessageId, UnixMillis};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait, IntoActiveModel,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

async fn memory_database() -> (DatabaseConnection, Repository) {
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

async fn seed_foundation(database: &DatabaseConnection) {
    entities::attached_project::ActiveModel {
        project_id: Set("project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    }
    .insert(database)
    .await
    .expect("project fixture should insert");
    entities::thread::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        project_id: Set("project-1".to_owned()),
        title: Set("Thread".to_owned()),
        created_at_ms: Set(2),
        updated_at_ms: Set(2),
        engine_run_config_version: Set(None),
        engine_run_config_revision: Set(0),
        engine_run_config: Set(None),
    }
    .insert(database)
    .await
    .expect("thread fixture should insert");
}

async fn seed_dispatch(
    database: &DatabaseConnection,
    message_id: &str,
    correlation_id: &str,
    ordinal: i64,
    queued_at_ms: i64,
    available_at_ms: i64,
) {
    entities::message::ActiveModel {
        message_id: Set(message_id.to_owned()),
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(ordinal),
        body: Set(format!("body-{message_id}")),
        accepted_at_ms: Set(queued_at_ms),
    }
    .insert(database)
    .await
    .expect("message fixture should insert");
    entities::message_dispatch::ActiveModel {
        message_id: Set(message_id.to_owned()),
        correlation_id: Set(correlation_id.to_owned()),
        state: Set(DispatchState::Queued),
        attempt_count: Set(0),
        queued_at_ms: Set(queued_at_ms),
        available_at_ms: Set(available_at_ms),
        lease_owner: Set(None),
        lease_expires_at_ms: Set(None),
        last_error: Set(Some("previous failure".to_owned())),
        updated_at_ms: Set(queued_at_ms),
    }
    .insert(database)
    .await
    .expect("dispatch fixture should insert");
}

const fn lease_owner(byte: u8) -> DispatchLeaseOwner {
    DispatchLeaseOwner::new([byte; 32])
}

const fn claim(
    owner_byte: u8,
    claimed_at_ms: i64,
    lease_expires_at_ms: i64,
) -> ClaimMessageDispatch {
    ClaimMessageDispatch {
        owner: lease_owner(owner_byte),
        claimed_at: UnixMillis::from_millis(claimed_at_ms),
        lease_expires_at: UnixMillis::from_millis(lease_expires_at_ms),
    }
}

fn complete(message_id: &str, owner_byte: u8, operated_at_ms: i64) -> CompleteMessageDispatch {
    CompleteMessageDispatch {
        message_id: MessageId::parse(message_id).expect("test message id should parse"),
        owner: lease_owner(owner_byte),
        operated_at: UnixMillis::from_millis(operated_at_ms),
    }
}

fn fail(
    message_id: &str,
    owner_byte: u8,
    operated_at_ms: i64,
    reason: &str,
) -> FailMessageDispatch {
    FailMessageDispatch {
        message_id: MessageId::parse(message_id).expect("test message id should parse"),
        owner: lease_owner(owner_byte),
        operated_at: UnixMillis::from_millis(operated_at_ms),
        reason: DispatchFailureReason::parse(reason).expect("test failure reason should parse"),
    }
}

fn requeue(
    message_id: &str,
    owner_byte: u8,
    operated_at_ms: i64,
    available_at_ms: i64,
    reason: &str,
) -> RequeueMessageDispatch {
    RequeueMessageDispatch {
        message_id: MessageId::parse(message_id).expect("test message id should parse"),
        owner: lease_owner(owner_byte),
        operated_at: UnixMillis::from_millis(operated_at_ms),
        available_at: UnixMillis::from_millis(available_at_ms),
        reason: DispatchFailureReason::parse(reason).expect("test failure reason should parse"),
    }
}

/// Claims the single seeded dispatch, panicking when it is not returned.
async fn claim_seeded(repository: &Repository, owner_byte: u8) -> u32 {
    let claimed = repository
        .claim_next_message_dispatch(claim(owner_byte, 10, 20))
        .await
        .expect("claim should succeed")
        .expect("seeded dispatch should be claimed");
    claimed.attempt_count
}

async fn dispatch(database: &DatabaseConnection, message_id: &str) -> entities::MessageDispatch {
    entities::message_dispatch::Entity::find_by_id(message_id)
        .one(database)
        .await
        .expect("dispatch should query")
        .expect("dispatch should exist")
}

async fn mutate_dispatch(
    database: &DatabaseConnection,
    message_id: &str,
    mutate: impl FnOnce(&mut entities::message_dispatch::ActiveModel),
) {
    let mut model = dispatch(database, message_id).await.into_active_model();
    mutate(&mut model);
    model
        .update(database)
        .await
        .expect("fixture mutation should update");
}

#[tokio::test]
async fn complete_clears_lease_metadata_and_rejects_duplicate_completion() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    assert_eq!(claim_seeded(&repository, 0x11).await, 1);
    mutate_dispatch(&database, "message-1", |model| {
        model.last_error = Set(Some("transient crash".to_owned()));
    })
    .await;

    let completed = repository
        .complete_message_dispatch(complete("message-1", 0x11, 15))
        .await
        .expect("live-lease completion should succeed");
    assert_eq!(completed.message_id.as_str(), "message-1");
    assert_eq!(completed.updated_at, UnixMillis::from_millis(15));

    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.state, DispatchState::Completed);
    assert_eq!(persisted.attempt_count, 1);
    assert_eq!(persisted.correlation_id, "request-1");
    assert_eq!(persisted.queued_at_ms, 3);
    assert_eq!(persisted.available_at_ms, 3);
    assert!(persisted.lease_owner.is_none());
    assert!(persisted.lease_expires_at_ms.is_none());
    assert!(persisted.last_error.is_none());
    assert_eq!(persisted.updated_at_ms, 15);

    let duplicate = repository
        .complete_message_dispatch(complete("message-1", 0x11, 16))
        .await;
    assert!(matches!(
        duplicate,
        Err(RepositoryError::InvalidDispatchState { state, .. }) if state == "completed"
    ));
    let after_duplicate = dispatch(&database, "message-1").await;
    assert_eq!(after_duplicate.state, DispatchState::Completed);
    assert_eq!(after_duplicate.updated_at_ms, 15);
}

#[tokio::test]
async fn terminal_fail_persists_bounded_reason_and_clears_lease() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    assert_eq!(claim_seeded(&repository, 0x11).await, 1);

    let failed = repository
        .fail_message_dispatch(fail("message-1", 0x11, 18, "engine halted"))
        .await
        .expect("live-lease terminal failure should succeed");
    assert_eq!(failed.message_id.as_str(), "message-1");
    assert_eq!(failed.updated_at, UnixMillis::from_millis(18));

    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.state, DispatchState::Failed);
    assert_eq!(persisted.attempt_count, 1);
    assert_eq!(persisted.last_error.as_deref(), Some("engine halted"));
    assert!(persisted.lease_owner.is_none());
    assert!(persisted.lease_expires_at_ms.is_none());
    assert_eq!(persisted.updated_at_ms, 18);
    assert_eq!(persisted.available_at_ms, 3);
}

#[tokio::test]
async fn requeue_defers_until_availability_then_claim_clears_error_and_increments_attempt() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    assert_eq!(claim_seeded(&repository, 0x11).await, 1);

    let requeued = repository
        .requeue_message_dispatch(requeue("message-1", 0x11, 12, 1_000, "backing off"))
        .await
        .expect("live-lease requeue should succeed");
    assert_eq!(requeued.updated_at, UnixMillis::from_millis(12));
    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.state, DispatchState::Queued);
    assert_eq!(persisted.attempt_count, 1);
    assert_eq!(persisted.available_at_ms, 1_000);
    assert_eq!(persisted.last_error.as_deref(), Some("backing off"));
    assert!(persisted.lease_owner.is_none());
    assert!(persisted.lease_expires_at_ms.is_none());
    assert_eq!(persisted.updated_at_ms, 12);

    let early = repository
        .claim_next_message_dispatch(claim(0x22, 999, 1_500))
        .await
        .expect("early claim lookup should succeed");
    assert!(early.is_none());
    let before_availability = dispatch(&database, "message-1").await;
    assert_eq!(before_availability.attempt_count, 1);
    assert_eq!(
        before_availability.last_error.as_deref(),
        Some("backing off")
    );

    let retried = repository
        .claim_next_message_dispatch(claim(0x22, 1_000, 1_500))
        .await
        .expect("availability-time claim should succeed")
        .expect("requeued dispatch should be claimable at availability");
    assert_eq!(retried.message_id.as_str(), "message-1");
    assert_eq!(retried.attempt_count, 2);
    let after_retry = dispatch(&database, "message-1").await;
    assert_eq!(after_retry.state, DispatchState::Leased);
    assert_eq!(after_retry.attempt_count, 2);
    assert!(after_retry.last_error.is_none());
    assert_eq!(after_retry.lease_owner.as_deref().map(str::len), Some(64));
}

#[tokio::test]
async fn zero_backoff_and_signed_negative_requeues_are_immediately_claimable() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;

    let _first = repository
        .claim_next_message_dispatch(claim(0x11, 10, 20))
        .await
        .expect("initial claim should succeed")
        .expect("seeded dispatch should be claimed");
    repository
        .requeue_message_dispatch(requeue("message-1", 0x11, 12, -5, "negative retry"))
        .await
        .expect("negative-availability requeue should succeed");

    let reclaimed = repository
        .claim_next_message_dispatch(claim(0x22, 11, 400))
        .await
        .expect("zero-backoff reclaim should succeed")
        .expect("earlier-than-operation availability should be immediately claimable");
    assert_eq!(reclaimed.attempt_count, 2);

    repository
        .requeue_message_dispatch(requeue("message-1", 0x22, 13, 13, "equality retry"))
        .await
        .expect("equality-availability requeue should succeed");
    let reclaimed_again = repository
        .claim_next_message_dispatch(claim(0x33, 13, 500))
        .await
        .expect("equality-time claim should succeed")
        .expect("availability equal to operation time should be immediately claimable");
    assert_eq!(reclaimed_again.attempt_count, 3);
    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.attempt_count, 3);
    assert!(persisted.last_error.is_none());
}

#[tokio::test]
async fn unfenced_transitions_are_typed_and_leave_the_row_unchanged() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    seed_dispatch(&database, "message-2", "request-2", 1, 4, 9_000).await;
    seed_dispatch(&database, "message-3", "request-3", 2, 5, 9_001).await;
    seed_dispatch(&database, "message-4", "request-4", 3, 6, 9_002).await;
    mutate_dispatch(&database, "message-2", |model| {
        model.state = Set(DispatchState::Completed);
    })
    .await;
    mutate_dispatch(&database, "message-3", |model| {
        model.state = Set(DispatchState::Failed);
    })
    .await;
    assert_eq!(claim_seeded(&repository, 0x11).await, 1);

    let unknown = repository
        .complete_message_dispatch(complete("message-zzz", 0x11, 12))
        .await;
    assert!(matches!(
        unknown,
        Err(RepositoryError::DispatchNotFound { .. })
    ));

    let foreign = repository
        .fail_message_dispatch(fail("message-1", 0x22, 12, "foreign owner"))
        .await;
    assert!(matches!(
        foreign,
        Err(RepositoryError::DispatchOwnerMismatch { .. })
    ));

    let dead = repository
        .requeue_message_dispatch(requeue("message-1", 0x11, 20, 99, "dead lease"))
        .await;
    assert!(matches!(
        dead,
        Err(RepositoryError::DispatchLeaseExpired {
            lease_expires_at_ms: 20,
            operated_at_ms: 20,
            ..
        })
    ));

    let completed_state = repository
        .fail_message_dispatch(fail("message-2", 0x11, 12, "already done"))
        .await;
    assert!(matches!(
        completed_state,
        Err(RepositoryError::InvalidDispatchState { state, .. }) if state == "completed"
    ));
    let failed_state = repository
        .complete_message_dispatch(complete("message-3", 0x11, 12))
        .await;
    assert!(matches!(
        failed_state,
        Err(RepositoryError::InvalidDispatchState { state, .. }) if state == "failed"
    ));
    let queued_state = repository
        .requeue_message_dispatch(requeue("message-4", 0x11, 12, 50, "still queued"))
        .await;
    assert!(matches!(
        queued_state,
        Err(RepositoryError::InvalidDispatchState { state, .. }) if state == "queued"
    ));

    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.state, DispatchState::Leased);
    assert_eq!(persisted.attempt_count, 1);
    assert_eq!(persisted.lease_expires_at_ms, Some(20));
    assert_eq!(persisted.lease_owner.as_deref().map(str::len), Some(64));
    assert!(persisted.last_error.is_none());
    assert_eq!(persisted.updated_at_ms, 10);
}

#[tokio::test]
async fn transitions_before_the_claim_stamp_are_rejected_as_chronology() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    seed_dispatch(&database, "message-2", "request-2", 1, 4, 3).await;
    seed_dispatch(&database, "message-3", "request-3", 2, 5, 3).await;

    let first = repository
        .claim_next_message_dispatch(claim(0x11, 10, 20))
        .await
        .expect("first claim should succeed")
        .expect("first dispatch should be claimed");
    assert_eq!(first.message_id.as_str(), "message-1");
    let second = repository
        .claim_next_message_dispatch(claim(0x11, 11, 21))
        .await
        .expect("second claim should succeed")
        .expect("second dispatch should be claimed");
    assert_eq!(second.message_id.as_str(), "message-2");
    let third = repository
        .claim_next_message_dispatch(claim(0x11, 12, 22))
        .await
        .expect("third claim should succeed")
        .expect("third dispatch should be claimed");
    assert_eq!(third.message_id.as_str(), "message-3");

    let early_complete = repository
        .complete_message_dispatch(complete("message-1", 0x11, 5))
        .await;
    assert!(matches!(
        early_complete,
        Err(RepositoryError::InvalidChronology {
            earlier_field: "message_dispatches.updated_at_ms",
            later_field: "message_dispatches.operated_at",
        })
    ));
    let early_fail = repository
        .fail_message_dispatch(fail("message-2", 0x11, 5, "too early"))
        .await;
    assert!(matches!(
        early_fail,
        Err(RepositoryError::InvalidChronology {
            earlier_field: "message_dispatches.updated_at_ms",
            later_field: "message_dispatches.operated_at",
        })
    ));
    let early_requeue = repository
        .requeue_message_dispatch(requeue("message-3", 0x11, 5, 90, "too early"))
        .await;
    assert!(matches!(
        early_requeue,
        Err(RepositoryError::InvalidChronology {
            earlier_field: "message_dispatches.updated_at_ms",
            later_field: "message_dispatches.operated_at",
        })
    ));

    for (message_id, stamp, expiry) in [
        ("message-1", 10, 20),
        ("message-2", 11, 21),
        ("message-3", 12, 22),
    ] {
        let persisted = dispatch(&database, message_id).await;
        assert_eq!(persisted.state, DispatchState::Leased);
        assert_eq!(persisted.attempt_count, 1);
        assert_eq!(persisted.lease_expires_at_ms, Some(expiry));
        assert_eq!(
            persisted.lease_owner.as_deref(),
            Some("11".repeat(32)).as_deref()
        );
        assert_eq!(persisted.available_at_ms, 3);
        assert!(persisted.last_error.is_none());
        assert_eq!(persisted.updated_at_ms, stamp);
    }
}

#[tokio::test]
async fn operations_equal_to_the_claim_stamp_remain_allowed() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    seed_dispatch(&database, "message-2", "request-2", 1, 4, 3).await;
    seed_dispatch(&database, "message-3", "request-3", 2, 5, 3).await;

    let _first = repository
        .claim_next_message_dispatch(claim(0x11, 10, 20))
        .await
        .expect("first claim should succeed")
        .expect("first dispatch should be claimed");
    let _second = repository
        .claim_next_message_dispatch(claim(0x11, 11, 21))
        .await
        .expect("second claim should succeed")
        .expect("second dispatch should be claimed");
    let _third = repository
        .claim_next_message_dispatch(claim(0x11, 12, 22))
        .await
        .expect("third claim should succeed")
        .expect("third dispatch should be claimed");

    let completed = repository
        .complete_message_dispatch(complete("message-1", 0x11, 10))
        .await
        .expect("completion at the update stamp must remain allowed");
    assert_eq!(completed.updated_at, UnixMillis::from_millis(10));
    let failed = repository
        .fail_message_dispatch(fail("message-2", 0x11, 11, "equal stamp"))
        .await
        .expect("failure at the update stamp must remain allowed");
    assert_eq!(failed.updated_at, UnixMillis::from_millis(11));
    let requeued = repository
        .requeue_message_dispatch(requeue("message-3", 0x11, 12, 12, "equal retry"))
        .await
        .expect("requeue at the update stamp must remain allowed");
    assert_eq!(requeued.updated_at, UnixMillis::from_millis(12));

    let completed_row = dispatch(&database, "message-1").await;
    assert_eq!(completed_row.state, DispatchState::Completed);
    assert_eq!(completed_row.attempt_count, 1);
    assert_eq!(completed_row.updated_at_ms, 10);
    let failed_row = dispatch(&database, "message-2").await;
    assert_eq!(failed_row.state, DispatchState::Failed);
    assert_eq!(failed_row.attempt_count, 1);
    assert_eq!(failed_row.last_error.as_deref(), Some("equal stamp"));
    assert_eq!(failed_row.updated_at_ms, 11);
    let requeued_row = dispatch(&database, "message-3").await;
    assert_eq!(requeued_row.state, DispatchState::Queued);
    assert_eq!(requeued_row.attempt_count, 1);
    assert_eq!(requeued_row.available_at_ms, 12);
    assert_eq!(requeued_row.updated_at_ms, 12);
}

#[tokio::test]
async fn stale_owner_cannot_finish_after_reclaim_but_new_owner_can() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    assert_eq!(claim_seeded(&repository, 0x11).await, 1);

    let recovered = repository
        .claim_next_message_dispatch(claim(0x22, 20, 300))
        .await
        .expect("expired recovery should succeed")
        .expect("expired dispatch should be reclaimed by the new owner");
    assert_eq!(recovered.attempt_count, 2);

    let stale_complete = repository
        .complete_message_dispatch(complete("message-1", 0x11, 26))
        .await;
    assert!(matches!(
        stale_complete,
        Err(RepositoryError::DispatchOwnerMismatch { .. })
    ));
    let stale_fail = repository
        .fail_message_dispatch(fail("message-1", 0x11, 26, "stale failure"))
        .await;
    assert!(matches!(
        stale_fail,
        Err(RepositoryError::DispatchOwnerMismatch { .. })
    ));
    let stale_requeue = repository
        .requeue_message_dispatch(requeue("message-1", 0x11, 26, 90, "stale retry"))
        .await;
    assert!(matches!(
        stale_requeue,
        Err(RepositoryError::DispatchOwnerMismatch { .. })
    ));
    let guarded = dispatch(&database, "message-1").await;
    assert_eq!(guarded.state, DispatchState::Leased);
    assert_eq!(guarded.attempt_count, 2);
    assert_eq!(guarded.lease_expires_at_ms, Some(300));

    let finished = repository
        .complete_message_dispatch(complete("message-1", 0x22, 250))
        .await
        .expect("current owner should finish the dispatch");
    assert_eq!(finished.updated_at, UnixMillis::from_millis(250));
    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.state, DispatchState::Completed);
    assert_eq!(persisted.attempt_count, 2);
    assert!(persisted.lease_owner.is_none());
    assert!(persisted.lease_expires_at_ms.is_none());
}

#[tokio::test]
async fn malformed_persisted_owner_yields_corrupt_data_without_mutation() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    assert_eq!(claim_seeded(&repository, 0x11).await, 1);
    mutate_dispatch(&database, "message-1", |model| {
        model.lease_owner = Set(Some("zz".repeat(32)));
    })
    .await;

    let corrupted_complete = repository
        .complete_message_dispatch(complete("message-1", 0x11, 12))
        .await;
    assert!(matches!(
        corrupted_complete,
        Err(RepositoryError::CorruptData {
            field: "lease_owner",
            ..
        })
    ));
    let corrupted_fail = repository
        .fail_message_dispatch(fail("message-1", 0x11, 12, "corrupt owner"))
        .await;
    assert!(matches!(
        corrupted_fail,
        Err(RepositoryError::CorruptData {
            field: "lease_owner",
            ..
        })
    ));
    let corrupted_requeue = repository
        .requeue_message_dispatch(requeue("message-1", 0x11, 12, 80, "corrupt owner"))
        .await;
    assert!(matches!(
        corrupted_requeue,
        Err(RepositoryError::CorruptData {
            field: "lease_owner",
            ..
        })
    ));

    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.state, DispatchState::Leased);
    assert_eq!(persisted.attempt_count, 1);
    assert_eq!(persisted.lease_expires_at_ms, Some(20));
    assert_eq!(persisted.lease_owner.as_deref().map(str::len), Some(64));
    assert_eq!(persisted.updated_at_ms, 10);
}

#[tokio::test]
async fn concurrent_complete_and_fail_admit_exactly_one_winner() {
    let temporary = TemporaryDatabase::new("message-dispatch-lifecycle-contention");
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
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;

    let claiming_repository = Repository::new(database.clone());
    let completing_repository = Repository::new(database.clone());
    let failing_repository = Repository::new(database.clone());
    assert_eq!(claim_seeded(&claiming_repository, 0x11).await, 1);

    let (completed, failed) = tokio::join!(
        completing_repository.complete_message_dispatch(complete("message-1", 0x11, 15)),
        failing_repository.fail_message_dispatch(fail("message-1", 0x11, 15, "lost race")),
    );

    let successes = [completed.is_ok(), failed.is_ok()]
        .into_iter()
        .filter(|succeeded| *succeeded)
        .count();
    assert_eq!(successes, 1, "exactly one fenced transition must win");
    match (&completed, &failed) {
        (Ok(_), Err(error)) | (Err(error), Ok(_)) => assert!(matches!(
            error,
            RepositoryError::InvalidDispatchState { .. }
        )),
        _ => panic!("exactly one concurrent transition should fail"),
    }

    let persisted = dispatch(&database, "message-1").await;
    assert!(
        persisted.state == DispatchState::Completed || persisted.state == DispatchState::Failed
    );
    assert_eq!(persisted.attempt_count, 1);
    assert!(persisted.lease_owner.is_none());
    assert!(persisted.lease_expires_at_ms.is_none());
    assert_eq!(persisted.updated_at_ms, 15);
    if persisted.state == DispatchState::Failed {
        assert_eq!(persisted.last_error.as_deref(), Some("lost race"));
    } else {
        assert!(persisted.last_error.is_none());
    }
}

#[tokio::test]
async fn repository_errors_redact_owner_hex_and_failure_text() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    assert_eq!(claim_seeded(&repository, 0x11).await, 1);

    let marker = "detonation payload kept secret";
    let foreign_failure = repository
        .fail_message_dispatch(fail("message-1", 0x22, 12, marker))
        .await
        .expect_err("foreign-owner failure must be rejected");
    let dead_requeue = repository
        .requeue_message_dispatch(requeue("message-1", 0x11, 20, 99, marker))
        .await
        .expect_err("exact-expiry requeue must be rejected");

    let owner_hexes = ["11".repeat(32), "22".repeat(32)];
    for hex in &owner_hexes {
        assert_eq!(hex.len(), 64, "one 32-byte owner encodes to 64 hex chars");
    }
    for error in [&foreign_failure, &dead_requeue] {
        let rendered_display = error.to_string();
        let rendered_debug = format!("{error:?}");
        for rendered in [&rendered_display, &rendered_debug] {
            for hex in &owner_hexes {
                assert!(!rendered.contains(hex.as_str()));
            }
            assert!(!rendered.contains(marker));
        }
    }
    assert!(foreign_failure.to_string().contains("message-1"));
    assert!(dead_requeue.to_string().contains("message-1"));

    mutate_dispatch(&database, "message-1", |model| {
        model.lease_owner = Set(Some("zz".repeat(32)));
    })
    .await;
    let corrupt = repository
        .complete_message_dispatch(complete("message-1", 0x11, 12))
        .await
        .expect_err("malformed owner must surface corrupt data");
    let rendered = corrupt.to_string();
    assert!(!rendered.contains("zz"));
}

#[test]
fn failure_reason_bounds_are_enforced_in_bytes() {
    assert!(matches!(
        DispatchFailureReason::parse(""),
        Err(DispatchFailureReasonError::Empty)
    ));

    let oversized = "x".repeat(4097);
    let too_long = DispatchFailureReason::parse(oversized.clone())
        .expect_err("4097-byte reasons must be rejected");
    assert!(matches!(
        too_long,
        DispatchFailureReasonError::TooLong {
            length: 4097,
            maximum: 4096,
        }
    ));
    let rendered = too_long.to_string();
    assert!(rendered.contains("4097"));
    assert!(rendered.contains("4096"));
    assert!(!rendered.contains("xxxx"));

    let maximum_ascii =
        DispatchFailureReason::parse("x".repeat(4096)).expect("4096-byte reasons must be accepted");
    assert_eq!(maximum_ascii.as_str().len(), 4096);

    let multibyte_boundary = "é".repeat(2048);
    let maximum_multibyte = DispatchFailureReason::parse(multibyte_boundary.clone())
        .expect("4096 UTF-8 bytes of multibyte text must be accepted");
    assert_eq!(maximum_multibyte.as_str().len(), 4096);
    let over_bytes = DispatchFailureReason::parse(format!("{multibyte_boundary}x"))
        .expect_err("byte length, not character count, must bound the reason");
    assert!(matches!(
        over_bytes,
        DispatchFailureReasonError::TooLong { length: 4097, .. }
    ));
}

#[tokio::test]
async fn maximum_length_reason_is_persisted_verbatim() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3).await;
    assert_eq!(claim_seeded(&repository, 0x11).await, 1);

    let reason = format!("{}{}", "é".repeat(2046), "tail");
    assert_eq!(reason.len(), 4096);
    repository
        .fail_message_dispatch(fail("message-1", 0x11, 18, &reason))
        .await
        .expect("maximum-length failure should persist");
    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.last_error.as_deref(), Some(reason.as_str()));
}

struct TemporaryDatabase {
    path: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "artisan-{label}-{}-{nanos}-{sequence}.sqlite3",
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
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(format!("{}-wal", self.path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", self.path.display()));
    }
}
