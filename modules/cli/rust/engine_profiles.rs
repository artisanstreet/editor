use std::{
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
};

use artisan_domain::EngineProfileId;
use clap::{Subcommand, ValueEnum};
use serde::{
    Deserialize, Serialize,
    de::{self, Deserializer, MapAccess, Visitor},
};

use crate::{
    CliError, credentials,
    engine_catalog::NativeOpenCode2Authority,
    engine_install,
    instance::{self, NativeAtomicReplaceOutcome, NativeInstanceConfig, NativeInstanceError},
};

const MAX_PROFILE_REGISTRY_BYTES: usize = 16 * 1024;
const MAX_PROFILES: usize = 64;
const PROFILE_REGISTRY_FORMAT_VERSION: u64 = 1;

const PROFILE_REGISTRY_FIELDS: &[&str] = &["engine_id", "format_version", "profiles"];
const PROFILE_FIELDS: &[&str] = &["profile_id", "home"];

#[derive(Debug, Subcommand)]
pub enum EngineProfileCommand {
    /// Register one explicit `OpenCode2` profile home.
    Register {
        #[arg(long, required = true, value_parser = parse_engine_profile_id)]
        profile_id: EngineProfileId,
        #[arg(long, required = true, value_enum)]
        home: EngineProfileHomeArg,
    },
    /// List registered `OpenCode2` profile homes.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Read one exact registered `OpenCode2` profile home.
    Read {
        #[arg(long, required = true, value_parser = parse_engine_profile_id)]
        profile_id: EngineProfileId,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum EngineProfileHomeArg {
    Primary,
    Named,
}

impl From<EngineProfileHomeArg> for ProfileHomeKind {
    fn from(value: EngineProfileHomeArg) -> Self {
        match value {
            EngineProfileHomeArg::Primary => Self::Primary,
            EngineProfileHomeArg::Named => Self::Named,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProfileHomeKind {
    Primary,
    Named,
}

impl ProfileHomeKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Named => "named",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EngineProfileRegistration {
    profile_id: EngineProfileId,
    home: ProfileHomeKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EngineProfileSummary {
    profile_id: EngineProfileId,
    home: ProfileHomeKind,
}

impl EngineProfileSummary {
    pub(crate) fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    pub(crate) const fn home(&self) -> ProfileHomeKind {
        self.home
    }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeOpenCode2ProfileError {
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
    pub(crate) const fn cli_reason(self) -> &'static str {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProfileRegistrationOutcome {
    Registered,
    AlreadyRegistered,
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

pub(crate) fn run(
    instance: &NativeInstanceConfig,
    command: &EngineProfileCommand,
) -> crate::Result<()> {
    match command {
        EngineProfileCommand::Register { profile_id, home } => {
            let home = (*home).into();
            let outcome = register_profile(instance, profile_id, home).map_err(profile_error)?;
            let status = match outcome {
                ProfileRegistrationOutcome::Registered => "Registered",
                ProfileRegistrationOutcome::AlreadyRegistered => "Already registered",
            };
            println!(
                "{status} OpenCode2 profile {profile_id} ({})",
                home.as_str()
            );
            Ok(())
        }
        EngineProfileCommand::List { json } => {
            let profiles = list_profiles(instance).map_err(profile_error)?;
            print_profile_list(profiles, *json);
            Ok(())
        }
        EngineProfileCommand::Read { profile_id, json } => {
            let profile = read_profile(instance, profile_id).map_err(profile_error)?;
            print_profile_read(&profile, *json);
            Ok(())
        }
    }
}

fn print_profile_list(profiles: Option<Vec<EngineProfileSummary>>, json: bool) {
    let status = if profiles.is_some() {
        "registered"
    } else {
        "not_registered"
    };
    let profiles = profiles.unwrap_or_default();
    if json {
        let profiles = profiles
            .iter()
            .map(|profile| {
                serde_json::json!({
                    "profile_id": profile.profile_id().as_str(),
                    "home": profile.home().as_str(),
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::json!({
                "schema": "artisan-engine-profile-list-v1",
                "status": status,
                "profiles": profiles,
            })
        );
        return;
    }

    if profiles.is_empty() {
        println!("OpenCode2 profiles: {status}");
    } else {
        println!("OpenCode2 profiles: {status}");
        for profile in profiles {
            println!("{} ({})", profile.profile_id(), profile.home().as_str());
        }
    }
}

fn print_profile_read(profile: &EngineProfileSummary, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "schema": "artisan-engine-profile-v1",
                "profile_id": profile.profile_id().as_str(),
                "home": profile.home().as_str(),
                "status": "registered",
            })
        );
    } else {
        println!(
            "OpenCode2 profile {} ({})",
            profile.profile_id(),
            profile.home().as_str()
        );
    }
}

fn profile_error(error: NativeOpenCode2ProfileError) -> CliError {
    CliError::OpenCode2Profile {
        reason: error.cli_reason(),
    }
}

pub(crate) fn register_profile(
    instance: &NativeInstanceConfig,
    profile_id: &EngineProfileId,
    home: ProfileHomeKind,
) -> Result<ProfileRegistrationOutcome, NativeOpenCode2ProfileError> {
    let authority = NativeOpenCode2Authority::new();
    authority
        .resolve_active(instance)
        .map_err(|_| NativeOpenCode2ProfileError::CertifiedEngineUnavailable)?;

    let lock = engine_install::acquire_install_lock(instance)
        .map_err(|_| NativeOpenCode2ProfileError::ProfileLockUnavailable)?;
    lock.fence_instance(instance)
        .map_err(|_| NativeOpenCode2ProfileError::ProfileLockUnavailable)?;
    authority
        .resolve_active(instance)
        .map_err(|_| NativeOpenCode2ProfileError::CertifiedEngineUnavailable)?;
    lock.fence_instance(instance)
        .map_err(|_| NativeOpenCode2ProfileError::ProfileLockUnavailable)?;

    let registry_path = profile_registry_path(instance)?;
    let mut registry =
        read_profile_registry(&registry_path)?.unwrap_or_else(ProfileRegistry::empty);

    let outcome = register_in_registry(&mut registry, profile_id, home)?;
    if outcome == ProfileRegistrationOutcome::AlreadyRegistered {
        let profile_home = derived_profile_home(instance, profile_id, home)?;
        credentials::validate_private_directory(&profile_home)
            .map_err(|error| map_private_directory_error(&error))?;
        lock.fence_instance(instance)
            .map_err(|_| NativeOpenCode2ProfileError::ProfileLockUnavailable)?;
        return Ok(ProfileRegistrationOutcome::AlreadyRegistered);
    }
    let bytes = encode_profile_registry(&registry)?;

    let profile_home = derived_profile_home(instance, profile_id, home)?;
    credentials::ensure_private_directory(&profile_home)
        .map_err(|error| map_private_directory_error(&error))?;
    lock.fence_instance(instance)
        .map_err(|_| NativeOpenCode2ProfileError::ProfileLockUnavailable)?;
    match instance::replace_native_file(&registry_path, &bytes)
        .map_err(|error| map_profile_atomic_error(&error))?
    {
        NativeAtomicReplaceOutcome::Committed => Ok(ProfileRegistrationOutcome::Registered),
        NativeAtomicReplaceOutcome::CommittedButUnverified => {
            Err(NativeOpenCode2ProfileError::ProfileAtomicPublishFailed)
        }
    }
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

fn list_profiles(
    instance: &NativeInstanceConfig,
) -> Result<Option<Vec<EngineProfileSummary>>, NativeOpenCode2ProfileError> {
    let registry_path = profile_registry_path(instance)?;
    let Some(registry) = read_profile_registry(&registry_path)? else {
        return Ok(None);
    };
    Ok(Some(
        registry
            .profiles
            .into_iter()
            .map(|profile| EngineProfileSummary {
                profile_id: profile.profile_id,
                home: profile.home,
            })
            .collect(),
    ))
}

fn read_profile(
    instance: &NativeInstanceConfig,
    profile_id: &EngineProfileId,
) -> Result<EngineProfileSummary, NativeOpenCode2ProfileError> {
    let registry_path = profile_registry_path(instance)?;
    let Some(registry) = read_profile_registry(&registry_path)? else {
        return Err(NativeOpenCode2ProfileError::ProfileNotFound);
    };
    registry
        .profiles
        .into_iter()
        .find(|profile| profile.profile_id == *profile_id)
        .map(|profile| EngineProfileSummary {
            profile_id: profile.profile_id,
            home: profile.home,
        })
        .ok_or(NativeOpenCode2ProfileError::ProfileNotFound)
}

fn profile_registry_path(
    instance: &NativeInstanceConfig,
) -> Result<PathBuf, NativeOpenCode2ProfileError> {
    NativeOpenCode2Authority::new()
        .managed_engine_root(instance)
        .map(|root| root.join("profiles.json"))
        .map_err(|_| NativeOpenCode2ProfileError::ProfileRegistryUnsafe)
}

fn derived_profile_home(
    instance: &NativeInstanceConfig,
    profile_id: &EngineProfileId,
    home: ProfileHomeKind,
) -> Result<PathBuf, NativeOpenCode2ProfileError> {
    let engine_root = NativeOpenCode2Authority::new()
        .managed_engine_root(instance)
        .map_err(|_| NativeOpenCode2ProfileError::ProfileHomeUnsafe)?;
    Ok(match home {
        ProfileHomeKind::Primary => engine_root.join("home"),
        ProfileHomeKind::Named => engine_root.join("homes").join(profile_id.as_str()),
    })
}

fn read_profile_registry(
    path: &Path,
) -> Result<Option<ProfileRegistry>, NativeOpenCode2ProfileError> {
    let bytes = match instance::read_bounded_native_file(path, MAX_PROFILE_REGISTRY_BYTES) {
        Ok(bytes) => bytes,
        Err(NativeInstanceError::NotFound) => return Ok(None),
        Err(NativeInstanceError::TooLarge) => {
            return Err(NativeOpenCode2ProfileError::ProfileRegistryTooLarge);
        }
        Err(NativeInstanceError::UnsafePath(_)) => {
            return Err(NativeOpenCode2ProfileError::ProfileRegistryUnsafe);
        }
        Err(_) => return Err(NativeOpenCode2ProfileError::ProfileRegistryUnavailable),
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

fn map_private_directory_error(
    error: &credentials::ForgeCredentialError,
) -> NativeOpenCode2ProfileError {
    match error {
        credentials::ForgeCredentialError::InvalidHome(_)
        | credentials::ForgeCredentialError::UnsafePath(_)
        | credentials::ForgeCredentialError::WindowsAcl => {
            NativeOpenCode2ProfileError::ProfileHomeUnsafe
        }
        credentials::ForgeCredentialError::Io { .. }
        | credentials::ForgeCredentialError::ManifestMalformed
        | credentials::ForgeCredentialError::ManifestSchema
        | credentials::ForgeCredentialError::ManifestVersion
        | credentials::ForgeCredentialError::ManifestTraversal
        | credentials::ForgeCredentialError::ManifestUnknownField
        | credentials::ForgeCredentialError::ManifestDuplicateField
        | credentials::ForgeCredentialError::PartialBundle
        | credentials::ForgeCredentialError::InvalidCapability { .. }
        | credentials::ForgeCredentialError::InvalidCertificate
        | credentials::ForgeCredentialError::KeyMismatch
        | credentials::ForgeCredentialError::Provisioning => {
            NativeOpenCode2ProfileError::ProfileHomeUnavailable
        }
    }
}

fn map_profile_atomic_error(error: &NativeInstanceError) -> NativeOpenCode2ProfileError {
    match error {
        NativeInstanceError::UnsafePath(_) => NativeOpenCode2ProfileError::ProfileRegistryUnsafe,
        _ => NativeOpenCode2ProfileError::ProfileAtomicPublishFailed,
    }
}

fn parse_engine_profile_id(value: &str) -> std::result::Result<EngineProfileId, String> {
    EngineProfileId::parse(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_registry_decoding_is_strict_and_publication_is_typed_sorted() {
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
    fn profile_registry_rejects_unknown_duplicate_missing_trailing_and_unsupported_values() {
        let malformed = [
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
        ];
        for bytes in malformed {
            assert!(
                matches!(
                    decode_profile_registry(bytes),
                    Err(NativeOpenCode2ProfileError::ProfileRegistryMalformed
                        | NativeOpenCode2ProfileError::ProfileRegistryUnsupportedEngine
                        | NativeOpenCode2ProfileError::ProfileRegistryUnsupportedVersion)
                ),
                "unexpectedly accepted malformed registry: {}",
                String::from_utf8_lossy(bytes)
            );
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
        assert_eq!(encode_profile_registry(&registry).unwrap(), before);

        let primary = EngineProfileId::parse("primary").unwrap();
        assert_eq!(
            register_in_registry(&mut registry, &primary, ProfileHomeKind::Primary),
            Ok(ProfileRegistrationOutcome::Registered)
        );
        let second_primary = EngineProfileId::parse("second").unwrap();
        let after_primary = encode_profile_registry(&registry).unwrap();
        assert_eq!(
            register_in_registry(&mut registry, &second_primary, ProfileHomeKind::Primary),
            Err(NativeOpenCode2ProfileError::PrimaryAlreadyRegistered)
        );
        assert_eq!(encode_profile_registry(&registry).unwrap(), after_primary);

        let mut full = ProfileRegistry {
            profiles: Vec::new(),
        };
        for index in 0..MAX_PROFILES {
            full.profiles.push(EngineProfileRegistration {
                profile_id: EngineProfileId::parse(format!("profile-{index:02}")).unwrap(),
                home: ProfileHomeKind::Named,
            });
        }
        full.sort();
        let full_before = encode_profile_registry(&full).unwrap();
        let extra = EngineProfileId::parse("extra").unwrap();
        assert_eq!(
            register_in_registry(&mut full, &extra, ProfileHomeKind::Named),
            Err(NativeOpenCode2ProfileError::ProfileLimit)
        );
        assert_eq!(encode_profile_registry(&full).unwrap(), full_before);
    }

    #[test]
    fn profile_home_derivation_uses_only_the_certified_engine_root() {
        let root = tempfile::tempdir().unwrap();
        let instance = sample_instance(root.path());
        let profile_id = EngineProfileId::parse("work.profile").unwrap();
        assert_eq!(
            derived_profile_home(&instance, &profile_id, ProfileHomeKind::Primary).unwrap(),
            root.path()
                .join("data")
                .join("toolchain")
                .join("opencode2")
                .join("home")
        );
        assert_eq!(
            derived_profile_home(&instance, &profile_id, ProfileHomeKind::Named).unwrap(),
            root.path()
                .join("data")
                .join("toolchain")
                .join("opencode2")
                .join("homes")
                .join("work.profile")
        );
    }

    #[test]
    fn list_and_read_missing_registry_are_read_only_and_never_select_a_default() {
        let root = tempfile::tempdir().unwrap();
        let instance = sample_instance(root.path());
        let registry_path = profile_registry_path(&instance).unwrap();
        assert_eq!(list_profiles(&instance).unwrap(), None);
        assert_eq!(
            read_profile(&instance, &EngineProfileId::parse("default").unwrap()),
            Err(NativeOpenCode2ProfileError::ProfileNotFound)
        );
        assert!(!registry_path.exists());
        assert!(!root.path().join("data").exists());
    }

    #[cfg(unix)]
    #[test]
    fn profile_home_reuses_private_directory_mode_and_rejects_unsafe_targets() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        credentials::ensure_private_directory(&private).unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&private)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            credentials::ensure_private_directory(&private),
            Err(credentials::ForgeCredentialError::WindowsAcl)
        );
        assert_eq!(
            std::fs::symlink_metadata(&private)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );

        let missing = root.path().join("missing");
        assert!(matches!(
            credentials::validate_private_directory(&missing),
            Err(credentials::ForgeCredentialError::Io { .. })
        ));
        assert!(!missing.exists());

        let target = root.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = root.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(matches!(
            credentials::ensure_private_directory(&link),
            Err(credentials::ForgeCredentialError::UnsafePath(_))
        ));

        let file = root.path().join("file");
        std::fs::write(&file, b"not a directory").unwrap();
        assert!(matches!(
            credentials::ensure_private_directory(&file),
            Err(credentials::ForgeCredentialError::UnsafePath(_))
        ));
    }

    #[test]
    fn profile_errors_never_embed_paths_or_credentials() {
        let errors = [
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
        for error in errors {
            assert!(!error.to_string().contains("C:\\secret"));
            assert!(!format!("{error:?}").contains("credential"));
            assert!(!format!("{error:?}").contains("profiles.json"));
        }
    }

    fn sample_instance(root: &Path) -> NativeInstanceConfig {
        NativeInstanceConfig::new(
            root.join("data").join("artisan.sqlite"),
            root.join("custody").join("lock"),
            root.join("readiness").join("ready"),
            root.join("credentials").join("manifest.json"),
            sample_listener(),
        )
        .unwrap()
    }

    fn sample_listener() -> crate::instance::NativeListenerConfig {
        use std::num::NonZeroU32;

        crate::instance::NativeListenerConfig::new(
            1,
            2,
            3,
            4,
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1).unwrap(),
        )
    }
}
