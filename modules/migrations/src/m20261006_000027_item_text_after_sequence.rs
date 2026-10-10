//! Records where each assistant reply's first text falls among a thread's
//! observations.
//!
//! A turn's work interleaves prose and tool steps, but they are stored
//! apart: prose as conversation items, steps in the observation ledger. The
//! Forge opens a reply's item when the turn starts, long before its first
//! text, so the item's ordinal says nothing about when its words arrived.
//! `text_after_sequence` is the thread's highest ledger delivery sequence at
//! the moment the item's first text was committed: the reply follows every
//! step at or below it and precedes every later one.
//!
//! Existing replies are backfilled from history: the first patch that gave
//! the item text, against the ledger rows committed strictly before it. A
//! step committed in the same batch as the text arrived after that text, so
//! it is not counted. Items that never received text stay `NULL`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "ALTER TABLE conversation_items ADD COLUMN text_after_sequence INTEGER NULL",
            )
            .await?;
        // One grouped pass over the patches instead of a scan per item:
        // `conversation_patches` has no index on `item_id`.
        connection
            .execute_unprepared(
                "CREATE TEMP TABLE item_first_text AS \
                 SELECT item_id, MIN(recorded_at_ms) AS first_text_ms \
                 FROM conversation_patches \
                 WHERE (kind = 'item_upsert' AND length(coalesce(body, '')) > 0) \
                    OR (kind = 'item_append' AND length(coalesce(fragment, '')) > 0) \
                 GROUP BY item_id",
            )
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX temp.item_first_text_item ON item_first_text(item_id)",
            )
            .await?;
        connection
            .execute_unprepared(
                "UPDATE conversation_items \
                 SET text_after_sequence = ( \
                     SELECT coalesce(max(ledger.delivery_sequence), 0) \
                     FROM observation_ledger AS ledger \
                     WHERE ledger.thread_id = conversation_items.thread_id \
                       AND ledger.committed_at_ms < ( \
                           SELECT first.first_text_ms FROM item_first_text AS first \
                           WHERE first.item_id = conversation_items.item_id)) \
                 WHERE item_kind = 'assistant_message' \
                   AND item_id IN (SELECT item_id FROM item_first_text)",
            )
            .await?;
        connection
            .execute_unprepared("DROP TABLE item_first_text")
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE conversation_items DROP COLUMN text_after_sequence")
            .await
            .map(|_| ())
    }
}
