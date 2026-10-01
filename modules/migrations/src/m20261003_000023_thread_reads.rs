//! Adds when the reader last had each thread open.
//!
//! The thread lists mark a thread whose latest run settled after the reader
//! last had it open. One row per thread holds that instant; the navigation
//! record advances it. Threads that exist when this migration runs count as
//! read as of now, so an existing history does not light up at once; a
//! thread without a row has never been open.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "CREATE TABLE thread_reads (\
                    thread_id TEXT NOT NULL PRIMARY KEY,\
                    read_at_ms INTEGER NOT NULL,\
                    FOREIGN KEY (thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE CASCADE,\
                    CHECK (typeof(read_at_ms) = 'integer')\
                )",
            )
            .await?;
        connection
            .execute_unprepared(
                "INSERT INTO thread_reads (thread_id, read_at_ms) \
                 SELECT thread_id, CAST(strftime('%s', 'now') AS INTEGER) * 1000 FROM threads",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS thread_reads")
            .await
            .map(|_| ())
    }
}
