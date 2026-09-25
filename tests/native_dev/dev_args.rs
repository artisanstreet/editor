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
fn no_arguments_runs_the_dev_profile_on_the_default_root() {
    let options = execute(&[]);
    assert_eq!(options.command, Command::Run);
    assert_eq!(options.profile, None);
    assert_eq!(options.root, None);
    assert_eq!(options.bin_dir, None);
    assert_eq!(options.keep, DEFAULT_KEEP);
}

#[test]
fn every_command_is_recognized() {
    for (word, command) in [
        ("run", Command::Run),
        ("stage", Command::Stage),
        ("where", Command::Where),
        ("prune", Command::Prune),
    ] {
        assert_eq!(execute(&[word]).command, command, "{word}");
    }
}

#[test]
fn flags_set_root_profile_binaries_and_retention() {
    let options = execute(&[
        "stage",
        "--root",
        "/tmp/dev-root",
        "--profile",
        "performance",
        "--bin-dir",
        "/tmp/bins",
        "--keep",
        "5",
    ]);
    assert_eq!(options.command, Command::Stage);
    assert_eq!(options.root, Some(PathBuf::from("/tmp/dev-root")));
    assert_eq!(options.profile.as_deref(), Some("performance"));
    assert_eq!(options.bin_dir, Some(PathBuf::from("/tmp/bins")));
    assert_eq!(options.keep, 5);
    assert_eq!(execute(&["--release"]).profile.as_deref(), Some("release"));
}

#[test]
fn help_is_available_anywhere() {
    assert_eq!(parse(&["--help"]).expect("help"), Action::Help);
    assert_eq!(parse(&["run", "-h"]).expect("help"), Action::Help);
}

#[test]
fn unknown_or_incomplete_invocations_fail_closed() {
    for invalid in [
        &["deploy"][..],
        &["--stage-only"][..],
        &["--root"][..],
        &["--keep", "many"][..],
        &["--profile", "../escape"][..],
        &["--profile", ""][..],
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
        "--root",
        "--profile",
        "--keep",
    ] {
        assert!(usage.contains(word), "{word}");
    }
}
