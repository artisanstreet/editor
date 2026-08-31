use std::{
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
};

use artisan_domain::EngineProfileId;
use serde::{
    Deserialize, Serialize,
    de::{self, Deserializer, MapAccess, Visitor},
};

use crate::{
    engine_core::{
        NativeOpenCode2Authority, NativeOpenCode2Error, NativeOpenCode2InstallLock,
        NativeOpenCode2InstallLockError, NativeOpenCode2InstallPathError,
        NativeOpenCode2InstallPaths, NativeOpenCode2InstallSpec, ResolvedOpenCode2Generation,
        platform_supported,
    },
    io::{AtomicReplaceOutcome, NativeFileError, VerifiedFileIdentity},
};

use crate::io as files;

const MAX_PROFILE_REGISTRY_BYTES: usize = 16 * 1024;
const MAX_PROFILES: usize = 64;
const PROFILE_REGISTRY_FORMAT_VERSION: u64 = 1;

const PROFILE_REGISTRY_FIELDS: &[&str] = &["engine_id", "format_version", "profiles"];
const PROFILE_FIELDS: &[&str] = &["profile_id", "home"];

/// Whether a registered profile uses the one primary home or a named home.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileHomeKind {
    Primary,
    Named,
}

impl ProfileHomeKind {
    /// Returns the stable registry spelling for this home kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Named => "named",
        }
    }
}

/// A validated profile registry entry. It contains no filesystem path or
/// mutable launch authority.
#[must_use = "retain the validated profile entry for the requested operation"]
#[derive(Clone, Eq, PartialEq)]
pub struct OpenCode2Profile {
    profile_id: EngineProfileId,
    home: ProfileHomeKind,
}

impl fmt::Debug for OpenCode2Profile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenCode2Profile")
            .finish_non_exhaustive()
    }
}

impl OpenCode2Profile {
    /// Returns the exact registered profile identifier.
    #[must_use]
    pub fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns whether this profile uses the primary or a named home.
    #[must_use]
    pub const fn home(&self) -> ProfileHomeKind {
        self.home
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EngineProfileRegistration {
    profile_id: EngineProfileId,
    home: ProfileHomeKind,
}

#[derive(Debug, Eq, PartialEq)]
struct ProfileRegistry {
    profiles: Vec<EngineProfileRegistration>,
}

impl ProfileRegistry {
    fn empty() -> Self {
        Self {
            profiles: Vec::new(),
        }
    }

    fn sort(&mut self) {
        self.profiles
            .sort_by(|left, right| left.profile_id.cmp(&right.profile_id));
    }
}

/// Bounded, path-free failures from profile registry and home operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOpenCode2ProfileError {
    ProfileRegistryTooLarge,
    ProfileRegistryMalformed,
    ProfileRegistryUnsupportedVersion,
    ProfileRegistryUnsupportedEngine,
    ProfileRegistryUnsafe,
    ProfileRegistryUnavailable,
    DuplicateProfile,
    MultiplePrimaryProfiles,
    ProfileNotFound,
    ProfileConflict,
    PrimaryAlreadyRegistered,
    ProfileLimit,
    ProfileHomeUnsafe,
    ProfileHomeUnavailable,
    ProfileAtomicPublishFailed,
    ProfileLockUnavailable,
    CertifiedEngineUnavailable,
}

impl NativeOpenCode2ProfileError {
    /// Returns the stable CLI classification for this failure.
    #[must_use]
    pub const fn cli_reason(self) -> &'static str {
        match self {
            Self::ProfileNotFound => "profile_not_found",
            Self::ProfileConflict => "profile_conflict",
            Self::PrimaryAlreadyRegistered => "primary_already_registered",
            Self::ProfileLimit => "profile_limit",
            Self::ProfileHomeUnsafe => "profile_home_unsafe",
            Self::ProfileHomeUnavailable => "profile_home_unavailable",
            Self::ProfileAtomicPublishFailed => "profile_publish_failed",
            Self::ProfileLockUnavailable => "profile_lock_unavailable",
            Self::ProfileRegistryTooLarge
            | Self::ProfileRegistryMalformed
            | Self::ProfileRegistryUnsupportedVersion
            | Self::ProfileRegistryUnsupportedEngine
            | Self::ProfileRegistryUnsafe
            | Self::ProfileRegistryUnavailable
            | Self::DuplicateProfile
            | Self::MultiplePrimaryProfiles
            | Self::CertifiedEngineUnavailable => "profile_registry_invalid",
        }
    }
}

impl fmt::Display for NativeOpenCode2ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ProfileRegistryTooLarge => "OpenCode2 profile registry is too large",
            Self::ProfileRegistryMalformed => "OpenCode2 profile registry is malformed",
            Self::ProfileRegistryUnsupportedVersion => {
                "OpenCode2 profile registry version is unsupported"
            }
            Self::ProfileRegistryUnsupportedEngine => {
                "OpenCode2 profile registry engine is unsupported"
            }
            Self::ProfileRegistryUnsafe => "OpenCode2 profile registry path is unsafe",
            Self::ProfileRegistryUnavailable => "OpenCode2 profile registry is unavailable",
            Self::DuplicateProfile => "OpenCode2 profile registry contains a duplicate profile",
            Self::MultiplePrimaryProfiles => {
                "OpenCode2 profile registry contains multiple primary profiles"
            }
            Self::ProfileNotFound => "OpenCode2 profile was not found",
            Self::ProfileConflict => "OpenCode2 profile conflicts with its existing home kind",
            Self::PrimaryAlreadyRegistered => "an OpenCode2 primary profile is already registered",
            Self::ProfileLimit => "OpenCode2 profile limit has been reached",
            Self::ProfileHomeUnsafe => "OpenCode2 profile home is unsafe",
            Self::ProfileHomeUnavailable => "OpenCode2 profile home is unavailable",
            Self::ProfileAtomicPublishFailed => "OpenCode2 profile registry publication failed",
            Self::ProfileLockUnavailable => "OpenCode2 installation lock is unavailable",
            Self::CertifiedEngineUnavailable => "certified OpenCode2 is unavailable",
        })
    }
}

impl std::error::Error for NativeOpenCode2ProfileError {}

/// Result of registering an exact profile mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileRegistrationOutcome {
    Registered,
    AlreadyRegistered,
}

/// Failure while resolving or revalidating a launch capability. Every
/// variant is payload-free so paths, profile registry bytes, and OS messages
/// cannot escape through diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOpenCode2ProfileLaunchError {
    UnsupportedPlatform,
    ProfileRegistryTooLarge,
    ProfileRegistryMalformed,
    ProfileRegistryUnsupportedVersion,
    ProfileRegistryUnsupportedEngine,
    ProfileRegistryUnsafe,
    ProfileRegistryUnavailable,
    DuplicateProfile,
    MultiplePrimaryProfiles,
    ProfileNotFound,
    ProfileHomeUnsafe,
    ProfileHomeUnavailable,
    LockUnavailable,
    InstallStateMissing,
    InstallStateInvalid,
    GenerationUnsafe,
    GenerationUntrusted,
    ExecutableUnavailable,
    ExecutableChanged,
    ExecutableSizeMismatch,
    ExecutableHashMismatch,
    ProfileChanged,
}

impl fmt::Display for NativeOpenCode2ProfileLaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedPlatform => "OpenCode2 profile launch is unsupported on this platform",
            Self::ProfileRegistryTooLarge => "OpenCode2 profile registry is too large",
            Self::ProfileRegistryMalformed => "OpenCode2 profile registry is malformed",
            Self::ProfileRegistryUnsupportedVersion => {
                "OpenCode2 profile registry version is unsupported"
            }
            Self::ProfileRegistryUnsupportedEngine => {
                "OpenCode2 profile registry engine is unsupported"
            }
            Self::ProfileRegistryUnsafe => "OpenCode2 profile registry path is unsafe",
            Self::ProfileRegistryUnavailable => "OpenCode2 profile registry is unavailable",
            Self::DuplicateProfile => "OpenCode2 profile registry contains a duplicate profile",
            Self::MultiplePrimaryProfiles => {
                "OpenCode2 profile registry contains multiple primary profiles"
            }
            Self::ProfileNotFound => "OpenCode2 profile was not found",
            Self::ProfileHomeUnsafe => "OpenCode2 profile home is unsafe",
            Self::ProfileHomeUnavailable => "OpenCode2 profile home is unavailable",
            Self::LockUnavailable => "OpenCode2 installation lock is unavailable",
            Self::InstallStateMissing => "OpenCode2 installation state is missing",
            Self::InstallStateInvalid => "OpenCode2 installation state is invalid",
            Self::GenerationUnsafe => "OpenCode2 active generation path is unsafe",
            Self::GenerationUntrusted => "OpenCode2 active generation is untrusted",
            Self::ExecutableUnavailable => "OpenCode2 executable is unavailable",
            Self::ExecutableChanged => "OpenCode2 executable changed during verification",
            Self::ExecutableSizeMismatch => "OpenCode2 executable size does not match",
            Self::ExecutableHashMismatch => "OpenCode2 executable hash does not match",
            Self::ProfileChanged => "OpenCode2 profile launch state changed",
        })
    }
}

impl std::error::Error for NativeOpenCode2ProfileLaunchError {}

/// A launch capability for one exact registered profile and certified active
/// generation. It is intentionally neither serializable nor cloneable. The
/// retained install lock prevents cooperating installation or registration
/// from replacing the certified generation while this value is live.
#[must_use = "retain the capability until the protected launch is complete"]
pub struct VerifiedOpenCode2ProfileLaunch {
    database_path: PathBuf,
    paths: NativeOpenCode2InstallPaths,
    profile_id: EngineProfileId,
    home: ProfileHomeKind,
    install_spec: NativeOpenCode2InstallSpec,
    profile_home: PathBuf,
    executable: PathBuf,
    generation_id: String,
    version: &'static str,
    executable_size_bytes: u64,
    executable_sha256: [u8; 32],
    executable_identity: VerifiedFileIdentity,
    install_lock: NativeOpenCode2InstallLock,
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
    pub const fn version(&self) -> &'static str {
        self.version
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
    #[must_use]
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
    #[must_use]
    pub fn revalidate(&self) -> Result<(), NativeOpenCode2ProfileLaunchError> {
        self.install_lock
            .fence(&self.paths)
            .map_err(map_launch_lock_error)?;
        let authority = NativeOpenCode2Authority::with_spec(self.install_spec);
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

#[derive(Serialize)]
struct ProfileRegistryDocument {
    engine_id: String,
    format_version: u64,
    profiles: Vec<ProfileRegistryEntryDocument>,
}

#[derive(Serialize)]
struct ProfileRegistryEntryDocument {
    profile_id: String,
    home: String,
}

impl<'de> Deserialize<'de> for ProfileRegistryEntryDocument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ProfileEntryVisitor;

        impl<'de> Visitor<'de> for ProfileEntryVisitor {
            type Value = ProfileRegistryEntryDocument;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an OpenCode2 profile registry entry")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut profile_id = None;
                let mut home = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "profile_id" => {
                            if profile_id.is_some() {
                                return Err(de::Error::duplicate_field("profile_id"));
                            }
                            profile_id = Some(map.next_value::<String>()?);
                        }
                        "home" => {
                            if home.is_some() {
                                return Err(de::Error::duplicate_field("home"));
                            }
                            home = Some(map.next_value::<String>()?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, PROFILE_FIELDS)),
                    }
                }
                Ok(Self::Value {
                    profile_id: profile_id.ok_or_else(|| de::Error::missing_field("profile_id"))?,
                    home: home.ok_or_else(|| de::Error::missing_field("home"))?,
                })
            }
        }

        deserializer.deserialize_map(ProfileEntryVisitor)
    }
}

impl<'de> Deserialize<'de> for ProfileRegistryDocument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ProfileRegistryVisitor;

        impl<'de> Visitor<'de> for ProfileRegistryVisitor {
            type Value = ProfileRegistryDocument;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an OpenCode2 profile registry")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut engine_id = None;
                let mut format_version = None;
                let mut profiles = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "engine_id" => {
                            if engine_id.is_some() {
                                return Err(de::Error::duplicate_field("engine_id"));
                            }
                            engine_id = Some(map.next_value::<String>()?);
                        }
                        "format_version" => {
                            if format_version.is_some() {
                                return Err(de::Error::duplicate_field("format_version"));
                            }
                            format_version = Some(map.next_value::<u64>()?);
                        }
                        "profiles" => {
                            if profiles.is_some() {
                                return Err(de::Error::duplicate_field("profiles"));
                            }
                            profiles = Some(map.next_value::<Vec<ProfileRegistryEntryDocument>>()?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, PROFILE_REGISTRY_FIELDS)),
                    }
                }
                Ok(Self::Value {
                    engine_id: engine_id.ok_or_else(|| de::Error::missing_field("engine_id"))?,
                    format_version: format_version
                        .ok_or_else(|| de::Error::missing_field("format_version"))?,
                    profiles: profiles.ok_or_else(|| de::Error::missing_field("profiles"))?,
                })
            }
        }

        deserializer.deserialize_map(ProfileRegistryVisitor)
    }
}

impl NativeOpenCode2Authority {
    /// Returns the profile registry location under the certified engine root.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2ProfileError`] when the database path is
    /// unsafe or the certified installation root is unavailable.
    #[must_use]
    pub fn profile_registry_path(
        &self,
        database_path: &Path,
    ) -> Result<PathBuf, NativeOpenCode2ProfileError> {
        self.install_paths(database_path)
            .map(|paths| paths.engine_root().join("profiles.json"))
            .map_err(map_profile_path_error)
    }

    /// Returns the exact private home selected by one registry entry.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2ProfileError`] when the database path or the
    /// derived profile home is unsafe or unavailable.
    #[must_use]
    pub fn profile_home_path(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
        home: ProfileHomeKind,
    ) -> Result<PathBuf, NativeOpenCode2ProfileError> {
        let paths = self
            .install_paths(database_path)
            .map_err(map_profile_path_error)?;
        derived_profile_home(&paths, profile_id, home)
    }

    /// Registers one exact profile mapping using the shared registry codec and
    /// the same install lock retained by launch capabilities.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2ProfileError`] when the certified installation,
    /// registry, profile home, lock, or atomic publication is invalid.
    #[must_use]
    pub fn register_profile(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
        home: ProfileHomeKind,
    ) -> Result<ProfileRegistrationOutcome, NativeOpenCode2ProfileError> {
        if !platform_supported() {
            return Err(NativeOpenCode2ProfileError::CertifiedEngineUnavailable);
        }
        let _pre_lock_generation = self
            .resolve_active(database_path)
            .map_err(|_| NativeOpenCode2ProfileError::CertifiedEngineUnavailable)?;
        let paths = self
            .install_paths(database_path)
            .map_err(map_profile_path_error)?;
        let lock = NativeOpenCode2InstallLock::acquire(&paths)
            .map_err(|_| NativeOpenCode2ProfileError::ProfileLockUnavailable)?;
        lock.fence(&paths)
            .map_err(|_| NativeOpenCode2ProfileError::ProfileLockUnavailable)?;
        let _post_lock_generation = self
            .resolve_active(database_path)
            .map_err(|_| NativeOpenCode2ProfileError::CertifiedEngineUnavailable)?;
        lock.fence(&paths)
            .map_err(|_| NativeOpenCode2ProfileError::ProfileLockUnavailable)?;

        let registry_path = paths.engine_root().join("profiles.json");
        let mut registry =
            read_profile_registry(&registry_path)?.unwrap_or_else(ProfileRegistry::empty);
        let outcome = register_in_registry(&mut registry, profile_id, home)?;
        let profile_home = derived_profile_home(&paths, profile_id, home)?;
        match outcome {
            ProfileRegistrationOutcome::AlreadyRegistered => {
                validate_profile_home(&profile_home).map_err(map_profile_home_error)?;
            }
            ProfileRegistrationOutcome::Registered => {
                files::ensure_private_directory(&profile_home).map_err(map_profile_home_error)?;
            }
        }
        lock.fence(&paths)
            .map_err(|_| NativeOpenCode2ProfileError::ProfileLockUnavailable)?;
        if outcome == ProfileRegistrationOutcome::AlreadyRegistered {
            return Ok(outcome);
        }
        let bytes = encode_profile_registry(&registry)?;
        match files::replace_file(&registry_path, &bytes).map_err(map_profile_atomic_error)? {
            AtomicReplaceOutcome::Committed => Ok(ProfileRegistrationOutcome::Registered),
            AtomicReplaceOutcome::CommittedButUnverified => {
                Err(NativeOpenCode2ProfileError::ProfileAtomicPublishFailed)
            }
        }
    }

    /// Lists the validated registry entries. A missing registry is distinct
    /// from an empty, valid registry.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2ProfileError`] when the registry path or
    /// bounded registry document is invalid or unavailable.
    #[must_use]
    pub fn list_profiles(
        &self,
        database_path: &Path,
    ) -> Result<Option<Vec<OpenCode2Profile>>, NativeOpenCode2ProfileError> {
        let registry_path = self.profile_registry_path(database_path)?;
        let Some(registry) = read_profile_registry(&registry_path)? else {
            return Ok(None);
        };
        Ok(Some(
            registry
                .profiles
                .into_iter()
                .map(|profile| OpenCode2Profile {
                    profile_id: profile.profile_id,
                    home: profile.home,
                })
                .collect(),
        ))
    }

    /// Reads one exact profile id. There is no primary or `default` fallback.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2ProfileError`] when the registry is invalid or
    /// the exact requested profile is absent.
    #[must_use]
    pub fn read_profile(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
    ) -> Result<OpenCode2Profile, NativeOpenCode2ProfileError> {
        let registry_path = self.profile_registry_path(database_path)?;
        read_exact_profile_from_path(&registry_path, profile_id)
    }

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
    #[must_use]
    pub fn resolve_profile_launch(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
    ) -> Result<VerifiedOpenCode2ProfileLaunch, NativeOpenCode2ProfileLaunchError> {
        if !platform_supported() {
            return Err(NativeOpenCode2ProfileLaunchError::UnsupportedPlatform);
        }
        let paths = self
            .install_paths(database_path)
            .map_err(map_launch_path_error)?;
        let install_lock =
            NativeOpenCode2InstallLock::acquire(&paths).map_err(map_launch_lock_error)?;
        let first =
            resolve_profile_under_fence(self, database_path, profile_id, &paths, &install_lock)?;
        let second =
            resolve_profile_under_fence(self, database_path, profile_id, &paths, &install_lock)?;
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
            install_spec: *self.spec(),
            profile_home: second.home,
            executable: generation.executable_path().to_path_buf(),
            generation_id: generation.generation_id().to_owned(),
            version: generation.version(),
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
    generation: ResolvedOpenCode2Generation,
}

fn resolve_profile_under_fence(
    authority: &NativeOpenCode2Authority,
    database_path: &Path,
    profile_id: &EngineProfileId,
    paths: &NativeOpenCode2InstallPaths,
    install_lock: &NativeOpenCode2InstallLock,
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

fn same_generation(
    left: &ResolvedOpenCode2Generation,
    right: &ResolvedOpenCode2Generation,
) -> bool {
    left.executable_path() == right.executable_path()
        && left.generation_id() == right.generation_id()
        && left.version() == right.version()
        && left.executable_size_bytes() == right.executable_size_bytes()
        && left.executable_sha256() == right.executable_sha256()
        && left.file_identity() == right.file_identity()
}

fn same_generation_as_capability(
    generation: &ResolvedOpenCode2Generation,
    capability: &VerifiedOpenCode2ProfileLaunch,
) -> bool {
    generation.executable_path() == capability.executable_path()
        && generation.generation_id() == capability.generation_id()
        && generation.version() == capability.version()
        && generation.executable_size_bytes() == capability.executable_size_bytes()
        && generation.executable_sha256() == capability.executable_sha256()
        && generation.file_identity() == capability.executable_identity()
}

fn register_in_registry(
    registry: &mut ProfileRegistry,
    profile_id: &EngineProfileId,
    home: ProfileHomeKind,
) -> Result<ProfileRegistrationOutcome, NativeOpenCode2ProfileError> {
    if let Some(existing) = registry
        .profiles
        .iter()
        .find(|profile| profile.profile_id == *profile_id)
    {
        return if existing.home == home {
            Ok(ProfileRegistrationOutcome::AlreadyRegistered)
        } else {
            Err(NativeOpenCode2ProfileError::ProfileConflict)
        };
    }

    if registry.profiles.len() >= MAX_PROFILES {
        return Err(NativeOpenCode2ProfileError::ProfileLimit);
    }
    if home == ProfileHomeKind::Primary
        && registry
            .profiles
            .iter()
            .any(|profile| profile.home == ProfileHomeKind::Primary)
    {
        return Err(NativeOpenCode2ProfileError::PrimaryAlreadyRegistered);
    }

    registry.profiles.push(EngineProfileRegistration {
        profile_id: profile_id.clone(),
        home,
    });
    registry.sort();
    Ok(ProfileRegistrationOutcome::Registered)
}

fn derived_profile_home(
    paths: &NativeOpenCode2InstallPaths,
    profile_id: &EngineProfileId,
    home: ProfileHomeKind,
) -> Result<PathBuf, NativeOpenCode2ProfileError> {
    let profile_home = match home {
        ProfileHomeKind::Primary => paths.engine_root().join("home"),
        ProfileHomeKind::Named => paths.engine_root().join("homes").join(profile_id.as_str()),
    };
    if !profile_home.is_absolute()
        || profile_home
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(NativeOpenCode2ProfileError::ProfileHomeUnsafe);
    }
    Ok(profile_home)
}

fn read_exact_profile_from_path(
    path: &Path,
    profile_id: &EngineProfileId,
) -> Result<OpenCode2Profile, NativeOpenCode2ProfileError> {
    let Some(registry) = read_profile_registry(path)? else {
        return Err(NativeOpenCode2ProfileError::ProfileNotFound);
    };
    registry
        .profiles
        .into_iter()
        .find(|profile| profile.profile_id == *profile_id)
        .map(|profile| OpenCode2Profile {
            profile_id: profile.profile_id,
            home: profile.home,
        })
        .ok_or(NativeOpenCode2ProfileError::ProfileNotFound)
}

fn read_profile_registry(
    path: &Path,
) -> Result<Option<ProfileRegistry>, NativeOpenCode2ProfileError> {
    let bytes = match files::read_bounded(path, MAX_PROFILE_REGISTRY_BYTES) {
        Ok(bytes) => bytes,
        Err(NativeFileError::NotFound) => return Ok(None),
        Err(NativeFileError::TooLarge) => {
            return Err(NativeOpenCode2ProfileError::ProfileRegistryTooLarge);
        }
        Err(NativeFileError::UnsafePath | NativeFileError::PrivatePermissions) => {
            return Err(NativeOpenCode2ProfileError::ProfileRegistryUnsafe);
        }
        Err(
            NativeFileError::FileChanged
            | NativeFileError::FileSizeMismatch
            | NativeFileError::FileHashMismatch
            | NativeFileError::Io,
        ) => return Err(NativeOpenCode2ProfileError::ProfileRegistryUnavailable),
    };
    decode_profile_registry(&bytes).map(Some)
}

fn decode_profile_registry(bytes: &[u8]) -> Result<ProfileRegistry, NativeOpenCode2ProfileError> {
    if bytes.len() > MAX_PROFILE_REGISTRY_BYTES {
        return Err(NativeOpenCode2ProfileError::ProfileRegistryTooLarge);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let document = ProfileRegistryDocument::deserialize(&mut deserializer)
        .map_err(|_| NativeOpenCode2ProfileError::ProfileRegistryMalformed)?;
    deserializer
        .end()
        .map_err(|_| NativeOpenCode2ProfileError::ProfileRegistryMalformed)?;
    validate_profile_registry(document)
}

fn validate_profile_registry(
    document: ProfileRegistryDocument,
) -> Result<ProfileRegistry, NativeOpenCode2ProfileError> {
    if document.engine_id != NativeOpenCode2Authority::certified_install_spec().engine_id() {
        return Err(NativeOpenCode2ProfileError::ProfileRegistryUnsupportedEngine);
    }
    if document.format_version != PROFILE_REGISTRY_FORMAT_VERSION {
        return Err(NativeOpenCode2ProfileError::ProfileRegistryUnsupportedVersion);
    }
    if document.profiles.len() > MAX_PROFILES {
        return Err(NativeOpenCode2ProfileError::ProfileLimit);
    }

    let mut ids = HashSet::with_capacity(document.profiles.len());
    let mut has_primary = false;
    let mut profiles = Vec::with_capacity(document.profiles.len());
    for entry in document.profiles {
        let profile_id = EngineProfileId::parse(entry.profile_id)
            .map_err(|_| NativeOpenCode2ProfileError::ProfileRegistryMalformed)?;
        if !ids.insert(profile_id.clone()) {
            return Err(NativeOpenCode2ProfileError::DuplicateProfile);
        }
        let home = match entry.home.as_str() {
            "primary" if !has_primary => {
                has_primary = true;
                ProfileHomeKind::Primary
            }
            "primary" => return Err(NativeOpenCode2ProfileError::MultiplePrimaryProfiles),
            "named" => ProfileHomeKind::Named,
            _ => return Err(NativeOpenCode2ProfileError::ProfileRegistryMalformed),
        };
        profiles.push(EngineProfileRegistration { profile_id, home });
    }
    let mut registry = ProfileRegistry { profiles };
    registry.sort();
    Ok(registry)
}

fn encode_profile_registry(
    registry: &ProfileRegistry,
) -> Result<Vec<u8>, NativeOpenCode2ProfileError> {
    let document = ProfileRegistryDocument {
        engine_id: NativeOpenCode2Authority::certified_install_spec()
            .engine_id()
            .to_owned(),
        format_version: PROFILE_REGISTRY_FORMAT_VERSION,
        profiles: registry
            .profiles
            .iter()
            .map(|profile| ProfileRegistryEntryDocument {
                profile_id: profile.profile_id.as_str().to_owned(),
                home: profile.home.as_str().to_owned(),
            })
            .collect(),
    };
    let bytes = serde_json::to_vec(&document)
        .map_err(|_| NativeOpenCode2ProfileError::ProfileRegistryMalformed)?;
    if bytes.len() > MAX_PROFILE_REGISTRY_BYTES {
        return Err(NativeOpenCode2ProfileError::ProfileRegistryTooLarge);
    }
    Ok(bytes)
}

fn validate_profile_home(path: &Path) -> Result<(), NativeFileError> {
    files::validate_private_directory(path)
}

fn map_profile_path_error(error: NativeOpenCode2InstallPathError) -> NativeOpenCode2ProfileError {
    match error {
        NativeOpenCode2InstallPathError::InvalidRoot => {
            NativeOpenCode2ProfileError::ProfileRegistryUnsafe
        }
        NativeOpenCode2InstallPathError::Unavailable => {
            NativeOpenCode2ProfileError::ProfileRegistryUnavailable
        }
    }
}

fn map_profile_home_error(error: NativeFileError) -> NativeOpenCode2ProfileError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            NativeOpenCode2ProfileError::ProfileHomeUnsafe
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => NativeOpenCode2ProfileError::ProfileHomeUnavailable,
    }
}

fn map_profile_atomic_error(error: NativeFileError) -> NativeOpenCode2ProfileError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            NativeOpenCode2ProfileError::ProfileRegistryUnsafe
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => NativeOpenCode2ProfileError::ProfileAtomicPublishFailed,
    }
}

fn map_launch_path_error(
    error: NativeOpenCode2InstallPathError,
) -> NativeOpenCode2ProfileLaunchError {
    match error {
        NativeOpenCode2InstallPathError::InvalidRoot => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsafe
        }
        NativeOpenCode2InstallPathError::Unavailable => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable
        }
    }
}

fn map_launch_lock_error(
    _error: NativeOpenCode2InstallLockError,
) -> NativeOpenCode2ProfileLaunchError {
    NativeOpenCode2ProfileLaunchError::LockUnavailable
}

fn map_launch_profile_error(
    error: NativeOpenCode2ProfileError,
) -> NativeOpenCode2ProfileLaunchError {
    match error {
        NativeOpenCode2ProfileError::ProfileRegistryTooLarge => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryTooLarge
        }
        NativeOpenCode2ProfileError::ProfileRegistryMalformed => {
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
        NativeOpenCode2ProfileError::ProfileRegistryUnavailable => {
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
        NativeOpenCode2ProfileError::ProfileConflict
        | NativeOpenCode2ProfileError::PrimaryAlreadyRegistered
        | NativeOpenCode2ProfileError::ProfileAtomicPublishFailed
        | NativeOpenCode2ProfileError::ProfileLockUnavailable
        | NativeOpenCode2ProfileError::CertifiedEngineUnavailable => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable
        }
        NativeOpenCode2ProfileError::ProfileLimit => {
            NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed
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

fn map_launch_authority_error(error: NativeOpenCode2Error) -> NativeOpenCode2ProfileLaunchError {
    match error {
        NativeOpenCode2Error::UnsupportedPlatform => {
            NativeOpenCode2ProfileLaunchError::UnsupportedPlatform
        }
        NativeOpenCode2Error::StateMissing => {
            NativeOpenCode2ProfileLaunchError::InstallStateMissing
        }
        NativeOpenCode2Error::StateTooLarge
        | NativeOpenCode2Error::StateMalformed
        | NativeOpenCode2Error::StateUnsupportedVersion
        | NativeOpenCode2Error::Io => NativeOpenCode2ProfileLaunchError::InstallStateInvalid,
        NativeOpenCode2Error::ActiveGenerationUntrusted => {
            NativeOpenCode2ProfileLaunchError::GenerationUntrusted
        }
        NativeOpenCode2Error::UnsafePath => NativeOpenCode2ProfileLaunchError::GenerationUnsafe,
        NativeOpenCode2Error::ExecutableUnavailable => {
            NativeOpenCode2ProfileLaunchError::ExecutableUnavailable
        }
        NativeOpenCode2Error::ExecutableChanged => {
            NativeOpenCode2ProfileLaunchError::ExecutableChanged
        }
        NativeOpenCode2Error::ExecutableSizeMismatch => {
            NativeOpenCode2ProfileLaunchError::ExecutableSizeMismatch
        }
        NativeOpenCode2Error::ExecutableHashMismatch => {
            NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn profile_registry_decoding_is_strict_sorted_and_exact() {
        let input = br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"zeta","home":"named"},{"profile_id":"alpha","home":"primary"}]}"#;
        let registry = decode_profile_registry(input).unwrap();
        assert_eq!(registry.profiles.len(), 2);
        assert_eq!(registry.profiles[0].profile_id.as_str(), "alpha");
        assert_eq!(registry.profiles[1].profile_id.as_str(), "zeta");
        assert_eq!(
            encode_profile_registry(&registry).unwrap(),
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"alpha","home":"primary"},{"profile_id":"zeta","home":"named"}]}"#
        );
    }

    #[test]
    fn profile_registry_rejects_missing_duplicate_malformed_and_ambiguous_entries() {
        for bytes in [
            br"{}".as_slice(),
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[],"extra":true}"#,
            br#"{"engine_id":"opencode2","engine_id":"opencode2","format_version":1,"profiles":[]}"#,
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","home":"named","extra":true}]}"#,
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","profile_id":"b","home":"named"}]}"#,
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a"}]}"#,
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[]} trailing"#,
            br#"{"engine_id":"wrong","format_version":1,"profiles":[]}"#,
            br#"{"engine_id":"opencode2","format_version":2,"profiles":[]}"#,
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","home":"other"}]}"#,
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a/b","home":"named"}]}"#,
        ] {
            assert!(matches!(
                decode_profile_registry(bytes),
                Err(NativeOpenCode2ProfileError::ProfileRegistryMalformed
                    | NativeOpenCode2ProfileError::ProfileRegistryUnsupportedEngine
                    | NativeOpenCode2ProfileError::ProfileRegistryUnsupportedVersion)
            ));
        }

        let duplicate_ids = br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","home":"named"},{"profile_id":"a","home":"named"}]}"#;
        assert_eq!(
            decode_profile_registry(duplicate_ids),
            Err(NativeOpenCode2ProfileError::DuplicateProfile)
        );
        let multiple_primary = br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","home":"primary"},{"profile_id":"b","home":"primary"}]}"#;
        assert_eq!(
            decode_profile_registry(multiple_primary),
            Err(NativeOpenCode2ProfileError::MultiplePrimaryProfiles)
        );
        assert_eq!(
            decode_profile_registry(&vec![b'x'; MAX_PROFILE_REGISTRY_BYTES + 1]),
            Err(NativeOpenCode2ProfileError::ProfileRegistryTooLarge)
        );
    }

    #[test]
    fn profile_registration_policy_is_idempotent_conflict_safe_and_bounded() {
        let id = EngineProfileId::parse("work").unwrap();
        let mut registry = ProfileRegistry {
            profiles: vec![EngineProfileRegistration {
                profile_id: id.clone(),
                home: ProfileHomeKind::Named,
            }],
        };
        let before = encode_profile_registry(&registry).unwrap();
        assert_eq!(
            register_in_registry(&mut registry, &id, ProfileHomeKind::Named),
            Ok(ProfileRegistrationOutcome::AlreadyRegistered)
        );
        assert_eq!(encode_profile_registry(&registry).unwrap(), before);
        assert_eq!(
            register_in_registry(&mut registry, &id, ProfileHomeKind::Primary),
            Err(NativeOpenCode2ProfileError::ProfileConflict)
        );

        let primary = EngineProfileId::parse("primary").unwrap();
        assert_eq!(
            register_in_registry(&mut registry, &primary, ProfileHomeKind::Primary),
            Ok(ProfileRegistrationOutcome::Registered)
        );
        let second_primary = EngineProfileId::parse("second").unwrap();
        assert_eq!(
            register_in_registry(&mut registry, &second_primary, ProfileHomeKind::Primary),
            Err(NativeOpenCode2ProfileError::PrimaryAlreadyRegistered)
        );

        let mut full = ProfileRegistry {
            profiles: Vec::new(),
        };
        for index in 0..MAX_PROFILES {
            full.profiles.push(EngineProfileRegistration {
                profile_id: EngineProfileId::parse(format!("profile-{index:02}")).unwrap(),
                home: ProfileHomeKind::Named,
            });
        }
        let extra = EngineProfileId::parse("extra").unwrap();
        assert_eq!(
            register_in_registry(&mut full, &extra, ProfileHomeKind::Named),
            Err(NativeOpenCode2ProfileError::ProfileLimit)
        );
    }

    #[test]
    fn profile_home_derivation_uses_only_the_certified_engine_root() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("data").join("artisan.sqlite");
        std::fs::create_dir(database.parent().unwrap()).unwrap();
        let authority = NativeOpenCode2Authority::new();
        let profile_id = EngineProfileId::parse("work.profile").unwrap();
        assert_eq!(
            authority
                .profile_home_path(&database, &profile_id, ProfileHomeKind::Primary)
                .unwrap(),
            root.path()
                .join("data")
                .join("toolchain")
                .join("opencode2")
                .join("home")
        );
        assert_eq!(
            authority
                .profile_home_path(&database, &profile_id, ProfileHomeKind::Named)
                .unwrap(),
            root.path()
                .join("data")
                .join("toolchain")
                .join("opencode2")
                .join("homes")
                .join("work.profile")
        );
    }

    #[test]
    fn missing_registry_and_default_are_never_discovered_by_read_authority() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("artisan.sqlite");
        let authority = NativeOpenCode2Authority::new();
        let registry = authority.profile_registry_path(&database).unwrap();
        assert_eq!(authority.list_profiles(&database).unwrap(), None);
        assert_eq!(
            authority.read_profile(&database, &EngineProfileId::parse("default").unwrap()),
            Err(NativeOpenCode2ProfileError::ProfileNotFound)
        );
        assert!(!registry.exists());
        assert!(!root.path().join("toolchain").exists());
    }

    #[cfg(unix)]
    #[test]
    fn profile_home_validation_is_private_and_rejects_unsafe_targets() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        files::ensure_private_directory(&private).unwrap();
        assert_eq!(
            fs::symlink_metadata(&private).unwrap().permissions().mode() & 0o777,
            0o700
        );

        fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            files::validate_private_directory(&private),
            Err(NativeFileError::PrivatePermissions)
        );

        let missing = root.path().join("missing");
        assert_eq!(
            files::validate_private_directory(&missing),
            Err(NativeFileError::NotFound)
        );
        assert!(!missing.exists());

        let target = root.path().join("target");
        fs::create_dir(&target).unwrap();
        let link = root.path().join("link");
        symlink(&target, &link).unwrap();
        assert_eq!(
            files::ensure_private_directory(&link),
            Err(NativeFileError::UnsafePath)
        );

        let file = root.path().join("file");
        fs::write(&file, b"not a directory").unwrap();
        assert_eq!(
            files::ensure_private_directory(&file),
            Err(NativeFileError::UnsafePath)
        );
    }

    #[test]
    fn profile_errors_and_launch_errors_are_path_and_secret_free() {
        let profile_errors = [
            NativeOpenCode2ProfileError::ProfileRegistryTooLarge,
            NativeOpenCode2ProfileError::ProfileRegistryMalformed,
            NativeOpenCode2ProfileError::ProfileRegistryUnsupportedVersion,
            NativeOpenCode2ProfileError::ProfileRegistryUnsupportedEngine,
            NativeOpenCode2ProfileError::ProfileRegistryUnsafe,
            NativeOpenCode2ProfileError::ProfileRegistryUnavailable,
            NativeOpenCode2ProfileError::DuplicateProfile,
            NativeOpenCode2ProfileError::MultiplePrimaryProfiles,
            NativeOpenCode2ProfileError::ProfileNotFound,
            NativeOpenCode2ProfileError::ProfileConflict,
            NativeOpenCode2ProfileError::PrimaryAlreadyRegistered,
            NativeOpenCode2ProfileError::ProfileLimit,
            NativeOpenCode2ProfileError::ProfileHomeUnsafe,
            NativeOpenCode2ProfileError::ProfileHomeUnavailable,
            NativeOpenCode2ProfileError::ProfileAtomicPublishFailed,
            NativeOpenCode2ProfileError::ProfileLockUnavailable,
            NativeOpenCode2ProfileError::CertifiedEngineUnavailable,
        ];
        for error in profile_errors {
            assert!(!error.to_string().contains("C:\\secret"));
            assert!(!format!("{error:?}").contains("profiles.json"));
            assert!(!format!("{error:?}").contains("credential"));
        }
        let launch_errors = [
            NativeOpenCode2ProfileLaunchError::UnsupportedPlatform,
            NativeOpenCode2ProfileLaunchError::ProfileRegistryTooLarge,
            NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed,
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedVersion,
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedEngine,
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsafe,
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable,
            NativeOpenCode2ProfileLaunchError::DuplicateProfile,
            NativeOpenCode2ProfileLaunchError::MultiplePrimaryProfiles,
            NativeOpenCode2ProfileLaunchError::ProfileNotFound,
            NativeOpenCode2ProfileLaunchError::ProfileHomeUnsafe,
            NativeOpenCode2ProfileLaunchError::ProfileHomeUnavailable,
            NativeOpenCode2ProfileLaunchError::LockUnavailable,
            NativeOpenCode2ProfileLaunchError::InstallStateMissing,
            NativeOpenCode2ProfileLaunchError::InstallStateInvalid,
            NativeOpenCode2ProfileLaunchError::GenerationUnsafe,
            NativeOpenCode2ProfileLaunchError::GenerationUntrusted,
            NativeOpenCode2ProfileLaunchError::ExecutableUnavailable,
            NativeOpenCode2ProfileLaunchError::ExecutableChanged,
            NativeOpenCode2ProfileLaunchError::ExecutableSizeMismatch,
            NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch,
            NativeOpenCode2ProfileLaunchError::ProfileChanged,
        ];
        for error in launch_errors {
            assert!(!error.to_string().contains("C:\\secret"));
            assert!(!format!("{error:?}").contains("profiles.json"));
            assert!(!format!("{error:?}").contains("OPENCODE_PASSWORD"));
        }
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn exact_default_and_primary_named_profile_resolution() {
        let (_root, database, authority, paths) = installed_fixture();
        let default_id = EngineProfileId::parse("default").unwrap();
        let primary_id = EngineProfileId::parse("work").unwrap();
        assert_eq!(
            authority.register_profile(&database, &default_id, ProfileHomeKind::Named),
            Ok(ProfileRegistrationOutcome::Registered)
        );
        assert_eq!(
            authority.register_profile(&database, &primary_id, ProfileHomeKind::Primary),
            Ok(ProfileRegistrationOutcome::Registered)
        );

        let named = authority
            .resolve_profile_launch(&database, &default_id)
            .unwrap();
        assert_eq!(named.profile_id(), &default_id);
        assert_eq!(named.home(), ProfileHomeKind::Named);
        assert_eq!(
            named.profile_home(),
            paths.engine_root().join("homes").join("default")
        );
        assert_eq!(named.generation_id(), generation_id());
        assert_eq!(named.version(), "1.2.3-test");
        assert_eq!(named.executable_path(), test_executable(&paths));
        drop(named);
        assert!(matches!(
            authority
                .resolve_profile_launch(&database, &EngineProfileId::parse("missing").unwrap()),
            Err(NativeOpenCode2ProfileLaunchError::ProfileNotFound)
        ));

        let primary = authority
            .resolve_profile_launch(&database, &primary_id)
            .unwrap();
        assert_eq!(primary.home(), ProfileHomeKind::Primary);
        assert_eq!(primary.profile_home(), paths.engine_root().join("home"));
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn launch_rejects_missing_duplicate_and_malformed_registries() {
        let (_root, database, authority, paths) = installed_fixture();
        let id = EngineProfileId::parse("default").unwrap();
        assert!(matches!(
            authority.resolve_profile_launch(&database, &id),
            Err(NativeOpenCode2ProfileLaunchError::ProfileNotFound)
        ));
        let registry = paths.engine_root().join("profiles.json");
        fs::write(
            &registry,
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"default","home":"named"},{"profile_id":"default","home":"named"}]}"#,
        )
        .unwrap();
        assert!(matches!(
            authority.resolve_profile_launch(&database, &id),
            Err(NativeOpenCode2ProfileLaunchError::DuplicateProfile)
        ));
        fs::write(
            &registry,
            br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"default","home":"other"}]}"#,
        )
        .unwrap();
        assert!(matches!(
            authority.resolve_profile_launch(&database, &id),
            Err(NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed)
        ));
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn generation_replacement_and_executable_identity_drift_fail_revalidation() {
        let (_root, database, authority, paths) = installed_fixture();
        let id = EngineProfileId::parse("default").unwrap();
        authority
            .register_profile(&database, &id, ProfileHomeKind::Named)
            .unwrap();
        let launch = authority.resolve_profile_launch(&database, &id).unwrap();
        let replacement_id = "generation-fedcba9876543210fedcba9876543210";
        write_generation(&paths, replacement_id, b"test executable");
        let state = authority.new_install_state(replacement_id, None).unwrap();
        assert_eq!(
            authority.write_install_state(paths.engine_root(), &state),
            Ok(AtomicReplaceOutcome::Committed)
        );
        assert_eq!(
            launch.revalidate(),
            Err(NativeOpenCode2ProfileLaunchError::ProfileChanged)
        );
        drop(launch);
        let replacement = authority.resolve_profile_launch(&database, &id).unwrap();
        assert_eq!(replacement.generation_id(), replacement_id);

        let executable = executable_for(&paths, replacement_id);
        fs::remove_file(&executable).unwrap();
        fs::write(&executable, b"test executable").unwrap();
        assert_eq!(
            replacement.revalidate(),
            Err(NativeOpenCode2ProfileLaunchError::ProfileChanged)
        );
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn executable_size_and_hash_failures_are_distinct_and_lock_is_retained() {
        let (_root, database, authority, paths) = installed_fixture();
        let id = EngineProfileId::parse("default").unwrap();
        authority
            .register_profile(&database, &id, ProfileHomeKind::Named)
            .unwrap();
        let executable = test_executable(&paths);
        fs::write(&executable, b"wrong").unwrap();
        assert!(matches!(
            authority.resolve_profile_launch(&database, &id),
            Err(NativeOpenCode2ProfileLaunchError::ExecutableSizeMismatch)
        ));
        fs::write(&executable, b"wrong content!!").unwrap();
        assert!(matches!(
            authority.resolve_profile_launch(&database, &id),
            Err(NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch)
        ));
        fs::write(&executable, b"test executable").unwrap();
        let launch = authority.resolve_profile_launch(&database, &id).unwrap();
        assert!(matches!(
            NativeOpenCode2InstallLock::try_acquire(&paths),
            Err(NativeOpenCode2InstallLockError::Busy)
        ));
        assert!(!format!("{launch:?}").contains(&database.to_string_lossy().to_string()));
        assert_eq!(
            launch.to_string(),
            "verified OpenCode2 profile launch capability"
        );
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    fn installed_fixture() -> (
        tempfile::TempDir,
        PathBuf,
        NativeOpenCode2Authority,
        NativeOpenCode2InstallPaths,
    ) {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("artisan.sqlite");
        let authority = NativeOpenCode2Authority::test();
        let paths = authority.install_paths(&database).unwrap();
        paths.prepare().unwrap();
        write_generation(&paths, generation_id(), b"test executable");
        let state = authority.new_install_state(generation_id(), None).unwrap();
        assert_eq!(
            authority.write_install_state(paths.engine_root(), &state),
            Ok(AtomicReplaceOutcome::Committed)
        );
        (root, database, authority, paths)
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    fn generation_id() -> &'static str {
        "generation-0123456789abcdef0123456789abcdef"
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    fn test_executable(paths: &NativeOpenCode2InstallPaths) -> PathBuf {
        executable_for(paths, generation_id())
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    fn executable_for(paths: &NativeOpenCode2InstallPaths, id: &str) -> PathBuf {
        paths.versions_root().join(id).join("opencode2.exe")
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    fn write_generation(paths: &NativeOpenCode2InstallPaths, id: &str, bytes: &[u8]) {
        let generation = paths.versions_root().join(id);
        fs::create_dir_all(&generation).unwrap();
        fs::write(generation.join("opencode2.exe"), bytes).unwrap();
    }
}
