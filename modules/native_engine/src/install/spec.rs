//! Managed engine installation paths, the exclusive install lock, and the
//! shared use lease.
//!
//! Every engine owns `<database parent>/toolchain/<engine>/` with its
//! `versions/` generations, `state.json`, `selection.json`, an exclusive
//! `install.lock` shared by installation, switching, pruning, and launch
//! resolution, and a `use.lock` that running engine processes hold shared so
//! a generation switch never happens mid-run.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io,
    path::{Component, Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use fs2::FileExt;

use crate::io as native_files;
use crate::io::NativeFileError;

use super::catalog::ManagedEngine;

const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_POLL: Duration = Duration::from_millis(50);

/// Failure while deriving or preparing the managed installation paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedInstallPathError {
    InvalidRoot,
    Unavailable,
}

impl fmt::Display for ManagedInstallPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "managed engine installation root is invalid",
            Self::Unavailable => "managed engine installation root is unavailable",
        })
    }
}

impl std::error::Error for ManagedInstallPathError {}

/// The validated filesystem locations of one engine's managed installation.
#[must_use = "retain the validated paths for the operation they authorize"]
#[derive(Clone)]
pub struct ManagedInstallPaths {
    engine: ManagedEngine,
    database_parent: PathBuf,
    toolchain_root: PathBuf,
    engine_root: PathBuf,
    versions_root: PathBuf,
    lock_path: PathBuf,
    use_lock_path: PathBuf,
}

impl fmt::Debug for ManagedInstallPaths {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedInstallPaths")
            .field("engine", &self.engine)
            .finish_non_exhaustive()
    }
}

impl ManagedInstallPaths {
    /// Derives the managed installation locations from an absolute database
    /// path.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallPathError::InvalidRoot`] for an unsafe or
    /// structurally invalid database path.
    pub fn derive(
        database_path: &Path,
        engine: ManagedEngine,
    ) -> Result<Self, ManagedInstallPathError> {
        if !is_absolute_without_parent_segments(database_path) {
            return Err(ManagedInstallPathError::InvalidRoot);
        }
        let database_parent = database_path
            .parent()
            .filter(|parent| parent.is_absolute() && !parent.as_os_str().is_empty())
            .ok_or(ManagedInstallPathError::InvalidRoot)?;
        let toolchain_root = database_parent.join("toolchain");
        let engine_root = toolchain_root.join(engine.id());
        if !is_absolute_without_parent_segments(&engine_root) {
            return Err(ManagedInstallPathError::InvalidRoot);
        }
        Ok(Self {
            engine,
            database_parent: database_parent.to_path_buf(),
            versions_root: engine_root.join("versions"),
            lock_path: engine_root.join("install.lock"),
            use_lock_path: engine_root.join("use.lock"),
            toolchain_root,
            engine_root,
        })
    }

    /// Creates the installation directories and verifies them.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallPathError`] when a directory is unsafe,
    /// unavailable, or cannot be created.
    pub fn prepare(&self) -> Result<(), ManagedInstallPathError> {
        native_files::ensure_directory(&self.toolchain_root).map_err(map_path_file_error)?;
        native_files::ensure_directory(&self.engine_root).map_err(map_path_file_error)?;
        native_files::ensure_directory(&self.versions_root).map_err(map_path_file_error)?;
        self.verify()
    }

    /// Verifies the installation directories and ancestor chain.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallPathError`] when a directory is unsafe or
    /// unavailable.
    pub fn verify(&self) -> Result<(), ManagedInstallPathError> {
        native_files::verify_directory(&self.database_parent).map_err(map_path_file_error)?;
        native_files::verify_directory(&self.toolchain_root).map_err(map_path_file_error)?;
        native_files::verify_directory(&self.engine_root).map_err(map_path_file_error)?;
        native_files::verify_directory(&self.versions_root).map_err(map_path_file_error)
    }

    /// Returns the engine these paths belong to.
    #[must_use]
    pub const fn engine(&self) -> ManagedEngine {
        self.engine
    }

    /// Returns the validated directory containing the database.
    #[must_use]
    pub fn database_parent(&self) -> &Path {
        &self.database_parent
    }

    /// Returns the validated toolchain root.
    #[must_use]
    pub fn toolchain_root(&self) -> &Path {
        &self.toolchain_root
    }

    /// Returns the validated engine root.
    #[must_use]
    pub fn engine_root(&self) -> &Path {
        &self.engine_root
    }

    /// Returns the validated generation root.
    #[must_use]
    pub fn versions_root(&self) -> &Path {
        &self.versions_root
    }

    /// Returns the path of the exclusive installation lock.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Returns the path of the shared use lease.
    #[must_use]
    pub fn use_lock_path(&self) -> &Path {
        &self.use_lock_path
    }
}

fn is_absolute_without_parent_segments(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

/// Failure while acquiring or fencing a managed installation lock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedInstallLockError {
    InvalidRoot,
    Unavailable,
    Timeout,
    Busy,
    IdentityChanged,
}

impl fmt::Display for ManagedInstallLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "managed engine installation root is invalid",
            Self::Unavailable => "managed engine installation lock is unavailable",
            Self::Timeout => "managed engine installation lock timed out",
            Self::Busy => "managed engine installation lock is busy",
            Self::IdentityChanged => "managed engine installation lock identity changed",
        })
    }
}

impl std::error::Error for ManagedInstallLockError {}

#[derive(Clone, Copy, Eq, PartialEq)]
enum LockMode {
    Exclusive,
    Shared,
}

/// RAII custody of a fenced lock file.
#[must_use = "the lock must remain live for the protected operation"]
pub struct ManagedInstallLock {
    path: PathBuf,
    file: File,
}

impl fmt::Debug for ManagedInstallLock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedInstallLock")
            .finish_non_exhaustive()
    }
}

impl ManagedInstallLock {
    /// Acquires and fences the exclusive installation lock, waiting briefly if
    /// another cooperating operation currently owns it.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallLockError`] when the lock path is unsafe,
    /// unavailable, changed, or cannot be acquired before the timeout.
    pub fn acquire(paths: &ManagedInstallPaths) -> Result<Self, ManagedInstallLockError> {
        Self::acquire_at(paths, paths.lock_path(), LockMode::Exclusive, true)
    }

    /// Attempts one non-blocking exclusive acquisition.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallLockError::Busy`] when another operation owns
    /// the lock, or another variant when the lock is unsafe, unavailable, or
    /// changed.
    pub fn try_acquire(paths: &ManagedInstallPaths) -> Result<Self, ManagedInstallLockError> {
        Self::acquire_at(paths, paths.lock_path(), LockMode::Exclusive, false)
    }

    fn acquire_at(
        paths: &ManagedInstallPaths,
        path: &Path,
        mode: LockMode,
        wait: bool,
    ) -> Result<Self, ManagedInstallLockError> {
        paths.verify().map_err(map_path_lock_error)?;
        let file = open_lock(path)?;
        let lock = Self {
            path: path.to_path_buf(),
            file,
        };
        let deadline = Instant::now()
            .checked_add(LOCK_TIMEOUT)
            .ok_or(ManagedInstallLockError::Timeout)?;
        loop {
            let attempt = match mode {
                LockMode::Exclusive => FileExt::try_lock_exclusive(&lock.file),
                LockMode::Shared => FileExt::try_lock_shared(&lock.file),
            };
            match attempt {
                Ok(()) => break,
                Err(error) if is_lock_contended(&error) && !wait => {
                    return Err(ManagedInstallLockError::Busy);
                }
                Err(error) if is_lock_contended(&error) => {
                    if Instant::now() >= deadline {
                        return Err(ManagedInstallLockError::Timeout);
                    }
                    thread::sleep(LOCK_POLL);
                }
                Err(_) => return Err(ManagedInstallLockError::Unavailable),
            }
        }
        lock.fence(paths)?;
        Ok(lock)
    }

    /// Revalidates the lock file's path and identity against the open lock
    /// handle.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallLockError`] when the lock or its path is
    /// unavailable, unsafe, or has changed.
    pub fn fence(&self, paths: &ManagedInstallPaths) -> Result<(), ManagedInstallLockError> {
        paths.verify().map_err(map_path_lock_error)?;
        native_files::verify_regular_file(&self.path).map_err(|error| match error {
            NativeFileError::UnsafePath
            | NativeFileError::FileChanged
            | NativeFileError::NotFound => ManagedInstallLockError::IdentityChanged,
            NativeFileError::TooLarge
            | NativeFileError::FileSizeMismatch
            | NativeFileError::FileHashMismatch
            | NativeFileError::Io
            | NativeFileError::PrivatePermissions => ManagedInstallLockError::Unavailable,
        })?;
        let open_id = native_files::file_identity(&self.file).map_err(map_file_lock_error)?;
        let path_id = native_files::path_identity(&self.path).map_err(map_file_lock_error)?;
        if open_id != path_id {
            return Err(ManagedInstallLockError::IdentityChanged);
        }
        Ok(())
    }
}

/// A shared lease held by every live engine process launched from a managed
/// generation. A generation switch requires the lease exclusively, so it is
/// deferred while any process of the engine is running.
#[must_use = "the lease must stay alive for as long as the engine process runs"]
pub struct EngineUseLease {
    _lock: ManagedInstallLock,
}

impl fmt::Debug for EngineUseLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EngineUseLease")
    }
}

impl EngineUseLease {
    /// Takes a shared use lease, waiting for an in-progress switch.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallLockError`] when the lease file is unsafe or a
    /// switch holds it beyond the timeout.
    pub fn acquire(paths: &ManagedInstallPaths) -> Result<Self, ManagedInstallLockError> {
        ManagedInstallLock::acquire_at(paths, paths.use_lock_path(), LockMode::Shared, true)
            .map(|lock| Self { _lock: lock })
    }
}

/// Exclusive custody of the use lease: proof that no managed process of the
/// engine is running while a generation switch is published.
#[must_use = "retain the idle proof while switching generations"]
pub struct EngineIdle {
    _lock: ManagedInstallLock,
}

impl fmt::Debug for EngineIdle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EngineIdle")
    }
}

impl EngineIdle {
    /// Attempts to prove the engine idle without waiting.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallLockError::Busy`] while any lease is held.
    pub fn try_acquire(paths: &ManagedInstallPaths) -> Result<Self, ManagedInstallLockError> {
        ManagedInstallLock::acquire_at(paths, paths.use_lock_path(), LockMode::Exclusive, false)
            .map(|lock| Self { _lock: lock })
    }
}

pub(crate) fn is_lock_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || matches!(
            (error.raw_os_error(), fs2::lock_contended_error().raw_os_error()),
            (Some(actual), Some(contended)) if actual == contended
        )
}

fn open_lock(path: &Path) -> Result<File, ManagedInstallLockError> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (native_files::metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file())
    {
        return Err(ManagedInstallLockError::Unavailable);
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|_| ManagedInstallLockError::Unavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| ManagedInstallLockError::Unavailable)?;
    if native_files::metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file() {
        return Err(ManagedInstallLockError::Unavailable);
    }
    native_files::verify_regular_file(path).map_err(|error| match error {
        NativeFileError::UnsafePath | NativeFileError::FileChanged => {
            ManagedInstallLockError::IdentityChanged
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io
        | NativeFileError::PrivatePermissions => ManagedInstallLockError::Unavailable,
    })?;
    let open_id = native_files::file_identity(&file).map_err(map_file_lock_error)?;
    let path_id = native_files::path_identity(path).map_err(map_file_lock_error)?;
    if open_id != path_id {
        return Err(ManagedInstallLockError::IdentityChanged);
    }
    Ok(file)
}

fn map_path_file_error(error: NativeFileError) -> ManagedInstallPathError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            ManagedInstallPathError::InvalidRoot
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => ManagedInstallPathError::Unavailable,
    }
}

pub(crate) fn map_path_lock_error(error: ManagedInstallPathError) -> ManagedInstallLockError {
    match error {
        ManagedInstallPathError::InvalidRoot => ManagedInstallLockError::InvalidRoot,
        ManagedInstallPathError::Unavailable => ManagedInstallLockError::Unavailable,
    }
}

fn map_file_lock_error(error: NativeFileError) -> ManagedInstallLockError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::FileChanged => {
            ManagedInstallLockError::IdentityChanged
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io
        | NativeFileError::PrivatePermissions => ManagedInstallLockError::Unavailable,
    }
}
