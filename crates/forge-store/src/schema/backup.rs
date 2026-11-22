use crate::progress::{self, Progress, Step};
use crate::retained::{self, RetainedError};
use artisan_api::{DatabaseRollback, Timestamp};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[cfg(test)]
#[path = "../../tests/schema/backup.rs"]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("{database} is not a Unicode path, which SQLite needs to copy it")]
    NotUnicode { database: PathBuf },
    #[error("measure {} and the free space beside it before backing up schema {version}", database.display())]
    Measure {
        database: PathBuf,
        version: usize,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the disk holding {} is full: backing up schema {version} before the upgrade needs about {needed} bytes and {available} are free; free some space and start Forge again",
        database.display()
    )]
    NoSpace {
        database: PathBuf,
        version: usize,
        needed: u64,
        available: u64,
        #[source]
        source: Option<Box<rusqlite::Error>>,
    },
    #[error("copy schema {version} of {} to {}", database.display(), backup.display())]
    Copy {
        database: PathBuf,
        version: usize,
        backup: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("report progress while backing up schema {version}")]
    Progress {
        version: usize,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("store the schema {version} backup {}", backup.display())]
    Store {
        version: usize,
        backup: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("keep the schema {version} backup {} with the images it uses", backup.display())]
    Keep {
        version: usize,
        backup: PathBuf,
        #[source]
        source: RetainedError,
    },
    #[error("record that the upgrade from schema {from} started")]
    Begin {
        from: usize,
        #[source]
        source: RetainedError,
    },
    #[error("read whether an earlier upgrade of {} was interrupted", database.display())]
    Attempt {
        database: PathBuf,
        #[source]
        source: RetainedError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum FinishUpgradeError {
    #[error("record that the upgrade of {} finished", database.display())]
    End {
        database: PathBuf,
        #[source]
        source: RetainedError,
    },
    #[error("remove the old backups beside {}", database.display())]
    Prune {
        database: PathBuf,
        #[source]
        source: RetainedError,
    },
    #[error("list the images the backups beside {} use", database.display())]
    Images {
        database: PathBuf,
        #[source]
        source: RetainedError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RollbackError {
    #[error("{} is not a Unicode path, which SQLite needs to copy it", database.display())]
    NotUnicode { database: PathBuf },
    #[error("open {} to read its schema", database.display())]
    Open {
        database: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("list the backups beside {}", database.display())]
    Backups {
        database: PathBuf,
        #[source]
        source: RetainedError,
    },
    #[error(
        "{} uses schema {found}, newer than {supported}, and there is no backup at schema {supported} or older beside it to go back to",
        database.display()
    )]
    NoBackup {
        database: PathBuf,
        found: usize,
        supported: usize,
    },
    #[error("set aside schema {found} of {} as {}", database.display(), aside.display())]
    SetAside {
        database: PathBuf,
        found: usize,
        aside: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("keep the set-aside schema {found} database {}", aside.display())]
    KeepAside {
        found: usize,
        aside: PathBuf,
        #[source]
        source: RetainedError,
    },
    #[error("remember the rollback of {} until Forge reports it", database.display())]
    Notice {
        database: PathBuf,
        #[source]
        source: RetainedError,
    },
    #[error("restore {} over {}", backup.display(), database.display())]
    Restore {
        database: PathBuf,
        backup: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("finish restoring {} over {}", backup.display(), database.display())]
    Settle {
        database: PathBuf,
        backup: PathBuf,
        #[source]
        source: RetainedError,
    },
    #[error("remove {} so the previous Forge starts without it", path.display())]
    Clear {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("finish removing {} for the previous Forge", database.display())]
    SettleClear {
        database: PathBuf,
        #[source]
        source: RetainedError,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Cleared {
    Nothing,
    SetAside { found: usize, aside: PathBuf },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rollback {
    Current,
    Restored {
        found: usize,
        restored: usize,
        aside: PathBuf,
    },
}

fn unicode(path: &Path) -> Option<&str> {
    path.to_str()
}

fn sync(path: &Path) -> std::io::Result<()> {
    std::fs::File::options().write(true).open(path)?.sync_all()
}

fn copy_into(conn: &Connection, target: &str) -> rusqlite::Result<()> {
    conn.execute(include_str!("../sql/backup/vacuum_into.sql"), [target])
        .map(drop)
}

fn remove_if_present(path: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[derive(Clone, Copy)]
struct Room {
    needed: u64,
    available: u64,
}

fn room(database: &Path) -> std::io::Result<Room> {
    let wal = {
        let mut name = database.as_os_str().to_owned();
        name.push("-wal");
        PathBuf::from(name)
    };
    let wal_bytes = match std::fs::metadata(&wal) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let folder = database
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(Room {
        needed: std::fs::metadata(database)?.len() + wal_bytes,
        available: fs2::available_space(folder)?,
    })
}

fn disk_full(error: &rusqlite::Error) -> bool {
    error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull)
}

fn file_database(conn: &Connection) -> Option<PathBuf> {
    conn.path()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

pub(super) fn before_migrating(
    conn: &mut Connection,
    version: usize,
    pace: &Progress,
) -> Result<(), BackupError> {
    let Some(database) = file_database(conn) else {
        return Ok(());
    };
    let resumable = retained::attempt(&database).map_err(|source| BackupError::Attempt {
        database: database.clone(),
        source,
    })?;
    match resumable {
        Some(0) => {
            tracing::info!(
                at = version,
                "resuming the interrupted creation of the database"
            );
            return Ok(());
        }
        Some(from) if from <= version && retained::backup(&database, from).is_file() => {
            tracing::info!(
                from,
                at = version,
                "resuming the interrupted upgrade; the schema {from} backup stays the one to go back to"
            );
            return Ok(());
        }
        Some(_) | None => {}
    }
    if version > 0 {
        back_up(conn, &database, version, pace)?;
    }
    retained::begin_attempt(&database, version).map_err(|source| BackupError::Begin {
        from: version,
        source,
    })
}

fn back_up(
    conn: &mut Connection,
    database: &Path,
    version: usize,
    pace: &Progress,
) -> Result<(), BackupError> {
    let backup = retained::backup(database, version);
    let partial = retained::partial(&backup);
    let stored = |source| BackupError::Store {
        version,
        backup: backup.clone(),
        source,
    };
    let measure = || {
        room(database).map_err(|source| BackupError::Measure {
            database: database.to_owned(),
            version,
            source,
        })
    };
    let target = unicode(&partial).ok_or_else(|| BackupError::NotUnicode {
        database: database.to_owned(),
    })?;
    if remove_if_present(&partial).map_err(stored)? {
        tracing::info!(
            version,
            partial = %partial.display(),
            "an earlier backup of schema {version} was cut short and can't be trusted; backing up again from the start"
        );
    }
    let before = measure()?;
    if before.available < before.needed {
        return Err(BackupError::NoSpace {
            database: database.to_owned(),
            version,
            needed: before.needed,
            available: before.available,
            source: None,
        });
    }
    let progress = |source| BackupError::Progress {
        version,
        source: Box::new(source),
    };
    progress::watch(conn, Step::Backup { version }, pace).map_err(progress)?;
    let copied = copy_into(conn, target);
    progress::unwatch(conn).map_err(progress)?;
    if let Err(source) = copied {
        remove_if_present(&partial).map_err(stored)?;
        if disk_full(&source) {
            let after = measure()?;
            return Err(BackupError::NoSpace {
                database: database.to_owned(),
                version,
                needed: before.needed,
                available: after.available,
                source: Some(Box::new(source)),
            });
        }
        return Err(BackupError::Copy {
            database: database.to_owned(),
            version,
            backup: backup.clone(),
            source: Box::new(source),
        });
    }
    sync(&partial).map_err(stored)?;
    let keep = |source| BackupError::Keep {
        version,
        backup: backup.clone(),
        source,
    };
    retained::record_images(&partial, &backup, pace).map_err(keep)?;
    retained::publish(&partial, &backup).map_err(keep)
}

pub(super) fn after_migrating(
    conn: &Connection,
    now: SystemTime,
    pace: &Progress,
) -> Result<(), FinishUpgradeError> {
    let Some(database) = file_database(conn) else {
        return Ok(());
    };
    retained::end_attempt(&database).map_err(|source| FinishUpgradeError::End {
        database: database.clone(),
        source,
    })?;
    retained::prune(&database, now).map_err(|source| FinishUpgradeError::Prune {
        database: database.clone(),
        source,
    })?;
    retained::record_missing_images(&database, pace).map_err(|source| FinishUpgradeError::Images {
        database: database.clone(),
        source,
    })
}

pub(crate) fn roll_back(
    database: &Path,
    supported: usize,
    at: Timestamp,
    pace: &Progress,
) -> Result<Rollback, RollbackError> {
    if !database.is_file() {
        return Ok(Rollback::Current);
    }
    let open = |source| RollbackError::Open {
        database: database.to_owned(),
        source: Box::new(source),
    };
    let mut conn = Connection::open(database).map_err(open)?;
    let found: usize = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(open)?;
    if found <= supported {
        return Ok(Rollback::Current);
    }
    let backup = retained::newest_backup_at_most(database, supported)
        .map_err(|source| RollbackError::Backups {
            database: database.to_owned(),
            source,
        })?
        .ok_or_else(|| RollbackError::NoBackup {
            database: database.to_owned(),
            found,
            supported,
        })?;
    let restored = match backup.kind {
        retained::Kind::Backup { version } => version,
        retained::Kind::Aside { .. } => unreachable!("invariant: only backups are restored"),
    };
    let aside = noticed_aside(&mut conn, database, found, restored, at, pace)?;
    conn.close().map_err(|(_, source)| open(source))?;
    let restore = |source| RollbackError::Restore {
        database: database.to_owned(),
        backup: backup.path.clone(),
        source,
    };
    let name = unicode(database).ok_or_else(|| RollbackError::NotUnicode {
        database: database.to_owned(),
    })?;
    ["-wal", "-shm"].into_iter().try_for_each(|suffix| {
        remove_if_present(Path::new(&format!("{name}{suffix}")))
            .map(drop)
            .map_err(restore)
    })?;
    std::fs::rename(&backup.path, database).map_err(restore)?;
    let settle = |source| RollbackError::Settle {
        database: database.to_owned(),
        backup: backup.path.clone(),
        source,
    };
    retained::discard(&backup.path).map_err(settle)?;
    retained::end_attempt(database).map_err(settle)?;
    Ok(Rollback::Restored {
        found,
        restored,
        aside,
    })
}

pub(crate) fn clear(
    database: &Path,
    at: Timestamp,
    pace: &Progress,
) -> Result<Cleared, RollbackError> {
    if !database.is_file() {
        return Ok(Cleared::Nothing);
    }
    let open = |source| RollbackError::Open {
        database: database.to_owned(),
        source: Box::new(source),
    };
    let mut conn = Connection::open(database).map_err(open)?;
    let found: usize = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(open)?;
    if found == 0 {
        return Ok(Cleared::Nothing);
    }
    let aside = noticed_aside(&mut conn, database, found, 0, at, pace)?;
    conn.close().map_err(|(_, source)| open(source))?;
    let name = unicode(database).ok_or_else(|| RollbackError::NotUnicode {
        database: database.to_owned(),
    })?;
    for suffix in ["-wal", "-shm", ""] {
        let path = PathBuf::from(format!("{name}{suffix}"));
        remove_if_present(&path).map_err(|source| RollbackError::Clear { path, source })?;
    }
    retained::end_attempt(database).map_err(|source| RollbackError::SettleClear {
        database: database.to_owned(),
        source,
    })?;
    Ok(Cleared::SetAside { found, aside })
}

fn noticed_aside(
    conn: &mut Connection,
    database: &Path,
    found: usize,
    restored: usize,
    at: Timestamp,
    pace: &Progress,
) -> Result<PathBuf, RollbackError> {
    let pending = retained::pending_notice(database).map_err(|source| RollbackError::Notice {
        database: database.to_owned(),
        source,
    })?;
    match pending {
        Some(notice)
            if usize::try_from(notice.found).is_ok_and(|noted| noted == found)
                && Path::new(&notice.set_aside).is_file() =>
        {
            Ok(PathBuf::from(notice.set_aside))
        }
        Some(_) | None => {
            let aside = set_aside(conn, database, found, pace)?;
            let notice = DatabaseRollback {
                found: u32::try_from(found).expect("invariant: schema versions fit in u32"),
                restored: u32::try_from(restored).expect("invariant: schema versions fit in u32"),
                set_aside: aside.display().to_string(),
                at,
            };
            retained::hold_notice(database, &notice).map_err(|source| RollbackError::Notice {
                database: database.to_owned(),
                source,
            })?;
            Ok(aside)
        }
    }
}

fn set_aside(
    conn: &mut Connection,
    database: &Path,
    found: usize,
    pace: &Progress,
) -> Result<PathBuf, RollbackError> {
    let aside = (0..usize::MAX)
        .map(|attempt| retained::aside(database, found, attempt))
        .find(|aside| !aside.exists() && !retained::partial(aside).exists())
        .expect("invariant: some rolled-back name is free");
    let partial = retained::partial(&aside);
    let failed = |source| RollbackError::SetAside {
        database: database.to_owned(),
        found,
        aside: aside.clone(),
        source: Box::new(source),
    };
    let target = unicode(&partial).ok_or_else(|| RollbackError::NotUnicode {
        database: database.to_owned(),
    })?;
    progress::watch(conn, Step::SetAside { found }, pace).map_err(failed)?;
    let copied = copy_into(conn, target);
    progress::unwatch(conn).map_err(failed)?;
    copied.map_err(failed)?;
    sync(&partial).map_err(|source| RollbackError::Restore {
        database: database.to_owned(),
        backup: partial.clone(),
        source,
    })?;
    let keep = |source| RollbackError::KeepAside {
        found,
        aside: aside.clone(),
        source,
    };
    retained::record_images(&partial, &aside, pace).map_err(keep)?;
    retained::publish(&partial, &aside).map_err(keep)?;
    Ok(aside)
}
