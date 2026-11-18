use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Write as _,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum ReadConfigError {
    #[error("read {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode {path}")]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum WriteConfigError {
    #[error("{path} has no parent directory")]
    NoParent { path: PathBuf },
    #[error("create the directory {path}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("create a temporary file beside {path}")]
    CreateTemporary {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("restrict the temporary file beside {path}")]
    Restrict {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("encode {path}")]
    Encode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("write the temporary file beside {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("replace {path}")]
    Replace {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} already exists")]
    AlreadyExists { path: PathBuf },
    #[error("flush the directory of {path} to disk")]
    SyncDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateConfigError {
    #[error("lock {path}")]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read the current contents before changing them")]
    Read(#[source] ReadConfigError),
    #[error("save the change")]
    Write(#[source] WriteConfigError),
}

pub fn read<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> Result<T, ReadConfigError> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| ReadConfigError::Decode {
            path: path.to_owned(),
            source,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(source) => Err(ReadConfigError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

#[derive(Clone, Copy)]
enum Replacement {
    Overwrite,
    CreateOnly,
}

pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), WriteConfigError> {
    publish(path, value, Replacement::Overwrite)
}

pub(crate) fn create<T: Serialize>(path: &Path, value: &T) -> Result<(), WriteConfigError> {
    publish(path, value, Replacement::CreateOnly)
}

pub(crate) fn update<T, R>(
    path: &Path,
    change: impl FnOnce(&mut T) -> R,
) -> Result<(T, R), UpdateConfigError>
where
    T: Default + Serialize + for<'de> Deserialize<'de>,
{
    let _lock = lock(path)?;
    let mut value = read(path).map_err(UpdateConfigError::Read)?;
    let outcome = change(&mut value);
    write(path, &value).map_err(UpdateConfigError::Write)?;
    Ok((value, outcome))
}

fn lock(path: &Path) -> Result<File, UpdateConfigError> {
    let mut lock_path = path.as_os_str().to_owned();
    lock_path.push(".lock");
    let lock_path = PathBuf::from(lock_path);
    let failed = |source| UpdateConfigError::Lock {
        path: lock_path.clone(),
        source,
    };
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(failed)?;
    file.lock().map_err(failed)?;
    Ok(file)
}

fn publish<T: Serialize>(
    path: &Path,
    value: &T,
    replacement: Replacement,
) -> Result<(), WriteConfigError> {
    let path_buf = || path.to_owned();
    let parent = path
        .parent()
        .ok_or_else(|| WriteConfigError::NoParent { path: path_buf() })?;
    std::fs::create_dir_all(parent).map_err(|source| WriteConfigError::CreateDirectory {
        path: parent.to_owned(),
        source,
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| {
        WriteConfigError::CreateTemporary {
            path: path_buf(),
            source,
        }
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o600))
            .map_err(|source| WriteConfigError::Restrict {
                path: path_buf(),
                source,
            })?;
    }
    serde_json::to_writer_pretty(temporary.as_file_mut(), value).map_err(|source| {
        WriteConfigError::Encode {
            path: path_buf(),
            source,
        }
    })?;
    temporary
        .flush()
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| WriteConfigError::Write {
            path: path_buf(),
            source,
        })?;
    let persisted = match replacement {
        Replacement::Overwrite => temporary.persist(path),
        Replacement::CreateOnly => temporary.persist_noclobber(path),
    };
    match persisted {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(WriteConfigError::AlreadyExists { path: path_buf() });
        }
        Err(error) => {
            return Err(WriteConfigError::Replace {
                path: path_buf(),
                source: error.error,
            });
        }
    }
    sync_directory(parent, path)
}

#[cfg(unix)]
fn sync_directory(parent: &Path, path: &Path) -> Result<(), WriteConfigError> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| WriteConfigError::SyncDirectory {
            path: path.to_owned(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_directory(_parent: &Path, path: &Path) -> Result<(), WriteConfigError> {
    File::options()
        .write(true)
        .open(path)
        .and_then(|replaced| replaced.sync_all())
        .map_err(|source| WriteConfigError::SyncDirectory {
            path: path.to_owned(),
            source,
        })
}
