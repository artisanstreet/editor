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
const RECEIPTS_MIGRATION: &str = "m20260824_000002_global_command_receipts";
const EXECUTION_MIGRATION: &str = "m20260824_000003_conversation_execution";
const ENGINE_CONFIG_MIGRATION: &str = "m20260830_000004_engine_run_config";

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
        "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name IN ('attached_projects', 'threads', 'messages', 'message_dispatches', 'command_receipts', 'conversation_state', 'conversation_ordinals', 'conversation_turns', 'assistant_runs', 'conversation_items', 'conversation_patches', 'run_checkpoints', 'run_batch_receipts')",
    )
    .await
}

#[tokio::test]
async fn empty_file_migrates_and_repeated_startup_is_idempotent() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("restart")?;
    let first = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;

    migrate_to_current(&first).await?;
    migrate_to_current(&first).await?;
    assert_eq!(native_table_count(&first).await?, 13);
    assert_eq!(
        scalar_i64(&first, "SELECT count(*) FROM seaql_migrations").await?,
        4
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
    assert_eq!(native_table_count(&reopened).await?, 13);
    assert_eq!(
        scalar_i64(&reopened, "SELECT count(*) FROM seaql_migrations").await?,
        4
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
async fn migration_records_both_immutable_versions_in_order() -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&database).await?;

    let rows = database
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT version FROM seaql_migrations ORDER BY applied_at ASC, version ASC",
        ))
        .await?;
    let versions = rows
        .iter()
        .map(|row| row.try_get_by_index::<String>(0))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        versions,
        [
            INITIAL_MIGRATION.to_string(),
            RECEIPTS_MIGRATION.to_string(),
            EXECUTION_MIGRATION.to_string(),
            ENGINE_CONFIG_MIGRATION.to_string()
        ]
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn controlled_down_and_reapply_restore_the_schema() -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&database).await?;
    assert_eq!(native_table_count(&database).await?, 13);

    Migrator::down(&database, None).await?;
    assert_eq!(native_table_count(&database).await?, 0);

    migrate_to_current(&database).await?;
    assert_eq!(native_table_count(&database).await?, 13);
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn receipt_migration_upgrades_an_existing_initial_schema() -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    Migrator::up(&database, Some(1)).await?;
    database
        .execute_unprepared(
            "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('p1', 'C:/work/p1', 'Project', 1)",
        )
        .await?;

    migrate_to_current(&database).await?;

    assert_eq!(native_table_count(&database).await?, 13);
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT count(*) FROM attached_projects WHERE project_id = 'p1'",
        )
        .await?,
        1
    );
    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM command_receipts").await?,
        0
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn command_receipts_enforce_global_identity_exact_shapes_and_relations()
-> Result<(), Box<dyn Error>> {
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
            "INSERT INTO command_receipts (request_id, command_kind, directory_id, project_id, accepted_at_ms) VALUES ('r1', 'attach_project', 'd1', 'p1', 1)",
        )
        .await?;
    let reused_across_kinds = database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, project_id, thread_id, title, accepted_at_ms) VALUES ('r1', 'create_thread', 'p1', 't1', 'Thread', 2)",
        )
        .await;
    assert!(reused_across_kinds.is_err());

    database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, project_id, thread_id, title, accepted_at_ms) VALUES ('r2', 'create_thread', 'p1', 't1', 'Thread', 2)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, message_id, body, accepted_at_ms) VALUES ('r3', 'queue_first_message', 't1', 'm1', 'hello', 3)",
        )
        .await?;

    let mixed_shape = database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, directory_id, project_id, title, accepted_at_ms) VALUES ('r4', 'attach_project', 'd1', 'p1', 'not allowed', 4)",
        )
        .await;
    assert!(mixed_shape.is_err());
    let missing_result = database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, directory_id, accepted_at_ms) VALUES ('r5', 'attach_project', 'd1', 5)",
        )
        .await;
    assert!(missing_result.is_err());
    let missing_foreign_key = database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, project_id, thread_id, title, accepted_at_ms) VALUES ('r6', 'create_thread', 'p1', 'missing', 'Thread', 6)",
        )
        .await;
    assert!(missing_foreign_key.is_err());

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

#[tokio::test]
async fn engine_config_migration_preserves_legacy_receipts_and_allows_set_history()
-> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    Migrator::up(&database, Some(3)).await?;
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
            "INSERT INTO command_receipts (request_id, command_kind, directory_id, project_id, accepted_at_ms) VALUES ('legacy-r1', 'attach_project', 'd1', 'p1', 3)",
        )
        .await?;

    migrate_to_current(&database).await?;

    let legacy = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT command_kind, engine_run_config_version, engine_run_config FROM command_receipts WHERE request_id = 'legacy-r1'",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("legacy receipt did not survive migration"))?;
    let legacy_kind: String = legacy.try_get_by_index(0)?;
    let legacy_version: Option<i64> = legacy.try_get_by_index(1)?;
    let legacy_blob: Option<Vec<u8>> = legacy.try_get_by_index(2)?;
    assert_eq!(legacy_kind, "attach_project");
    assert!(legacy_version.is_none());
    assert!(legacy_blob.is_none());
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT engine_run_config_revision FROM threads WHERE thread_id = 't1'",
        )
        .await?,
        0
    );

    database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_result_revision) VALUES ('engine-r1', 'set_thread_engine_config', 't1', 4, 1, X'00', 1)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_expected_revision, engine_run_config_result_revision) VALUES ('engine-r2', 'set_thread_engine_config', 't1', 5, 1, X'01', 1, 2)",
        )
        .await?;
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT count(*) FROM command_receipts WHERE command_kind = 'set_thread_engine_config' AND thread_id = 't1'",
        )
        .await?,
        2
    );

    let oversized_receipt = database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_result_revision) VALUES ('engine-oversized', 'set_thread_engine_config', 't1', 6, 1, zeroblob(65537), 3)",
        )
        .await;
    assert!(oversized_receipt.is_err());
    let zero_expected_revision = database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_expected_revision, engine_run_config_result_revision) VALUES ('engine-zero-expected', 'set_thread_engine_config', 't1', 7, 1, X'02', 0, 3)",
        )
        .await;
    assert!(zero_expected_revision.is_err());
    let zero_result_revision = database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_result_revision) VALUES ('engine-zero-result', 'set_thread_engine_config', 't1', 8, 1, X'03', 0)",
        )
        .await;
    assert!(zero_result_revision.is_err());

    let invalid_thread_shape = database
        .execute_unprepared(
            "UPDATE threads SET engine_run_config_revision = 1 WHERE thread_id = 't1'",
        )
        .await;
    assert!(invalid_thread_shape.is_err());

    let index_sql = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT sql FROM sqlite_schema WHERE type = 'index' AND name = 'uq_command_receipts_kind_thread_id'",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("engine receipt index did not survive migration"))?;
    let index_sql: String = index_sql.try_get_by_index(0)?;
    assert!(index_sql.contains("WHERE command_kind <> 'set_thread_engine_config'"));
    database.close().await?;
    Ok(())
}
