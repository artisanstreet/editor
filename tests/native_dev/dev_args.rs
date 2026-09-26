//! `nix run .#dev` arguments, and the internal `editor` group the Linux
//! runner hands the Windows runner.

use std::{ffi::OsString, path::PathBuf};

use native_dev::{
    Action, Command, DEFAULT_KEEP, DevArgs, DevError, EditorArgs, EditorPlatform, nix::Stage,
    parse, usage,
};

fn parse_words(arguments: &[&str]) -> Result<Action, DevError> {
    let argv: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    parse(&argv)
}

fn execute(arguments: &[&str]) -> DevArgs {
    match parse_words(arguments).expect("valid invocation") {
        Action::Execute(options) => options,
        other => panic!("expected an execution, got {other:?}"),
    }
}

fn editor(arguments: &[&str]) -> EditorArgs {
    match parse_words(arguments).expect("valid editor invocation") {
        Action::Editor(options) => options,
        other => panic!("expected the editor half, got {other:?}"),
    }
}

#[test]
fn a_bare_run_builds_debug_and_picks_the_platform() {
    let options = execute(&[]);
    assert_eq!(options.command, Command::Run);
    assert_eq!(options.stage, Stage::Debug);
    assert_eq!(options.editor, None);
    assert_eq!(options.root, None);
    assert_eq!(options.keep, DEFAULT_KEEP);
    assert!(!options.attach);
}

#[test]
fn every_command_and_option_is_recognized() {
    for (word, command) in [
        ("run", Command::Run),
        ("stage", Command::Stage),
        ("where", Command::Where),
        ("prune", Command::Prune),
    ] {
        assert_eq!(execute(&[word]).command, command);
    }
    let options = execute(&[
        "stage",
        "--production",
        "--linux",
        "--root",
        "/tmp/verify/Artisan Street Dev",
        "--windows-root",
        r"C:\Users\ada\AppData\Local\Artisan Street Verify",
        "--listen",
        "auto:4533",
        "--host-name",
        "Ubuntu verify",
        "--keep",
        "1",
        "--attach",
    ]);
    assert_eq!(options.command, Command::Stage);
    assert_eq!(options.stage, Stage::Production);
    assert_eq!(options.editor, Some(EditorPlatform::Linux));
    assert_eq!(
        options.root,
        Some(PathBuf::from("/tmp/verify/Artisan Street Dev"))
    );
    assert_eq!(
        options.windows_root,
        Some(OsString::from(
            r"C:\Users\ada\AppData\Local\Artisan Street Verify"
        ))
    );
    assert_eq!(options.listen.as_deref(), Some("auto:4533"));
    assert_eq!(options.host_name.as_deref(), Some("Ubuntu verify"));
    assert_eq!(options.keep, 1);
    assert!(options.attach);
    assert_eq!(
        execute(&["--windows"]).editor,
        Some(EditorPlatform::Windows)
    );
}

#[test]
fn the_editor_half_takes_its_payload_invitation_and_root() {
    let options = editor(&[
        "editor",
        "run",
        "--payload",
        r"\\wsl.localhost\Ubuntu\nix\store\x-artisan-windows-debug",
        "--invitation",
        r"\\wsl.localhost\Ubuntu\home\ada\.local\share\Artisan Street Dev\host.json",
        "--keep",
        "2",
        "--root",
        r"C:\Artisan Street Verify",
    ]);
    assert_eq!(options.command, Command::Run);
    assert!(options.payload.is_some() && options.invitation.is_some());
    assert_eq!(options.keep, 2);
    assert_eq!(
        options.root,
        Some(PathBuf::from(r"C:\Artisan Street Verify"))
    );
    assert_eq!(editor(&["editor", "where"]).command, Command::Where);
    assert_eq!(editor(&["editor", "prune"]).command, Command::Prune);
}

#[test]
fn help_is_available_anywhere() {
    assert_eq!(parse_words(&["--help"]).expect("help"), Action::Help);
    assert_eq!(parse_words(&["stage", "-h"]).expect("help"), Action::Help);
}

#[test]
fn unknown_or_incomplete_invocations_fail_closed() {
    for invalid in [
        &["build"][..],
        &["--root"],
        &["--keep", "many"],
        &["--payload", "/nix/store/x"],
        &["editor"],
        &["editor", "run"],
        &["editor", "stage", "--payload", "/x"],
        &["editor", "run", "--invitation", "/h.json", "--production"],
    ] {
        assert!(
            matches!(parse_words(invalid), Err(DevError::Usage { .. })),
            "{invalid:?}"
        );
    }
}

#[test]
fn usage_names_every_command_and_both_halves() {
    let text = usage();
    for word in [
        "run",
        "stage",
        "where",
        "prune",
        "--production",
        "--root",
        "--windows-root",
        "--listen",
        "artisan-forge-dev.service",
        "Artisan Street Dev",
    ] {
        assert!(text.contains(word), "usage mentions {word}");
    }
}
