use artisan_editor_cli::commands::{Cli, Commands, EngineCommand};
use clap::Parser;

#[test]
fn engine_list_parses_without_selection_arguments() {
    let cli = Cli::try_parse_from(["ae", "engine", "list"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Engine {
            command: EngineCommand::List { json: false },
        })
    ));
}

#[test]
fn engine_list_json_is_an_explicit_format_flag() {
    let cli = Cli::try_parse_from(["ae", "engine", "list", "--json"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Engine {
            command: EngineCommand::List { json: true },
        })
    ));
}

#[test]
fn engine_list_rejects_selection_and_execution_flags() {
    for extra in [
        ["--profile", "default"],
        ["--model", "model-id"],
        ["--provider", "provider-id"],
        ["--variant", "variant-id"],
        ["--path", "C:\\engine.exe"],
        [
            "--sha256",
            "452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf",
        ],
    ] {
        let mut arguments = vec!["ae", "engine", "list"];
        arguments.extend(extra);
        assert!(
            Cli::try_parse_from(arguments).is_err(),
            "unexpectedly accepted engine selection arguments"
        );
    }
}
