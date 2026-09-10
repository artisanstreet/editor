//! Allows queue-message receipts to carry their accepted engine snapshot.
//!
//! `queue_message` admission captures the authoritative thread engine
//! settings into the receipt snapshot columns so dispatch can never run
//! a queued send on a later-selected engine. The checked-in
//! `command_receipts` CHECK arm for `queue_message` still requires all
//! four snapshot columns to be NULL, which aborts every configured
//! accept with a CHECK failure.
//!
//! This migration rebuilds `command_receipts` following the exact
//! `m20260906_000009_engine_run_config_v2` pattern (rename, recreate,
//! copy, drop, recreate the three indexes): identical columns in
//! identical order, identical foreign keys, identical arms for every
//! other command kind. Only the `queue_message` arm widens, from
//! legacy-null-only to legacy-null OR one complete bounded snapshot:
//! integer codec version `IN (1, 2)`, blob `1..65536` bytes, no expected
//! revision (the queue path never sets one), and integer result
//! revision `1..i64::MAX` — the same bounds the `set_thread_engine_config`
//! arm already enforces. Downgrade restores the legacy arm and aborts
//! loudly if snapshot-carrying queue rows are present instead of
//! silently discarding them. Historical migrations are never edited.

use sea_orm_migration::prelude::*;

const QUEUE_ARM_SNAPSHOT: &str = "(command_kind = 'queue_message' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NOT NULL AND (body IS NULL OR (typeof(body) = 'text' AND length(body) <= 65536)) AND ((engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR (typeof(engine_run_config_version) = 'integer' AND engine_run_config_version IN (1, 2) AND typeof(engine_run_config) = 'blob' AND length(engine_run_config) BETWEEN 1 AND 65536 AND engine_run_config_expected_revision IS NULL AND typeof(engine_run_config_result_revision) = 'integer' AND engine_run_config_result_revision BETWEEN 1 AND 9223372036854775807)))";

const QUEUE_ARM_LEGACY: &str = "(command_kind = 'queue_message' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NOT NULL AND (body IS NULL OR (typeof(body) = 'text' AND length(body) <= 65536)) AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL)";

/// Widens the `queue_message` receipt arm to the accepted snapshot shape.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rebuild_command_receipts(manager.get_connection(), QUEUE_ARM_SNAPSHOT).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rebuild_command_receipts(manager.get_connection(), QUEUE_ARM_LEGACY).await
    }
}

async fn rebuild_command_receipts(
    connection: &SchemaManagerConnection<'_>,
    queue_arm: &str,
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
                    {queue_arm} OR\
                    (command_kind = 'set_thread_engine_config' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NULL AND body IS NULL AND typeof(engine_run_config_version) = 'integer' AND engine_run_config_version IN (1, 2) AND typeof(engine_run_config) = 'blob' AND length(engine_run_config) BETWEEN 1 AND 65536 AND (engine_run_config_expected_revision IS NULL OR (typeof(engine_run_config_expected_revision) = 'integer' AND engine_run_config_expected_revision BETWEEN 1 AND 9223372036854775807)) AND typeof(engine_run_config_result_revision) = 'integer' AND engine_run_config_result_revision BETWEEN 1 AND 9223372036854775807)\
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
