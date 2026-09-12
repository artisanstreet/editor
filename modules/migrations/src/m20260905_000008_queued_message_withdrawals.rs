//! Adds durable, payload-free receipts for queued-message withdrawal.
//!
//! The original `messages`, `message_image_attachments`,
//! `command_receipts`, and `message_dispatches` rows remain authoritative and
//! immutable except for the claim-compatible dispatch state fence performed by
//! the database repository. This table records every valid withdrawal command
//! outcome, including `too_late` and `not_queued`, so an exact retry can
//! reproduce the original result without changing the shared command-receipt
//! schema.

use sea_orm_migration::prelude::*;

const IDENTIFIER_MAX_BYTES: i64 = 128;

/// Creates the bounded queued-message withdrawal receipt table.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE queued_message_withdrawals (\
                    withdrawal_request_id TEXT NOT NULL PRIMARY KEY,\
                    thread_id TEXT NOT NULL,\
                    message_id TEXT NOT NULL,\
                    original_request_id TEXT NOT NULL,\
                    outcome TEXT NOT NULL,\
                    accepted_at_ms INTEGER NOT NULL,\
                    FOREIGN KEY(thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    CHECK (typeof(withdrawal_request_id) = 'text' AND length(CAST(withdrawal_request_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(thread_id) = 'text' AND length(CAST(thread_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(message_id) = 'text' AND length(CAST(message_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(original_request_id) = 'text' AND length(CAST(original_request_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(outcome) = 'text' AND outcome IN ('withdrawn', 'too_late', 'not_queued')),\
                    CHECK (typeof(accepted_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_queued_message_withdrawals_target ON queued_message_withdrawals(thread_id, message_id, original_request_id)",
            )
            .await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_queued_message_withdrawals_target")
            .await?;
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS queued_message_withdrawals")
            .await
            .map(|_| ())
    }
}
