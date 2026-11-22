use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn turn_servers(fixture: &Fixture, seen: &Mutex<Vec<String>>) -> Vec<String> {
    seen.lock().unwrap().clear();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("go"))
        .await
        .unwrap();
    fixture.settled().await;
    let mut names = seen.lock().unwrap().clone();
    names.retain(|name| ["up", "down", "local"].contains(&name.as_str()));
    names.sort();
    names
}

async fn recording_fixture() -> (Fixture, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let fixture = fixture(Arc::new(move |turn, _| {
        recorded.lock().unwrap().extend(
            turn.launch
                .mcp_servers
                .iter()
                .map(|server| server.name.clone()),
        );
        Box::pin(async { Outcome::done() })
    }))
    .await;
    (fixture, seen)
}

fn counting(listener: tokio::net::TcpListener) -> Arc<AtomicUsize> {
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&connections);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    connections
}

async fn configure(
    fixture: &Fixture,
    up: std::net::SocketAddr,
    down: std::net::SocketAddr,
    local: &str,
) {
    fixture
        .hub
        .store()
        .set_setting(
            "mcp_servers".to_owned(),
            serde_json::json!([
                { "name": "up", "url": format!("http://{up}/mcp") },
                { "name": "down", "url": format!("http://{down}/mcp") },
                { "name": "local", "command": local },
            ]),
        )
        .await
        .unwrap();
}

async fn vacated() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    address
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cold_turn_starts_without_servers_that_do_not_answer() {
    let (fixture, seen) = recording_fixture().await;
    let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    configure(
        &fixture,
        up.local_addr().unwrap(),
        vacated().await,
        "server",
    )
    .await;
    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["local", "up"],
        "a server nobody listens on is left out; command servers always start"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_trusts_a_fresh_check_and_a_config_change_checks_again() {
    let (fixture, seen) = recording_fixture().await;
    tokio::spawn(fixture.hub.mcp().watch_server_changes());
    let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_address = up.local_addr().unwrap();
    let up_connections = counting(up);
    let down = vacated().await;
    configure(&fixture, up_address, down, "server").await;
    test_support::wait::eventually(
        "the changed MCP servers to be checked and the check to reach the server",
        test_support::wait::LIVENESS,
        || async {
            fixture.hub.mcp().reachability().probed(2) && up_connections.load(Ordering::SeqCst) > 0
        },
    )
    .await;
    let checked = up_connections.load(Ordering::SeqCst);

    let down_connections = counting(tokio::net::TcpListener::bind(down).await.unwrap());
    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["local", "up"],
        "a fresh check that found the server silent stands for the turn"
    );
    assert_eq!(
        (
            up_connections.load(Ordering::SeqCst),
            down_connections.load(Ordering::SeqCst)
        ),
        (checked, 0),
        "a turn with a fresh check waits on no server"
    );

    configure(&fixture, up_address, down, "other-server").await;
    test_support::wait::eventually(
        "a config change to check the servers again",
        test_support::wait::LIVENESS,
        || async {
            down_connections.load(Ordering::SeqCst) > 0
                && fixture.hub.mcp().reachability().probed(2)
        },
    )
    .await;
    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["down", "local", "up"],
        "a server that answers again is back once the servers are checked again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn servers_configured_before_the_watcher_first_runs_are_still_checked() {
    let (fixture, _seen) = recording_fixture().await;
    let watching = fixture.hub.mcp().watch_server_changes();
    let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_address = up.local_addr().unwrap();
    let up_connections = counting(up);
    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_address = other.local_addr().unwrap();
    let _other_connections = counting(other);
    configure(&fixture, up_address, other_address, "server").await;
    tokio::spawn(watching);
    test_support::wait::eventually(
        "the watcher to check the servers configured before it first ran",
        test_support::wait::LIVENESS,
        || async {
            fixture.hub.mcp().reachability().probed(2) && up_connections.load(Ordering::SeqCst) > 0
        },
    )
    .await;
}
