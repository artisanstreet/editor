//! Integration coverage for the Claude readiness probe.
//!
//! Exercises the public `artisan_native_engine::claude` probe API: version
//! extraction, auth-document strictness, absent/unavailable/timeout state
//! separation, reason bounds, readiness truth, and option validation. Live
//! process behavior is limited to the missing-binary spawn failure in
//! `claude_discovery.rs`; deadline and byte-limit enforcement against a real
//! child is controller-gate work.

use std::time::Duration;

use artisan_native_engine::claude::{
    CLAUDE_ENGINE_ID, CLAUDE_PROTOCOL_VERSION, CLAUDE_TRANSPORT, ClaudeAuthState, ClaudeProbeError,
    ClaudeProbeOptions, ClaudeProbePhase, DEFAULT_AUTH_REASON_UNAVAILABLE, MAX_AUTH_REASON_BYTES,
    NATIVE_CONTINUATION_VERSION, classify_authentication, parse_auth_logged_in,
    parse_claude_version, sanitize_auth_reason,
};

#[test]
fn descriptor_constants_match_the_typescript_adapter() {
    assert_eq!(CLAUDE_ENGINE_ID, "claude");
    assert_eq!(CLAUDE_TRANSPORT, "claude-cli-stream-json");
    assert_eq!(CLAUDE_PROTOCOL_VERSION, "claude-stream-json-v1");
    assert_eq!(NATIVE_CONTINUATION_VERSION, "2.1.220");
}

#[test]
fn version_extraction_finds_versions_and_rejects_noise() {
    assert_eq!(
        parse_claude_version("2.1.220 (Claude Code)"),
        Some("2.1.220".to_owned())
    );
    assert_eq!(
        parse_claude_version("update to 1.2.3-beta.1+build.5 today"),
        Some("1.2.3-beta.1+build.5".to_owned())
    );
    for malformed in ["", "no version here", "1.2", "1.2.3abc", "v1.2.3", "x1.2.3"] {
        assert_eq!(parse_claude_version(malformed), None, "{malformed:?}");
    }
    // Bare markers do not participate in the optional suffix groups, exactly
    // like the adapter regex, so the triple still matches.
    assert_eq!(parse_claude_version("1.2.3-"), Some("1.2.3".to_owned()));
    assert_eq!(parse_claude_version("1.2.3.4"), Some("1.2.3".to_owned()));
}

#[test]
fn auth_document_requires_the_exact_logged_in_shape() {
    assert_eq!(parse_auth_logged_in(br#"{"loggedIn": true}"#), Some(true));
    assert_eq!(
        parse_auth_logged_in(br#"  {"loggedIn": false}  "#),
        Some(false)
    );
    let malformed: &[&[u8]] = &[
        b"",
        b"not json",
        b"null",
        b"{}",
        b"{\"loggedIn\": \"yes\"}",
        b"{\"loggedIn\": 1}",
        b"{\"loggedin\": true}",
        b"{\"loggedIn\": true} trailing",
        b"{\"loggedIn\": true, \"extra\": 1}",
        b"[{\"loggedIn\": true}]",
    ];
    for document in malformed {
        assert_eq!(parse_auth_logged_in(malformed), None);
    }
}

#[test]
fn absent_unavailable_and_malformed_states_stay_distinct() {
    let unauthenticated = classify_authentication(true, br#"{"loggedIn": false}"#, b"");
    assert_eq!(unauthenticated.state(), ClaudeAuthState::Unauthenticated);
    assert!(!unauthenticated.is_ready());
    assert_eq!(unauthenticated.reason(), None);

    let failed_exit = classify_authentication(false, br#"{"loggedIn": true}"#, b"sign-in needed");
    assert_eq!(failed_exit.state(), ClaudeAuthState::Unknown);
    assert_eq!(failed_exit.reason(), Some("sign-in needed"));

    let malformed = classify_authentication(true, b"not json", b"");
    assert_eq!(malformed.state(), ClaudeAuthState::Unknown);
    assert_eq!(malformed.reason(), Some(DEFAULT_AUTH_REASON_UNAVAILABLE));
}

#[test]
fn authenticated_is_the_only_ready_state() {
    let authenticated = classify_authentication(true, br#"{"loggedIn": true}"#, b"");
    assert_eq!(authenticated.state(), ClaudeAuthState::Authenticated);
    assert!(authenticated.is_ready());

    for auth in [
        classify_authentication(true, br#"{"loggedIn": false}"#, b""),
        classify_authentication(false, b"", b""),
        classify_authentication(true, b"", b"timeout?"),
    ] {
        assert!(!auth.is_ready());
    }
}

#[test]
fn auth_reason_is_trimmed_bounded_and_fallback_safe() {
    assert_eq!(
        sanitize_auth_reason(b"  not logged in  "),
        "not logged in".to_owned()
    );
    assert_eq!(
        sanitize_auth_reason(b""),
        DEFAULT_AUTH_REASON_UNAVAILABLE.to_owned()
    );
    assert_eq!(
        sanitize_auth_reason(b"   \n\t  "),
        DEFAULT_AUTH_REASON_UNAVAILABLE.to_owned()
    );
    let long = "é".repeat(MAX_AUTH_REASON_BYTES);
    let trimmed = sanitize_auth_reason(long.as_bytes());
    assert!(trimmed.len() <= MAX_AUTH_REASON_BYTES);
    assert!(trimmed.starts_with('é'));
}

#[test]
fn probe_options_validate_deadlines_bounds_and_shapes() {
    ClaudeProbeOptions::default().validate().unwrap();

    for invalid in [
        ClaudeProbeOptions::default().with_version_timeout(Duration::ZERO),
        ClaudeProbeOptions::default().with_auth_timeout(Duration::ZERO),
        ClaudeProbeOptions::default().with_version_timeout(Duration::from_secs(301)),
        ClaudeProbeOptions::default().with_max_stdout_bytes(0),
        ClaudeProbeOptions::default().with_max_stderr_bytes(usize::MAX),
        ClaudeProbeOptions::default().with_executable_args(vec![String::new()]),
        ClaudeProbeOptions::default().with_executable_args(vec!["x".repeat(1025)]),
    ] {
        assert_eq!(
            invalid.validate(),
            Err(ClaudeProbeError::InvalidConfiguration)
        );
    }
    assert_eq!(
        ClaudeProbeError::InvalidConfiguration.cli_reason(),
        "invalid_configuration"
    );
}

#[test]
fn probe_errors_stay_path_free_and_classified() {
    assert_eq!(
        ClaudeProbeError::VersionExit { code: Some(1) }.to_string(),
        "Claude --version exited with code 1"
    );
    assert_eq!(
        ClaudeProbeError::VersionExit { code: None }.to_string(),
        "Claude --version terminated under a signal"
    );
    assert_eq!(
        ClaudeProbeError::VersionMalformed.cli_reason(),
        "version_malformed"
    );
    assert_eq!(
        ClaudeProbeError::OutputTooLarge {
            phase: ClaudeProbePhase::Authentication,
        }
        .cli_reason(),
        "output_too_large"
    );
    let timeout = ClaudeProbeError::Timeout {
        phase: ClaudeProbePhase::Version,
        timeout: Duration::from_secs(15),
    };
    assert_eq!(
        timeout.to_string(),
        "Claude version probe timed out after 15000ms"
    );
    assert_eq!(
        ClaudeProbeError::SpawnFailed {
            phase: ClaudeProbePhase::Authentication,
        }
        .cli_reason(),
        "spawn_failed"
    );
}
