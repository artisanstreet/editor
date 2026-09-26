//! External behavior tests for the immutable native migration set.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::{
    QueueMessageInput, Repository, SetThreadEngineConfigInput, SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, EngineAgentId, EngineConfigUpdatePrecondition,
    EngineModelId, EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig,
    EngineRuntimeControls, EngineRuntimeControlsInput, EngineSelection, FilesystemAccess,
    FiniteMillis, MessageId, NetworkAccess, OpenCode2Selection, PermissionId, QueueMessagePayload,
    ReceiptDisposition, RequestId, ThreadId, UnixMillis, WebSearchAccess,
};
use artisan_migrations::{Migrator, migrate_to_current};
use sea_orm_migration::MigratorTrait;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

const INITIAL_MIGRATION: &str = "m20260824_000001_initial_native_schema";
const RECEIPTS_MIGRATION: &str = "m20260824_000002_global_command_receipts";
const EXECUTION_MIGRATION: &str = "m20260824_000003_conversation_execution";
const ENGINE_CONFIG_MIGRATION: &str = "m20260830_000004_engine_run_config";
const MULTIMODAL_MIGRATION: &str = "m20260905_000005_multimodal_messages";
const MODEL_FAVORITES_MIGRATION: &str = "m20260905_000006_model_favorites";
const RUN_USAGE_MIGRATION: &str = "m20260905_000007_run_usage";
const WITHDRAWALS_MIGRATION: &str = "m20260905_000008_queued_message_withdrawals";
const ENGINE_CONFIG_V2_MIGRATION: &str = "m20260906_000009_engine_run_config_v2";
const RUN_INTERACTIONS_MIGRATION: &str = "m20260908_000010_run_interactions";
const OBSERVATION_LEDGER_MIGRATION: &str = "m20260909_000011_observation_ledger";
const STEER_TARGET_MIGRATION: &str = "m20260910_000012_message_steer_target";
const QUEUE_SNAPSHOT_MIGRATION: &str = "m20260910_000013_queue_message_config_snapshot";
const STREAMING_SPEED_MIGRATION: &str = "m20260913_000014_streaming_speed";
const COMPOSER_DRAFTS_MIGRATION: &str = "m20260925_000015_composer_drafts";
const FAILED_MESSAGE_RECOVERIES_MIGRATION: &str = "m20260926_000016_failed_message_recoveries";
const DRAFT_SUBMISSIONS_MIGRATION: &str = "m20260927_000017_composer_draft_submissions";
const ATTACHMENT_SOURCES_MIGRATION: &str = "m20260928_000018_composer_attachment_sources";
const USER_PREFERENCES_MIGRATION: &str = "m20260929_000019_user_preferences";
const CHUNKED_ATTACHMENTS_MIGRATION: &str = "m20260930_000020_chunked_composer_attachments";
const ITEM_THREAD_INDEX_MIGRATION: &str = "m20261001_000021_conversation_item_thread_index";
const PROJECT_DRAFT_SUBMISSIONS_MIGRATION: &str = "m20261002_000022_project_draft_submissions";

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

async fn migrated_engine_config_database()
-> Result<sea_orm_migration::sea_orm::DatabaseConnection, Box<dyn Error>> {
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
    Ok(database)
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
        22
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
        22
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
            ENGINE_CONFIG_MIGRATION.to_string(),
            MULTIMODAL_MIGRATION.to_string(),
            MODEL_FAVORITES_MIGRATION.to_string(),
            RUN_USAGE_MIGRATION.to_string(),
            WITHDRAWALS_MIGRATION.to_string(),
            ENGINE_CONFIG_V2_MIGRATION.to_string(),
            RUN_INTERACTIONS_MIGRATION.to_string(),
            OBSERVATION_LEDGER_MIGRATION.to_string(),
            STEER_TARGET_MIGRATION.to_string(),
            QUEUE_SNAPSHOT_MIGRATION.to_string(),
            STREAMING_SPEED_MIGRATION.to_string(),
            COMPOSER_DRAFTS_MIGRATION.to_string(),
            FAILED_MESSAGE_RECOVERIES_MIGRATION.to_string(),
            DRAFT_SUBMISSIONS_MIGRATION.to_string(),
            ATTACHMENT_SOURCES_MIGRATION.to_string(),
            USER_PREFERENCES_MIGRATION.to_string(),
            CHUNKED_ATTACHMENTS_MIGRATION.to_string(),
            ITEM_THREAD_INDEX_MIGRATION.to_string(),
            PROJECT_DRAFT_SUBMISSIONS_MIGRATION.to_string()
        ]
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn attachment_store_keeps_picked_images_and_their_draft_references()
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
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t1', 'p1', 'Thread', 2, 3)",
        )
        .await?;
    // A picked image larger than a message image (5 MiB) is stored as it is;
    // one over the upload bound (32 MiB) is not.
    let insert = |size: i64, digest: u8| {
        format!(
            "INSERT INTO composer_attachments (digest, mime_type, size_bytes, bytes, stored_at_ms) VALUES (x'{}', 'image/png', {size}, zeroblob({size}), 1)",
            format!("{digest:02x}").repeat(32)
        )
    };
    database
        .execute_unprepared(&insert(20 * 1024 * 1024, 1))
        .await?;
    assert!(
        database
            .execute_unprepared(&insert(32 * 1024 * 1024 + 1, 2))
            .await
            .is_err()
    );
    database
        .execute_unprepared(
            "INSERT INTO composer_drafts (scope_kind, scope_id, revision, body, updated_at_ms) VALUES ('thread', 't1', 1, '', 4)",
        )
        .await?;
    database
        .execute_unprepared(&format!(
            "INSERT INTO composer_draft_attachments (scope_kind, scope_id, position, digest, name) VALUES ('thread', 't1', 0, x'{}', 'screenshot.png')",
            "01".repeat(32)
        ))
        .await?;
    // The rebuilt reference still restricts deleting the stored image.
    assert!(
        database
            .execute_unprepared("DELETE FROM composer_attachments")
            .await
            .is_err()
    );
    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM composer_draft_attachments").await?,
        1
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
    let database = migrated_engine_config_database().await?;

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
    assert!(index_sql.contains(
        "WHERE command_kind IN ('attach_project', 'create_thread', 'queue_first_message')"
    ));
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn run_interactions_migration_enforces_request_and_receipt_shapes()
-> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&database).await?;
    database
        .execute_unprepared(
            "INSERT INTO pending_run_interactions (run_id, interaction_id, thread_id, kind, state, request_json, requested_sequence, requested_at_ms, binding_version) VALUES ('run-1', 'approval-1', 'thread-1', 'approval', 'requested', '{\"description\":\"x\"}', 1, 100, 1)",
        )
        .await?;
    // Resolving without the kind-matching decision is rejected.
    let resolved_without_decision = database
        .execute_unprepared(
            "UPDATE pending_run_interactions SET state = 'resolved', resolved_at_ms = 200, resolved_sequence = 2 WHERE run_id = 'run-1'",
        )
        .await;
    assert!(resolved_without_decision.is_err());
    database
        .execute_unprepared(
            "UPDATE pending_run_interactions SET state = 'resolved', approved = 0, resolved_at_ms = 200, resolved_sequence = 2 WHERE run_id = 'run-1'",
        )
        .await?;
    // Receipts echo the decision for approvals and the answers for questions.
    database
        .execute_unprepared(
            "INSERT INTO run_interaction_receipts (request_id, command_kind, thread_id, run_id, interaction_id, outcome, disposition, intent_key, approved, binding_version, responded_at_ms) VALUES ('req-1', 'respond_approval', 'thread-1', 'run-1', 'approval-1', 'applied', 'accepted', 'intent', 0, 1, 200)",
        )
        .await?;
    let question_without_answers = database
        .execute_unprepared(
            "INSERT INTO run_interaction_receipts (request_id, command_kind, thread_id, run_id, interaction_id, outcome, disposition, intent_key, binding_version, responded_at_ms) VALUES ('req-2', 'respond_question', 'thread-1', 'run-1', 'q-1', 'applied', 'accepted', 'intent', 1, 200)",
        )
        .await;
    assert!(question_without_answers.is_err());
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn engine_config_migration_enforces_thread_and_run_snapshot_shapes()
-> Result<(), Box<dyn Error>> {
    let database = migrated_engine_config_database().await?;
    database
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('t2', 'p1', 'Configured thread', 4, 4, 1, 1, X'00')",
        )
        .await?;
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT engine_run_config_version FROM threads WHERE thread_id = 't2'",
        )
        .await?,
        1
    );
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT engine_run_config_revision FROM threads WHERE thread_id = 't2'",
        )
        .await?,
        1
    );
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT length(engine_run_config) FROM threads WHERE thread_id = 't2'",
        )
        .await?,
        1
    );

    let invalid_partial_thread = database
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('t3', 'p1', 'Partial thread', 5, 5, 1, 1, NULL)",
        )
        .await;
    assert!(invalid_partial_thread.is_err());
    let oversized_thread = database
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('t4', 'p1', 'Oversized thread', 6, 6, 1, 1, zeroblob(65537))",
        )
        .await;
    assert!(oversized_thread.is_err());

    database
        .execute_unprepared(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m1', 't1', 0, 'hello', 7)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO conversation_ordinals (thread_id, ordinal, kind, entity_id) VALUES ('t1', 0, 'turn', 'turn1')",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO conversation_turns (turn_id, thread_id, ordinal, kind, revision, lifecycle, created_at_ms, updated_at_ms) VALUES ('turn1', 't1', 0, 'turn', 0, 'pending', 7, 7)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m2', 't1', 1, 'world', 8)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO conversation_ordinals (thread_id, ordinal, kind, entity_id) VALUES ('t1', 1, 'turn', 'turn2')",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO conversation_turns (turn_id, thread_id, ordinal, kind, revision, lifecycle, created_at_ms, updated_at_ms) VALUES ('turn2', 't1', 1, 'turn', 0, 'pending', 8, 8)",
        )
        .await?;

    database
        .execute_unprepared(
            "INSERT INTO assistant_runs (run_id, thread_id, run_start_key, origin_message_id, origin_turn_id, lifecycle, generation, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('run-valid', 't1', zeroblob(32), 'm1', 'turn1', 'queued', 0, 10, 10, 1, 1, X'00')",
        )
        .await?;
    let invalid_snapshot = database
        .execute_unprepared(
            "INSERT INTO assistant_runs (run_id, thread_id, run_start_key, origin_message_id, origin_turn_id, lifecycle, generation, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('run-invalid', 't1', zeroblob(31) || X'01', 'm2', 'turn2', 'queued', 0, 11, 11, 1, 1, NULL)",
        )
        .await;
    assert!(invalid_snapshot.is_err());
    let immutable_snapshot_update = database
        .execute_unprepared(
            "UPDATE assistant_runs SET engine_run_config = X'01' WHERE run_id = 'run-valid'",
        )
        .await;
    assert!(immutable_snapshot_update.is_err());
    database.close().await?;
    Ok(())
}

async fn seed_v2_guard_scope(
    database: &sea_orm_migration::sea_orm::DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
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
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m1', 't1', 0, 'hello', 7)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO conversation_ordinals (thread_id, ordinal, kind, entity_id) VALUES ('t1', 0, 'turn', 'turn1')",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO conversation_turns (turn_id, thread_id, ordinal, kind, revision, lifecycle, created_at_ms, updated_at_ms) VALUES ('turn1', 't1', 0, 'turn', 0, 'pending', 7, 7)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m2', 't1', 1, 'second', 8)",
        )
        .await?;
    Ok(())
}

async fn assert_v2_shape_guards(
    database: &sea_orm_migration::sea_orm::DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    // Codec version 2 rows persist on every guarded table.
    database
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('t-v2', 'p1', 'V2 thread', 4, 4, 2, 1, X'00')",
        )
        .await?;
    assert_eq!(
        scalar_i64(
            database,
            "SELECT engine_run_config_version FROM threads WHERE thread_id = 't-v2'",
        )
        .await?,
        2
    );
    database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_result_revision) VALUES ('engine-v2', 'set_thread_engine_config', 't-v2', 5, 2, X'00', 1)",
        )
        .await?;
    // The lane-000005 queue_message arm survives the receipts rebuild with
    // both body states.
    database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, message_id, body, accepted_at_ms) VALUES ('queue-msg-1', 'queue_message', 't-v2', 'm1', 'hello', 6)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, message_id, accepted_at_ms) VALUES ('queue-msg-2', 'queue_message', 't-v2', 'm2', 7)",
        )
        .await?;
    assert_eq!(
        scalar_i64(
            database,
            "SELECT count(*) FROM command_receipts WHERE command_kind = 'queue_message'",
        )
        .await?,
        2
    );
    database
        .execute_unprepared(
            "INSERT INTO assistant_runs (run_id, thread_id, run_start_key, origin_message_id, origin_turn_id, lifecycle, generation, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('run-v2', 't1', zeroblob(32), 'm1', 'turn1', 'queued', 0, 10, 10, 2, 1, X'00')",
        )
        .await?;

    // Version 0 and 3 stay outside every durable guard.
    for version in [0, 3] {
        let thread_insert = database
            .execute_unprepared(&format!(
                "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('t-v{version}', 'p1', 'Out of range', 4, 4, {version}, 1, X'00')"
            ))
            .await;
        assert!(
            thread_insert.is_err(),
            "version {version} thread insert must be rejected"
        );
        let receipt_insert = database
            .execute_unprepared(&format!(
                "INSERT INTO command_receipts (request_id, command_kind, thread_id, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_result_revision) VALUES ('engine-v{version}', 'set_thread_engine_config', 't-v2', 5, {version}, X'00', 1)"
            ))
            .await;
        assert!(
            receipt_insert.is_err(),
            "version {version} receipt insert must be rejected"
        );
        let run_insert = database
            .execute_unprepared(&format!(
                "INSERT INTO assistant_runs (run_id, thread_id, run_start_key, origin_message_id, origin_turn_id, lifecycle, generation, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('run-v{version}', 't1', zeroblob(32), 'm1', 'turn1', 'queued', 0, 10, 10, {version}, 1, X'00')"
            ))
            .await;
        assert!(
            run_insert.is_err(),
            "version {version} run snapshot must be rejected"
        );
    }
    let thread_update = database
        .execute_unprepared(
            "UPDATE threads SET engine_run_config_version = 3 WHERE thread_id = 't-v2'",
        )
        .await;
    assert!(
        thread_update.is_err(),
        "version 3 thread update must be rejected"
    );
    Ok(())
}

#[tokio::test]
async fn engine_config_v2_migration_widens_shape_guards_and_down_restores_them()
-> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&database).await?;
    seed_v2_guard_scope(&database).await?;
    assert_v2_shape_guards(&database).await?;
    database.close().await?;

    // Downgrade restores the version-1-only guards on a v1-shaped database.
    let downgraded = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&downgraded).await?;
    seed_v2_guard_scope(&downgraded).await?;
    // Downgrade restores the version-1-only guards on a v1-shaped database:
    // roll back to exactly the pre-v2 boundary (the first 8 migrations),
    // however many newer migrations exist above it. A fixed step count
    // would silently land on the wrong boundary as migrations are added.
    let applied = scalar_i64(&downgraded, "SELECT count(*) FROM seaql_migrations").await?;
    let steps = u32::try_from(applied - 8)
        .map_err(|_| std::io::Error::other("applied migrations must reach the v2 boundary"))?;
    Migrator::down(&downgraded, Some(steps)).await?;
    let v2_after_down = downgraded
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('t-v2-down', 'p1', 'V2 thread', 4, 4, 2, 1, X'00')",
        )
        .await;
    assert!(
        v2_after_down.is_err(),
        "downgrade must restore the version-1-only thread guard"
    );
    downgraded
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('t-v1-down', 'p1', 'V1 thread', 4, 4, 1, 1, X'00')",
        )
        .await?;
    // The downgrade rebuild keeps the queue_message arm as well.
    downgraded
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, message_id, body, accepted_at_ms) VALUES ('queue-msg-down', 'queue_message', 't-v1-down', 'm1', 'hello', 6)",
        )
        .await?;
    migrate_to_current(&downgraded).await?;
    downgraded
        .execute_unprepared(
            "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms, engine_run_config_version, engine_run_config_revision, engine_run_config) VALUES ('t-v2-again', 'p1', 'V2 thread', 4, 4, 2, 1, X'00')",
        )
        .await?;
    downgraded.close().await?;
    Ok(())
}

fn upgrade_fixture_engine_config() -> EngineRunConfig {
    let one = FiniteMillis::new(1).expect("one millisecond is valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget is valid"),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit is valid"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit is valid"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit is valid"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness line limit is valid"),
        max_header_count: CountLimit::new(8).expect("header count is valid"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer limit is valid"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr limit is valid"),
        observation_capacity: CountLimit::new(16).expect("observation capacity is valid"),
    })
    .expect("runtime relationships are valid");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-upgrade").expect("permission id is valid"),
        EngineAgentId::parse("agent-upgrade").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-upgrade").expect("profile id is valid"),
            EngineModelId::parse("model-upgrade").expect("model id is valid"),
            EngineRouteId::parse("route-upgrade").expect("route id is valid"),
            None,
            permission,
        )),
        runtime,
    )
}

/// A user database captured before the steer-target and snapshot
/// migrations upgrades through them with every row, index, and foreign
/// key intact: the legacy queue receipt replays, its absent target
/// reads NULL, and a configured fresh accept succeeds after upgrade.
#[tokio::test]
async fn queue_steer_and_snapshot_migrations_preserve_legacy_rows() -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    Migrator::up(&database, Some(11)).await?;
    seed_legacy_queue_database(&database).await?;
    migrate_to_current(&database).await?;
    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM seaql_migrations").await?,
        22
    );
    for (table, expected) in [
        ("messages", 1),
        ("message_dispatches", 1),
        ("command_receipts", 1),
    ] {
        assert_eq!(
            scalar_i64(&database, &format!("SELECT count(*) FROM {table}"),).await?,
            expected,
            "{table} rows must survive the upgrade"
        );
    }
    let repository = Repository::new(database.clone());
    let replay = repository
        .lookup_queue_message(
            &RequestId::parse("queue-legacy")?,
            &ThreadId::parse("t1")?,
            &QueueMessagePayload::text_only("legacy bytes")?,
            None,
        )
        .await?
        .ok_or_else(|| std::io::Error::other("legacy receipt should replay"))?;
    assert_eq!(replay.receipt.disposition, ReceiptDisposition::Duplicate);
    assert_eq!(replay.message_id.as_str(), "m1");
    let target: Option<String> = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT steer_run_id FROM message_dispatches WHERE message_id = 'm1'",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("legacy dispatch should exist"))?
        .try_get_by_index(0)?;
    assert_eq!(target, None, "legacy target reads NULL");
    let violations = database
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA foreign_key_check",
        ))
        .await?;
    assert!(
        violations.is_empty(),
        "upgrade must leave no foreign key violations"
    );
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("upgrade-engine")?,
            thread_id: ThreadId::parse("t1")?,
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: upgrade_fixture_engine_config(),
            accepted_at: UnixMillis::from_millis(10),
        })
        .await?;
    let accepted = repository
        .queue_message(QueueMessageInput {
            request_id: RequestId::parse("queue-fresh")?,
            message_id: MessageId::parse("m2")?,
            thread_id: ThreadId::parse("t1")?,
            payload: QueueMessagePayload::text_only("fresh bytes")?,
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(20),
        })
        .await?;
    assert_eq!(accepted.receipt.disposition, ReceiptDisposition::Accepted);
    let snapshot = repository
        .read_receipt_engine_settings(&RequestId::parse("queue-fresh")?)
        .await?
        .ok_or_else(|| std::io::Error::other("fresh accept must capture a snapshot"))?;
    assert_eq!(
        snapshot.config().selection().profile_id().as_str(),
        "profile-upgrade"
    );
    database.close().await?;
    Ok(())
}

async fn seed_legacy_queue_database(
    database: &sea_orm_migration::sea_orm::DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
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
            "INSERT INTO messages (message_id, thread_id, ordinal, body, accepted_at_ms) VALUES ('m1', 't1', 0, 'legacy bytes', 3)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO message_dispatches (message_id, correlation_id, state, attempt_count, queued_at_ms, available_at_ms, updated_at_ms) VALUES ('m1', 'queue-legacy', 'queued', 0, 3, 3, 3)",
        )
        .await?;
    database
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, thread_id, message_id, body, accepted_at_ms) VALUES ('queue-legacy', 'queue_message', 't1', 'm1', 'legacy bytes', 3)",
        )
        .await?;
    Ok(())
}

/// Verifies a migrated database carries the full schema: all nineteen
/// migration records, exactly one copy of each engine-config shape
/// trigger, and the widened version guard live.
async fn assert_migrated_schema(
    database: &sea_orm_migration::sea_orm::DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    assert_eq!(
        scalar_i64(database, "SELECT count(*) FROM seaql_migrations").await?,
        22,
        "migration must record every version exactly once"
    );
    assert_eq!(
        scalar_i64(
            database,
            "SELECT count(*) FROM sqlite_schema WHERE type = 'trigger' AND name IN \
             ('ck_threads_engine_run_config_shape_insert', \
              'ck_threads_engine_run_config_shape_update', \
              'ck_assistant_runs_engine_run_config_shape_insert')",
        )
        .await?,
        3,
        "exactly one copy of each shape trigger must exist"
    );
    let guard_sql: String = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT sql FROM sqlite_schema WHERE type = 'trigger' \
             AND name = 'ck_threads_engine_run_config_shape_insert'",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("shape trigger must exist after migration"))?
        .try_get_by_index(0)?;
    assert!(
        guard_sql.contains("IN (1, 2)"),
        "surviving guard must be the widened version"
    );
    Ok(())
}

/// Eight sequential independently unique fresh files, each on the default
/// pool, migrate cleanly and re-enter idempotently with records and
/// guards intact.
#[tokio::test]
async fn sequential_fresh_files_migrate_and_reenter_idempotently() -> Result<(), Box<dyn Error>> {
    for index in 0..8 {
        let temp = TempDatabase::new(&format!("sequential-migrate-{index}"))?;
        let database = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
        migrate_to_current(&database).await?;
        migrate_to_current(&database).await?;
        assert_migrated_schema(&database).await?;
        database.close().await?;
    }
    Ok(())
}

/// The user-preferences migration seeds the singleton at revision zero,
/// keeps its revision from decreasing, and clears navigation references to
/// deleted projects and threads.
#[tokio::test]
async fn user_preferences_start_empty_and_follow_their_projects() -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::in_memory().sqlx_logging(false)).await?;
    migrate_to_current(&database).await?;
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT count(*) FROM user_preferences WHERE state_id = 1 AND revision = 0 AND default_engine_config IS NULL"
        )
        .await?,
        1
    );
    for statement in [
        "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('p1', 'C:/p1', 'P1', 1)",
        "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t1', 'p1', 'T', 2, 2)",
        "INSERT INTO navigation_projects (project_id, recency, last_thread_id) VALUES ('p1', 1, 't1')",
        "UPDATE user_preferences SET revision = 1, route_project_id = 'p1', route_thread_id = 't1' WHERE state_id = 1",
    ] {
        database.execute_unprepared(statement).await?;
    }
    assert!(
        database
            .execute_unprepared("UPDATE user_preferences SET revision = 0 WHERE state_id = 1")
            .await
            .is_err()
    );
    database
        .execute_unprepared("DELETE FROM threads WHERE thread_id = 't1'")
        .await?;
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT count(*) FROM navigation_projects WHERE last_thread_id IS NULL"
        )
        .await?,
        1
    );
    database
        .execute_unprepared("DELETE FROM attached_projects WHERE project_id = 'p1'")
        .await?;
    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM navigation_projects").await?,
        0
    );
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT count(*) FROM user_preferences WHERE route_project_id IS NULL AND route_thread_id IS NULL"
        )
        .await?,
        1
    );
    database.close().await?;
    Ok(())
}
