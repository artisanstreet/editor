//! Explicit Forge application configuration and owning startup boundary.
//!
//! Process assembly selects the concrete [`SqliteConfig`]—including the
//! database location—and injects it through [`ForgeConfig`]; nothing in this
//! boundary guesses a data directory, falls back to a default, or derives a
//! location from its environment. [`ForgeApp`] consumes that configuration
//! exactly once: it opens migrated [`ForgeStorage`] before becoming ready and
//! releases the owned storage through a typed shutdown boundary.

use artisan_database::{Repository, SqliteConfig};
use thiserror::Error;

use crate::storage::{ForgeStorage, ForgeStorageCloseError, ForgeStorageOpenError};

/// Explicit startup configuration injected into Forge by process assembly.
///
/// The type deliberately has no `Default`: starting Forge without a
/// caller-selected database location is a defect this boundary makes
/// impossible to express.
pub struct ForgeConfig {
    sqlite: SqliteConfig,
}

impl ForgeConfig {
    /// Creates startup configuration around the caller-selected SQLite
    /// configuration.
    #[must_use]
    pub const fn new(sqlite: SqliteConfig) -> Self {
        Self { sqlite }
    }
}

/// Owning Forge application value created by configured startup.
///
/// The value exclusively owns its [`ForgeStorage`]; no storage or repository
/// access exists before startup completes, and [`Self::shutdown`] is the
/// typed boundary that closes the owned pool.
pub struct ForgeApp {
    storage: ForgeStorage,
}

impl ForgeApp {
    /// Starts Forge from an explicit configuration.
    ///
    /// The owned database is opened and migrated to the current schema before
    /// a ready [`ForgeApp`] is returned.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeStartupError`] when the injected configuration cannot
    /// produce ready storage. No application value exists on failure.
    pub async fn start(config: ForgeConfig) -> Result<Self, ForgeStartupError> {
        let ForgeConfig { sqlite } = config;
        let storage = ForgeStorage::open(sqlite)
            .await
            .map_err(|source| ForgeStartupError { source })?;
        Ok(Self { storage })
    }

    /// Returns the repository facade backed by this application's migrated
    /// storage.
    #[must_use]
    pub const fn repository(&self) -> &Repository {
        self.storage.repository()
    }

    /// Shuts Forge down by closing its owned storage exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeShutdownError`] when the owned database pool cannot be
    /// closed cleanly.
    pub async fn shutdown(self) -> Result<(), ForgeShutdownError> {
        self.storage
            .close()
            .await
            .map_err(|source| ForgeShutdownError { source })
    }
}

/// Failure to bring the Forge application to ready state.
#[derive(Debug, Error)]
#[error("failed to start the Forge application")]
pub struct ForgeStartupError {
    /// Typed failure of the underlying storage startup stage.
    #[source]
    source: ForgeStorageOpenError,
}

/// Failure to close the Forge application's owned storage during shutdown.
#[derive(Debug, Error)]
#[error("failed to shut down the Forge application")]
pub struct ForgeShutdownError {
    /// Typed failure of the underlying storage close stage.
    #[source]
    source: ForgeStorageCloseError,
}
