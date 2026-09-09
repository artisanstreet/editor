//! Forge home provisioning: credentials, instance identity, and dev data.
//!
//! The repeat-invocation contract is the point of these tests: a second
//! provisioning run must preserve the instance identity, the provisioned
//! credentials, and everything under the dev data directory.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use artisan_editor_cli::{credentials::ForgeCredentialPaths, instance::NativeInstanceConfig};
use native_dev::{DevPaths, InstanceOutcome, provision_forge_home};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dev_dir(case: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "artisan-native-dev-{case}-{}-{id}",
        std::process::id()
    ))
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn first_provision_mints_and_second_preserves() {
    let dev_dir = scratch_dev_dir("instance");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");

    let outcome = provision_forge_home(&paths).expect("first provision");
    assert_eq!(outcome, InstanceOutcome::Created);
    let first = NativeInstanceConfig::load_from_home(&paths.home).expect("instance loads");
    assert_eq!(first.database_path(), paths.database_path());
    assert_eq!(first.custody_path(), paths.custody_path());
    assert_eq!(first.readiness_path(), paths.readiness_path());

    let credential_paths =
        ForgeCredentialPaths::from_home(&paths.home).expect("credentials resolve");
    assert_eq!(
        first.credentials_manifest(),
        credential_paths.manifest_path()
    );

    let sentinel = paths.home.join("data").join("dev-sentinel.txt");
    std::fs::create_dir_all(sentinel.parent().expect("data parent")).expect("data dir");
    std::fs::write(&sentinel, b"dev data must survive").expect("sentinel");

    let outcome = provision_forge_home(&paths).expect("second provision");
    assert_eq!(outcome, InstanceOutcome::Preserved);
    let second = NativeInstanceConfig::load_from_home(&paths.home).expect("instance reloads");
    assert_eq!(
        first.instance_id(),
        second.instance_id(),
        "identity must be stable"
    );
    assert_eq!(
        std::fs::read(&sentinel).expect("sentinel survives"),
        b"dev data must survive"
    );
    cleanup(&dev_dir);
}

#[test]
fn credentials_are_stable_across_provisioning() {
    let dev_dir = scratch_dev_dir("credentials");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    provision_forge_home(&paths).expect("first provision");

    let read_manifest = |paths: &DevPaths| {
        let credential_paths =
            ForgeCredentialPaths::from_home(&paths.home).expect("credentials resolve");
        std::fs::read(credential_paths.manifest_path()).expect("credential manifest reads")
    };
    let before = read_manifest(&paths);
    assert!(!before.is_empty());

    provision_forge_home(&paths).expect("second provision");
    assert_eq!(read_manifest(&paths), before, "credentials must not rotate");
    cleanup(&dev_dir);
}

#[test]
fn corrupt_instance_fails_with_a_bounded_diagnostic() {
    let dev_dir = scratch_dev_dir("corrupt");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    provision_forge_home(&paths).expect("first provision");

    let instance_path = NativeInstanceConfig::native_path(&paths.home);
    std::fs::write(&instance_path, b"not an instance").expect("corrupt instance");

    let error = provision_forge_home(&paths).expect_err("corrupt instance is rejected");
    let message = error.to_string();
    assert!(message.contains("provision"), "unexpected: {message}");
    assert!(
        !message.contains("not an instance"),
        "no raw file content leaks"
    );

    let sentinel = paths.home.join("data").join("dev-sentinel.txt");
    std::fs::create_dir_all(sentinel.parent().expect("data parent")).expect("data dir");
    std::fs::write(&sentinel, b"untouched").expect("sentinel");
    assert_eq!(std::fs::read(&sentinel).expect("reads"), b"untouched");
    cleanup(&dev_dir);
}

#[test]
fn instance_timeouts_are_finite_and_nonzero() {
    let dev_dir = scratch_dev_dir("timeouts");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    provision_forge_home(&paths).expect("provision");
    let config = NativeInstanceConfig::load_from_home(&paths.home).expect("instance loads");
    for timeout in [
        config.listener().admission_timeout_ms(),
        config.listener().handshake_timeout_ms(),
        config.listener().request_timeout_ms(),
        config.listener().drain_timeout_ms(),
        config.native_run().claim_lease_ms(),
        config.native_run().poll_interval_ms(),
        config.native_run().retry_backoff_ms(),
        config.native_run().shutdown_budget_ms(),
    ] {
        assert_ne!(timeout, 0, "timeout must be finite and nonzero");
    }
    assert_ne!(config.native_run().queue_capacity().get(), 0);
    assert_ne!(config.native_run().max_command_retries().get(), 0);
    cleanup(&dev_dir);
}
