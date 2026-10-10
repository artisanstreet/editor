//! Where threads live: settling into History, the Forge filing quiet
//! threads, renames that generated titles never replace, and the archive.

use artisan_database::{AttachProjectInput, Repository, SqliteConfig, connect};
use artisan_domain::{
    ARCHIVE_RETENTION_MS, DirectoryId, DisplayName, HISTORY_IDLE_MS, ProjectId, RequestId,
    RootPath, ThreadChange, ThreadId, ThreadPlacement, ThreadTitle, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ConnectionTrait, DatabaseConnection};

const NOW: i64 = 10 * HISTORY_IDLE_MS;

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
            request_id: RequestId::parse("attach").unwrap(),
            directory_id: DirectoryId::parse("directory").unwrap(),
            project_id: ProjectId::parse("project").unwrap(),
            root_path: RootPath::parse("C:/repos/artisan").unwrap(),
            display_name: DisplayName::parse("Artisan").unwrap(),
            attached_at: UnixMillis::from_millis(1),
        })
        .await
        .expect("project should attach");
    (database, repository)
}

/// A saved thread whose one message was sent at `message_at`.
async fn seed(database: &DatabaseConnection, thread: &str, message_at: i64) {
    for statement in [
        format!(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms, engine_run_config_revision) \
             VALUES ('{thread}', 'project', 'New thread', 1, 1, 0)"
        ),
        format!(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) \
             VALUES ('{thread}-message', '{thread}', 0, 'Hello', {message_at})"
        ),
    ] {
        database
            .execute_unprepared(&statement)
            .await
            .expect("thread fixture should insert");
    }
}

async fn view(database: &DatabaseConnection, thread: &str, at: i64) {
    database
        .execute_unprepared(&format!(
            "INSERT INTO thread_reads (thread_id, read_at_ms) VALUES ('{thread}', {at}) \
             ON CONFLICT(thread_id) DO UPDATE SET read_at_ms = excluded.read_at_ms"
        ))
        .await
        .expect("read should record");
}

fn id(thread: &str) -> ThreadId {
    ThreadId::parse(thread).unwrap()
}

async fn change(repository: &Repository, thread: &str, change: ThreadChange, at: i64) {
    repository
        .update_thread(&id(thread), &change, UnixMillis::from_millis(at))
        .await
        .expect("thread should change");
}

async fn placements(repository: &Repository) -> Vec<(String, ThreadPlacement)> {
    repository
        .list_recent_threads(100)
        .await
        .expect("recent threads should list")
        .into_iter()
        .map(|(summary, placement)| (summary.thread_id.as_str().to_owned(), placement))
        .collect()
}

#[tokio::test]
async fn a_settled_thread_stays_in_history_until_its_next_activity() {
    let (database, repository) = repository().await;
    seed(&database, "thread", NOW - 10).await;
    assert_eq!(
        placements(&repository).await,
        [("thread".to_owned(), ThreadPlacement::Inbox)]
    );
    let before = repository.recent_threads_fingerprint().await.unwrap();
    change(&repository, "thread", ThreadChange::Settle, NOW).await;
    assert_ne!(
        repository.recent_threads_fingerprint().await.unwrap(),
        before,
        "settling pushes the listing"
    );
    assert_eq!(
        placements(&repository).await,
        [("thread".to_owned(), ThreadPlacement::History)]
    );
    // Viewing a settled thread leaves it where it is.
    view(&database, "thread", NOW + 5).await;
    assert_eq!(
        placements(&repository).await,
        [("thread".to_owned(), ThreadPlacement::History)]
    );
    database
        .execute_unprepared(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) \
             VALUES ('next', 'thread', 1, 'Again', 10000000000)",
        )
        .await
        .unwrap();
    assert_eq!(
        placements(&repository).await,
        [("thread".to_owned(), ThreadPlacement::Inbox)],
        "a new message brings it back into view"
    );
}

#[tokio::test]
async fn the_forge_files_threads_left_alone_for_a_day() {
    let (database, repository) = repository().await;
    let stale = NOW - HISTORY_IDLE_MS - 1;
    seed(&database, "quiet", stale).await;
    seed(&database, "recent", NOW - 10).await;
    seed(&database, "viewed", stale).await;
    view(&database, "viewed", NOW - 10).await;
    seed(&database, "open", stale).await;
    database
        .execute_unprepared(
            "UPDATE user_preferences SET route_project_id = 'project', route_thread_id = 'open'",
        )
        .await
        .unwrap();

    let filed = repository
        .file_quiet_threads(UnixMillis::from_millis(NOW))
        .await
        .expect("quiet threads should file");
    assert_eq!(filed, 1);
    let placed = placements(&repository).await;
    for (thread, expected) in [
        ("quiet", ThreadPlacement::History),
        ("recent", ThreadPlacement::Inbox),
        ("viewed", ThreadPlacement::Inbox),
        ("open", ThreadPlacement::Inbox),
    ] {
        assert!(
            placed.contains(&(thread.to_owned(), expected)),
            "{thread} should be {expected:?}: {placed:?}"
        );
    }
    assert_eq!(
        repository
            .file_quiet_threads(UnixMillis::from_millis(NOW + 1))
            .await
            .unwrap(),
        0,
        "a filed thread is not filed again"
    );
}

#[tokio::test]
async fn a_rename_survives_the_generated_title() {
    let (database, repository) = repository().await;
    seed(&database, "thread", NOW).await;
    let title = ThreadTitle::parse("New thread").unwrap();
    change(&repository, "thread", ThreadChange::Rename(title), NOW).await;
    repository
        .record_generated_thread_title(&id("thread"), &ThreadTitle::parse("Generated").unwrap())
        .await
        .unwrap();
    let listed = repository.list_recent_threads(100).await.unwrap();
    assert_eq!(
        listed[0].0.title.as_str(),
        "Hello",
        "a placeholder title still reads as the first message"
    );
    let stored: String = database
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT title FROM threads WHERE thread_id = 'thread'",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index(0)
        .unwrap();
    assert_eq!(stored, "New thread", "the reader's title is kept");

    let renamed = ThreadTitle::parse("Mine").unwrap();
    change(&repository, "thread", ThreadChange::Rename(renamed), NOW).await;
    assert_eq!(
        repository.list_recent_threads(100).await.unwrap()[0]
            .0
            .title
            .as_str(),
        "Mine"
    );
}

#[tokio::test]
async fn archived_threads_leave_every_listing_until_restored() {
    let (database, repository) = repository().await;
    seed(&database, "kept", NOW).await;
    seed(&database, "first", NOW - 2 * HISTORY_IDLE_MS).await;
    seed(&database, "second", NOW - 2 * HISTORY_IDLE_MS).await;
    change(&repository, "second", ThreadChange::Archive, NOW - 5).await;
    change(&repository, "first", ThreadChange::Archive, NOW - 9).await;
    // Archiving again keeps the original moment.
    change(&repository, "first", ThreadChange::Archive, NOW).await;

    assert_eq!(
        placements(&repository).await,
        [("kept".to_owned(), ThreadPlacement::Inbox)]
    );
    let project = repository
        .list_threads(&ProjectId::parse("project").unwrap())
        .await
        .unwrap();
    assert_eq!(project.threads().len(), 1);
    let archived = repository.list_archived_threads(100).await.unwrap();
    assert_eq!(
        archived
            .iter()
            .map(|(summary, at)| (summary.thread_id.as_str(), at.as_millis()))
            .collect::<Vec<_>>(),
        [("first", NOW - 9), ("second", NOW - 5)]
    );
    assert_eq!(
        repository
            .file_quiet_threads(UnixMillis::from_millis(NOW))
            .await
            .unwrap(),
        0,
        "archived threads are not filed"
    );

    change(&repository, "first", ThreadChange::Unarchive, NOW).await;
    assert_eq!(
        repository.list_archived_threads(100).await.unwrap().len(),
        1
    );
    assert!(
        placements(&repository)
            .await
            .contains(&("first".to_owned(), ThreadPlacement::Inbox))
    );
    assert_eq!(
        repository
            .file_quiet_threads(UnixMillis::from_millis(NOW + 1))
            .await
            .unwrap(),
        0,
        "a restored thread counts as just seen"
    );
}

#[tokio::test]
async fn expired_archived_threads_are_erased() {
    let (database, repository) = repository().await;
    seed(&database, "expired", 1).await;
    seed(&database, "young", 1).await;
    change(&repository, "expired", ThreadChange::Archive, 10).await;
    change(&repository, "young", ThreadChange::Archive, 20).await;

    let at = UnixMillis::from_millis(10 + ARCHIVE_RETENTION_MS);
    let erased = repository
        .erase_expired_archived_threads(at, 10)
        .await
        .expect("expired threads should erase");
    assert_eq!(erased, [id("expired")]);
    let remaining: i64 = database
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Sqlite,
            "SELECT (SELECT COUNT(*) FROM threads) * 100 + (SELECT COUNT(*) FROM messages) * 10 \
             + (SELECT COUNT(*) FROM thread_lifecycle)",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index(0)
        .unwrap();
    assert_eq!(remaining, 111, "only the young thread is left");
    assert!(
        repository
            .update_thread(&id("expired"), &ThreadChange::Unarchive, at)
            .await
            .is_err(),
        "an erased thread is gone"
    );
}
