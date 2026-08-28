//! Immutable, ordered migrations for the native Artisan database.
//!
//! This schema belongs to the Rust application. Importing a legacy
//! TypeScript-era database is an explicit future boundary and is never an
//! implicit startup migration.

mod m20260824_000001_initial_native_schema;
mod m20260824_000002_global_command_receipts;
mod m20260824_000003_conversation_execution;

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DatabaseConnection;
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
/// Calling this function after the schema is current is a no-op. Forge calls
/// it during startup after opening its sole production database handle.
///
/// # Errors
///
/// Returns [`MigrationError`] with the original `SeaORM` migration failure.
pub async fn migrate_to_current(database: &DatabaseConnection) -> Result<(), MigrationError> {
    Migrator::up(database, None)
        .await
        .map_err(|source| MigrationError { source })
}
