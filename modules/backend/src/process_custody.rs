#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

//! Whole-process custody for an explicitly selected Forge lock file.
//!
//! Process assembly acquires this guard before opening Forge storage and keeps
//! it alive until after the application shuts down. The lock file is only an
//! operating-system lock carrier: this module never writes ownership data to
//! it and never removes it when custody ends.
//!
//! Callers must select the lock path inside a stable, application-owned parent
//! namespace that is not writable by an untrusted actor while Forge runs. The
//! acquisition checks are fail-closed for the filesystem shape they observe;
//! they do not make that path namespace immutable against a later rename or
//! reparse-point replacement.

use std::{
    collections::HashSet,
    error::Error,
    fmt,
    fs::{self, File, Metadata, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use fs2::FileExt;

/// Windows' reparse-point attribute, exposed here without requiring unsafe
/// Win32 bindings. `symlink_metadata` preserves this attribute for links and
/// junctions, so it can be rejected before the path is opened.
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// The Windows flag that makes `CreateFile` open a reparse point itself
/// instead of following it. Normal files continue to open normally. It
/// protects the final-entry open from following a reparse point, but does not
/// make the parent namespace immutable across separate path operations.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// A failure while acquiring whole-process Forge custody.
///
/// The path-bearing variants identify the rejected filesystem shape without
/// exposing file contents. I/O sources remain available through
/// [`std::error::Error::source`], while [`Display`](fmt::Display) reports only
/// a bounded operation and the explicit path.
#[derive(Debug)]
pub enum ForgeProcessCustodyError {
    /// The requested lock path has no inspectable parent.
    ParentMissing {
        /// The missing parent path.
        path: PathBuf,
    },
    /// A parent component exists but is not a directory.
    ParentNotDirectory {
        /// The non-directory parent component.
        path: PathBuf,
    },
    /// A parent component is a symbolic link.
    ParentSymlink {
        /// The symbolic-link parent component.
        path: PathBuf,
    },
    /// A parent component is a Windows reparse point.
    ParentReparsePoint {
        /// The reparse-point parent component.
        path: PathBuf,
    },
    /// A concurrent creator removed the lock file before it could be opened.
    LockPathMissing {
        /// The lock path that disappeared after creation raced.
        path: PathBuf,
    },
    /// The final path exists but is not a regular file.
    LockPathNotRegular {
        /// The rejected final path.
        path: PathBuf,
    },
    /// The final path is a symbolic link.
    LockPathSymlink {
        /// The rejected symbolic-link path.
        path: PathBuf,
    },
    /// The final path is a Windows reparse point.
    LockPathReparsePoint {
        /// The rejected reparse-point path.
        path: PathBuf,
    },
    /// Inspection of a parent component failed.
    InspectParent {
        /// The parent component that could not be inspected.
        path: PathBuf,
        /// The underlying inspection failure.
        source: io::Error,
    },
    /// Inspection of the final path failed.
    InspectLockPath {
        /// The lock path that could not be inspected.
        path: PathBuf,
        /// The underlying inspection failure.
        source: io::Error,
    },
    /// Creation of a missing lock file failed.
    CreateLockFile {
        /// The lock path whose creation failed.
        path: PathBuf,
        /// The underlying creation failure.
        source: io::Error,
    },
    /// Opening the exact regular lock file read/write failed.
    OpenLockFile {
        /// The lock path whose open failed.
        path: PathBuf,
        /// The underlying open failure.
        source: io::Error,
    },
    /// The operating system rejected the exclusive lock for a reason other
    /// than contention.
    Lock {
        /// The lock path whose OS lock failed.
        path: PathBuf,
        /// The underlying locking failure.
        source: io::Error,
    },
    /// Another process or this process already owns custody for the path.
    Contended {
        /// The contended lock path.
        path: PathBuf,
    },
}

impl ForgeProcessCustodyError {
    /// Returns whether the acquisition failed because custody is occupied.
    #[must_use]
    pub const fn is_contention(&self) -> bool {
        matches!(self, Self::Contended { .. })
    }
}

impl fmt::Display for ForgeProcessCustodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParentMissing { path } => {
                write!(
                    formatter,
                    "Forge process custody parent is missing: {}",
                    path.display()
                )
            }
            Self::ParentNotDirectory { path } => write!(
                formatter,
                "Forge process custody parent is not a directory: {}",
                path.display()
            ),
            Self::ParentSymlink { path } => write!(
                formatter,
                "Forge process custody parent is a symbolic link: {}",
                path.display()
            ),
            Self::ParentReparsePoint { path } => write!(
                formatter,
                "Forge process custody parent is a reparse point: {}",
                path.display()
            ),
            Self::LockPathMissing { path } => write!(
                formatter,
                "Forge process custody lock path disappeared during creation: {}",
                path.display()
            ),
            Self::LockPathNotRegular { path } => write!(
                formatter,
                "Forge process custody lock path is not a regular file: {}",
                path.display()
            ),
            Self::LockPathSymlink { path } => write!(
                formatter,
                "Forge process custody lock path is a symbolic link: {}",
                path.display()
            ),
            Self::LockPathReparsePoint { path } => write!(
                formatter,
                "Forge process custody lock path is a reparse point: {}",
                path.display()
            ),
            Self::InspectParent { path, .. } => write!(
                formatter,
                "failed to inspect Forge process custody parent: {}",
                path.display()
            ),
            Self::InspectLockPath { path, .. } => write!(
                formatter,
                "failed to inspect Forge process custody lock path: {}",
                path.display()
            ),
            Self::CreateLockFile { path, .. } => write!(
                formatter,
                "failed to create Forge process custody lock file: {}",
                path.display()
            ),
            Self::OpenLockFile { path, .. } => write!(
                formatter,
                "failed to open Forge process custody lock file: {}",
                path.display()
            ),
            Self::Lock { path, .. } => {
                write!(
                    formatter,
                    "failed to lock Forge process custody file: {}",
                    path.display()
                )
            }
            Self::Contended { path } => write!(
                formatter,
                "Forge process custody is already held for lock path: {}",
                path.display()
            ),
        }
    }
}

impl Error for ForgeProcessCustodyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InspectParent { source, .. }
            | Self::InspectLockPath { source, .. }
            | Self::CreateLockFile { source, .. }
            | Self::OpenLockFile { source, .. }
            | Self::Lock { source, .. } => Some(source),
            Self::ParentMissing { .. }
            | Self::ParentNotDirectory { .. }
            | Self::ParentSymlink { .. }
            | Self::ParentReparsePoint { .. }
            | Self::LockPathMissing { .. }
            | Self::LockPathNotRegular { .. }
            | Self::LockPathSymlink { .. }
            | Self::LockPathReparsePoint { .. }
            | Self::Contended { .. } => None,
        }
    }
}

/// Owning whole-process custody for one explicitly selected lock file.
///
/// The guard is intentionally neither [`Default`](Default) nor [`Clone`]. It
/// owns the exact read/write [`File`] on which the exclusive OS lock lives;
/// dropping the guard drops that file and therefore releases custody. The
/// lock file itself is never unlinked.
#[must_use = "Forge process custody must remain alive through application shutdown"]
pub struct ForgeProcessCustody {
    file: File,
    lock_path: PathBuf,
}

impl ForgeProcessCustody {
    /// Acquires exclusive whole-process custody for an explicit lock-file path.
    ///
    /// The parent directory and every existing ancestor are inspected without
    /// following links. A missing final file is created atomically with
    /// owner-only Unix permissions; a concurrent creator is re-inspected as a
    /// regular file before it is opened. The returned guard retains the exact
    /// read/write file used for the single nonblocking lock attempt.
    ///
    /// The caller is responsible for selecting a stable, application-owned
    /// parent namespace that untrusted actors cannot modify while Forge runs.
    /// These checks do not freeze the namespace against an adversarial rename
    /// or reparse-point replacement between path operations or after
    /// acquisition; custody is held by the retained file descriptor.
    ///
    /// # Errors
    ///
    /// Returns a path-shape error for missing, non-directory, symbolic-link,
    /// reparse-point, or non-regular entries; an operation-specific I/O error
    /// for inspection, creation, opening, or locking failures; or
    /// [`ForgeProcessCustodyError::Contended`] immediately when another owner
    /// holds the lock.
    pub fn acquire(lock_path: impl AsRef<Path>) -> Result<Self, ForgeProcessCustodyError> {
        let lock_path = lock_path.as_ref().to_path_buf();
        let parent = lock_path
            .parent()
            .ok_or_else(|| ForgeProcessCustodyError::ParentMissing {
                path: lock_path.clone(),
            })?;

        validate_parent_chain(parent)?;
        let file = open_or_create_lock_file(&lock_path)?;
        validate_open_file(&lock_path, &file)?;

        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                if !claim_process_path(&lock_path) {
                    return Err(ForgeProcessCustodyError::Contended { path: lock_path });
                }
                Ok(Self { file, lock_path })
            }
            Err(source) if is_lock_contention(&source) => {
                Err(ForgeProcessCustodyError::Contended { path: lock_path })
            }
            Err(source) => Err(ForgeProcessCustodyError::Lock {
                path: lock_path,
                source,
            }),
        }
    }
}

impl Drop for ForgeProcessCustody {
    fn drop(&mut self) {
        // Release the in-process claim first. Rust drops the owned `File`
        // field only after this method returns, so the OS lock remains held
        // while the claim is removed.
        release_process_path(&self.lock_path);
        // The exact `File` field is deliberately not unlocked or unlinked
        // here. Its ordinary field drop releases the OS lock after this
        // method returns, and the path remains available for the next process.
    }
}

/// Checks every existing parent component without resolving it to a target.
fn validate_parent_chain(parent: &Path) -> Result<(), ForgeProcessCustodyError> {
    let mut current = parent;
    loop {
        let metadata = match fs::symlink_metadata(current) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(ForgeProcessCustodyError::ParentMissing {
                    path: current.to_path_buf(),
                });
            }
            Err(source) => {
                return Err(ForgeProcessCustodyError::InspectParent {
                    path: current.to_path_buf(),
                    source,
                });
            }
        };

        if metadata.file_type().is_symlink() {
            return Err(ForgeProcessCustodyError::ParentSymlink {
                path: current.to_path_buf(),
            });
        }
        if is_reparse_point(&metadata) {
            return Err(ForgeProcessCustodyError::ParentReparsePoint {
                path: current.to_path_buf(),
            });
        }
        if !metadata.is_dir() {
            return Err(ForgeProcessCustodyError::ParentNotDirectory {
                path: current.to_path_buf(),
            });
        }

        let Some(next) = current.parent() else {
            break;
        };
        if next == current || next.as_os_str().is_empty() {
            break;
        }
        current = next;
    }

    Ok(())
}

/// Inspects the final path without following a symbolic link.
fn inspect_lock_path(path: &Path) -> Result<Option<Metadata>, ForgeProcessCustodyError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validate_lock_metadata(path, &metadata)?;
            Ok(Some(metadata))
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ForgeProcessCustodyError::InspectLockPath {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Opens the pre-existing regular file, or creates it atomically first.
fn open_or_create_lock_file(path: &Path) -> Result<File, ForgeProcessCustodyError> {
    if inspect_lock_path(path)?.is_some() {
        let file = open_lock_file(path)?;
        validate_open_file(path, &file)?;
        return Ok(file);
    }

    match create_lock_file(path) {
        Ok(file) => Ok(file),
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            // A different creator won the create_new race. Re-inspect the
            // resulting path before opening it; no broad create/open retry is
            // permitted if that creator removed or changed the entry.
            if inspect_lock_path(path)?.is_none() {
                return Err(ForgeProcessCustodyError::LockPathMissing {
                    path: path.to_path_buf(),
                });
            }
            let file = open_lock_file(path)?;
            validate_open_file(path, &file)?;
            Ok(file)
        }
        Err(source) => Err(ForgeProcessCustodyError::CreateLockFile {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Creates the lock carrier without truncating or rewriting any existing file.
fn create_lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    configure_no_reparse_open(&mut options);
    options.open(path)
}

/// Opens only for read/write; neither this path nor the existing payload is
/// truncated or otherwise rewritten.
fn open_lock_file(path: &Path) -> Result<File, ForgeProcessCustodyError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    configure_no_reparse_open(&mut options);
    options
        .open(path)
        .map_err(|source| ForgeProcessCustodyError::OpenLockFile {
            path: path.to_path_buf(),
            source,
        })
}

/// Verifies metadata from the retained descriptor as well as the path
/// inspection. A successfully created/opened guard remains tied to the
/// regular, non-reparse file represented by that descriptor, but this check
/// does not freeze or protect the directory entry if the namespace changes
/// afterwards.
fn validate_open_file(path: &Path, file: &File) -> Result<(), ForgeProcessCustodyError> {
    let metadata = file
        .metadata()
        .map_err(|source| ForgeProcessCustodyError::InspectLockPath {
            path: path.to_path_buf(),
            source,
        })?;
    validate_lock_metadata(path, &metadata)
}

fn validate_lock_metadata(
    path: &Path,
    metadata: &Metadata,
) -> Result<(), ForgeProcessCustodyError> {
    if metadata.file_type().is_symlink() {
        return Err(ForgeProcessCustodyError::LockPathSymlink {
            path: path.to_path_buf(),
        });
    }
    if is_reparse_point(metadata) {
        return Err(ForgeProcessCustodyError::LockPathReparsePoint {
            path: path.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(ForgeProcessCustodyError::LockPathNotRegular {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
const fn is_reparse_point(_: &Metadata) -> bool {
    false
}

#[cfg(windows)]
fn configure_no_reparse_open(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;

    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(windows))]
const fn configure_no_reparse_open(_: &mut OpenOptions) {}

/// Tracks exact caller-supplied paths only to make same-process reentrant
/// acquisition fail closed on platforms whose native lock API may otherwise
/// allow it. This is process memory, not lock-file ownership metadata.
fn held_process_paths() -> &'static Mutex<HashSet<PathBuf>> {
    static HELD_PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    HELD_PATHS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn claim_process_path(path: &Path) -> bool {
    let mut paths = held_process_paths()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    paths.insert(path.to_path_buf())
}

fn release_process_path(path: &Path) {
    let mut paths = held_process_paths()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    paths.remove(path);
}

fn is_lock_contention(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(windows)]
    {
        // fs2 preserves LockFileEx's ERROR_LOCK_VIOLATION instead of mapping
        // it to WouldBlock on Windows.
        error.raw_os_error() == Some(33)
    }
    #[cfg(not(windows))]
    {
        false
    }
}
