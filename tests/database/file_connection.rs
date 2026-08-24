//! External tests for Forge's file-backed SQLite connection policy.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::{ConnectError, SqliteConfig, connect};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

struct TempDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TempDatabase {
    fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "artisan-editor-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory)?;
        let database = directory.join("forge.sqlite3");
        Ok(Self {
            directory,
            database,
        })
    }

    fn database(&self) -> &Path {
        &self.database
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.directory);
    }
}

async fn pragma_i64(
    database: &sea_orm::DatabaseConnection,
    pragma: &str,
) -> Result<i64, Box<dyn Error>> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("PRAGMA {pragma}"),
        ))
        .await?
        .ok_or_else(|| std::io::Error::other(format!("PRAGMA {pragma} returned no row")))?;
    Ok(row.try_get_by_index(0)?)
}

async fn pragma_string(
    database: &sea_orm::DatabaseConnection,
    pragma: &str,
) -> Result<String, Box<dyn Error>> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("PRAGMA {pragma}"),
        ))
        .await?
        .ok_or_else(|| std::io::Error::other(format!("PRAGMA {pragma} returned no row")))?;
    Ok(row.try_get_by_index(0)?)
}

#[tokio::test]
async fn file_database_reopens_with_committed_data() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("reopen")?;
    let first = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
    first
        .execute_unprepared(
            "CREATE TABLE durable_value (id INTEGER PRIMARY KEY, value TEXT NOT NULL)",
        )
        .await?;
    first
        .execute_unprepared("INSERT INTO durable_value (id, value) VALUES (1, 'kept')")
        .await?;
    first.close().await?;

    let reopened = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
    let row = reopened
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT value FROM durable_value WHERE id = 1",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("committed row was not present after reopen"))?;
    let value: String = row.try_get_by_index(0)?;
    assert_eq!(value, "kept");
    reopened.close().await?;
    Ok(())
}

#[tokio::test]
async fn file_database_applies_the_production_pragmas() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("pragmas")?;
    let database = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;

    assert_eq!(pragma_i64(&database, "foreign_keys").await?, 1);
    assert_eq!(pragma_string(&database, "journal_mode").await?, "wal");
    assert_eq!(pragma_i64(&database, "synchronous").await?, 1);
    assert_eq!(pragma_i64(&database, "busy_timeout").await?, 5_000);
    assert_eq!(pragma_i64(&database, "temp_store").await?, 2);
    assert_eq!(pragma_i64(&database, "cache_size").await?, -65_536);
    assert_eq!(
        pragma_i64(&database, "journal_size_limit").await?,
        8_388_608
    );
    assert_eq!(pragma_i64(&database, "wal_autocheckpoint").await?, 1_000);
    assert_eq!(pragma_i64(&database, "auto_vacuum").await?, 2);

    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn rejects_invalid_config_and_invalid_paths() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("invalid")?;

    let bad_pool = connect(
        SqliteConfig::file(temp.database())
            .min_connections(5)
            .max_connections(4),
    )
    .await;
    assert!(matches!(bad_pool, Err(ConnectError::InvalidConfig { .. })));

    let directory_as_database = connect(SqliteConfig::file(&temp.directory)).await;
    assert!(matches!(
        directory_as_database,
        Err(ConnectError::InvalidPath { .. })
    ));

    let missing_parent = temp.directory.join("missing").join("forge.sqlite3");
    let missing_parent_result = connect(SqliteConfig::file(missing_parent)).await;
    assert!(matches!(
        missing_parent_result,
        Err(ConnectError::Connect { .. })
    ));
    Ok(())
}
