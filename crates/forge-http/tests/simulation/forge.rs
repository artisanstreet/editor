use crate::plan::Stop;
use forge_http::PairInvite;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

const SIDE_QUERIES: [&str; 3] = [
    forge_runs::SUGGESTIONS_KEY,
    forge_runs::YOU_SHOULD_KNOW_KEY,
    forge_runs::SUMMARY_TITLES_KEY,
];
const STOPPED_WITHIN: Duration = Duration::from_secs(60);

pub(crate) struct Forge {
    data: PathBuf,
    disk: Disk,
    clock: Clock,
    running: Option<Running>,
}

#[derive(Clone, Default)]
pub(crate) struct Clock(Arc<AtomicI64>);

impl Clock {
    pub(crate) fn step(&self, by_ms: i64) {
        self.0.fetch_add(by_ms, Ordering::SeqCst);
    }

    fn wall(&self) -> artisan_api::WallClock {
        let offset = Arc::clone(&self.0);
        artisan_api::WallClock::new(move || {
            artisan_api::now().saturating_add(offset.load(Ordering::SeqCst))
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct Disk(Arc<AtomicU32>);

impl Disk {
    pub(crate) fn fail_next_commit(&self) {
        self.0.store(1, Ordering::SeqCst);
    }

    pub(crate) fn heal(&self) {
        self.0.store(0, Ordering::SeqCst);
    }

    fn connections(&self) -> forge_store::Connections {
        let failing = Arc::clone(&self.0);
        forge_store::Connections::new(move |path| {
            let mut conn = rusqlite::Connection::open(path)?;
            let failing = Arc::clone(&failing);
            conn.commit_hook(Some(move || {
                failing
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                        left.checked_sub(1)
                    })
                    .is_ok()
            }))?;
            Ok(conn)
        })
    }
}

struct Running {
    runtime: Runtime,
    address: SocketAddr,
    stop: oneshot::Sender<()>,
    serving: tokio::task::JoinHandle<()>,
}

impl Forge {
    pub(crate) fn start(data: &Path) -> (Self, PairInvite) {
        let mut forge = Self {
            data: data.to_path_buf(),
            disk: Disk::default(),
            clock: Clock::default(),
            running: None,
        };
        let store = forge.launch();
        let invite = forge
            .running
            .as_ref()
            .expect("invariant: Forge was just launched")
            .runtime
            .block_on(forge_http::invite(&store))
            .unwrap_or_else(|error| panic!("invite the Editor: {}", diagnostics::chain(&error)));
        (forge, invite)
    }

    pub(crate) const fn disk(&self) -> &Disk {
        &self.disk
    }

    pub(crate) const fn clock(&self) -> &Clock {
        &self.clock
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.running
            .as_ref()
            .expect("invariant: the simulation only talks to a running Forge")
            .address
    }

    pub(crate) fn restart(&mut self, stop: Stop) {
        let running = self
            .running
            .take()
            .expect("invariant: only a running Forge restarts");
        match stop {
            Stop::Graceful => {
                running
                    .stop
                    .send(())
                    .expect("Forge is still serving when asked to stop");
                running.runtime.block_on(async {
                    test_support::wait::within("Forge to stop", STOPPED_WITHIN, running.serving)
                        .await
                        .expect("Forge stops without panicking");
                });
                running.runtime.shutdown_timeout(STOPPED_WITHIN);
            }
            Stop::Killed => running.runtime.shutdown_timeout(STOPPED_WITHIN),
        }
        self.wait_until_released();
        drop(self.launch());
    }

    fn wait_until_released(&self) {
        let held = || -> Vec<PathBuf> {
            std::fs::read_dir("/proc/self/fd")
                .expect("list this process's open files")
                .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
                .filter(|path| path.starts_with(&self.data))
                .collect()
        };
        let unlocked = || {
            let lock = std::fs::File::open(self.data.join("forge.lock"))
                .expect("open the data folder's lock");
            fs2::FileExt::try_lock_exclusive(&lock).is_ok()
        };
        test_support::wait::wait_until(
            "the stopped Forge and the engines it was starting to let go of its data folder",
            STOPPED_WITHIN,
            || held().is_empty() && unlocked(),
        );
    }

    fn launch(&mut self) -> forge_store::Store {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Forge's runtime starts");
        let server = runtime
            .block_on(forge_http::start_in(
                forge_http::ServeConfig {
                    data_dir: self.data.clone(),
                    listen: SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
                },
                forge_store::Environment {
                    connections: self.disk.connections(),
                    clock: self.clock.wall(),
                },
            ))
            .unwrap_or_else(|error| panic!("Forge starts: {}", diagnostics::chain(&error)));
        let address = server.local_addr().expect("Forge listens");
        let store = server.store().clone();
        runtime.block_on(async {
            for key in SIDE_QUERIES {
                store
                    .set_setting(key.to_owned(), serde_json::json!(false))
                    .await
                    .expect("turn off side queries, which would launch the engine again");
            }
        });
        let (stop, stop_requested) = oneshot::channel();
        let serving = runtime.spawn(server.run_until(async {
            drop(stop_requested.await);
        }));
        self.running = Some(Running {
            runtime,
            address,
            stop,
            serving,
        });
        store
    }
}

impl Drop for Forge {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            running.runtime.shutdown_background();
        }
    }
}
