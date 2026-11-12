#![cfg(target_os = "linux")]

use artisan_api::PairRequest;
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use forge_http::{Identity, ServeConfig, tls};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, Output};

fn ae() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ae"))
}

/// An isolated Artisan Street: HOME and the XDG folders in a temp dir.
fn isolated(command: &mut Command, home: &Path) {
    command
        .env("HOME", home.join("home"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_CONFIG_HOME", home.join("config"));
}

/// The Editor pairs with a local Forge by running `ae pair --json` while
/// that Forge is serving; the printed code must pair it.
#[tokio::test(flavor = "multi_thread")]
async fn pair_json_beside_a_running_forge_pairs_an_editor() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data/artisan-street/forge/data");
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
        isolated(&mut command, &home);
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

fn carrier_name() -> String {
    format!("ae-{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[expect(
    clippy::unwrap_used,
    reason = "a test fixture; a failure fails the test"
)]
fn publish(dir: &Path, key: &SigningKey, version: &str) -> Vec<u8> {
    std::fs::create_dir_all(dir).unwrap();
    let mut carrier = std::fs::read(env!("CARGO_BIN_EXE_ae")).unwrap();
    carrier.extend_from_slice(format!("release {version}").as_bytes());
    let sha256 = hex(&Sha256::digest(&carrier));
    std::fs::write(dir.join(carrier_name()), &carrier).unwrap();
    let manifest = serde_json::json!({
        "version": version,
        "artifacts": [{"app": "ae", "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "file": carrier_name(), "sha256": sha256}],
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
        write!(hex, "{byte:02x}").ok();
        hex
    })
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
    isolated(&mut command, home);
    command.output().unwrap()
}

#[test]
fn a_release_installs_through_its_own_ae_and_bad_ones_are_refused() {
    let home = tempfile::tempdir().unwrap();
    let release = home.path().join("release");
    let app = home.path().join("data/artisan-street/ae");
    let key = SigningKey::from_bytes(&[7; 32]);
    let installed = || std::fs::read(app.join("ae")).unwrap_or_default();

    let mut newest = Vec::new();
    for version in ["1.0.0", "1.1.0"] {
        newest = publish(&release, &key, version);
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

    publish(&release, &key, "1.2.0");
    std::fs::write(release.join(carrier_name()), b"not the signed installer").unwrap();
    let output = install(home.path(), &release, &key);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tampered"));
    assert!(installed() == newest, "a refused download changes nothing");

    publish(&release, &SigningKey::from_bytes(&[8; 32]), "1.3.0");
    let output = install(home.path(), &release, &key);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not signed by Artisan's release key")
    );
    assert!(
        installed() == newest,
        "a release signed by someone else changes nothing"
    );
}

#[test]
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
    let app = home.path().join("data/artisan-street/ae");
    assert_eq!(std::fs::read_to_string(app.join("ae")).unwrap(), "ae dev");
    assert_eq!(
        std::fs::read_to_string(app.join("version")).unwrap().trim(),
        "0.5.0-dev+abc1234"
    );
}

#[path = "ae/release-feed.rs"]
mod release_feed;
