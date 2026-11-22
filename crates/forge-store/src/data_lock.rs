use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

#[derive(Debug, thiserror::Error)]
pub enum WaitForDataError {
    #[error(transparent)]
    Take(#[from] LockDataError),
    #[error("another Forge still holds {} after {} s", dir.display(), waited.as_secs())]
    StillHeld { dir: PathBuf, waited: Duration },
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

    pub fn acquire_waiting(
        dir: &Path,
        patience: &Patience<impl PatienceClock>,
        waiting: impl FnOnce(),
    ) -> Result<Self, WaitForDataError> {
        use fs2::FileExt as _;
        let (path, file) = open(dir)?;
        let try_lock = |file: &std::fs::File| match file.try_lock_exclusive() {
            Ok(()) => Ok(true),
            Err(held) if contended(&held) => Ok(false),
            Err(source) => Err(LockDataError::Lock {
                path: path.clone(),
                source,
            }),
        };
        if !try_lock(&file)? {
            waiting();
            let started = patience.clock.now();
            loop {
                let waited = patience.clock.now().saturating_duration_since(started);
                if waited >= patience.deadline {
                    return Err(WaitForDataError::StillHeld {
                        dir: dir.to_owned(),
                        waited,
                    });
                }
                patience
                    .clock
                    .pause(patience.retry.min(patience.deadline.saturating_sub(waited)));
                if try_lock(&file)? {
                    break;
                }
            }
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

pub trait PatienceClock {
    fn now(&self) -> Instant;
    fn pause(&self, pause: Duration);
}

#[derive(Clone, Copy, Debug)]
pub struct SystemClock;

impl PatienceClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn pause(&self, pause: Duration) {
        std::thread::sleep(pause);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Patience<C> {
    deadline: Duration,
    retry: Duration,
    clock: C,
}

impl Patience<SystemClock> {
    #[must_use]
    pub const fn system(deadline: Duration) -> Self {
        Self {
            deadline,
            retry: Duration::from_millis(100),
            clock: SystemClock,
        }
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

#[cfg(test)]
#[path = "../tests/data_lock/unit.rs"]
mod tests;
