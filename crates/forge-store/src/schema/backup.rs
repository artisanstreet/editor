use rusqlite::Connection;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "../../tests/schema/backup.rs"]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("measure {database} and the free space beside it before backing up schema {version}")]
    Measure {
        database: String,
        version: usize,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the disk holding {database} is full: backing up schema {version} before the upgrade needs about {needed} bytes and {available} are free; free some space and start Forge again"
    )]
    NoSpace {
        database: String,
        version: usize,
        needed: u64,
        available: u64,
        #[source]
        source: Option<Box<rusqlite::Error>>,
    },
    #[error("copy schema {version} of {database} to {backup}")]
    Copy {
        database: String,
        version: usize,
        backup: String,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("store the schema {version} backup {backup}")]
    Store {
        version: usize,
        backup: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RollbackError {
    #[error("{database} is not a Unicode path, which SQLite needs to copy it")]
    NotUnicode { database: PathBuf },
    #[error("open {database} to read its schema")]
    Open {
        database: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error(
        "{database} uses schema {found}, newer than this Forge ({supported}), and there is no backup at {backup} to roll back to"
    )]
    NoBackup {
        database: PathBuf,
        found: usize,
        supported: usize,
        backup: PathBuf,
    },
    #[error("set aside schema {found} of {database} as {aside}")]
    SetAside {
        database: PathBuf,
        found: usize,
        aside: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("restore {backup} over {database}")]
    Restore {
        database: PathBuf,
        backup: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Rollback {
    Current,
    Restored {
        found: usize,
        restored: usize,
        aside: PathBuf,
    },
}

fn backup_name(database: &str, version: usize) -> String {
    format!("{database}.schema-{version}")
}

fn sync(path: &Path) -> std::io::Result<()> {
    std::fs::File::options().write(true).open(path)?.sync_all()
}

fn copy_into(conn: &Connection, target: &str) -> rusqlite::Result<()> {
    conn.execute(include_str!("../sql/backup/vacuum_into.sql"), [target])
        .map(drop)
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
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
    let folder = database.parent().unwrap_or_else(|| Path::new("."));
    Ok(Room {
        needed: std::fs::metadata(database)?.len() + wal_bytes,
        available: fs2::available_space(folder)?,
    })
}

fn disk_full(error: &rusqlite::Error) -> bool {
    error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull)
}

pub fn before_migrating(conn: &Connection, version: usize) -> Result<(), BackupError> {
    match conn.path() {
        Some(database) if !database.is_empty() && version > 0 => {
            let backup = backup_name(database, version);
            let partial = format!("{backup}.partial");
            let stored = |source| BackupError::Store {
                version,
                backup: backup.clone(),
                source,
            };
            let measure = || {
                room(Path::new(database)).map_err(|source| BackupError::Measure {
                    database: database.to_owned(),
                    version,
                    source,
                })
            };
            remove_if_present(Path::new(&partial)).map_err(stored)?;
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
            if let Err(source) = copy_into(conn, &partial) {
                remove_if_present(Path::new(&partial)).map_err(stored)?;
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
            sync(Path::new(&partial)).map_err(stored)?;
            std::fs::rename(&partial, &backup).map_err(stored)?;
            (1..version)
                .try_for_each(|older| remove_if_present(Path::new(&backup_name(database, older))))
                .map_err(stored)
        }
        Some(_) | None => Ok(()),
    }
}

pub fn roll_back(database: &Path, supported: usize) -> Result<Rollback, RollbackError> {
    if !database.is_file() {
        return Ok(Rollback::Current);
    }
    let open = |source| RollbackError::Open {
        database: database.to_owned(),
        source: Box::new(source),
    };
    let conn = Connection::open(database).map_err(open)?;
    let found: usize = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(open)?;
    if found <= supported {
        return Ok(Rollback::Current);
    }
    let name = database.to_str().ok_or_else(|| RollbackError::NotUnicode {
        database: database.to_owned(),
    })?;
    let backup = PathBuf::from(backup_name(name, supported));
    if !backup.is_file() {
        return Err(RollbackError::NoBackup {
            database: database.to_owned(),
            found,
            supported,
            backup,
        });
    }
    let aside = (0..usize::MAX)
        .map(|attempt| format!("{name}.rolled-back-from-{found}-{attempt}"))
        .find(|aside| !Path::new(aside).exists())
        .expect("invariant: some rolled-back name is free");
    let set_aside = |source| RollbackError::SetAside {
        database: database.to_owned(),
        found,
        aside: PathBuf::from(&aside),
        source: Box::new(source),
    };
    copy_into(&conn, &aside).map_err(set_aside)?;
    conn.close().map_err(|(_, source)| set_aside(source))?;
    let restore = |source| RollbackError::Restore {
        database: database.to_owned(),
        backup: backup.clone(),
        source,
    };
    sync(Path::new(&aside)).map_err(restore)?;
    ["-wal", "-shm"].into_iter().try_for_each(|suffix| {
        remove_if_present(Path::new(&format!("{name}{suffix}"))).map_err(restore)
    })?;
    std::fs::rename(&backup, database).map_err(restore)?;
    Ok(Rollback::Restored {
        found,
        restored: supported,
        aside: PathBuf::from(aside),
    })
}
