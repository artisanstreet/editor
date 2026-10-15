//! Forge's network API: HTTPS (HTTP/2) with mutual TLS, JSON routes and one
//! server-sent event stream. Nothing is tied to a connection; a client's
//! only sync state is the last seq it saw.

mod events;
mod extensions;
mod repository;
mod routes;
pub mod tls;

use anyhow::{Context, Result};
use artisan_api::EngineId;
use artisan_api::Timestamp;
use forge_engines::Engines;
use forge_runs::Hub;
use forge_store::Store;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tower::Service;

pub use tls::Identity;

/// Default port for `ae forge serve`.
pub const DEFAULT_PORT: u16 = 47_900;
const IDENTITY_KEY: &str = "forge_identity";
const PAIR_CODE_LIFETIME_MS: Timestamp = 10 * 60 * 1000;

#[derive(Clone, Debug)]
pub struct ServeConfig {
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
}

/// What a new Editor needs to pair: Forge's certificate (to pin) and a
/// one-time code (to prove the user allowed it).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairInvite {
    pub code: String,
    pub forge_certificate: String,
    pub expires_at: Timestamp,
}

/// Opens the store and engines under `data_dir` and serves until the
/// listener fails. Holds an exclusive lock on the data directory.
pub async fn serve(config: ServeConfig) -> Result<()> {
    start(config).await?.run().await
}

/// A bound, not yet serving Forge.
pub struct Server {
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    app: axum::Router,
    store: Store,
    hub: Hub,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    _lock: std::fs::File,
}

pub async fn start(config: ServeConfig) -> Result<Server> {
    start_with_harness(config, std::sync::Arc::new(forge_engines::harness)).await
}

/// [`start`] with the engine adapters swapped, so clients can be tested
/// against a scripted engine.
pub async fn start_with_harness(
    config: ServeConfig,
    harness_for: forge_runs::HarnessFor,
) -> Result<Server> {
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("create {}", config.data_dir.display()))?;
    let lock = lock_data_dir(&config.data_dir)?;
    let store = Store::open(&config.data_dir).context("open the store")?;
    let identity = identity(&store).await?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls::server_config(&identity)?));
    let shutdown = CancellationToken::new();
    let tasks = TaskTracker::new();
    let engines = Engines::new(&config.data_dir);
    let hub = Hub::with_harness(store.clone(), engines, config.data_dir.clone(), harness_for);
    let startup_shutdown = hub.shutdown_on_drop();
    let prepared = async {
        if let Err(error) = store.gc_content().await {
            tracing::warn!("collect unreferenced text sources: {error:#}");
        }
        if let Err(error) = store.gc_blobs().await {
            tracing::warn!("collect unreferenced blobs: {error:#}");
        }
        hub.reconcile_global_prompt()
            .await
            .context("reconcile global prompt")?;
        hub.reconcile_skills()
            .await
            .context("reconcile managed skills")?;
        let app = routes::router(routes::App {
            hub: hub.clone(),
            forge_version: env!("CARGO_PKG_VERSION").to_owned(),
            shutdown: shutdown.clone(),
            tasks: tasks.clone(),
            repositories: Arc::new(repository::Repositories::new()?),
            icons: Arc::default(),
        });
        // A bound socket is the detached launcher's readiness boundary.
        // Mandatory setup must succeed before another process can observe it.
        let listener = TcpListener::bind(config.listen)
            .await
            .with_context(|| format!("listen on {}", config.listen))?;
        let address = listener.local_addr()?;
        // Failed binds must leave durable inputs untouched, without launching
        // a controller that will never accept its paired clients.
        hub.resume_pending();
        anyhow::Ok((listener, address, app))
    }
    .await;
    let (listener, address, app) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            hub.shutdown();
            shutdown.cancel();
            tasks.close();
            return Err(error);
        }
    };
    let engines = hub.engines().clone();
    let stop = shutdown.clone();
    tasks.spawn(async move {
        use futures::StreamExt as _;
        tokio::select! {
            () = stop.cancelled() => {},
            () = async {
                futures::stream::iter(EngineId::ALL).for_each_concurrent(3, |engine| {
                    let engines = &engines;
                    async move { engines.refresh(engine).await; }
                }).await;
                engines.keep_usage_fresh().await;
            } => {},
        }
    });
    tracing::info!(%address, data = %config.data_dir.display(), "Forge is serving");
    let server = Server {
        listener,
        acceptor,
        app,
        store,
        hub,
        shutdown,
        tasks,
        _lock: lock,
    };
    startup_shutdown.disarm();
    Ok(server)
}

impl Server {
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    pub async fn run(self) -> Result<()> {
        let store = self.store.clone();
        let stop = self.shutdown.clone();
        self.tasks.spawn(async move {
            let hour = std::time::Duration::from_secs(60 * 60);
            let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + hour, hour);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = stop.cancelled() => return,
                    _ = interval.tick() => {}
                }
                // Each batch is its own writer transaction. Yield between them,
                // allowing accepted input and Item writes to use the same owner.
                while !stop.is_cancelled() {
                    match store.gc_content().await {
                        Ok(0) => break,
                        Ok(_) => tokio::task::yield_now().await,
                        Err(error) => {
                            tracing::warn!("source collection failed: {error:#}");
                            break;
                        }
                    }
                }
            }
        });
        loop {
            let (socket, peer) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Out of descriptors or memory fails every accept until
                    // a connection closes; pause instead of spinning.
                    tracing::warn!("accept a connection: {error}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            };
            let acceptor = self.acceptor.clone();
            let app = self.app.clone();
            let stop = self.shutdown.clone();
            self.tasks.spawn(async move {
                tokio::select! {
                    () = stop.cancelled() => {},
                    () = async {
                let stream = match acceptor.accept(socket).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        tracing::debug!(%peer, "TLS handshake failed: {error}");
                        return;
                    }
                };
                let Some(certificate) = stream
                    .get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|certs| certs.first())
                else {
                    return;
                };
                let client = routes::Client {
                    certificate: tls::certificate_id(certificate),
                };
                let service = hyper::service::service_fn(
                    move |mut request: hyper::Request<hyper::body::Incoming>| {
                        request.extensions_mut().insert(client.clone());
                        app.clone().call(request)
                    },
                );
                if let Err(error) =
                    hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                        .serve_connection_with_upgrades(TokioIo::new(stream), service)
                        .await
                {
                    tracing::debug!(%peer, "connection ended: {error}");
                }
                    } => {},
                }
            });
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.hub.shutdown();
        self.shutdown.cancel();
        self.tasks.close();
    }
}

/// Forge's certificate, made on first use and kept in the store.
pub async fn identity(store: &Store) -> Result<Identity> {
    if let Some(stored) = store.secret(IDENTITY_KEY).await? {
        return serde_json::from_str(&stored).context("read Forge's identity");
    }
    let fresh = serde_json::to_string(&Identity::generate("artisan-forge")?)?;
    let stored = store.secret_or_insert(IDENTITY_KEY, fresh).await?;
    serde_json::from_str(&stored).context("read Forge's identity")
}

/// Makes a one-time pairing code, valid for ten minutes.
pub async fn invite(store: &Store) -> Result<PairInvite> {
    let identity = identity(store).await?;
    let (code, expires_at) = new_code()?;
    store.add_pair_code(hash_code(&code), expires_at).await?;
    Ok(PairInvite {
        code,
        forge_certificate: identity.certificate,
        expires_at,
    })
}

/// [`invite`] without opening the store, so it works while Forge runs on
/// `data_dir` (opening a `Store` would recover, i.e. cancel, its live runs).
pub fn invite_offline(data_dir: &Path) -> Result<PairInvite> {
    let mut admin = forge_store::admin::Admin::open(data_dir)?;
    let stored = match admin.secret(IDENTITY_KEY)? {
        Some(stored) => stored,
        None => {
            let fresh = serde_json::to_string(&Identity::generate("artisan-forge")?)?;
            admin.secret_or_insert(IDENTITY_KEY, &fresh)?
        }
    };
    let identity: Identity = serde_json::from_str(&stored).context("read Forge's identity")?;
    let (code, expires_at) = new_code()?;
    admin.add_pair_code(&hash_code(&code), expires_at)?;
    Ok(PairInvite {
        code,
        forge_certificate: identity.certificate,
        expires_at,
    })
}

fn new_code() -> Result<(String, Timestamp)> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("random code: {error}"))?;
    // 32 symbols, so `% 32` is unbiased; no 0/O or 1/I to misread.
    let alphabet = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let code = bytes
        .iter()
        .map(|byte| alphabet[usize::from(byte % 32)] as char)
        .collect();
    Ok((code, artisan_api::now() + PAIR_CODE_LIFETIME_MS))
}

pub(crate) fn hash_code(code: &str) -> String {
    let normalised: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    hex::encode(Sha256::digest(normalised.as_bytes()))
}

/// One Forge per data directory; anything else that opens the `Store`
/// (like the legacy import) takes the same lock.
pub fn lock_data_dir(dir: &Path) -> Result<std::fs::File> {
    use fs2::FileExt;
    let path = dir.join("forge.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.try_lock_exclusive()
        .with_context(|| format!("another Forge is using {}", dir.display()))?;
    Ok(file)
}

#[cfg(test)]
mod tests;
