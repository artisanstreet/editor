//! Launch control: staged paths and bounded startup confirmation.
//!
//! Two guarantees are pinned here. First, the launcher spawns the staged
//! Editor and checks the staged Forge — never build-output paths — so a
//! test with different source and staged directories proves the wiring.
//! Second, startup confirmation reads the Editor's own receipt: ready and
//! failed receipts resolve immediately, while missing or corrupt receipts
//! wait out the (short, test-controlled) deadline.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use native_dev::{
    BinarySet, DevPaths, StartupWait, read_receipt, staged_editor, staged_forge, wait_for_startup,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dev_dir(case: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "artisan-native-dev-{case}-{}-{id}",
        std::process::id()
    ))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn receipt_env_and_schema_match_the_frontend_contract() {
    assert_eq!(
        native_dev::STARTUP_RECEIPT_ENV,
        "ARTISAN_DEV_STARTUP_RECEIPT"
    );
    assert_eq!(native_dev::STARTUP_RECEIPT_SCHEMA, "artisan-dev-startup-v1");
}

#[test]
fn launch_uses_staged_paths_not_sources() {
    let dev_dir = scratch_dev_dir("staged-paths");
    let sources = dev_dir.join("sources");
    std::fs::create_dir_all(&sources).expect("sources");
    let source_path = |stem: &str| {
        let path = sources.join(native_dev::exe_name(stem));
        std::fs::write(&path, b"source").expect("source binary");
        path
    };
    let set = BinarySet {
        ae: source_path("ae"),
        editor: source_path("editor"),
        forge: source_path("forge"),
        installer: source_path("installer"),
    };
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    assert_ne!(staged_editor(&paths), set.editor);
    assert_ne!(staged_forge(&paths), set.forge);
    assert_eq!(
        staged_editor(&paths),
        paths.version_bin.join(native_dev::exe_name("editor"))
    );
    assert_eq!(
        staged_forge(&paths),
        paths.version_bin.join(native_dev::exe_name("forge"))
    );
    cleanup(&dev_dir);
}

fn write_receipt(dir: &Path, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("receipt dir");
    let path = dir.join("startup-receipt.json");
    std::fs::write(&path, body).expect("receipt write");
    path
}

#[test]
fn ready_receipt_resolves_immediately() {
    let dir = scratch_dev_dir("receipt-ready");
    let path = write_receipt(
        &dir,
        r#"{"schema":"artisan-dev-startup-v1","status":"ready","stage":"initial-catalog","detail":"ok"}"#,
    );
    match read_receipt(&path) {
        Some(StartupWait::Ready { stage }) => assert_eq!(stage, "initial-catalog"),
        other => panic!("expected a ready receipt, got {other:?}"),
    }
    cleanup(&dir);
}

#[test]
fn failed_receipt_carries_stage_and_reason() {
    let dir = scratch_dev_dir("receipt-failed");
    let path = write_receipt(
        &dir,
        r#"{"schema":"artisan-dev-startup-v1","status":"failed","stage":"handshake","detail":"handshake (authentication)"}"#,
    );
    match read_receipt(&path) {
        Some(StartupWait::Failed { stage, reason }) => {
            assert_eq!(stage, "handshake");
            assert!(reason.contains("authentication"), "reason: {reason}");
        }
        other => panic!("expected a failed receipt, got {other:?}"),
    }
    cleanup(&dir);
}

#[test]
fn missing_and_corrupt_receipts_are_not_receipts() {
    let dir = scratch_dev_dir("receipt-invalid");
    std::fs::create_dir_all(&dir).expect("receipt dir");
    let missing = dir.join("absent.json");
    assert_eq!(read_receipt(&missing), None);

    let corrupt = write_receipt(&dir, "not json");
    assert_eq!(read_receipt(&corrupt), None);

    let wrong_schema = write_receipt(&dir, r#"{"schema":"other","status":"ready"}"#);
    assert_eq!(read_receipt(&wrong_schema), None);

    let unknown_status = write_receipt(
        &dir,
        r#"{"schema":"artisan-dev-startup-v1","status":"starting"}"#,
    );
    assert_eq!(read_receipt(&unknown_status), None);
    cleanup(&dir);
}

/// Spawns a short sleeper as an Editor stand-in.
#[cfg(windows)]
fn spawn_sleeper() -> std::process::Child {
    std::process::Command::new("cmd")
        .args(["/C", "ping -n 6 127.0.0.1 >nul"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("sleeper spawns")
}

/// Spawns a short sleeper as an Editor stand-in.
#[cfg(not(windows))]
fn spawn_sleeper() -> std::process::Child {
    std::process::Command::new("sleep")
        .arg("5")
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("sleeper spawns")
}

/// Spawns an immediate failure as an Editor stand-in.
#[cfg(windows)]
fn spawn_exiter() -> std::process::Child {
    std::process::Command::new("cmd")
        .args(["/C", "exit 3"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("exiter spawns")
}

/// Spawns an immediate failure as an Editor stand-in.
#[cfg(not(windows))]
fn spawn_exiter() -> std::process::Child {
    std::process::Command::new("sh")
        .args(["-c", "exit 3"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("exiter spawns")
}

#[test]
fn wait_returns_timeout_without_a_receipt() {
    let dir = scratch_dev_dir("wait-timeout");
    std::fs::create_dir_all(&dir).expect("receipt dir");
    let mut child = spawn_sleeper();
    let outcome = wait_for_startup(
        &mut child,
        &dir.join("absent.json"),
        Duration::from_millis(300),
    );
    assert_eq!(outcome, StartupWait::Timeout);
    let _ = child.kill();
    let _ = child.wait();
    cleanup(&dir);
}

#[test]
fn wait_sees_an_editor_that_exits_early() {
    let dir = scratch_dev_dir("wait-exited");
    std::fs::create_dir_all(&dir).expect("receipt dir");
    let mut child = spawn_exiter();
    let outcome = wait_for_startup(
        &mut child,
        &dir.join("absent.json"),
        Duration::from_millis(5_000),
    );
    match outcome {
        StartupWait::EditorExited { code } => assert_eq!(code, Some(3)),
        other => panic!("expected an early exit, got {other:?}"),
    }
    let _ = child.wait();
    cleanup(&dir);
}

#[test]
fn wait_resolves_a_receipt_written_mid_wait() {
    let dir = scratch_dev_dir("wait-ready");
    std::fs::create_dir_all(&dir).expect("receipt dir");
    let receipt = dir.join("startup-receipt.json");
    let writer = receipt.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(
            &writer,
            r#"{"schema":"artisan-dev-startup-v1","status":"ready","stage":"initial-catalog","detail":"ok"}"#,
        )
        .expect("receipt write");
    });
    let mut child = spawn_sleeper();
    let outcome = wait_for_startup(&mut child, &receipt, Duration::from_millis(5_000));
    assert!(
        matches!(outcome, StartupWait::Ready { .. }),
        "got {outcome:?}"
    );
    let _ = handle.join();
    let _ = child.kill();
    let _ = child.wait();
    cleanup(&dir);
}
