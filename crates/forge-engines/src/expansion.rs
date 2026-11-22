use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub bytes: u64,
    pub files: u64,
    pub folders: usize,
    pub spare_space: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ExpansionError {
    #[error("unpacking {} would pass {limit} bytes", member.display())]
    Bytes { member: PathBuf, limit: u64 },
    #[error("unpacking {} would pass {limit} files", member.display())]
    Files { member: PathBuf, limit: u64 },
    #[error("unpacking {} would pass {limit} folders", member.display())]
    Folders { member: PathBuf, limit: usize },
    #[error(
        "unpacking {} needs {needed} bytes free in {}, which has {available}",
        member.display(),
        root.display()
    )]
    Space {
        member: PathBuf,
        root: PathBuf,
        needed: u64,
        available: u64,
    },
    #[error("checking the free space in {}", root.display())]
    FreeSpace {
        root: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) struct Expansion<'root> {
    root: &'root Path,
    limits: Limits,
    bytes: u64,
    files: u64,
    folders: BTreeSet<PathBuf>,
}

impl<'root> Expansion<'root> {
    pub(crate) const fn new(root: &'root Path, limits: Limits) -> Self {
        Self {
            root,
            limits,
            bytes: 0,
            files: 0,
            folders: BTreeSet::new(),
        }
    }

    pub(crate) fn admit_folder(&mut self, relative: &Path) -> Result<(), ExpansionError> {
        for folder in relative.ancestors() {
            if folder.as_os_str().is_empty() || !self.folders.insert(folder.to_path_buf()) {
                continue;
            }
            if self.folders.len() > self.limits.folders {
                return Err(ExpansionError::Folders {
                    member: relative.to_path_buf(),
                    limit: self.limits.folders,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn admit_file(&mut self, relative: &Path, size: u64) -> Result<(), ExpansionError> {
        if let Some(parent) = relative.parent() {
            self.admit_folder(parent)?;
        }
        self.files += 1;
        if self.files > self.limits.files {
            return Err(ExpansionError::Files {
                member: relative.to_path_buf(),
                limit: self.limits.files,
            });
        }
        self.bytes = self.bytes.saturating_add(size);
        if self.bytes > self.limits.bytes {
            return Err(ExpansionError::Bytes {
                member: relative.to_path_buf(),
                limit: self.limits.bytes,
            });
        }
        let available =
            fs2::available_space(self.root).map_err(|source| ExpansionError::FreeSpace {
                root: self.root.to_path_buf(),
                source,
            })?;
        let needed = size.saturating_add(self.limits.spare_space);
        if available < needed {
            return Err(ExpansionError::Space {
                member: relative.to_path_buf(),
                root: self.root.to_path_buf(),
                needed,
                available,
            });
        }
        Ok(())
    }
}
