use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{self, DispatchState};
use artisan_database::{
    ClaimMessageDispatch, DispatchLeaseOwner, Repository, RepositoryError, SqliteConfig, connect,
};
use artisan_domain::UnixMillis;
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait, IntoActiveModel,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

// These ambiguity checks fail to compile if the secret accidentally gains a
// common formatting or duplication trait. They need no test-only dependency.
const _: fn() = || {
    struct DebugMarker;
    trait AmbiguousIfDebug<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
    impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<DebugMarker> for T {}
    let _ = <DispatchLeaseOwner as AmbiguousIfDebug<_>>::marker;
};

const _: fn() = || {
    struct DisplayMarker;
    trait AmbiguousIfDisplay<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDisplay<()> for T {}
    impl<T: ?Sized + std::fmt::Display> AmbiguousIfDisplay<DisplayMarker> for T {}
    let _ = <DispatchLeaseOwner as AmbiguousIfDisplay<_>>::marker;
};

const _: fn() = || {
    struct CloneMarker;
    trait AmbiguousIfClone<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<CloneMarker> for T {}
    let _ = <DispatchLeaseOwner as AmbiguousIfClone<_>>::marker;
};

const _: fn() = || {
    struct PartialEqMarker;
    trait AmbiguousIfPartialEq<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfPartialEq<()> for T {}
    impl<T: ?Sized + PartialEq> AmbiguousIfPartialEq<PartialEqMarker> for T {}
    let _ = <DispatchLeaseOwner as AmbiguousIfPartialEq<_>>::marker;
};

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
    attempt_count: i32,
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
        attempt_count: Set(attempt_count),
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

fn claim(owner_byte: u8, claimed_at_ms: i64, lease_expires_at_ms: i64) -> ClaimMessageDispatch {
    ClaimMessageDispatch {
        owner: DispatchLeaseOwner::new([owner_byte; 32]),
        claimed_at: UnixMillis::from_millis(claimed_at_ms),
        lease_expires_at: UnixMillis::from_millis(lease_expires_at_ms),
    }
}

async fn dispatch(database: &DatabaseConnection, message_id: &str) -> entities::MessageDispatch {
    entities::message_dispatch::Entity::find_by_id(message_id)
        .one(database)
        .await
        .expect("dispatch should query")
        .expect("dispatch should exist")
}

#[tokio::test]
async fn empty_queue_returns_no_claim() {
    let (_database, repository) = memory_database().await;

    let claimed = repository
        .claim_next_message_dispatch(claim(0x11, 10, 20))
        .await
        .expect("empty claim should succeed");

    assert!(claimed.is_none());
}

#[tokio::test]
async fn claim_is_deterministic_atomic_and_redacts_ownership() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-c", "request-c", 0, 5, 7, 0).await;
    seed_dispatch(&database, "message-b", "request-b", 1, 4, 8, 0).await;
    seed_dispatch(&database, "message-a", "request-a", 2, 4, 8, 0).await;

    let claimed = repository
        .claim_next_message_dispatch(claim(0xab, 10, 20))
        .await
        .expect("claim should succeed")
        .expect("one dispatch should be eligible");
    assert_eq!(claimed.message_id.as_str(), "message-c");
    assert_eq!(claimed.correlation_id.as_str(), "request-c");
    assert_eq!(claimed.attempt_count, 1);
    assert!(
        claimed
            .owner
            .constant_time_eq(&DispatchLeaseOwner::new([0xab; 32]))
    );
    assert!(
        !claimed
            .owner
            .constant_time_eq(&DispatchLeaseOwner::new([0xac; 32]))
    );
    assert_eq!(claimed.queued_at, UnixMillis::from_millis(5));
    assert_eq!(claimed.available_at, UnixMillis::from_millis(7));
    assert_eq!(claimed.lease_expires_at, UnixMillis::from_millis(20));
    assert_eq!(claimed.updated_at, UnixMillis::from_millis(10));

    let persisted = dispatch(&database, "message-c").await;
    assert_eq!(persisted.state, DispatchState::Leased);
    assert_eq!(persisted.attempt_count, 1);
    assert_eq!(persisted.lease_owner.as_deref().map(str::len), Some(64));
    assert_eq!(persisted.lease_expires_at_ms, Some(20));
    assert_eq!(persisted.updated_at_ms, 10);
    assert!(persisted.last_error.is_none());

    let second = repository
        .claim_next_message_dispatch(claim(0xbc, 10, 20))
        .await
        .expect("second claim should succeed")
        .expect("another dispatch should be eligible");
    assert_eq!(second.message_id.as_str(), "message-a");
}

#[tokio::test]
async fn competing_claimers_cannot_share_one_dispatch() {
    let temporary = TemporaryDatabase::new("message-dispatch-contention");
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
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3, 0).await;

    let first_repository = Repository::new(database.clone());
    let second_repository = Repository::new(database.clone());
    let (first, second) = tokio::join!(
        first_repository.claim_next_message_dispatch(claim(0x11, 10, 20)),
        second_repository.claim_next_message_dispatch(claim(0x22, 10, 20)),
    );
    let results = [
        first.expect("first competing claim should not fail"),
        second.expect("second competing claim should not fail"),
    ];
    assert_eq!(results.iter().filter(|result| result.is_some()).count(), 1);
    let winner = results
        .iter()
        .find_map(Option::as_ref)
        .expect("one claim should win");
    assert!(
        winner
            .owner
            .constant_time_eq(&DispatchLeaseOwner::new([0x11; 32]))
            || winner
                .owner
                .constant_time_eq(&DispatchLeaseOwner::new([0x22; 32]))
    );

    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.state, DispatchState::Leased);
    assert_eq!(persisted.attempt_count, 1);
    assert_eq!(persisted.lease_expires_at_ms, Some(20));
    assert_eq!(persisted.lease_owner.as_deref().map(str::len), Some(64));
}

#[tokio::test]
async fn lease_denies_early_reclaim_and_recovers_at_expiry() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3, 0).await;

    repository
        .claim_next_message_dispatch(claim(0x11, 10, 20))
        .await
        .expect("initial claim should succeed")
        .expect("initial claim should return the dispatch");
    let mut persisted = dispatch(&database, "message-1").await.into_active_model();
    persisted.last_error = Set(Some("worker crashed".to_owned()));
    persisted
        .update(&database)
        .await
        .expect("crash fixture should update");

    let denied = repository
        .claim_next_message_dispatch(claim(0x22, 19, 30))
        .await
        .expect("pre-expiry lookup should succeed");
    assert!(denied.is_none());
    let before_expiry = dispatch(&database, "message-1").await;
    assert_eq!(before_expiry.attempt_count, 1);
    assert_eq!(before_expiry.lease_owner.as_deref().map(str::len), Some(64));
    assert_eq!(before_expiry.last_error.as_deref(), Some("worker crashed"));

    let recovered = repository
        .claim_next_message_dispatch(claim(0x22, 20, 30))
        .await
        .expect("expired dispatch recovery should succeed")
        .expect("expired dispatch should be reclaimed");
    assert_eq!(recovered.message_id.as_str(), "message-1");
    assert_eq!(recovered.attempt_count, 2);
    assert!(
        recovered
            .owner
            .constant_time_eq(&DispatchLeaseOwner::new([0x22; 32]))
    );
    let after_expiry = dispatch(&database, "message-1").await;
    assert_eq!(after_expiry.attempt_count, 2);
    assert_eq!(after_expiry.lease_owner.as_deref().map(str::len), Some(64));
    assert_eq!(after_expiry.lease_expires_at_ms, Some(30));
    assert!(after_expiry.last_error.is_none());
}

#[tokio::test]
async fn exhausted_queued_head_does_not_block_later_claimable_dispatch() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(
        &database,
        "message-exhausted",
        "request-exhausted",
        0,
        3,
        3,
        i32::MAX,
    )
    .await;
    seed_dispatch(&database, "message-ready", "request-ready", 1, 4, 4, 0).await;
    let exhausted_before = dispatch(&database, "message-exhausted").await;

    let claimed = repository
        .claim_next_message_dispatch(claim(0x11, 10, 20))
        .await
        .expect("claim should skip the exhausted head")
        .expect("later eligible dispatch should be claimed");
    assert_eq!(claimed.message_id.as_str(), "message-ready");
    assert_eq!(claimed.attempt_count, 1);

    let exhausted_after = dispatch(&database, "message-exhausted").await;
    assert_eq!(exhausted_after, exhausted_before);
    let ready_after = dispatch(&database, "message-ready").await;
    assert_eq!(ready_after.state, DispatchState::Leased);
    assert_eq!(ready_after.attempt_count, 1);
    assert_eq!(ready_after.lease_expires_at_ms, Some(20));

    let exhausted = repository
        .claim_next_message_dispatch(claim(0x22, 10, 30))
        .await;
    assert!(matches!(
        exhausted,
        Err(RepositoryError::DispatchAttemptLimit { message_id })
            if message_id.as_str() == "message-exhausted"
    ));
    assert_eq!(
        dispatch(&database, "message-exhausted").await,
        exhausted_before
    );
}

#[tokio::test]
async fn exhausted_expired_lease_does_not_block_later_claimable_dispatch() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(
        &database,
        "message-exhausted",
        "request-exhausted",
        0,
        3,
        3,
        i32::MAX,
    )
    .await;
    seed_dispatch(&database, "message-ready", "request-ready", 1, 4, 4, 0).await;
    let mut exhausted = dispatch(&database, "message-exhausted")
        .await
        .into_active_model();
    exhausted.state = Set(DispatchState::Leased);
    exhausted.lease_owner = Set(Some("11".repeat(32)));
    exhausted.lease_expires_at_ms = Set(Some(5));
    exhausted.last_error = Set(Some("worker crashed on final attempt".to_owned()));
    exhausted.updated_at_ms = Set(4);
    exhausted
        .update(&database)
        .await
        .expect("expired exhausted fixture should update");
    let exhausted_before = dispatch(&database, "message-exhausted").await;

    let claimed = repository
        .claim_next_message_dispatch(claim(0x22, 10, 20))
        .await
        .expect("claim should skip the exhausted expired lease")
        .expect("later eligible dispatch should be claimed");
    assert_eq!(claimed.message_id.as_str(), "message-ready");
    assert_eq!(claimed.attempt_count, 1);

    assert_eq!(
        dispatch(&database, "message-exhausted").await,
        exhausted_before
    );
}

#[tokio::test]
async fn invalid_transitions_leave_the_dispatch_unchanged() {
    let (database, repository) = memory_database().await;
    seed_foundation(&database).await;
    seed_dispatch(&database, "message-1", "request-1", 0, 3, 3, i32::MAX).await;

    let invalid_window = repository
        .claim_next_message_dispatch(claim(0x11, 10, 10))
        .await;
    assert!(matches!(
        invalid_window,
        Err(RepositoryError::InvalidDispatchLeaseWindow { .. })
    ));
    let exhausted = repository
        .claim_next_message_dispatch(claim(0x22, 10, 20))
        .await;
    assert!(matches!(
        exhausted,
        Err(RepositoryError::DispatchAttemptLimit { .. })
    ));

    let persisted = dispatch(&database, "message-1").await;
    assert_eq!(persisted.state, DispatchState::Queued);
    assert_eq!(persisted.attempt_count, i32::MAX);
    assert!(persisted.lease_owner.is_none());
    assert!(persisted.lease_expires_at_ms.is_none());
    assert_eq!(persisted.last_error.as_deref(), Some("previous failure"));
    assert_eq!(persisted.updated_at_ms, 3);
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
