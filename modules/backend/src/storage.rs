//! Forge-owned startup and shutdown boundary for the native database.
//!
//! The database crate owns SQLite connection policy and repository behavior;
//! the migrations crate owns the immutable schema history. Forge composes
//! those foundations here so no service can observe a repository until every
//! pending migration has succeeded. Product assembly supplies the concrete
//! [`SqliteConfig`] after its data-directory policy is selected—this boundary
//! does not guess a source-tree path or silently open a legacy database.

use std::time::Duration;

use artisan_database::{ConnectError, Repository, SqliteConfig, checkpoint_wal, connect};
use artisan_migrations::{MigrationError, migrate_to_current};
use sea_orm::{DatabaseConnection, DbErr};
use thiserror::Error;

use crate::error_chain::ErrorChain;

/// How often committed WAL pages are moved into the database file.
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(1);

/// An opened, current-schema native database owned exclusively by Forge.
#[derive(Debug)]
pub struct ForgeStorage {
    database: DatabaseConnection,
    repository: Repository,
    checkpointer: Checkpointer,
}

impl ForgeStorage {
    /// Opens Forge's configured database and applies every pending migration
    /// before constructing its repository facade.
    ///
    /// # Errors
    ///
    /// Returns a typed connection or migration failure with the original
    /// lower-level source preserved. No [`ForgeStorage`] value is exposed
    /// unless both startup stages complete.
    pub async fn open(config: SqliteConfig) -> Result<Self, ForgeStorageOpenError> {
        let database = connect(config)
            .await
            .map_err(|source| ForgeStorageOpenError::Connect { source })?;
        migrate_to_current(&database)
            .await
            .map_err(|source| ForgeStorageOpenError::Migrate { source })?;
        let repository = Repository::new(database.clone());
        let checkpointer = Checkpointer(tokio::spawn(checkpoint_periodically(database.clone())));
        Ok(Self {
            database,
            repository,
            checkpointer,
        })
    }

    /// Returns the repository facade backed by this owned database pool.
    #[must_use]
    pub const fn repository(&self) -> &Repository {
        &self.repository
    }

    /// Drops Forge's repository handle and explicitly closes the underlying
    /// database pool.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeStorageCloseError`] when `SeaORM` cannot close the pool
    /// cleanly.
    pub async fn close(self) -> Result<(), ForgeStorageCloseError> {
        let Self {
            database,
            repository,
            checkpointer,
        } = self;
        drop(checkpointer);
        drop(repository);
        database
            .close()
            .await
            .map_err(|source| ForgeStorageCloseError { source })
    }
}

/// The background checkpoint task, stopped when the storage closes or drops.
#[derive(Debug)]
struct Checkpointer(tokio::task::JoinHandle<()>);

impl Drop for Checkpointer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Checkpoints the WAL on its own pool connection, outside the write gate,
/// so commits never wait for the database file's fsync. Logs the first
/// failure of a run of failures and the recovery after it.
async fn checkpoint_periodically(database: DatabaseConnection) {
    let mut ticks = tokio::time::interval(CHECKPOINT_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut failing = false;
    loop {
        ticks.tick().await;
        match checkpoint_wal(&database).await {
            Ok(()) if failing => {
                failing = false;
                eprintln!("Forge database checkpoint recovered");
            }
            Ok(()) => {}
            Err(error) if !failing => {
                failing = true;
                eprintln!(
                    "Forge database checkpoint failed (retrying every {} s): {}",
                    CHECKPOINT_INTERVAL.as_secs(),
                    ErrorChain(&error)
                );
            }
            Err(_) => {}
        }
    }
}

/// Failure to produce a ready, current-schema Forge storage boundary.
#[derive(Debug, Error)]
pub enum ForgeStorageOpenError {
    /// SQLite connection policy or opening failed.
    #[error("failed to open Forge's native sqlite database")]
    Connect {
        #[source]
        source: ConnectError,
    },

    /// The database opened, but its schema could not reach the current
    /// immutable migration version.
    #[error("failed to migrate Forge's native sqlite database")]
    Migrate {
        #[source]
        source: MigrationError,
    },
}

/// Failure to close Forge's owned database pool cleanly.
#[derive(Debug, Error)]
#[error("failed to close Forge's native sqlite database")]
pub struct ForgeStorageCloseError {
    #[source]
    source: DbErr,
}
