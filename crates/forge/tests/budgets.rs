#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const IDLE_FOR: Duration = Duration::from_secs(600);
const STARTUP_YIELDS: usize = 256;

async fn recorded(data: &Path) {
    tokio::time::timeout(Duration::from_secs(60), async {
        while forge_http::upgrade::gateway(data)
            .expect("the gateway record reads")
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the gateway records itself within a minute");
}

async fn idle(wakeups: &AtomicU64, stop: tokio::sync::oneshot::Sender<()>) -> u64 {
    for _ in 0..STARTUP_YIELDS {
        tokio::task::yield_now().await;
    }
    let before = wakeups.load(Ordering::Relaxed);
    test_support::clock::paused(tokio::time::sleep(IDLE_FOR)).await;
    let woke = wakeups.load(Ordering::Relaxed) - before;
    stop.send(()).expect("the gateway is still serving");
    woke
}

async fn generations_stopped(data: &Path) {
    let registry = forge_runs::runtime::Registry::open(data).expect("the runtime registry opens");
    tokio::time::timeout(Duration::from_secs(30), async {
        while !registry.peers().expect("the generations list").is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the gateway's generation stops within thirty seconds");
}

#[test]
fn an_idle_gateway_sleeps_until_its_generation_or_an_upgrade_needs_it() {
    assert!(
        std::env::var_os("NOTIFY_SOCKET").is_none(),
        "the gateway would report readiness to the service manager running this test"
    );
    let budgets: serde_json::Value = serde_json::from_str(include_str!("budgets/gateway.json"))
        .expect("the budget file is valid JSON");
    let budget = &budgets["idle_wakeups_per_ten_minutes"];
    let limit = budget["limit"].as_u64().expect("the budget has a limit");
    let reason = budget["reason"].as_str().expect("the budget has a reason");
    let wakeups = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&wakeups);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .on_thread_unpark(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        })
        .build()
        .expect("the test runtime starts");
    let directory = tempfile::tempdir().expect("a Forge data folder");
    let data = directory.path().join("data");
    let woke = runtime.block_on(async {
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serve = forge_http::upgrade::serve(
            forge_http::ServeConfig {
                data_dir: data.clone(),
                listen: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
            },
            PathBuf::from(env!("CARGO_BIN_EXE_forge")),
            None,
            diagnostics::Destination::Stderr,
            async {
                stopped
                    .await
                    .expect("the test sends stop before dropping its sender");
            },
        );
        tokio::pin!(serve);
        tokio::select! {
            served = &mut serve => panic!(
                "the gateway stopped before recording itself: {}",
                served.map_or_else(|error| diagnostics::chain(&error), |()| "no error".to_owned())
            ),
            () = recorded(&data) => {}
        }
        let (served, woke) = tokio::join!(serve, idle(&wakeups, stop));
        if let Err(error) = served {
            panic!("the gateway stops cleanly: {}", diagnostics::chain(&error));
        }
        generations_stopped(&data).await;
        woke
    });
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write as _;
        let mut summary = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .expect("the job summary opens for appending");
        writeln!(
            summary,
            "| Budget | Measured | Limit |\n| --- | ---: | ---: |\n| `gateway idle_wakeups_per_ten_minutes` | {woke} | {limit} |"
        )
        .expect("the job summary accepts the gateway budget");
    }
    assert!(
        woke <= limit,
        "an idle gateway woke {woke} times in ten minutes against a budget of {limit}: {reason}"
    );
}
