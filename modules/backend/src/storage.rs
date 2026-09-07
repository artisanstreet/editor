//! Forge-owned startup and shutdown boundary for the native database.
//!
//! The database crate owns SQLite connection policy and repository behavior;
//! the migrations crate owns the immutable schema history. Forge composes
//! those foundations here so no service can observe a repository until every
//! pending migration has succeeded. Product assembly supplies the concrete
//! [`SqliteConfig`] after its data-directory policy is selected—this boundary
//! does not guess a source-tree path or silently open a legacy database.

use artisan_database::{ConnectError, Repository, SqliteConfig, connect};
use artisan_migrations::{MigrationError, migrate_to_current};
use sea_orm::{DatabaseConnection, DbErr};
use thiserror::Error;

/// An opened, current-schema native database owned exclusively by Forge.
#[derive(Debug)]
pub struct ForgeStorage {
    database: DatabaseConnection,
    repository: Repository,
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
        Ok(Self {
            database,
            repository,
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
        } = self;
        drop(repository);
        database
            .close()
            .await
            .map_err(|source| ForgeStorageCloseError { source })
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
