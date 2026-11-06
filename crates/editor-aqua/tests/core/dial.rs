use editor_aqua::core::{
    Core, EditorFiles, HostConfig, HostId, HostKind, Services, save_onboarding_for_tests,
};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const ACCOUNT: &str = "acct_dial";
const MACHINE: &str = "mch_dial";
const CORE_URL: &str = "http://127.0.0.1:1";

fn write_files(files: &EditorFiles) {
    let profile = serde_json::json!({
        "account_id": ACCOUNT,
        "email": "dial@example.invalid",
        "display_name": "Dial",
        "totp_enrolled": false,
        "machines": [{
            "id": MACHINE,
            "label": "Local Forge",
            "public_key": {"kty": "OKP", "crv": "Ed25519", "x": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},
            "created_at": "2026-01-01T00:00:00.000Z"
        }]
    });
    let account = serde_json::json!({
        "base_url": CORE_URL,
        "session_token": "dial-session",
        "profile": profile,
    });
    std::fs::write(files.dir.join("account.json"), account.to_string()).unwrap();
    save_onboarding_for_tests(files, CORE_URL, ACCOUNT, MACHINE).unwrap();
    let hosts = serde_json::json!({
        "hosts": [HostConfig {
            id: HostId(MACHINE.into()),
            name: "Local Forge".into(),
            address: "127.0.0.1:1".into(),
            fingerprint: "0".repeat(64),
            kind: HostKind::Local,
        }],
        "last": MACHINE,
    });
    std::fs::write(files.dir.join("hosts.json"), hosts.to_string()).unwrap();
}

#[test]
fn saved_local_forge_is_dialed_while_the_binding_read_is_pending() {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    write_files(&files);
    let (release, blocked) = mpsc::channel::<()>();
    let blocked = Mutex::new(blocked);
    let services = Services::scripted(
        CORE_URL,
        Arc::new(move |_| {
            let _ = blocked.lock().unwrap().recv();
            anyhow::bail!("released")
        }),
        None,
    );
    let mut core = Core::with_services(files, services).unwrap();
    let id = HostId(MACHINE.into());
    let deadline = Instant::now() + Duration::from_secs(10);
    while core.host(&id).is_none() {
        assert!(
            Instant::now() < deadline,
            "the saved local Forge was not dialed while `ae account status` was pending"
        );
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
    assert!(
        core.account().reading_local_forge(),
        "the binding read should still be pending"
    );
    drop(release);
}
