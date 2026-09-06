//! Codex readiness probe fixtures: account decoding, real subprocess bounds.
//!
//! Registration (controller-owned, not part of this packet):
//! `tests/native_engine/BUILD.bazel` gains a `rust_test` target for
//! `codex_probe.rs` depending on `//modules/native_engine:native_engine`.
//!
//! CLI login-status text is never account evidence: only
//! `parse_codex_account_read` counts, and a bare status string stays
//! `AccountInvalid`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use artisan_native_engine::codex::{
    CODEX_ACCOUNT_OUTPUT_BOUND_BYTES, CODEX_VERSION_OUTPUT_BOUND_BYTES, CodexAuthState,
    CodexProbeError, classify_codex_auth, codex_readiness, parse_codex_account_read,
    run_codex_version, validate_codex_version_output,
};

#[test]
fn account_matrix_covers_all_wire_shapes() {
    let api_key =
        parse_codex_account_read(br#"{"account":{"type":"apiKey"},"requiresOpenaiAuth":false}"#)
            .unwrap();
    assert!(classify_codex_auth(api_key).is_authenticated());

    let chatgpt = parse_codex_account_read(
        br#"{"account":{"type":"chatgpt","email":"user@example.com","planType":"plus"},"requiresOpenaiAuth":false}"#,
    )
    .unwrap();
    assert!(classify_codex_auth(chatgpt).is_authenticated());

    let chatgpt_null_email = parse_codex_account_read(
        br#"{"account":{"type":"chatgpt","email":null,"planType":null},"requiresOpenaiAuth":false}"#,
    )
    .unwrap();
    assert!(classify_codex_auth(chatgpt_null_email).is_authenticated());

    let bedrock = parse_codex_account_read(
        br#"{"account":{"type":"amazonBedrock","credentialSource":"profile"},"requiresOpenaiAuth":false}"#,
    )
    .unwrap();
    assert!(classify_codex_auth(bedrock).is_authenticated());

    let absent_required =
        parse_codex_account_read(br#"{"account":null,"requiresOpenaiAuth":true}"#).unwrap();
    assert!(!classify_codex_auth(absent_required).is_authenticated());

    let absent_free =
        parse_codex_account_read(br#"{"account":null,"requiresOpenaiAuth":false}"#).unwrap();
    assert!(!classify_codex_auth(absent_free).is_authenticated());
}

#[test]
fn forward_compatible_fields_are_ignored_not_rejected() {
    let api_key_extra = parse_codex_account_read(
        br#"{"account":{"type":"apiKey","keyLastFour":"1234"},"requiresOpenaiAuth":false}"#,
    )
    .unwrap();
    assert!(classify_codex_auth(api_key_extra).is_authenticated());

    let top_level_extra = parse_codex_account_read(
        br#"{"account":null,"requiresOpenaiAuth":false,"planType":"team"}"#,
    )
    .unwrap();
    assert!(!classify_codex_auth(top_level_extra).is_authenticated());
}

#[test]
fn unauthenticated_reasons_follow_requires_openai_auth() {
    let required =
        parse_codex_account_read(br#"{"account":null,"requiresOpenaiAuth":true}"#).unwrap();
    assert_eq!(
        classify_codex_auth(required),
        CodexAuthState::Unauthenticated {
            reason: "OpenAI authentication required"
        }
    );
    let free = parse_codex_account_read(br#"{"account":null,"requiresOpenaiAuth":false}"#).unwrap();
    assert_eq!(
        classify_codex_auth(free),
        CodexAuthState::Unauthenticated {
            reason: "No ChatGPT or API-key account is active"
        }
    );
}

#[test]
fn login_status_text_is_not_account_evidence() {
    for bytes in [
        b"Logged in as user@example.com".as_slice(),
        b"Not logged in".as_slice(),
        b"apiKey".as_slice(),
    ] {
        assert_eq!(
            parse_codex_account_read(bytes),
            Err(CodexProbeError::AccountInvalid)
        );
    }
}

#[test]
fn malformed_account_documents_stay_invalid() {
    for bytes in [
        br#"{}"#.as_slice(),
        br#"{"account":null}"#,
        br#"{"requiresOpenaiAuth":false}"#,
        br#"{"account":null,"requiresOpenaiAuth":"yes"}"#,
        br#"{"account":"apiKey","requiresOpenaiAuth":false}"#,
        br#"{"account":{"type":"oauth"},"requiresOpenaiAuth":false}"#,
        br#"{"account":{"type":"chatgpt","planType":null},"requiresOpenaiAuth":false}"#,
        br#"{"account":{"type":"chatgpt","email":null},"requiresOpenaiAuth":false}"#,
        br#"{"account":{"type":"chatgpt","email":1,"planType":null},"requiresOpenaiAuth":false}"#,
        br#"{"account":{"type":"amazonBedrock"},"requiresOpenaiAuth":false}"#,
        br#"{"account":null,"requiresOpenaiAuth":false} trailing"#,
    ] {
        assert_eq!(
            parse_codex_account_read(bytes),
            Err(CodexProbeError::AccountInvalid),
            "unexpected acceptance"
        );
    }
    let oversized = vec![b'x'; CODEX_ACCOUNT_OUTPUT_BOUND_BYTES + 1];
    assert_eq!(
        parse_codex_account_read(&oversized),
        Err(CodexProbeError::OutputTooLarge)
    );
}

#[test]
fn version_output_validation_preserves_failure_kinds() {
    assert!(validate_codex_version_output(b"codex-cli 0.145.0").is_ok());
    assert_eq!(
        validate_codex_version_output(b"no version"),
        Err(CodexProbeError::VersionUnparseable)
    );
    assert_eq!(
        validate_codex_version_output(b"codex-cli 0.141.9"),
        Err(CodexProbeError::VersionTooOld)
    );
}

#[test]
fn readiness_is_ready_only_when_authenticated() {
    let version = validate_codex_version_output(b"codex-cli 0.145.0").unwrap();
    let authed =
        parse_codex_account_read(br#"{"account":{"type":"apiKey"},"requiresOpenaiAuth":false}"#)
            .unwrap();
    let ready = codex_readiness(version.clone(), authed);
    assert!(ready.ready());
    assert_eq!(ready.version(), "0.145.0");

    let absent =
        parse_codex_account_read(br#"{"account":null,"requiresOpenaiAuth":true}"#).unwrap();
    let not_ready = codex_readiness(version, absent);
    assert!(!not_ready.ready());
}

#[test]
fn probe_errors_are_redacted_with_stable_reasons() {
    let errors = [
        CodexProbeError::InvalidBinary,
        CodexProbeError::Unavailable,
        CodexProbeError::Timeout,
        CodexProbeError::OutputTooLarge,
        CodexProbeError::VersionUnparseable,
        CodexProbeError::VersionTooOld,
        CodexProbeError::AccountInvalid,
        CodexProbeError::Protocol,
    ];
    let reasons = [
        "invalid_binary",
        "unavailable",
        "timeout",
        "output_too_large",
        "version_unparseable",
        "version_too_old",
        "account_invalid",
        "protocol",
    ];
    for (error, expected) in errors.into_iter().zip(reasons) {
        assert_eq!(error.cli_reason(), expected);
        let display = error.to_string();
        assert!(!display.contains("secret"));
        assert!(!display.contains("sk-"));
        assert!(!format!("{error:?}").contains("token"));
    }
}

enum FixtureKind {
    Normal,
    StdoutFlood,
    StderrFlood,
    Slow,
    NonZero,
}

#[cfg(unix)]
fn fixture_command(kind: FixtureKind) -> (PathBuf, Vec<String>) {
    let script = match kind {
        FixtureKind::Normal => r#"printf 'codex-cli 0.145.0\n'"#,
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
fn fixture_command(kind: FixtureKind) -> (PathBuf, Vec<String>) {
    let shell = std::env::var_os("COMSPEC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cmd.exe"));
    let script = match kind {
        FixtureKind::Normal => "echo codex-cli 0.145.0".to_owned(),
        FixtureKind::StdoutFlood => format!("for /L %i in (1,1,2000) do @echo {}", "x".repeat(100)),
        FixtureKind::StderrFlood => {
            format!("for /L %i in (1,1,2000) do @echo {} 1>&2", "x".repeat(100))
        }
        FixtureKind::Slow => "timeout /T 30 /NOBREAK >NUL".to_owned(),
        FixtureKind::NonZero => "exit 3".to_owned(),
    };
    (shell, vec!["/C".to_owned(), script])
}

#[test]
fn version_probe_reports_real_child_output() {
    let (executable, args) = fixture_command(FixtureKind::Normal);
    let output = run_codex_version(
        &executable,
        &args,
        Duration::from_secs(15),
        CODEX_VERSION_OUTPUT_BOUND_BYTES,
    )
    .unwrap();
    assert_eq!(
        validate_codex_version_output(&output).unwrap().as_str(),
        "0.145.0"
    );
}

#[test]
fn version_probe_bounds_stdout_flood_without_deadlock() {
    let (executable, args) = fixture_command(FixtureKind::StdoutFlood);
    let started = Instant::now();
    let outcome = run_codex_version(&executable, &args, Duration::from_secs(30), 16 * 1024);
    assert_eq!(outcome, Err(CodexProbeError::OutputTooLarge));
    assert!(started.elapsed() < Duration::from_secs(25));
}

#[test]
fn version_probe_bounds_stderr_flood_without_deadlock() {
    let (executable, args) = fixture_command(FixtureKind::StderrFlood);
    let started = Instant::now();
    let outcome = run_codex_version(&executable, &args, Duration::from_secs(30), 16 * 1024);
    assert_eq!(outcome, Err(CodexProbeError::OutputTooLarge));
    assert!(started.elapsed() < Duration::from_secs(25));
}

#[test]
fn version_probe_kills_long_running_child_on_deadline() {
    let (executable, args) = fixture_command(FixtureKind::Slow);
    let started = Instant::now();
    let outcome = run_codex_version(
        &executable,
        &args,
        Duration::from_millis(500),
        CODEX_VERSION_OUTPUT_BOUND_BYTES,
    );
    assert_eq!(outcome, Err(CodexProbeError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(15));
}

#[test]
fn version_probe_maps_exit_and_spawn_failures() {
    let (executable, args) = fixture_command(FixtureKind::NonZero);
    assert_eq!(
        run_codex_version(
            &executable,
            &args,
            Duration::from_secs(15),
            CODEX_VERSION_OUTPUT_BOUND_BYTES
        ),
        Err(CodexProbeError::Unavailable)
    );

    let missing = std::env::temp_dir().join("artisan-native-engine-absent-codex.exe");
    assert!(!missing.exists());
    assert_eq!(
        run_codex_version(
            &missing,
            &[],
            Duration::from_secs(15),
            CODEX_VERSION_OUTPUT_BOUND_BYTES
        ),
        Err(CodexProbeError::InvalidBinary)
    );
}
