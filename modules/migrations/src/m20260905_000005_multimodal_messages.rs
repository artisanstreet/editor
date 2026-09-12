//! Adds ordered image bytes and the general queue-message receipt shape.
//!
//! Existing first-message receipts keep their unique `(kind, thread)` fence.
//! General queue-message receipts deliberately do not use that index: a
//! thread may accept any number of subsequent messages while each request id
//! remains globally idempotent.

use sea_orm_migration::prelude::*;
use sea_orm_migration::SchemaManagerConnection;

const RECEIPT_COLUMNS: &str = "request_id, command_kind, directory_id, project_id, thread_id, title, message_id, body, accepted_at_ms, engine_run_config_version, engine_run_config, engine_run_config_expected_revision, engine_run_config_result_revision";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                r"CREATE TABLE message_image_attachments (
                    message_id TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    mime_type TEXT NOT NULL,
                    name TEXT NOT NULL,
                    size_bytes INTEGER NOT NULL,
                    bytes BLOB NOT NULL,
                    PRIMARY KEY (message_id, position),
                    FOREIGN KEY(message_id) REFERENCES messages(message_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
                    CHECK (position >= 0),
                    CHECK (length(mime_type) BETWEEN 1 AND 64),
                    CHECK (length(name) BETWEEN 1 AND 256),
                    CHECK (size_bytes BETWEEN 1 AND 5242880),
                    CHECK (typeof(bytes) = 'blob' AND length(bytes) = size_bytes)
                )",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_message_image_attachments_message_position ON message_image_attachments(message_id, position)",
            )
            .await?;
        rebuild_command_receipts(connection).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("DROP INDEX IF EXISTS idx_message_image_attachments_message_position")
            .await?;
        connection
            .execute_unprepared("DROP TABLE message_image_attachments")
            .await?;
        restore_command_receipts(connection).await
    }
}

async fn rebuild_command_receipts(connection: &SchemaManagerConnection<'_>) -> Result<(), DbErr> {
    drop_receipt_indexes(connection).await?;
    connection
        .execute_unprepared("ALTER TABLE command_receipts RENAME TO command_receipts_old")
        .await?;
    connection
        .execute_unprepared(
            r"CREATE TABLE command_receipts (
                request_id TEXT NOT NULL PRIMARY KEY,
                command_kind TEXT NOT NULL,
                directory_id TEXT NULL,
                project_id TEXT NULL,
                thread_id TEXT NULL,
                title TEXT NULL,
                message_id TEXT NULL,
                body TEXT NULL,
                accepted_at_ms INTEGER NOT NULL,
                engine_run_config_version INTEGER NULL,
                engine_run_config BLOB NULL,
                engine_run_config_expected_revision INTEGER NULL,
                engine_run_config_result_revision INTEGER NULL,
                FOREIGN KEY(project_id) REFERENCES attached_projects(project_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
                FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
                FOREIGN KEY(message_id) REFERENCES messages(message_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
                CHECK (
                    (command_kind = 'attach_project' AND directory_id IS NOT NULL AND project_id IS NOT NULL AND thread_id IS NULL AND title IS NULL AND message_id IS NULL AND body IS NULL AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR
                    (command_kind = 'create_thread' AND directory_id IS NULL AND project_id IS NOT NULL AND thread_id IS NOT NULL AND title IS NOT NULL AND message_id IS NULL AND body IS NULL AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR
                    (command_kind = 'queue_first_message' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NOT NULL AND body IS NOT NULL AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR
                    (command_kind = 'queue_message' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NOT NULL AND (body IS NULL OR (typeof(body) = 'text' AND length(body) <= 65536)) AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR
                    (command_kind = 'set_thread_engine_config' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NULL AND body IS NULL AND typeof(engine_run_config_version) = 'integer' AND engine_run_config_version = 1 AND typeof(engine_run_config) = 'blob' AND length(engine_run_config) BETWEEN 1 AND 65536 AND (engine_run_config_expected_revision IS NULL OR (typeof(engine_run_config_expected_revision) = 'integer' AND engine_run_config_expected_revision BETWEEN 1 AND 9223372036854775807)) AND typeof(engine_run_config_result_revision) = 'integer' AND engine_run_config_result_revision BETWEEN 1 AND 9223372036854775807)
                )
            )",
        )
        .await?;
    connection
        .execute_unprepared(&format!(
            "INSERT INTO command_receipts ({RECEIPT_COLUMNS}) SELECT {RECEIPT_COLUMNS} FROM command_receipts_old"
        ))
        .await?;
    connection
        .execute_unprepared("DROP TABLE command_receipts_old")
        .await?;
    create_receipt_indexes(connection).await
}

async fn restore_command_receipts(connection: &SchemaManagerConnection<'_>) -> Result<(), DbErr> {
    drop_receipt_indexes(connection).await?;
    connection
        .execute_unprepared("ALTER TABLE command_receipts RENAME TO command_receipts_new")
        .await?;
    connection
        .execute_unprepared(
            r"CREATE TABLE command_receipts (
                request_id TEXT NOT NULL PRIMARY KEY,
                command_kind TEXT NOT NULL,
                directory_id TEXT NULL,
                project_id TEXT NULL,
                thread_id TEXT NULL,
                title TEXT NULL,
                message_id TEXT NULL,
                body TEXT NULL,
                accepted_at_ms INTEGER NOT NULL,
                engine_run_config_version INTEGER NULL,
                engine_run_config BLOB NULL,
                engine_run_config_expected_revision INTEGER NULL,
                engine_run_config_result_revision INTEGER NULL,
                FOREIGN KEY(project_id) REFERENCES attached_projects(project_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
                FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
                FOREIGN KEY(message_id) REFERENCES messages(message_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
                CHECK (
                    (command_kind = 'attach_project' AND directory_id IS NOT NULL AND project_id IS NOT NULL AND thread_id IS NULL AND title IS NULL AND message_id IS NULL AND body IS NULL AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR
                    (command_kind = 'create_thread' AND directory_id IS NULL AND project_id IS NOT NULL AND thread_id IS NOT NULL AND title IS NOT NULL AND message_id IS NULL AND body IS NULL AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR
                    (command_kind = 'queue_first_message' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NOT NULL AND body IS NOT NULL AND engine_run_config_version IS NULL AND engine_run_config IS NULL AND engine_run_config_expected_revision IS NULL AND engine_run_config_result_revision IS NULL) OR
                    (command_kind = 'set_thread_engine_config' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NULL AND body IS NULL AND typeof(engine_run_config_version) = 'integer' AND engine_run_config_version = 1 AND typeof(engine_run_config) = 'blob' AND length(engine_run_config) BETWEEN 1 AND 65536 AND (engine_run_config_expected_revision IS NULL OR (typeof(engine_run_config_expected_revision) = 'integer' AND engine_run_config_expected_revision BETWEEN 1 AND 9223372036854775807)) AND typeof(engine_run_config_result_revision) = 'integer' AND engine_run_config_result_revision BETWEEN 1 AND 9223372036854775807)
                )
            )",
        )
        .await?;
    connection
        .execute_unprepared(&format!(
            "INSERT INTO command_receipts ({RECEIPT_COLUMNS}) SELECT {RECEIPT_COLUMNS} FROM command_receipts_new WHERE command_kind <> 'queue_message'"
        ))
        .await?;
    connection
        .execute_unprepared("DROP TABLE command_receipts_new")
        .await?;
    create_receipt_indexes(connection).await
}

async fn drop_receipt_indexes(connection: &SchemaManagerConnection<'_>) -> Result<(), DbErr> {
    for index in [
        "uq_command_receipts_kind_thread_id",
        "uq_command_receipts_message_id",
        "idx_command_receipts_kind_project_id",
    ] {
        connection
            .execute_unprepared(&format!("DROP INDEX IF EXISTS {index}"))
            .await?;
    }
    Ok(())
}

async fn create_receipt_indexes(connection: &SchemaManagerConnection<'_>) -> Result<(), DbErr> {
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
        .await?;
    Ok(())
}
