use artisan_editor_cli::{
    CliError,
    commands::{Cli, Commands, EngineCommand},
};
use clap::Parser;

fn engine_command(arguments: &[&str]) -> EngineCommand {
    let mut full = vec!["ae", "engine"];
    full.extend_from_slice(arguments);
    match Cli::try_parse_from(full).unwrap().command {
        Some(Commands::Engine { command, .. }) => command,
        other => panic!("not an engine command: {other:?}"),
    }
}

#[test]
fn list_and_status_parse_with_an_explicit_json_flag() {
    assert!(matches!(
        engine_command(&["list"]),
        EngineCommand::List { json: false }
    ));
    assert!(matches!(
        engine_command(&["list", "--json"]),
        EngineCommand::List { json: true }
    ));
    assert!(matches!(
        engine_command(&["status", "claude", "--json"]),
        EngineCommand::Status { json: true, .. }
    ));
}

#[test]
fn install_and_update_accept_an_optional_engine_only() {
    assert!(matches!(
        engine_command(&["install"]),
        EngineCommand::Install { engine: None }
    ));
    assert!(matches!(
        engine_command(&["install", "codex"]),
        EngineCommand::Install { engine: Some(_) }
    ));
    assert!(matches!(
        engine_command(&["update", "claude"]),
        EngineCommand::Update { engine: Some(_) }
    ));
    for flag in [
        "--url",
        "--integrity",
        "--sha256",
        "--path",
        "--force",
        "--version",
    ] {
        assert!(
            Cli::try_parse_from(["ae", "engine", "install", "codex", flag, "x"]).is_err(),
            "unexpectedly accepted {flag}"
        );
    }
    assert!(Cli::try_parse_from(["ae", "engine", "install", "gemini"]).is_err());
}

#[test]
fn use_takes_an_engine_and_a_selection_and_rollback_takes_an_engine() {
    match engine_command(&["use", "claude", "2.1.282"]) {
        EngineCommand::Use { selection, .. } => assert_eq!(selection, "2.1.282"),
        other => panic!("{other:?}"),
    }
    match engine_command(&["use", "codex", "latest"]) {
        EngineCommand::Use { selection, .. } => assert_eq!(selection, "latest"),
        other => panic!("{other:?}"),
    }
    assert!(Cli::try_parse_from(["ae", "engine", "use", "claude"]).is_err());
    assert!(matches!(
        engine_command(&["rollback", "codex"]),
        EngineCommand::Rollback { .. }
    ));
    assert!(matches!(
        engine_command(&["versions", "opencode2", "--json"]),
        EngineCommand::Versions { json: true, .. }
    ));
}

#[test]
fn login_passes_trailing_arguments_to_the_engine() {
    match engine_command(&["login", "codex", "--", "--device-auth"]) {
        EngineCommand::Login { args, .. } => assert_eq!(args, ["--device-auth"]),
        other => panic!("{other:?}"),
    }
    match engine_command(&["login", "claude"]) {
        EngineCommand::Login { args, .. } => assert!(args.is_empty()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_explicit_forge_database_is_accepted_on_every_engine_command() {
    let cli = Cli::try_parse_from([
        "ae",
        "engine",
        "list",
        "--database",
        "/home/user/.local/state/artisan-forge/forge.db",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Engine {
            database: Some(_),
            command: EngineCommand::List { .. },
        })
    ));
}

#[test]
fn engine_errors_are_path_free_and_install_failures_exit_with_four() {
    let error = CliError::EngineInstall {
        engine: "Claude Code",
        reason: "integrity_mismatch",
    };
    assert_eq!(error.exit_code(), 4);
    assert_eq!(
        error.to_string(),
        "Claude Code installation failed (integrity_mismatch)"
    );
    assert!(!format!("{error:?}").contains("https://"));
}
