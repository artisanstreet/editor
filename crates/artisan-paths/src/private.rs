use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, thiserror::Error)]
pub enum PrivateFileError {
    #[error("creating the private folder {path}")]
    CreateFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("restricting {path} to this user")]
    Restrict {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing the private file {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("replacing the private file {path}")]
    Replace {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the private file {path} has no folder")]
    NoFolder { path: PathBuf },
}

pub fn create_folder(path: &Path) -> Result<(), PrivateFileError> {
    std::fs::create_dir_all(path).map_err(|source| PrivateFileError::CreateFolder {
        path: path.to_path_buf(),
        source,
    })?;
    restrict(path, FOLDER_MODE)
}

pub fn restrict_file(path: &Path) -> Result<(), PrivateFileError> {
    restrict(path, FILE_MODE)
}

pub fn write_file(path: &Path, contents: &[u8]) -> Result<(), PrivateFileError> {
    let folder = path
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .ok_or_else(|| PrivateFileError::NoFolder {
            path: path.to_path_buf(),
        })?;
    create_folder(folder)?;
    let temporary = temporary_path(path);
    let written = write_new(&temporary, contents).and_then(|()| {
        std::fs::rename(&temporary, path).map_err(|source| PrivateFileError::Replace {
            path: path.to_path_buf(),
            source,
        })
    });
    if written.is_err() {
        match std::fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(PrivateFileError::Write {
                    path: temporary,
                    source: error,
                });
            }
        }
    }
    written
}

fn write_new(path: &Path, contents: &[u8]) -> Result<(), PrivateFileError> {
    let failed = |source| PrivateFileError::Write {
        path: path.to_path_buf(),
        source,
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, FILE_MODE);
    let mut file = options.open(path).map_err(failed)?;
    restrict(path, FILE_MODE)?;
    file.write_all(contents).map_err(failed)?;
    file.sync_all().map_err(failed)
}

fn temporary_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

const FOLDER_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<(), PrivateFileError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|source| {
        PrivateFileError::Restrict {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "Windows keeps per-user ACLs on the profile folder Artisan's data lives in"
)]
fn restrict(_path: &Path, _mode: u32) -> Result<(), PrivateFileError> {
    Ok(())
}

#[cfg(test)]
#[path = "../tests/paths/private.rs"]
mod tests;
