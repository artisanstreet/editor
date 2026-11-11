//! Forge's network API: HTTPS (HTTP/2) with mutual TLS, JSON routes and one
//! server-sent event stream. Nothing is tied to a connection; a client's
//! only sync state is the last seq it saw.

mod account_auth;
mod account_service;
pub mod core;
mod events;
mod extensions;
mod gateway;
mod generation;
mod relay;
mod repository;
mod routes;
pub mod sync;
pub mod tls;
#[cfg(unix)]
pub mod upgrade;

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

/// Forge's port unless `config.json` in its data says otherwise.
pub const DEFAULT_PORT: u16 = 47_900;

/// Where Forge listens: `listen` in `<data>/config.json`, else loopback on
/// [`DEFAULT_PORT`].
pub fn configured_listen(data_dir: &Path) -> Result<SocketAddr> {
    #[derive(Deserialize)]
    struct Config {
        listen: Option<SocketAddr>,
    }
    let path = data_dir.join("config.json");
    let listen = match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice::<Config>(&bytes)
                .with_context(|| format!("read {}", path.display()))?
                .listen
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    Ok(listen
        .unwrap_or_else(|| SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), DEFAULT_PORT)))
}
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

/// Opens the store and engines under `data_dir` and serves until `stop`
/// resolves. Holds an exclusive lock on the data directory.
pub async fn serve(config: ServeConfig, stop: impl Future<Output = ()>) -> Result<()> {
    start(config).await?.run_until(stop).await
}

/// How long a stopping Forge waits for its runs to settle. Within the
/// service manager's stop timeout (30 s).
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// A bound, not yet serving Forge.
pub struct Server {
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    app: axum::Router,
    store: Store,
    hub: Hub,
    shutdown: CancellationToken,
    drain: CancellationToken,
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
    start_mode(config, harness_for, None).await
}

pub async fn start_generation(
    config: ServeConfig,
    registry: forge_runs::runtime::Registry,
) -> Result<Server> {
    start_mode(config, Arc::new(forge_engines::harness), Some(registry)).await
}

pub async fn start_generation_with_harness(
    config: ServeConfig,
    registry: forge_runs::runtime::Registry,
    harness_for: forge_runs::HarnessFor,
) -> Result<Server> {
    start_mode(config, harness_for, Some(registry)).await
}

async fn start_mode(
    config: ServeConfig,
    harness_for: forge_runs::HarnessFor,
    registry: Option<forge_runs::runtime::Registry>,
) -> Result<Server> {
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("create {}", config.data_dir.display()))?;
    let lock = if registry.is_some() {
        lock_generation_dir(&config.data_dir)?
    } else {
        lock_data_dir(&config.data_dir)?
    };
    let store = if registry.is_some() {
        Store::join(&config.data_dir)
    } else {
        Store::open(&config.data_dir)
    }
    .context("open the store")?;
    let identity = identity(&store).await?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls::server_config(&identity)?));
    let shutdown = CancellationToken::new();
    let drain = CancellationToken::new();
    let tasks = TaskTracker::new();
    let engines = Engines::new(&config.data_dir);
    let inherited = if registry.is_some() {
        Some(TcpListener::bind(config.listen).await?)
    } else {
        None
    };
    let (hub, control) = if let Some(registry) = &registry {
        let (generation, control) = generation::prepare(
            registry,
            inherited
                .as_ref()
                .context("generation listener missing")?
                .local_addr()?,
        )
        .await?;
        (
            Hub::with_generation(
                store.clone(),
                engines,
                config.data_dir.clone(),
                harness_for,
                generation,
            ),
            Some(control),
        )
    } else {
        (
            Hub::with_harness(store.clone(), engines, config.data_dir.clone(), harness_for),
            None,
        )
    };
    let startup_shutdown = hub.shutdown_on_drop();
    let prepared = async {
        if registry.is_none()
            && let Err(error) = store.gc_content().await
        {
            tracing::warn!("collect unreferenced text sources: {error:#}");
        }
        if registry.is_none()
            && let Err(error) = store.gc_blobs().await
        {
            tracing::warn!("collect unreferenced blobs: {error:#}");
        }
        reconcile_startup(&hub).await?;
        let listener = match inherited {
            Some(listener) => listener,
            None => TcpListener::bind(config.listen)
                .await
                .with_context(|| format!("listen on {}", config.listen))?,
        };
        let address = listener.local_addr()?;
        let app =
            start_services(&hub, &config.data_dir, control, &shutdown, &drain, &tasks).await?;
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
    refresh_engines(&hub, &shutdown, &tasks);
    tracing::info!(%address, data = %config.data_dir.display(), "Forge is serving");
    let server = Server {
        listener,
        acceptor,
        app,
        store,
        hub,
        shutdown,
        drain,
        tasks,
        _lock: lock,
    };
    startup_shutdown.disarm();
    Ok(server)
}

async fn reconcile_startup(hub: &Hub) -> Result<()> {
    hub.reconcile_global_prompt()
        .await
        .context("reconcile global prompt")?;
    hub.reconcile_skills()
        .await
        .context("reconcile managed skills")?;
    hub.ensure_scratch_project()
        .await
        .context("ensure the scratch project")?;
    Ok(())
}

fn refresh_engines(hub: &Hub, shutdown: &CancellationToken, tasks: &TaskTracker) {
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
}

async fn start_services(
    hub: &Hub,
    data_dir: &Path,
    control: Option<TcpListener>,
    shutdown: &CancellationToken,
    drain: &CancellationToken,
    tasks: &TaskTracker,
) -> Result<axum::Router> {
    let routes_app = routes::App {
        hub: hub.clone(),
        forge_version: env!("CARGO_PKG_VERSION").to_owned(),
        shutdown: shutdown.clone(),
        tasks: tasks.clone(),
        repositories: Arc::new(repository::Repositories::new()?),
        icons: Arc::default(),
    };
    let app = routes::router(routes_app.clone());
    gateway::start(hub, shutdown.clone(), tasks).await?;
    if let Some(control) = control {
        generation::start(hub, control, shutdown, drain, tasks);
    }
    if hub.generation().is_none() {
        tasks.spawn(account_service::run(
            data_dir.to_path_buf(),
            routes_app,
            shutdown.clone(),
            tasks.clone(),
        ));
    } else {
        generation::start_primary_services(data_dir.to_path_buf(), routes_app, shutdown, tasks);
    }
    hub.resume_pending();
    let auth = hub.clone();
    tasks.spawn(async move { auth.check_mcp_auth().await });
    let agents = hub.clone();
    tasks.spawn(async move { agents.reconcile_agents().await });
    let servers = hub.clone();
    let stop = shutdown.clone();
    tasks.spawn(async move { servers.watch_mcp_servers(stop).await });
    if hub.generation().is_none() {
        tasks.spawn(remove_inactive_threads(
            hub.clone(),
            hub.store().clone(),
            shutdown.clone(),
        ));
    }
    Ok(app)
}

impl Server {
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    #[must_use]
    pub const fn store(&self) -> &Store {
        &self.store
    }

    pub const fn hub(&self) -> &Hub {
        &self.hub
    }

    pub async fn run(self) -> Result<()> {
        self.run_until(std::future::pending()).await
    }

    /// Serves until `stop` resolves, then stops every run so it ends
    /// interrupted, and carries on when Forge starts again.
    pub async fn run_until(self, stop_requested: impl Future<Output = ()>) -> Result<()> {
        let connections = TaskTracker::new();
        let store = self.store.clone();
        let hub = self.hub.clone();
        let stop = self.shutdown.clone();
        self.tasks.spawn(async move {
            let hour = std::time::Duration::from_hours(1);
            let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + hour, hour);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = stop.cancelled() => return,
                    _ = interval.tick() => {}
                }
                if hub
                    .generation()
                    .is_some_and(|generation| !generation.accepting())
                {
                    continue;
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
        tokio::pin!(stop_requested);
        loop {
            let accepted = tokio::select! {
                () = &mut stop_requested => break,
                () = self.drain.cancelled() => break,
                accepted = self.listener.accept() => accepted,
            };
            let (socket, peer) = match accepted {
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
            let drain = self.drain.clone();
            connections.spawn(async move {
                serve_connection(socket, peer, acceptor, app, drain).await;
            });
        }
        tracing::info!("stopping: settling live runs");
        self.drain.cancel();
        self.store.relay_live(artisan_api::Event::Reset);
        self.hub.stop(STOP_GRACE).await;
        connections.close();
        if tokio::time::timeout(STOP_GRACE, connections.wait())
            .await
            .is_err()
        {
            tracing::warn!("HTTP connections exceeded the drain grace period");
        }
        Ok(())
    }
}

async fn serve_connection(
    socket: tokio::net::TcpStream,
    peer: SocketAddr,
    acceptor: tokio_rustls::TlsAcceptor,
    app: axum::Router,
    drain: CancellationToken,
) {
    let stream = tokio::select! {
        ()=drain.cancelled()=>return,
        result=acceptor.accept(socket)=>match result {Ok(stream)=>stream,Err(error)=>{tracing::debug!(%peer,"TLS handshake failed: {error}");return;}}
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
    let service =
        hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
            request.extensions_mut().insert(client.clone());
            app.clone().call(request)
        });
    let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
    tokio::pin!(connection);
    tokio::select! {
        result=&mut connection=>{if let Err(error)=result {tracing::debug!(%peer,"connection ended: {error}");}}
        ()=drain.cancelled()=>{
            connection.as_mut().graceful_shutdown();
            if let Ok(Err(error))=tokio::time::timeout(STOP_GRACE,connection).await {tracing::debug!(%peer,"connection drain: {error}");}
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

async fn remove_inactive_threads(hub: Hub, store: Store, shutdown: CancellationToken) {
    loop {
        if let Err(error) = hub.remove_inactive_threads().await {
            tracing::warn!("remove inactive threads: {error:#}");
        }
        if let Err(error) = store.gc_content().await {
            tracing::warn!("collect unreferenced text sources: {error:#}");
        }
        if let Err(error) = store.gc_blobs().await {
            tracing::warn!("collect unreferenced blobs: {error:#}");
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(std::time::Duration::from_hours(1)) => {}
        }
    }
}

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
    let stored = if let Some(stored) = admin.secret(IDENTITY_KEY)? {
        stored
    } else {
        let fresh = serde_json::to_string(&Identity::generate("artisan-forge")?)?;
        admin.secret_or_insert(IDENTITY_KEY, &fresh)?
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

fn lock_generation_dir(dir: &Path) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("forge.lock"))?;
    fs2::FileExt::try_lock_shared(&file).context("an older Forge still exclusively owns this data directory; bootstrap the upgrade-capable service first")?;
    Ok(file)
}

#[cfg(test)]
#[path = "../tests/generation.rs"]
mod generation_tests;
#[cfg(test)]
mod tests;
