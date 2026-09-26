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
    BinarySet, DevPaths, EditorOutput, StartupWait, editor_library_path, editor_log_path,
    editor_output, read_receipt, spawn_editor, staged_editor, staged_forge, wait_for_startup,
    windows_command_line,
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
    std::fs::write(
        &editor,
        "#!/bin/sh\necho editor started \"$@\"\nexec sleep 5\n",
    )
    .expect("stand-in");
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let log = dir.join("editor.log");
    let receipt = dir.join("startup-receipt.json");
    let mut process = spawn_editor(
        &editor,
        &dir,
        &receipt,
        &EditorOutput::Detached { log: log.clone() },
        &["--host-home".into(), "/tmp/host home".into()],
    )
    .expect("detached spawn");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !std::fs::read_to_string(&log)
        .is_ok_and(|text| text.contains("editor started --host-home /tmp/host home"))
    {
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
        &[],
    )
    .expect("detached spawn");
    assert_ne!(process.pid(), 0);
    let outcome = wait_for_startup(process.child_mut(), &receipt, Duration::from_secs(20));
    assert_eq!(outcome, StartupWait::EditorExited { code: Some(0) });
    process.release();
    cleanup(&dir);
}

#[test]
fn the_linux_editor_loads_recorded_graphics_libraries_then_host_drivers() {
    let exists = |directory: &str| directory == "/usr/lib/wsl/lib";
    assert_eq!(
        editor_library_path(
            Some("/nix/store/a/lib:/nix/store/b/lib"),
            Some("/opt/lib"),
            exists
        ),
        Some("/nix/store/a/lib:/nix/store/b/lib:/opt/lib:/usr/lib/wsl/lib".to_owned())
    );
    assert_eq!(
        editor_library_path(Some("/nix/store/a/lib"), None, |_| false),
        Some("/nix/store/a/lib".to_owned())
    );
    assert_eq!(editor_library_path(None, Some("/opt/lib"), exists), None);
}

#[test]
fn windows_command_lines_quote_like_the_c_runtime() {
    let line = |arguments: &[&str]| {
        windows_command_line(
            &arguments
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        line(&["--host-home", r"C:\Users\Ada Lovelace\hosts\abc-def"]),
        r#"--host-home "C:\Users\Ada Lovelace\hosts\abc-def""#
    );
    assert_eq!(line(&[r"C:\trailing dir\"]), r#""C:\trailing dir\\""#);
    assert_eq!(line(&[r#"say "hi""#]), r#""say \"hi\"""#);
    assert_eq!(line(&[""]), r#""""#);
    assert_eq!(line(&[]), "");
}
