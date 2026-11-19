use crate::progress::{self, Progress, Step};
use artisan_api::{BlobHash, DatabaseRollback};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const BACKUPS_KEPT: usize = 2;
const BACKUP_DAYS: u64 = 30;
const ASIDES_KEPT: usize = 2;
const MANIFEST: &str = "blobs";
const ATTEMPT: &str = "upgrading";
const NOTICE: &str = "rollback";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Backup { version: usize },
    Aside { found: usize, attempt: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Retained {
    pub(crate) kind: Kind,
    pub(crate) path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum RetainedError {
    #[error("list the databases kept beside {}", database.display())]
    List {
        database: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read the age of {}", path.display())]
    Age {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open {} to list the images it uses", database.display())]
    OpenKept {
        database: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("list the images {} uses", database.display())]
    ScanKept {
        database: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} does not hold a schema version", path.display())]
    Attempt { path: PathBuf },
    #[error("{} does not hold a rollback notice", path.display())]
    Notice {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{} holds {line:?}, which is not an image hash", path.display())]
    Manifest { path: PathBuf, line: String },
}

fn sibling(database: &Path, suffix: &str) -> PathBuf {
    let mut name = database.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

pub(crate) fn backup(database: &Path, version: usize) -> PathBuf {
    sibling(database, &format!("schema-{version}"))
}

pub(crate) fn aside(database: &Path, found: usize, attempt: usize) -> PathBuf {
    sibling(database, &format!("rolled-back-from-{found}-{attempt}"))
}

pub(crate) fn partial(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".partial");
    PathBuf::from(name)
}

fn manifest(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(MANIFEST);
    PathBuf::from(name)
}

fn kind_of(suffix: &str) -> Option<Kind> {
    if let Some(version) = suffix.strip_prefix("schema-") {
        return version.parse().ok().map(|version| Kind::Backup { version });
    }
    let (found, attempt) = suffix.strip_prefix("rolled-back-from-")?.split_once('-')?;
    Some(Kind::Aside {
        found: found.parse().ok()?,
        attempt: attempt.parse().ok()?,
    })
}

fn folder(database: &Path) -> &Path {
    database
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn list(database: &Path) -> Result<Vec<Retained>, RetainedError> {
    let listed = |source| RetainedError::List {
        database: database.to_owned(),
        source,
    };
    let name = database
        .file_name()
        .expect("invariant: the database path ends in its file name");
    let mut prefix = name.to_owned();
    prefix.push(".");
    let mut kept = Vec::new();
    for entry in std::fs::read_dir(folder(database)).map_err(listed)? {
        let entry = entry.map_err(listed)?;
        let file_name = entry.file_name();
        let kind = file_name
            .as_encoded_bytes()
            .strip_prefix(prefix.as_encoded_bytes())
            .and_then(|suffix| std::str::from_utf8(suffix).ok())
            .and_then(kind_of);
        if let Some(kind) = kind
            && entry.file_type().map_err(listed)?.is_file()
        {
            kept.push(Retained {
                kind,
                path: entry.path(),
            });
        }
    }
    kept.sort_by_key(|retained| retained.kind);
    Ok(kept)
}

pub(crate) fn newest_backup_at_most(
    database: &Path,
    supported: usize,
) -> Result<Option<Retained>, RetainedError> {
    Ok(list(database)?
        .into_iter()
        .filter(
            |retained| matches!(retained.kind, Kind::Backup { version } if version <= supported),
        )
        .max_by_key(|retained| retained.kind))
}

fn remove_if_present(path: &Path) -> Result<(), RetainedError> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RetainedError::Remove {
            path: path.to_owned(),
            source,
        }),
        Ok(()) => Ok(()),
    }
}

pub(crate) fn discard(path: &Path) -> Result<(), RetainedError> {
    remove_if_present(path)?;
    remove_if_present(&manifest(path))
}

#[cfg(unix)]
fn sync_folder(database: &Path) -> Result<(), RetainedError> {
    let folder = folder(database);
    std::fs::File::open(folder)
        .and_then(|folder| folder.sync_all())
        .map_err(|source| RetainedError::Write {
            path: folder.to_owned(),
            source,
        })
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "Windows has no folder fsync and NTFS journals renames; callers handle one signature"
)]
fn sync_folder(_database: &Path) -> Result<(), RetainedError> {
    Ok(())
}

fn write_durably(path: &Path, contents: &[u8]) -> Result<(), RetainedError> {
    let written = |source| RetainedError::Write {
        path: path.to_owned(),
        source,
    };
    let staged = partial(path);
    let mut file = std::fs::File::create(&staged).map_err(written)?;
    file.write_all(contents).map_err(written)?;
    file.sync_all().map_err(written)?;
    drop(file);
    std::fs::rename(&staged, path).map_err(written)
}

fn read_if_present(path: &Path) -> Result<Option<String>, RetainedError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(RetainedError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

pub(crate) fn attempt(database: &Path) -> Result<Option<usize>, RetainedError> {
    let path = sibling(database, ATTEMPT);
    read_if_present(&path)?
        .map(|text| {
            text.trim()
                .parse()
                .map_err(|_not_a_version| RetainedError::Attempt { path: path.clone() })
        })
        .transpose()
}

pub(crate) fn begin_attempt(database: &Path, from: usize) -> Result<(), RetainedError> {
    write_durably(&sibling(database, ATTEMPT), format!("{from}\n").as_bytes())?;
    sync_folder(database)
}

pub(crate) fn end_attempt(database: &Path) -> Result<(), RetainedError> {
    remove_if_present(&sibling(database, ATTEMPT))?;
    sync_folder(database)
}

pub(crate) fn pending_notice(database: &Path) -> Result<Option<DatabaseRollback>, RetainedError> {
    let path = sibling(database, NOTICE);
    read_if_present(&path)?
        .map(|text| {
            serde_json::from_str(&text).map_err(|source| RetainedError::Notice {
                path: path.clone(),
                source,
            })
        })
        .transpose()
}

pub(crate) fn hold_notice(database: &Path, notice: &DatabaseRollback) -> Result<(), RetainedError> {
    let text = serde_json::to_vec(notice).expect("invariant: a rollback notice encodes as JSON");
    write_durably(&sibling(database, NOTICE), &text)?;
    sync_folder(database)
}

pub(crate) fn release_notice(database: &Path) -> Result<(), RetainedError> {
    remove_if_present(&sibling(database, NOTICE))?;
    sync_folder(database)
}

pub(crate) fn publish(partial: &Path, kept: &Path) -> Result<(), RetainedError> {
    std::fs::rename(partial, kept).map_err(|source| RetainedError::Write {
        path: kept.to_owned(),
        source,
    })?;
    sync_folder(kept)
}

pub(crate) fn record_images(
    reading: &Path,
    kept: &Path,
    pace: &Progress,
) -> Result<(), RetainedError> {
    let opened = |source| RetainedError::OpenKept {
        database: reading.to_owned(),
        source: Box::new(source),
    };
    let scanned = |source| RetainedError::ScanKept {
        database: reading.to_owned(),
        source: Box::new(source),
    };
    let mut conn = Connection::open_with_flags(
        reading,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(opened)?;
    progress::watch(&mut conn, Step::ListImages, pace).map_err(scanned)?;
    let hashes = image_hashes(&conn).map_err(scanned);
    progress::unwatch(&mut conn).map_err(scanned)?;
    let mut text = String::new();
    for hash in hashes? {
        text.push_str(&hash);
        text.push('\n');
    }
    write_durably(&manifest(kept), text.as_bytes())
}

fn image_hashes(conn: &Connection) -> rusqlite::Result<std::collections::BTreeSet<String>> {
    let tables: Vec<String> = conn
        .prepare(include_str!("sql/retained/tables_with_bodies.sql"))?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut hashes = std::collections::BTreeSet::new();
    for table in tables {
        let quoted = table.replace('"', "\"\"");
        let sql = include_str!("sql/retained/hashes.sql").replace("__TABLE__", &quoted);
        let mut select = conn.prepare(&sql)?;
        for hash in select.query_map([], |row| row.get::<_, String>(0))? {
            let hash = BlobHash(hash?);
            if hash.is_valid() {
                hashes.insert(hash.0);
            }
        }
    }
    Ok(hashes)
}

pub(crate) fn record_missing_images(database: &Path, pace: &Progress) -> Result<(), RetainedError> {
    for retained in list(database)? {
        if manifest(&retained.path).is_file() {
            continue;
        }
        match record_images(&retained.path, &retained.path, pace) {
            Ok(()) => {}
            Err(error @ (RetainedError::OpenKept { .. } | RetainedError::ScanKept { .. })) => {
                tracing::warn!(
                    kept = %retained.path.display(),
                    error = %diagnostics::chain(&error),
                    "a kept database doesn't read, so the images only it uses aren't protected"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn kept_images(database: &Path) -> Result<HashSet<String>, RetainedError> {
    let mut hashes = HashSet::new();
    for retained in list(database)? {
        let path = manifest(&retained.path);
        let Some(text) = read_if_present(&path)? else {
            continue;
        };
        for line in text.lines() {
            let hash = BlobHash(line.to_owned());
            if !hash.is_valid() {
                return Err(RetainedError::Manifest {
                    path,
                    line: line.to_owned(),
                });
            }
            hashes.insert(hash.0);
        }
    }
    Ok(hashes)
}

fn modified(path: &Path) -> Result<SystemTime, RetainedError> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|source| RetainedError::Age {
            path: path.to_owned(),
            source,
        })
}

fn older_than(path: &Path, limit: Duration, now: SystemTime) -> Result<bool, RetainedError> {
    Ok(match now.duration_since(modified(path)?) {
        Ok(age) => age > limit,
        Err(_written_after_now) => false,
    })
}

pub(crate) fn prune(database: &Path, now: SystemTime) -> Result<(), RetainedError> {
    let kept = list(database)?;
    let limit = Duration::from_hours(BACKUP_DAYS * 24);
    let mut backups: Vec<&Retained> = kept
        .iter()
        .filter(|retained| matches!(retained.kind, Kind::Backup { .. }))
        .collect();
    backups.reverse();
    for (rank, backup) in backups.into_iter().enumerate() {
        if rank >= BACKUPS_KEPT || older_than(&backup.path, limit, now)? {
            tracing::info!(backup = %backup.path.display(), "removed an old database backup");
            discard(&backup.path)?;
        }
    }
    let mut asides: Vec<(SystemTime, &Retained)> = Vec::new();
    for retained in kept
        .iter()
        .filter(|retained| matches!(retained.kind, Kind::Aside { .. }))
    {
        asides.push((modified(&retained.path)?, retained));
    }
    asides.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.kind.cmp(&left.1.kind)));
    for (_, aside) in asides.into_iter().skip(ASIDES_KEPT) {
        tracing::info!(aside = %aside.path.display(), "removed an old set-aside database");
        discard(&aside.path)?;
    }
    sync_folder(database)
}

#[cfg(test)]
#[path = "../tests/retained/unit.rs"]
mod tests;
