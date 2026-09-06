//! Allows engine configuration codec version 2 in durable shape guards.
//!
//! Codec version 1 keeps the byte-identical legacy `OpenCode` 2 encoding.
//! Codec version 2 carries the tagged per-engine selections. The base
//! migration pinned every durable guard to version 1, so persisting any
//! non-`OpenCode` 2 configuration aborted; this migration widens the threads
//! shape triggers, the assistant-run snapshot trigger, and the
//! command-receipts `set_thread_engine_config` CHECK arm to `IN (1, 2)`.
//! Applied migrations are never edited. Downgrade restores the
//! version-1-only guards and aborts loudly if version 2 rows are present
//! instead of silently discarding them.

use sea_orm_migration::SchemaManagerConnection;
use sea_orm_migration::prelude::*;

const THREAD_CONFIG_SHAPE_V2: &str = "((NEW.engine_run_config_version IS NULL AND NEW.engine_run_config_revision = 0 AND NEW.engine_run_config IS NULL) OR (typeof(NEW.engine_run_config_version) = 'integer' AND NEW.engine_run_config_version IN (1, 2) AND typeof(NEW.engine_run_config_revision) = 'integer' AND NEW.engine_run_config_revision BETWEEN 1 AND 9223372036854775807 AND typeof(NEW.engine_run_config) = 'blob' AND length(NEW.engine_run_config) BETWEEN 1 AND 65536))";
const CONFIGURED_SNAPSHOT_SHAPE_V2: &str = "(typeof(NEW.engine_run_config_version) = 'integer' AND NEW.engine_run_config_version IN (1, 2) AND typeof(NEW.engine_run_config_revision) = 'integer' AND NEW.engine_run_config_revision BETWEEN 1 AND 9223372036854775807 AND typeof(NEW.engine_run_config) = 'blob' AND length(NEW.engine_run_config) BETWEEN 1 AND 65536)";

const THREAD_CONFIG_SHAPE_V1: &str = "((NEW.engine_run_config_version IS NULL AND NEW.engine_run_config_revision = 0 AND NEW.engine_run_config IS NULL) OR (typeof(NEW.engine_run_config_version) = 'integer' AND NEW.engine_run_config_version = 1 AND typeof(NEW.engine_run_config_revision) = 'integer' AND NEW.engine_run_config_revision BETWEEN 1 AND 9223372036854775807 AND typeof(NEW.engine_run_config) = 'blob' AND length(NEW.engine_run_config) BETWEEN 1 AND 65536))";
const CONFIGURED_SNAPSHOT_SHAPE_V1: &str = "(typeof(NEW.engine_run_config_version) = 'integer' AND NEW.engine_run_config_version = 1 AND typeof(NEW.engine_run_config_revision) = 'integer' AND NEW.engine_run_config_revision BETWEEN 1 AND 9223372036854775807 AND typeof(NEW.engine_run_config) = 'blob' AND length(NEW.engine_run_config) BETWEEN 1 AND 65536)";

/// Widens the durable engine configuration shape guards to codec version 2.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        recreate_shape_triggers(
            connection,
            THREAD_CONFIG_SHAPE_V2,
            CONFIGURED_SNAPSHOT_SHAPE_V2,
        )
        .await?;
        rebuild_command_receipts(connection, "IN (1, 2)").await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        recreate_shape_triggers(
            connection,
            THREAD_CONFIG_SHAPE_V1,
            CONFIGURED_SNAPSHOT_SHAPE_V1,
        )
        .await?;
        rebuild_command_receipts(connection, "= 1").await
    }
}

async fn recreate_shape_triggers(
    connection: &SchemaManagerConnection<'_>,
    thread_shape: &str,
    snapshot_shape: &str,
) -> Result<(), DbErr> {
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
        .execute_unprepared(&format!(
            "CREATE TRIGGER ck_threads_engine_run_config_shape_insert BEFORE INSERT ON threads BEGIN SELECT CASE WHEN NOT {thread_shape} THEN RAISE(ABORT, 'invalid thread engine config shape') END; END"
        ))
        .await?;
    connection
        .execute_unprepared(&format!(
            "CREATE TRIGGER ck_threads_engine_run_config_shape_update BEFORE UPDATE OF engine_run_config_version, engine_run_config_revision, engine_run_config ON threads BEGIN SELECT CASE WHEN NOT {thread_shape} THEN RAISE(ABORT, 'invalid thread engine config shape') END; END"
        ))
        .await?;
    connection
        .execute_unprepared(&format!(
            "CREATE TRIGGER ck_assistant_runs_engine_run_config_shape_insert BEFORE INSERT ON assistant_runs BEGIN SELECT CASE WHEN NOT {snapshot_shape} THEN RAISE(ABORT, 'assistant run requires an engine config snapshot') END; END"
        ))
        .await
        .map(|_| ())
}

async fn rebuild_command_receipts(
    connection: &SchemaManagerConnection<'_>,
    version_predicate: &str,
) -> Result<(), DbErr> {
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
        .execute_unprepared(&format!(
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
                    (command_kind = 'queue_message' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NOT NULL AND (body IS NULL OR (typeof(body) = 'text' AND length(body) <= 65536)) AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR\
                    (command_kind = 'set_thread_engine_config' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NULL AND body IS NULL AND typeof(engine_run_config_version) = 'integer' AND engine_run_config_version {version_predicate} AND typeof(engine_run_config) = 'blob' AND length(engine_run_config) BETWEEN 1 AND 65536 AND (engine_run_config_expected_revision IS NULL OR (typeof(engine_run_config_expected_revision) = 'integer' AND engine_run_config_expected_revision BETWEEN 1 AND 9223372036854775807)) AND typeof(engine_run_config_result_revision) = 'integer' AND engine_run_config_result_revision BETWEEN 1 AND 9223372036854775807)\
                )\
            )"
        ))
        .await?;
    connection
        .execute_unprepared(
            "INSERT INTO command_receipts (request_id, command_kind, directory_id, project_id, thread_id, title, message_id, body, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_expected_revision, engine_run_config_result_revision) SELECT request_id, command_kind, directory_id, project_id, thread_id, title, message_id, body, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_expected_revision, engine_run_config_result_revision FROM command_receipts_old",
        )
        .await?;
    connection
        .execute_unprepared("DROP TABLE command_receipts_old")
        .await?;
    connection
        .execute_unprepared(
            "CREATE UNIQUE INDEX uq_command_receipts_kind_thread_id ON command_receipts(command_kind, thread_id) WHERE command_kind IN ('attach_project', 'create_thread', 'queue_first_message')",
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
        .map(|_| ())
}
