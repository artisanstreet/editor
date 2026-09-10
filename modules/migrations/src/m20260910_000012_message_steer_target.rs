//! Durable steer-target hint for queued message dispatches.
//!
//! A named same-engine live send carries the observed run it must steer
//! into. The hint is dispatch-scoped delivery state on the existing
//! `message_dispatches` row — never message content — so it lives beside
//! the lease columns, not beside the immutable payload. `NULL` is a fresh
//! send in every state; existing rows read as unnamed without backfill.
//!
//! The hint names intent only: dispatch revalidates liveness and the
//! same-engine rule before delivery, and fails typed otherwise. No fresh
//! run is ever started from a named hint implicitly.

use sea_orm_migration::prelude::*;

/// Records the observed live run a queued message must steer into.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared(
                "ALTER TABLE message_dispatches \
                    ADD COLUMN steer_run_id TEXT NULL \
                    CHECK (steer_run_id IS NULL OR (typeof(steer_run_id) = 'text' AND length(CAST(steer_run_id AS BLOB)) BETWEEN 1 AND 128))",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let connection = manager.get_connection();
        connection
            .execute_unprepared("ALTER TABLE message_dispatches DROP COLUMN steer_run_id")
            .await
            .map(|_| ())
    }
}
