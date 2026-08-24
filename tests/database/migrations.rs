//! External behavior tests for the immutable native migration set.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::{SqliteConfig, connect};
use artisan_migrations::{Migrator, migrate_to_current};
use sea_orm_migration::MigratorTrait;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

const INITIAL_MIGRATION: &str = "m20260824_000001_initial_native_schema";

struct TempDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TempDatabase {
    fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "artisan-editor-migrations-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory)?;
        let database = directory.join("forge.sqlite3");
        Ok(Self {
            directory,
            database,
        })
    }

    fn database(&self) -> &Path {
        &self.database
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.directory);
    }
}

async fn scalar_i64(
    database: &sea_orm_migration::sea_orm::DatabaseConnection,
    sql: &str,
) -> Result<i64, Box<dyn Error>> {
    let row = database
        .query_one_raw(Statement::from_string(DbBackend::Sqlite, sql))
        .await?
        .ok_or_else(|| std::io::Error::other("scalar query returned no row"))?;
    Ok(row.try_get_by_index(0)?)
}

async fn native_table_count(
    database: &sea_orm_migration::sea_orm::DatabaseConnection,
) -> Result<i64, Box<dyn Error>> {
    scalar_i64(
        database,
        "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name IN ('attached_projects', 'threads', 'messages', 'message_dispatches')",
    )
    .await
}

#[tokio::test]
async fn empty_file_migrates_and_repeated_startup_is_idempotent() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("restart")?;
    let first = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;

    migrate_to_current(&first).await?;
    migrate_to_current(&first).await?;
    assert_eq!(native_table_count(&first).await?, 4);
    assert_eq!(
        scalar_i64(&first, "SELECT count(*) FROM seaql_migrations").await?,
        1
    );
    first
        .execute_unprepared(
            "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('p1', 'C:/work/p1', 'Project', 1)",
        )
        .await?;
    first
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t1', 'p1', 'First thread', 2, 3)",
        )
        .await?;
    first
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t2', 'p1', 'Recent thread', 4, 10)",
        )
        .await?;
    first
        .execute_unprepared(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m1', 't1', 0, 'hello', 3)",
        )
        .await?;
    first
        .execute_unprepared(
            "INSERT INTO message_dispatches (message_id, correlation_id, state, attempt_count, queued_at_ms, available_at_ms, updated_at_ms) VALUES ('m1', 'c1', 'queued', 0, 3, 3, 3)",
        )
        .await?;
    first.close().await?;

    let reopened = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
    migrate_to_current(&reopened).await?;
    assert_eq!(native_table_count(&reopened).await?, 4);
    assert_eq!(
        scalar_i64(&reopened, "SELECT count(*) FROM seaql_migrations").await?,
        1
    );
    let queued = reopened
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT correlation_id, state, attempt_count FROM message_dispatches WHERE message_id = 'm1'",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("queued dispatch did not survive reopen"))?;
    let correlation_id: String = queued.try_get_by_index(0)?;
    let state: String = queued.try_get_by_index(1)?;
    let attempt_count: i64 = queued.try_get_by_index(2)?;
    assert_eq!(correlation_id, "c1");
    assert_eq!(state, "queued");
    assert_eq!(attempt_count, 0);
    let threads = reopened
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT thread_id, title, created_at_ms, updated_at_ms FROM threads ORDER BY updated_at_ms DESC, thread_id ASC",
        ))
        .await?;
    assert_eq!(threads.len(), 2);
    let most_recent = threads
        .first()
        .ok_or_else(|| std::io::Error::other("thread list was empty after reopen"))?;
    let first_thread_id: String = most_recent.try_get_by_index(0)?;
    let first_title: String = most_recent.try_get_by_index(1)?;
    let first_created_at_ms: i64 = most_recent.try_get_by_index(2)?;
    let first_updated_at_ms: i64 = most_recent.try_get_by_index(3)?;
    assert_eq!(first_thread_id, "t2");
    assert_eq!(first_title, "Recent thread");
    assert_eq!(first_created_at_ms, 4);
    assert_eq!(first_updated_at_ms, 10);
    reopened.close().await?;
    Ok(())
}

#[tokio::test]
async fn migration_records_the_immutable_initial_version() -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&database).await?;

    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT version FROM seaql_migrations",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("migration version row was not recorded"))?;
    let version: String = row.try_get_by_index(0)?;
    assert_eq!(version, INITIAL_MIGRATION);
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn controlled_down_and_reapply_restore_the_schema() -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&database).await?;
    assert_eq!(native_table_count(&database).await?, 4);

    Migrator::down(&database, None).await?;
    assert_eq!(native_table_count(&database).await?, 0);

    migrate_to_current(&database).await?;
    assert_eq!(native_table_count(&database).await?, 4);
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn thread_schema_enforces_title_and_recency_invariants() -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&database).await?;
    database
        .execute_unprepared(
            "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('p1', 'C:/work/p1', 'Project', 1)",
        )
        .await?;
    let blank_title = database
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t2', 'p1', '   ', 2, 2)",
        )
        .await;
    assert!(blank_title.is_err());
    let regressed_update = database
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t3', 'p1', 'Bad time', 3, 2)",
        )
        .await;
    assert!(regressed_update.is_err());
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn schema_enforces_queue_and_relationship_invariants() -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&database).await?;
    database
        .execute_unprepared(
            "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('p1', 'C:/work/p1', 'Project', 1)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t1', 'p1', 'Thread', 2, 2)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m1', 't1', 0, 'hello', 3)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO message_dispatches (message_id, correlation_id, state, attempt_count, queued_at_ms, available_at_ms, updated_at_ms) VALUES ('m1', 'c1', 'queued', 0, 3, 3, 3)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m2', 't1', 1, 'second', 4)",
        )
        .await?;

    let invalid_state = database
        .execute_unprepared(
            "INSERT INTO message_dispatches (message_id, correlation_id, state, attempt_count, queued_at_ms, available_at_ms, updated_at_ms) VALUES ('m2', 'c2', 'unknown', 0, 4, 4, 4)",
        )
        .await;
    assert!(invalid_state.is_err());

    let missing_lease = database
        .execute_unprepared(
            "INSERT INTO message_dispatches (message_id, correlation_id, state, attempt_count, queued_at_ms, available_at_ms, updated_at_ms) VALUES ('m2', 'c2', 'leased', 1, 4, 4, 4)",
        )
        .await;
    assert!(missing_lease.is_err());

    let duplicate_correlation = database
        .execute_unprepared(
            "INSERT INTO message_dispatches (message_id, correlation_id, state, attempt_count, queued_at_ms, available_at_ms, updated_at_ms) VALUES ('m2', 'c1', 'queued', 0, 4, 4, 4)",
        )
        .await;
    assert!(duplicate_correlation.is_err());

    let duplicate_ordinal = database
        .execute_unprepared(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m3', 't1', 0, 'duplicate', 5)",
        )
        .await;
    assert!(duplicate_ordinal.is_err());

    let negative_ordinal = database
        .execute_unprepared(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m4', 't1', -1, 'negative', 6)",
        )
        .await;
    assert!(negative_ordinal.is_err());

    database
        .execute_unprepared(
            "UPDATE message_dispatches SET state = 'leased', attempt_count = 1, lease_owner = 'forge-1', lease_expires_at_ms = 100, updated_at_ms = 10 WHERE message_id = 'm1'",
        )
        .await?;
    database
        .execute_unprepared(
            "UPDATE message_dispatches SET state = 'running', updated_at_ms = 11 WHERE message_id = 'm1'",
        )
        .await?;
    database
        .execute_unprepared(
            "UPDATE message_dispatches SET state = 'completed', lease_owner = NULL, lease_expires_at_ms = NULL, updated_at_ms = 12 WHERE message_id = 'm1'",
        )
        .await?;
    let completed = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT state FROM message_dispatches WHERE message_id = 'm1'",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("dispatch row disappeared"))?;
    let completed_state: String = completed.try_get_by_index(0)?;
    assert_eq!(completed_state, "completed");

    let negative_attempt = database
        .execute_unprepared(
            "UPDATE message_dispatches SET attempt_count = -1 WHERE message_id = 'm1'",
        )
        .await;
    assert!(negative_attempt.is_err());

    let referenced_project = database
        .execute_unprepared("DELETE FROM attached_projects WHERE project_id = 'p1'")
        .await;
    assert!(referenced_project.is_err());

    let referenced_message = database
        .execute_unprepared("DELETE FROM messages WHERE message_id = 'm1'")
        .await;
    assert!(referenced_message.is_err());
    database.close().await?;
    Ok(())
}
