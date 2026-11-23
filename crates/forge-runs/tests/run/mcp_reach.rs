use super::*;
use crate::mcp_auth::ReadMcpServersError;
use crate::mcp_reach::support::{Gate, scripted};
use crate::mcp_reach::{FRESH, MARGIN, PATIENCE, REFRESH, Reachability};
use artisan_api::McpServer;
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

fn watching(fixture: &Fixture) -> impl Future<Output = ()> + Send + 'static {
    fixture.hub.mcp().watch_server_changes()
}

struct Counted {
    connections: Arc<AtomicUsize>,
    accepting: tokio::task::JoinHandle<()>,
}

fn counting(listener: tokio::net::TcpListener) -> Counted {
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&connections);
    let accepting = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    Counted {
        connections,
        accepting,
    }
}

impl Counted {
    fn count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    async fn close(self) {
        self.accepting.abort();
        assert!(
            self.accepting.await.unwrap_err().is_cancelled(),
            "the listener stops by being cancelled"
        );
    }
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
async fn the_first_turn_leaves_out_a_server_nobody_listens_on() {
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
        "a turn waits for a server's first check, so a refused server never reaches the engine; command servers always start"
    );
}

#[cfg(target_os = "linux")]
struct Unanswered {
    _listener: tokio::net::TcpListener,
    _queued: Vec<std::net::TcpStream>,
    address: std::net::SocketAddr,
}

#[cfg(target_os = "linux")]
fn unanswered() -> Unanswered {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(0).unwrap();
    let address = listener.local_addr().unwrap();
    let mut queued = Vec::new();
    loop {
        assert!(
            queued.len() < 16,
            "the listener's accept queue never filled up"
        );
        match std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)) {
            Ok(stream) => queued.push(stream),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
            Err(error) => panic!("fill the listener's accept queue: {error}"),
        }
    }
    Unanswered {
        _listener: listener,
        _queued: queued,
        address,
    }
}

#[cfg(target_os = "linux")]
type Launches = mpsc::UnboundedReceiver<Vec<String>>;

#[cfg(target_os = "linux")]
async fn launching_fixture() -> (Fixture, Launches) {
    let (report, launches) = mpsc::unbounded_channel();
    let fixture = fixture(Arc::new(move |turn, _| {
        report
            .send(
                turn.launch
                    .mcp_servers
                    .iter()
                    .map(|server| server.name.clone())
                    .collect(),
            )
            .unwrap();
        Box::pin(async { Outcome::done() })
    }))
    .await;
    (fixture, launches)
}

#[cfg(target_os = "linux")]
async fn launch(fixture: &Fixture, launches: &mut Launches) -> (Vec<String>, Duration) {
    let started = std::time::Instant::now();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("go"))
        .await
        .unwrap();
    let mut names = tokio::time::timeout(test_support::wait::LIVENESS, launches.recv())
        .await
        .expect("the turn launched within the liveness bound")
        .expect("the harness reports every launch");
    let waited = started.elapsed();
    names.retain(|name| ["up", "down", "local"].contains(&name.as_str()));
    names.sort();
    (names, waited)
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_on_a_fresh_forge_waits_only_the_patience_for_a_hanging_server_and_never_again() {
    let (fixture, mut launches) = launching_fixture().await;
    let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hanging = unanswered();
    configure(
        &fixture,
        up.local_addr().unwrap(),
        hanging.address,
        "server",
    )
    .await;

    let (first, waited) = launch(&fixture, &mut launches).await;
    fixture.settled().await;
    assert_eq!(
        first,
        ["local", "up"],
        "a turn resumed before any check ran waits for the first check and leaves out the hanging server"
    );
    assert!(
        waited < PATIENCE + MARGIN,
        "the first check is bounded by its patience, not by the engine's 30 s MCP wait: waited {waited:?}"
    );
    assert!(
        fixture.hub.mcp().reachability().probed(2),
        "the turn launched only after both first checks finished"
    );

    fixture.hub.mcp().reachability().age(FRESH);
    let (stale, waited) = launch(&fixture, &mut launches).await;
    assert!(
        waited < PATIENCE,
        "a turn on an old check does not wait for the new one: waited {waited:?}"
    );
    assert!(
        fixture.hub.mcp().reachability().reprobing(hanging.address),
        "the turn on an old check launched while the hanging server was still being checked again"
    );
    fixture.settled().await;
    assert_eq!(
        stale,
        ["local", "up"],
        "a turn on an old check starts on that check rather than waiting for the new one"
    );
}

fn servers(remote: &[(&str, std::net::SocketAddr)]) -> Vec<McpServer> {
    let mut configured: Vec<serde_json::Value> = remote
        .iter()
        .map(|(name, address)| serde_json::json!({ "name": name, "url": format!("http://{address}/mcp") }))
        .collect();
    configured.push(serde_json::json!({ "name": "local", "command": "server" }));
    serde_json::from_value(serde_json::Value::Array(configured)).unwrap()
}

fn start_turn(
    reachability: &Arc<Reachability>,
    servers: &[McpServer],
) -> tokio::task::JoinHandle<(Vec<String>, Duration)> {
    let (reachability, servers) = (Arc::clone(reachability), servers.to_vec());
    tokio::spawn(async move {
        let started = tokio::time::Instant::now();
        let thread = ThreadId::new();
        let checks = reachability.join_first_checks(&servers, &thread).await;
        let offered = names(&reachability.answering(&servers, &checks, &thread));
        (offered, started.elapsed())
    })
}

fn names(servers: &[McpServer]) -> Vec<String> {
    let mut names: Vec<String> = servers.iter().map(|server| server.name.clone()).collect();
    names.sort();
    names
}

async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..10_000 {
        if done() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("{what} never happened");
}

async fn until_turns_wait(gate: &Gate, turns: usize) {
    until("the turns to join the check in flight", || {
        gate.turns_waiting() >= turns
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn turns_started_during_a_first_check_share_it_and_leave_out_a_silent_server() {
    let reachability = Arc::new(Reachability::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let connections = counting(listener);
    let gate = reachability.first_probe_in_flight(address);
    let configured = servers(&[("down", address)]);

    let first = start_turn(&reachability, &configured);
    let second = start_turn(&reachability, &configured);
    until_turns_wait(&gate, 2).await;
    gate.open(&reachability, false);

    for turn in [first, second] {
        let (offered, waited) = turn.await.unwrap();
        assert_eq!(
            offered,
            ["local"],
            "the shared first check found the server silent"
        );
        assert_eq!(
            waited,
            Duration::ZERO,
            "the turns launch as soon as the check reports"
        );
    }
    assert_eq!(
        connections.count(),
        0,
        "turns join the check in flight instead of each connecting to the server"
    );
}

#[tokio::test(start_paused = true)]
async fn turns_launch_without_a_server_at_the_deadline_when_its_first_check_never_reports() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    let _gate = reachability.first_probe_in_flight(address);
    let configured = servers(&[("down", address)]);

    let first = start_turn(&reachability, &configured);
    let second = start_turn(&reachability, &configured);
    for turn in [first, second] {
        let (offered, waited) = turn.await.unwrap();
        assert_eq!(
            offered,
            ["local"],
            "a server whose first check never reported is left out rather than risking the engine's 30 s MCP wait"
        );
        assert_eq!(
            waited,
            PATIENCE + MARGIN,
            "a turn waits for a stuck check no longer than the check's own patience plus a margin"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_first_check_whose_task_died_is_started_again_by_the_next_turn() {
    let reachability = Arc::new(Reachability::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let connections = counting(listener);
    drop(reachability.first_probe_in_flight(address));

    let (offered, _) = start_turn(&reachability, &servers(&[("up", address)]))
        .await
        .unwrap();
    assert_eq!(offered, ["local", "up"]);
    assert_eq!(
        reachability.answers(address),
        Some(true),
        "the turn checked the server again rather than waiting on a check nobody runs"
    );
    test_support::wait::eventually(
        "the server to see the check's connection",
        test_support::wait::LIVENESS,
        || async { connections.count() > 0 },
    )
    .await;
    assert_eq!(connections.count(), 1, "the server was checked once");
}

#[tokio::test(start_paused = true)]
async fn a_server_that_goes_silent_while_a_turn_waits_on_another_is_left_out() {
    let reachability = Arc::new(Reachability::default());
    let (early, late) = (vacated().await, vacated().await);
    reachability
        .first_probe_in_flight(early)
        .open(&reachability, true);
    let gate = reachability.first_probe_in_flight(late);

    let turn = start_turn(&reachability, &servers(&[("down", early), ("up", late)]));
    until_turns_wait(&gate, 1).await;
    reachability
        .reprobe_in_flight(early)
        .open(&reachability, false);
    gate.open(&reachability, true);

    let (offered, _) = turn.await.unwrap();
    assert_eq!(
        offered,
        ["local", "up"],
        "the turn filters on the checks as they stand when it launches, not when it began waiting"
    );
}

#[tokio::test(start_paused = true)]
async fn a_late_result_from_a_check_that_was_replaced_is_ignored() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    let replaced = reachability.first_probe_in_flight(address);
    reachability.forget(address);
    let current = reachability.first_probe_in_flight(address);

    current.open(&reachability, false);
    replaced.open(&reachability, true);

    assert_eq!(
        reachability.answers(address),
        Some(false),
        "only the check that owns the server's entry decides its result"
    );
    let (offered, _) = start_turn(&reachability, &servers(&[("down", address)]))
        .await
        .unwrap();
    assert_eq!(offered, ["local"]);
}

#[tokio::test(start_paused = true)]
async fn a_repeat_check_whose_task_died_is_claimed_again() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    reachability
        .first_probe_in_flight(address)
        .open(&reachability, true);
    assert!(!reachability.due(address), "a fresh check is not repeated");
    drop(reachability.reprobe_in_flight(address));
    assert!(
        reachability.due(address),
        "a repeat check nobody runs any more is started again"
    );
}

#[tokio::test(start_paused = true)]
async fn a_changed_result_is_checked_again_sooner_than_a_steady_one() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    reachability
        .first_probe_in_flight(address)
        .open(&reachability, true);
    tokio::time::advance(REFRESH.saturating_sub(Duration::from_millis(1))).await;
    assert!(!reachability.due(address));
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(
        reachability.due(address),
        "a new result is checked again after the short interval"
    );

    reachability
        .reprobe_in_flight(address)
        .open(&reachability, true);
    tokio::time::advance(REFRESH).await;
    assert!(
        !reachability.due(address),
        "a result that held is not checked again after the short interval"
    );
    tokio::time::advance(FRESH.saturating_sub(REFRESH)).await;
    assert!(
        reachability.due(address),
        "a result that held is checked again after the long interval"
    );

    reachability
        .reprobe_in_flight(address)
        .open(&reachability, false);
    tokio::time::advance(REFRESH).await;
    assert!(
        reachability.due(address),
        "a result that flipped is back on the short interval"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_removed_from_the_config_is_no_longer_checked() {
    let (fixture, _seen) = recording_fixture().await;
    tokio::spawn(watching(&fixture));
    let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_address = up.local_addr().unwrap();
    let _up = counting(up);
    configure(&fixture, up_address, vacated().await, "server").await;
    test_support::wait::eventually(
        "both configured servers to be checked",
        test_support::wait::LIVENESS,
        || async { fixture.hub.mcp().reachability().probed(2) },
    )
    .await;

    fixture
        .hub
        .store()
        .set_setting(
            "mcp_servers".to_owned(),
            serde_json::json!([{ "name": "up", "url": format!("http://{up_address}/mcp") }]),
        )
        .await
        .unwrap();
    test_support::wait::eventually(
        "the removed server's check to be dropped",
        test_support::wait::LIVENESS,
        || async { fixture.hub.mcp().reachability().checked_endpoints() == 1 },
    )
    .await;
    assert_eq!(
        fixture.hub.mcp().reachability().answers(up_address),
        Some(true),
        "the server still configured keeps its check"
    );
}

#[tokio::test(start_paused = true)]
async fn a_check_that_reports_after_the_deadline_stays_out_of_that_turn() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    let gate = reachability.first_probe_in_flight(address);
    let configured = servers(&[("up", address)]);
    let thread = ThreadId::new();

    let checks = reachability.join_first_checks(&configured, &thread).await;
    gate.open(&reachability, true);
    assert_eq!(
        names(&reachability.answering(&configured, &checks, &thread)),
        ["local"],
        "a server whose check missed the turn's deadline stays out of that turn even if it answers before launch"
    );
    let next = reachability.join_first_checks(&configured, &thread).await;
    assert_eq!(
        names(&reachability.answering(&configured, &next, &thread)),
        ["local", "up"],
        "the next turn uses the result that arrived"
    );
}

#[tokio::test(start_paused = true)]
async fn an_old_server_list_cannot_bring_back_a_removed_server() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    reachability
        .first_probe_in_flight(address)
        .open(&reachability, true);
    reachability.configure(&servers(&[]));

    let (offered, _) = start_turn(&reachability, &servers(&[("up", address)]))
        .await
        .unwrap();
    assert_eq!(offered, ["local"]);
    assert_eq!(
        reachability.checked_endpoints(),
        0,
        "a turn reading the list from before the removal does not start checking the server again"
    );
}

fn undecodable() -> Result<Vec<McpServer>, ReadMcpServersError> {
    Err(ReadMcpServersError::Decode(
        serde_json::from_str::<Vec<McpServer>>("{").unwrap_err(),
    ))
}

#[tokio::test(start_paused = true)]
async fn a_failed_read_of_the_server_list_is_retried_after_a_pause() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    reachability
        .first_probe_in_flight(address)
        .open(&reachability, true);
    reachability.age(FRESH);
    let (servers, script) = scripted([undecodable(), undecodable(), Ok(Vec::new())]);
    let watcher = tokio::spawn(Arc::clone(&reachability).watch(servers));

    until("the watcher's first read", || script.reads() == 1).await;
    tokio::time::advance(REFRESH.saturating_sub(Duration::from_millis(1))).await;
    assert_eq!(
        script.reads(),
        1,
        "a failed read waits before reading again, even with a server overdue"
    );
    tokio::time::advance(Duration::from_millis(1)).await;
    until("the watcher's second read", || script.reads() == 2).await;
    tokio::time::advance(REFRESH).await;
    until("the watcher's third read", || script.reads() == 3).await;
    assert_eq!(
        reachability.checked_endpoints(),
        0,
        "the first list that reads is applied"
    );
    tokio::time::advance(FRESH).await;
    assert_eq!(
        script.reads(),
        3,
        "with nothing left to check the watcher stops reading"
    );

    drop(script);
    tokio::time::timeout(PATIENCE, watcher)
        .await
        .expect("the watcher stops when its change feed closes")
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_repeat_check_that_dies_while_idle_is_started_again() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    reachability
        .first_probe_in_flight(address)
        .open(&reachability, true);
    let configured = servers(&[("up", address)]);
    let (servers, script) = scripted([Ok(configured.clone()), Ok(configured)]);
    let _watcher = tokio::spawn(Arc::clone(&reachability).watch(servers));
    until("the watcher's first read", || script.reads() == 1).await;

    drop(reachability.reprobe_in_flight(address));
    until("the watcher to notice the abandoned check", || {
        script.reads() == 2
    })
    .await;
    assert!(
        !reachability.due(address),
        "the abandoned check was started again without waiting for the server to come due"
    );
}

#[tokio::test(start_paused = true)]
async fn a_turn_trusts_fresh_checks_and_starts_none_of_its_own() {
    let reachability = Arc::new(Reachability::default());
    let (up, down) = (vacated().await, vacated().await);
    reachability
        .first_probe_in_flight(up)
        .open(&reachability, true);
    reachability
        .first_probe_in_flight(down)
        .open(&reachability, false);

    let (offered, waited) = start_turn(&reachability, &servers(&[("up", up), ("down", down)]))
        .await
        .unwrap();
    assert_eq!(
        offered,
        ["local", "up"],
        "a fresh check that found a server silent stands for the turn"
    );
    assert_eq!(
        waited,
        Duration::ZERO,
        "a turn with fresh checks waits on no server"
    );
    assert!(
        !reachability.reprobing(up) && !reachability.reprobing(down),
        "a turn with fresh checks starts no check of its own"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_config_change_checks_the_servers_again() {
    let (fixture, seen) = recording_fixture().await;
    tokio::spawn(watching(&fixture));
    let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_address = up.local_addr().unwrap();
    let _up = counting(up);
    let down = vacated().await;
    configure(&fixture, up_address, down, "server").await;
    test_support::wait::eventually(
        "the configured MCP servers to be checked",
        test_support::wait::LIVENESS,
        || async { fixture.hub.mcp().reachability().answers(down) == Some(false) },
    )
    .await;

    let down_listener = counting(tokio::net::TcpListener::bind(down).await.unwrap());
    configure(&fixture, up_address, down, "other-server").await;
    test_support::wait::eventually(
        "a config change to check the servers again",
        test_support::wait::LIVENESS,
        || async {
            down_listener.count() > 0
                && fixture.hub.mcp().reachability().answers(down) == Some(true)
        },
    )
    .await;
    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["down", "local", "up"],
        "a server that answers again is back once the servers are checked again"
    );
}

#[tokio::test(start_paused = true)]
async fn a_first_check_that_reports_after_the_deadline_is_late_even_if_the_turn_sees_it_at_once() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    let gate = reachability.first_probe_in_flight(address);
    let configured = servers(&[("up", address)]);
    let deadline = tokio::time::Instant::now() + PATIENCE + MARGIN;

    let turn = {
        let (reachability, configured) = (Arc::clone(&reachability), configured.clone());
        tokio::spawn(async move {
            let thread = ThreadId::new();
            let checks = reachability.join_first_checks(&configured, &thread).await;
            names(&reachability.answering(&configured, &checks, &thread))
        })
    };
    until_turns_wait(&gate, 1).await;
    gate.publish_at(&reachability, true, deadline + Duration::from_millis(1));

    assert_eq!(
        turn.await.unwrap(),
        ["local"],
        "a result stamped after the turn's deadline is late for that turn, however soon the turn sees it"
    );
}

#[tokio::test(start_paused = true)]
async fn a_failed_first_read_of_the_server_list_is_retried_with_nothing_checked_yet() {
    let reachability = Arc::new(Reachability::default());
    let address = vacated().await;
    let (servers, script) = scripted([undecodable(), Ok(servers(&[("up", address)]))]);
    let _watcher = tokio::spawn(Arc::clone(&reachability).watch(servers));

    until("the watcher's first read", || script.reads() == 1).await;
    tokio::time::advance(REFRESH).await;
    until("the watcher to read the list again", || script.reads() == 2).await;
    assert_eq!(
        reachability.checked_endpoints(),
        1,
        "once the list reads, its server is checked"
    );
}

#[tokio::test(start_paused = true)]
async fn a_change_to_the_server_list_is_read_fresh_rather_than_replayed() {
    let reachability = Arc::new(Reachability::default());
    let (kept, dropped) = (vacated().await, vacated().await);
    let (servers, script) = scripted([
        Ok(servers(&[("up", kept), ("down", dropped)])),
        Ok(servers(&[("up", kept)])),
    ]);
    let _watcher = tokio::spawn(Arc::clone(&reachability).watch(servers));
    until("the watcher's first read", || script.reads() == 1).await;
    until("both servers to be checked", || {
        reachability.checked_endpoints() == 2
    })
    .await;

    script.edit();
    until("the watcher to read the changed list", || {
        script.reads() == 2
    })
    .await;
    assert_eq!(
        reachability.checked_endpoints(),
        1,
        "the watcher applies the list as it stands now"
    );
    assert!(reachability.answers(dropped).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_added_just_before_a_turn_is_checked_and_offered() {
    let reachability = Arc::new(Reachability::default());
    reachability.configure(&servers(&[]));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let _listener = counting(listener);

    let (offered, _) = start_turn(&reachability, &servers(&[("up", address)]))
        .await
        .unwrap();
    assert_eq!(
        offered,
        ["local", "up"],
        "a server the watcher has not heard about yet is checked by the turn, not left out"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn servers_configured_before_the_watcher_first_runs_are_still_checked() {
    let (fixture, _seen) = recording_fixture().await;
    let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up_address = up.local_addr().unwrap();
    let up = counting(up);
    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_address = other.local_addr().unwrap();
    let _other = counting(other);
    configure(&fixture, up_address, other_address, "server").await;
    tokio::spawn(watching(&fixture));
    test_support::wait::eventually(
        "the watcher to check the servers configured before it existed",
        test_support::wait::LIVENESS,
        || async { fixture.hub.mcp().reachability().probed(2) && up.count() > 0 },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_stops_or_returns_while_idle_is_noticed_without_a_config_change() {
    let (fixture, seen) = recording_fixture().await;
    let up = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let down = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let down_address = down.local_addr().unwrap();
    let down = counting(down);
    configure(&fixture, up.local_addr().unwrap(), down_address, "server").await;
    tokio::spawn(watching(&fixture));
    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["down", "local", "up"],
        "both servers answer at first"
    );

    down.close().await;
    fixture.hub.mcp().reachability().age(FRESH);
    test_support::wait::eventually(
        "the periodic check to notice the server stopped",
        test_support::wait::LIVENESS,
        || async { fixture.hub.mcp().reachability().answers(down_address) == Some(false) },
    )
    .await;
    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["local", "up"],
        "a server that stopped while Forge sat idle is left out of the next turn"
    );

    let _down = counting(tokio::net::TcpListener::bind(down_address).await.unwrap());
    fixture.hub.mcp().reachability().age(FRESH);
    test_support::wait::eventually(
        "the periodic check to notice the server is back",
        test_support::wait::LIVENESS,
        || async { fixture.hub.mcp().reachability().answers(down_address) == Some(true) },
    )
    .await;
    assert_eq!(
        turn_servers(&fixture, &seen).await,
        ["down", "local", "up"],
        "a server that came back while Forge sat idle is offered again"
    );
}
