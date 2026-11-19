use super::*;
use crate::schema::{MIGRATIONS, Upgrade, apply, migrate_to};
use rusqlite::config::DbConfig;

const EVERY: Duration = Duration::from_secs(5);

fn upgrade() -> Upgrade {
    Upgrade::of(&crate::Environment::system())
}

fn back_to(database: &Path, supported: usize) -> Result<Rollback, RollbackError> {
    roll_back(database, supported, 1, EVERY)
}

fn interrupted_upgrade(database: &Path, from: usize, committed: usize) {
    let mut conn = open(database);
    before_migrating(&mut conn, from, EVERY).expect("the newer Forge backs up the release");
    for (index, steps) in MIGRATIONS.iter().enumerate().take(committed).skip(from) {
        apply(&mut conn, index + 1, steps, 1).expect("a migration commits");
    }
}

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
        migrate_to(&mut conn, previous, upgrade()).expect("previous Forge creates its database");
        remember(&conn, "before upgrade");
    }
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, MIGRATIONS.len(), upgrade()).expect("newer Forge upgrades");
        remember(&conn, "after upgrade");
    }
    match roll_back(&database, previous, 1, EVERY).expect("roll back") {
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
    migrate_to(&mut conn, previous, upgrade()).expect("previous Forge opens its restored database");
    assert_eq!(
        back_to(&database, previous).expect("second start"),
        Rollback::Current
    );
}

#[test]
fn a_newer_database_without_a_backup_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, MIGRATIONS.len(), upgrade())
            .expect("newer Forge creates its database");
        remember(&conn, "newer");
    }
    let error = back_to(&database, MIGRATIONS.len() - 1).expect_err("no backup");
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
        migrate_to(&mut conn, previous, upgrade()).expect("previous Forge creates its database");
        remember(&conn, "before upgrade");
    }
    let mut crashed = open(&database);
    crashed
        .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .expect("leave the log unchecked on close, as a crash does");
    migrate_to(&mut crashed, MIGRATIONS.len(), upgrade()).expect("newer Forge upgrades");
    remember(&crashed, "only in the log");
    drop(crashed);
    assert!(
        dir.path().join("forge.sqlite3-wal").exists(),
        "the crashed Forge left its log"
    );

    match back_to(&database, previous).expect("roll back after the crash") {
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

#[test]
fn an_upgrade_interrupted_after_any_committed_migration_still_goes_back_to_where_it_started() {
    let released = MIGRATIONS.len() - 4;
    for committed in released + 1..MIGRATIONS.len() {
        let dir = tempfile::tempdir().expect("data dir");
        let database = dir.path().join("forge.sqlite3");
        {
            let mut conn = open(&database);
            migrate_to(&mut conn, released, upgrade()).expect("the release creates its database");
            remember(&conn, "released");
        }
        interrupted_upgrade(&database, released, committed);
        {
            let mut conn = open(&database);
            migrate_to(&mut conn, MIGRATIONS.len(), upgrade())
                .expect("the newer Forge finishes the upgrade after a restart");
            remember(&conn, "upgraded");
        }

        match back_to(&database, released).expect("the release goes back") {
            Rollback::Restored { restored, .. } => assert_eq!(
                restored, released,
                "stopped after migration {committed}: the release gets its own schema back"
            ),
            Rollback::Current => panic!("stopped after migration {committed}: nothing restored"),
        }
        assert_eq!(version(&database), released);
        assert_eq!(keys(&database), ["released"]);
    }
}

#[test]
fn an_older_release_two_upgrades_back_still_gets_its_database() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    let oldest = MIGRATIONS.len() - 2;
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, oldest, upgrade()).expect("the oldest release creates its database");
        remember(&conn, "oldest");
    }
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, oldest + 1, upgrade()).expect("the next release upgrades");
        remember(&conn, "next");
    }
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, MIGRATIONS.len(), upgrade()).expect("today's Forge upgrades");
        remember(&conn, "today");
    }

    match back_to(&database, oldest).expect("the oldest release goes back") {
        Rollback::Restored {
            restored, aside, ..
        } => {
            assert_eq!(restored, oldest);
            assert_eq!(keys(&aside), ["next", "oldest", "today"]);
        }
        Rollback::Current => panic!("nothing was restored"),
    }
    assert_eq!(keys(&database), ["oldest"]);
}

#[test]
fn a_release_between_two_backups_restores_the_older_one_and_upgrades_it_itself() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    let oldest = MIGRATIONS.len() - 3;
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, oldest, upgrade()).expect("the oldest release creates its database");
        remember(&conn, "oldest");
    }
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, MIGRATIONS.len(), upgrade()).expect("today's Forge upgrades");
        remember(&conn, "today");
    }

    match back_to(&database, oldest + 1).expect("a release that never ran here goes back") {
        Rollback::Restored { restored, .. } => assert_eq!(restored, oldest),
        Rollback::Current => panic!("nothing was restored"),
    }
    let mut conn = open(&database);
    migrate_to(&mut conn, oldest + 1, upgrade()).expect("that release upgrades the older backup");
    assert_eq!(keys(&database), ["oldest"]);
}

#[test]
fn a_rollback_cut_short_after_restoring_is_still_reported_by_the_next_forge() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    let previous = MIGRATIONS.len() - 1;
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, previous, upgrade()).expect("previous Forge creates its database");
    }
    {
        let mut conn = open(&database);
        migrate_to(&mut conn, MIGRATIONS.len(), upgrade()).expect("newer Forge upgrades");
    }

    let Rollback::Restored { aside, .. } = back_to(&database, previous).expect("roll back") else {
        panic!("nothing was restored");
    };

    let notice = retained::pending_notice(&database)
        .expect("read the pending notice")
        .expect("the rollback waits to be reported");
    assert_eq!(
        (notice.found, notice.restored, notice.set_aside),
        (
            u32::try_from(MIGRATIONS.len()).expect("small"),
            u32::try_from(previous).expect("small"),
            aside.display().to_string()
        )
    );
}

#[test]
fn a_long_backup_keeps_writing_to_the_log_so_ae_sees_forge_working() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    let previous = MIGRATIONS.len() - 1;
    let mut conn = open(&database);
    migrate_to(&mut conn, previous, upgrade()).expect("previous Forge creates its database");
    let settings = conn.transaction().expect("begin writing settings");
    for key in 0..5000 {
        remember(&settings, &format!("setting {key}"));
    }
    settings.commit().expect("commit the settings");

    let (logged, _capturing) = test_support::logs::capture();
    before_migrating(&mut conn, previous, Duration::ZERO).expect("back up before upgrading");

    assert!(
        logged
            .text()
            .contains(&format!("still backing up schema {previous}")),
        "{}",
        logged.text()
    );
}
