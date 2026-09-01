use artisan_editor_cli::{
    CliError,
    commands::{Cli, Commands, EngineCommand, EngineProfileCommand, EngineProfileHomeArg},
};
use artisan_native_engine::NativeOpenCode2ProfileLaunchError;
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

#[test]
fn verify_parser_succeeds_and_requires_explicit_profile_id() {
    let verify =
        Cli::try_parse_from(["ae", "engine", "profile", "verify", "--profile-id", "work"]).unwrap();
    assert!(matches!(
        verify.command,
        Some(Commands::Engine {
            command: EngineCommand::Profile {
                command: EngineProfileCommand::Verify { profile_id, json: false },
            },
        }) if profile_id.as_str() == "work"
    ));

    let verify_json = Cli::try_parse_from([
        "ae",
        "engine",
        "profile",
        "verify",
        "--profile-id",
        "work",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        verify_json.command,
        Some(Commands::Engine {
            command: EngineCommand::Profile {
                command: EngineProfileCommand::Verify { profile_id, json: true },
            },
        }) if profile_id.as_str() == "work"
    ));

    assert!(Cli::try_parse_from(["ae", "engine", "profile", "verify"]).is_err());
    assert!(Cli::try_parse_from(["ae", "engine", "profile", "verify", "--json"]).is_err());
}

#[test]
fn verify_rejects_invalid_profile_ids_and_unknown_arguments() {
    for value in ["", ".", "..", "../secret", "a/b", "a:b", "é"] {
        assert!(
            Cli::try_parse_from(["ae", "engine", "profile", "verify", "--profile-id", value,])
                .is_err(),
            "unexpectedly accepted profile id {value:?}"
        );
    }
    for flag in [
        "--path",
        "--model",
        "--provider",
        "--variant",
        "--credential",
        "--executable",
        "--profile",
        "--home",
        "--default",
        "--profile-home",
    ] {
        assert!(
            Cli::try_parse_from([
                "ae",
                "engine",
                "profile",
                "verify",
                "--profile-id",
                "work",
                flag,
                "value",
            ])
            .is_err(),
            "unexpectedly accepted {flag}"
        );
    }
    assert!(
        Cli::try_parse_from([
            "ae",
            "engine",
            "profile",
            "verify",
            "--profile-id",
            "work",
            "--unknown",
        ])
        .is_err()
    );
}

#[test]
fn verify_launch_errors_map_to_pathless_profile_error_with_stable_exit_code() {
    // Mirrors engine_profiles::launch_cli_reason exhaustive mapping.
    fn reason_for(error: NativeOpenCode2ProfileLaunchError) -> &'static str {
        match error {
            NativeOpenCode2ProfileLaunchError::UnsupportedPlatform => "unsupported_platform",
            NativeOpenCode2ProfileLaunchError::ProfileRegistryTooLarge
            | NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed
            | NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedVersion
            | NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedEngine
            | NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsafe
            | NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable
            | NativeOpenCode2ProfileLaunchError::DuplicateProfile
            | NativeOpenCode2ProfileLaunchError::MultiplePrimaryProfiles => {
                "profile_registry_invalid"
            }
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

    for error in [
        NativeOpenCode2ProfileLaunchError::UnsupportedPlatform,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryTooLarge,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedVersion,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedEngine,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsafe,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable,
        NativeOpenCode2ProfileLaunchError::DuplicateProfile,
        NativeOpenCode2ProfileLaunchError::MultiplePrimaryProfiles,
        NativeOpenCode2ProfileLaunchError::ProfileNotFound,
        NativeOpenCode2ProfileLaunchError::ProfileHomeUnsafe,
        NativeOpenCode2ProfileLaunchError::ProfileHomeUnavailable,
        NativeOpenCode2ProfileLaunchError::LockUnavailable,
        NativeOpenCode2ProfileLaunchError::InstallStateMissing,
        NativeOpenCode2ProfileLaunchError::InstallStateInvalid,
        NativeOpenCode2ProfileLaunchError::GenerationUnsafe,
        NativeOpenCode2ProfileLaunchError::GenerationUntrusted,
        NativeOpenCode2ProfileLaunchError::ExecutableUnavailable,
        NativeOpenCode2ProfileLaunchError::ExecutableChanged,
        NativeOpenCode2ProfileLaunchError::ExecutableSizeMismatch,
        NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch,
        NativeOpenCode2ProfileLaunchError::ProfileChanged,
    ] {
        let reason = reason_for(error);
        let cli_error = CliError::OpenCode2Profile { reason };
        assert_eq!(cli_error.exit_code(), 4);
        let display = cli_error.to_string();
        assert_eq!(
            display,
            format!("OpenCode2 profile operation failed ({reason})")
        );
        assert!(!display.contains('/'));
        assert!(!display.contains('\\'));
        assert!(!display.contains("C:"));
        assert!(!format!("{cli_error:?}").contains("C:\\secret"));
        assert!(!format!("{error}").contains("C:\\secret"));
        assert!(!format!("{error:?}").contains("profiles.json"));
        assert!(!reason.contains('/'));
        assert!(!reason.contains('\\'));
    }
}

#[test]
fn launch_capability_display_and_debug_are_redacted() {
    // VerifiedOpenCode2ProfileLaunch intentionally hides all fields in Debug/Display.
    // We assert the CliError wrapping the reason also stays redacted, mirroring the
    // authority's path-free guarantee without constructing a live install-lock capability.
    let error = CliError::OpenCode2Profile {
        reason: "profile_not_found",
    };
    let debug = format!("{error:?}");
    assert!(debug.contains("profile_not_found"));
    assert!(!debug.contains("C:\\"));
    assert!(!debug.contains("/tmp"));
    assert!(!debug.contains("profiles.json"));
    assert!(!debug.contains("opencode2.exe"));

    for launch_error in [
        NativeOpenCode2ProfileLaunchError::ProfileNotFound,
        NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch,
    ] {
        let display = launch_error.to_string();
        assert!(!display.contains("C:\\"));
        assert!(!format!("{launch_error:?}").contains("C:\\"));
    }
}
