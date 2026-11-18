use crate::plan::Stop;
use forge_http::PairInvite;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
    running: Option<Running>,
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
            .block_on(forge_http::start(forge_http::ServeConfig {
                data_dir: self.data.clone(),
                listen: SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
            }))
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
