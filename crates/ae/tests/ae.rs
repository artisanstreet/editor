//! The real `ae` binary, the way the Editor and users run it.

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

/// The Editor pairs with a local Forge by running `ae forge pair --json`
/// while that Forge is serving; the printed code must pair it.
#[tokio::test(flavor = "multi_thread")]
async fn pair_json_beside_a_running_forge_pairs_an_editor() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("forge");
    let server = forge_http::start(ServeConfig {
        data_dir: data.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
    })
    .await
    .unwrap();
    let address = server.local_addr().unwrap();
    tokio::spawn(server.run());

    let output = tokio::task::spawn_blocking(move || {
        ae().args([
            "forge",
            "pair",
            "--json",
            "--listen",
            &address.to_string(),
            "--data-dir",
        ])
        .arg(&data)
        .output()
        .unwrap()
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
    assert_eq!(paired.status(), 200, "the code from `ae forge pair` pairs");
    let state = http.get(format!("{base}/state")).send().await.unwrap();
    assert_eq!(state.status(), 200, "the paired device is let in");
}

/// Writes a signed release `version` into `dir`, holding an `ae` whose
/// contents name the version.
fn publish(dir: &Path, key: &SigningKey, version: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let file = format!("artisan-{version}.tar.gz");
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let body = format!("ae {version}");
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(
            &mut header,
            format!("ae{}", std::env::consts::EXE_SUFFIX),
            body.as_bytes(),
        )
        .unwrap();
    let archive = builder.into_inner().unwrap().finish().unwrap();
    let sha256 = hex(&Sha256::digest(&archive));
    std::fs::write(dir.join(&file), &archive).unwrap();
    let manifest = serde_json::json!({
        "version": version,
        "artifacts": [{"os": std::env::consts::OS, "arch": std::env::consts::ARCH, "file": file, "sha256": sha256}],
    })
    .to_string();
    std::fs::write(dir.join("release.json"), &manifest).unwrap();
    let signature =
        base64::engine::general_purpose::STANDARD.encode(key.sign(manifest.as_bytes()).to_bytes());
    std::fs::write(dir.join("release.json.sig"), signature).unwrap();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// Installs into a temp root; everything outside it is only printed.
fn install(home: &Path, release: &Path, key: &SigningKey) -> Output {
    let key_hex = hex(&key.verifying_key().to_bytes());
    ae().args(["--dry-run", "install", "forge", "--from"])
        .arg(release)
        .arg("--data-dir")
        .arg(home.join("forge"))
        .env("ARTISAN_HOME", home.join("root"))
        .env("ARTISAN_RELEASE_PUBLIC_KEY_HEX", key_hex)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .output()
        .unwrap()
}

#[test]
fn installing_releases_swaps_current_and_keeps_one_previous() {
    let home = tempfile::tempdir().unwrap();
    let release = home.path().join("release");
    let root = home.path().join("root");
    let key = SigningKey::from_bytes(&[7; 32]);
    let read = |name: &str| {
        std::fs::read_to_string(root.join(name))
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let stable_ae = root
        .join("bin")
        .join(format!("ae{}", std::env::consts::EXE_SUFFIX));

    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        publish(&release, &key, version);
        let output = install(home.path(), &release, &key);
        assert!(
            output.status.success(),
            "installing {version}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(read("current"), "1.2.0");
    assert_eq!(read("previous"), "1.1.0");
    let mut kept: Vec<_> = std::fs::read_dir(root.join("versions"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    kept.sort();
    assert_eq!(
        kept,
        ["1.1.0", "1.2.0"],
        "only current and previous are kept"
    );
    assert_eq!(
        std::fs::read_to_string(&stable_ae).unwrap(),
        "ae 1.2.0",
        "the ae on PATH is the new one"
    );

    // A download that doesn't match the signed manifest is refused and
    // leaves the install as it was.
    publish(&release, &key, "1.3.0");
    std::fs::write(
        release.join("artisan-1.3.0.tar.gz"),
        b"not the signed archive",
    )
    .unwrap();
    let output = install(home.path(), &release, &key);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("tampered"));
    assert_eq!(read("current"), "1.2.0");
    assert_eq!(std::fs::read_to_string(&stable_ae).unwrap(), "ae 1.2.0");

    // So is a release signed by someone else.
    publish(&release, &SigningKey::from_bytes(&[8; 32]), "1.4.0");
    let output = install(home.path(), &release, &key);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not signed by Artisan's release key")
    );
    assert_eq!(read("current"), "1.2.0");
}
