//! Managed engine inspection and executable-resolution façade.
//!
//! Composes the catalog, the installation paths and locks, the validated
//! install-state codec, and the native file-verification seam into the single
//! authority every launch resolves through. There is no discovery here: an
//! engine is either the verified active generation of its managed install or
//! unavailable.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
    time::SystemTime,
};

use crate::io as native_files;
use crate::io::{AtomicReplaceOutcome, NativeFileError, VerifiedFileIdentity};

use super::{
    catalog::{ArtifactPlan, Distribution, HostPlatform, ManagedEngine, UnsupportedReason},
    selection::{EngineSelection, read_selection, write_selection},
    spec::{
        ManagedInstallLock, ManagedInstallLockError, ManagedInstallPathError, ManagedInstallPaths,
        map_path_lock_error,
    },
    state::{
        MAX_STATE_BYTES, ManagedEngineError, ManagedGeneration, ManagedStateError,
        ManagedToolchainState, decode_state, map_state_decoder_error, map_state_file_error,
        map_state_replace_error, map_state_seam_error, state_path_for_root, validate_install_state,
    },
    version::EngineVersion,
};

/// The result of inspecting one managed engine.
#[must_use = "inspection results contain the managed generation decision"]
#[derive(Debug)]
pub enum EngineInspection {
    UnsupportedPlatform(UnsupportedReason),
    NotInstalled,
    Ready(Box<ResolvedGeneration>),
}

/// A managed generation whose executable identity, size, and hash were
/// verified together against its install record.
#[must_use = "retain the verified generation for the protected launch"]
pub struct ResolvedGeneration {
    pub(crate) engine: ManagedEngine,
    pub(crate) executable: PathBuf,
    pub(crate) generation_root: PathBuf,
    pub(crate) generation_id: String,
    pub(crate) version: EngineVersion,
    pub(crate) executable_size_bytes: u64,
    pub(crate) executable_sha256: [u8; 32],
    pub(crate) tool_dirs: Vec<PathBuf>,
    pub(crate) verified_file_id: VerifiedFileIdentity,
}

impl fmt::Debug for ResolvedGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedGeneration")
            .field("engine", &self.engine)
            .finish_non_exhaustive()
    }
}

impl ResolvedGeneration {
    /// Returns the engine this generation belongs to.
    #[must_use]
    pub const fn engine(&self) -> ManagedEngine {
        self.engine
    }

    /// Returns the verified executable path.
    #[must_use]
    pub fn executable_path(&self) -> &Path {
        &self.executable
    }

    /// Returns the generation directory.
    #[must_use]
    pub fn generation_root(&self) -> &Path {
        &self.generation_root
    }

    /// Returns the generation identifier.
    #[must_use]
    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    /// Returns the installed release version.
    #[must_use]
    pub fn version(&self) -> &EngineVersion {
        &self.version
    }

    /// Returns the verified executable size in bytes.
    #[must_use]
    pub fn executable_size_bytes(&self) -> u64 {
        self.executable_size_bytes
    }

    /// Returns the verified executable SHA-256 digest.
    #[must_use]
    pub fn executable_sha256(&self) -> &[u8; 32] {
        &self.executable_sha256
    }

    /// Returns the generation's tool directories for the engine `PATH`.
    #[must_use]
    pub fn tool_dirs(&self) -> &[PathBuf] {
        &self.tool_dirs
    }

    pub(crate) const fn file_identity(&self) -> VerifiedFileIdentity {
        self.verified_file_id
    }
}

/// The engine-agnostic authority over one engine's managed installation.
#[must_use = "use the authority for managed engine operations"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagedEngineAuthority {
    engine: ManagedEngine,
    platform: HostPlatform,
}

impl ManagedEngineAuthority {
    /// Constructs the authority for `engine` on this host.
    pub fn new(engine: ManagedEngine) -> Self {
        Self::for_platform(engine, HostPlatform::current())
    }

    /// Constructs the authority for `engine` on an explicit platform.
    pub const fn for_platform(engine: ManagedEngine, platform: HostPlatform) -> Self {
        Self { engine, platform }
    }

    /// Returns the managed engine.
    #[must_use]
    pub const fn engine(&self) -> ManagedEngine {
        self.engine
    }

    /// Returns the platform artifacts are resolved for.
    #[must_use]
    pub const fn platform(&self) -> HostPlatform {
        self.platform
    }

    /// Returns the artifact plan, or why the engine is unsupported here.
    ///
    /// # Errors
    ///
    /// Returns the [`UnsupportedReason`] for an unsupported platform.
    pub const fn plan(&self) -> Result<ArtifactPlan, UnsupportedReason> {
        match self.engine.distribution(self.platform) {
            Distribution::Supported(plan) => Ok(plan),
            Distribution::Unsupported(reason) => Err(reason),
        }
    }

    /// Derives the installation paths for an absolute database path.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallPathError`] when the database path is unsafe.
    pub fn install_paths(
        &self,
        database_path: &Path,
    ) -> Result<ManagedInstallPaths, ManagedInstallPathError> {
        ManagedInstallPaths::derive(database_path, self.engine)
    }

    /// Acquires and fences the exclusive installation lock.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallLockError`] when the root or lock is unsafe,
    /// unavailable, changed, or busy beyond the timeout.
    pub fn acquire_install_lock(
        &self,
        database_path: &Path,
    ) -> Result<ManagedInstallLock, ManagedInstallLockError> {
        let paths = self
            .install_paths(database_path)
            .map_err(map_path_lock_error)?;
        ManagedInstallLock::acquire(&paths)
    }

    /// Inspects the installation without installing or discovering anything.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when managed state or its active
    /// executable fails validation.
    pub fn inspect(&self, database_path: &Path) -> Result<EngineInspection, ManagedEngineError> {
        if let Err(reason) = self.plan() {
            return Ok(EngineInspection::UnsupportedPlatform(reason));
        }
        match self.resolve_active(database_path) {
            Ok(generation) => Ok(EngineInspection::Ready(Box::new(generation))),
            Err(ManagedEngineError::StateMissing) => Ok(EngineInspection::NotInstalled),
            Err(error) => Err(error),
        }
    }

    /// Resolves and verifies the active generation.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the platform, managed state,
    /// generation, executable path, identity, size, or hash is invalid.
    pub fn resolve_active(
        &self,
        database_path: &Path,
    ) -> Result<ResolvedGeneration, ManagedEngineError> {
        let plan = self
            .plan()
            .map_err(|_| ManagedEngineError::UnsupportedPlatform)?;
        let paths = self
            .install_paths(database_path)
            .map_err(map_path_authority_error)?;
        let state = self
            .read_install_state(paths.engine_root())
            .map_err(map_state_seam_error)?
            .ok_or(ManagedEngineError::StateMissing)?;
        self.verify_generation(&paths, &plan, &state.active)
    }

    /// Verifies one recorded generation on disk.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the generation's executable is
    /// missing, unsafe, changed, or does not match its record.
    pub fn verify_generation(
        &self,
        paths: &ManagedInstallPaths,
        plan: &ArtifactPlan,
        generation: &ManagedGeneration,
    ) -> Result<ResolvedGeneration, ManagedEngineError> {
        let version = generation
            .parsed_version()
            .ok_or(ManagedEngineError::ActiveGenerationUntrusted)?;
        let sha256 = decode_sha256(&generation.sha256)
            .ok_or(ManagedEngineError::ActiveGenerationUntrusted)?;
        let generation_root = paths.versions_root().join(&generation.directory);
        let executable = generation_root.join(&generation.binary);
        let size = match generation.size {
            Some(size) => size,
            None => std::fs::symlink_metadata(&executable)
                .map_err(|_| ManagedEngineError::ExecutableUnavailable)?
                .len(),
        };
        let verified_file_id =
            verify_cached(&executable, size, &sha256).map_err(map_executable_error)?;
        Ok(ResolvedGeneration {
            engine: self.engine,
            tool_dirs: plan
                .layout
                .tool_dirs()
                .iter()
                .map(|directory| generation_root.join(directory))
                .collect(),
            executable,
            generation_root,
            generation_id: generation.directory.clone(),
            version,
            executable_size_bytes: size,
            executable_sha256: sha256,
            verified_file_id,
        })
    }

    /// Returns the engine root for an absolute database path.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the database path is unsafe.
    pub fn managed_engine_root(&self, database_path: &Path) -> Result<PathBuf, ManagedEngineError> {
        self.install_paths(database_path)
            .map(|paths| paths.engine_root().to_path_buf())
            .map_err(map_path_authority_error)
    }

    /// Reads and validates the bounded install-state document.
    ///
    /// A missing state file is returned as `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedStateError`] when the state path, bytes, or decoded
    /// state fails validation.
    pub fn read_install_state(
        &self,
        engine_root: &Path,
    ) -> Result<Option<ManagedToolchainState>, ManagedStateError> {
        let state_path = state_path_for_root(engine_root, self.engine)?;
        let bytes = match native_files::read_bounded(&state_path, MAX_STATE_BYTES) {
            Ok(bytes) => bytes,
            Err(NativeFileError::NotFound) => return Ok(None),
            Err(error) => return Err(map_state_file_error(error)),
        };
        let state = decode_state(&bytes).map_err(map_state_decoder_error)?;
        validate_install_state(&state, self.engine, self.platform)?;
        Ok(Some(state))
    }

    /// Encodes a validated install-state value.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedStateError`] when the state is invalid, cannot be
    /// encoded, or exceeds the bounded representation.
    pub fn encode_install_state(
        &self,
        state: &ManagedToolchainState,
    ) -> Result<Vec<u8>, ManagedStateError> {
        validate_install_state(state, self.engine, self.platform)?;
        let bytes = serde_json::to_vec(state).map_err(|_| ManagedStateError::Encode)?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(ManagedStateError::TooLarge);
        }
        Ok(bytes)
    }

    /// Atomically publishes a validated install-state document.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedStateError`] when the state, destination, or atomic
    /// publication fails validation.
    pub fn write_install_state(
        &self,
        engine_root: &Path,
        state: &ManagedToolchainState,
    ) -> Result<AtomicReplaceOutcome, ManagedStateError> {
        let state_path = state_path_for_root(engine_root, self.engine)?;
        let bytes = self.encode_install_state(state)?;
        native_files::replace_file(&state_path, &bytes).map_err(map_state_replace_error)
    }

    /// Reads the persisted version selection (`latest` when unset).
    ///
    /// # Errors
    ///
    /// Returns [`ManagedStateError`] when the selection document is invalid.
    pub fn read_selection(&self, engine_root: &Path) -> Result<EngineSelection, ManagedStateError> {
        read_selection(engine_root, self.engine)
    }

    /// Atomically persists the version selection.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedStateError`] when the publication fails.
    pub fn write_selection(
        &self,
        engine_root: &Path,
        selection: &EngineSelection,
    ) -> Result<AtomicReplaceOutcome, ManagedStateError> {
        write_selection(engine_root, self.engine, selection)
    }
}

/// Filesystem facts that change whenever a file's bytes can have changed.
#[derive(Clone, Copy, Eq, PartialEq)]
struct Fingerprint {
    identity: VerifiedFileIdentity,
    size: u64,
    sha256: [u8; 32],
    modified: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}

static VERIFIED: LazyLock<Mutex<HashMap<PathBuf, Fingerprint>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Verifies `path` like [`native_files::verify_file`], skipping the rehash
/// when this process already verified the same file identity, size, and
/// change times against the same digest. Engine executables are hundreds of
/// megabytes and are resolved for every spawn.
fn verify_cached(
    path: &Path,
    size: u64,
    sha256: &[u8; 32],
) -> Result<VerifiedFileIdentity, NativeFileError> {
    let current = |identity| fingerprint(path, identity, size, sha256);
    if let Ok(identity) = native_files::path_identity(path).map(VerifiedFileIdentity::new)
        && let Some(observed) = current(identity)
        && VERIFIED
            .lock()
            .ok()
            .and_then(|cache| cache.get(path).copied())
            == Some(observed)
        && native_files::verify_regular_file(path).is_ok()
    {
        return Ok(identity);
    }
    let identity = native_files::verify_file(path, size, sha256)?;
    if let (Some(observed), Ok(mut cache)) = (current(identity), VERIFIED.lock()) {
        cache.insert(path.to_path_buf(), observed);
    }
    Ok(identity)
}

fn fingerprint(
    path: &Path,
    identity: VerifiedFileIdentity,
    size: u64,
    sha256: &[u8; 32],
) -> Option<Fingerprint> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() != size {
        return None;
    }
    #[cfg(unix)]
    let changed = {
        use std::os::unix::fs::MetadataExt;
        (metadata.ctime(), metadata.ctime_nsec())
    };
    Some(Fingerprint {
        identity,
        size,
        sha256: *sha256,
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        changed,
    })
}

pub(crate) fn decode_sha256(value: &str) -> Option<[u8; 32]> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (byte, [high, low]) in digest.iter_mut().zip(bytes.as_chunks::<2>().0) {
        *byte = (hex_nibble(*high)? << 4) | hex_nibble(*low)?;
    }
    Some(digest)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

pub(crate) fn map_path_authority_error(error: ManagedInstallPathError) -> ManagedEngineError {
    match error {
        ManagedInstallPathError::InvalidRoot => ManagedEngineError::UnsafePath,
        ManagedInstallPathError::Unavailable => ManagedEngineError::Io,
    }
}

fn map_executable_error(error: NativeFileError) -> ManagedEngineError {
    match error {
        NativeFileError::NotFound => ManagedEngineError::ExecutableUnavailable,
        NativeFileError::TooLarge | NativeFileError::FileSizeMismatch => {
            ManagedEngineError::ExecutableSizeMismatch
        }
        NativeFileError::FileChanged => ManagedEngineError::ExecutableChanged,
        NativeFileError::FileHashMismatch => ManagedEngineError::ExecutableHashMismatch,
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            ManagedEngineError::UnsafePath
        }
        NativeFileError::Io => ManagedEngineError::Io,
    }
}
