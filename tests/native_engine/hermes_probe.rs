//! Hermes probe: auth-`Unknown` reporting, inventory/profile data, and
//! bounded `--version` execution (`Absent` vs too-old vs timeout/malformed).
//!
//! Pure assembly and evaluation use fixture inputs only. Fixture *processes*
//! run through [`spawn_capture`] on Windows (`cmd.exe` one-liners) to prove
//! the timeout and output-bound paths kill and reap the child.

use std::path::PathBuf;
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

use artisan_native_engine::hermes::inventory::{
    AUTH_UNKNOWN_REASON, DEFAULT_PROFILE_ID, HERMES_ENGINE_ID, INSTALLED_PROFILE_AUTH_OWNER,
    MODEL_OPTIONS_METHOD, installed_profile_layout, live_inventory_request,
};
use artisan_native_engine::hermes::probe::{
    HermesAuthState, HermesProbeError, HermesProbeInput, ProbeLimits, assemble_probe,
    check_minimum_version, default_probe_limits, evaluate_version_output, spawn_capture,
};
use artisan_native_engine::hermes::resolve::{
    ResolvedHermesExecutable, resolve_hermes_executable_from_parts,
};
use artisan_native_engine::hermes::version::parse_hermes_version;

const FIXTURE_CANARY: &str = "HERMES-PROBE-CANARY-7f3a";

fn fixture_executable() -> ResolvedHermesExecutable {
    resolve_hermes_executable_from_parts(Some("C:\\tools\\hermes.exe"), None, None)
        .expect("fixture executable resolves")
}

fn fixture_input() -> HermesProbeInput {
    HermesProbeInput::new(DEFAULT_PROFILE_ID, PathBuf::from("C:\\work\\repo"))
}

#[test]
fn authentication_is_unknown_with_profile_owned_reason() {
    let version = parse_hermes_version("Hermes Agent v0.20.0").expect("parses");
    let probe = assemble_probe(&fixture_executable(), &fixture_input(), version);
    assert_eq!(
        probe.authentication(),
        HermesAuthState::Unknown {
            reason: AUTH_UNKNOWN_REASON
        }
    );
    assert_eq!(probe.authentication().state(), "unknown");
    assert_eq!(probe.authentication().reason(), AUTH_UNKNOWN_REASON);
    assert_eq!(AUTH_UNKNOWN_REASON, "owned-by-installed-profile");
    assert!(probe.ready());
    assert_eq!(probe.version().triple(), [0, 20, 0]);
    assert_eq!(
        probe.executable().path(),
        PathBuf::from("C:\\tools\\hermes.exe").as_path()
    );
}

#[test]
fn probe_never_synthesizes_authenticated_state() {
    // The enum has no authenticated spelling to construct; matching must be
    // exhaustive over `Unknown` only.
    let version = parse_hermes_version("Hermes Agent v1.4.0").expect("parses");
    let probe = assemble_probe(&fixture_executable(), &fixture_input(), version);
    match probe.authentication() {
        HermesAuthState::Unknown { reason } => assert_eq!(reason, AUTH_UNKNOWN_REASON),
    }
}

#[test]
fn inventory_request_records_model_options_shape() {
    let request = live_inventory_request();
    assert_eq!(request.method(), MODEL_OPTIONS_METHOD);
    assert_eq!(MODEL_OPTIONS_METHOD, "model.options");
    assert!(request.explicit_only());
    assert!(!request.include_unconfigured());
    assert!(!request.refresh());
    let version = parse_hermes_version("Hermes Agent v0.20.0").expect("parses");
    let probe = assemble_probe(&fixture_executable(), &fixture_input(), version);
    assert_eq!(probe.inventory_request(), request);
}

#[test]
fn installed_profile_layout_marks_auth_owner() {
    let layout = installed_profile_layout("default");
    assert_eq!(layout.engine_id(), HERMES_ENGINE_ID);
    assert_eq!(HERMES_ENGINE_ID, "hermes");
    assert_eq!(layout.profile_id(), "default");
    assert!(layout.default_profile());
    assert_eq!(layout.auth_owner(), INSTALLED_PROFILE_AUTH_OWNER);
    let named = installed_profile_layout("work");
    assert_eq!(named.profile_id(), "work");
    assert!(!named.default_profile());
    assert_eq!(named.auth_owner(), INSTALLED_PROFILE_AUTH_OWNER);
}

#[test]
fn default_limits_mirror_typescript_bounds() {
    let limits = default_probe_limits();
    assert_eq!(limits.maximum_bytes_per_stream(), 1024 * 1024);
    assert_eq!(limits.deadline(), Duration::from_secs(30));
}

#[test]
fn too_old_gate_reports_found_triple() {
    let version = parse_hermes_version("Hermes Agent v0.19.9").expect("parses");
    let error = check_minimum_version(&version).expect_err("too old");
    assert_eq!(error, HermesProbeError::VersionTooOld { found: [0, 19, 9] });
    let rendered = format!("{error}");
    assert!(rendered.contains("0.19.9"));
    assert!(rendered.contains("0.20.0"));
}

#[test]
fn error_display_redacts_child_output() {
    let error = HermesProbeError::VersionUnrecognized {
        stdout_bytes: 18,
        stderr_bytes: 4,
    };
    let rendered = format!("{error}");
    assert!(!rendered.contains(FIXTURE_CANARY));
    assert!(rendered.contains("18 stdout bytes"));
    let debug = format!("{error:?}");
    assert!(!debug.contains(FIXTURE_CANARY));
    let exit = HermesProbeError::NonZeroExit { exit_code: Some(1) };
    assert!(!format!("{exit}").contains(FIXTURE_CANARY));
}

/// Fixture process shell for spawn-path tests (Windows native gate only).
#[cfg(windows)]
fn fixture_shell() -> PathBuf {
    std::env::var_os("COMSPEC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cmd.exe"))
}

#[test]
#[cfg(windows)]
fn spawn_success_captures_version_output() {
    let shell = fixture_shell();
    let limits = ProbeLimits::new(1024 * 1024, Duration::from_secs(30));
    let output = spawn_capture(&shell, &["/C", "echo Hermes Agent v0.20.0"], &limits)
        .expect("fixture version echo runs");
    assert_eq!(output.exit_code(), Some(0));
    let version = evaluate_version_output(&output).expect("fixture output evaluates");
    assert_eq!(version.triple(), [0, 20, 0]);
}

#[test]
#[cfg(windows)]
fn spawn_missing_executable_is_distinct() {
    let limits = ProbeLimits::new(4096, Duration::from_secs(10));
    let error = spawn_capture(
        PathBuf::from("C:\\no-such-dir\\no-such-hermes.exe").as_path(),
        &["--version"],
        &limits,
    )
    .expect_err("missing executable fails");
    assert_eq!(error, HermesProbeError::SpawnFailed);
}

#[test]
#[cfg(windows)]
fn spawn_timeout_kills_and_reaps() {
    let shell = fixture_shell();
    // `ping -n 6` waits roughly five seconds; the probe must kill at 500 ms.
    let limits = ProbeLimits::new(64 * 1024, Duration::from_millis(500));
    let started = Instant::now();
    let error = spawn_capture(&shell, &["/C", "ping -n 6 127.0.0.1 >NUL"], &limits)
        .expect_err("slow fixture times out");
    let elapsed = started.elapsed();
    assert_eq!(error, HermesProbeError::ProbeTimeout);
    // Elapsed well under the fixture's natural five seconds proves the child
    // was killed and reaped instead of awaited.
    assert!(
        elapsed < Duration::from_secs(4),
        "elapsed {elapsed:?} proves kill plus reap"
    );
}

#[test]
#[cfg(windows)]
fn spawn_output_bound_kills_and_reaps() {
    let shell = fixture_shell();
    let limits = ProbeLimits::new(4096, Duration::from_secs(30));
    let error = spawn_capture(
        &shell,
        &[
            "/C",
            "for /L %i in (1,1,2000) do @echo xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        ],
        &limits,
    )
    .expect_err("oversized fixture breaches the bound");
    assert_eq!(error, HermesProbeError::OutputTooLarge);
}

#[test]
#[cfg(windows)]
fn malformed_fixture_output_redacts_canary() {
    let shell = fixture_shell();
    let limits = ProbeLimits::new(64 * 1024, Duration::from_secs(30));
    let line = format!("echo {FIXTURE_CANARY} not a version");
    let output = spawn_capture(&shell, &["/C", line.as_str()], &limits).expect("fixture echo runs");
    assert_eq!(output.exit_code(), Some(0));
    let error = evaluate_version_output(&output).expect_err("canary is not a version");
    assert_eq!(
        error,
        HermesProbeError::VersionUnrecognized {
            stdout_bytes: output.stdout_len(),
            stderr_bytes: output.stderr_len(),
        }
    );
    assert!(!format!("{error}").contains(FIXTURE_CANARY));
    assert!(!format!("{error:?}").contains(FIXTURE_CANARY));
    assert!(!format!("{output:?}").contains(FIXTURE_CANARY));
}

#[test]
#[cfg(windows)]
fn nonzero_exit_keeps_stderr_redacted() {
    let shell = fixture_shell();
    let limits = ProbeLimits::new(64 * 1024, Duration::from_secs(30));
    let line = format!("echo {FIXTURE_CANARY} >&2 & exit /b 3");
    let output =
        spawn_capture(&shell, &["/C", line.as_str()], &limits).expect("fixture failure runs");
    let error = evaluate_version_output(&output).expect_err("nonzero exit fails");
    assert_eq!(error, HermesProbeError::NonZeroExit { exit_code: Some(3) });
    assert!(!format!("{error}").contains(FIXTURE_CANARY));
}
