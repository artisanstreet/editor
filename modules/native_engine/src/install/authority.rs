//! Certified `OpenCode2` inspection and executable-resolution façade.
//!
//! Composes the certified spec, the exclusive install lock, the validated
//! install-state codec, and the native file-verification seam into the single
//! authority used by profile registration and launch resolution.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use crate::io as native_files;
use crate::io::{AtomicReplaceOutcome, NativeFileError, VerifiedFileIdentity};

use super::{
    spec::{
        CERTIFIED_ARCHITECTURE, CERTIFIED_ARCHIVE_MEMBER, CERTIFIED_ARTIFACT_KIND,
        CERTIFIED_BINARY, CERTIFIED_DOWNLOAD_BOUND_BYTES, CERTIFIED_ENGINE_ID,
        CERTIFIED_EXECUTABLE_SHA256, CERTIFIED_EXECUTABLE_SHA256_HEX,
        CERTIFIED_EXECUTABLE_SIZE_BYTES, CERTIFIED_NPM_INTEGRITY_SHA512, CERTIFIED_NPM_URL,
        CERTIFIED_PLATFORM, CERTIFIED_UPSTREAM_COMMIT, CERTIFIED_VERSION,
        NativeOpenCode2InstallLock, NativeOpenCode2InstallLockError,
        NativeOpenCode2InstallPathError, NativeOpenCode2InstallPaths, NativeOpenCode2InstallSpec,
        map_path_lock_error, platform_supported,
    },
    state::{
        MAX_STATE_BYTES, ManagedToolchainStateV1, NativeOpenCode2Error, NativeOpenCode2State,
        NativeOpenCode2StateError, decode_state, map_state_decoder_error, map_state_file_error,
        map_state_replace_error, map_state_seam_error, state_path_for_root, validate_install_state,
    },
};

/// The result of inspecting the certified `OpenCode2` installation.
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
    pub(crate) executable: PathBuf,
    pub(crate) generation_id: String,
    pub(crate) version: &'static str,
    pub(crate) upstream_commit: &'static str,
    pub(crate) executable_size_bytes: u64,
    pub(crate) executable_sha256: [u8; 32],
    pub(crate) verified_file_id: VerifiedFileIdentity,
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

/// Shared authority for the certified `OpenCode2` specification, install state,
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
    /// Constructs the explicit certified `OpenCode2` authority.
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

    /// Returns the immutable certified `OpenCode2` artifact specification.
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

fn map_path_authority_error(error: NativeOpenCode2InstallPathError) -> NativeOpenCode2Error {
    match error {
        NativeOpenCode2InstallPathError::InvalidRoot => NativeOpenCode2Error::UnsafePath,
        NativeOpenCode2InstallPathError::Unavailable => NativeOpenCode2Error::Io,
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
