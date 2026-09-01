use artisan_domain::EngineProfileId;
use artisan_native_engine::{
    NativeOpenCode2Authority, NativeOpenCode2ProfileError, NativeOpenCode2ProfileLaunchError,
    OpenCode2Profile, ProfileHomeKind, ProfileRegistrationOutcome, VerifiedOpenCode2ProfileLaunch,
};
use clap::{Subcommand, ValueEnum};

use crate::{CliError, instance::NativeInstanceConfig};

pub(crate) type EngineProfileSummary = OpenCode2Profile;

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
    /// Verify one exact registered `OpenCode2` profile launch.
    Verify {
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

pub(crate) fn run(
    instance: &NativeInstanceConfig,
    command: &EngineProfileCommand,
) -> crate::Result<()> {
    let authority = NativeOpenCode2Authority::new();
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
            let profiles = authority
                .list_profiles(instance.database_path())
                .map_err(profile_error)?;
            print_profile_list(profiles, *json);
            Ok(())
        }
        EngineProfileCommand::Read { profile_id, json } => {
            let profile = authority
                .read_profile(instance.database_path(), profile_id)
                .map_err(profile_error)?;
            print_profile_read(&profile, *json);
            Ok(())
        }
        EngineProfileCommand::Verify { profile_id, json } => {
            let launch = NativeOpenCode2Authority::new()
                .resolve_profile_launch(instance.database_path(), profile_id)
                .map_err(launch_error)?;
            print_verify(&launch, *json);
            drop(launch);
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

fn print_verify(launch: &VerifiedOpenCode2ProfileLaunch, json: bool) {
    let profile_id = launch.profile_id().as_str();
    let home = launch.home().as_str();
    let generation = launch.generation_id();
    let version = launch.version();
    let size_bytes = launch.executable_size_bytes();
    let sha256 = hex_sha256(launch.executable_sha256());
    if json {
        println!(
            "{}",
            serde_json::json!({
                "schema": "artisan-engine-profile-verify-v1",
                "profile_id": profile_id,
                "home": home,
                "status": "verified",
                "generation": generation,
                "version": version,
                "size_bytes": size_bytes,
                "sha256": sha256,
            })
        );
    } else {
        println!(
            "Verified OpenCode2 profile {profile_id} ({home}) generation {generation} {version} size {size_bytes} sha256 {sha256}"
        );
    }
}

fn hex_sha256(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn profile_error(error: NativeOpenCode2ProfileError) -> CliError {
    CliError::OpenCode2Profile {
        reason: error.cli_reason(),
    }
}

fn launch_error(error: NativeOpenCode2ProfileLaunchError) -> CliError {
    CliError::OpenCode2Profile {
        reason: launch_cli_reason(error),
    }
}

fn launch_cli_reason(error: NativeOpenCode2ProfileLaunchError) -> &'static str {
    match error {
        NativeOpenCode2ProfileLaunchError::UnsupportedPlatform => "unsupported_platform",
        NativeOpenCode2ProfileLaunchError::ProfileRegistryTooLarge
        | NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed
        | NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedVersion
        | NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedEngine
        | NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsafe
        | NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable
        | NativeOpenCode2ProfileLaunchError::DuplicateProfile
        | NativeOpenCode2ProfileLaunchError::MultiplePrimaryProfiles => "profile_registry_invalid",
        NativeOpenCode2ProfileLaunchError::ProfileNotFound => "profile_not_found",
        NativeOpenCode2ProfileLaunchError::ProfileHomeUnsafe => "profile_home_unsafe",
        NativeOpenCode2ProfileLaunchError::ProfileHomeUnavailable => "profile_home_unavailable",
        NativeOpenCode2ProfileLaunchError::LockUnavailable => "profile_lock_unavailable",
        NativeOpenCode2ProfileLaunchError::InstallStateMissing => "install_state_missing",
        NativeOpenCode2ProfileLaunchError::InstallStateInvalid => "install_state_invalid",
        NativeOpenCode2ProfileLaunchError::GenerationUnsafe => "generation_unsafe",
        NativeOpenCode2ProfileLaunchError::GenerationUntrusted => "generation_untrusted",
        NativeOpenCode2ProfileLaunchError::ExecutableUnavailable => "executable_unavailable",
        NativeOpenCode2ProfileLaunchError::ExecutableChanged => "executable_changed",
        NativeOpenCode2ProfileLaunchError::ExecutableSizeMismatch => "executable_size_mismatch",
        NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch => "executable_hash_mismatch",
        NativeOpenCode2ProfileLaunchError::ProfileChanged => "profile_changed",
    }
}

pub(crate) fn register_profile(
    instance: &NativeInstanceConfig,
    profile_id: &EngineProfileId,
    home: ProfileHomeKind,
) -> Result<ProfileRegistrationOutcome, NativeOpenCode2ProfileError> {
    NativeOpenCode2Authority::new().register_profile(instance.database_path(), profile_id, home)
}

fn parse_engine_profile_id(value: &str) -> std::result::Result<EngineProfileId, String> {
    EngineProfileId::parse(value).map_err(|error| error.to_string())
}
