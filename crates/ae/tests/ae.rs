#[cfg(unix)]
use artisan_api::PairRequest;
use artisan_paths::App;
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
#[cfg(unix)]
use forge_http::{Identity, ServeConfig, tls};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::{BufRead as _, Write as _};
use std::path::Path;
use std::process::{Command, Output};
use test_support::sandbox::{artisan_street, isolate};

fn ae() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ae"))
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn pair_json_beside_a_running_forge_pairs_an_editor() {
    let dir = tempfile::tempdir().unwrap();
    let data = App::Forge
        .dir_under(&artisan_street(dir.path()))
        .join("data");
    let server = forge_http::start(ServeConfig {
        data_dir: data.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
    })
    .await
    .unwrap();
    let address = server.local_addr().unwrap();
    std::fs::write(
        data.join("config.json"),
        format!(r#"{{"listen": "{address}"}}"#),
    )
    .unwrap();
    tokio::spawn(server.run());

    let home = dir.path().to_owned();
    let output = tokio::task::spawn_blocking(move || {
        let mut command = ae();
        command.args(["pair", "--json"]);
        isolate(&mut command, &home);
        command.output().unwrap()
    })
    .await
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let invite: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(invite["address"], address.to_string());
    let fingerprint = invite["forge_fingerprint"].as_str().unwrap();
    assert_eq!(
        fingerprint,
        tls::fingerprint_of(invite["forge_certificate"].as_str().unwrap()).unwrap()
    );

    let identity = Identity::generate("artisan-editor").unwrap();
    let http = reqwest::Client::builder()
        .use_preconfigured_tls(tls::client_config(&identity, fingerprint).unwrap())
        .build()
        .unwrap();
    let base = format!("https://localhost:{}", address.port());
    let pair = PairRequest {
        code: invite["code"].as_str().unwrap().to_owned(),
        device_name: "laptop".into(),
    };
    let paired = http
        .post(format!("{base}/pair"))
        .json(&pair)
        .send()
        .await
        .unwrap();
    assert_eq!(paired.status(), 200, "the code from `ae pair` pairs");
    let state = http.get(format!("{base}/state")).send().await.unwrap();
    assert_eq!(state.status(), 200, "the paired device is let in");
}

#[cfg(unix)]
#[test]
fn pairing_while_forge_is_stopped_leaves_an_older_database_for_forge_to_upgrade() {
    use std::io::Read as _;
    let dir = tempfile::tempdir().unwrap();
    let data = App::Forge
        .dir_under(&artisan_street(dir.path()))
        .join("data");
    std::fs::create_dir_all(&data).unwrap();
    let database = data.join("forge.sqlite3");
    let mut released = Vec::new();
    flate2::read::GzDecoder::new(
        &include_bytes!("../../forge-store/tests/fixtures/released/schema-26.sqlite3.gz")[..],
    )
    .read_to_end(&mut released)
    .unwrap();
    std::fs::write(&database, released).unwrap();

    let mut command = ae();
    command.args(["pair", "--json"]);
    isolate(&mut command, dir.path());
    let output = command.output().unwrap();

    assert!(!output.status.success(), "ae refuses an older database");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("schema 26"), "{stderr}");
    let schema: usize = rusqlite::Connection::open(&database)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(schema, 26, "only Forge upgrades its database");
    assert!(
        !data.join("forge.sqlite3.schema-26").exists(),
        "ae leaves no backup of an upgrade it didn't do"
    );
}

const fn carrier_app() -> &'static str {
    if cfg!(windows) { "editor" } else { "ae" }
}

fn carrier_name() -> String {
    format!(
        "{}-{}-{}{}",
        carrier_app(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::EXE_SUFFIX
    )
}

#[cfg(unix)]
fn carrier() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_ae"))
}

#[cfg(windows)]
fn carrier() -> std::path::PathBuf {
    let carrier = Path::new(env!("CARGO_BIN_EXE_ae"))
        .with_file_name("examples")
        .join("editor_carrier.exe");
    assert!(
        carrier.is_file(),
        "{} is missing; build it first (`cargo build -p ae --examples`)",
        carrier.display()
    );
    carrier
}

#[expect(
    clippy::unwrap_used,
    reason = "a test fixture; a failure fails the test"
)]
fn publish(dir: &Path, key: &SigningKey, version: &str) -> Vec<u8> {
    std::fs::create_dir_all(dir).unwrap();
    let mut carrier = std::fs::read(carrier()).unwrap();
    carrier.extend_from_slice(format!("release {version}").as_bytes());
    let sha256 = hex(&Sha256::digest(&carrier));
    std::fs::write(dir.join(carrier_name()), &carrier).unwrap();
    let manifest = serde_json::json!({
        "version": version,
        "artifacts": [{"app": carrier_app(), "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "file": carrier_name(), "sha256": sha256}],
    })
    .to_string();
    std::fs::write(dir.join("release.json"), &manifest).unwrap();
    let signature =
        base64::engine::general_purpose::STANDARD.encode(key.sign(manifest.as_bytes()).to_bytes());
    std::fs::write(dir.join("release.json.sig"), signature).unwrap();
    carrier
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, byte| {
        write!(hex, "{byte:02x}").expect("invariant: writing to a String cannot fail");
        hex
    })
}

struct Served {
    address: std::net::SocketAddr,
    stopping: std::sync::Arc<std::sync::atomic::AtomicBool>,
    server: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        drop(std::net::TcpStream::connect_timeout(
            &self.address,
            test_support::wait::LIVENESS,
        ));
        let stopped = self.server.take().map(std::thread::JoinHandle::join);
        if !std::thread::panicking() {
            stopped
                .expect("invariant: the feed is served until it is dropped")
                .expect("the feed served only release files");
        }
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "a test fixture; a failure fails the test"
)]
fn serve(directory: std::path::PathBuf) -> (String, Served) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let feed = format!("http://{address}/nightly/release.json");
    let stopping = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = std::sync::Arc::clone(&stopping);
    let server = std::thread::spawn(move || {
        for stream in listener.incoming() {
            if stop.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let mut stream = stream.unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header == "\r\n" || header.is_empty() {
                    break;
                }
            }
            let mut parts = request.split_whitespace();
            let method = parts.next().unwrap();
            let path = parts.next().unwrap().strip_prefix("/nightly/").unwrap();
            assert!(!path.contains('/'), "only release files are served");
            let body = std::fs::read(directory.join(path)).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            if method != "HEAD" {
                stream.write_all(&body).unwrap();
            }
        }
    });
    (
        feed,
        Served {
            address,
            stopping,
            server: Some(server),
        },
    )
}

#[expect(
    clippy::unwrap_used,
    reason = "a test fixture; a failure fails the test"
)]
fn install(home: &Path, from: &Path, key: &SigningKey) -> Output {
    let mut command = ae();
    command
        .args([
            "install", "--forge", "none", "--editor", "none", "--yes", "--from",
        ])
        .arg(from)
        .env(
            "ARTISAN_RELEASE_PUBLIC_KEY_HEX",
            hex(&key.verifying_key().to_bytes()),
        );
    isolate(&mut command, home);
    command.output().unwrap()
}

#[cfg(unix)]
#[test]
fn a_release_installs_through_its_own_ae() {
    let home = tempfile::tempdir().unwrap();
    let release = home.path().join("release");
    let app = App::Ae.dir_under(&artisan_street(home.path()));
    let key = SigningKey::from_bytes(&[7; 32]);
    let installed = || std::fs::read(app.join("ae")).unwrap_or_default();

    for version in ["1.0.0", "1.1.0"] {
        let newest = publish(&release, &key, version);
        let output = install(home.path(), &release, &key);
        assert!(
            output.status.success(),
            "installing {version}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            installed() == newest,
            "the installed ae is the one {version} published"
        );
        assert_eq!(
            std::fs::read_to_string(app.join("version")).unwrap().trim(),
            env!("CARGO_PKG_VERSION"),
            "the recorded version is the installer's own"
        );
    }
    let notices = std::fs::read_to_string(app.join("NOTICES.txt")).unwrap();
    for section in ["LICENSE.md", "notices/ae.txt", "notices/forge.txt"] {
        assert!(
            notices.contains(&format!("==== {section} ====")),
            "the installed ae carries the {section} notices"
        );
    }
    assert!(notices.contains("Copyright (c) Tokio Contributors"));
    let entries: Vec<_> = std::fs::read_dir(&app)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        !entries.iter().any(|name| name.starts_with(".incoming")),
        "nothing is left staged: {entries:?}"
    );
    assert_eq!(
        std::fs::read_link(home.path().join("home/.local/bin/ae")).unwrap(),
        app.join("ae"),
        "`ae` on PATH is the installed one"
    );
}

#[test]
fn a_tampered_installer_or_a_release_signed_by_someone_else_installs_nothing() {
    let home = tempfile::tempdir().unwrap();
    let release = home.path().join("release");
    let app = App::Ae.dir_under(&artisan_street(home.path()));
    let key = SigningKey::from_bytes(&[7; 32]);

    publish(&release, &key, "1.2.0");
    std::fs::write(release.join(carrier_name()), b"not the signed installer").unwrap();
    let output = install(home.path(), &release, &key);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tampered"), "{stderr}");
    assert!(
        !app.join(App::Ae.binary_name()).exists(),
        "a refused download installs nothing"
    );

    publish(&release, &SigningKey::from_bytes(&[8; 32]), "1.3.0");
    let output = install(home.path(), &release, &key);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not signed by Artisan's release key"),
        "{stderr}"
    );
    assert!(
        !app.join(App::Ae.binary_name()).exists(),
        "a release signed by someone else installs nothing"
    );
}

#[test]
#[cfg_attr(
    windows,
    ignore = "writes the Windows user's registry (PATH, uninstall entry); CI runs it on a throwaway runner"
)]
fn a_local_build_installs_with_its_version() {
    let home = tempfile::tempdir().unwrap();
    let build = home.path().join("stage");
    let os = build.join(std::env::consts::OS);
    std::fs::create_dir_all(&os).unwrap();
    std::fs::write(
        os.join(format!("ae{}", std::env::consts::EXE_SUFFIX)),
        "ae dev",
    )
    .unwrap();
    std::fs::write(os.join("VERSION"), "0.5.0-dev+abc1234\n").unwrap();
    let output = install(home.path(), &build, &SigningKey::from_bytes(&[7; 32]));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let app = App::Ae.dir_under(&artisan_street(home.path()));
    assert_eq!(
        std::fs::read_to_string(app.join(App::Ae.binary_name())).unwrap(),
        "ae dev"
    );
    assert_eq!(
        std::fs::read_to_string(app.join("version")).unwrap().trim(),
        "0.5.0-dev+abc1234"
    );
}

#[test]
#[cfg_attr(
    windows,
    ignore = "writes the Windows user's registry (PATH, uninstall entry); CI runs it on a throwaway runner"
)]
fn uninstalling_ae_itself_names_the_kept_data_instead_of_a_command_it_removed() {
    let home = tempfile::tempdir().unwrap();
    let build = home.path().join("stage");
    let os = build.join(std::env::consts::OS);
    std::fs::create_dir_all(&os).unwrap();
    std::fs::write(os.join(App::Ae.binary_name()), "ae dev").unwrap();
    std::fs::write(os.join("VERSION"), "0.5.0-dev+abc1234\n").unwrap();
    let installed = install(home.path(), &build, &SigningKey::from_bytes(&[7; 32]));
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let data = App::Ae.dir_under(&artisan_street(home.path())).join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("kept"), "kept").unwrap();

    let mut command = ae();
    command.args(["uninstall", "--yes"]);
    isolate(&mut command, home.path());
    let output = command.output().unwrap();

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{said}");
    assert!(data.join("kept").exists(), "the data is kept: {said}");
    assert!(
        said.contains(&data.display().to_string()),
        "the kept folder is named: {said}"
    );
    assert!(
        !said.contains("ae uninstall --purge"),
        "no advice to run the ae that was just removed: {said}"
    );
}

#[test]
fn a_failed_install_says_why_not_only_what_failed() {
    let home = tempfile::tempdir().unwrap();
    let release = home.path().join("release");
    std::fs::create_dir_all(&release).unwrap();
    std::fs::write(release.join("release.json"), "not json").unwrap();
    let output = install(home.path(), &release, &SigningKey::from_bytes(&[7; 32]));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.lines().last(),
        Some(
            format!(
                "error: read release.json from {}: expected ident at line 1 column 2",
                release.display()
            )
            .as_str()
        ),
        "{stderr}"
    );
}

#[test]
#[cfg_attr(
    windows,
    ignore = "writes the Windows user's registry (PATH, uninstall entry); CI runs it on a throwaway runner"
)]
fn the_installed_ae_replaces_itself_with_a_newer_build_while_it_runs() {
    let home = tempfile::tempdir().unwrap();
    let stage = |name: &str, version: &str| {
        let build = home.path().join(name);
        let os = build.join(std::env::consts::OS);
        std::fs::create_dir_all(&os).unwrap();
        let mut ae = std::fs::read(env!("CARGO_BIN_EXE_ae")).unwrap();
        ae.extend_from_slice(format!("build {version}").as_bytes());
        std::fs::write(os.join(App::Ae.binary_name()), &ae).unwrap();
        std::fs::write(os.join("VERSION"), format!("{version}\n")).unwrap();
        (build, ae)
    };
    let (first, _) = stage("first", "0.5.0-dev+aaaaaaa");
    let output = install(home.path(), &first, &SigningKey::from_bytes(&[7; 32]));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let app = App::Ae.dir_under(&artisan_street(home.path()));
    let installed = app.join(App::Ae.binary_name());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (second, newer) = stage("second", "0.6.0-dev+bbbbbbb");

    let mut command = Command::new(&installed);
    command.args([
        "install", "--forge", "none", "--editor", "none", "--yes", "--from",
    ]);
    command.arg(&second);
    isolate(&mut command, home.path());
    let output = command.output().unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        std::fs::read(&installed).unwrap() == newer,
        "the running ae was replaced by the newer build"
    );
    assert_eq!(
        std::fs::read_to_string(app.join("version")).unwrap().trim(),
        "0.6.0-dev+bbbbbbb"
    );
}

#[cfg(unix)]
#[path = "ae/release-feed.rs"]
mod release_feed;

#[cfg(windows)]
#[path = "ae/windows-feed.rs"]
mod windows_feed;
