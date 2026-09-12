//! SQLite connection configuration and startup policy.

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, DbErr, Statement,
};
use sqlx_sqlite::{SqliteAutoVacuum, SqliteJournalMode, SqliteSynchronous};
use thiserror::Error;

const MEMORY_DATABASE_URL: &str = "sqlite::memory:";
const FILE_DATABASE_URL: &str = "sqlite:";
const DEFAULT_MIN_CONNECTIONS: u32 = 1;
const DEFAULT_MAX_CONNECTIONS: u32 = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Upper bound on the findings a single open-time verification may report,
/// so a badly damaged image cannot produce unbounded result rows.
const QUICK_CHECK_FINDING_LIMIT: u32 = 100;
/// The single verdict `PRAGMA quick_check` reports for a usable database.
const VERIFICATION_OK: &str = "ok";
/// Lowest SQLite version the schema relies on.
///
/// The down paths of the engine-run-config and message-steer-target
/// migrations use `ALTER TABLE ... DROP COLUMN`, which SQLite only supports
/// from 3.35.0. The bundled runtime satisfies this; the open-time check keeps
/// a system-linked SQLite from silently accepting migrations the schema
/// cannot later reverse.
pub const MIN_SQLITE_VERSION: (u32, u32, u32) = (3, 35, 0);

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

    /// The opened file database failed SQLite's usability verification.
    #[error("sqlite database `{location}` failed integrity verification: {findings}")]
    CorruptImage {
        /// Human-readable database location.
        location: String,
        /// Findings reported by the bounded open-time `PRAGMA quick_check`.
        findings: String,
    },

    /// The usability verification failed before it could produce a verdict.
    #[error("failed to verify sqlite database `{location}`")]
    VerificationFailed {
        /// Human-readable database location.
        location: String,
        /// Underlying `SeaORM` failure from running the verification query.
        #[source]
        source: DbErr,
    },

    /// The linked SQLite is older than the schema's migration floor.
    #[error(
        "sqlite database `{location}` runs sqlite {found}, but {minimum} is required for reversible migrations"
    )]
    UnsupportedSqliteVersion {
        /// Human-readable database location.
        location: String,
        /// Version reported by the linked SQLite, when readable.
        found: String,
        /// Lowest supported version, rendered `major.minor.patch`.
        minimum: String,
    },
}

/// Opens a configured SQLite database through `SeaORM`.
///
/// File-backed databases additionally run a bounded `PRAGMA quick_check`
/// once before the pool is handed out, so a damaged database image is
/// rejected as a typed startup failure instead of surfacing later as
/// mid-operation read or write errors.
///
/// # Errors
///
/// Returns a typed validation error before opening unusable configurations,
/// [`ConnectError::Connect`] with the original `SeaORM` error when opening
/// the database fails, and [`ConnectError::CorruptImage`] or
/// [`ConnectError::VerificationFailed`] when the opened file database does
/// not pass usability verification.
pub async fn connect(config: SqliteConfig) -> Result<DatabaseConnection, ConnectError> {
    config.validate()?;
    let location = config.location_display();
    let initialize_auto_vacuum = match &config.location {
        Location::Memory => false,
        Location::File { path } => is_missing_or_empty_file(path),
    };

    let connection = Database::connect(config.connect_options(initialize_auto_vacuum))
        .await
        .map_err(|source| ConnectError::Connect {
            location: location.clone(),
            source,
        })?;

    verify_version(&connection, &location).await?;

    if matches!(config.location, Location::File { .. }) {
        verify_image(&connection, &location).await?;
    }

    Ok(connection)
}

/// Rejects a SQLite older than the schema's reversible-migration floor.
///
/// # Errors
///
/// Returns [`ConnectError::VerificationFailed`] when the version query
/// cannot run, and [`ConnectError::UnsupportedSqliteVersion`] when the
/// reported version is missing, unparseable, or below
/// [`MIN_SQLITE_VERSION`].
async fn verify_version(database: &DatabaseConnection, location: &str) -> Result<(), ConnectError> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT sqlite_version()".to_owned(),
        ))
        .await
        .map_err(|source| ConnectError::VerificationFailed {
            location: location.to_owned(),
            source,
        })?;

    let found = row
        .ok_or_else(|| ConnectError::UnsupportedSqliteVersion {
            location: location.to_owned(),
            found: "unknown".to_owned(),
            minimum: format_version(MIN_SQLITE_VERSION),
        })?
        .try_get_by_index::<String>(0)
        .map_err(|source| ConnectError::VerificationFailed {
            location: location.to_owned(),
            source,
        })?;

    let supported = parse_sqlite_version(&found).is_some_and(|version| version >= MIN_SQLITE_VERSION);
    if supported {
        return Ok(());
    }

    Err(ConnectError::UnsupportedSqliteVersion {
        location: location.to_owned(),
        found,
        minimum: format_version(MIN_SQLITE_VERSION),
    })
}

/// Parses a SQLite `major.minor.patch` version string.
///
/// A missing patch component counts as zero so short forms like `3.45` are
/// accepted; any other shape returns `None` and is treated as unsupported.
fn parse_sqlite_version(raw: &str) -> Option<(u32, u32, u32)> {
    let mut parts = raw.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    Some((major, minor, patch))
}

/// Renders a parsed version back to `major.minor.patch`.
fn format_version(version: (u32, u32, u32)) -> String {
    let (major, minor, patch) = version;
    format!("{major}.{minor}.{patch}")
}

/// Verifies an opened file database image with SQLite's `quick_check`.
///
/// SQLite reads only the first page while opening a file database, so an
/// image with interior damage opens successfully and then fails later,
/// potentially during a write path. The bounded check runs once per open to
/// turn that latent failure into a typed startup error. Memory databases are
/// process-local and start empty every time, so they carry no damage across
/// sessions and skip it.
///
/// # Errors
///
/// Returns [`ConnectError::VerificationFailed`] when the verification query
/// itself fails before producing a verdict, and
/// [`ConnectError::CorruptImage`] with the reported findings otherwise.
async fn verify_image(database: &DatabaseConnection, location: &str) -> Result<(), ConnectError> {
    let verdicts = database
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("PRAGMA quick_check({QUICK_CHECK_FINDING_LIMIT})"),
        ))
        .await
        .map_err(|source| ConnectError::VerificationFailed {
            location: location.to_owned(),
            source,
        })?;

    let mut healthy = false;
    let mut findings = Vec::new();
    for verdict in verdicts {
        let finding = verdict.try_get_by_index::<String>(0).map_err(|source| {
            ConnectError::VerificationFailed {
                location: location.to_owned(),
                source,
            }
        })?;

        if finding == VERIFICATION_OK {
            healthy = true;
        } else {
            findings.push(finding);
        }
    }

    if healthy && findings.is_empty() {
        return Ok(());
    }

    Err(ConnectError::CorruptImage {
        location: location.to_owned(),
        findings: if findings.is_empty() {
            "verification returned no usable verdict".to_owned()
        } else {
            findings.join("; ")
        },
    })
}

fn is_missing_or_empty_file(path: &Path) -> bool {
    match path.metadata() {
        Ok(metadata) => metadata.is_file() && metadata.len() == 0,
        Err(error) => error.kind() == ErrorKind::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_version_parser_accepts_release_shapes() {
        assert_eq!(parse_sqlite_version("3.35.0"), Some((3, 35, 0)));
        assert_eq!(parse_sqlite_version(" 3.45.1\n"), Some((3, 45, 1)));
        assert_eq!(parse_sqlite_version("3.45"), Some((3, 45, 0)));
        assert_eq!(parse_sqlite_version("4.0.0"), Some((4, 0, 0)));
    }

    #[test]
    fn sqlite_version_parser_rejects_malformed_input() {
        assert_eq!(parse_sqlite_version(""), None);
        assert_eq!(parse_sqlite_version("3"), None);
        assert_eq!(parse_sqlite_version("3.x.0"), None);
        assert_eq!(parse_sqlite_version("3.35.0-beta"), None);
    }

    #[test]
    fn version_floor_is_the_drop_column_floor() {
        // `ALTER TABLE ... DROP COLUMN` needs 3.35.0; keep the gate and the
        // documented floor in one place.
        assert_eq!(format_version(MIN_SQLITE_VERSION), "3.35.0");
    }

    #[tokio::test]
    async fn linked_sqlite_meets_the_reversible_migration_floor() {
        // A system-linked SQLite below the floor must fail the open instead
        // of surfacing later as an unreversible down migration.
        let connection = connect(SqliteConfig::in_memory())
            .await
            .expect("bundled sqlite meets the migration floor");
        let row = connection
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT sqlite_version()".to_owned(),
            ))
            .await
            .expect("version query runs")
            .expect("version query returns a row");
        let raw = row.try_get_by_index::<String>(0).expect("version is text");
        let parsed = parse_sqlite_version(&raw).expect("reported version parses");
        assert!(
            parsed >= MIN_SQLITE_VERSION,
            "linked sqlite {raw} is below the migration floor"
        );
    }
}
