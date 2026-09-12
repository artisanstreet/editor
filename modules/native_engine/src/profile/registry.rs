//! Bounded profile registry codec, validation, and home derivation.
//!
//! Split out of `profile.rs`; the registry is a strict, bounded JSON document
//! with no discovery or fallback semantics.

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use artisan_domain::EngineProfileId;
use serde::{
    Deserialize, Serialize,
    de::{self, Deserializer, MapAccess, Visitor},
};

use crate::engine_core::{
    NativeOpenCode2Authority, NativeOpenCode2InstallPathError, NativeOpenCode2InstallPaths,
};
use crate::io as files;
use crate::io::NativeFileError;

use super::types::{
    EngineProfileRegistration, NativeOpenCode2ProfileError, OpenCode2Profile, ProfileHomeKind,
    ProfileRegistrationOutcome, ProfileRegistry,
};
use super::{
    MAX_PROFILE_REGISTRY_BYTES, MAX_PROFILES, PROFILE_FIELDS, PROFILE_REGISTRY_FIELDS,
    PROFILE_REGISTRY_FORMAT_VERSION,
};
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

pub(super) fn register_in_registry(
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

pub(super) fn derived_profile_home(
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

pub(super) fn read_exact_profile_from_path(
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

pub(super) fn read_profile_registry(
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

pub(super) fn decode_profile_registry(
    bytes: &[u8],
) -> Result<ProfileRegistry, NativeOpenCode2ProfileError> {
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

pub(super) fn encode_profile_registry(
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

pub(super) fn validate_profile_home(path: &Path) -> Result<(), NativeFileError> {
    files::validate_private_directory(path)
}

pub(super) fn map_profile_path_error(
    error: NativeOpenCode2InstallPathError,
) -> NativeOpenCode2ProfileError {
    match error {
        NativeOpenCode2InstallPathError::InvalidRoot => {
            NativeOpenCode2ProfileError::ProfileRegistryUnsafe
        }
        NativeOpenCode2InstallPathError::Unavailable => {
            NativeOpenCode2ProfileError::ProfileRegistryUnavailable
        }
    }
}

pub(super) fn map_profile_home_error(error: NativeFileError) -> NativeOpenCode2ProfileError {
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

pub(super) fn map_profile_atomic_error(error: NativeFileError) -> NativeOpenCode2ProfileError {
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
