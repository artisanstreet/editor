//! `ae account enroll/status/confirm --json` against a fake Core: the real
//! binary, an isolated HOME/XDG, and canned enrollment endpoints. Asserts
//! the actual stdout shapes, the pending → claimed → enrolled transitions,
//! and that stdout never carries key material.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const ENROLLMENT_ID: &str = "enr_test00000000000000000001";
const CODE: &str = "TESTCODE-ABCDEFGH";
const MACHINE_ID: &str = "mch_test00000000000000000001";
const ACCOUNT_ID: &str = "acct_test00000000000000000000";
const EMAIL: &str = "op@example.invalid";

fn ae() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ae"))
}

/// Isolated per-command environment on every OS: HOME/USERPROFILE plus the
/// platform data roots, all under temp and set only on the child (never
/// global). Also drops a Forge binary placeholder where `wsl::forge_target`
/// looks, so Windows runs stay local instead of querying real WSL, and no
/// real user account file is ever read.
fn isolated(command: &mut Command, home: &std::path::Path) -> std::io::Result<()> {
    let localappdata = home.join("localappdata");
    command
        .env("HOME", home.join("home"))
        .env("USERPROFILE", home.join("home"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("LOCALAPPDATA", &localappdata);
    let (root, folder, binary) = if cfg!(windows) {
        (
            localappdata.join("Artisan Street"),
            "Forge",
            "forge.exe",
        )
    } else if cfg!(target_os = "macos") {
        (
            home.join("home/Library/Application Support/Artisan Street"),
            "Forge",
            "forge",
        )
    } else {
        (home.join("data/artisan-street"), "forge", "forge")
    };
    let binary = root.join(folder).join(binary);
    if let Some(parent) = binary.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&binary, [])?;
    Ok(())
}

fn forge_data_dir(home: &std::path::Path) -> std::path::PathBuf {
    if cfg!(windows) {
        home.join("localappdata/Artisan Street/Forge/data")
    } else if cfg!(target_os = "macos") {
        home.join("home/Library/Application Support/Artisan Street/Forge/data")
    } else {
        home.join("data/artisan-street/forge/data")
    }
}

fn account_file(home: &std::path::Path) -> std::path::PathBuf {
    forge_data_dir(home).join("ast_account.json")
}

/// Fake Core: canned enrollment endpoints over blocking std IO (no extra
/// test dependencies). Status flips pending → claimed after the first
/// poll, modelling approval without a session.
struct FakeCore {
    url: String,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeCore {
    fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        let polls = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let polls = Arc::clone(&polls);
            let shutdown = Arc::clone(&shutdown);
            move || {
                if listener.set_nonblocking(true).is_err() {
                    return;
                }
                while !shutdown.load(Ordering::SeqCst) {
                    let Ok((stream, _)) = listener.accept() else {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    };
                    serve(stream, &polls);
                }
            }
        });
        Ok(Self {
            url,
            shutdown,
            thread: Some(thread),
        })
    }
}

impl Drop for FakeCore {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _joined = thread.join();
        }
    }
}

fn serve(stream: std::net::TcpStream, polls: &AtomicUsize) {
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(value) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 0 {
        let mut body = vec![0_u8; content_length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
    }
    let (code, payload) = route(&method, &path, polls);
    let response = format!(
        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let mut stream = reader.into_inner();
    if stream.write_all(response.as_bytes()).is_ok() {
        let _flushed = stream.flush();
    }
}

fn route(method: &str, path: &str, polls: &AtomicUsize) -> (u16, String) {
    let (code, body) = if method == "POST" && path == "/v1/machines/enrollments" {
        (
            201,
            serde_json::json!({
                "enrollment_id": ENROLLMENT_ID,
                "code": CODE,
                "expires_at": "2030-01-01T00:00:00.000Z",
                "key_fingerprint": "test-fingerprint",
            }),
        )
    } else if method == "GET"
        && path.starts_with("/v1/machines/enrollments/")
        && path.ends_with("/status")
    {
        if polls.fetch_add(1, Ordering::SeqCst) == 0 {
            (
                200,
                serde_json::json!({
                    "enrollment_id": ENROLLMENT_ID,
                    "status": "pending",
                    "expires_at": "2030-01-01T00:00:00.000Z",
                }),
            )
        } else {
            (
                200,
                serde_json::json!({
                    "enrollment_id": ENROLLMENT_ID,
                    "status": "claimed",
                    "account_id": ACCOUNT_ID,
                    "email": EMAIL,
                    "display_name": "Op",
                    "expires_at": "2030-01-01T00:00:00.000Z",
                }),
            )
        }
    } else if method == "POST" && path == "/v1/machines/enrollments/confirm" {
        (
            200,
            serde_json::json!({
                "machine_id": MACHINE_ID,
                "account_id": ACCOUNT_ID,
            }),
        )
    } else {
        return (404, serde_json::json!({"error": "not found"}).to_string());
    };
    (code, body.to_string())
}

fn run(home: &std::path::Path, args: &[&str]) -> std::io::Result<Output> {
    let mut command = ae();
    command.args(args);
    isolated(&mut command, home)?;
    command.output()
}

fn stdout_json(output: &Output) -> serde_json::Result<serde_json::Value> {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
}

/// Full lifecycle through the real binary: no-enrollment null shape,
/// enroll code, pending → claimed transitions, guarded confirm, enrolled
/// shape — with stdout secret-free throughout.
#[test]
fn enroll_status_confirm_json_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_owned();
    let fake = FakeCore::start().unwrap();
    let core = fake.url.clone();

    // Fresh machine: null shape, exit 0 (not an error).
    let null = stdout_json(&run(&home, &["account", "status", "--json"]).unwrap()).unwrap();
    assert_eq!(null["enrolled"], false);
    assert!(null["enrollment_id"].is_null());
    assert_eq!(null["status"], "none");

    // Enroll: stdout carries the exact stub code, never key material.
    let enrolled = stdout_json(
        &run(
            &home,
            &[
                "account",
                "--core",
                &core,
                "enroll",
                "--json",
                "--label",
                "test-forge",
            ],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(enrolled["code"], CODE);
    assert_eq!(enrolled["enrollment_id"], ENROLLMENT_ID);
    let file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(account_file(&home)).unwrap()).unwrap();
    let secret = file["secret_b64u"].as_str().unwrap().to_owned();
    let public = file["public_b64u"].as_str().unwrap().to_owned();
    let stdout = serde_json::to_string(&enrolled).unwrap();
    assert!(!stdout.contains(&secret), "stdout must not carry the secret");
    assert!(!stdout.contains(&public), "stdout must not carry the key");

    // First poll: pending; second: claimed (the stub models approval).
    let pending =
        stdout_json(&run(&home, &["account", "--core", &core, "status", "--json"]).unwrap())
            .unwrap();
    assert_eq!(pending["enrolled"], false);
    assert_eq!(pending["enrollment_id"], ENROLLMENT_ID);
    assert_eq!(pending["status"], "pending");
    let claimed =
        stdout_json(&run(&home, &["account", "--core", &core, "status", "--json"]).unwrap())
            .unwrap();
    assert_eq!(claimed["status"], "claimed");
    assert_eq!(claimed["account_id"], ACCOUNT_ID);
    assert_eq!(claimed["email"], EMAIL);

    // Confirm into the expected account, then the enrolled shape reads
    // back the stub's machine binding with no further network.
    let confirmed = run(
        &home,
        &[
            "account",
            "--core",
            &core,
            "confirm",
            "--yes",
            "--account-id",
            ACCOUNT_ID,
        ],
    )
    .unwrap();
    assert!(
        confirmed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&confirmed.stderr)
    );
    let bound = stdout_json(&run(&home, &["account", "status", "--json"]).unwrap()).unwrap();
    assert_eq!(bound["enrolled"], true);
    assert_eq!(bound["machine_id"], MACHINE_ID);
    assert_eq!(bound["account_id"], ACCOUNT_ID);

    // Human output still works alongside the flag.
    let human = run(&home, &["account", "--core", &core, "status"]).unwrap();
    assert!(human.status.success());
    assert!(
        String::from_utf8_lossy(&human.stdout).contains(MACHINE_ID),
        "human status names the machine"
    );
}
