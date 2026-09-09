//! Grok readiness-probe tests: auth-state classification, `XAI_API_KEY`
//! present/absent method selection, and timeout/output-bound behavior through
//! the bounded-spawn seam with fixture processes. No live `grok` CLI needed;
//! no secrets leave these fixtures.

use artisan_native_engine::grok::probe::{
    BoundedChildOutput, GrokAuthMethod, GrokAuthState, GrokProbe, GrokProbeError, GrokProbeLimits,
    GrokProbePhase, MAX_PROBE_EXCERPT_CHARS, XAI_API_KEY_ENV, classify_auth_result,
    classify_auth_spawn, classify_version_output, is_authenticated_output, preferred_auth_method,
    redact_probe_excerpt, run_bounded_command, select_auth_method,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn ok_output(exit_code: i32, combined_output: &str) -> BoundedChildOutput {
    BoundedChildOutput {
        exit_code,
        combined_output: combined_output.to_owned(),
    }
}

#[test]
fn api_key_env_name_is_documented() {
    assert_eq!(XAI_API_KEY_ENV, "XAI_API_KEY");
}

#[test]
fn authenticated_output_rejects_not_signed_in_semantics() {
    assert!(is_authenticated_output(
        "grok-build/1.2.3\nmodel-a\nmodel-b\n"
    ));
    assert!(is_authenticated_output(""));
    assert!(!is_authenticated_output(
        "Error: not authenticated; run grok login"
    ));
    assert!(!is_authenticated_output("NOT AUTHENTICATED"));
    assert!(!is_authenticated_output("You are not logged in."));
    assert!(!is_authenticated_output("NOT LOGGED IN - please sign in"));
}

#[test]
fn auth_method_selection_mirrors_typescript_auth_method() {
    let both = ["xai.api_key", "cached_token"];
    assert_eq!(
        select_auth_method(&both, true),
        Some(GrokAuthMethod::XaiApiKey)
    );
    assert_eq!(
        select_auth_method(&both, false),
        Some(GrokAuthMethod::CachedToken)
    );
    assert_eq!(
        select_auth_method(&["cached_token"], true),
        Some(GrokAuthMethod::CachedToken)
    );
    assert_eq!(select_auth_method(&["xai.api_key"], false), None);
    assert_eq!(select_auth_method(&[], true), None);
    assert_eq!(select_auth_method(&[], false), None);
}

#[test]
fn auth_method_ids_match_typescript_strings() {
    assert_eq!(GrokAuthMethod::XaiApiKey.as_str(), "xai.api_key");
    assert_eq!(GrokAuthMethod::CachedToken.as_str(), "cached_token");
    assert_eq!(preferred_auth_method(true), GrokAuthMethod::XaiApiKey);
    assert_eq!(preferred_auth_method(false), GrokAuthMethod::CachedToken);
}

#[test]
fn version_classification_accepts_clean_reports() {
    assert_eq!(
        classify_version_output(0, "grok 1.2.3").as_deref(),
        Ok("1.2.3")
    );
}

#[test]
fn version_classification_rejects_bad_exits_and_malformed_output() {
    let nonzero = classify_version_output(1, "grok 1.2.3");
    assert!(matches!(nonzero, Err(GrokProbeError::InvalidBinary { .. })));
    let malformed = classify_version_output(0, "no version here");
    assert!(matches!(
        malformed,
        Err(GrokProbeError::InvalidBinary { .. })
    ));
    let empty = classify_version_output(0, "");
    assert!(matches!(empty, Err(GrokProbeError::InvalidBinary { .. })));
    // Reasons carry redacted excerpts, never secrets or unbounded text.
    if let Err(GrokProbeError::InvalidBinary { reason }) = malformed {
        assert!(reason.contains("no version here"));
        assert!(reason.len() <= 256);
    } else {
        panic!("expected InvalidBinary");
    }
}

#[test]
fn auth_result_keeps_absent_distinct_from_unavailable() {
    assert_eq!(
        classify_auth_result(0, "model-a\nmodel-b\n"),
        GrokAuthState::Authenticated
    );
    // Answered "not signed in" on any exit: absent, never unavailable.
    assert_eq!(
        classify_auth_result(0, "not logged in"),
        GrokAuthState::Absent
    );
    assert_eq!(
        classify_auth_result(1, "Error: not authenticated"),
        GrokAuthState::Absent
    );
    // Ran but unusable with no auth semantics: unavailable.
    assert_eq!(classify_auth_result(1, "boom"), GrokAuthState::Unavailable);
}

#[test]
fn auth_spawn_mapping_keeps_timeout_and_malformed_distinct() {
    assert_eq!(
        classify_auth_spawn(&Ok(ok_output(0, "model-a"))),
        GrokAuthState::Authenticated
    );
    assert_eq!(
        classify_auth_spawn(&Ok(ok_output(0, "not authenticated"))),
        GrokAuthState::Absent
    );
    assert_eq!(
        classify_auth_spawn(&Err(GrokProbeError::Timeout {
            phase: GrokProbePhase::Auth
        })),
        GrokAuthState::Timeout
    );
    assert_eq!(
        classify_auth_spawn(&Err(GrokProbeError::OutputTooLarge {
            phase: GrokProbePhase::Auth
        })),
        GrokAuthState::Malformed
    );
    assert_eq!(
        classify_auth_spawn(&Err(GrokProbeError::Unavailable {
            reason: "gone".to_owned()
        })),
        GrokAuthState::Unavailable
    );
    assert_eq!(
        classify_auth_spawn(&Err(GrokProbeError::NotInstalled)),
        GrokAuthState::Unavailable
    );
    assert_eq!(
        classify_auth_spawn(&Err(GrokProbeError::InvalidBinary {
            reason: "bad".to_owned()
        })),
        GrokAuthState::Unavailable
    );
}

#[test]
fn probe_shape_records_method_without_executing_auth() {
    let probe = GrokProbe {
        executable: PathBuf::from("grok"),
        version: "1.2.3".to_owned(),
        auth: GrokAuthState::Absent,
        preferred_auth_method: preferred_auth_method(false),
    };
    assert!(!probe.ready());
    assert_eq!(probe.preferred_auth_method, GrokAuthMethod::CachedToken);
    let ready = GrokProbe {
        auth: GrokAuthState::Authenticated,
        preferred_auth_method: preferred_auth_method(true),
        ..probe
    };
    assert!(ready.ready());
    assert_eq!(ready.preferred_auth_method, GrokAuthMethod::XaiApiKey);
}

#[test]
fn redact_excerpt_caps_length_and_passes_short_text_through() {
    assert_eq!(redact_probe_excerpt("short"), "short");
    assert_eq!(redact_probe_excerpt(""), "");
    let long = "x".repeat(MAX_PROBE_EXCERPT_CHARS + 100);
    assert_eq!(redact_probe_excerpt(&long).len(), MAX_PROBE_EXCERPT_CHARS);
}

/// Fixture shell: echoes a marker and exits 0 on every host.
#[cfg(windows)]
fn echo_fixture(marker: &str) -> (PathBuf, [String; 2]) {
    (
        PathBuf::from("cmd"),
        ["/C".to_owned(), format!("echo {marker}")],
    )
}

/// Fixture shell: echoes a marker and exits 0 on every host.
#[cfg(not(windows))]
fn echo_fixture(marker: &str) -> (PathBuf, [String; 2]) {
    (
        PathBuf::from("sh"),
        ["-c".to_owned(), format!("echo {marker}")],
    )
}

/// Fixture sleeper (~2 s) used to prove the deadline fires.
#[cfg(windows)]
fn sleep_fixture() -> (PathBuf, [String; 2]) {
    (
        PathBuf::from("cmd"),
        ["/C".to_owned(), "ping -n 3 127.0.0.1 >NUL".to_owned()],
    )
}

/// Fixture sleeper (~2 s) used to prove the deadline fires.
#[cfg(not(windows))]
fn sleep_fixture() -> (PathBuf, [String; 2]) {
    (PathBuf::from("sh"), ["-c".to_owned(), "sleep 2".to_owned()])
}

fn run_fixture(
    program: &Path,
    args: &[String],
    timeout_ms: u64,
    max_bytes: usize,
) -> Result<BoundedChildOutput, GrokProbeError> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_bounded_command(
        program,
        &arg_refs,
        Duration::from_millis(timeout_ms),
        max_bytes,
        GrokProbePhase::Auth,
    )
}

#[test]
fn bounded_spawn_collects_fixture_process_output() {
    let (program, args) = echo_fixture("hello-grok-probe");
    let output =
        run_fixture(&program, &args, 10_000, 64 * 1024).expect("fixture echo must complete");
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.combined_output, "hello-grok-probe");
}

#[test]
fn bounded_spawn_enforces_deadline_and_reaps_child() {
    let (program, args) = sleep_fixture();
    let result = run_fixture(&program, &args, 150, 64 * 1024);
    assert_eq!(
        result,
        Err(GrokProbeError::Timeout {
            phase: GrokProbePhase::Auth
        })
    );
}

#[test]
fn bounded_spawn_enforces_output_bound() {
    let (program, args) = echo_fixture(&"x".repeat(3000));
    let result = run_fixture(&program, &args, 10_000, 64);
    assert_eq!(
        result,
        Err(GrokProbeError::OutputTooLarge {
            phase: GrokProbePhase::Auth
        })
    );
}

#[test]
fn bounded_spawn_reports_absent_binary_as_not_installed() {
    let missing = Path::new("definitely-not-a-grok-binary-xyz");
    let result = run_bounded_command(
        missing,
        &[],
        Duration::from_secs(5),
        1024,
        GrokProbePhase::Version,
    );
    assert_eq!(result, Err(GrokProbeError::NotInstalled));
}

#[test]
fn probe_limits_default_to_bounded_values() {
    let limits = GrokProbeLimits::default();
    assert_eq!(limits.max_output_bytes, 1_048_576);
    assert!(limits.version_timeout <= Duration::from_secs(60));
    assert!(limits.auth_timeout <= Duration::from_secs(60));
}
