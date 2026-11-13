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
    child: Child,
    data: PathBuf,
}
impl Drop for Running {
    fn drop(&mut self) {
        let gateway = upgrade::gateway(&self.data).ok().flatten().map(|gateway| gateway.pid);
        let backends = forge_runs::runtime::Registry::open(&self.data)
            .and_then(|registry| registry.peers())
            .map(|peers| peers.into_iter().map(|member| member.pid).collect::<Vec<_>>())
            .unwrap_or_default();
        let pids: Vec<u32> = gateway
            .into_iter()
            .chain(backends)
            .chain([self.child.id()])
            .collect();
        signal("-TERM", &pids);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while pids.iter().any(|pid| alive(*pid)) && std::time::Instant::now() < deadline {
            self.child.try_wait().ok();
            std::thread::sleep(Duration::from_millis(50));
        }
        signal("-KILL", &pids);
        self.child.wait().ok();
    }
}

fn signal(name: &str, pids: &[u32]) {
    let live: Vec<String> = pids
        .iter()
        .filter(|pid| alive(**pid))
        .map(ToString::to_string)
        .collect();
    if !live.is_empty() {
        Command::new("kill").arg(name).args(&live).status().ok();
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
    let log = std::fs::File::create(directory.path().join("forge.log")).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_forge"))
        .arg("--data")
        .arg(&data)
        .arg("--listen")
        .arg("127.0.0.1:0")
        .env_remove("NOTIFY_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .unwrap();
    let mut running = Running { child, data };
    let gateway = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(status) = running.child.try_wait().unwrap() {
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
        .timeout(Duration::from_secs(10))
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
async fn replacement_preserves_the_listening_port_and_serves_requests_during_gateway_handoff() {
    let (directory, mut running, original) = start().await;
    let _idle = std::net::TcpStream::connect(original.address).unwrap();
    let client = device(&running.data, original.address).await;
    let url = format!("https://localhost:{}/state", original.address.port());
    assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    let data = running.data.clone();
    let replace = tokio::spawn(async move {
        upgrade::replace(&data, std::path::Path::new(env!("CARGO_BIN_EXE_forge"))).await
    });
    let mut observed = 0;
    while !replace.is_finished() {
        assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
        observed += 1;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let replacement = replace.await.unwrap().unwrap_or_else(|error| {
        panic!(
            "{error:#}: {}",
            std::fs::read_to_string(directory.path().join("forge.log")).unwrap()
        )
    });
    assert_ne!(replacement.pid, original.pid);
    assert_ne!(replacement.backend.id, original.backend.id);
    assert_eq!(replacement.address, original.address);
    assert!(observed > 0);
    for _ in 0..30 {
        assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::timeout(Duration::from_secs(15), async {
        while running.child.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
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
    assert!(
        upgrade::replace(&running.data, std::path::Path::new("/bin/false"))
            .await
            .is_err()
    );
    let current = upgrade::gateway(&running.data).unwrap().unwrap();
    assert_eq!(current.id, original.id);
    assert_eq!(current.backend.id, original.backend.id);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
    upgrade::stop(&running.data).await.unwrap();
}
