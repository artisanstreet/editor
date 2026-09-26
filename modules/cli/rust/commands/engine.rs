//! `ae engine`: the operator surface of the Forge-managed engine authority.
//!
//! Every subcommand operates on the same authority, state, and selection as
//! the Forge's background engine manager, for the Forge database of this
//! installation's native instance or an explicit `--database`.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use artisan_native_engine::{
    EngineInspection, EngineOperations, EngineSelection, HttpsTransport, InstallError,
    InstallProgress, ManagedEngine, ManagedEngineAuthority, SwitchOutcome, VersionListing,
    read_trust_records, record_for, resolve_launch_target_in,
};

use crate::{CliError, Result, paths::Layout};

use super::{EngineCommand, cli::EngineArg, load_native_instance, require_installation};

pub(super) fn engine_command(
    layout: &Layout,
    database: Option<&Path>,
    command: &EngineCommand,
) -> Result<()> {
    if let EngineCommand::Profile { command } = command {
        require_installation(layout).map_err(|_| profile_surface_error())?;
        let instance = load_native_instance(layout).map_err(|_| profile_surface_error())?;
        return crate::engine_profiles::run(&instance, command);
    }
    let database = forge_database(layout, database)?;
    match command {
        EngineCommand::List { json } => {
            list(&database, *json);
            Ok(())
        }
        EngineCommand::Status { engine, json } => {
            status(&database, engine.engine(), *json);
            Ok(())
        }
        EngineCommand::Versions { engine, json } => versions(&database, engine.engine(), *json),
        EngineCommand::Install { engine } | EngineCommand::Update { engine } => {
            ensure(&database, *engine)
        }
        EngineCommand::Use { engine, selection } => select(&database, engine.engine(), selection),
        EngineCommand::Rollback { engine } => rollback(&database, engine.engine()),
        EngineCommand::Login { engine, args } => login(&database, engine.engine(), args),
        EngineCommand::Profile { .. } => unreachable!("profiles are handled above"),
    }
}

pub(super) fn profile_surface_error() -> CliError {
    CliError::OpenCode2Profile {
        reason: "profile_registry_invalid",
    }
}

fn forge_database(layout: &Layout, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(database) = explicit {
        return if database.is_absolute() {
            Ok(database.to_path_buf())
        } else {
            Err(CliError::EngineAuthority {
                engine: "managed",
                reason: "database_path_not_absolute",
            })
        };
    }
    require_installation(layout)?;
    load_native_instance(layout).map(|instance| instance.database_path().to_path_buf())
}

fn transport(engine: ManagedEngine) -> Result<HttpsTransport> {
    HttpsTransport::new().map_err(|error| CliError::EngineInstall {
        engine: engine.display_name(),
        reason: error.code(),
    })
}

fn install_error(engine: ManagedEngine) -> impl Fn(InstallError) -> CliError {
    move |error| CliError::EngineInstall {
        engine: engine.display_name(),
        reason: error.code(),
    }
}

/// One engine's local status, shared by `list` and `status`.
fn describe(database: &Path, engine: ManagedEngine) -> serde_json::Value {
    let authority = ManagedEngineAuthority::new(engine);
    let paths = authority.install_paths(database).ok();
    let selection = paths
        .as_ref()
        .and_then(|paths| authority.read_selection(paths.engine_root()).ok())
        .map_or_else(|| "invalid".to_owned(), |selection| selection.to_string());
    let state = paths
        .as_ref()
        .and_then(|paths| authority.read_install_state(paths.engine_root()).ok())
        .flatten();
    let overridden = std::env::var_os(engine.override_env()).is_some_and(|value| !value.is_empty());
    let mut value = match authority.inspect(database) {
        Ok(EngineInspection::UnsupportedPlatform(reason)) => serde_json::json!({
            "status": "unsupported",
            "reason": reason.code(),
            "message": reason.message(),
        }),
        Ok(EngineInspection::NotInstalled) => serde_json::json!({ "status": "not_installed" }),
        Ok(EngineInspection::Ready(generation)) => serde_json::json!({
            "status": "ready",
            "version": generation.version().as_str(),
            "generation": generation.generation_id(),
            "executable": generation.executable_path(),
        }),
        Err(error) => serde_json::json!({ "status": "invalid", "reason": error.cli_reason() }),
    };
    value["engine_id"] = engine.id().into();
    value["name"] = engine.display_name().into();
    value["selection"] = selection.into();
    value["floor"] = engine.floor().into();
    if let Ok(plan) = authority.plan() {
        value["integrity"] = plan.integrity.code().into();
        let trusted = paths
            .as_ref()
            .zip(value["version"].as_str())
            .and_then(|(paths, version)| {
                let records = read_trust_records(paths.engine_root(), engine).ok()?;
                record_for(&records, version, authority.platform())
                    .map(|record| record.first_seen_at_ms)
            });
        if let Some(first_seen) = trusted {
            value["trusted_since"] =
                artisan_domain::iso_millis(i64::try_from(first_seen).unwrap_or(i64::MAX)).into();
        }
    }
    if let Some(state) = state {
        value["pending"] = state.pending.map(|pending| pending.version).into();
        value["previous"] = state
            .previous
            .iter()
            .map(|generation| generation.version.clone())
            .collect::<Vec<_>>()
            .into();
    }
    if overridden {
        value["override"] = engine.override_env().into();
    }
    value
}

fn list(database: &Path, json: bool) {
    let engines = ManagedEngine::ALL
        .into_iter()
        .map(|engine| describe(database, engine))
        .collect::<Vec<_>>();
    if json {
        println!(
            "{}",
            serde_json::json!({ "schema": "artisan-engine-list-v2", "engines": engines })
        );
        return;
    }
    for engine in &engines {
        println!("{}", summary_line(engine));
    }
}

fn summary_line(engine: &serde_json::Value) -> String {
    let name = engine["name"].as_str().unwrap_or_default();
    let selection = engine["selection"].as_str().unwrap_or("latest");
    let held = if selection == "latest" {
        String::from("follows latest")
    } else {
        format!("held at {selection}")
    };
    let mut line = match engine["status"].as_str() {
        Some("ready") => format!(
            "{name}: ready {} ({held})",
            engine["version"].as_str().unwrap_or_default()
        ),
        Some("not_installed") => format!("{name}: not installed ({held})"),
        Some("unsupported") => format!(
            "{name}: unsupported ({})",
            engine["message"].as_str().unwrap_or_default()
        ),
        _ => format!(
            "{name}: invalid ({})",
            engine["reason"].as_str().unwrap_or_default()
        ),
    };
    match engine["integrity"].as_str() {
        Some("vendor_checksum") => line.push_str("; verified by vendor checksum"),
        Some("trust_on_first_download") => {
            let since = engine["trusted_since"]
                .as_str()
                .and_then(|since| since.get(..10));
            match since {
                Some(date) => {
                    let _ = write!(line, "; trusted on first download (hash recorded {date})");
                }
                None => line.push_str("; trusted on first download"),
            }
        }
        _ => {}
    }
    if let Some(pending) = engine["pending"].as_str() {
        let _ = write!(line, "; {pending} waits for the engine to be idle");
    }
    if let Some(variable) = engine["override"].as_str() {
        let _ = write!(line, "; WARNING: {variable} overrides the managed binary");
    }
    line
}

fn status(database: &Path, engine: ManagedEngine, json: bool) {
    let value = describe(database, engine);
    if json {
        println!("{value}");
    } else {
        println!("{}", summary_line(&value));
        if let Some(previous) = value["previous"].as_array().filter(|list| !list.is_empty()) {
            let versions = previous
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>();
            println!("  rollback targets: {}", versions.join(", "));
        }
    }
}

fn versions(database: &Path, engine: ManagedEngine, json: bool) -> Result<()> {
    let transport = transport(engine)?;
    let operations =
        EngineOperations::new(ManagedEngineAuthority::new(engine), database, &transport);
    let listing = operations.list_versions().map_err(install_error(engine))?;
    if json {
        let entries = listing.iter().map(listing_json).collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::json!({ "engine_id": engine.id(), "versions": entries })
        );
        return Ok(());
    }
    for entry in &listing {
        let mut marks = Vec::new();
        if entry.active {
            marks.push("current");
        } else if entry.installed {
            marks.push("installed");
        }
        if entry.below_floor {
            marks.push("below floor");
        }
        if marks.is_empty() {
            println!("{}", entry.version);
        } else {
            println!("{} ({})", entry.version, marks.join(", "));
        }
    }
    Ok(())
}

fn listing_json(entry: &VersionListing) -> serde_json::Value {
    serde_json::json!({
        "version": entry.version.as_str(),
        "installed": entry.installed,
        "active": entry.active,
        "below_floor": entry.below_floor,
    })
}

fn ensure(database: &Path, engine: Option<EngineArg>) -> Result<()> {
    let engines = engine.map_or_else(
        || {
            ManagedEngine::ALL
                .into_iter()
                .filter(|engine| ManagedEngineAuthority::new(*engine).plan().is_ok())
                .collect::<Vec<_>>()
        },
        |engine| vec![engine.engine()],
    );
    let mut first_error = None;
    for engine in engines {
        let transport = transport(engine)?;
        let operations =
            EngineOperations::new(ManagedEngineAuthority::new(engine), database, &transport);
        match operations.ensure_selected(&report_progress(engine)) {
            Ok(outcome) => print_outcome(engine, &outcome),
            Err(error) => {
                eprintln!("{}: {}", engine.display_name(), error.code());
                first_error.get_or_insert(install_error(engine)(error));
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn select(database: &Path, engine: ManagedEngine, selection: &str) -> Result<()> {
    let selection = EngineSelection::parse(selection).ok_or(CliError::EngineInstall {
        engine: engine.display_name(),
        reason: "selection_invalid",
    })?;
    let transport = transport(engine)?;
    let operations =
        EngineOperations::new(ManagedEngineAuthority::new(engine), database, &transport);
    let outcome = operations
        .select(&selection, &report_progress(engine))
        .map_err(install_error(engine))?;
    print_outcome(engine, &outcome);
    Ok(())
}

fn rollback(database: &Path, engine: ManagedEngine) -> Result<()> {
    let transport = transport(engine)?;
    let operations =
        EngineOperations::new(ManagedEngineAuthority::new(engine), database, &transport);
    let outcome = operations.rollback().map_err(install_error(engine))?;
    print_outcome(engine, &outcome);
    Ok(())
}

fn report_progress(engine: ManagedEngine) -> impl Fn(InstallProgress) {
    move |progress| match progress {
        InstallProgress::Downloading {
            received_bytes,
            total_bytes: Some(total),
        } if total > 0 => eprintln!(
            "{}: downloading {}%",
            engine.display_name(),
            received_bytes.saturating_mul(100) / total
        ),
        InstallProgress::Downloading { received_bytes, .. } => eprintln!(
            "{}: downloading {} MiB",
            engine.display_name(),
            received_bytes / (1024 * 1024)
        ),
        InstallProgress::Resolving
        | InstallProgress::Extracting
        | InstallProgress::Verifying
        | InstallProgress::Activating => {
            eprintln!("{}: {}", engine.display_name(), phase_name(progress));
        }
    }
}

const fn phase_name(progress: InstallProgress) -> &'static str {
    match progress {
        InstallProgress::Resolving => "resolving the selected version",
        InstallProgress::Downloading { .. } => "downloading",
        InstallProgress::Extracting => "extracting",
        InstallProgress::Verifying => "verifying",
        InstallProgress::Activating => "activating",
    }
}

fn print_outcome(engine: ManagedEngine, outcome: &SwitchOutcome) {
    let name = engine.display_name();
    match outcome {
        SwitchOutcome::AlreadyActive(version) => println!("{name} {version} is current"),
        SwitchOutcome::Activated(version) => println!("{name} {version} is now active"),
        SwitchOutcome::Pending(version) => {
            println!("{name} {version} is installed and activates when the engine is idle");
        }
    }
}

/// Runs the engine's own sign-in flow with the managed executable and the
/// exact environment Forge spawns use, so credentials land in the Forge
/// engine home rather than the operator's personal configuration.
fn login(database: &Path, engine: ManagedEngine, args: &[String]) -> Result<()> {
    let failure = |reason| CliError::EngineAuthority {
        engine: engine.display_name(),
        reason,
    };
    let target = resolve_launch_target_in(engine, database, &|name| std::env::var_os(name))
        .map_err(|error| failure(error.cli_reason()))?;
    let environment = target
        .environment()
        .map_err(|error| failure(error.cli_reason()))?;
    let arguments = if args.is_empty() {
        default_login_arguments(engine)
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect()
    } else {
        args.to_vec()
    };
    eprintln!(
        "{}: signing in under {}",
        engine.display_name(),
        target.home().display()
    );
    let status = std::process::Command::new(target.executable())
        .args(&arguments)
        .env_clear()
        .envs(environment)
        .status()
        .map_err(|_| failure("login_spawn_failed"))?;
    if status.success() {
        Ok(())
    } else {
        Err(failure("login_failed"))
    }
}

const fn default_login_arguments(engine: ManagedEngine) -> &'static [&'static str] {
    match engine {
        ManagedEngine::Claude | ManagedEngine::OpenCode2 => &["auth", "login"],
        ManagedEngine::Codex | ManagedEngine::Cursor | ManagedEngine::Grok => &["login"],
    }
}

#[cfg(test)]
mod tests {
    use super::summary_line;

    #[test]
    fn status_lines_name_the_trust_mode() {
        let vendor = serde_json::json!({
            "name": "Codex", "status": "ready", "version": "0.157.1",
            "selection": "latest", "integrity": "vendor_checksum",
        });
        assert_eq!(
            summary_line(&vendor),
            "Codex: ready 0.157.1 (follows latest); verified by vendor checksum"
        );
        let trusted = serde_json::json!({
            "name": "Grok Build", "status": "ready", "version": "1.0.41",
            "selection": "1.0.41", "integrity": "trust_on_first_download",
            "trusted_since": "2026-09-26T09:30:00.000Z",
        });
        assert_eq!(
            summary_line(&trusted),
            "Grok Build: ready 1.0.41 (held at 1.0.41); trusted on first download (hash recorded 2026-09-26)"
        );
    }
}
