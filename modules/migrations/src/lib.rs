//! Immutable, ordered migrations for the native Artisan database.
//!
//! This schema belongs to the Rust application. Importing a legacy
//! TypeScript-era database is an explicit future boundary and is never an
//! implicit startup migration.

mod m20260824_000001_initial_native_schema;
mod m20260824_000002_global_command_receipts;
mod m20260824_000003_conversation_execution;
mod m20260830_000004_engine_run_config;
mod m20260905_000005_multimodal_messages;
mod m20260905_000006_model_favorites;

mod m20260905_000007_run_usage;

mod m20260905_000008_queued_message_withdrawals;

mod m20260906_000009_engine_run_config_v2;

mod m20260908_000010_run_interactions;

mod m20260909_000011_observation_ledger;

mod m20260910_000012_message_steer_target;
mod m20260910_000013_queue_message_config_snapshot;

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{DatabaseConnection, TransactionTrait};
use thiserror::Error;

/// Ordered migration set for the native database.
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260824_000001_initial_native_schema::Migration),
            Box::new(m20260824_000002_global_command_receipts::Migration),
            Box::new(m20260824_000003_conversation_execution::Migration),
            Box::new(m20260830_000004_engine_run_config::Migration),
            Box::new(m20260905_000005_multimodal_messages::Migration),
            Box::new(m20260905_000006_model_favorites::Migration),
            Box::new(m20260905_000007_run_usage::Migration),
            Box::new(m20260905_000008_queued_message_withdrawals::Migration),
            Box::new(m20260906_000009_engine_run_config_v2::Migration),
            Box::new(m20260908_000010_run_interactions::Migration),
            Box::new(m20260909_000011_observation_ledger::Migration),
            Box::new(m20260910_000012_message_steer_target::Migration),
            Box::new(m20260910_000013_queue_message_config_snapshot::Migration),
        ]
    }
}

/// Failure to bring the native database schema to its current version.
#[derive(Debug, Error)]
#[error("failed to migrate native sqlite database to the current schema")]
pub struct MigrationError {
    #[source]
    source: DbErr,
}

/// Applies every pending native migration in order.
///
/// The whole set runs inside ONE owned transaction on ONE pooled
/// connection. SeaORM's migrator does not wrap SQLite migrations itself,
/// and the observed failure reproduces across pooled connections within
/// one open: drop/create DDL (notably migration 000009's shape triggers)
/// interleaves and fails with already-exists conflicts. One connection
/// plus one atomic migration set removes that interleaving surface, and
/// a failed set is explicitly rolled back so a retry never meets
/// half-applied DDL without its tracking records. No serialization
/// guarantee beyond that is claimed: a racing opener can still observe a
/// read-to-write BUSY failure, which surfaces typed like any other
/// migration error.
///
/// Calling this function after the schema is current is a no-op. Forge calls
/// it during startup after opening its sole production database handle.
///
/// # Errors
///
/// Returns [`MigrationError`] with the original `SeaORM` migration failure.
pub async fn migrate_to_current(database: &DatabaseConnection) -> Result<(), MigrationError> {
    let transaction = database
        .begin()
        .await
        .map_err(|source| MigrationError { source })?;
    if let Err(source) = Migrator::up(&transaction, None).await {
        let _ = transaction.rollback().await;
        return Err(MigrationError { source });
    }
    transaction
        .commit()
        .await
        .map_err(|source| MigrationError { source })
}
