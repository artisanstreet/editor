//! Adds durable thread engine configuration and immutable run snapshots.
//!
//! The migration keeps the legacy rows unconfigured, adds database-level
//! tuple guards, and rebuilds the receipt table so the new command kind does
//! not weaken the existing payload-shape checks.

use sea_orm_migration::SchemaManagerConnection;
use sea_orm_migration::prelude::*;

const THREAD_CONFIG_SHAPE: &str = "((engine_run_config_version IS NULL AND engine_run_config_revision = 0 AND engine_run_config IS NULL) OR (typeof(engine_run_config_version) = 'integer' AND engine_run_config_version = 1 AND typeof(engine_run_config_revision) = 'integer' AND engine_run_config_revision BETWEEN 1 AND 9223372036854775807 AND typeof(engine_run_config) = 'blob' AND length(engine_run_config) BETWEEN 1 AND 65536))";
const CONFIGURED_SNAPSHOT_SHAPE: &str = "(typeof(engine_run_config_version) = 'integer' AND engine_run_config_version = 1 AND typeof(engine_run_config_revision) = 'integer' AND engine_run_config_revision BETWEEN 1 AND 9223372036854775807 AND typeof(engine_run_config) = 'blob' AND length(engine_run_config) BETWEEN 1 AND 65536)";

/// Adds engine configuration columns, snapshot fences, and the set-command
/// receipt shape.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "ALTER TABLE threads ADD COLUMN engine_run_config_version INTEGER NULL",
            )
            .await?;
        connection
            .execute_unprepared(
                "ALTER TABLE threads ADD COLUMN engine_run_config_revision INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        connection
            .execute_unprepared("ALTER TABLE threads ADD COLUMN engine_run_config BLOB NULL")
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TRIGGER ck_threads_engine_run_config_shape_insert BEFORE INSERT ON threads BEGIN SELECT CASE WHEN NOT {THREAD_CONFIG_SHAPE} THEN RAISE(ABORT, 'invalid thread engine config shape') END; END"
            ))
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TRIGGER ck_threads_engine_run_config_shape_update BEFORE UPDATE OF engine_run_config_version, engine_run_config_revision, engine_run_config ON threads BEGIN SELECT CASE WHEN NOT {THREAD_CONFIG_SHAPE} THEN RAISE(ABORT, 'invalid thread engine config shape') END; END"
            ))
            .await?;

        connection
            .execute_unprepared(
                "ALTER TABLE assistant_runs ADD COLUMN engine_run_config_version INTEGER NULL",
            )
            .await?;
        connection
            .execute_unprepared(
                "ALTER TABLE assistant_runs ADD COLUMN engine_run_config_revision INTEGER NULL",
            )
            .await?;
        connection
            .execute_unprepared("ALTER TABLE assistant_runs ADD COLUMN engine_run_config BLOB NULL")
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TRIGGER ck_assistant_runs_engine_run_config_shape_insert BEFORE INSERT ON assistant_runs BEGIN SELECT CASE WHEN NOT {CONFIGURED_SNAPSHOT_SHAPE} THEN RAISE(ABORT, 'assistant run requires an engine config snapshot') END; END"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE TRIGGER ck_assistant_runs_engine_run_config_immutable BEFORE UPDATE OF engine_run_config_version, engine_run_config_revision, engine_run_config ON assistant_runs WHEN NOT (OLD.engine_run_config_version IS NEW.engine_run_config_version AND OLD.engine_run_config_revision IS NEW.engine_run_config_revision AND OLD.engine_run_config IS NEW.engine_run_config) BEGIN SELECT RAISE(ABORT, 'assistant run engine config snapshot is immutable'); END",
            )
            .await?;

        rebuild_command_receipts(connection).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("DROP TRIGGER IF EXISTS ck_threads_engine_run_config_shape_insert")
            .await?;
        connection
            .execute_unprepared("DROP TRIGGER IF EXISTS ck_threads_engine_run_config_shape_update")
            .await?;
        connection
            .execute_unprepared(
                "DROP TRIGGER IF EXISTS ck_assistant_runs_engine_run_config_shape_insert",
            )
            .await?;
        connection
            .execute_unprepared(
                "DROP TRIGGER IF EXISTS ck_assistant_runs_engine_run_config_immutable",
            )
            .await?;
        connection
            .execute_unprepared("ALTER TABLE assistant_runs DROP COLUMN engine_run_config")
            .await?;
        connection
            .execute_unprepared("ALTER TABLE assistant_runs DROP COLUMN engine_run_config_revision")
            .await?;
        connection
            .execute_unprepared("ALTER TABLE assistant_runs DROP COLUMN engine_run_config_version")
            .await?;
        connection
            .execute_unprepared("ALTER TABLE threads DROP COLUMN engine_run_config")
            .await?;
        connection
            .execute_unprepared("ALTER TABLE threads DROP COLUMN engine_run_config_revision")
            .await?;
        connection
            .execute_unprepared("ALTER TABLE threads DROP COLUMN engine_run_config_version")
            .await?;
        restore_command_receipts(connection).await
    }
}

async fn rebuild_command_receipts(connection: &SchemaManagerConnection<'_>) -> Result<(), DbErr> {
    connection
        .execute_unprepared("DROP INDEX IF EXISTS uq_command_receipts_kind_thread_id")
        .await?;
    connection
        .execute_unprepared("DROP INDEX IF EXISTS uq_command_receipts_message_id")
        .await?;
    connection
        .execute_unprepared("DROP INDEX IF EXISTS idx_command_receipts_kind_project_id")
        .await?;
    connection
        .execute_unprepared("ALTER TABLE command_receipts RENAME TO command_receipts_old")
        .await?;
    connection
        .execute_unprepared(
            "CREATE TABLE command_receipts (\
                request_id TEXT NOT NULL PRIMARY KEY,\
                command_kind TEXT NOT NULL,\
                directory_id TEXT NULL,\
                project_id TEXT NULL,\
                thread_id TEXT NULL,\
                title TEXT NULL,\
                message_id TEXT NULL,\
                body TEXT NULL,\
                accepted_at_ms INTEGER NOT NULL,\
                engine_run_config_version INTEGER NULL,\
                engine_run_config BLOB NULL,\
                engine_run_config_expected_revision INTEGER NULL,\
                engine_run_config_result_revision INTEGER NULL,\
                FOREIGN KEY(project_id) REFERENCES attached_projects(project_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                FOREIGN KEY(message_id) REFERENCES messages(message_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                CHECK (\
                    (command_kind = 'attach_project' AND directory_id IS NOT NULL AND project_id IS NOT NULL AND thread_id IS NULL AND title IS NULL AND message_id IS NULL AND body IS NULL AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR\
                    (command_kind = 'create_thread' AND directory_id IS NULL AND project_id IS NOT NULL AND thread_id IS NOT NULL AND title IS NOT NULL AND message_id IS NULL AND body IS NULL AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR\
                    (command_kind = 'queue_first_message' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NOT NULL AND body IS NOT NULL AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR\
                    (command_kind = 'set_thread_engine_config' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NULL AND body IS NULL AND typeof(engine_run_config_version) = 'integer' AND engine_run_config_version = 1 AND typeof(engine_run_config) = 'blob' AND length(engine_run_config) BETWEEN 1 AND 65536 AND (engine_run_config_expected_revision IS NULL OR (typeof(engine_run_config_expected_revision) = 'integer' AND engine_run_config_expected_revision BETWEEN 1 AND 9223372036854775807)) AND typeof(engine_run_config_result_revision) = 'integer' AND engine_run_config_result_revision BETWEEN 1 AND 9223372036854775807)\
                )\
            )",
        )
        .await?;
    connection
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, directory_id, project_id, thread_id, title, message_id, body, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_expected_revision, engine_run_config_result_revision) SELECT request_id, command_kind, directory_id, project_id, thread_id, title, message_id, body, accepted_at_ms, NULL, NULL, NULL, NULL FROM command_receipts_old",
        )
        .await?;
    connection
        .execute_unprepared("DROP TABLE command_receipts_old")
        .await?;
    create_command_receipt_indexes(connection).await
}

async fn restore_command_receipts(connection: &SchemaManagerConnection<'_>) -> Result<(), DbErr> {
    connection
        .execute_unprepared("DROP INDEX IF EXISTS uq_command_receipts_kind_thread_id")
        .await?;
    connection
        .execute_unprepared("DROP INDEX IF EXISTS uq_command_receipts_message_id")
        .await?;
    connection
        .execute_unprepared("DROP INDEX IF EXISTS idx_command_receipts_kind_project_id")
        .await?;
    connection
        .execute_unprepared("ALTER TABLE command_receipts RENAME TO command_receipts_new")
        .await?;
    connection
        .execute_unprepared(
            "CREATE TABLE command_receipts (\
                request_id TEXT NOT NULL PRIMARY KEY,\
                command_kind TEXT NOT NULL,\
                directory_id TEXT NULL,\
                project_id TEXT NULL,\
                thread_id TEXT NULL,\
                title TEXT NULL,\
                message_id TEXT NULL,\
                body TEXT NULL,\
                accepted_at_ms INTEGER NOT NULL,\
                FOREIGN KEY(project_id) REFERENCES attached_projects(project_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                FOREIGN KEY(message_id) REFERENCES messages(message_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                CHECK (\
                    (command_kind = 'attach_project' AND directory_id IS NOT NULL AND project_id IS NOT NULL AND thread_id IS NULL AND title IS NULL AND message_id IS NULL AND body IS NULL) OR\
                    (command_kind = 'create_thread' AND directory_id IS NULL AND project_id IS NOT NULL AND thread_id IS NOT NULL AND title IS NOT NULL AND message_id IS NULL AND body IS NULL) OR\
                    (command_kind = 'queue_first_message' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NOT NULL AND body IS NOT NULL)\
                )\
            )",
        )
        .await?;
    connection
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, directory_id, project_id, thread_id, title, message_id, body, accepted_at_ms) SELECT request_id, command_kind, directory_id, project_id, thread_id, title, message_id, body, accepted_at_ms FROM command_receipts_new WHERE command_kind <> 'set_thread_engine_config'",
        )
        .await?;
    connection
        .execute_unprepared("DROP TABLE command_receipts_new")
        .await?;
    create_legacy_command_receipt_indexes(connection).await
}

async fn create_command_receipt_indexes(
    connection: &SchemaManagerConnection<'_>,
) -> Result<(), DbErr> {
    connection
        .execute_unprepared(
            "CREATE UNIQUE INDEX uq_command_receipts_kind_thread_id ON command_receipts(command_kind, thread_id) WHERE command_kind <> 'set_thread_engine_config'",
        )
        .await?;
    connection
        .execute_unprepared(
            "CREATE UNIQUE INDEX uq_command_receipts_message_id ON command_receipts(message_id)",
        )
        .await?;
    connection
        .execute_unprepared(
            "CREATE INDEX idx_command_receipts_kind_project_id ON command_receipts(command_kind, project_id)",
        )
        .await
}

async fn create_legacy_command_receipt_indexes(
    connection: &SchemaManagerConnection<'_>,
) -> Result<(), DbErr> {
    connection
        .execute_unprepared(
            "CREATE UNIQUE INDEX uq_command_receipts_kind_thread_id ON command_receipts(command_kind, thread_id)",
        )
        .await?;
    connection
        .execute_unprepared(
            "CREATE UNIQUE INDEX uq_command_receipts_message_id ON command_receipts(message_id)",
        )
        .await?;
    connection
        .execute_unprepared(
            "CREATE INDEX idx_command_receipts_kind_project_id ON command_receipts(command_kind, project_id)",
        )
        .await
}
