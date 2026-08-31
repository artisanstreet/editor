use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io,
    path::{Component, Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use fs2::FileExt;
use serde::{
    Deserialize, Serialize,
    de::{self, Deserializer, MapAccess, Visitor},
};

use crate::io as native_files;
use crate::io::{AtomicReplaceOutcome, NativeFileError, VerifiedFileIdentity};

const CERTIFIED_ENGINE_ID: &str = "opencode2";
const CERTIFIED_VERSION: &str = "0.0.0-beta-17778";
const CERTIFIED_UPSTREAM_COMMIT: &str = "0d2684b67308380fc47540fe55deb55306a08e3f";
const CERTIFIED_PLATFORM: &str = "win32";
const CERTIFIED_ARCHITECTURE: &str = "x64";
const CERTIFIED_ARTIFACT_KIND: &str = "npm-tarball";
const CERTIFIED_ARCHIVE_MEMBER: &str = "package/bin/opencode2.exe";
const CERTIFIED_BINARY: &str = "opencode2.exe";
const CERTIFIED_NPM_INTEGRITY_SHA512: &str =
    "Z0oMvTBUhxmz1IYuQSMOZTpI2HoWjeIjdxJ39SoGrhDwvJZK7OI0rgIMYtDGavOucOQT8oxrazUiO4j+2hVMpw==";
const CERTIFIED_DOWNLOAD_BOUND_BYTES: u64 = 268_435_456;
const CERTIFIED_EXECUTABLE_SIZE_BYTES: u64 = 144_313_344;
const CERTIFIED_EXECUTABLE_SHA256_HEX: &str =
    "452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf";
const CERTIFIED_EXECUTABLE_SHA256: [u8; 32] = [
    0x45, 0x27, 0x94, 0xa7, 0x64, 0xe1, 0x03, 0x3e, 0x62, 0x9c, 0x4c, 0xd4, 0x0b, 0xde, 0x64, 0x33,
    0xc1, 0x0c, 0x6b, 0xd3, 0x24, 0x33, 0xfb, 0x3b, 0xe2, 0x79, 0xbf, 0x03, 0x96, 0x9a, 0x6e, 0xdf,
];
const CERTIFIED_NPM_URL: &str = "https://registry.npmjs.org/@opencode-ai/cli-windows-x64/-/cli-windows-x64-0.0.0-beta-17778.tgz";

const MAX_STATE_BYTES: usize = 16 * 1024;
const MAX_GENERATION_ID_BYTES: usize = 128;
const MAX_BINARY_PATH_BYTES: usize = 256;
const MAX_VERSION_BYTES: usize = 128;
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_POLL: Duration = Duration::from_millis(50);

/// The exact certified OpenCode2 artifact identity shared by installation and
/// launch resolution.
#[derive(Clone, Copy)]
pub struct NativeOpenCode2InstallSpec {
    engine_id: &'static str,
    version: &'static str,
    upstream_commit: &'static str,
    platform: &'static str,
    architecture: &'static str,
    artifact_kind: &'static str,
    archive_member: &'static str,
    binary: &'static str,
    npm_integrity_sha512: &'static str,
    npm_url: &'static str,
    download_bound_bytes: u64,
    executable_size_bytes: u64,
    executable_sha256: [u8; 32],
    executable_sha256_hex: &'static str,
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

    fn generation(&self, directory: &str) -> ManagedGenerationV1 {
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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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

fn is_lock_contended(error: &io::Error) -> bool {
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

/// Bounded, path-free failures from certified OpenCode2 inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOpenCode2Error {
    UnsupportedPlatform,
    StateMissing,
    StateTooLarge,
    StateMalformed,
    StateUnsupportedVersion,
    ActiveGenerationUntrusted,
    UnsafePath,
    ExecutableUnavailable,
    ExecutableChanged,
    ExecutableSizeMismatch,
    ExecutableHashMismatch,
    Io,
}

impl NativeOpenCode2Error {
    /// Returns the stable CLI classification for this failure.
    #[must_use]
    pub const fn cli_reason(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::StateMissing => "not_installed",
            Self::StateTooLarge | Self::StateMalformed | Self::StateUnsupportedVersion => {
                "state_invalid"
            }
            Self::ActiveGenerationUntrusted => "generation_untrusted",
            Self::UnsafePath => "unsafe_path",
            Self::ExecutableUnavailable => "executable_unavailable",
            Self::ExecutableChanged => "executable_changed",
            Self::ExecutableSizeMismatch => "executable_size_mismatch",
            Self::ExecutableHashMismatch => "executable_hash_mismatch",
            Self::Io => "io",
        }
    }
}

impl fmt::Display for NativeOpenCode2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedPlatform => "OpenCode2 is unsupported on this platform",
            Self::StateMissing => "OpenCode2 managed state is missing",
            Self::StateTooLarge => "OpenCode2 managed state exceeds its bound",
            Self::StateMalformed => "OpenCode2 managed state is malformed",
            Self::StateUnsupportedVersion => "OpenCode2 managed state version is unsupported",
            Self::ActiveGenerationUntrusted => "OpenCode2 active generation is untrusted",
            Self::UnsafePath => "OpenCode2 managed path is unsafe",
            Self::ExecutableUnavailable => "OpenCode2 executable is unavailable",
            Self::ExecutableChanged => "OpenCode2 executable changed during verification",
            Self::ExecutableSizeMismatch => "OpenCode2 executable size does not match",
            Self::ExecutableHashMismatch => "OpenCode2 executable hash does not match",
            Self::Io => "OpenCode2 authority I/O failed",
        })
    }
}

impl std::error::Error for NativeOpenCode2Error {}

/// Bounded, path-free failures from certified install-state codec operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOpenCode2StateError {
    InvalidRoot,
    TooLarge,
    Malformed,
    UnsupportedVersion,
    ActiveGenerationUntrusted,
    UnsafePath,
    Io,
    Encode,
    AtomicPublishFailed,
}

impl fmt::Display for NativeOpenCode2StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "OpenCode2 install state root is invalid",
            Self::TooLarge => "OpenCode2 install state exceeds its bound",
            Self::Malformed => "OpenCode2 install state is malformed",
            Self::UnsupportedVersion => "OpenCode2 install state version is unsupported",
            Self::ActiveGenerationUntrusted => {
                "OpenCode2 install state has an untrusted active generation"
            }
            Self::UnsafePath => "OpenCode2 install state path is unsafe",
            Self::Io => "OpenCode2 install state I/O failed",
            Self::Encode => "OpenCode2 install state encoding failed",
            Self::AtomicPublishFailed => "OpenCode2 install state publication failed",
        })
    }
}

impl std::error::Error for NativeOpenCode2StateError {}

/// The result of inspecting the certified OpenCode2 installation.
#[must_use = "inspection results contain the certified generation decision"]
#[derive(Debug)]
pub enum OpenCode2Inspection {
    UnsupportedPlatform,
    NotInstalled,
    Ready(ResolvedOpenCode2Generation),
}

/// A certified active generation whose executable identity, size, and hash
/// were verified together.
#[must_use = "retain the verified generation for the protected launch"]
pub struct ResolvedOpenCode2Generation {
    executable: PathBuf,
    generation_id: String,
    version: &'static str,
    upstream_commit: &'static str,
    executable_size_bytes: u64,
    executable_sha256: [u8; 32],
    verified_file_id: VerifiedFileIdentity,
}

impl fmt::Debug for ResolvedOpenCode2Generation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedOpenCode2Generation")
            .finish_non_exhaustive()
    }
}

impl ResolvedOpenCode2Generation {
    /// Returns the certified executable path.
    #[must_use]
    pub fn executable_path(&self) -> &Path {
        &self.executable
    }

    /// Returns the certified generation identifier.
    #[must_use]
    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    /// Returns the certified artifact version.
    #[must_use]
    pub fn version(&self) -> &'static str {
        self.version
    }

    /// Returns the certified upstream source commit.
    #[must_use]
    pub fn upstream_commit(&self) -> &'static str {
        self.upstream_commit
    }

    /// Returns the certified executable size in bytes.
    #[must_use]
    pub fn executable_size_bytes(&self) -> u64 {
        self.executable_size_bytes
    }

    /// Returns the certified executable SHA-256 digest.
    #[must_use]
    pub fn executable_sha256(&self) -> &[u8; 32] {
        &self.executable_sha256
    }

    pub(crate) const fn file_identity(&self) -> VerifiedFileIdentity {
        self.verified_file_id
    }
}

/// A validated, non-serializable view of the managed OpenCode2 install state.
#[must_use = "retain validated install state for the operation it authorizes"]
pub struct NativeOpenCode2State {
    inner: ManagedToolchainStateV1,
}

impl fmt::Debug for NativeOpenCode2State {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeOpenCode2State")
            .finish_non_exhaustive()
    }
}

/// Shared authority for the certified OpenCode2 specification, install state,
/// and filesystem verification.
#[must_use = "use the authority for certified OpenCode2 operations"]
pub struct NativeOpenCode2Authority {
    install_spec: NativeOpenCode2InstallSpec,
}

impl fmt::Debug for NativeOpenCode2Authority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeOpenCode2Authority")
            .finish_non_exhaustive()
    }
}

impl NativeOpenCode2Authority {
    /// Constructs the explicit certified OpenCode2 authority.
    #[must_use]
    // This compatibility-preserved constructor deliberately has no `Default`:
    // callers must opt into the certified authority explicitly rather than
    // implying ambient or inferred launch configuration.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            install_spec: Self::certified_install_spec(),
        }
    }

    pub(crate) const fn with_spec(install_spec: NativeOpenCode2InstallSpec) -> Self {
        Self { install_spec }
    }

    /// Returns the immutable certified OpenCode2 artifact specification.
    #[must_use]
    pub const fn certified_install_spec() -> NativeOpenCode2InstallSpec {
        NativeOpenCode2InstallSpec {
            engine_id: CERTIFIED_ENGINE_ID,
            version: CERTIFIED_VERSION,
            upstream_commit: CERTIFIED_UPSTREAM_COMMIT,
            platform: CERTIFIED_PLATFORM,
            architecture: CERTIFIED_ARCHITECTURE,
            artifact_kind: CERTIFIED_ARTIFACT_KIND,
            archive_member: CERTIFIED_ARCHIVE_MEMBER,
            binary: CERTIFIED_BINARY,
            npm_integrity_sha512: CERTIFIED_NPM_INTEGRITY_SHA512,
            npm_url: CERTIFIED_NPM_URL,
            download_bound_bytes: CERTIFIED_DOWNLOAD_BOUND_BYTES,
            executable_size_bytes: CERTIFIED_EXECUTABLE_SIZE_BYTES,
            executable_sha256: CERTIFIED_EXECUTABLE_SHA256,
            executable_sha256_hex: CERTIFIED_EXECUTABLE_SHA256_HEX,
        }
    }

    /// Derives the certified installation paths for an absolute database path.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2InstallPathError`] when the database path is
    /// unsafe or the derived installation root is unavailable.
    #[must_use]
    pub fn install_paths(
        &self,
        database_path: &Path,
    ) -> Result<NativeOpenCode2InstallPaths, NativeOpenCode2InstallPathError> {
        NativeOpenCode2InstallPaths::derive(database_path, &self.install_spec)
    }

    /// Acquires and fences the shared exclusive installation lock.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2InstallLockError`] when the installation root
    /// or lock is unsafe, unavailable, changed, or busy beyond the timeout.
    #[must_use]
    pub fn acquire_install_lock(
        &self,
        database_path: &Path,
    ) -> Result<NativeOpenCode2InstallLock, NativeOpenCode2InstallLockError> {
        let paths = self
            .install_paths(database_path)
            .map_err(map_path_lock_error)?;
        NativeOpenCode2InstallLock::acquire(&paths)
    }

    /// Inspects the certified installation without discovering or selecting a
    /// profile.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2Error`] when managed state or its active
    /// executable fails certified validation.
    #[must_use]
    pub fn inspect(
        &self,
        database_path: &Path,
    ) -> Result<OpenCode2Inspection, NativeOpenCode2Error> {
        if !platform_supported() {
            return Ok(OpenCode2Inspection::UnsupportedPlatform);
        }
        match self.resolve_active(database_path) {
            Ok(generation) => Ok(OpenCode2Inspection::Ready(generation)),
            Err(NativeOpenCode2Error::StateMissing) => Ok(OpenCode2Inspection::NotInstalled),
            Err(error) => Err(error),
        }
    }

    /// Resolves and verifies the certified active generation.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2Error`] when the platform, managed state,
    /// generation, executable path, identity, size, or hash is invalid.
    #[must_use]
    pub fn resolve_active(
        &self,
        database_path: &Path,
    ) -> Result<ResolvedOpenCode2Generation, NativeOpenCode2Error> {
        if !platform_supported() {
            return Err(NativeOpenCode2Error::UnsupportedPlatform);
        }
        let paths = self
            .install_paths(database_path)
            .map_err(map_path_authority_error)?;
        let state = self
            .read_install_state(paths.engine_root())
            .map_err(map_state_seam_error)?
            .ok_or(NativeOpenCode2Error::StateMissing)?;
        let active = &state.inner.active;
        let executable = paths
            .versions_root()
            .join(&active.directory)
            .join(&active.binary);
        let verified_file_id = native_files::verify_file(
            &executable,
            self.install_spec.executable_size_bytes(),
            self.install_spec.executable_sha256(),
        )
        .map_err(map_executable_error)?;
        Ok(ResolvedOpenCode2Generation {
            executable,
            generation_id: active.directory.clone(),
            version: self.install_spec.version(),
            upstream_commit: self.install_spec.upstream_commit(),
            executable_size_bytes: self.install_spec.executable_size_bytes(),
            executable_sha256: *self.install_spec.executable_sha256(),
            verified_file_id,
        })
    }

    /// Returns the certified engine root for an absolute database path.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2Error`] when the database path is unsafe or
    /// the derived installation root is unavailable.
    #[must_use]
    pub fn managed_engine_root(
        &self,
        database_path: &Path,
    ) -> Result<PathBuf, NativeOpenCode2Error> {
        self.install_paths(database_path)
            .map(|paths| paths.engine_root().to_path_buf())
            .map_err(map_path_authority_error)
    }

    /// Builds a validated install-state value for one exact generation.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2StateError`] when the generation or optional
    /// previous state does not satisfy the certified state specification.
    #[must_use]
    pub fn new_install_state(
        &self,
        generation_id: &str,
        previous: Option<&NativeOpenCode2State>,
    ) -> Result<NativeOpenCode2State, NativeOpenCode2StateError> {
        if let Some(previous) = previous {
            validate_install_state(&previous.inner, &self.install_spec)?;
        }
        let state = NativeOpenCode2State {
            inner: ManagedToolchainStateV1 {
                active: self.install_spec.generation(generation_id),
                format_version: 1,
                previous: previous.map(|state| state.inner.active.clone()),
            },
        };
        validate_install_state(&state.inner, &self.install_spec)?;
        Ok(state)
    }

    /// Reads and validates the bounded install-state document.
    ///
    /// A missing state file is returned as `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2StateError`] when the state path, bytes, or
    /// decoded state fails certified validation.
    #[must_use]
    pub fn read_install_state(
        &self,
        engine_root: &Path,
    ) -> Result<Option<NativeOpenCode2State>, NativeOpenCode2StateError> {
        let state_path = state_path_for_root(engine_root, self.install_spec.engine_id())?;
        let bytes = match native_files::read_bounded(&state_path, MAX_STATE_BYTES) {
            Ok(bytes) => bytes,
            Err(NativeFileError::NotFound) => return Ok(None),
            Err(error) => return Err(map_state_file_error(error)),
        };
        let inner = decode_state(&bytes).map_err(map_state_decoder_error)?;
        validate_install_state(&inner, &self.install_spec)?;
        Ok(Some(NativeOpenCode2State { inner }))
    }

    /// Encodes a validated install-state value with the shared state codec.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2StateError`] when the state is invalid, cannot
    /// be encoded, or exceeds the bounded representation.
    #[must_use]
    pub fn encode_install_state(
        &self,
        state: &NativeOpenCode2State,
    ) -> Result<Vec<u8>, NativeOpenCode2StateError> {
        validate_install_state(&state.inner, &self.install_spec)?;
        let bytes =
            serde_json::to_vec(&state.inner).map_err(|_| NativeOpenCode2StateError::Encode)?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(NativeOpenCode2StateError::TooLarge);
        }
        Ok(bytes)
    }

    /// Atomically publishes a validated install-state document.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2StateError`] when the state, destination, or
    /// atomic publication fails certified validation.
    #[must_use]
    pub fn write_install_state(
        &self,
        engine_root: &Path,
        state: &NativeOpenCode2State,
    ) -> Result<AtomicReplaceOutcome, NativeOpenCode2StateError> {
        let state_path = state_path_for_root(engine_root, self.install_spec.engine_id())?;
        let bytes = self.encode_install_state(state)?;
        native_files::replace_file(&state_path, &bytes).map_err(map_state_replace_error)
    }

    pub(crate) fn spec(&self) -> &NativeOpenCode2InstallSpec {
        &self.install_spec
    }

    #[cfg(all(test, target_os = "windows", target_arch = "x86_64"))]
    pub(crate) fn test() -> Self {
        Self {
            install_spec: NativeOpenCode2InstallSpec {
                engine_id: "opencode2",
                version: "1.2.3-test",
                upstream_commit: "test-commit",
                platform: "win32",
                architecture: "x64",
                artifact_kind: "test-artifact",
                archive_member: "package/bin/opencode2.exe",
                binary: "opencode2.exe",
                npm_integrity_sha512: "test-integrity",
                npm_url: "https://example.invalid/test.tgz",
                download_bound_bytes: 1024,
                executable_size_bytes: 15,
                executable_sha256: [
                    0xff, 0x87, 0x15, 0xf0, 0x27, 0x07, 0x31, 0xbb, 0xdb, 0x0b, 0xb3, 0x58, 0x6a,
                    0x77, 0xd0, 0x32, 0xf5, 0xe8, 0x83, 0xb8, 0x90, 0x9d, 0xca, 0xfb, 0xf3, 0xe8,
                    0x90, 0x9c, 0xb8, 0xc7, 0x12, 0x01,
                ],
                executable_sha256_hex: "ff8715f0270731bbdb0bb3586a77d032f5e883b8909dcafbf3e8909cb8c71201",
            },
        }
    }
}

const STATE_FIELDS: &[&str] = &["active", "format_version", "previous"];
const GENERATION_FIELDS: &[&str] = &["binary", "directory", "sha256", "version"];

#[derive(Clone, Serialize)]
struct ManagedToolchainStateV1 {
    active: ManagedGenerationV1,
    format_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous: Option<ManagedGenerationV1>,
}

#[derive(Clone, Serialize)]
struct ManagedGenerationV1 {
    binary: String,
    directory: String,
    sha256: String,
    version: String,
}

impl<'de> Deserialize<'de> for ManagedGenerationV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct GenerationVisitor;

        impl<'de> Visitor<'de> for GenerationVisitor {
            type Value = ManagedGenerationV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an OpenCode2 generation object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut binary = None;
                let mut directory = None;
                let mut sha256 = None;
                let mut version = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "binary" => {
                            if binary.is_some() {
                                return Err(de::Error::duplicate_field("binary"));
                            }
                            binary = Some(map.next_value()?);
                        }
                        "directory" => {
                            if directory.is_some() {
                                return Err(de::Error::duplicate_field("directory"));
                            }
                            directory = Some(map.next_value()?);
                        }
                        "sha256" => {
                            if sha256.is_some() {
                                return Err(de::Error::duplicate_field("sha256"));
                            }
                            sha256 = Some(map.next_value()?);
                        }
                        "version" => {
                            if version.is_some() {
                                return Err(de::Error::duplicate_field("version"));
                            }
                            version = Some(map.next_value()?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, GENERATION_FIELDS)),
                    }
                }
                Ok(ManagedGenerationV1 {
                    binary: binary.ok_or_else(|| de::Error::missing_field("binary"))?,
                    directory: directory.ok_or_else(|| de::Error::missing_field("directory"))?,
                    sha256: sha256.ok_or_else(|| de::Error::missing_field("sha256"))?,
                    version: version.ok_or_else(|| de::Error::missing_field("version"))?,
                })
            }
        }
        deserializer.deserialize_map(GenerationVisitor)
    }
}

impl<'de> Deserialize<'de> for ManagedToolchainStateV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StateVisitor;

        impl<'de> Visitor<'de> for StateVisitor {
            type Value = ManagedToolchainStateV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an OpenCode2 managed state object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut active = None;
                let mut format_version = None;
                let mut previous = None;
                let mut previous_seen = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "active" => {
                            if active.is_some() {
                                return Err(de::Error::duplicate_field("active"));
                            }
                            active = Some(map.next_value()?);
                        }
                        "format_version" => {
                            if format_version.is_some() {
                                return Err(de::Error::duplicate_field("format_version"));
                            }
                            format_version = Some(map.next_value()?);
                        }
                        "previous" => {
                            if previous_seen {
                                return Err(de::Error::duplicate_field("previous"));
                            }
                            previous_seen = true;
                            let value: Option<ManagedGenerationV1> = map.next_value()?;
                            previous =
                                Some(value.ok_or_else(|| {
                                    de::Error::custom("previous must be an object")
                                })?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, STATE_FIELDS)),
                    }
                }
                Ok(ManagedToolchainStateV1 {
                    active: active.ok_or_else(|| de::Error::missing_field("active"))?,
                    format_version: format_version
                        .ok_or_else(|| de::Error::missing_field("format_version"))?,
                    previous,
                })
            }
        }
        deserializer.deserialize_map(StateVisitor)
    }
}

/// Returns whether the certified OpenCode2 executable is supported here.
#[must_use]
pub const fn platform_supported() -> bool {
    cfg!(all(target_os = "windows", target_arch = "x86_64"))
}

fn state_path_for_root(
    engine_root: &Path,
    engine_id: &str,
) -> Result<PathBuf, NativeOpenCode2StateError> {
    let expected_suffix = Path::new("toolchain").join(engine_id);
    if !engine_root.is_absolute()
        || engine_root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        || !engine_root.ends_with(expected_suffix)
    {
        return Err(NativeOpenCode2StateError::InvalidRoot);
    }
    Ok(engine_root.join("state.json"))
}

fn decode_state(bytes: &[u8]) -> Result<ManagedToolchainStateV1, NativeOpenCode2Error> {
    if bytes.len() > MAX_STATE_BYTES {
        return Err(NativeOpenCode2Error::StateTooLarge);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let state = ManagedToolchainStateV1::deserialize(&mut deserializer)
        .map_err(|_| NativeOpenCode2Error::StateMalformed)?;
    deserializer
        .end()
        .map_err(|_| NativeOpenCode2Error::StateMalformed)?;
    Ok(state)
}

fn validate_install_state(
    state: &ManagedToolchainStateV1,
    spec: &NativeOpenCode2InstallSpec,
) -> Result<(), NativeOpenCode2StateError> {
    validate_state_version(state).map_err(map_state_validation_error)?;
    validate_generation(&state.active, true, spec).map_err(map_state_validation_error)?;
    if let Some(previous) = state.previous.as_ref() {
        validate_generation(previous, false, spec).map_err(map_state_validation_error)?;
    }
    Ok(())
}

fn validate_state_version(state: &ManagedToolchainStateV1) -> Result<(), NativeOpenCode2Error> {
    if state.format_version != 1 {
        return Err(NativeOpenCode2Error::StateUnsupportedVersion);
    }
    Ok(())
}

fn validate_generation(
    generation: &ManagedGenerationV1,
    active: bool,
    spec: &NativeOpenCode2InstallSpec,
) -> Result<(), NativeOpenCode2Error> {
    if !is_safe_basename(&generation.directory, MAX_GENERATION_ID_BYTES)
        || !is_safe_relative_path(&generation.binary, MAX_BINARY_PATH_BYTES)
    {
        return Err(NativeOpenCode2Error::UnsafePath);
    }
    if !is_safe_sha256(&generation.sha256) || !is_safe_version(&generation.version) {
        return Err(NativeOpenCode2Error::ActiveGenerationUntrusted);
    }
    if active
        && (generation.binary != spec.binary()
            || generation.version != spec.version()
            || generation.sha256 != spec.executable_sha256_hex())
    {
        return Err(NativeOpenCode2Error::ActiveGenerationUntrusted);
    }
    Ok(())
}

fn is_safe_basename(value: &str, maximum_bytes: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= maximum_bytes
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
}

fn is_safe_relative_path(value: &str, maximum_bytes: usize) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > maximum_bytes
        || !bytes[0].is_ascii_alphanumeric()
        || bytes.iter().any(|byte| {
            !byte.is_ascii_alphanumeric() && !matches!(*byte, b'.' | b'_' | b'-' | b'/' | b'\\')
        })
    {
        return false;
    }
    let mut segment_start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(*byte, b'/' | b'\\') {
            if index == segment_start || &bytes[segment_start..index] == b".." {
                return false;
            }
            segment_start = index + 1;
        }
    }
    segment_start < bytes.len() && &bytes[segment_start..] != b".."
}

fn is_safe_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_safe_version(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_VERSION_BYTES || !value.is_ascii() {
        return false;
    }
    let (release, prerelease) = value
        .split_once('-')
        .map_or((value, None), |(release, pre)| (release, Some(pre)));
    let mut release_parts = release.split('.');
    for _ in 0..3 {
        let Some(part) = release_parts.next() else {
            return false;
        };
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
    }
    if release_parts.next().is_some() {
        return false;
    }
    prerelease.is_none_or(|pre| {
        !pre.is_empty()
            && pre
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    })
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

fn map_path_lock_error(error: NativeOpenCode2InstallPathError) -> NativeOpenCode2InstallLockError {
    match error {
        NativeOpenCode2InstallPathError::InvalidRoot => {
            NativeOpenCode2InstallLockError::InvalidRoot
        }
        NativeOpenCode2InstallPathError::Unavailable => {
            NativeOpenCode2InstallLockError::Unavailable
        }
    }
}

fn map_path_authority_error(error: NativeOpenCode2InstallPathError) -> NativeOpenCode2Error {
    match error {
        NativeOpenCode2InstallPathError::InvalidRoot => NativeOpenCode2Error::UnsafePath,
        NativeOpenCode2InstallPathError::Unavailable => NativeOpenCode2Error::Io,
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

fn map_state_file_error(error: NativeFileError) -> NativeOpenCode2StateError {
    match error {
        NativeFileError::TooLarge => NativeOpenCode2StateError::TooLarge,
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            NativeOpenCode2StateError::UnsafePath
        }
        NativeFileError::NotFound
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => NativeOpenCode2StateError::Io,
    }
}

fn map_state_decoder_error(error: NativeOpenCode2Error) -> NativeOpenCode2StateError {
    match error {
        NativeOpenCode2Error::StateTooLarge => NativeOpenCode2StateError::TooLarge,
        NativeOpenCode2Error::StateMalformed => NativeOpenCode2StateError::Malformed,
        other => map_state_validation_error(other),
    }
}

fn map_state_validation_error(error: NativeOpenCode2Error) -> NativeOpenCode2StateError {
    match error {
        NativeOpenCode2Error::StateUnsupportedVersion => {
            NativeOpenCode2StateError::UnsupportedVersion
        }
        NativeOpenCode2Error::UnsafePath => NativeOpenCode2StateError::UnsafePath,
        NativeOpenCode2Error::ActiveGenerationUntrusted => {
            NativeOpenCode2StateError::ActiveGenerationUntrusted
        }
        NativeOpenCode2Error::StateTooLarge => NativeOpenCode2StateError::TooLarge,
        NativeOpenCode2Error::StateMalformed => NativeOpenCode2StateError::Malformed,
        NativeOpenCode2Error::UnsupportedPlatform
        | NativeOpenCode2Error::StateMissing
        | NativeOpenCode2Error::ExecutableUnavailable
        | NativeOpenCode2Error::ExecutableChanged
        | NativeOpenCode2Error::ExecutableSizeMismatch
        | NativeOpenCode2Error::ExecutableHashMismatch
        | NativeOpenCode2Error::Io => NativeOpenCode2StateError::Malformed,
    }
}

fn map_state_seam_error(error: NativeOpenCode2StateError) -> NativeOpenCode2Error {
    match error {
        NativeOpenCode2StateError::InvalidRoot | NativeOpenCode2StateError::UnsafePath => {
            NativeOpenCode2Error::UnsafePath
        }
        NativeOpenCode2StateError::TooLarge => NativeOpenCode2Error::StateTooLarge,
        NativeOpenCode2StateError::Malformed => NativeOpenCode2Error::StateMalformed,
        NativeOpenCode2StateError::UnsupportedVersion => {
            NativeOpenCode2Error::StateUnsupportedVersion
        }
        NativeOpenCode2StateError::ActiveGenerationUntrusted => {
            NativeOpenCode2Error::ActiveGenerationUntrusted
        }
        NativeOpenCode2StateError::Io
        | NativeOpenCode2StateError::Encode
        | NativeOpenCode2StateError::AtomicPublishFailed => NativeOpenCode2Error::Io,
    }
}

fn map_state_replace_error(error: NativeFileError) -> NativeOpenCode2StateError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            NativeOpenCode2StateError::UnsafePath
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => NativeOpenCode2StateError::AtomicPublishFailed,
    }
}

fn map_executable_error(error: NativeFileError) -> NativeOpenCode2Error {
    match error {
        NativeFileError::NotFound => NativeOpenCode2Error::ExecutableUnavailable,
        NativeFileError::TooLarge | NativeFileError::FileSizeMismatch => {
            NativeOpenCode2Error::ExecutableSizeMismatch
        }
        NativeFileError::FileChanged => NativeOpenCode2Error::ExecutableChanged,
        NativeFileError::FileHashMismatch => NativeOpenCode2Error::ExecutableHashMismatch,
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            NativeOpenCode2Error::UnsafePath
        }
        NativeFileError::Io => NativeOpenCode2Error::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_contention_predicate_uses_canonical_identity_and_fails_closed() {
        assert!(is_lock_contended(&fs2::lock_contended_error()));
        assert!(is_lock_contended(&io::Error::from(
            io::ErrorKind::WouldBlock
        )));
        assert!(!is_lock_contended(&io::Error::from(io::ErrorKind::Other)));
    }

    #[test]
    fn certified_install_spec_is_fixed_and_diagnostic_free() {
        let authority = NativeOpenCode2Authority::new();
        let spec = NativeOpenCode2Authority::certified_install_spec();
        assert_eq!(spec.engine_id(), "opencode2");
        assert_eq!(spec.version(), "0.0.0-beta-17778");
        assert_eq!(
            spec.upstream_commit(),
            "0d2684b67308380fc47540fe55deb55306a08e3f"
        );
        assert_eq!(spec.platform(), "win32");
        assert_eq!(spec.architecture(), "x64");
        assert_eq!(spec.artifact_kind(), "npm-tarball");
        assert_eq!(spec.archive_member(), "package/bin/opencode2.exe");
        assert_eq!(spec.binary(), "opencode2.exe");
        assert_eq!(spec.download_bound_bytes(), 268_435_456);
        assert_eq!(spec.executable_size_bytes(), 144_313_344);
        assert_eq!(
            spec.executable_sha256_hex(),
            "452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf"
        );
        assert_eq!(
            spec.npm_integrity_sha512(),
            "Z0oMvTBUhxmz1IYuQSMOZTpI2HoWjeIjdxJ39SoGrhDwvJZK7OI0rgIMYtDGavOucOQT8oxrazUiO4j+2hVMpw=="
        );
        assert_eq!(
            spec.npm_url(),
            "https://registry.npmjs.org/@opencode-ai/cli-windows-x64/-/cli-windows-x64-0.0.0-beta-17778.tgz"
        );
        assert_eq!(
            spec.executable_sha256(),
            &[
                0x45, 0x27, 0x94, 0xa7, 0x64, 0xe1, 0x03, 0x3e, 0x62, 0x9c, 0x4c, 0xd4, 0x0b, 0xde,
                0x64, 0x33, 0xc1, 0x0c, 0x6b, 0xd3, 0x24, 0x33, 0xfb, 0x3b, 0xe2, 0x79, 0xbf, 0x03,
                0x96, 0x9a, 0x6e, 0xdf,
            ]
        );
        assert_eq!(authority.spec().engine_id(), spec.engine_id());
        assert!(!format!("{authority:?}").contains("registry.npmjs.org"));
        assert!(!format!("{spec:?}").contains("opencode2.exe"));
    }

    #[test]
    fn install_state_codec_rejects_syntax_errors_and_full_validation_rejects_unsafe_values() {
        let active = r#"{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"}"#;
        let valid = format!(r#"{{"active":{active},"format_version":1}}"#);
        assert!(decode_state(valid.as_bytes()).is_ok());
        for malformed in [
            format!(r#"{{"active":{active},"format_version":1,"extra":true}}"#),
            format!(r#"{{"active":{active},"active":{active},"format_version":1}}"#),
            format!(r#"{{"active":{active},"format_version":1}} trailing"#),
        ] {
            assert!(matches!(
                decode_state(malformed.as_bytes()),
                Err(NativeOpenCode2Error::StateMalformed)
            ));
        }

        let authority = NativeOpenCode2Authority::new();
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("opencode2");
        fs::create_dir_all(&engine_root).unwrap();
        let state_path = engine_root.join("state.json");

        fs::write(
            &state_path,
            format!(r#"{{"active":{active},"format_version":2}}"#),
        )
        .unwrap();
        assert!(matches!(
            authority.read_install_state(&engine_root),
            Err(NativeOpenCode2StateError::UnsupportedVersion)
        ));

        fs::write(
            &state_path,
            r#"{"active":{"binary":"opencode2.exe","directory":"../escape","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#,
        )
        .unwrap();
        assert!(matches!(
            authority.read_install_state(&engine_root),
            Err(NativeOpenCode2StateError::UnsafePath)
        ));
    }

    #[test]
    fn state_decoder_accepts_active_with_or_without_previous() {
        let active = r#"{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"}"#;
        let without_previous = format!(r#"{{"active":{active},"format_version":1}}"#);
        let state = decode_state(without_previous.as_bytes()).unwrap();
        assert_eq!(state.format_version, 1);
        assert!(state.previous.is_none());
        assert_eq!(state.active.directory, "generation-a");

        let previous = r#"{"binary":"old.exe","directory":"generation-old","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","version":"1.2.3"}"#;
        let with_previous =
            format!(r#"{{"active":{active},"format_version":1,"previous":{previous}}}"#);
        let state = decode_state(with_previous.as_bytes()).unwrap();
        assert_eq!(state.previous.as_ref().unwrap().directory, "generation-old");
        let spec = NativeOpenCode2Authority::certified_install_spec();
        validate_generation(&state.active, true, &spec).unwrap();
        validate_generation(state.previous.as_ref().unwrap(), false, &spec).unwrap();
    }

    #[test]
    fn state_decoder_rejects_malformed_duplicate_unknown_trailing_and_null_previous() {
        let valid = r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#;
        for malformed in [
            r#"{"active":{}}"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1,"extra":true}"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1,"format_version":1}"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1} trailing"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1,"previous":null}"#,
        ] {
            assert!(matches!(
                decode_state(malformed.as_bytes()),
                Err(NativeOpenCode2Error::StateMalformed)
            ));
        }
        assert!(decode_state(valid.as_bytes()).is_ok());
        assert!(matches!(
            decode_state(&[0xff, 0xfe]),
            Err(NativeOpenCode2Error::StateMalformed)
        ));
    }

    #[test]
    fn state_decoder_rejects_unsupported_format_and_oversized_bytes() {
        let valid = r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":2}"#;
        let state = decode_state(valid.as_bytes()).unwrap();
        assert_eq!(state.format_version, 2);
        assert_eq!(
            validate_state_version(&state),
            Err(NativeOpenCode2Error::StateUnsupportedVersion)
        );
        assert!(matches!(
            decode_state(&vec![b' '; MAX_STATE_BYTES + 1]),
            Err(NativeOpenCode2Error::StateTooLarge)
        ));
    }

    #[test]
    fn install_state_constructor_retains_only_the_previous_active_generation() {
        let authority = NativeOpenCode2Authority::new();
        let first = authority.new_install_state("generation-a", None).unwrap();
        let second = authority
            .new_install_state("generation-b", Some(&first))
            .unwrap();

        assert_eq!(second.inner.active.directory, "generation-b");
        assert_eq!(
            second.inner.previous.as_ref().unwrap().directory,
            "generation-a"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn install_state_round_trips_without_serializing_null_previous() {
        let authority = NativeOpenCode2Authority::new();
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("opencode2");
        fs::create_dir_all(&engine_root).unwrap();

        let first = authority.new_install_state("generation-a", None).unwrap();
        let first_bytes = authority.encode_install_state(&first).unwrap();
        let first_text = String::from_utf8(first_bytes.clone()).unwrap();
        assert!(!first_text.contains("previous"));
        assert!(!first_text.contains("null"));
        assert!(matches!(
            authority.write_install_state(&engine_root, &first),
            Ok(AtomicReplaceOutcome::Committed)
        ));
        assert_eq!(fs::read_dir(&engine_root).unwrap().count(), 1);

        let first_read = authority.read_install_state(&engine_root).unwrap().unwrap();
        assert_eq!(
            authority.encode_install_state(&first_read).unwrap(),
            first_bytes
        );

        let second = authority
            .new_install_state("generation-b", Some(&first_read))
            .unwrap();
        let second_bytes = authority.encode_install_state(&second).unwrap();
        let second_text = String::from_utf8(second_bytes.clone()).unwrap();
        assert!(second_text.contains("\"previous\""));
        assert!(!second_text.contains("\"previous\":null"));
        assert!(matches!(
            authority.write_install_state(&engine_root, &second),
            Ok(AtomicReplaceOutcome::Committed)
        ));
        assert_eq!(fs::read_dir(&engine_root).unwrap().count(), 1);

        let second_read = authority.read_install_state(&engine_root).unwrap().unwrap();
        assert_eq!(second_read.inner.active.directory, "generation-b");
        assert_eq!(
            second_read.inner.previous.as_ref().unwrap().directory,
            "generation-a"
        );
        assert_eq!(
            authority.encode_install_state(&second_read).unwrap(),
            second_bytes
        );
    }

    #[test]
    fn install_state_validation_is_opaque_and_path_free() {
        let authority = NativeOpenCode2Authority::new();
        assert!(matches!(
            authority.read_install_state(Path::new("toolchain/opencode2")),
            Err(NativeOpenCode2StateError::InvalidRoot)
        ));

        let unsupported = NativeOpenCode2State {
            inner: ManagedToolchainStateV1 {
                active: certified_generation("generation-a"),
                format_version: 2,
                previous: None,
            },
        };
        assert_eq!(
            authority.encode_install_state(&unsupported),
            Err(NativeOpenCode2StateError::UnsupportedVersion)
        );

        let malformed = NativeOpenCode2State {
            inner: ManagedToolchainStateV1 {
                active: ManagedGenerationV1 {
                    binary: "opencode2.exe".into(),
                    directory: "generation-a".into(),
                    sha256: "not-a-digest".into(),
                    version: "0.0.0-beta-17778".into(),
                },
                format_version: 1,
                previous: None,
            },
        };
        assert_eq!(
            authority.encode_install_state(&malformed),
            Err(NativeOpenCode2StateError::ActiveGenerationUntrusted)
        );
        assert!(!format!("{}", NativeOpenCode2StateError::UnsafePath).contains("toolchain"));
    }

    #[test]
    fn unsafe_generation_binary_version_and_digest_values_fail_closed() {
        assert!(!is_safe_basename("../generation", MAX_GENERATION_ID_BYTES));
        assert!(!is_safe_relative_path(
            "../opencode2.exe",
            MAX_BINARY_PATH_BYTES
        ));
        assert!(!is_safe_relative_path(
            "C:\\opencode2.exe",
            MAX_BINARY_PATH_BYTES
        ));
        assert!(!is_safe_relative_path(
            "nested//opencode2.exe",
            MAX_BINARY_PATH_BYTES
        ));
        assert!(!is_safe_sha256(&"A".repeat(64)));
        assert!(!is_safe_version("0.0"));
        let mut active = certified_generation("generation-a");
        active.binary = "nested/opencode2.exe".into();
        assert_eq!(
            validate_generation(
                &active,
                true,
                &NativeOpenCode2Authority::certified_install_spec(),
            ),
            Err(NativeOpenCode2Error::ActiveGenerationUntrusted)
        );
        active = certified_generation("generation-a");
        active.directory = "../generation-a".into();
        assert_eq!(
            validate_generation(
                &active,
                true,
                &NativeOpenCode2Authority::certified_install_spec(),
            ),
            Err(NativeOpenCode2Error::UnsafePath)
        );
        active = certified_generation("generation-a");
        active.version = "1.2.3".into();
        assert_eq!(
            validate_generation(
                &active,
                true,
                &NativeOpenCode2Authority::certified_install_spec(),
            ),
            Err(NativeOpenCode2Error::ActiveGenerationUntrusted)
        );
        active = certified_generation("generation-a");
        active.sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into();
        assert_eq!(
            validate_generation(
                &active,
                true,
                &NativeOpenCode2Authority::certified_install_spec(),
            ),
            Err(NativeOpenCode2Error::ActiveGenerationUntrusted)
        );
    }

    #[test]
    fn authority_errors_are_payload_free() {
        let errors = [
            NativeOpenCode2Error::UnsupportedPlatform,
            NativeOpenCode2Error::StateMissing,
            NativeOpenCode2Error::StateTooLarge,
            NativeOpenCode2Error::StateMalformed,
            NativeOpenCode2Error::StateUnsupportedVersion,
            NativeOpenCode2Error::ActiveGenerationUntrusted,
            NativeOpenCode2Error::UnsafePath,
            NativeOpenCode2Error::ExecutableUnavailable,
            NativeOpenCode2Error::ExecutableChanged,
            NativeOpenCode2Error::ExecutableSizeMismatch,
            NativeOpenCode2Error::ExecutableHashMismatch,
            NativeOpenCode2Error::Io,
        ];
        for error in errors {
            assert!(!error.to_string().contains("C:\\secret"));
            assert!(!format!("{error:?}").contains("profiles.json"));
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn authority_and_resolved_debug_are_redacted() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("opencode2.exe");
        let bytes = b"debug fixture";
        fs::write(&path, bytes).unwrap();
        let identity =
            native_files::verify_file(&path, bytes.len() as u64, &digest_array(bytes)).unwrap();
        let generation = ResolvedOpenCode2Generation {
            executable: PathBuf::from("C:\\secret\\opencode2.exe"),
            generation_id: "generation-a".into(),
            version: CERTIFIED_VERSION,
            upstream_commit: CERTIFIED_UPSTREAM_COMMIT,
            executable_size_bytes: CERTIFIED_EXECUTABLE_SIZE_BYTES,
            executable_sha256: CERTIFIED_EXECUTABLE_SHA256,
            verified_file_id: identity,
        };
        let debug = format!("{generation:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains(CERTIFIED_EXECUTABLE_SHA256_HEX));
        assert!(!debug.contains("volume"));
        assert!(!debug.contains("dev"));
    }

    #[test]
    fn exact_database_parent_toolchain_root_is_used() {
        let root = tempfile::tempdir().unwrap();
        let database_parent = root.path().join("data");
        fs::create_dir(&database_parent).unwrap();
        let database = database_parent.join("artisan.sqlite");
        let spec = NativeOpenCode2Authority::certified_install_spec();
        let paths = NativeOpenCode2InstallPaths::derive(&database, &spec).unwrap();
        assert_eq!(
            paths.engine_root(),
            database_parent.join("toolchain").join("opencode2")
        );
        assert_eq!(
            paths.versions_root(),
            database_parent
                .join("toolchain")
                .join("opencode2")
                .join("versions")
        );

        let traversal = root
            .path()
            .join("data")
            .join("..")
            .join("other")
            .join("db.sqlite");
        assert!(matches!(
            NativeOpenCode2InstallPaths::derive(&traversal, &spec),
            Err(NativeOpenCode2InstallPathError::InvalidRoot)
        ));
    }

    #[test]
    fn missing_state_is_not_a_fallback_to_previous() {
        let state = NativeOpenCode2State {
            inner: ManagedToolchainStateV1 {
                active: certified_generation("active-generation"),
                format_version: 1,
                previous: Some(certified_generation("previous-generation")),
            },
        };
        assert_eq!(state.inner.active.directory, "active-generation");
        assert_eq!(
            state.inner.previous.as_ref().unwrap().directory,
            "previous-generation"
        );

        let root = tempfile::tempdir().unwrap();
        let database = database_path(root.path());
        let authority = NativeOpenCode2Authority::new();
        let paths = authority.install_paths(&database).unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Ok(None)
        ));
        assert!(!platform_supported() || authority.resolve_active(&database).is_err());
    }

    #[test]
    fn active_generation_is_the_only_generation_path_candidate() {
        let root = tempfile::tempdir().unwrap();
        let paths = NativeOpenCode2Authority::new()
            .install_paths(&database_path(root.path()))
            .unwrap();
        let state = NativeOpenCode2State {
            inner: ManagedToolchainStateV1 {
                active: certified_generation("active-generation"),
                format_version: 1,
                previous: Some(certified_generation("previous-generation")),
            },
        };
        let active_path = paths
            .versions_root()
            .join(&state.inner.active.directory)
            .join(&state.inner.active.binary);
        assert!(active_path.ends_with("active-generation/opencode2.exe"));
        assert!(!active_path.ends_with("previous-generation/opencode2.exe"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn replaced_executable_identity_cannot_reuse_the_old_trust() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("opencode2.exe");
        let original = b"original binary";
        let replacement = b"replaced binary";
        assert_eq!(original.len(), replacement.len());
        fs::write(&path, original).unwrap();
        let original_hash = digest_array(original);
        let original_id =
            native_files::verify_file(&path, original.len() as u64, &original_hash).unwrap();

        let backup = root.path().join("opencode2.old");
        fs::rename(&path, &backup).unwrap();
        fs::write(&path, replacement).unwrap();
        let replacement_hash = digest_array(replacement);
        let replacement_id =
            native_files::verify_file(&path, replacement.len() as u64, &replacement_hash).unwrap();
        assert_ne!(original_id, replacement_id);
        assert_eq!(
            native_files::verify_file(&path, original.len() as u64, &original_hash),
            Err(NativeFileError::FileHashMismatch)
        );
    }

    #[test]
    fn unsupported_platform_does_not_read_managed_state() {
        let root = tempfile::tempdir().unwrap();
        let inspection = NativeOpenCode2Authority::new().inspect(&database_path(root.path()));
        if !platform_supported() {
            assert!(matches!(
                inspection,
                Ok(OpenCode2Inspection::UnsupportedPlatform)
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn state_and_executable_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let database = database_path(root.path());
        let authority = NativeOpenCode2Authority::new();
        let paths = authority.install_paths(&database).unwrap();
        fs::create_dir_all(paths.engine_root()).unwrap();
        let real_state = root.path().join("real-state.json");
        fs::write(&real_state, b"{}").unwrap();
        symlink(&real_state, paths.engine_root().join("state.json")).unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Err(NativeOpenCode2StateError::UnsafePath)
        ));

        let executable = root.path().join("executable.exe");
        fs::write(&executable, b"native").unwrap();
        let link = root.path().join("executable-link.exe");
        symlink(&executable, &link).unwrap();
        let expected = digest_array(b"native");
        assert!(matches!(
            native_files::verify_file(&link, 6, &expected),
            Err(NativeFileError::UnsafePath)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlinks_are_rejected_before_state_read() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        fs::create_dir(&data).unwrap();
        let database = data.join("artisan.sqlite");
        let authority = NativeOpenCode2Authority::new();
        let paths = authority.install_paths(&database).unwrap();
        let real_data = root.path().join("real-data");
        fs::create_dir_all(real_data.join("toolchain").join("opencode2")).unwrap();
        fs::remove_dir(&data).unwrap();
        symlink(&real_data, &data).unwrap();
        let state = br#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#;
        fs::write(paths.engine_root().join("state.json"), state).unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Err(NativeOpenCode2StateError::UnsafePath)
        ));
    }

    #[test]
    fn bounded_state_reader_never_returns_bytes_above_its_limit() {
        let root = tempfile::tempdir().unwrap();
        let authority = NativeOpenCode2Authority::new();
        let paths = authority
            .install_paths(&database_path(root.path()))
            .unwrap();
        fs::create_dir_all(paths.engine_root()).unwrap();
        fs::write(
            paths.engine_root().join("state.json"),
            vec![b'x'; MAX_STATE_BYTES + 1],
        )
        .unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Err(NativeOpenCode2StateError::TooLarge)
        ));
    }

    #[test]
    fn native_file_verification_rejects_size_and_hash_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("opencode2.exe");
        let bytes = b"native executable fixture";
        fs::write(&path, bytes).unwrap();
        let expected = digest_array(bytes);
        assert!(native_files::verify_file(&path, (bytes.len() + 1) as u64, &expected).is_err());
        assert_eq!(
            native_files::verify_file(&path, bytes.len() as u64, &[0; 32]),
            Err(NativeFileError::FileHashMismatch)
        );
        assert!(native_files::verify_file(&path, bytes.len() as u64, &expected).is_ok());
    }

    #[test]
    fn state_bytes_remain_unchanged_after_bounded_read() {
        let root = tempfile::tempdir().unwrap();
        let authority = NativeOpenCode2Authority::new();
        let paths = authority
            .install_paths(&database_path(root.path()))
            .unwrap();
        fs::create_dir_all(paths.engine_root()).unwrap();
        let state = br#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#;
        let state_path = paths.engine_root().join("state.json");
        fs::write(&state_path, state).unwrap();
        let before = fs::read(&state_path).unwrap();
        let _ = authority.read_install_state(paths.engine_root()).unwrap();
        let after = fs::read(&state_path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn a_different_engine_root_is_never_considered() {
        let root = tempfile::tempdir().unwrap();
        let authority = NativeOpenCode2Authority::new();
        let other_state = root
            .path()
            .join("data")
            .join("toolchain")
            .join("other-engine")
            .join("state.json");
        fs::create_dir_all(other_state.parent().unwrap()).unwrap();
        fs::write(&other_state, b"not OpenCode2 state").unwrap();
        let paths = authority
            .install_paths(&database_path(root.path()))
            .unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Ok(None)
        ));
    }

    fn certified_generation(directory: &str) -> ManagedGenerationV1 {
        NativeOpenCode2Authority::certified_install_spec().generation(directory)
    }

    fn database_path(root: &Path) -> PathBuf {
        root.join("artisan.sqlite")
    }

    fn digest_array(bytes: &[u8]) -> [u8; 32] {
        use sha2::{Digest, Sha256};

        let digest = Sha256::digest(bytes);
        let mut result = [0_u8; 32];
        result.copy_from_slice(&digest);
        result
    }
}
