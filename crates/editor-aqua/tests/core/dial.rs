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
    std::fs::write(files.dir.join("account.json"), account.to_string())
        .expect("the fixture file is written");
    save_onboarding_for_tests(files, CORE_URL, ACCOUNT, MACHINE)
        .expect("the onboarding fixture is saved");
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
    std::fs::write(files.dir.join("hosts.json"), hosts.to_string())
        .expect("the fixture file is written");
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
            match blocked.lock().unwrap().recv() {
                Ok(()) | Err(mpsc::RecvError) => {}
            }
            Err(editor_aqua::core::ae::AeError::Failed {
                command: "account status".to_owned(),
                detail: "released".to_owned(),
            })
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

#[cfg(unix)]
async fn pair_server() -> (String, String, Arc<std::sync::atomic::AtomicUsize>) {
    use artisan_api::tls::Identity;
    use base64::Engine as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test listener binds");
    let address = listener
        .local_addr()
        .expect("the test listener has an address")
        .to_string();
    let identity = Identity::generate("pairing-forge").expect("a test identity generates");
    let certificate = identity
        .certificate_der()
        .expect("the test identity encodes its certificate");
    let encoded = base64::engine::general_purpose::STANDARD.encode(certificate.as_ref());
    let config = rustls::ServerConfig::builder_with_provider(artisan_api::tls::provider())
        .with_safe_default_protocol_versions()
        .expect("the test TLS config accepts the default protocol versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate],
            identity
                .key_der()
                .expect("the test identity encodes its key"),
        )
        .expect("the test TLS config accepts the identity");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let pairings = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&pairings);
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener
                .accept()
                .await
                .expect("the test listener accepts a connection");
            let Ok(mut socket) = acceptor.accept(socket).await else {
                continue;
            };
            let mut request = Vec::new();
            let mut byte = [0; 1];
            while !request.ends_with(b"\r\n\r\n")
                && socket.read(&mut byte).await.expect("the test socket reads") == 1
            {
                request.push(byte[0]);
            }
            if request.starts_with(b"POST /pair ") {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            let body = br#"{"device_id":"device-1"}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket
                .write_all(head.as_bytes())
                .await
                .expect("the test Forge writes its reply");
            socket
                .write_all(body)
                .await
                .expect("the test Forge writes its reply");
            socket
                .shutdown()
                .await
                .expect("the test Forge writes its reply");
        }
    });
    (address, encoded, pairings)
}

#[cfg(unix)]
#[test]
fn a_pairing_that_cannot_be_saved_is_not_left_connected_and_can_be_retried() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    write_files(&files);
    std::fs::remove_file(files.dir.join("hosts.json")).unwrap();
    let (address, certificate, pairings) =
        editor_aqua::core::runtime::runtime().block_on(pair_server());
    let invite = serde_json::json!({
        "code": "pair-code",
        "forge_certificate": certificate,
        "address": address,
    })
    .to_string();
    let status = serde_json::json!({
        "enrolled": true,
        "machine_id": MACHINE,
        "account_id": ACCOUNT,
    })
    .to_string();
    let services = Services::scripted(
        CORE_URL,
        Arc::new(move |args| match args {
            ["account", ..] => Ok(status.clone()),
            ["pair", "--json"] => Ok(invite.clone()),
            _ => Err(editor_aqua::core::ae::AeError::Failed {
                command: args.join(" "),
                detail: "unexpected ae call".to_owned(),
            }),
        }),
        None,
    );
    let mut core = Core::with_services(files.clone(), services).unwrap();
    std::fs::set_permissions(&files.dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let id = HostId(MACHINE.into());
    let deadline = Instant::now() + Duration::from_secs(20);
    while core
        .account()
        .last_error()
        .is_none_or(|error| !error.contains("save the pairing"))
    {
        assert!(
            Instant::now() < deadline,
            "the pairing never reported that it could not be saved"
        );
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
    assert_eq!(pairings.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(core.host(&id).is_none());

    std::fs::set_permissions(&files.dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    core.connect_machine(MACHINE);
    while core.host(&id).is_none() {
        assert!(
            Instant::now() < deadline,
            "retrying the pairing did not connect this machine's Forge"
        );
        core.drain_with(|_, _| {});
        std::thread::yield_now();
    }
    assert_eq!(pairings.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        files
            .hosts()
            .unwrap()
            .hosts
            .iter()
            .any(|host| host.id == id)
    );
}
