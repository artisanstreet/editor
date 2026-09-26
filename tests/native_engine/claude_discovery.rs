//! Integration coverage for Claude executable selection.
//!
//! Exercises the public  discovery API:
//! override precedence, blank handling, byte bounds, verbatim paths with
//! spaces, the managed-only fallback (never ), and the live
//! missing-binary spawn failure.

use std::ffi::OsStr;
use std::time::Duration;

use artisan_native_engine::claude::{
    CLAUDE_EXECUTABLE_ENV_VAR, ClaudeDiscoveryError, ClaudeExecutableSource, ClaudeProbeOptions,
    ClaudeProbeRunner, discover_claude_executable, select_override_value,
};

#[test]
fn explicit_override_wins_and_blanks_fall_through() {
    assert_eq!(
        select_override_value(Some("  managed  "), Some("env")).unwrap(),
        Some("managed".to_owned())
    );
    assert_eq!(
        select_override_value(Some("   "), Some("env")).unwrap(),
        Some("env".to_owned())
    );
    assert_eq!(select_override_value(Some(""), Some("  ")).unwrap(), None);
    assert_eq!(select_override_value(None, None).unwrap(), None);
}

#[test]
fn overlong_override_is_rejected_without_a_path() {
    let long = "x".repeat(1025);
    let error = select_override_value(Some(&long), None).unwrap_err();
    assert_eq!(error, ClaudeDiscoveryError::ValueTooLong);
    assert_eq!(error.cli_reason(), "executable_override_too_long");
    assert_eq!(
        error.to_string(),
        "Claude executable override exceeds its bound"
    );
}

#[test]
fn without_a_managed_install_claude_is_not_resolved_from_path() {
    if std::env::var_os(CLAUDE_EXECUTABLE_ENV_VAR).is_none() {
        assert_eq!(
            discover_claude_executable(None),
            Err(ClaudeDiscoveryError::NotInstalled)
        );
    }
}

#[test]
fn discovery_preserves_a_spaced_override_as_one_command() {
    let spaced = "C:\\Program Files\\Claude\\claude.exe";
    let discovered = discover_claude_executable(Some(spaced)).unwrap();
    assert_eq!(
        discovered.source(),
        ClaudeExecutableSource::ExplicitOverride
    );
    assert_eq!(discovered.command(), OsStr::new(spaced));
    assert_eq!(discovered.into_command(), OsStr::new(spaced));
}

#[test]
fn missing_binary_fails_the_probe_as_a_spawn_error() {
    let executable =
        discover_claude_executable(Some("artisan-missing-claude-probe-binary")).unwrap();
    let runner = ClaudeProbeRunner::new(
        executable,
        ClaudeProbeOptions::default()
            .with_version_timeout(Duration::from_secs(10))
            .with_auth_timeout(Duration::from_secs(10)),
    )
    .unwrap();
    let error = runner.run().unwrap_err();
    assert_eq!(error.cli_reason(), "spawn_failed");
    assert!(error.to_string().contains("could not start"));
}
