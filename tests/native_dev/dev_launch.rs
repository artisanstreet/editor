//! Launch control: installed paths and bounded startup confirmation.
//!
//! Two guarantees are pinned here. First, the launcher spawns the installed
//! Editor and checks the installed Forge — never build-output paths — so a
//! test with different source and installed directories proves the wiring.
//! Second, startup confirmation reads the Editor's own receipt: ready and
//! failed receipts resolve immediately, while missing or corrupt receipts
//! wait out the (short, test-controlled) deadline.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use native_dev::{
    BinarySet, DevPaths, EditorOutput, ReadinessReconcile, StartupWait, editor_log_path,
    editor_output, read_receipt, reconcile_stale_readiness, spawn_editor, staged_editor,
    staged_forge, wait_for_startup,
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

fn version_root(paths: &DevPaths) -> PathBuf {
    paths
        .home
        .join("versions")
        .join("0.0.0-dev.1+gabc.b0123456789")
}

#[test]
fn receipt_env_and_schema_match_the_frontend_contract() {
    assert_eq!(
        native_dev::STARTUP_RECEIPT_ENV,
        "ARTISAN_DEV_STARTUP_RECEIPT"
    );
    assert_eq!(native_dev::STARTUP_RECEIPT_SCHEMA, "artisan-dev-startup-v1");
    assert_eq!(native_dev::OWNED_DEV_FORGE_ENV, "ARTISAN_DEV_OWNED_FORGE");
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
    let installed = version_root(&paths);
    assert_ne!(staged_editor(&installed), set.editor);
    assert_ne!(staged_forge(&installed), set.forge);
    assert_eq!(
        staged_editor(&installed),
        installed.join("bin").join(native_dev::exe_name("editor"))
    );
    assert_eq!(
        staged_forge(&installed),
        installed.join("bin").join(native_dev::exe_name("forge"))
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

#[test]
fn editor_output_detaches_unless_attached_or_on_a_terminal() {
    let log = PathBuf::from("/dev-root/.dev-runner/editor.log");
    // A piped or redirected run (an agent shell, `| tee`, CI) must not hand
    // a long-lived Editor the pipe the caller waits on for end-of-file.
    assert_eq!(
        editor_output(false, false, log.clone()),
        EditorOutput::Detached { log: log.clone() }
    );
    // A terminal has no end-of-file to wait on, and an attached runner lives
    // exactly as long as the Editor: both keep the Editor's output visible.
    assert_eq!(
        editor_output(false, true, log.clone()),
        EditorOutput::Inherit
    );
    assert_eq!(
        editor_output(true, false, log.clone()),
        EditorOutput::Inherit
    );
    assert_eq!(editor_output(true, true, log), EditorOutput::Inherit);
}

#[test]
fn editor_log_lives_in_the_runner_directory() {
    let dev_dir = scratch_dev_dir("editor-log");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    assert_eq!(
        editor_log_path(&paths),
        paths.runner_dir().join("editor.log")
    );
}

/// A detached Editor on Unix writes to the runner's log and holds none of
/// the runner's standard streams, so a caller reading `cargo dev` through a
/// pipe sees end-of-file as soon as the runner returns.
#[cfg(target_os = "linux")]
#[test]
fn detached_editor_writes_its_log_and_holds_no_runner_stream() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = scratch_dev_dir("detached-log");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let editor = dir.join("editor");
    std::fs::write(&editor, "#!/bin/sh\necho editor started\nexec sleep 5\n").expect("stand-in");
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let log = dir.join("editor.log");
    let receipt = dir.join("startup-receipt.json");
    let mut process = spawn_editor(
        &editor,
        &dir,
        &receipt,
        &EditorOutput::Detached { log: log.clone() },
    )
    .expect("detached spawn");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !std::fs::read_to_string(&log).is_ok_and(|text| text.contains("editor started")) {
        assert!(
            std::time::Instant::now() < deadline,
            "the editor's output reaches its log"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    for descriptor in [1, 2] {
        let target = std::fs::read_link(format!("/proc/{}/fd/{descriptor}", process.pid()))
            .expect("descriptor readable");
        assert_eq!(
            target, log,
            "fd {descriptor} is the log, not a runner stream"
        );
    }
    let stdin = std::fs::read_link(format!("/proc/{}/fd/0", process.pid())).expect("stdin");
    assert_eq!(stdin, Path::new("/dev/null"));
    assert_eq!(
        wait_for_startup(process.child_mut(), &receipt, Duration::from_millis(100)),
        StartupWait::Timeout
    );
    let _ = process.stop();
    cleanup(&dir);
}

/// A detached Windows launch goes through the shell watcher, which reports
/// the Editor's process id and exits with the Editor's exit code.
#[cfg(windows)]
#[test]
fn detached_windows_launch_reports_the_editor_and_its_exit() {
    let dir = scratch_dev_dir("detached-windows");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let root = std::env::var_os("SystemRoot").expect("SystemRoot");
    // A stand-in that runs and exits on its own, without arguments.
    let editor = PathBuf::from(root).join("System32").join("hostname.exe");
    let receipt = dir.join("startup-receipt.json");
    let mut process = spawn_editor(
        &editor,
        &dir,
        &receipt,
        &EditorOutput::Detached {
            log: dir.join("editor.log"),
        },
    )
    .expect("detached spawn");
    assert_ne!(process.pid(), 0);
    let outcome = wait_for_startup(process.child_mut(), &receipt, Duration::from_secs(20));
    assert_eq!(outcome, StartupWait::EditorExited { code: Some(0) });
    process.release();
    cleanup(&dir);
}

/// A syntactically valid readiness receipt naming a pid that cannot exist.
///
/// `u32::MAX` passes the CLI receipt validation (nonzero pid, loopback
/// endpoint, 64-hex pin) while no live process can match it, so the
/// launcher must treat it as stale — exactly the owned-Forge-killed state
/// the restart fix targets.
fn dead_forge_receipt() -> Vec<u8> {
    br#"{"schema":"artisan-forge-ready-v1","endpoint":"127.0.0.1:9","certificate_sha256":"abababababababababababababababababababababababababababababababab","pid":4294967295}"#.to_vec()
}

fn readiness_home(case: &str) -> (PathBuf, DevPaths) {
    let dev_dir = scratch_dev_dir(case);
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    // Mirror provision: the readiness and custody directories exist before
    // any Forge runs, so reconcile only ever sees unexpected shapes.
    for runtime_path in [
        paths.readiness_path(),
        paths.custody_path(),
        paths.database_path(),
    ] {
        std::fs::create_dir_all(runtime_path.parent().expect("runtime parent"))
            .expect("runtime dir");
    }
    (dev_dir, paths)
}

#[test]
fn missing_readiness_reconciles_to_absent() {
    let (dev_dir, paths) = readiness_home("reconcile-missing");
    assert_eq!(
        reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
            .expect("missing reconciles"),
        ReadinessReconcile::Absent
    );
    cleanup(&dev_dir);
}

#[test]
fn stale_valid_readiness_permits_a_second_launch() {
    let (dev_dir, paths) = readiness_home("reconcile-stale");
    std::fs::write(paths.readiness_path(), dead_forge_receipt()).expect("stale receipt");
    // Publish temporaries are never swept: a stale temporary cannot block
    // the next publish, and deleting by pattern would violate preservation.
    let stray = paths
        .readiness_path()
        .parent()
        .expect("parent")
        .join(".artisan-forge-ready-19808-0.tmp");
    std::fs::write(&stray, b"orphan publish temporary").expect("stray tmp");
    let sibling = paths
        .readiness_path()
        .parent()
        .expect("parent")
        .join("notes.txt");
    std::fs::write(&sibling, b"operator notes").expect("sibling");

    assert_eq!(
        reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
            .expect("stale reconciles"),
        ReadinessReconcile::CleanedStale { pid: u32::MAX }
    );
    assert!(
        !paths.readiness_path().exists(),
        "stale receipt must be gone before the next publish"
    );
    assert!(stray.exists(), "publish temporaries are never swept");
    assert!(sibling.exists(), "unrelated siblings are preserved");
    cleanup(&dev_dir);
}

#[test]
fn malformed_readiness_is_preserved_and_refused() {
    let (dev_dir, paths) = readiness_home("reconcile-malformed");
    std::fs::write(paths.readiness_path(), b"not a receipt").expect("malformed receipt");
    let error = reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
        .expect_err("malformed refused");
    assert!(
        error.to_string().contains("malformed"),
        "unexpected: {error}"
    );
    assert!(
        paths.readiness_path().exists(),
        "malformed bytes are preserved, never deleted"
    );
    cleanup(&dev_dir);
}

#[test]
fn oversized_readiness_is_preserved_and_refused() {
    let (dev_dir, paths) = readiness_home("reconcile-oversized");
    std::fs::write(paths.readiness_path(), vec![b'x'; 5000]).expect("oversized receipt");
    let error = reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
        .expect_err("oversized refused");
    assert!(
        error.to_string().contains("size bound"),
        "unexpected: {error}"
    );
    assert!(
        paths.readiness_path().exists(),
        "oversized bytes are preserved"
    );
    cleanup(&dev_dir);
}

#[test]
fn non_file_readiness_is_preserved_and_refused() {
    let (dev_dir, paths) = readiness_home("reconcile-dir");
    std::fs::create_dir_all(paths.readiness_path()).expect("directory at receipt path");
    let error = reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
        .expect_err("directory refused");
    assert!(
        error.to_string().contains("not a regular file"),
        "unexpected: {error}"
    );
    assert!(paths.readiness_path().is_dir(), "directory is preserved");
    cleanup(&dev_dir);
}

#[test]
fn publish_temporaries_are_never_swept() {
    let (dev_dir, paths) = readiness_home("reconcile-tmp");
    let parent = paths
        .readiness_path()
        .parent()
        .expect("parent")
        .to_path_buf();
    // Even the exact runtime temporary shape is preserved: stale
    // temporaries cannot block the next publish, so nothing but a
    // proven-stale receipt is ever removed.
    let exact = parent.join(".artisan-forge-ready-7-3.tmp");
    std::fs::write(&exact, b"orphan").expect("exact tmp");
    std::fs::write(paths.readiness_path(), dead_forge_receipt()).expect("stale receipt");
    assert_eq!(
        reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
            .expect("stale reconciles"),
        ReadinessReconcile::CleanedStale { pid: u32::MAX }
    );
    assert!(!paths.readiness_path().exists(), "stale receipt removed");
    assert!(exact.exists(), "exact publish temporary is preserved");
    cleanup(&dev_dir);
}

#[test]
fn held_custody_refuses_and_preserves_the_receipt() {
    let (dev_dir, paths) = readiness_home("reconcile-custody");
    std::fs::write(paths.readiness_path(), dead_forge_receipt()).expect("stale receipt");
    std::fs::write(paths.custody_path(), b"custody carrier").expect("custody file");

    // A live Forge holds the home's custody lock from startup until after
    // shutdown. Holding it here simulates that live owner: even though the
    // receipt's pid is dead, the home must not be touched.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(paths.custody_path())
        .expect("custody opens");
    fs2::FileExt::try_lock_exclusive(&held).expect("test holds custody");
    let error = reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
        .expect_err("custody refuses");
    assert!(
        matches!(error, native_dev::DevError::CustodyHeld { .. }),
        "unexpected: {error}"
    );
    assert!(
        paths.readiness_path().exists(),
        "receipt preserved while custody is held"
    );
    drop(held);

    // With custody released, the same stale receipt reconciles normally.
    assert_eq!(
        reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
            .expect("stale reconciles"),
        ReadinessReconcile::CleanedStale { pid: u32::MAX }
    );
    assert!(!paths.readiness_path().exists());
    cleanup(&dev_dir);
}

#[test]
fn missing_custody_shape_fails_closed() {
    let (dev_dir, paths) = readiness_home("reconcile-no-custody");
    std::fs::write(paths.readiness_path(), dead_forge_receipt()).expect("stale receipt");
    // A receipt with no custody directory is an unexpected shape: a Forge
    // can only have run here if custody existed, so fail closed instead of
    // inventing custody to justify removal.
    std::fs::remove_dir_all(paths.custody_path().parent().expect("custody parent"))
        .expect("custody dir removed");
    let error = reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
        .expect_err("missing refused");
    assert!(error.to_string().contains("custody"), "unexpected: {error}");
    assert!(
        paths.readiness_path().exists(),
        "receipt preserved on unexpected custody shape"
    );
    cleanup(&dev_dir);
}

/// A symlinked ancestor must refuse removal: deleting through it could
/// operate outside the dev home. Unix-only: Windows reparse points cannot
/// be fabricated without privileges, and the same walker covers both.
#[cfg(unix)]
#[test]
fn symlinked_parent_refuses_and_preserves() {
    use std::os::unix::fs::symlink;

    let outer = scratch_dev_dir("reconcile-symlink");
    let real = outer.join("real").join("Artisan Street Dev");
    std::fs::create_dir_all(real.join("readiness")).expect("real readiness");
    std::fs::create_dir_all(real.join("custody")).expect("real custody");
    let linked = outer.join("linked");
    symlink(outer.join("real"), &linked).expect("ancestor symlink");
    // Every owned path now resolves through the symlinked ancestor.
    let paths = DevPaths::new(&linked.join("Artisan Street Dev")).expect("absolute root");
    std::fs::write(paths.readiness_path(), dead_forge_receipt()).expect("stale receipt");
    let error = reconcile_stale_readiness(&paths, &staged_forge(&version_root(&paths)))
        .expect_err("symlink refused");
    assert!(
        error.to_string().contains("symbolic link"),
        "unexpected: {error}"
    );
    assert!(
        paths.readiness_path().exists(),
        "receipt preserved behind a symlinked parent"
    );
    cleanup(&outer);
}
