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
        file.try_lock_exclusive().map_err(|source| {
            if source.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
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

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}
