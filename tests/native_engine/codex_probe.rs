//! Codex readiness probe fixtures: account parsing, auth states, bounds.
//!
//! Registration (controller-owned, not part of this packet):
//! `tests/native_engine/BUILD.bazel` gains a `rust_test` target for
//! `codex_probe.rs` depending on `//modules/native_engine:native_engine`.
//!
//! CLI login-status text is never treated as account evidence here: only
//! `parse_codex_account_read` counts, and a bare status string stays
//! `AccountInvalid`.

use artisan_native_engine::codex::{
    CODEX_ACCOUNT_OUTPUT_BOUND_BYTES, CODEX_VERSION_OUTPUT_BOUND_BYTES, CodexProbeError,
    CodexVersionFixture, classify_codex_auth, classify_version_fixture, codex_readiness,
    parse_codex_account_read, validate_codex_version_output,
};

#[test]
fn account_matrix_covers_all_wire_shapes() {
    let api_key =
        parse_codex_account_read(br#"{"account":{"type":"apiKey"},"requiresOpenaiAuth":false}"#)
            .unwrap();
    assert_eq!(classify_codex_auth(api_key).is_authenticated(), true);

    let chatgpt = parse_codex_account_read(
        br#"{"account":{"type":"chatgpt","email":"user@example.com","planType":"plus"},"requiresOpenaiAuth":false}"#,
    )
    .unwrap();
    assert_eq!(classify_codex_auth(chatgpt).is_authenticated(), true);

    let chatgpt_null_email = parse_codex_account_read(
        br#"{"account":{"type":"chatgpt","email":null,"planType":null},"requiresOpenaiAuth":false}"#,
    )
    .unwrap();
    assert_eq!(
        classify_codex_auth(chatgpt_null_email).is_authenticated(),
        true
    );

    let bedrock = parse_codex_account_read(
        br#"{"account":{"type":"amazonBedrock","credentialSource":"profile"},"requiresOpenaiAuth":false}"#,
    )
    .unwrap();
    assert_eq!(classify_codex_auth(bedrock).is_authenticated(), true);

    let absent_required =
        parse_codex_account_read(br#"{"account":null,"requiresOpenaiAuth":true}"#).unwrap();
    assert_eq!(
        classify_codex_auth(absent_required).is_authenticated(),
        false
    );

    let absent_free =
        parse_codex_account_read(br#"{"account":null,"requiresOpenaiAuth":false}"#).unwrap();
    assert_eq!(classify_codex_auth(absent_free).is_authenticated(), false);
}

#[test]
fn unauthenticated_reasons_follow_requires_openai_auth() {
    let required =
        parse_codex_account_read(br#"{"account":null,"requiresOpenaiAuth":true}"#).unwrap();
    assert_eq!(
        classify_codex_auth(required),
        artisan_native_engine::codex::CodexAuthState::Unauthenticated {
            reason: "OpenAI authentication required"
        }
    );
    let free = parse_codex_account_read(br#"{"account":null,"requiresOpenaiAuth":false}"#).unwrap();
    assert_eq!(
        classify_codex_auth(free),
        artisan_native_engine::codex::CodexAuthState::Unauthenticated {
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
        br#"{"account":null,"requiresOpenaiAuth":"yes"}"#,
        br#"{"account":{"type":"oauth"},"requiresOpenaiAuth":false}"#,
        br#"{"account":{"type":"apiKey","extra":1},"requiresOpenaiAuth":false}"#,
        br#"{"account":{"type":"chatgpt","email":1,"planType":null},"requiresOpenaiAuth":false}"#,
        br#"{"account":{"type":"amazonBedrock"},"requiresOpenaiAuth":false}"#,
        br#"{"account":null,"requiresOpenaiAuth":false,"extra":1}"#,
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
fn fixture_classifier_covers_timeout_bounds_and_exit_codes() {
    let timeout = classify_version_fixture(CodexVersionFixture {
        exit_code: Some(0),
        stdout: b"codex-cli 0.145.0".to_vec(),
        stdout_truncated: false,
        timed_out: true,
    });
    assert_eq!(timeout, Err(CodexProbeError::Timeout));

    let truncated = classify_version_fixture(CodexVersionFixture {
        exit_code: Some(0),
        stdout: b"codex-cli 0.145.0".to_vec(),
        stdout_truncated: true,
        timed_out: false,
    });
    assert_eq!(truncated, Err(CodexProbeError::OutputTooLarge));

    let oversized = classify_version_fixture(CodexVersionFixture {
        exit_code: Some(0),
        stdout: vec![b'x'; CODEX_VERSION_OUTPUT_BOUND_BYTES + 1],
        stdout_truncated: false,
        timed_out: false,
    });
    assert_eq!(oversized, Err(CodexProbeError::OutputTooLarge));

    let nonzero = classify_version_fixture(CodexVersionFixture {
        exit_code: Some(1),
        stdout: b"".to_vec(),
        stdout_truncated: false,
        timed_out: false,
    });
    assert_eq!(nonzero, Err(CodexProbeError::Unavailable));

    let signaled = classify_version_fixture(CodexVersionFixture {
        exit_code: None,
        stdout: b"".to_vec(),
        stdout_truncated: false,
        timed_out: false,
    });
    assert_eq!(signaled, Err(CodexProbeError::Unavailable));

    let ok = classify_version_fixture(CodexVersionFixture {
        exit_code: Some(0),
        stdout: b"codex-cli 0.145.0".to_vec(),
        stdout_truncated: false,
        timed_out: false,
    })
    .unwrap();
    assert_eq!(ok, b"codex-cli 0.145.0");
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
    ];
    let reasons = [
        "invalid_binary",
        "unavailable",
        "timeout",
        "output_too_large",
        "version_unparseable",
        "version_too_old",
        "account_invalid",
    ];
    for (error, expected) in errors.into_iter().zip(reasons) {
        assert_eq!(error.cli_reason(), expected);
        let display = error.to_string();
        assert!(!display.contains("C:\\secret"));
        assert!(!display.contains("sk-"));
        assert!(!format!("{error:?}").contains("token"));
    }
}
