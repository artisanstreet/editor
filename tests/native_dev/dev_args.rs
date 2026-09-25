//! Command-line parsing: commands, flags, and fail-closed usage errors.

use std::{ffi::OsString, path::PathBuf};

use native_dev::{Action, Command, DEFAULT_KEEP, DevArgs, DevError};

fn parse(arguments: &[&str]) -> Result<Action, DevError> {
    let argv: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    DevArgs::parse(&argv)
}

fn execute(arguments: &[&str]) -> DevArgs {
    match parse(arguments).expect("valid invocation") {
        Action::Execute(options) => options,
        Action::Help => panic!("unexpected help for {arguments:?}"),
    }
}

#[test]
fn running_needs_only_a_payload() {
    let options = execute(&["--payload", "/nix/store/abc-artisan-linux-debug"]);
    assert_eq!(options.command, Command::Run);
    assert_eq!(
        options.payload,
        Some(PathBuf::from("/nix/store/abc-artisan-linux-debug"))
    );
    assert_eq!(options.root, None);
    assert_eq!(options.keep, DEFAULT_KEEP);
    assert!(
        !options.attach,
        "runs return once the Editor confirms startup"
    );
}

#[test]
fn every_command_is_recognized() {
    for (word, command) in [
        ("run", Command::Run),
        ("stage", Command::Stage),
        ("where", Command::Where),
        ("prune", Command::Prune),
    ] {
        let options = execute(&[word, "--payload", "/payload"]);
        assert_eq!(options.command, command, "{word}");
    }
    assert_eq!(execute(&["where"]).command, Command::Where);
    assert_eq!(execute(&["prune"]).command, Command::Prune);
}

#[test]
fn flags_set_root_retention_and_attachment() {
    let options = execute(&[
        "stage",
        "--payload",
        "/payload",
        "--root",
        "/tmp/dev-root",
        "--keep",
        "5",
        "--attach",
    ]);
    assert_eq!(options.command, Command::Stage);
    assert_eq!(options.root, Some(PathBuf::from("/tmp/dev-root")));
    assert_eq!(options.keep, 5);
    assert!(options.attach);
}

#[test]
fn help_is_available_anywhere() {
    assert_eq!(parse(&["--help"]).expect("help"), Action::Help);
    assert_eq!(parse(&["run", "-h"]).expect("help"), Action::Help);
}

#[test]
fn unknown_incomplete_or_payloadless_invocations_fail_closed() {
    for invalid in [
        &["deploy"][..],
        &[][..],
        &["stage"][..],
        &["--profile", "production"][..],
        &["--bin-dir", "/bins"][..],
        &["--payload"][..],
        &["where", "--keep", "many"][..],
    ] {
        assert!(
            matches!(parse(invalid), Err(DevError::Usage { .. })),
            "{invalid:?} must be refused"
        );
    }
}

#[test]
fn usage_names_every_command() {
    let usage = native_dev::usage();
    for word in [
        "run",
        "stage",
        "where",
        "prune",
        "--payload",
        "--root",
        "--keep",
        "nix run .#dev",
    ] {
        assert!(usage.contains(word), "{word}");
    }
}
