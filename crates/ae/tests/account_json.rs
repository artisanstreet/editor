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

fn isolated(command: &mut Command, home: &std::path::Path) -> std::io::Result<()> {
    let localappdata = home.join("localappdata");
    command
        .env("HOME", home.join("home"))
        .env("USERPROFILE", home.join("home"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("LOCALAPPDATA", &localappdata);
    let (root, folder, binary) = if cfg!(windows) {
        (localappdata.join("Artisan Street"), "Forge", "forge.exe")
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

struct FakeCore {
    url: String,
    address: std::net::SocketAddr,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeCore {
    fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let url = format!("http://{address}");
        let polls = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let polls = Arc::clone(&polls);
            let shutdown = Arc::clone(&shutdown);
            move || {
                for accepted in listener.incoming() {
                    if shutdown.load(Ordering::SeqCst) {
                        break;
                    }
                    match accepted {
                        Ok(stream) => serve(stream, &polls),
                        Err(_unaccepted) => break,
                    }
                }
            }
        });
        Ok(Self {
            url,
            address,
            shutdown,
            thread: Some(thread),
        })
    }
}

impl Drop for FakeCore {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        drop(std::net::TcpStream::connect(self.address));
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
    } else if method == "GET" && path == "/v1/config/bundles/latest" {
        (200, serde_json::json!({ "bundle": null }))
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

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one account enrollment lifecycle is exercised against a shared server"
)]
fn enroll_status_confirm_json_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_owned();
    let fake = FakeCore::start().unwrap();
    let core = fake.url.clone();

    let null = stdout_json(&run(&home, &["account", "status", "--json"]).unwrap()).unwrap();
    assert_eq!(null["enrolled"], false);
    assert!(null["enrollment_id"].is_null());
    assert_eq!(null["status"], "none");

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
    assert!(
        !stdout.contains(&secret),
        "stdout must not carry the secret"
    );
    assert!(!stdout.contains(&public), "stdout must not carry the key");

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

    let human = run(&home, &["account", "--core", &core, "status"]).unwrap();
    assert!(human.status.success());
    assert!(
        String::from_utf8_lossy(&human.stdout).contains(MACHINE_ID),
        "human status names the machine"
    );

    let refused = run(&home, &["account", "--core", &core, "enroll", "--json"]).unwrap();
    assert!(!refused.status.success(), "replacement must be explicit");
    let replaced = stdout_json(
        &run(
            &home,
            &["account", "--core", &core, "enroll", "--json", "--replace"],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(replaced["code"], CODE);
    let replacement: serde_json::Value =
        serde_json::from_slice(&std::fs::read(account_file(&home)).unwrap()).unwrap();
    assert_ne!(
        replacement["secret_b64u"], secret,
        "revoked keys cannot be reused"
    );
    assert_ne!(
        replacement["public_b64u"], public,
        "the machine key must rotate"
    );
    assert!(
        replacement.get("machine_id").is_none(),
        "old binding must clear"
    );
    assert!(
        replacement.get("account_id").is_none(),
        "old account must clear"
    );
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
        "replacement must confirm: {}",
        String::from_utf8_lossy(&confirmed.stderr)
    );
}

#[test]
fn syncing_with_forge_stopped_reaches_core_instead_of_crashing() {
    use base64::Engine as _;
    let home = tempfile::tempdir().unwrap();
    let core = FakeCore::start().unwrap();
    let key = artisan_account::MachineKeypair::generate().unwrap();
    let account = serde_json::json!({
        "secret_b64u": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.secret()),
        "public_b64u": key.public_b64u(),
        "machine_id": MACHINE_ID,
        "account_id": ACCOUNT_ID,
    });
    let path = account_file(home.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, account.to_string()).unwrap();

    let output = run(home.path(), &["account", "--core", &core.url, "sync"]).unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("Core has no config bundle yet"),
        "stderr: {stderr}"
    );
}
