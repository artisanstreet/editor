//! Records each ledger row's observation kind beside its payload.
//!
//! The work rows of a settled turn (its tool calls and commands) now stay on
//! the Forge until the turn's section is opened, so a history read has to
//! tell a work row from the rest without decoding every payload. The kind
//! is the payload's own tag: the canonical envelope holds one observation
//! whose first field is that tag, so it is copied out with plain string
//! functions. The index serves the two reads the split needs: one turn's
//! rows in delivery order, and a thread's rows by kind.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The text that precedes the tag in every canonical ledger payload.
const TAG_PREFIX: &str = r#""observations":[{"tag":""#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "ALTER TABLE observation_ledger \
                 ADD COLUMN observation_tag TEXT NOT NULL DEFAULT ''",
            )
            .await?;
        // `body` is the payload as text and `start` the first character of
        // its tag; the tag runs to the next quote.
        let body = "CAST(observation_bytes AS TEXT)";
        let start = format!("instr({body}, '{TAG_PREFIX}') + {}", TAG_PREFIX.len());
        connection
            .execute_unprepared(&format!(
                "UPDATE observation_ledger \
                 SET observation_tag = substr({body}, {start}, \
                                              instr(substr({body}, {start}), '\"') - 1) \
                 WHERE instr({body}, '{TAG_PREFIX}') > 0"
            ))
            .await?;
        connection
            .execute_unprepared(
                "CREATE INDEX idx_observation_ledger_turn_tag \
                 ON observation_ledger (thread_id, turn_id, observation_tag, delivery_sequence)",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("DROP INDEX IF EXISTS idx_observation_ledger_turn_tag")
            .await?;
        connection
            .execute_unprepared("ALTER TABLE observation_ledger DROP COLUMN observation_tag")
            .await
            .map(|_| ())
    }
}
