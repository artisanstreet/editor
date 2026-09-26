//! Registry-facing authority operations: path derivation, registration,
//! listing, and exact reads.
//!
//! Split out of `profile.rs`; launch resolution lives in `launch.rs`.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use artisan_domain::EngineProfileId;

use crate::engine_core::{ManagedInstallLock, NativeOpenCode2Authority};
use crate::io as files;
use crate::io::AtomicReplaceOutcome;

use super::registry::{
    derived_profile_home, encode_profile_registry, map_profile_atomic_error,
    map_profile_home_error, map_profile_path_error, read_exact_profile_from_path,
    read_profile_registry, register_in_registry, validate_profile_home,
};
use super::types::{
    NativeOpenCode2ProfileError, OpenCode2Profile, ProfileHomeKind, ProfileRegistrationOutcome,
    ProfileRegistry,
};
impl NativeOpenCode2Authority {
    /// Returns the profile registry location under the certified engine root.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOpenCode2ProfileError`] when the database path is
    /// unsafe or the certified installation root is unavailable.
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
    pub fn register_profile(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
        home: ProfileHomeKind,
    ) -> Result<ProfileRegistrationOutcome, NativeOpenCode2ProfileError> {
        if !self.platform_supported() {
            return Err(NativeOpenCode2ProfileError::CertifiedEngineUnavailable);
        }
        let _pre_lock_generation = self
            .resolve_active(database_path)
            .map_err(|_| NativeOpenCode2ProfileError::CertifiedEngineUnavailable)?;
        let paths = self
            .install_paths(database_path)
            .map_err(map_profile_path_error)?;
        let lock = ManagedInstallLock::acquire(&paths)
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
    pub fn read_profile(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
    ) -> Result<OpenCode2Profile, NativeOpenCode2ProfileError> {
        let registry_path = self.profile_registry_path(database_path)?;
        read_exact_profile_from_path(&registry_path, profile_id)
    }
}
