//! `SQLite` persistence boundary owned by Forge.
//!
//! The crate intentionally exposes one narrow capability today: opening a
//! configured `SQLite` database through `SeaORM` with typed connection errors.
//! Entities, repositories, and migrations stay outside this boundary so the
//! seam between Forge and its storage engine remains small and reviewable.
//!
//! Only process-local memory databases are configurable during the Phase 1
//! proofs. File-backed locations arrive with the schema work packets, where
//! path-to-URL handling can be designed and tested deliberately.

use std::fmt;

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};
use thiserror::Error;

pub use artisan_domain::WorkspaceId;

/// Deterministic URL for a process-local `SQLite` memory database.
///
/// `SQLx` rewrites `sqlite::memory:` into a unique, shared-cache database per
/// connection pool: every physical connection inside one pool observes the
/// same schema and rows, while two pools never share state. Shared cache also
/// means the database lives exactly as long as its pool holds at least one
/// connection open, which [`SqliteConfig`] guarantees through
/// `min_connections`.
const MEMORY_DATABASE_URL: &str = "sqlite::memory:";

/// Explicit connection configuration for a `SQLite` database owned by Forge.
///
/// Defaults are deliberate:
///
/// - a memory pool keeps at least one physical connection alive so the
///   shared-cache memory database cannot vanish while the handle exists;
/// - every pool carries a finite upper bound on physical connections;
/// - `SeaORM`'s per-statement `SQLx` logging starts enabled because a running
///   Forge wants the trail, and tests silence it through
///   [`SqliteConfig::sqlx_logging`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqliteConfig {
    database_url: String,
    min_connections: u32,
    max_connections: u32,
    sqlx_logging: bool,
}

impl SqliteConfig {
    /// Configuration for a deterministic, process-local memory database.
    ///
    /// Each call to [`connect`] with this configuration yields an isolated
    /// memory database, and pooled connections inside one such database share
    /// a single schema and row space.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            database_url: MEMORY_DATABASE_URL.to_owned(),
            min_connections: 1,
            max_connections: 4,
            sqlx_logging: true,
        }
    }

    /// Sets the smallest number of physical connections the pool keeps open.
    ///
    /// Memory databases require at least one resident connection; a value of
    /// zero therefore produces [`ConnectError::InvalidConfig`].
    #[must_use]
    pub fn min_connections(mut self, min_connections: u32) -> Self {
        self.min_connections = min_connections;
        self
    }

    /// Sets the largest number of physical connections the pool may hold.
    ///
    /// A value of zero cannot acquire connections at all and therefore
    /// produces [`ConnectError::InvalidConfig`].
    #[must_use]
    pub fn max_connections(mut self, max_connections: u32) -> Self {
        self.max_connections = max_connections;
        self
    }

    /// Enables or disables `SeaORM`'s per-statement `SQLx` log output.
    #[must_use]
    pub const fn sqlx_logging(mut self, sqlx_logging: bool) -> Self {
        self.sqlx_logging = sqlx_logging;
        self
    }

    fn validate(self) -> Result<Self, ConnectError> {
        if self.min_connections == 0 {
            return Err(ConnectError::InvalidConfig {
                reason: "min_connections must be at least one for an in-memory database".to_owned(),
            });
        }

        if self.max_connections == 0 {
            return Err(ConnectError::InvalidConfig {
                reason: "max_connections must be at least one".to_owned(),
            });
        }

        if self.min_connections > self.max_connections {
            return Err(ConnectError::InvalidConfig {
                reason: format!(
                    "min_connections ({}) exceeds max_connections ({})",
                    self.min_connections, self.max_connections
                ),
            });
        }

        Ok(self)
    }

    fn into_connect_options(self) -> ConnectOptions {
        let mut options = ConnectOptions::new(self.database_url);
        options
            .min_connections(self.min_connections)
            .max_connections(self.max_connections)
            .sqlx_logging(self.sqlx_logging);
        options
    }
}

/// Failure modes for opening a configured `SQLite` database.
#[derive(Debug, Error)]
pub enum ConnectError {
    /// The configuration cannot describe a usable connection pool.
    #[error("invalid sqlite connection configuration: {reason}")]
    InvalidConfig {
        /// Why the configuration cannot be satisfied.
        reason: String,
    },

    /// `SeaORM` failed to open the configured database.
    #[error("failed to open sqlite database `{location}`")]
    Connect {
        /// The database location that `SeaORM` rejected.
        location: String,
        /// The underlying `SeaORM` failure, preserved as the error source.
        #[source]
        source: DbErr,
    },
}

impl fmt::Display for SqliteConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.database_url)
    }
}

/// Opens the configured `SQLite` database through `SeaORM`.
///
/// # Errors
///
/// Returns [`ConnectError::InvalidConfig`] when the configuration cannot
/// describe a usable pool, and [`ConnectError::Connect`] when `SeaORM` cannot
/// open the database, keeping the original [`DbErr`] as the error source.
pub async fn connect(config: SqliteConfig) -> Result<DatabaseConnection, ConnectError> {
    let config = config.validate()?;
    let location = config.database_url.clone();

    Database::connect(config.into_connect_options())
        .await
        .map_err(|source| ConnectError::Connect { location, source })
}
