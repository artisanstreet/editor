use artisan_editor_cli::{
    CliError,
    commands::{Cli, Commands, EngineCommand},
};
use clap::Parser;

#[test]
fn engine_install_is_the_exact_argument_free_command() {
    let cli = Cli::try_parse_from(["ae", "engine", "install"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Engine {
            command: EngineCommand::Install,
        })
    ));
}

#[test]
fn engine_install_rejects_selection_path_and_network_flags() {
    for (flag, value) in [
        ("--url", Some("https://example.invalid")),
        ("--version", Some("1.2.3")),
        ("--integrity", Some("sha512-value")),
        ("--path", Some("C:\\engine.exe")),
        ("--profile", Some("default")),
        ("--model", Some("model")),
        ("--provider", Some("provider")),
        ("--variant", Some("variant")),
        ("--sha256", Some("digest")),
        ("--resume", None),
        ("--force", None),
        ("--json", None),
        ("--yes", None),
    ] {
        let mut arguments = vec!["ae", "engine", "install", flag];
        if let Some(value) = value {
            arguments.push(value);
        }
        assert!(
            Cli::try_parse_from(arguments).is_err(),
            "unexpectedly accepted {flag}"
        );
    }
    assert!(Cli::try_parse_from(["ae", "engine", "install", "unexpected"]).is_err());
}

#[test]
fn install_error_is_path_free_and_uses_exit_code_four() {
    let error = CliError::OpenCode2Install {
        reason: "instance_invalid",
    };
    assert_eq!(error.exit_code(), 4);
    assert_eq!(
        error.to_string(),
        "OpenCode2 installation failed (instance_invalid)"
    );
    assert!(!format!("{error:?}").contains("C:\\"));
    assert!(!format!("{error}").contains("registry.npmjs.org"));
}
