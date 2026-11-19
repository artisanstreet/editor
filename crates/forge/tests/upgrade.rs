#![cfg(test)]
#![cfg(unix)]

use artisan_api::PairRequest;
use forge_http::{Identity, tls, upgrade};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Running {
    child: Option<Child>,
    data: PathBuf,
}
impl Drop for Running {
    fn drop(&mut self) {
        let gateway = upgrade::gateway(&self.data)
            .ok()
            .flatten()
            .map(|gateway| gateway.pid);
        let backends = forge_runs::runtime::Registry::open(&self.data)
            .and_then(|registry| registry.peers())
            .map(|peers| {
                peers
                    .into_iter()
                    .map(|member| member.pid)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let pids: Vec<u32> = gateway
            .into_iter()
            .chain(backends)
            .chain(self.child.as_ref().map(Child::id))
            .collect();
        signal("-TERM", &pids);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while pids.iter().any(|pid| alive(*pid)) && std::time::Instant::now() < deadline {
            if let Some(child) = self.child.as_mut() {
                let _reaped = child.try_wait();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        signal("-KILL", &pids);
        if let Some(child) = self.child.as_mut() {
            let _reaped = child.wait();
        }
    }
}

fn signal(name: &str, pids: &[u32]) {
    let live: Vec<String> = pids
        .iter()
        .filter(|pid| alive(**pid))
        .map(ToString::to_string)
        .collect();
    if !live.is_empty() {
        let _signalled = Command::new("kill").arg(name).args(&live).status();
    }
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn start() -> (tempfile::TempDir, Running, upgrade::Gateway) {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("data");
    let child = Command::new(env!("CARGO_BIN_EXE_forge"))
        .arg("--data")
        .arg(&data)
        .arg("--listen")
        .arg("127.0.0.1:0")
        .arg("--log")
        .arg(directory.path().join("forge.log"))
        .env_remove("NOTIFY_SOCKET")
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut running = Running {
        child: Some(child),
        data,
    };
    let gateway = tokio::time::timeout(test_support::wait::LIVENESS, async {
        loop {
            if let Some(status) = running.child.as_mut().unwrap().try_wait().unwrap() {
                panic!(
                    "Forge exited {status}: {}",
                    std::fs::read_to_string(directory.path().join("forge.log")).unwrap()
                );
            }
            if let Ok(Some(gateway)) = upgrade::gateway(&running.data) {
                break gateway;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    (directory, running, gateway)
}

async fn device(data: &std::path::Path, address: std::net::SocketAddr) -> reqwest::Client {
    let store = forge_store::Store::join(data).unwrap();
    let invite = forge_http::invite(&store).await.unwrap();
    let identity = Identity::generate("upgrade-test").unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(test_support::wait::LIVENESS)
        .use_preconfigured_tls(
            tls::client_config(
                &identity,
                &tls::fingerprint_of(&invite.forge_certificate).unwrap(),
            )
            .unwrap(),
        )
        .build()
        .unwrap();
    let paired = client
        .post(format!("https://localhost:{}/pair", address.port()))
        .json(&PairRequest {
            code: invite.code,
            device_name: "upgrade-test".into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(paired.status(), 200);
    client
}

#[tokio::test(flavor = "multi_thread")]
async fn a_detached_forge_logs_its_backend_to_the_same_file() {
    let (directory, _running, gateway) = start().await;
    let log = std::fs::read_to_string(directory.path().join("forge.log")).unwrap();
    let starts = log
        .lines()
        .filter(|line| line.contains("starting Forge"))
        .count();
    assert_eq!(starts, 2, "the gateway and its backend both log: {log}");
    assert!(
        log.contains(&format!(
            "Forge is serving address={}",
            gateway.backend.address
        )),
        "{log}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn replacement_preserves_the_listening_port_and_serves_requests_during_gateway_handoff() {
    let (directory, mut running, original) = start().await;
    let _idle = std::net::TcpStream::connect(original.address).unwrap();
    let client = device(&running.data, original.address).await;
    let url = format!("https://localhost:{}/state", original.address.port());
    assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    let data = running.data.clone();
    let mut replace = tokio::spawn(async move {
        upgrade::replace(&data, std::path::Path::new(env!("CARGO_BIN_EXE_forge"))).await
    });
    let mut observed = 0;
    let replaced = tokio::time::timeout(test_support::wait::LIVENESS, async {
        loop {
            tokio::select! {
                replaced = &mut replace => break replaced,
                served = client.get(&url).send() => {
                    assert_eq!(served.unwrap().status(), 200);
                    observed += 1;
                }
            }
        }
    })
    .await
    .expect("the replacement finishes in time");
    let replacement = replaced.unwrap().unwrap_or_else(|error| {
        panic!(
            "{}: {}",
            diagnostics::chain(&error),
            std::fs::read_to_string(directory.path().join("forge.log")).unwrap()
        )
    });
    assert_ne!(replacement.pid, original.pid);
    assert_ne!(replacement.backend.id, original.backend.id);
    assert_eq!(replacement.address, original.address);
    assert!(observed > 0);
    for _ in 0..30 {
        assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    }
    let mut original_gateway = running.child.take().unwrap();
    tokio::time::timeout(
        test_support::wait::LIVENESS,
        tokio::task::spawn_blocking(move || original_gateway.wait()),
    )
    .await
    .expect("the replaced gateway exits in time")
    .unwrap()
    .unwrap();
    let final_gateway = upgrade::replace(
        &running.data,
        std::path::Path::new(env!("CARGO_BIN_EXE_forge")),
    )
    .await
    .unwrap();
    assert_ne!(final_gateway.pid, replacement.pid);
    assert_eq!(final_gateway.address, original.address);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    upgrade::stop(&running.data).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_candidate_leaves_the_original_generation_and_connections_serving() {
    let (_directory, running, original) = start().await;
    let client = device(&running.data, original.address).await;
    let url = format!("https://localhost:{}/state", original.address.port());
    upgrade::replace(&running.data, std::path::Path::new("/bin/false"))
        .await
        .expect_err("a candidate that exits at once is rejected");
    let current = upgrade::gateway(&running.data).unwrap().unwrap();
    assert_eq!(current.id, original.id);
    assert_eq!(current.backend.id, original.backend.id);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    upgrade::stop(&running.data).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_the_gateway_cannot_hand_to_its_backend_is_logged_with_both_ids() {
    let (directory, running, gateway) = start().await;
    let active = running.data.join("runtime").join("active.json");
    let mut unreachable: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&active).unwrap()).unwrap();
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    unreachable["id"] = serde_json::json!("unreachable-generation");
    unreachable["address"] = serde_json::json!(closed.local_addr().unwrap());
    drop(closed);
    upgrade::write(&active, &unreachable).unwrap();
    let _client = std::net::TcpStream::connect(gateway.address).unwrap();
    let logged = tokio::time::timeout(test_support::wait::LIVENESS, async {
        loop {
            let log = std::fs::read_to_string(directory.path().join("forge.log")).unwrap();
            if let Some(line) = log
                .lines()
                .find(|line| line.contains("unreachable-generation"))
            {
                break line.to_owned();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(logged.contains(&gateway.id), "{logged}");
    tokio::time::timeout(test_support::wait::LIVENESS, async {
        loop {
            let restored = forge_runs::runtime::read::<forge_runs::runtime::Member>(&active)
                .unwrap()
                .is_some_and(|member| member.id != "unreachable-generation");
            if restored {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backend_that_dies_is_replaced_before_anyone_connects() {
    let (directory, running, gateway) = start().await;
    signal("-KILL", &[gateway.backend.pid]);
    let restarted = tokio::time::timeout(test_support::wait::LIVENESS, async {
        loop {
            if let Some(current) = upgrade::gateway(&running.data)
                .unwrap()
                .filter(|current| current.backend.id != gateway.backend.id)
            {
                break current.backend;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "no generation replaced the killed backend: {}",
            std::fs::read_to_string(directory.path().join("forge.log")).unwrap()
        )
    });
    assert_ne!(restarted.pid, gateway.backend.pid);
    let serving = upgrade::health(&running.data).await.unwrap();
    assert_eq!(serving.backend.id, restarted.id);
    upgrade::stop(&running.data).await.unwrap();
}

#[test]
fn a_start_that_upgrades_the_database_keeps_asking_systemd_for_more_time() {
    use std::io::Read as _;
    const RELEASED_DATABASE: &[u8] =
        include_bytes!("../../forge-store/tests/fixtures/released/schema-26.sqlite3.gz");
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let mut released = Vec::new();
    flate2::read::GzDecoder::new(RELEASED_DATABASE)
        .read_to_end(&mut released)
        .unwrap();
    std::fs::write(data.join("forge.sqlite3"), released).unwrap();
    let socket = directory.path().join("notify.sock");
    let manager = std::os::unix::net::UnixDatagram::bind(&socket).unwrap();
    manager
        .set_read_timeout(Some(test_support::wait::LIVENESS))
        .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_forge"))
        .arg("--data")
        .arg(&data)
        .arg("--listen")
        .arg("127.0.0.1:0")
        .arg("--log")
        .arg(directory.path().join("forge.log"))
        .env("NOTIFY_SOCKET", &socket)
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _running = Running {
        child: Some(child),
        data,
    };

    let mut message = [0_u8; 512];
    let received = manager.recv(&mut message).unwrap_or_else(|error| {
        panic!(
            "Forge said nothing to systemd while upgrading: {error}: {}",
            std::fs::read_to_string(directory.path().join("forge.log")).unwrap()
        )
    });

    let said = std::str::from_utf8(&message[..received]).unwrap();
    let extension: u64 = said
        .lines()
        .find_map(|line| line.strip_prefix("EXTEND_TIMEOUT_USEC="))
        .unwrap_or_else(|| panic!("the first word to systemd asks for more time: {said:?}"))
        .parse()
        .unwrap();
    assert!(
        extension >= 60_000_000,
        "each heartbeat buys at least a minute: {said:?}"
    );
}
