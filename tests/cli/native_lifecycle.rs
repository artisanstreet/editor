use std::num::NonZeroU32;

use artisan_editor_cli::{Cli, CliError, commands::Commands};
use clap::Parser;

#[test]
fn parser_accepts_only_the_pid_fenced_idle_stop_form() {
    let cli = Cli::try_parse_from(["ae", "stop", "--pid", "6172", "--if-idle"])
        .expect("validated idle stop form");
    assert!(matches!(
        cli.command,
        Some(Commands::Stop { pid, if_idle: true })
            if pid == NonZeroU32::new(6172).expect("test PID")
    ));

    for argv in [
        vec!["ae", "stop"],
        vec!["ae", "stop", "--pid", "6172"],
        vec!["ae", "stop", "--if-idle"],
        vec!["ae", "stop", "--pid", "0", "--if-idle"],
        vec!["ae", "stop", "--pid", "6172", "--force"],
        vec!["ae", "stop", "--instance-id", "forge-1"],
    ] {
        assert!(Cli::try_parse_from(argv).is_err());
    }
}

#[test]
fn status_parser_keeps_json_optional_and_restart_is_not_a_lifecycle_alias() {
    assert!(matches!(
        Cli::try_parse_from(["ae", "status"]).unwrap().command,
        Some(Commands::Status { json: false })
    ));
    assert!(matches!(
        Cli::try_parse_from(["ae", "status", "--json"])
            .unwrap()
            .command,
        Some(Commands::Status { json: true })
    ));
    assert!(matches!(
        Cli::try_parse_from(["ae", "restart"]).unwrap().command,
        Some(Commands::Restart { .. })
    ));
    assert_eq!(CliError::UnsupportedLifecycleControl.exit_code(), 1);
}

#[test]
fn lifecycle_errors_are_bounded_and_redacted() {
    let secret = "R3CONNECT-CAPABILITY-BYTES certificate-bytes nonce /private/home";
    let errors = [
        CliError::LifecycleCredentialState { reason: "record" },
        CliError::LifecycleCustody { reason: "custody" },
        CliError::LifecycleReadiness {
            reason: "readiness",
        },
        CliError::LifecycleService {
            reason: "transport",
        },
        CliError::LifecycleAmbiguous,
        CliError::LifecycleBusy,
    ];
    for error in errors {
        let rendered = error.to_string();
        assert!(!rendered.contains(secret));
    }
}
