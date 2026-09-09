//! Argument parsing for the native dev launcher.
//!
//! The launcher accepts exactly three flags; anything else fails closed
//! with usage text so a typo never stages half a home.

use std::{
    ffi::OsString,
    sync::atomic::{AtomicU64, Ordering},
};

use native_dev::{Action, DevArgs, usage};

fn argv(flags: &[&str]) -> Vec<OsString> {
    flags.iter().map(OsString::from).collect()
}

#[test]
fn empty_argv_runs_with_defaults() {
    let action = DevArgs::parse(&argv(&[])).expect("empty argv parses");
    assert_eq!(
        action,
        Action::Run(DevArgs {
            dev_dir: None,
            bin_dir: None,
            stage_only: false,
        })
    );
}

#[test]
fn stage_only_is_accepted() {
    let action = DevArgs::parse(&argv(&["--stage-only"])).expect("stage-only parses");
    assert!(matches!(
        action,
        Action::Run(DevArgs {
            stage_only: true,
            ..
        })
    ));
}

#[test]
fn explicit_directories_are_kept_verbatim() {
    let action = DevArgs::parse(&argv(&[
        "--dev-dir",
        "/tmp/artisan-dev",
        "--bin-dir",
        "/tmp/artisan-bins",
    ]))
    .expect("explicit directories parse");
    let Action::Run(args) = action else {
        panic!("expected a run action");
    };
    assert_eq!(
        args.dev_dir,
        Some(std::path::PathBuf::from("/tmp/artisan-dev"))
    );
    assert_eq!(
        args.bin_dir,
        Some(std::path::PathBuf::from("/tmp/artisan-bins"))
    );
    assert!(!args.stage_only);
}

#[test]
fn help_flags_select_help() {
    for flag in ["--help", "-h"] {
        let action = DevArgs::parse(&argv(&[flag])).expect("help parses");
        assert_eq!(action, Action::Help, "flag {flag}");
    }
}

#[test]
fn unknown_flags_fail_closed() {
    let error = DevArgs::parse(&argv(&["--watch"])).expect_err("unknown flag is rejected");
    assert!(error.to_string().contains("--watch"), "unexpected: {error}");
}

#[test]
fn missing_flag_values_fail_closed() {
    for flag in ["--dev-dir", "--bin-dir"] {
        let error = DevArgs::parse(&argv(&[flag])).expect_err("missing value is rejected");
        assert!(error.to_string().contains(flag), "unexpected: {error}");
    }
}

#[test]
fn usage_names_every_flag() {
    let text = usage();
    for flag in ["--dev-dir", "--bin-dir", "--stage-only"] {
        assert!(text.contains(flag), "usage is missing {flag}");
    }
    assert!(text.contains(".dist/dev"), "usage names the default home");
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[test]
fn argument_parsing_has_no_process_side_effects() {
    let before = COUNTER.fetch_add(1, Ordering::Relaxed);
    let action = DevArgs::parse(&argv(&["--stage-only"])).expect("parses");
    assert!(matches!(action, Action::Run(_)));
    assert_eq!(COUNTER.load(Ordering::Relaxed), before + 1);
}
