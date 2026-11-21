use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing_subscriber::fmt::time::{FormatTime as _, SystemTime};

const RETRY_SHARE: u64 = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Stderr,
    File(PathBuf),
}

#[derive(Clone, Copy, Debug)]
pub struct Limit {
    pub bytes: u64,
    pub archives: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenLogError {
    #[error("create the log folder {path}")]
    CreateFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open the log file {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open the log's rotation lock {path}")]
    OpenLock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RotateError {
    #[error("measure the log file {path}")]
    Measure {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("take the log's rotation lock {path}")]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("release the log's rotation lock {path}")]
    Unlock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("look for the log archive {path}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove the oldest log archive {path}")]
    Drop {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("move the log {from} to {to}")]
    Shift {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("reopen the log file {path}")]
    Reopen {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug)]
enum Rotation {
    Working,
    Failing { appended: u64, reported: String },
}

#[derive(Debug)]
pub struct Writer {
    path: PathBuf,
    lock_path: PathBuf,
    lock: File,
    file: File,
    limit: Limit,
    rotation: Rotation,
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

fn archive(path: &Path, number: usize) -> PathBuf {
    suffixed(path, &format!(".{number}"))
}

fn append(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn absent_is_fine(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

pub fn open(path: &Path, limit: Limit) -> Result<Writer, OpenLogError> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|source| OpenLogError::CreateFolder {
            path: folder.to_owned(),
            source,
        })?;
    }
    let lock_path = suffixed(path, ".lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|source| OpenLogError::OpenLock {
            path: lock_path.clone(),
            source,
        })?;
    let file = append(path).map_err(|source| OpenLogError::Open {
        path: path.to_owned(),
        source,
    })?;
    Ok(Writer {
        path: path.to_owned(),
        lock_path,
        lock,
        file,
        limit,
        rotation: Rotation::Working,
    })
}

impl Writer {
    fn rotate_if_full(&mut self) -> Result<(), RotateError> {
        let size = self
            .file
            .metadata()
            .map_err(|source| RotateError::Measure {
                path: self.path.clone(),
                source,
            })?
            .len();
        if size < self.limit.bytes {
            return Ok(());
        }
        self.lock.lock().map_err(|source| RotateError::Lock {
            path: self.lock_path.clone(),
            source,
        })?;
        let rotated = self.rotate_locked();
        let unlocked = self.lock.unlock().map_err(|source| RotateError::Unlock {
            path: self.lock_path.clone(),
            source,
        });
        rotated.and(unlocked)
    }

    fn rotate_locked(&mut self) -> Result<(), RotateError> {
        let shared = match std::fs::metadata(&self.path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(source) => {
                return Err(RotateError::Measure {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        if shared >= self.limit.bytes {
            self.shift()?;
        }
        self.file = append(&self.path).map_err(|source| RotateError::Reopen {
            path: self.path.clone(),
            source,
        })?;
        Ok(())
    }

    fn shift(&self) -> Result<(), RotateError> {
        let last = match self.free_archive()? {
            Some(number) => number,
            None => self.drop_oldest()?,
        };
        (1..=last).rev().try_for_each(|number| {
            let from = match number {
                1 => self.path.clone(),
                newer => archive(&self.path, newer - 1),
            };
            let to = archive(&self.path, number);
            absent_is_fine(std::fs::rename(&from, &to)).map_err(|source| RotateError::Shift {
                from,
                to,
                source,
            })
        })
    }

    fn drop_oldest(&self) -> Result<usize, RotateError> {
        let oldest = match self.limit.archives {
            0 => self.path.clone(),
            archives => archive(&self.path, archives),
        };
        absent_is_fine(std::fs::remove_file(&oldest)).map_err(|source| RotateError::Drop {
            path: oldest,
            source,
        })?;
        Ok(self.limit.archives)
    }

    fn retry_due(&self) -> bool {
        match self.rotation {
            Rotation::Working => true,
            Rotation::Failing { appended, .. } => appended >= retry_after(self.limit),
        }
    }

    fn failed(&mut self, error: &RotateError) -> std::io::Result<()> {
        let failure = crate::chain(error);
        let known =
            matches!(&self.rotation, Rotation::Failing { reported, .. } if *reported == failure);
        let line = (!known).then(|| report_line(&failure));
        self.rotation = Rotation::Failing {
            appended: 0,
            reported: failure,
        };
        match line {
            Some(line) => self.file.write_all(line.as_bytes()),
            None => Ok(()),
        }
    }

    fn free_archive(&self) -> Result<Option<usize>, RotateError> {
        for number in 1..=self.limit.archives {
            let path = archive(&self.path, number);
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Some(number));
                }
                Err(source) => return Err(RotateError::Inspect { path, source }),
            }
        }
        Ok(None)
    }
}

fn report_line(failure: &str) -> String {
    let mut line = String::new();
    SystemTime
        .format_time(&mut tracing_subscriber::fmt::format::Writer::new(&mut line))
        .expect("invariant: formatting a timestamp into a String cannot fail");
    writeln!(
        line,
        " ERROR {}: rotating the log failed, so it keeps growing until a rotation succeeds: {failure}",
        module_path!(),
    )
    .expect("invariant: formatting into a String cannot fail");
    line
}

fn retry_after(limit: Limit) -> u64 {
    (limit.bytes / RETRY_SHARE).max(1)
}

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.retry_due() {
            match self.rotate_if_full() {
                Ok(()) => self.rotation = Rotation::Working,
                Err(error) => self.failed(&error)?,
            }
        }
        let written = self.file.write(buf)?;
        if let Rotation::Failing { appended, .. } = &mut self.rotation {
            *appended = appended.saturating_add(
                u64::try_from(written).expect("invariant: a write's length fits in u64"),
            );
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
