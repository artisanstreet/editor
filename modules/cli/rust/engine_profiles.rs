use artisan_domain::EngineProfileId;
use artisan_native_engine::{
    NativeOpenCode2Authority, NativeOpenCode2ProfileError, OpenCode2Profile, ProfileHomeKind,
    ProfileRegistrationOutcome,
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
    NativeOpenCode2Authority::new().register_profile(instance.database_path(), profile_id, home)
}

fn parse_engine_profile_id(value: &str) -> std::result::Result<EngineProfileId, String> {
    EngineProfileId::parse(value).map_err(|error| error.to_string())
}
