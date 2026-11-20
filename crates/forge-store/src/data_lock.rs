use std::path::{Path, PathBuf};

const LOCK_FILE: &str = "forge.lock";

#[derive(Debug, thiserror::Error)]
pub enum LockDataError {
    #[error("open the data folder lock {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("another Forge is using {}", dir.display())]
    Held {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("lock {}", path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug)]
pub struct DataLock {
    _file: std::fs::File,
    dir: PathBuf,
}

impl DataLock {
    pub fn acquire(dir: &Path) -> Result<Self, LockDataError> {
        use fs2::FileExt as _;
        let (path, file) = open(dir)?;
        file.try_lock_exclusive().map_err(|source| {
            if contended(&source) {
                LockDataError::Held {
                    dir: dir.to_owned(),
                    source,
                }
            } else {
                LockDataError::Lock { path, source }
            }
        })?;
        Ok(Self {
            _file: file,
            dir: dir.to_owned(),
        })
    }

    pub fn acquire_waiting(dir: &Path, waiting: impl FnOnce()) -> Result<Self, LockDataError> {
        use fs2::FileExt as _;
        let (path, file) = open(dir)?;
        match file.try_lock_exclusive() {
            Ok(()) => {}
            Err(held) if contended(&held) => {
                waiting();
                file.lock_exclusive()
                    .map_err(|source| LockDataError::Lock { path, source })?;
            }
            Err(source) => return Err(LockDataError::Lock { path, source }),
        }
        Ok(Self {
            _file: file,
            dir: dir.to_owned(),
        })
    }

    #[must_use]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }
}

fn open(dir: &Path) -> Result<(PathBuf, std::fs::File), LockDataError> {
    let path = dir.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| LockDataError::Open {
            path: path.clone(),
            source,
        })?;
    Ok((path, file))
}

fn contended(error: &std::io::Error) -> bool {
    error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}
