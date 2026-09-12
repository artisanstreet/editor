use crate::{
    CliError, Result,
    engine_catalog::{NativeOpenCode2Authority, OpenCode2Inspection},
    engine_install::{self, InstallOutcome},
    instance::NativeInstanceConfig,
    paths::Layout,
};

use super::{EngineCommand, load_native_instance, require_installation};

pub(super) fn engine_command(layout: &Layout, command: &EngineCommand) -> Result<()> {
    if matches!(command, EngineCommand::Install) {
        require_installation(layout).map_err(|_| CliError::OpenCode2Install {
            reason: "installation_invalid",
        })?;
        let instance = load_native_instance(layout).map_err(|_| CliError::OpenCode2Install {
            reason: "instance_invalid",
        })?;
        return match engine_install::install(&instance).map_err(|error| {
            CliError::OpenCode2Install {
                reason: error.cli_reason(),
            }
        })? {
            InstallOutcome::Installed => {
                println!("OpenCode2 installed");
                Ok(())
            }
            InstallOutcome::AlreadyInstalled => {
                println!("OpenCode2 already installed");
                Ok(())
            }
        };
    }

    let is_profile = matches!(command, EngineCommand::Profile { .. });
    if is_profile {
        require_installation(layout).map_err(|_| profile_surface_error())?;
    } else {
        require_installation(layout)?;
    }
    let instance = load_native_instance(layout).map_err(|error| {
        if is_profile {
            profile_surface_error()
        } else {
            match error {
                CliError::MissingInstance => CliError::MissingInstance,
                _ => CliError::OpenCode2Authority {
                    reason: "instance_invalid",
                },
            }
        }
    })?;
    match command {
        EngineCommand::List { json } => list_engines(&instance, *json),
        EngineCommand::Install => unreachable!("install is handled above"),
        EngineCommand::Profile { command } => crate::engine_profiles::run(&instance, command),
    }
}

pub(super) fn profile_surface_error() -> CliError {
    CliError::OpenCode2Profile {
        reason: "profile_registry_invalid",
    }
}

fn list_engines(instance: &NativeInstanceConfig, json: bool) -> Result<()> {
    let authority = NativeOpenCode2Authority::new();
    let spec = NativeOpenCode2Authority::certified_install_spec();
    let inspection = authority.inspect(instance.database_path());
    match inspection {
        Ok(OpenCode2Inspection::UnsupportedPlatform) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema": "artisan-engine-list-v1",
                        "engines": [{
                            "engine_id": spec.engine_id(),
                            "status": "unsupported_platform",
                        }],
                    })
                );
            } else {
                println!("OpenCode2: unsupported platform");
            }
            Ok(())
        }
        Ok(OpenCode2Inspection::NotInstalled) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema": "artisan-engine-list-v1",
                        "engines": [{
                            "engine_id": spec.engine_id(),
                            "status": "not_installed",
                        }],
                    })
                );
            } else {
                println!("OpenCode2: not installed");
            }
            Ok(())
        }
        Ok(OpenCode2Inspection::Ready(generation)) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema": "artisan-engine-list-v1",
                        "engines": [{
                            "engine_id": spec.engine_id(),
                            "status": "ready",
                            "generation": generation.generation_id(),
                            "version": spec.version(),
                            "upstream_commit": spec.upstream_commit(),
                            "binary": spec.binary(),
                            "size_bytes": spec.executable_size_bytes(),
                            "sha256": spec.executable_sha256_hex(),
                        }],
                    })
                );
            } else {
                println!(
                    "OpenCode2: ready ({}, generation {})",
                    spec.version(),
                    generation.generation_id()
                );
            }
            Ok(())
        }
        Err(error) => {
            let reason = error.cli_reason();
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema": "artisan-engine-list-v1",
                        "engines": [{
                            "engine_id": spec.engine_id(),
                            "status": "invalid",
                            "reason": reason,
                        }],
                    })
                );
            }
            Err(CliError::OpenCode2Authority { reason })
        }
    }
}
