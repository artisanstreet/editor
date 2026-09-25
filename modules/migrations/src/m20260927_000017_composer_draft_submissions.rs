//! Makes sending a composer draft and saving one idempotent.
//!
//! `composer_draft_submissions` records each thread draft revision the Forge
//! queued as a message: submitting the same revision again answers that
//! message instead of queueing another. `composer_draft_save_receipts`
//! records the revision each save request was given, so a save request that
//! arrives again (a retransmission after its answer was lost) answers its
//! original revision instead of writing its body over a newer draft.

use sea_orm_migration::prelude::*;

const IDENTIFIER_MAX_BYTES: i64 = 128;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "CREATE TABLE composer_draft_submissions (\
                    thread_id TEXT NOT NULL,\
                    draft_revision INTEGER NOT NULL,\
                    message_id TEXT NOT NULL UNIQUE,\
                    cleared_revision INTEGER NOT NULL,\
                    submitted_at_ms INTEGER NOT NULL,\
                    PRIMARY KEY (thread_id, draft_revision),\
                    FOREIGN KEY (thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    FOREIGN KEY (message_id) REFERENCES messages(message_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    CHECK (typeof(draft_revision) = 'integer' AND draft_revision > 0),\
                    CHECK (typeof(cleared_revision) = 'integer' AND cleared_revision > draft_revision),\
                    CHECK (typeof(submitted_at_ms) = 'integer')\
                )",
            )
            .await?;
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE composer_draft_save_receipts (\
                    request_id TEXT NOT NULL PRIMARY KEY,\
                    scope_kind TEXT NOT NULL,\
                    scope_id TEXT NOT NULL,\
                    revision INTEGER NOT NULL,\
                    saved_at_ms INTEGER NOT NULL,\
                    CHECK (typeof(request_id) = 'text' AND length(CAST(request_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (typeof(revision) = 'integer' AND revision > 0),\
                    CHECK (typeof(saved_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_composer_draft_save_receipts_saved_at ON composer_draft_save_receipts(saved_at_ms)",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        for statement in [
            "DROP INDEX IF EXISTS idx_composer_draft_save_receipts_saved_at",
            "DROP TABLE IF EXISTS composer_draft_save_receipts",
            "DROP TABLE IF EXISTS composer_draft_submissions",
        ] {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }
}
