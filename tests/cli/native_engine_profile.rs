use artisan_editor_cli::{
    CliError,
    commands::{Cli, Commands, EngineCommand, EngineProfileCommand, EngineProfileHomeArg},
};
use clap::Parser;

#[test]
fn all_profile_commands_require_their_explicit_arguments() {
    let register = Cli::try_parse_from([
        "ae",
        "engine",
        "profile",
        "register",
        "--profile-id",
        "work",
        "--home",
        "named",
    ])
    .unwrap();
    assert!(matches!(
        register.command,
        Some(Commands::Engine {
            command: EngineCommand::Profile {
                command: EngineProfileCommand::Register {
                    profile_id,
                    home: EngineProfileHomeArg::Named,
                },
            },
        }) if profile_id.as_str() == "work"
    ));

    let list = Cli::try_parse_from(["ae", "engine", "profile", "list", "--json"]).unwrap();
    assert!(matches!(
        list.command,
        Some(Commands::Engine {
            command: EngineCommand::Profile {
                command: EngineProfileCommand::List { json: true },
            },
        })
    ));

    let read =
        Cli::try_parse_from(["ae", "engine", "profile", "read", "--profile-id", "work"]).unwrap();
    assert!(matches!(
        read.command,
        Some(Commands::Engine {
            command: EngineCommand::Profile {
                command: EngineProfileCommand::Read { profile_id, json: false },
            },
        }) if profile_id.as_str() == "work"
    ));

    for arguments in [
        vec!["ae", "engine", "profile", "register"],
        vec![
            "ae",
            "engine",
            "profile",
            "register",
            "--profile-id",
            "work",
        ],
        vec!["ae", "engine", "profile", "register", "--home", "named"],
        vec!["ae", "engine", "profile", "read"],
    ] {
        assert!(Cli::try_parse_from(arguments).is_err());
    }
}

#[test]
fn profile_ids_and_home_values_are_strict_and_no_selection_flags_exist() {
    for value in ["", ".", "..", "../secret", "a/b", "a:b", "é"] {
        assert!(
            Cli::try_parse_from(["ae", "engine", "profile", "read", "--profile-id", value,])
                .is_err(),
            "unexpectedly accepted profile id {value:?}"
        );
    }
    assert!(
        Cli::try_parse_from([
            "ae",
            "engine",
            "profile",
            "register",
            "--profile-id",
            "work",
            "--home",
            "other",
        ])
        .is_err()
    );

    for flag in [
        "--path",
        "--model",
        "--provider",
        "--variant",
        "--credential",
        "--executable",
        "--profile",
    ] {
        assert!(
            Cli::try_parse_from(["ae", "engine", "profile", "list", flag, "value"]).is_err(),
            "unexpectedly accepted {flag}"
        );
    }
}

#[test]
fn profile_cli_error_is_stable_pathless_and_configuration_scoped() {
    let error = CliError::OpenCode2Profile {
        reason: "profile_home_unsafe",
    };
    assert_eq!(error.exit_code(), 4);
    assert_eq!(
        error.to_string(),
        "OpenCode2 profile operation failed (profile_home_unsafe)"
    );
    assert!(!format!("{error:?}").contains("C:\\secret"));
}
