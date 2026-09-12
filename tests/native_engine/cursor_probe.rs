//! Cursor readiness probe fixtures: auth states, model resolution,
//! startup-failure classification, and real subprocess bounds.
//!
//! Registration (controller-owned, not part of this packet):
//! `tests/native_engine/BUILD.bazel` gains a `rust_test` target for
//! `cursor_probe.rs` depending on `//modules/native_engine:native_engine`,
//! plus a Cargo `[[test]] cursor_probe` entry in
//! `modules/native_engine/Cargo.toml`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use artisan_native_engine::cursor::{
    BoundedChildOutput, CURSOR_ARTISAN_CODE_UNAVAILABLE_MODEL, CURSOR_ENGINE_ID,
    CURSOR_IMAGE_INPUT, CursorAcpInputs, CursorAuthState, CursorModelInputs, CursorPermissionMode,
    CursorProbeError, CursorProbeLimits, CursorProbePhase, CursorSpeed, MAX_PROBE_EXCERPT_CHARS,
    classify_auth_result, classify_auth_spawn, classify_cursor_startup_failure,
    classify_version_output, cursor_acp_args, is_authenticated_output, probe_cursor_readiness,
    redact_probe_excerpt, resolve_cursor_model, run_bounded_command, select_auth_method,
};

fn model_inputs(
    model: Option<&str>,
    effort: Option<&str>,
    speed: Option<CursorSpeed>,
) -> CursorModelInputs {
    CursorModelInputs {
        model: model.map(str::to_owned),
        reasoning_effort: effort.map(str::to_owned),
        speed,
    }
}

#[test]
fn model_resolution_matrix_matches_typescript() {
    assert_eq!(
        resolve_cursor_model(None, Some("high"), Some(CursorSpeed::Fast)),
        None
    );
    // Bracket passthrough ignores effort and speed.
    assert_eq!(
        resolve_cursor_model(Some("auto[fast]"), Some("high"), Some(CursorSpeed::Fast)),
        Some("auto[fast]".to_owned())
    );
    // Effort suffix unless already present.
    assert_eq!(
        resolve_cursor_model(Some("composer-1"), Some("high"), None),
        Some("composer-1-high".to_owned())
    );
    assert_eq!(
        resolve_cursor_model(Some("composer-1-high"), Some("low"), None),
        Some("composer-1-high".to_owned())
    );
    assert_eq!(
        resolve_cursor_model(
            Some("composer-1-high-fast"),
            Some("low"),
            Some(CursorSpeed::Fast)
        ),
        Some("composer-1-high-fast".to_owned())
    );
    // `-fast` append rules.
    assert_eq!(
        resolve_cursor_model(Some("composer-1"), None, Some(CursorSpeed::Fast)),
        Some("composer-1-fast".to_owned())
    );
    assert_eq!(
        resolve_cursor_model(Some("composer-1-fast"), None, Some(CursorSpeed::Fast)),
        Some("composer-1-fast".to_owned())
    );
    assert_eq!(
        resolve_cursor_model(Some("composer-1"), Some("medium"), Some(CursorSpeed::Fast)),
        Some("composer-1-medium-fast".to_owned())
    );
    // No controls: identity. Empty effort behaves like none.
    assert_eq!(
        resolve_cursor_model(Some("composer-1"), None, None),
        Some("composer-1".to_owned())
    );
    assert_eq!(
        resolve_cursor_model(Some("composer-1"), Some("  "), Some(CursorSpeed::Normal)),
        Some("composer-1".to_owned())
    );
    // `xhigh` counts as an effort suffix, not a base needing one.
    assert_eq!(
        resolve_cursor_model(Some("composer-1-xhigh"), Some("low"), None),
        Some("composer-1-xhigh".to_owned())
    );
}

#[test]
fn acp_args_matrix_matches_typescript() {
    let read_only = CursorAcpInputs {
        model: model_inputs(Some("composer-1-high"), None, None),
        permission_mode: Some(CursorPermissionMode::Force),
        read_only: true,
    };
    assert_eq!(
        cursor_acp_args(&read_only),
        vec!["--model", "composer-1-high", "--mode", "ask", "acp"]
    );
    let force = CursorAcpInputs {
        model: model_inputs(Some("composer-1"), Some("max"), Some(CursorSpeed::Fast)),
        permission_mode: Some(CursorPermissionMode::Force),
        read_only: false,
    };
    assert_eq!(
        cursor_acp_args(&force),
        vec!["--model", "composer-1-max-fast", "--force", "acp"]
    );
    let bare = CursorAcpInputs {
        model: model_inputs(None, None, None),
        permission_mode: None,
        read_only: false,
    };
    assert_eq!(cursor_acp_args(&bare), vec!["acp"]);
}

#[test]
fn image_input_is_recorded_as_image_blocks() {
    assert_eq!(CURSOR_IMAGE_INPUT, "image");
}

#[test]
fn startup_failure_captures_model_with_provider_code() {
    let failure = classify_cursor_startup_failure(
        "acp failed: Cannot use this model: gemini-3. Valid models are composer-1, gemini-3.",
    )
    .unwrap();
    assert_eq!(failure.model(), "gemini-3");
    assert_eq!(failure.artisan_code(), "AE-PROVIDER-206");
    assert_eq!(
        failure.artisan_code(),
        CURSOR_ARTISAN_CODE_UNAVAILABLE_MODEL
    );
    assert_eq!(failure.engine_id(), "cursor");
    assert_eq!(failure.engine_id(), CURSOR_ENGINE_ID);
    assert_eq!(
        failure.message(),
        "Cursor does not make model gemini-3 available to this account."
    );

    let newline =
        classify_cursor_startup_failure("Cannot use this model:  composer-1-xhigh  \nretry later")
            .unwrap();
    assert_eq!(newline.model(), "composer-1-xhigh");

    let eos = classify_cursor_startup_failure("boom: CANNOT USE THIS MODEL: opus-4").unwrap();
    assert_eq!(eos.model(), "opus-4");
}

#[test]
fn startup_failure_absent_without_rejection_shape() {
    for stderr in [
        "",
        "all good",
        "Cannot use this model:",
        "Cannot use this model:   ",
        "Cannot use this model and that one",
    ] {
        assert_eq!(
            classify_cursor_startup_failure(stderr),
            None,
            "input: {stderr}"
        );
    }
    // A 161-character model name exceeds the `{1,160}` bound with no
    // `. Valid models` terminator, so no rejection is recorded.
    let long = format!("Cannot use this model: {}", "a".repeat(161));
    assert_eq!(classify_cursor_startup_failure(&long), None);
}

#[test]
fn auth_state_classification_keeps_states_distinct() {
    assert!(is_authenticated_output("status: ok"));
    assert!(!is_authenticated_output("Not authenticated: sign in first"));
    assert!(!is_authenticated_output("user is NOT LOGGED IN"));

    // Answered "not signed in" regardless of exit code.
    assert_eq!(
        classify_auth_result(0, "Error: not authenticated"),
        CursorAuthState::Absent
    );
    assert_eq!(
        classify_auth_result(3, "not logged in"),
        CursorAuthState::Absent
    );
    // Otherwise exit 0 is authenticated, other exits unavailable.
    assert_eq!(
        classify_auth_result(0, "cursor-agent status ok"),
        CursorAuthState::Authenticated
    );
    assert_eq!(
        classify_auth_result(2, "cursor-agent status ok"),
        CursorAuthState::Unavailable
    );

    assert_eq!(
        classify_auth_spawn(&Err(CursorProbeError::Timeout {
            phase: CursorProbePhase::Auth
        })),
        CursorAuthState::Timeout
    );
    assert_eq!(
        classify_auth_spawn(&Err(CursorProbeError::OutputTooLarge {
            phase: CursorProbePhase::Auth
        })),
        CursorAuthState::Malformed
    );
    assert_eq!(
        classify_auth_spawn(&Err(CursorProbeError::Unavailable {
            reason: "gone".to_owned()
        })),
        CursorAuthState::Unavailable
    );
}

#[test]
fn auth_method_selection_mirrors_typescript() {
    assert_eq!(
        select_auth_method(&["cursor_login"])
            .map(artisan_native_engine::cursor::CursorAuthMethod::as_str),
        Some("cursor_login")
    );
    assert_eq!(select_auth_method(&[]), None);
    assert_eq!(select_auth_method(&["other"]), None);
}

#[test]
fn version_output_classification_redacts_with_bounds() {
    assert_eq!(
        classify_version_output(0, "cursor-agent 2025.09.06-abc123").unwrap(),
        "2025.09.06-abc123"
    );
    let invalid = classify_version_output(3, "cursor-agent 2025.09.06-abc123").unwrap_err();
    assert!(matches!(invalid, CursorProbeError::InvalidBinary { .. }));
    let malformed = classify_version_output(0, "no version").unwrap_err();
    assert!(matches!(malformed, CursorProbeError::InvalidBinary { .. }));

    let long = "y".repeat(MAX_PROBE_EXCERPT_CHARS + 64);
    assert_eq!(
        redact_probe_excerpt(&long).chars().count(),
        MAX_PROBE_EXCERPT_CHARS
    );
    assert_eq!(redact_probe_excerpt("  padded  "), "padded");
}

enum FixtureKind {
    Normal,
    StdoutFlood,
    StderrFlood,
    Slow,
    NonZero,
}

#[cfg(unix)]
fn fixture_command(kind: &FixtureKind) -> (PathBuf, Vec<String>) {
    let script = match kind {
        FixtureKind::Normal => r#"printf 'cursor-agent 2025.09.06-fixture01\n'"#,
        FixtureKind::StdoutFlood => "cat /dev/zero | head -c 300000",
        FixtureKind::StderrFlood => "cat /dev/zero | head -c 300000 >&2",
        FixtureKind::Slow => "sleep 30",
        FixtureKind::NonZero => "exit 3",
    };
    (
        PathBuf::from("sh"),
        vec!["-c".to_owned(), script.to_owned()],
    )
}

#[cfg(windows)]
fn fixture_command(kind: &FixtureKind) -> (PathBuf, Vec<String>) {
    let shell = std::env::var_os("COMSPEC").map_or_else(|| PathBuf::from("cmd.exe"), PathBuf::from);
    let script = match kind {
        FixtureKind::Normal => "echo cursor-agent 2025.09.06-fixture01".to_owned(),
        FixtureKind::StdoutFlood => format!("for /L %i in (1,1,2000) do @echo {}", "x".repeat(100)),
        FixtureKind::StderrFlood => {
            format!("for /L %i in (1,1,2000) do @echo {} 1>&2", "x".repeat(100))
        }
        FixtureKind::Slow => "ping -n 30 127.0.0.1 >NUL & rem".to_owned(),
        FixtureKind::NonZero => "exit 3".to_owned(),
    };
    (shell, vec!["/C".to_owned(), script])
}

fn run_fixture(
    kind: &FixtureKind,
    timeout: Duration,
    max_bytes: usize,
    phase: CursorProbePhase,
) -> Result<BoundedChildOutput, CursorProbeError> {
    let (program, args) = fixture_command(kind);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_bounded_command(&program, &arg_refs, timeout, max_bytes, phase)
}

#[test]
fn bounded_spawn_reports_real_child_output() {
    let output = run_fixture(
        &FixtureKind::Normal,
        Duration::from_secs(15),
        1_048_576,
        CursorProbePhase::Version,
    )
    .unwrap();
    assert_eq!(output.exit_code, 0);
    assert_eq!(
        classify_version_output(output.exit_code, &output.combined_output).unwrap(),
        "2025.09.06-fixture01"
    );
}

#[test]
fn bounded_spawn_enforces_output_bounds_without_deadlock() {
    for kind in [FixtureKind::StdoutFlood, FixtureKind::StderrFlood] {
        let started = Instant::now();
        let outcome = run_fixture(
            &kind,
            Duration::from_secs(30),
            16 * 1024,
            CursorProbePhase::Auth,
        );
        assert_eq!(
            outcome,
            Err(CursorProbeError::OutputTooLarge {
                phase: CursorProbePhase::Auth
            })
        );
        assert!(started.elapsed() < Duration::from_secs(25));
    }
}

#[test]
fn bounded_spawn_kills_long_running_child_on_deadline() {
    let started = Instant::now();
    let outcome = run_fixture(
        &FixtureKind::Slow,
        Duration::from_millis(500),
        1_048_576,
        CursorProbePhase::Version,
    );
    assert_eq!(
        outcome,
        Err(CursorProbeError::Timeout {
            phase: CursorProbePhase::Version
        })
    );
    assert!(started.elapsed() < Duration::from_secs(15));
}

#[test]
fn bounded_spawn_maps_exit_and_spawn_failures() {
    let outcome = run_fixture(
        &FixtureKind::NonZero,
        Duration::from_secs(15),
        1_048_576,
        CursorProbePhase::Version,
    )
    .unwrap();
    assert_eq!(
        classify_version_output(outcome.exit_code, &outcome.combined_output).unwrap_err(),
        CursorProbeError::InvalidBinary {
            reason: "cursor --version exited 3: no version output".to_owned()
        }
    );

    let missing = std::env::temp_dir().join("artisan-native-engine-absent-cursor.exe");
    assert!(!missing.exists());
    assert_eq!(
        run_bounded_command(
            &missing,
            &[],
            Duration::from_secs(15),
            1_048_576,
            CursorProbePhase::Version
        ),
        Err(CursorProbeError::NotInstalled)
    );
}

#[test]
fn full_readiness_probe_is_honest_without_binary() {
    let missing = std::env::temp_dir().join("artisan-native-engine-absent-cursor-probe.exe");
    assert!(!missing.exists());
    assert_eq!(
        probe_cursor_readiness(&missing, &CursorProbeLimits::default()),
        Err(CursorProbeError::NotInstalled)
    );
}

#[cfg(unix)]
#[test]
fn full_readiness_probe_reports_authenticated_fixture() {
    use std::os::unix::fs::PermissionsExt;

    // A static fixture binary cannot answer Absent for one phase only, so
    // the success path runs here end to end while Absent stays covered by
    // `auth_state_classification_keeps_states_distinct`.
    let pid = std::process::id();
    let directory = std::env::temp_dir().join(format!("artisan-cursor-probe-{pid}"));
    std::fs::create_dir_all(&directory).unwrap();
    let script = directory.join("cursor-fixture");
    std::fs::write(
        &script,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"cursor-agent 2025.09.06-fixture01\"; else echo \"ok\"; fi\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let probe = probe_cursor_readiness(&script, &CursorProbeLimits::default()).unwrap();
    assert!(probe.ready());
    assert_eq!(probe.auth(), CursorAuthState::Authenticated);
    assert_eq!(probe.version(), "2025.09.06-fixture01");
    assert_eq!(probe.executable(), script.as_path());
    std::fs::remove_dir_all(&directory).ok();
}
