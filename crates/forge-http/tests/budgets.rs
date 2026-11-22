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

fn budget(name: &str) -> Budget {
    let mut budgets: BTreeMap<String, Budget> =
        serde_json::from_str(include_str!("budgets/forge.json"))
            .expect("the budget file is valid JSON");
    budgets.remove(name).expect("the budget is in the file")
}

fn summarize(name: &str, measured: u64, limit: u64) {
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write as _;
        let mut summary = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .expect("the job summary opens for appending");
        writeln!(
            summary,
            "| Budget | Measured | Limit |\n| --- | ---: | ---: |\n| `{name}` | {measured} | {limit} |"
        )
        .expect("the job summary accepts the Forge budget");
    }
}

struct Running {
    hub: forge_runs::Hub,
    member: forge_runs::runtime::Member,
    stop: tokio::sync::oneshot::Sender<()>,
    serving: tokio::task::JoinHandle<()>,
}

impl Running {
    fn overlapping(&self) -> bool {
        self.hub
            .generation()
            .is_some_and(forge_runs::runtime::Generation::overlapping)
    }

    async fn stop(self) {
        self.stop.send(()).expect("Forge is still serving");
        self.serving.await.expect("Forge stops cleanly");
    }
}

async fn generation(data: &std::path::Path, registry: &forge_runs::runtime::Registry) -> Running {
    let server = forge_http::start_generation(
        forge_http::ServeConfig {
            data_dir: data.to_owned(),
            listen: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
        },
        registry.clone(),
    )
    .await
    .expect("the generation starts");
    let hub = server.hub().clone();
    let member = server
        .hub()
        .generation()
        .expect("a managed generation")
        .member()
        .clone();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(server.run_until(async {
        stopped
            .await
            .expect("the test sends stop before dropping its sender");
    }));
    Running {
        hub,
        member,
        stop,
        serving,
    }
}

async fn activate(registry: &forge_runs::runtime::Registry, member: &forge_runs::runtime::Member) {
    forge_runs::runtime::request(member, &forge_runs::runtime::Command::Activate)
        .await
        .expect("the generation activates");
    registry
        .promote(member)
        .expect("the generation is promoted");
}

fn counting_runtime(wakeups: &Arc<AtomicU64>) -> tokio::runtime::Runtime {
    let counter = Arc::clone(wakeups);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .on_thread_unpark(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        })
        .build()
        .expect("the test runtime starts")
}

#[test]
fn quiet_overlapping_generations_sleep_instead_of_polling() {
    let name = "overlap_wakeups_per_ten_minutes";
    let budget = budget(name);
    let wakeups = Arc::new(AtomicU64::new(0));
    let runtime = counting_runtime(&wakeups);
    let directory = tempfile::tempdir().expect("a Forge data folder");
    let woke = runtime.block_on(async {
        let _store = forge_store::Store::open(directory.path()).expect("the store opens");
        let registry = forge_runs::runtime::Registry::open(directory.path())
            .expect("the generation registry opens");
        let old = generation(directory.path(), &registry).await;
        activate(&registry, &old.member).await;
        let new = generation(directory.path(), &registry).await;
        activate(&registry, &new.member).await;
        assert!(old.overlapping() && new.overlapping());
        for _ in 0..STARTUP_YIELDS {
            tokio::task::yield_now().await;
        }
        tokio::time::pause();
        let before = wakeups.load(Ordering::Relaxed);
        tokio::time::sleep(IDLE_FOR).await;
        let woke = wakeups.load(Ordering::Relaxed) - before;
        tokio::time::resume();
        new.stop().await;
        old.stop().await;
        woke
    });
    summarize(name, woke, budget.limit);
    assert!(
        woke <= budget.limit,
        "two quiet overlapping generations woke {woke} times in ten minutes against a budget of {}: {}",
        budget.limit,
        budget.reason
    );
}

#[test]
fn a_generation_that_cannot_be_reached_is_retried_ever_less_often() {
    let name = "unreachable_generation_wakeups_per_ten_minutes";
    let budget = budget(name);
    let wakeups = Arc::new(AtomicU64::new(0));
    let runtime = counting_runtime(&wakeups);
    let directory = tempfile::tempdir().expect("a Forge data folder");
    let woke = runtime.block_on(async {
        let _store = forge_store::Store::open(directory.path()).expect("the store opens");
        let registry = forge_runs::runtime::Registry::open(directory.path())
            .expect("the generation registry opens");
        let _listed = registry
            .register(forge_runs::runtime::Member {
                id: String::new(),
                control: "unreachable".to_owned(),
                token: "unreachable".to_owned(),
                address: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 9)),
                pid: std::process::id(),
                protocol: forge_runs::runtime::PROTOCOL,
                schema: forge_store::Store::schema_version(),
            })
            .expect("the unreachable generation is listed");
        let running = generation(directory.path(), &registry).await;
        activate(&registry, &running.member).await;
        assert!(running.overlapping());
        for _ in 0..STARTUP_YIELDS {
            tokio::task::yield_now().await;
        }
        tokio::time::pause();
        let before = wakeups.load(Ordering::Relaxed);
        tokio::time::sleep(IDLE_FOR).await;
        let woke = wakeups.load(Ordering::Relaxed) - before;
        tokio::time::resume();
        running.stop().await;
        woke
    });
    summarize(name, woke, budget.limit);
    assert!(
        woke <= budget.limit,
        "a generation following one it cannot reach woke {woke} times in ten minutes against a budget of {}: {}",
        budget.limit,
        budget.reason
    );
}

#[test]
fn an_idle_forge_sleeps_instead_of_polling() {
    let name = "idle_wakeups_per_ten_minutes";
    let budget = budget(name);
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
        let before = wakeups.load(Ordering::Relaxed);
        test_support::clock::paused(tokio::time::sleep(IDLE_FOR)).await;
        let woke = wakeups.load(Ordering::Relaxed) - before;
        stop.send(()).expect("Forge is still serving");
        serving.await.expect("Forge stops cleanly");
        woke
    });
    summarize(name, woke, budget.limit);
    assert!(
        woke <= budget.limit,
        "an idle Forge woke {woke} times in ten minutes against a budget of {}: {}",
        budget.limit,
        budget.reason
    );
}
