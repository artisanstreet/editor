//! SQLite connection configuration and startup policy.

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbErr};
use sqlx_sqlite::{SqliteAutoVacuum, SqliteJournalMode, SqliteSynchronous};
use thiserror::Error;

const MEMORY_DATABASE_URL: &str = "sqlite::memory:";
const FILE_DATABASE_URL: &str = "sqlite:";
const DEFAULT_MIN_CONNECTIONS: u32 = 1;
const DEFAULT_MAX_CONNECTIONS: u32 = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The physical SQLite location opened by [`SqliteConfig`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum Location {
    Memory,
    File { path: PathBuf },
}

/// Explicit connection configuration for a SQLite database owned by Forge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SqliteConfig {
    location: Location,
    min_connections: u32,
    max_connections: u32,
    sqlx_logging: bool,
}

impl SqliteConfig {
    /// Creates configuration for an isolated process-local memory database.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            location: Location::Memory,
            min_connections: DEFAULT_MIN_CONNECTIONS,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            sqlx_logging: true,
        }
    }

    /// Creates configuration for a file-backed production database.
    ///
    /// The database file is created when missing. Its parent directory must
    /// already exist; deciding and creating Forge's data directory belongs to
    /// process assembly rather than this persistence boundary.
    #[must_use]
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            location: Location::File { path: path.into() },
            min_connections: DEFAULT_MIN_CONNECTIONS,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            sqlx_logging: true,
        }
    }

    /// Sets the minimum number of physical connections retained by the pool.
    #[must_use]
    pub const fn min_connections(mut self, min_connections: u32) -> Self {
        self.min_connections = min_connections;
        self
    }

    /// Sets the maximum number of physical connections held by the pool.
    #[must_use]
    pub const fn max_connections(mut self, max_connections: u32) -> Self {
        self.max_connections = max_connections;
        self
    }

    /// Enables or disables `SeaORM`'s per-statement `SQLx` logging.
    #[must_use]
    pub const fn sqlx_logging(mut self, sqlx_logging: bool) -> Self {
        self.sqlx_logging = sqlx_logging;
        self
    }

    fn validate(&self) -> Result<(), ConnectError> {
        if self.min_connections == 0 {
            return Err(ConnectError::InvalidConfig {
                reason: "min_connections must be at least one".to_owned(),
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

        if let Location::File { path, .. } = &self.location {
            if path.as_os_str().is_empty() {
                return Err(ConnectError::InvalidPath {
                    path: path.clone(),
                    reason: "database path is empty".to_owned(),
                });
            }

            if path.is_dir() {
                return Err(ConnectError::InvalidPath {
                    path: path.clone(),
                    reason: "database path names a directory".to_owned(),
                });
            }
        }

        Ok(())
    }

    fn connect_options(&self, initialize_auto_vacuum: bool) -> ConnectOptions {
        let database_url = match self.location {
            Location::Memory => MEMORY_DATABASE_URL,
            Location::File { .. } => FILE_DATABASE_URL,
        };

        let mut options = ConnectOptions::new(database_url);
        options
            .min_connections(self.min_connections)
            .max_connections(self.max_connections)
            .connect_timeout(CONNECT_TIMEOUT)
            .acquire_timeout(ACQUIRE_TIMEOUT)
            .sqlx_logging(self.sqlx_logging);

        if let Location::File { path } = &self.location {
            let path = path.clone();
            options.map_sqlx_sqlite_opts(move |sqlite| {
                let sqlite = sqlite
                    .filename(path.clone())
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .synchronous(SqliteSynchronous::Normal)
                    .busy_timeout(BUSY_TIMEOUT)
                    .pragma("temp_store", "MEMORY")
                    .pragma("cache_size", "-65536")
                    .pragma("journal_size_limit", "8388608")
                    .pragma("wal_autocheckpoint", "1000");

                if initialize_auto_vacuum {
                    sqlite.auto_vacuum(SqliteAutoVacuum::Incremental)
                } else {
                    sqlite
                }
            });
        }

        options
    }

    fn location_display(&self) -> String {
        match &self.location {
            Location::Memory => "memory database".to_owned(),
            Location::File { path } => path.display().to_string(),
        }
    }
}

impl fmt::Display for SqliteConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.location_display())
    }
}

/// Failure modes for opening a configured SQLite database.
#[derive(Debug, Error)]
pub enum ConnectError {
    /// The pool configuration cannot be satisfied.
    #[error("invalid sqlite connection configuration: {reason}")]
    InvalidConfig {
        /// Why the configuration is invalid.
        reason: String,
    },

    /// The configured file path cannot name a SQLite database.
    #[error("invalid sqlite database path `{path}`: {reason}", path = .path.display())]
    InvalidPath {
        /// Rejected filesystem path.
        path: PathBuf,
        /// Why the path is invalid.
        reason: String,
    },

    /// `SeaORM` failed to open the configured database.
    #[error("failed to open sqlite database `{location}`")]
    Connect {
        /// Human-readable database location.
        location: String,
        /// Underlying `SeaORM` failure.
        #[source]
        source: DbErr,
    },
}

/// Opens a configured SQLite database through `SeaORM`.
///
/// # Errors
///
/// Returns a typed validation error before opening unusable configurations,
/// or [`ConnectError::Connect`] with the original `SeaORM` error when opening
/// the database fails.
pub async fn connect(config: SqliteConfig) -> Result<DatabaseConnection, ConnectError> {
    config.validate()?;
    let location = config.location_display();
    let initialize_auto_vacuum = match &config.location {
        Location::Memory => false,
        Location::File { path } => is_missing_or_empty_file(path),
    };

    Database::connect(config.connect_options(initialize_auto_vacuum))
        .await
        .map_err(|source| ConnectError::Connect { location, source })
}

fn is_missing_or_empty_file(path: &Path) -> bool {
    match path.metadata() {
        Ok(metadata) => metadata.is_file() && metadata.len() == 0,
        Err(error) => error.kind() == ErrorKind::NotFound,
    }
}
