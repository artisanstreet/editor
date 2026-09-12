//! Certified `OpenCode2` artifact specification, installation paths, and lock.
//!
//! Owns the fixed certified identity of the managed artifact, the validated
//! derivation of the toolchain locations, and the exclusive installation lock
//! shared by installation, registration, and launch resolution.

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

use super::state::ManagedGenerationV1;

pub(crate) const CERTIFIED_ENGINE_ID: &str = "opencode2";
pub(crate) const CERTIFIED_VERSION: &str = "0.0.0-beta-17778";
pub(crate) const CERTIFIED_UPSTREAM_COMMIT: &str = "0d2684b67308380fc47540fe55deb55306a08e3f";
pub(crate) const CERTIFIED_PLATFORM: &str = "win32";
pub(crate) const CERTIFIED_ARCHITECTURE: &str = "x64";
pub(crate) const CERTIFIED_ARTIFACT_KIND: &str = "npm-tarball";
pub(crate) const CERTIFIED_ARCHIVE_MEMBER: &str = "package/bin/opencode2.exe";
pub(crate) const CERTIFIED_BINARY: &str = "opencode2.exe";
pub(crate) const CERTIFIED_NPM_INTEGRITY_SHA512: &str =
    "Z0oMvTBUhxmz1IYuQSMOZTpI2HoWjeIjdxJ39SoGrhDwvJZK7OI0rgIMYtDGavOucOQT8oxrazUiO4j+2hVMpw==";
pub(crate) const CERTIFIED_DOWNLOAD_BOUND_BYTES: u64 = 268_435_456;
pub(crate) const CERTIFIED_EXECUTABLE_SIZE_BYTES: u64 = 144_313_344;
pub(crate) const CERTIFIED_EXECUTABLE_SHA256_HEX: &str =
    "452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf";
pub(crate) const CERTIFIED_EXECUTABLE_SHA256: [u8; 32] = [
    0x45, 0x27, 0x94, 0xa7, 0x64, 0xe1, 0x03, 0x3e, 0x62, 0x9c, 0x4c, 0xd4, 0x0b, 0xde, 0x64, 0x33,
    0xc1, 0x0c, 0x6b, 0xd3, 0x24, 0x33, 0xfb, 0x3b, 0xe2, 0x79, 0xbf, 0x03, 0x96, 0x9a, 0x6e, 0xdf,
];
pub(crate) const CERTIFIED_NPM_URL: &str = "https://registry.npmjs.org/@opencode-ai/cli-windows-x64/-/cli-windows-x64-0.0.0-beta-17778.tgz";

const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_POLL: Duration = Duration::from_millis(50);

/// The exact certified `OpenCode2` artifact identity shared by installation and
/// launch resolution.
#[derive(Clone, Copy)]
pub struct NativeOpenCode2InstallSpec {
    pub(crate) engine_id: &'static str,
    pub(crate) version: &'static str,
    pub(crate) upstream_commit: &'static str,
    pub(crate) platform: &'static str,
    pub(crate) architecture: &'static str,
    pub(crate) artifact_kind: &'static str,
    pub(crate) archive_member: &'static str,
    pub(crate) binary: &'static str,
    pub(crate) npm_integrity_sha512: &'static str,
    pub(crate) npm_url: &'static str,
    pub(crate) download_bound_bytes: u64,
    pub(crate) executable_size_bytes: u64,
    pub(crate) executable_sha256: [u8; 32],
    pub(crate) executable_sha256_hex: &'static str,
}

impl fmt::Debug for NativeOpenCode2InstallSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeOpenCode2InstallSpec")
            .finish_non_exhaustive()
    }
}

impl NativeOpenCode2InstallSpec {
    /// Returns the certified engine identifier.
    #[must_use]
    pub const fn engine_id(&self) -> &'static str {
        self.engine_id
    }

    /// Returns the certified artifact version.
    #[must_use]
    pub const fn version(&self) -> &'static str {
        self.version
    }

    /// Returns the certified upstream source commit.
    #[must_use]
    pub const fn upstream_commit(&self) -> &'static str {
        self.upstream_commit
    }

    /// Returns the certified target platform.
    #[must_use]
    pub const fn platform(&self) -> &'static str {
        self.platform
    }

    /// Returns the certified target architecture.
    #[must_use]
    pub const fn architecture(&self) -> &'static str {
        self.architecture
    }

    /// Returns the certified artifact kind.
    #[must_use]
    pub const fn artifact_kind(&self) -> &'static str {
        self.artifact_kind
    }

    /// Returns the exact archive member containing the executable.
    #[must_use]
    pub const fn archive_member(&self) -> &'static str {
        self.archive_member
    }

    /// Returns the certified executable file name.
    #[must_use]
    pub const fn binary(&self) -> &'static str {
        self.binary
    }

    /// Returns the certified npm package integrity value.
    #[must_use]
    pub const fn npm_integrity_sha512(&self) -> &'static str {
        self.npm_integrity_sha512
    }

    /// Returns the certified npm package URL.
    #[must_use]
    pub const fn npm_url(&self) -> &'static str {
        self.npm_url
    }

    /// Returns the maximum permitted download size in bytes.
    #[must_use]
    pub const fn download_bound_bytes(&self) -> u64 {
        self.download_bound_bytes
    }

    /// Returns the certified executable size in bytes.
    #[must_use]
    pub const fn executable_size_bytes(&self) -> u64 {
        self.executable_size_bytes
    }

    /// Returns the certified executable SHA-256 digest.
    #[must_use]
    pub const fn executable_sha256(&self) -> &[u8; 32] {
        &self.executable_sha256
    }

    /// Returns the certified executable SHA-256 digest in hexadecimal form.
    #[must_use]
    pub const fn executable_sha256_hex(&self) -> &'static str {
        self.executable_sha256_hex
    }

    pub(crate) fn generation(&self, directory: &str) -> ManagedGenerationV1 {
        ManagedGenerationV1 {
            binary: self.binary.to_owned(),
            directory: directory.to_owned(),
            sha256: self.executable_sha256_hex.to_owned(),
            version: self.version.to_owned(),
        }
    }
}

/// Failure while deriving or preparing the certified installation paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOpenCode2InstallPathError {
    InvalidRoot,
    Unavailable,
}

impl fmt::Display for NativeOpenCode2InstallPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "OpenCode2 installation root is invalid",
            Self::Unavailable => "OpenCode2 installation root is unavailable",
        })
    }
}

impl std::error::Error for NativeOpenCode2InstallPathError {}

/// The validated filesystem locations used by the certified installation.
#[must_use = "retain the validated paths for the operation they authorize"]
#[derive(Clone)]
pub struct NativeOpenCode2InstallPaths {
    database_parent: PathBuf,
    toolchain_root: PathBuf,
    engine_root: PathBuf,
    versions_root: PathBuf,
    lock_path: PathBuf,
}

impl fmt::Debug for NativeOpenCode2InstallPaths {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeOpenCode2InstallPaths")
            .finish_non_exhaustive()
    }
}

impl NativeOpenCode2InstallPaths {
    /// Derives the certified installation locations from an absolute database
    /// path.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2InstallPathError::InvalidRoot`] for an unsafe
    /// or structurally invalid database path.
    pub fn derive(
        database_path: &Path,
        spec: &NativeOpenCode2InstallSpec,
    ) -> Result<Self, NativeOpenCode2InstallPathError> {
        if !database_path.is_absolute()
            || database_path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(NativeOpenCode2InstallPathError::InvalidRoot);
        }
        let database_parent = database_path
            .parent()
            .filter(|parent| parent.is_absolute() && !parent.as_os_str().is_empty())
            .ok_or(NativeOpenCode2InstallPathError::InvalidRoot)?;
        let toolchain_root = database_parent.join("toolchain");
        let engine_root = toolchain_root.join(spec.engine_id());
        let versions_root = engine_root.join("versions");
        let lock_path = engine_root.join("install.lock");
        if !engine_root.is_absolute()
            || engine_root
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(NativeOpenCode2InstallPathError::InvalidRoot);
        }
        Ok(Self {
            database_parent: database_parent.to_path_buf(),
            toolchain_root,
            engine_root,
            versions_root,
            lock_path,
        })
    }

    /// Creates the certified installation directories and verifies them.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2InstallPathError`] when a directory is unsafe,
    /// unavailable, or cannot be created.
    pub fn prepare(&self) -> Result<(), NativeOpenCode2InstallPathError> {
        native_files::ensure_directory(&self.toolchain_root).map_err(map_path_file_error)?;
        native_files::ensure_directory(&self.engine_root).map_err(map_path_file_error)?;
        native_files::ensure_directory(&self.versions_root).map_err(map_path_file_error)?;
        self.verify()
    }

    /// Verifies the certified installation directories and ancestor chain.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2InstallPathError`] when a directory is unsafe
    /// or unavailable.
    pub fn verify(&self) -> Result<(), NativeOpenCode2InstallPathError> {
        native_files::verify_directory(&self.database_parent).map_err(map_path_file_error)?;
        native_files::verify_directory(&self.toolchain_root).map_err(map_path_file_error)?;
        native_files::verify_directory(&self.engine_root).map_err(map_path_file_error)?;
        native_files::verify_directory(&self.versions_root).map_err(map_path_file_error)
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

    /// Returns the validated certified engine root.
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
}

/// Failure while acquiring or fencing the certified installation lock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOpenCode2InstallLockError {
    InvalidRoot,
    Unavailable,
    Timeout,
    Busy,
    IdentityChanged,
}

impl fmt::Display for NativeOpenCode2InstallLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "OpenCode2 installation root is invalid",
            Self::Unavailable => "OpenCode2 installation lock is unavailable",
            Self::Timeout => "OpenCode2 installation lock timed out",
            Self::Busy => "OpenCode2 installation lock is busy",
            Self::IdentityChanged => "OpenCode2 installation lock identity changed",
        })
    }
}

impl std::error::Error for NativeOpenCode2InstallLockError {}

/// RAII custody of the exclusive lock shared by installation, registration,
/// and profile launch resolution.
#[must_use = "the lock must remain live for the protected operation"]
pub struct NativeOpenCode2InstallLock {
    path: PathBuf,
    file: File,
}

impl fmt::Debug for NativeOpenCode2InstallLock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeOpenCode2InstallLock")
            .finish_non_exhaustive()
    }
}

impl NativeOpenCode2InstallLock {
    /// Acquires and fences the exclusive installation lock, waiting briefly if
    /// another cooperating operation currently owns it.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2InstallLockError`] when the lock path is
    /// unsafe, unavailable, changed, or cannot be acquired before the timeout.
    pub fn acquire(
        paths: &NativeOpenCode2InstallPaths,
    ) -> Result<Self, NativeOpenCode2InstallLockError> {
        Self::acquire_inner(paths, true)
    }

    /// Attempts one non-blocking acquisition. It gives tests and callers a
    /// typed way to prove that a live launch capability retains the fence.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2InstallLockError::Busy`] when another
    /// operation owns the lock, or another variant when the lock is unsafe,
    /// unavailable, or changed.
    pub fn try_acquire(
        paths: &NativeOpenCode2InstallPaths,
    ) -> Result<Self, NativeOpenCode2InstallLockError> {
        Self::acquire_inner(paths, false)
    }

    fn acquire_inner(
        paths: &NativeOpenCode2InstallPaths,
        wait: bool,
    ) -> Result<Self, NativeOpenCode2InstallLockError> {
        paths.verify().map_err(map_path_lock_error)?;
        let file = open_lock(paths.lock_path())?;
        let lock = Self {
            path: paths.lock_path().to_path_buf(),
            file,
        };
        let deadline = Instant::now()
            .checked_add(LOCK_TIMEOUT)
            .ok_or(NativeOpenCode2InstallLockError::Timeout)?;
        loop {
            match lock.file.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if is_lock_contended(&error) && !wait => {
                    return Err(NativeOpenCode2InstallLockError::Busy);
                }
                Err(error) if is_lock_contended(&error) => {
                    if Instant::now() >= deadline {
                        return Err(NativeOpenCode2InstallLockError::Timeout);
                    }
                    thread::sleep(LOCK_POLL);
                }
                Err(_) => return Err(NativeOpenCode2InstallLockError::Unavailable),
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
    /// Returns [`NativeOpenCode2InstallLockError`] when the lock or its path
    /// is unavailable, unsafe, or has changed.
    pub fn fence(
        &self,
        paths: &NativeOpenCode2InstallPaths,
    ) -> Result<(), NativeOpenCode2InstallLockError> {
        paths.verify().map_err(map_path_lock_error)?;
        native_files::verify_regular_file(&self.path).map_err(|error| match error {
            NativeFileError::UnsafePath
            | NativeFileError::FileChanged
            | NativeFileError::NotFound => NativeOpenCode2InstallLockError::IdentityChanged,
            NativeFileError::TooLarge
            | NativeFileError::FileSizeMismatch
            | NativeFileError::FileHashMismatch
            | NativeFileError::Io
            | NativeFileError::PrivatePermissions => NativeOpenCode2InstallLockError::Unavailable,
        })?;
        let open_id = native_files::file_identity(&self.file).map_err(map_file_lock_error)?;
        let path_id = native_files::path_identity(&self.path).map_err(map_file_lock_error)?;
        if open_id != path_id {
            return Err(NativeOpenCode2InstallLockError::IdentityChanged);
        }
        Ok(())
    }
}

pub(crate) fn is_lock_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || matches!(
            (error.raw_os_error(), fs2::lock_contended_error().raw_os_error()),
            (Some(actual), Some(contended)) if actual == contended
        )
}

fn open_lock(path: &Path) -> Result<File, NativeOpenCode2InstallLockError> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (native_files::metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file())
    {
        return Err(NativeOpenCode2InstallLockError::Unavailable);
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|_| NativeOpenCode2InstallLockError::Unavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| NativeOpenCode2InstallLockError::Unavailable)?;
    if native_files::metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file() {
        return Err(NativeOpenCode2InstallLockError::Unavailable);
    }
    native_files::verify_regular_file(path).map_err(|error| match error {
        NativeFileError::UnsafePath | NativeFileError::FileChanged => {
            NativeOpenCode2InstallLockError::IdentityChanged
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io
        | NativeFileError::PrivatePermissions => NativeOpenCode2InstallLockError::Unavailable,
    })?;
    let open_id = native_files::file_identity(&file).map_err(map_file_lock_error)?;
    let path_id = native_files::path_identity(path).map_err(map_file_lock_error)?;
    if open_id != path_id {
        return Err(NativeOpenCode2InstallLockError::IdentityChanged);
    }
    Ok(file)
}

/// Returns whether the certified `OpenCode2` executable is supported here.
#[must_use]
pub const fn platform_supported() -> bool {
    cfg!(all(target_os = "windows", target_arch = "x86_64"))
}

fn map_path_file_error(error: NativeFileError) -> NativeOpenCode2InstallPathError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            NativeOpenCode2InstallPathError::InvalidRoot
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => NativeOpenCode2InstallPathError::Unavailable,
    }
}

pub(crate) fn map_path_lock_error(
    error: NativeOpenCode2InstallPathError,
) -> NativeOpenCode2InstallLockError {
    match error {
        NativeOpenCode2InstallPathError::InvalidRoot => {
            NativeOpenCode2InstallLockError::InvalidRoot
        }
        NativeOpenCode2InstallPathError::Unavailable => {
            NativeOpenCode2InstallLockError::Unavailable
        }
    }
}

fn map_file_lock_error(error: NativeFileError) -> NativeOpenCode2InstallLockError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::FileChanged => {
            NativeOpenCode2InstallLockError::IdentityChanged
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io
        | NativeFileError::PrivatePermissions => NativeOpenCode2InstallLockError::Unavailable,
    }
}
