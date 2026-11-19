use std::io::Read as _;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ReadFileError {
    #[error("could not open {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is a directory", path.display())]
    Directory { path: PathBuf },
    #[error("{} is not a regular file", path.display())]
    NotRegular { path: PathBuf },
    #[error("{} is larger than {limit} bytes", path.display())]
    TooLarge { path: PathBuf, limit: u64 },
    #[error("could not read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub(super) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, ReadFileError> {
    let file = match open(path) {
        Ok(file) => file,
        Err(_) if path.is_dir() => {
            return Err(ReadFileError::Directory {
                path: path.to_path_buf(),
            });
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata().map_err(|source| ReadFileError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.is_dir() {
        return Err(ReadFileError::Directory {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(ReadFileError::NotRegular {
            path: path.to_path_buf(),
        });
    }
    if metadata.len() > limit {
        return Err(ReadFileError::TooLarge {
            path: path.to_path_buf(),
            limit,
        });
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| ReadFileError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > limit {
        return Err(ReadFileError::TooLarge {
            path: path.to_path_buf(),
            limit,
        });
    }
    Ok(bytes)
}

#[cfg(unix)]
fn open(path: &Path) -> Result<std::fs::File, ReadFileError> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let nonblocking = i32::try_from(rustix::fs::OFlags::NONBLOCK.bits())
        .expect("invariant: O_NONBLOCK fits in an open flag");
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nonblocking)
        .open(path)
        .map_err(|source| ReadFileError::Open {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(windows)]
fn open(path: &Path) -> Result<std::fs::File, ReadFileError> {
    use std::path::{Component, Prefix};
    let device = path.components().next().is_some_and(|component| {
        matches!(
            component,
            Component::Prefix(prefix) if matches!(
                prefix.kind(),
                Prefix::DeviceNS(_) | Prefix::Verbatim(_)
            )
        )
    });
    if device {
        return Err(ReadFileError::NotRegular {
            path: path.to_path_buf(),
        });
    }
    std::fs::File::open(path).map_err(|source| ReadFileError::Open {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
#[path = "../../tests/claudeai/files.rs"]
mod tests;
