//! Verified profile launch capabilities and their bounded error mapping.
//!
//! Split out of `profile.rs`; a capability retains the install lock and
//! revalidates the exact registry, home, generation, and executable.

#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};

use artisan_domain::EngineProfileId;

use crate::engine_core::{
    ManagedEngineError, ManagedInstallLock, ManagedInstallLockError, ManagedInstallPathError,
    ManagedInstallPaths, NativeOpenCode2Authority, ResolvedGeneration,
};
use crate::io::{NativeFileError, VerifiedFileIdentity};

use super::registry::{derived_profile_home, read_exact_profile_from_path, validate_profile_home};
use super::types::{
    NativeOpenCode2ProfileError, NativeOpenCode2ProfileLaunchError, OpenCode2Profile,
    ProfileHomeKind,
};
/// A launch capability for one exact registered profile and certified active
/// generation. It is intentionally neither serializable nor cloneable. The
/// retained install lock prevents cooperating installation or registration
/// from replacing the certified generation while this value is live.
#[must_use = "retain the capability until the protected launch is complete"]
pub struct VerifiedOpenCode2ProfileLaunch {
    database_path: PathBuf,
    paths: ManagedInstallPaths,
    profile_id: EngineProfileId,
    home: ProfileHomeKind,
    authority: NativeOpenCode2Authority,
    profile_home: PathBuf,
    executable: PathBuf,
    generation_id: String,
    version: String,
    executable_size_bytes: u64,
    executable_sha256: [u8; 32],
    executable_identity: VerifiedFileIdentity,
    install_lock: ManagedInstallLock,
}

impl fmt::Debug for VerifiedOpenCode2ProfileLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedOpenCode2ProfileLaunch")
            .finish_non_exhaustive()
    }
}

impl fmt::Display for VerifiedOpenCode2ProfileLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("verified OpenCode2 profile launch capability")
    }
}

impl VerifiedOpenCode2ProfileLaunch {
    /// Returns the exact profile identifier selected by the registry.
    #[must_use]
    pub fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns whether the profile uses the primary or a named home.
    #[must_use]
    pub const fn home(&self) -> ProfileHomeKind {
        self.home
    }

    /// Returns the exact validated private profile home.
    #[must_use]
    pub fn profile_home(&self) -> &Path {
        &self.profile_home
    }

    /// Returns the exact certified executable path.
    #[must_use]
    pub fn executable_path(&self) -> &Path {
        &self.executable
    }

    /// Returns the exact certified active generation identifier.
    #[must_use]
    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    /// Returns the certified executable version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
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

    /// Returns the opaque filesystem identity captured during verification.
    pub const fn executable_identity(&self) -> VerifiedFileIdentity {
        self.executable_identity
    }

    /// Rechecks the same exact profile home, active generation, executable
    /// identity, size, and hash while retaining this capability's lock.
    /// No discovery or fallback is performed.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2ProfileLaunchError::ProfileChanged`] or a
    /// bounded launch error when the retained lock, registry, home,
    /// generation, executable identity, size, or hash no longer matches.
    pub fn revalidate(&self) -> Result<(), NativeOpenCode2ProfileLaunchError> {
        self.install_lock
            .fence(&self.paths)
            .map_err(map_launch_lock_error)?;
        let authority = self.authority;
        let profile = read_exact_profile_from_path(
            &self.paths.engine_root().join("profiles.json"),
            &self.profile_id,
        )
        .map_err(map_launch_profile_error)?;
        if profile.home != self.home {
            return Err(NativeOpenCode2ProfileLaunchError::ProfileChanged);
        }
        let profile_home = derived_profile_home(&self.paths, &self.profile_id, profile.home)
            .map_err(map_launch_profile_error)?;
        if profile_home != self.profile_home {
            return Err(NativeOpenCode2ProfileLaunchError::ProfileChanged);
        }
        validate_profile_home(&profile_home).map_err(map_launch_home_error)?;
        let generation = authority
            .resolve_active(&self.database_path)
            .map_err(map_launch_authority_error)?;
        if !same_generation_as_capability(&generation, self) {
            return Err(NativeOpenCode2ProfileLaunchError::ProfileChanged);
        }
        self.install_lock
            .fence(&self.paths)
            .map_err(map_launch_lock_error)
    }
}

impl NativeOpenCode2Authority {
    /// Resolves one exact registered profile into a retained launch
    /// capability. The registry, active state, generation, executable
    /// identity, size, and hash are read again while the exclusive install
    /// fence is held immediately before this returns.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2ProfileLaunchError`] when the exact profile,
    /// private home, retained generation, executable identity, size, hash, or
    /// install fence cannot be certified.
    pub fn resolve_profile_launch(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
    ) -> Result<VerifiedOpenCode2ProfileLaunch, NativeOpenCode2ProfileLaunchError> {
        if !self.platform_supported() {
            return Err(NativeOpenCode2ProfileLaunchError::UnsupportedPlatform);
        }
        let paths = self
            .install_paths(database_path)
            .map_err(map_launch_path_error)?;
        let install_lock = ManagedInstallLock::acquire(&paths).map_err(map_launch_lock_error)?;
        let first =
            resolve_profile_under_fence(*self, database_path, profile_id, &paths, &install_lock)?;
        let second =
            resolve_profile_under_fence(*self, database_path, profile_id, &paths, &install_lock)?;
        if first.profile.home != second.profile.home
            || first.home != second.home
            || !same_generation(&first.generation, &second.generation)
        {
            return Err(NativeOpenCode2ProfileLaunchError::ProfileChanged);
        }
        install_lock.fence(&paths).map_err(map_launch_lock_error)?;
        let generation = second.generation;
        Ok(VerifiedOpenCode2ProfileLaunch {
            database_path: database_path.to_path_buf(),
            paths,
            profile_id: second.profile.profile_id,
            home: second.profile.home,
            authority: *self,
            profile_home: second.home,
            executable: generation.executable_path().to_path_buf(),
            generation_id: generation.generation_id().to_owned(),
            version: generation.version().to_string(),
            executable_size_bytes: generation.executable_size_bytes(),
            executable_sha256: *generation.executable_sha256(),
            executable_identity: generation.file_identity(),
            install_lock,
        })
    }
}

struct ProfileResolution {
    profile: OpenCode2Profile,
    home: PathBuf,
    generation: ResolvedGeneration,
}

fn resolve_profile_under_fence(
    authority: NativeOpenCode2Authority,
    database_path: &Path,
    profile_id: &EngineProfileId,
    paths: &ManagedInstallPaths,
    install_lock: &ManagedInstallLock,
) -> Result<ProfileResolution, NativeOpenCode2ProfileLaunchError> {
    install_lock.fence(paths).map_err(map_launch_lock_error)?;
    let registry_path = paths.engine_root().join("profiles.json");
    let profile = read_exact_profile_from_path(&registry_path, profile_id)
        .map_err(map_launch_profile_error)?;
    let home = derived_profile_home(paths, &profile.profile_id, profile.home)
        .map_err(map_launch_profile_error)?;
    validate_profile_home(&home).map_err(map_launch_home_error)?;
    let generation = authority
        .resolve_active(database_path)
        .map_err(map_launch_authority_error)?;
    install_lock.fence(paths).map_err(map_launch_lock_error)?;
    Ok(ProfileResolution {
        profile,
        home,
        generation,
    })
}

fn same_generation(left: &ResolvedGeneration, right: &ResolvedGeneration) -> bool {
    left.executable_path() == right.executable_path()
        && left.generation_id() == right.generation_id()
        && left.version() == right.version()
        && left.executable_size_bytes() == right.executable_size_bytes()
        && left.executable_sha256() == right.executable_sha256()
        && left.file_identity() == right.file_identity()
}

fn same_generation_as_capability(
    generation: &ResolvedGeneration,
    capability: &VerifiedOpenCode2ProfileLaunch,
) -> bool {
    generation.executable_path() == capability.executable_path()
        && generation.generation_id() == capability.generation_id()
        && generation.version().as_str() == capability.version()
        && generation.executable_size_bytes() == capability.executable_size_bytes()
        && generation.executable_sha256() == capability.executable_sha256()
        && generation.file_identity() == capability.executable_identity()
}

fn map_launch_path_error(error: ManagedInstallPathError) -> NativeOpenCode2ProfileLaunchError {
    match error {
        ManagedInstallPathError::InvalidRoot => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsafe
        }
        ManagedInstallPathError::Unavailable => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable
        }
    }
}

fn map_launch_lock_error(_error: ManagedInstallLockError) -> NativeOpenCode2ProfileLaunchError {
    NativeOpenCode2ProfileLaunchError::LockUnavailable
}

fn map_launch_profile_error(
    error: NativeOpenCode2ProfileError,
) -> NativeOpenCode2ProfileLaunchError {
    match error {
        NativeOpenCode2ProfileError::ProfileRegistryTooLarge => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryTooLarge
        }
        NativeOpenCode2ProfileError::ProfileRegistryMalformed
        | NativeOpenCode2ProfileError::ProfileLimit => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed
        }
        NativeOpenCode2ProfileError::ProfileRegistryUnsupportedVersion => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedVersion
        }
        NativeOpenCode2ProfileError::ProfileRegistryUnsupportedEngine => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedEngine
        }
        NativeOpenCode2ProfileError::ProfileRegistryUnsafe => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsafe
        }
        NativeOpenCode2ProfileError::ProfileRegistryUnavailable
        | NativeOpenCode2ProfileError::ProfileConflict
        | NativeOpenCode2ProfileError::PrimaryAlreadyRegistered
        | NativeOpenCode2ProfileError::ProfileAtomicPublishFailed
        | NativeOpenCode2ProfileError::ProfileLockUnavailable
        | NativeOpenCode2ProfileError::CertifiedEngineUnavailable => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable
        }
        NativeOpenCode2ProfileError::DuplicateProfile => {
            NativeOpenCode2ProfileLaunchError::DuplicateProfile
        }
        NativeOpenCode2ProfileError::MultiplePrimaryProfiles => {
            NativeOpenCode2ProfileLaunchError::MultiplePrimaryProfiles
        }
        NativeOpenCode2ProfileError::ProfileNotFound => {
            NativeOpenCode2ProfileLaunchError::ProfileNotFound
        }
        NativeOpenCode2ProfileError::ProfileHomeUnsafe => {
            NativeOpenCode2ProfileLaunchError::ProfileHomeUnsafe
        }
        NativeOpenCode2ProfileError::ProfileHomeUnavailable => {
            NativeOpenCode2ProfileLaunchError::ProfileHomeUnavailable
        }
    }
}

fn map_launch_home_error(error: NativeFileError) -> NativeOpenCode2ProfileLaunchError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            NativeOpenCode2ProfileLaunchError::ProfileHomeUnsafe
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => NativeOpenCode2ProfileLaunchError::ProfileHomeUnavailable,
    }
}

fn map_launch_authority_error(error: ManagedEngineError) -> NativeOpenCode2ProfileLaunchError {
    match error {
        ManagedEngineError::UnsupportedPlatform => {
            NativeOpenCode2ProfileLaunchError::UnsupportedPlatform
        }
        ManagedEngineError::StateMissing => NativeOpenCode2ProfileLaunchError::InstallStateMissing,
        ManagedEngineError::StateTooLarge
        | ManagedEngineError::StateMalformed
        | ManagedEngineError::StateUnsupportedVersion
        | ManagedEngineError::Io => NativeOpenCode2ProfileLaunchError::InstallStateInvalid,
        ManagedEngineError::ActiveGenerationUntrusted => {
            NativeOpenCode2ProfileLaunchError::GenerationUntrusted
        }
        ManagedEngineError::UnsafePath => NativeOpenCode2ProfileLaunchError::GenerationUnsafe,
        ManagedEngineError::ExecutableUnavailable => {
            NativeOpenCode2ProfileLaunchError::ExecutableUnavailable
        }
        ManagedEngineError::ExecutableChanged => {
            NativeOpenCode2ProfileLaunchError::ExecutableChanged
        }
        ManagedEngineError::ExecutableSizeMismatch => {
            NativeOpenCode2ProfileLaunchError::ExecutableSizeMismatch
        }
        ManagedEngineError::ExecutableHashMismatch => {
            NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch
        }
    }
}
