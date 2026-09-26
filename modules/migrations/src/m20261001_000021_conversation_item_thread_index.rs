//! Indexes conversation items by thread and kind.
//!
//! Thread listings ask, per thread, whether assistant text has started; the
//! recent-threads listing and its change fingerprint ask it for every saved
//! thread whenever a commit wakes a connection. Without this index each
//! question scans the whole transcript table.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX idx_conversation_items_thread_id_item_kind \
                 ON conversation_items(thread_id, item_kind)",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_conversation_items_thread_id_item_kind")
            .await
            .map(|_| ())
    }
}
