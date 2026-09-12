//! Validated profile records and the bounded profile/launch error types.
//!
//! Split out of `profile.rs` during the module split. Path-free errors are
//! intentionally payload-free so paths, registry bytes, and OS messages cannot
//! escape through diagnostics.

#![forbid(unsafe_code)]

use std::fmt;

use artisan_domain::EngineProfileId;
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
    pub(super) profile_id: EngineProfileId,
    pub(super) home: ProfileHomeKind,
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
pub(super) struct EngineProfileRegistration {
    pub(super) profile_id: EngineProfileId,
    pub(super) home: ProfileHomeKind,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ProfileRegistry {
    pub(super) profiles: Vec<EngineProfileRegistration>,
}

impl ProfileRegistry {
    pub(super) fn empty() -> Self {
        Self {
            profiles: Vec::new(),
        }
    }

    pub(super) fn sort(&mut self) {
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

impl NativeOpenCode2ProfileLaunchError {
    /// Returns the stable CLI classification for this launch failure.
    ///
    /// Registry structural failures intentionally share
    /// `profile_registry_invalid`.
    #[must_use]
    pub const fn cli_reason(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::ProfileRegistryTooLarge
            | Self::ProfileRegistryMalformed
            | Self::ProfileRegistryUnsupportedVersion
            | Self::ProfileRegistryUnsupportedEngine
            | Self::ProfileRegistryUnsafe
            | Self::ProfileRegistryUnavailable
            | Self::DuplicateProfile
            | Self::MultiplePrimaryProfiles => "profile_registry_invalid",
            Self::ProfileNotFound => "profile_not_found",
            Self::ProfileHomeUnsafe => "profile_home_unsafe",
            Self::ProfileHomeUnavailable => "profile_home_unavailable",
            Self::LockUnavailable => "profile_lock_unavailable",
            Self::InstallStateMissing => "install_state_missing",
            Self::InstallStateInvalid => "install_state_invalid",
            Self::GenerationUnsafe => "generation_unsafe",
            Self::GenerationUntrusted => "generation_untrusted",
            Self::ExecutableUnavailable => "executable_unavailable",
            Self::ExecutableChanged => "executable_changed",
            Self::ExecutableSizeMismatch => "executable_size_mismatch",
            Self::ExecutableHashMismatch => "executable_hash_mismatch",
            Self::ProfileChanged => "profile_changed",
        }
    }
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
