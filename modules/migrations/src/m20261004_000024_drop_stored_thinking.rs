//! Removes thinking summaries from the observation ledger.
//!
//! A thinking summary is shown only while it is the newest thing its run
//! produced, so the Forge now keeps the current one in memory and stores
//! none. The rows written before that were replayed on every thread open and
//! were most of the ledger. Each ledger row holds one observation in the
//! canonical envelope, whose first observation field is its tag, so the tag
//! prefix identifies a thinking row exactly; a quote inside any text value is
//! escaped and cannot match it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DELETE FROM observation_ledger \
                 WHERE instr(CAST(observation_bytes AS TEXT), \
                             '\"observations\":[{\"tag\":\"reasoning_summary_') > 0",
            )
            .await
            .map(|_| ())
    }

    /// The summaries were never needed again, and nothing can rebuild them.
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
