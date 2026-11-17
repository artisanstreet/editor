use file_rotate::{ContentLimit, FileRotate, compression::Compression, suffix::AppendCount};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Stderr,
    File(PathBuf),
}

pub type Writer = FileRotate<AppendCount>;

#[derive(Clone, Copy, Debug)]
pub struct Limit {
    pub bytes: usize,
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
}

pub fn open(path: &Path, limit: Limit) -> Result<Writer, OpenLogError> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|source| OpenLogError::CreateFolder {
            path: folder.to_owned(),
            source,
        })?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| OpenLogError::Open {
            path: path.to_owned(),
            source,
        })?;
    Ok(FileRotate::new(
        path,
        AppendCount::new(limit.archives),
        ContentLimit::Bytes(limit.bytes),
        Compression::None,
        None,
    ))
}
