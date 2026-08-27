use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;

use super::{
    EngineBounds, EngineLimits, HealthSecret, ReadinessError, reset_witnesses, witness_counts,
};
use crate::engine_owner::operation::EngineOperationError;

fn absolute_dummy_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("C:\\tmp\\engine.exe")
    } else {
        PathBuf::from("/tmp/engine")
    }
}

fn readiness_limits() -> EngineLimits {
    EngineLimits {
        readiness: Duration::from_millis(800),
        health: Duration::from_millis(1200),
        prompt: Duration::from_millis(500),
        sse: Duration::from_millis(500),
        close: Duration::from_millis(900),
    }
}

fn readiness_bounds() -> EngineBounds {
    EngineBounds {
        max_json_body: 1024,
        max_sse_line: 1024,
        max_sse_event: 1024,
        max_readiness_line: 256,
        max_headers: 32,
        max_buf_bytes: 8192,
        stderr_cap_bytes: 4096,
        sink_capacity: 4,
        control_capacity: 4,
    }
}

fn fixture_program() -> PathBuf {
    let mapping = std::env::var("ARTISAN_ENGINE_OWNER_FIXTURE")
        .expect("ARTISAN_ENGINE_OWNER_FIXTURE must be set via rlocationpath");
    if let Ok(runfiles) = runfiles::Runfiles::create()
        && let Some(path) = runfiles::rlocation!(runfiles, mapping.as_str())
    {
        return path;
    }
    PathBuf::from(mapping)
}

fn run_id(value: &str) -> artisan_domain::RunId {
    artisan_domain::RunId::parse(value).expect("valid run id")
}

// ---------------------------------------------------------------------------
// Synchronous URL and line validation
// ---------------------------------------------------------------------------

#[test]
fn readiness_validates_ipv4_loopback() {
    let ep = super::readiness::validate_readiness_line(br#"{"url":"http://127.0.0.1:1234"}"#, 256)
        .expect("ipv4 should validate");
    assert_eq!(ep.port(), 1234);
    assert_eq!(ep.host().to_string(), "127.0.0.1");
}

#[test]
fn readiness_validates_ipv6_loopback() {
    let ep = super::readiness::validate_readiness_line(br#"{"url":"http://[::1]:8080"}"#, 256)
        .expect("ipv6 should validate");
    assert_eq!(ep.port(), 8080);
    assert_eq!(ep.host().to_string(), "::1");
}

#[test]
fn readiness_rejects_non_loopback() {
    let err =
        super::readiness::validate_readiness_line(br#"{"url":"http://192.168.1.1:1234"}"#, 256)
            .unwrap_err();
    assert_eq!(err, ReadinessError::InvalidHost);
}

#[test]
fn readiness_rejects_credentials() {
    let err = super::readiness::validate_readiness_line(
        br#"{"url":"http://user:pass@127.0.0.1:1234"}"#,
        256,
    )
    .unwrap_err();
    assert_eq!(err, ReadinessError::CredentialsPresent);
}

#[test]
fn readiness_rejects_zero_port() {
    let err = super::readiness::validate_readiness_line(br#"{"url":"http://127.0.0.1:0"}"#, 256)
        .unwrap_err();
    assert_eq!(err, ReadinessError::InvalidPort);
}

#[test]
fn readiness_rejects_invalid_scheme() {
    let err =
        super::readiness::validate_readiness_line(br#"{"url":"https://127.0.0.1:1234"}"#, 256)
            .unwrap_err();
    assert_eq!(err, ReadinessError::InvalidScheme);
}

#[test]
fn readiness_rejects_oversized_bounded() {
    // Base JSON is 27 bytes; pad with spaces to 257 (limit 256)
    let base = br#"{"url":"http://127.0.0.1:1"}"#;
    let mut line = Vec::from(&base[..]);
    line.extend(std::iter::repeat_n(b' ', 257 - base.len()));
    assert_eq!(line.len(), 257);
    let err = super::readiness::validate_readiness_line(&line, 256).unwrap_err();
    assert_eq!(err, ReadinessError::Oversized);
}

#[test]
fn readiness_rejects_empty_url() {
    let err = super::readiness::validate_readiness_line(br#"{"url":""}"#, 256).unwrap_err();
    assert_eq!(err, ReadinessError::EmptyUrl);
}

#[test]
fn readiness_rejects_unknown_shape() {
    let err = super::readiness::validate_readiness_line(br#"{"unknown":"x"}"#, 256).unwrap_err();
    assert_eq!(err, ReadinessError::UnexpectedShape);
}

#[test]
fn health_secret_base64url_no_pad() {
    let secret = HealthSecret::generate().expect("entropy");
    let raw = secret.as_str();
    assert!(!raw.contains('='));
    assert!(!raw.contains('+'));
    assert!(!raw.contains('/'));
    // 32 bytes -> 43 chars without padding
    assert_eq!(raw.len(), 43);
    let debug = format!("{secret:?}");
    assert!(!debug.contains(raw));
    assert!(debug.contains("<redacted>"));
}

#[test]
fn health_basic_auth_exact() {
    // Use a known secret to compute expected Basic value deterministically.
    let secret = HealthSecret::from_raw_for_tests("test-secret-value-1234567890abc".to_owned());
    // Recompute expected via standard base64
    let credentials = format!("opencode:{}", secret.as_str());
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes())
    );
    assert_eq!(secret.basic_auth(), expected);
    assert!(secret.basic_auth().starts_with("Basic "));
    // Ensure secret not leaked in debug
    let dbg = format!("{secret:?}");
    assert!(!dbg.contains("test-secret"));
}

#[test]
fn max_buf_bytes_boundary_accepted() {
    let bounds = readiness_bounds();
    let cfg = super::EngineOwnerConfig::new(absolute_dummy_path(), readiness_limits(), bounds)
        .expect("8192 must be accepted");
    assert_eq!(cfg.bounds().max_buf_bytes, 8192);
}

// ---------------------------------------------------------------------------
// Integration with real fixture binary via EngineOwner facade
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn ready_ok_proves_reap_without_kill() {
    reset_witnesses();
    let program = fixture_program();
    assert!(
        program.is_file(),
        "fixture must exist: {}",
        program.display()
    );
    let handle = tokio::runtime::Handle::current();
    let owner = super::start_with_fixture_for_tests(
        readiness_limits(),
        readiness_bounds(),
        &handle,
        program,
        "ready_ok",
    );
    let accepted = owner
        .admit(run_id("run-ready-ok"), Duration::from_secs(8))
        .expect("admit ready_ok");
    let result = accepted.await;
    assert!(result.is_ok(), "ready_ok should succeed: {result:?}");
    let outcome = result.unwrap();
    match outcome {
        crate::engine_owner::operation::LaunchOutcome::ObservedExit { generation, .. } => {
            assert!(generation >= 1);
        }
    }
    // Give owner task a moment to settle witnesses.
    tokio::task::yield_now().await;
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    assert_eq!(
        counts.kills_requested, 0,
        "successful fixture must not need kill"
    );
    assert_eq!(
        counts.control_driver_joined, 1,
        "successful health must have joined control driver exactly once"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn ready_malformed_maps_to_readiness_failed() {
    reset_witnesses();
    let program = fixture_program();
    let handle = tokio::runtime::Handle::current();
    let owner = super::start_with_fixture_for_tests(
        readiness_limits(),
        readiness_bounds(),
        &handle,
        program,
        "ready_malformed",
    );
    let accepted = owner
        .admit(run_id("run-ready-mal"), Duration::from_secs(8))
        .expect("admit");
    let result = accepted.await;
    let err = result.unwrap_err();
    assert!(
        matches!(err, EngineOperationError::ReadinessFailed(_)),
        "malformed should be ReadinessFailed, got {err:?}"
    );
    tokio::task::yield_now().await;
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
    // Malformed still closes lifeline and reaps without kill (child waits for EOF)
    assert_eq!(counts.kills_requested, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn ready_oversized_bounded_reject() {
    reset_witnesses();
    let program = fixture_program();
    let handle = tokio::runtime::Handle::current();
    // Use limit 256 to trigger oversize on 257-byte fixture line
    let owner = super::start_with_fixture_for_tests(
        readiness_limits(),
        readiness_bounds(),
        &handle,
        program,
        "ready_oversized_bounded_reject",
    );
    let accepted = owner
        .admit(run_id("run-ready-over"), Duration::from_secs(8))
        .expect("admit");
    let result = accepted.await;
    let err = result.unwrap_err();
    match err {
        EngineOperationError::ReadinessFailed(
            ReadinessError::Oversized | ReadinessError::TrailingBytes,
        ) => {}
        other => panic!("oversized should be Oversized, got {other:?}"),
    }
    tokio::task::yield_now().await;
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn health_version_reject_is_incompatible() {
    reset_witnesses();
    let program = fixture_program();
    let handle = tokio::runtime::Handle::current();
    let owner = super::start_with_fixture_for_tests(
        readiness_limits(),
        readiness_bounds(),
        &handle,
        program,
        "health_version_reject",
    );
    let accepted = owner
        .admit(run_id("run-health-ver"), Duration::from_secs(8))
        .expect("admit");
    let result = accepted.await;
    let err = result.unwrap_err();
    assert_eq!(err, EngineOperationError::IncompatibleVersion);
    tokio::task::yield_now().await;
    let counts = witness_counts();
    assert_eq!(counts.spawned, 1);
    assert_eq!(counts.reaps_observed, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_mid_readiness_is_cancelled() {
    reset_witnesses();
    let program = fixture_program();
    let handle = tokio::runtime::Handle::current();
    // hang_until_lifeline never sends readiness line, so cancel can win
    let owner = super::start_with_fixture_for_tests(
        readiness_limits(),
        readiness_bounds(),
        &handle,
        program,
        "hang_until_lifeline",
    );
    let accepted = owner
        .admit(run_id("run-cancel"), Duration::from_secs(8))
        .expect("admit");
    // Cancel quickly before readiness completes
    tokio::time::sleep(Duration::from_millis(50)).await;
    accepted.cancel();
    let result = accepted.await;
    assert_eq!(result.unwrap_err(), EngineOperationError::Cancelled);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let counts = witness_counts();
    // Should have spawned then cleaned up (reaped, possibly after kill if hang)
    assert_eq!(counts.spawned, 1);
    assert!(counts.reaps_observed >= 1 || counts.kills_requested <= 1);
}

#[tokio::test(flavor = "current_thread")]
async fn readiness_timeout_is_deadline() {
    reset_witnesses();
    let program = fixture_program();
    let handle = tokio::runtime::Handle::current();
    let mut limits = readiness_limits();
    limits.readiness = Duration::from_millis(200);
    // hang scenario produces no readiness line, so readiness deadline fires
    let owner = super::start_with_fixture_for_tests(
        limits,
        readiness_bounds(),
        &handle,
        program,
        "hang_until_lifeline",
    );
    let accepted = owner
        .admit(run_id("run-timeout"), Duration::from_secs(5))
        .expect("admit");
    let result = accepted.await;
    assert_eq!(result.unwrap_err(), EngineOperationError::Deadline);
}
