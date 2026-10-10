//! Adds where each thread lives: in view, settled into History, or archived.
//!
//! One optional row per thread. `history_at_ms` is when the reader settled
//! the thread, or the Forge filed it after a quiet day; activity after that
//! instant brings it back into view. `archived_at_ms` takes the thread out
//! of every listing until it is restored or erased. `titled_by_reader`
//! keeps a rename from being replaced by a generated title. A thread without
//! a row is in view, archived never, and named by the Forge.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "CREATE TABLE thread_lifecycle (\
                    thread_id TEXT NOT NULL PRIMARY KEY,\
                    history_at_ms INTEGER NULL,\
                    archived_at_ms INTEGER NULL,\
                    titled_by_reader INTEGER NOT NULL DEFAULT 0,\
                    FOREIGN KEY (thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE CASCADE,\
                    CHECK (history_at_ms IS NULL OR typeof(history_at_ms) = 'integer'),\
                    CHECK (archived_at_ms IS NULL OR typeof(archived_at_ms) = 'integer'),\
                    CHECK (titled_by_reader IN (0, 1))\
                )",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_thread_lifecycle_archived_at \
                 ON thread_lifecycle(archived_at_ms) WHERE archived_at_ms IS NOT NULL",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS thread_lifecycle")
            .await
            .map(|_| ())
    }
}
