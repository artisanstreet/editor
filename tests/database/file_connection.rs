//! External tests for Forge's file-backed SQLite connection policy.

use std::error::Error;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::sqlite_write_retry::{
    INITIAL_RETRY_DELAY, SqliteWriteRetryDecision, decide_write_retry, is_retryable_write_error,
};
use artisan_database::{ConnectError, SqliteConfig, connect};
use sea_orm::{
    ConnectionTrait, DbBackend, SqliteTransactionMode, Statement, TransactionOptions,
    TransactionTrait,
};

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

/// Creates a populated database whose table data spills well past the first
/// page, so interior pages exist and can be damaged independently of the
/// header page SQLite reads while opening.
async fn seed_payload_database(temp: &TempDatabase) -> Result<(), Box<dyn Error>> {
    let database = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
    database
        .execute_unprepared("CREATE TABLE payload (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
        .await?;
    let filler = "x".repeat(512);
    for id in 1..=32 {
        let insert = format!("INSERT INTO payload (id, body) VALUES ({id}, '{filler}')");
        database.execute_unprepared(&insert).await?;
    }
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn verified_reopen_keeps_a_healthy_database_usable() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("verify-healthy")?;
    seed_payload_database(&temp).await?;

    let reopened = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
    reopened
        .execute_unprepared("INSERT INTO payload (id, body) VALUES (33, 'after verification')")
        .await?;

    let row = reopened
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) FROM payload",
        ))
        .await?
        .ok_or_else(|| {
            std::io::Error::other("payload rows were not readable after verification")
        })?;
    let count: i64 = row.try_get_by_index(0)?;
    assert_eq!(count, 33);

    reopened.close().await?;
    Ok(())
}

#[tokio::test]
async fn connect_rejects_an_image_with_interior_page_damage() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("verify-corrupt")?;
    seed_payload_database(&temp).await?;

    // Zero every page past the second one. The header page that SQLite reads
    // while opening stays valid, so without open-time verification this image
    // would open successfully and fail later on first real access.
    let size = fs::metadata(temp.database())?.len();
    assert!(size > 2 * 4_096);
    let mut damaged = fs::OpenOptions::new().write(true).open(temp.database())?;
    damaged.seek(SeekFrom::Start(2 * 4_096))?;
    damaged.write_all(&vec![0_u8; usize::try_from(size - 2 * 4_096)?])?;
    drop(damaged);

    let outcome = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await;
    assert!(matches!(
        outcome,
        Err(ConnectError::VerificationFailed { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn connect_reports_findings_for_a_damaged_freelist() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("verify-freelist")?;
    let database = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
    database
        .execute_unprepared("CREATE TABLE junk (id INTEGER PRIMARY KEY, body TEXT NOT NULL)")
        .await?;
    let filler = "y".repeat(400);
    for id in 1..=20 {
        let insert = format!("INSERT INTO junk (id, body) VALUES ({id}, '{filler}')");
        database.execute_unprepared(&insert).await?;
    }
    database.execute_unprepared("DROP TABLE junk").await?;
    database.close().await?;

    // Dropping the table moves its pages to the freelist, whose trunk page
    // then sits past the live pages. Garbage there leaves a structurally
    // openable image that verification must report through its findings.
    let mut damaged = fs::OpenOptions::new().write(true).open(temp.database())?;
    damaged.seek(SeekFrom::Start(3 * 4_096))?;
    damaged.write_all(&[0xAB_u8; 64])?;
    drop(damaged);

    match connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await {
        Err(ConnectError::CorruptImage { findings, .. }) => {
            assert!(
                findings.contains("Freelist"),
                "verification did not report the damaged freelist: {findings}"
            );
        }
        unexpected => {
            return Err(std::io::Error::other(format!(
                "expected a corrupt-image failure, got: {unexpected:?}"
            ))
            .into());
        }
    }
    Ok(())
}

#[tokio::test]
async fn connect_accepts_garbage_in_unused_page_space() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("verify-unused-space")?;
    seed_payload_database(&temp).await?;

    // Cell content grows toward the end of each page, so bytes just past a
    // page's cell-pointer array are unallocated. Corrupting them must not
    // fail verification: the logical image is still fully consistent.
    let mut damaged = fs::OpenOptions::new().write(true).open(temp.database())?;
    damaged.seek(SeekFrom::Start(2 * 4_096 + 256))?;
    damaged.write_all(&[0xFF_u8; 512])?;
    drop(damaged);

    let reopened = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
    let row = reopened
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) FROM payload",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("payload rows were not readable"))?;
    let count: i64 = row.try_get_by_index(0)?;
    assert_eq!(count, 32);

    reopened.close().await?;
    Ok(())
}

#[tokio::test]
async fn connect_rejects_a_truncated_image() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("verify-truncated")?;
    seed_payload_database(&temp).await?;

    let size = fs::metadata(temp.database())?.len();
    let truncated = fs::OpenOptions::new().write(true).open(temp.database())?;
    truncated.set_len(size / 2)?;
    drop(truncated);

    // A short image already fails while SQLite applies its startup pragmas,
    // before verification runs; this guards that rejection stays in place.
    let outcome = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await;
    assert!(matches!(outcome, Err(ConnectError::Connect { .. })));
    Ok(())
}

#[tokio::test]
async fn connect_treats_an_existing_empty_file_as_a_fresh_database() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("verify-empty-file")?;
    fs::write(temp.database(), [])?;

    let database = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
    assert_eq!(pragma_i64(&database, "auto_vacuum").await?, 2);
    database.close().await?;
    Ok(())
}

/// Reproduces the WAL `SQLITE_BUSY_SNAPSHOT` upgrade deterministically and
/// proves the immediate fence that every read-then-write repository
/// transaction now takes cannot reach it.
#[tokio::test]
async fn deferred_snapshot_upgrade_fails_busy_snapshot_while_immediate_begin_succeeds()
-> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("wal-busy-snapshot")?;
    let reader = connect(
        SqliteConfig::file(temp.database())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await?;
    let writer = connect(
        SqliteConfig::file(temp.database())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await?;

    reader
        .execute_unprepared("CREATE TABLE probe (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
        .await?;
    reader
        .execute_unprepared("INSERT INTO probe (id, value) VALUES (1, 'initial')")
        .await?;

    // A deferred read opens a snapshot without the writer fence.
    let deferred = reader.begin().await?;
    deferred
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT value FROM probe WHERE id = 1".to_owned(),
        ))
        .await?;

    // A concurrent commit advances the WAL past that snapshot.
    writer
        .execute_unprepared("UPDATE probe SET value = 'concurrent' WHERE id = 1")
        .await?;

    // Upgrading the stale snapshot to a write fails deterministically with
    // SQLITE_BUSY_SNAPSHOT, a failure class no busy_timeout covers.
    let upgrade = deferred
        .execute_unprepared("UPDATE probe SET value = 'stale' WHERE id = 1")
        .await;
    let error = upgrade.expect_err("a stale snapshot upgrade must fail");
    assert!(
        is_retryable_write_error(&error),
        "SQLITE_BUSY_SNAPSHOT must classify as retryable writer contention: {error}"
    );
    assert_eq!(
        decide_write_retry(&error, 0),
        SqliteWriteRetryDecision::Retry {
            repetition: 0,
            delay: INITIAL_RETRY_DELAY,
        }
    );
    deferred.rollback().await?;

    // BEGIN IMMEDIATE takes the writer fence before any read, so the same
    // interleaving cannot invalidate a snapshot the transaction later
    // upgrades.
    let immediate = reader
        .begin_with_options(TransactionOptions {
            sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
            ..Default::default()
        })
        .await?;
    immediate
        .execute_unprepared("UPDATE probe SET value = 'fenced' WHERE id = 1")
        .await?;
    immediate.commit().await?;

    let row = reader
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT value FROM probe WHERE id = 1".to_owned(),
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("probe row disappeared"))?;
    assert_eq!(row.try_get_by_index::<String>(0)?, "fenced");

    reader.close().await?;
    writer.close().await?;
    Ok(())
}
