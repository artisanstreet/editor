use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskFault {
    WriteError,
    Full,
}

impl DiskFault {
    fn error(self) -> rusqlite::Error {
        let (code, message) = match self {
            Self::WriteError => (rusqlite::ffi::SQLITE_IOERR_WRITE, "disk I/O error"),
            Self::Full => (rusqlite::ffi::SQLITE_FULL, "database or disk is full"),
        };
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), Some(message.to_owned()))
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    fault: DiskFault,
    commits: u32,
}

static PENDING: Mutex<Option<HashMap<PathBuf, Pending>>> = Mutex::new(None);

pub fn fail_commits(data_dir: &Path, fault: DiskFault, commits: NonZeroU32) {
    PENDING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(
            database(data_dir),
            Pending {
                fault,
                commits: commits.get(),
            },
        );
}

pub fn heal(data_dir: &Path) {
    if let Some(pending) = PENDING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        pending.remove(&database(data_dir));
    }
}

fn database(data_dir: &Path) -> PathBuf {
    data_dir.join("forge.sqlite3")
}

pub(crate) fn before_commit(database: Option<&str>) -> Result<(), rusqlite::Error> {
    let Some(database) = database else {
        return Ok(());
    };
    let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(faults) = pending.as_mut() else {
        return Ok(());
    };
    let path = Path::new(database);
    let Some(next) = faults.get_mut(path) else {
        return Ok(());
    };
    let fault = next.fault;
    next.commits -= 1;
    if next.commits == 0 {
        faults.remove(path);
    }
    Err(fault.error())
}
