#![cfg(target_os = "linux")]

use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const IDLE_FOR: Duration = Duration::from_secs(600);
const STARTUP_YIELDS: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    limit: u64,
    reason: String,
}

#[test]
fn an_idle_forge_sleeps_instead_of_polling() {
    let budgets: BTreeMap<String, Budget> =
        serde_json::from_str(include_str!("budgets/forge.json"))
            .expect("the budget file is valid JSON");
    let budget = &budgets["idle_wakeups_per_ten_minutes"];
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
    let woke = runtime.block_on(async {
        let server = forge_http::start(forge_http::ServeConfig {
            data_dir: directory.path().to_owned(),
            listen: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
        })
        .await
        .expect("Forge starts");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = tokio::spawn(server.run_until(async {
            stopped
                .await
                .expect("the test sends stop before dropping its sender");
        }));
        for _ in 0..STARTUP_YIELDS {
            tokio::task::yield_now().await;
        }
        tokio::time::pause();
        let before = wakeups.load(Ordering::Relaxed);
        tokio::time::sleep(IDLE_FOR).await;
        let woke = wakeups.load(Ordering::Relaxed) - before;
        tokio::time::resume();
        stop.send(()).expect("Forge is still serving");
        serving.await.expect("Forge stops cleanly");
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
            "| Budget | Measured | Limit |\n| --- | ---: | ---: |\n| `idle_wakeups_per_ten_minutes` | {woke} | {} |",
            budget.limit
        )
        .expect("the job summary accepts the Forge budget");
    }
    assert!(
        woke <= budget.limit,
        "an idle Forge woke {woke} times in ten minutes against a budget of {}: {}",
        budget.limit,
        budget.reason
    );
}
