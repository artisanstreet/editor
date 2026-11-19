use super::*;
use crate::schema::{MIGRATIONS, migrate_to};
use rusqlite::config::DbConfig;

fn open(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open the database");
    conn.pragma_update(None, "journal_mode", "WAL")
        .expect("enable WAL");
    conn
}

fn remember(conn: &Connection, key: &str) {
    conn.execute(include_str!("backup/remember.sql"), [key])
        .expect("write a setting");
}

fn version(path: &Path) -> usize {
    Connection::open(path)
        .expect("open the database")
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read the schema version")
}

fn keys(path: &Path) -> Vec<String> {
    let conn = Connection::open(path).expect("open the database");
    let mut select = conn
        .prepare(include_str!("backup/keys.sql"))
        .expect("prepare");
    select
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("read keys")
}

#[test]
fn reinstalling_the_previous_forge_restores_its_data_and_sets_newer_data_aside() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    let previous = MIGRATIONS.len() - 1;
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, previous).expect("previous Forge creates its database");
        remember(&conn, "before upgrade");
    }
    std::fs::write(dir.path().join("forge.sqlite3.schema-1"), "stale").expect("older backup");
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, MIGRATIONS.len()).expect("newer Forge upgrades");
        remember(&conn, "after upgrade");
    }
    assert!(
        !dir.path().join("forge.sqlite3.schema-1").exists(),
        "only the latest backup is kept"
    );
    match roll_back(&database, previous).expect("roll back") {
        Rollback::Restored {
            found,
            restored,
            aside,
        } => {
            assert_eq!((found, restored), (MIGRATIONS.len(), previous));
            assert_eq!(version(&aside), MIGRATIONS.len());
            assert_eq!(keys(&aside), ["after upgrade", "before upgrade"]);
        }
        Rollback::Current => panic!("a newer database was left in place"),
    }
    assert_eq!(version(&database), previous);
    assert_eq!(keys(&database), ["before upgrade"]);
    let mut conn = open(&database);
    migrate_to(&mut conn, previous).expect("previous Forge opens its restored database");
    assert_eq!(
        roll_back(&database, previous).expect("second start"),
        Rollback::Current
    );
}

#[test]
fn a_newer_database_without_a_backup_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, MIGRATIONS.len()).expect("newer Forge creates its database");
        remember(&conn, "newer");
    }
    let error = roll_back(&database, MIGRATIONS.len() - 1).expect_err("no backup");
    assert!(matches!(error, RollbackError::NoBackup { .. }), "{error}");
    assert_eq!(version(&database), MIGRATIONS.len());
    assert_eq!(keys(&database), ["newer"]);
}

#[test]
fn a_newer_database_with_its_log_still_on_disk_is_rolled_back_keeping_the_logged_writes() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    let previous = MIGRATIONS.len() - 1;
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, previous).expect("previous Forge creates its database");
        remember(&conn, "before upgrade");
    }
    let mut crashed = open(&database);
    crashed
        .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .expect("leave the log unchecked on close, as a crash does");
    migrate_to(&mut crashed, MIGRATIONS.len()).expect("newer Forge upgrades");
    remember(&crashed, "only in the log");
    drop(crashed);
    assert!(
        dir.path().join("forge.sqlite3-wal").exists(),
        "the crashed Forge left its log"
    );

    match roll_back(&database, previous).expect("roll back after the crash") {
        Rollback::Restored { aside, .. } => {
            assert_eq!(
                keys(&aside),
                ["before upgrade", "only in the log"],
                "the logged write is kept with the newer data"
            );
        }
        Rollback::Current => panic!("a newer database was left in place"),
    }
    assert_eq!(version(&database), previous);
    assert_eq!(keys(&database), ["before upgrade"]);
}
