//! Records failed messages the Forge moved into a new thread.
//!
//! A recovery creates a thread in the same project, stores the failed
//! message's payload as that thread's composer draft, and inserts one row
//! here. The failed listing excludes recovered messages, and the row keeps
//! the recovery's request identity and destination so a replay answers the
//! same thread.

use sea_orm_migration::prelude::*;

const IDENTIFIER_MAX_BYTES: i64 = 128;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(&format!(
                "CREATE TABLE failed_message_recoveries (\
                    message_id TEXT NOT NULL PRIMARY KEY,\
                    request_id TEXT NOT NULL UNIQUE,\
                    thread_id TEXT NOT NULL,\
                    new_thread_id TEXT NOT NULL,\
                    recovered_at_ms INTEGER NOT NULL,\
                    FOREIGN KEY (message_id) REFERENCES messages(message_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    FOREIGN KEY (thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    FOREIGN KEY (new_thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    CHECK (typeof(request_id) = 'text' AND length(CAST(request_id AS BLOB)) BETWEEN 1 AND {IDENTIFIER_MAX_BYTES}),\
                    CHECK (thread_id <> new_thread_id),\
                    CHECK (typeof(recovered_at_ms) = 'integer')\
                )"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_failed_message_recoveries_thread_id ON failed_message_recoveries(thread_id)",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        for statement in [
            "DROP INDEX IF EXISTS idx_failed_message_recoveries_thread_id",
            "DROP TABLE IF EXISTS failed_message_recoveries",
        ] {
            connection.execute_unprepared(statement).await?;
        }
        Ok(())
    }
}
