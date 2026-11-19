mod account_auth;
mod account_service;
pub mod core;
mod events;
mod extensions;
mod gateway;
mod generation;
mod handshakes;
mod relay;
mod repository;
mod routes;
pub mod sync;
pub mod tls;
#[cfg(unix)]
pub mod upgrade;

use artisan_api::EngineId;
use artisan_api::Timestamp;
use forge_engines::Engines;
use forge_runs::Hub;
use forge_store::Store;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tower::Service;

pub use tls::Identity;

const DEFAULT_PORT: u16 = 47_900;

#[derive(Debug, thiserror::Error)]
pub enum ConfiguredListenError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub fn configured_listen(data_dir: &Path) -> Result<SocketAddr, ConfiguredListenError> {
    #[derive(Deserialize)]
    struct Config {
        listen: Option<SocketAddr>,
    }
    let path = data_dir.join("config.json");
    let listen = match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice::<Config>(&bytes)
                .map_err(|source| ConfiguredListenError::Parse {
                    path: path.clone(),
                    source,
                })?
                .listen
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(ConfiguredListenError::Read { path, source }),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairInvite {
    pub code: String,
    pub forge_certificate: String,
    pub expires_at: Timestamp,
}

const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("create the data folder {}", path.display())]
    CreateDataFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Lock(#[from] LockDataError),
    #[error("open the generation lock {}", path.display())]
    OpenGenerationLock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "an older Forge still exclusively owns {}; bootstrap the upgrade-capable service first",
        dir.display()
    )]
    OlderForge {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open the store in {}", dir.display())]
    Store {
        dir: PathBuf,
        #[source]
        source: Box<forge_store::OpenStoreError>,
    },
    #[error(transparent)]
    Identity(Box<IdentityError>),
    #[error("configure TLS with Forge's identity")]
    Tls(#[source] Box<artisan_api::tls::TlsConfigError>),
    #[error("listen on {address}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("read the address Forge listens on")]
    LocalAddr(#[source] std::io::Error),
    #[error(transparent)]
    Generation(Box<generation::PrepareGenerationError>),
    #[error("reconcile the global prompt")]
    ReconcilePrompt(#[source] Box<forge_runs::GlobalPromptError>),
    #[error("reconcile the managed skills")]
    ReconcileSkills(#[source] Box<forge_runs::ReconcileSkillsError>),
    #[error("ensure the scratch project")]
    ScratchProject(#[source] Box<forge_runs::EnsureScratchProjectError>),
    #[error("build the repository observer's HTTP client")]
    Repositories(#[source] reqwest::Error),
    #[error(transparent)]
    Gateway(#[from] gateway::StartGatewayError),
}

impl From<IdentityError> for StartError {
    fn from(error: IdentityError) -> Self {
        Self::Identity(Box::new(error))
    }
}

impl From<generation::PrepareGenerationError> for StartError {
    fn from(error: generation::PrepareGenerationError) -> Self {
        Self::Generation(Box::new(error))
    }
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("listener", &self.listener)
            .field("store", &self.store)
            .field("hub", &self.hub)
            .finish_non_exhaustive()
    }
}

pub struct Server {
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    app: axum::Router,
    store: Store,
    hub: Hub,
    shutdown: CancellationToken,
    drain: CancellationToken,
    tasks: TaskTracker,
    _lock: HeldLock,
}

pub async fn start(config: ServeConfig) -> Result<Server, StartError> {
    start_in(config, forge_store::Environment::system()).await
}

pub async fn start_in(
    config: ServeConfig,
    environment: forge_store::Environment,
) -> Result<Server, StartError> {
    start_with_harness(config, Arc::new(forge_engines::harness), environment).await
}

async fn start_with_harness(
    config: ServeConfig,
    harness_for: forge_runs::HarnessFor,
    environment: forge_store::Environment,
) -> Result<Server, StartError> {
    start_mode(config, harness_for, None, environment).await
}

pub async fn start_generation(
    config: ServeConfig,
    registry: forge_runs::runtime::Registry,
) -> Result<Server, StartError> {
    start_mode(
        config,
        Arc::new(forge_engines::harness),
        Some(registry),
        forge_store::Environment::system(),
    )
    .await
}

#[cfg(test)]
pub(crate) async fn start_generation_with_harness(
    config: ServeConfig,
    registry: forge_runs::runtime::Registry,
    harness_for: forge_runs::HarnessFor,
) -> Result<Server, StartError> {
    start_mode(
        config,
        harness_for,
        Some(registry),
        forge_store::Environment::system(),
    )
    .await
}

#[expect(
    clippy::too_many_lines,
    reason = "one startup sequence that unwinds the hub on any failure"
)]
async fn start_mode(
    config: ServeConfig,
    harness_for: forge_runs::HarnessFor,
    registry: Option<forge_runs::runtime::Registry>,
    environment: forge_store::Environment,
) -> Result<Server, StartError> {
    std::fs::create_dir_all(&config.data_dir).map_err(|source| StartError::CreateDataFolder {
        path: config.data_dir.clone(),
        source,
    })?;
    let (lock, store) = if registry.is_some() {
        let lock = lock_generation_dir(&config.data_dir)?;
        (
            HeldLock::Generation { _shared: lock },
            Store::join_in(&config.data_dir, environment),
        )
    } else {
        let lock = lock_data_dir(&config.data_dir)?;
        let store = Store::open_restoring_in(&config.data_dir, &lock, environment);
        (HeldLock::Data { _exclusive: lock }, store)
    };
    let store = store.map_err(|source| StartError::Store {
        dir: config.data_dir.clone(),
        source: Box::new(source),
    })?;
    let identity = identity(&store).await?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(
        tls::server_config(&identity).map_err(|error| StartError::Tls(Box::new(error)))?,
    ));
    let shutdown = CancellationToken::new();
    let drain = CancellationToken::new();
    let tasks = TaskTracker::new();
    let engines = Engines::new(&config.data_dir);
    let bind = |source| StartError::Bind {
        address: config.listen,
        source,
    };
    let (hub, control, inherited) = if let Some(registry) = &registry {
        let listener = TcpListener::bind(config.listen).await.map_err(bind)?;
        let (generation, control) = generation::prepare(
            registry,
            listener.local_addr().map_err(StartError::LocalAddr)?,
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
            Some(listener),
        )
    } else {
        (
            Hub::with_harness(store.clone(), engines, config.data_dir.clone(), harness_for),
            None,
            None,
        )
    };
    let startup_shutdown = hub.shutdown_on_drop();
    let prepared = async {
        if registry.is_none()
            && let Err(error) = store.gc_content().await
        {
            tracing::warn!(
                error = diagnostics::chain(&error),
                "collect unreferenced text sources"
            );
        }
        if registry.is_none()
            && let Err(error) = store.gc_blobs().await
        {
            tracing::warn!(
                error = diagnostics::chain(&error),
                "collect unreferenced blobs"
            );
        }
        reconcile_startup(&hub).await?;
        let listener = match inherited {
            Some(listener) => listener,
            None => TcpListener::bind(config.listen).await.map_err(bind)?,
        };
        let address = listener.local_addr().map_err(StartError::LocalAddr)?;
        let app =
            start_services(&hub, &config.data_dir, control, &shutdown, &drain, &tasks).await?;
        Ok::<_, StartError>((listener, address, app))
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

async fn reconcile_startup(hub: &Hub) -> Result<(), StartError> {
    hub.extensions()
        .reconcile_global_prompt()
        .await
        .map_err(|error| StartError::ReconcilePrompt(Box::new(error)))?;
    hub.extensions()
        .reconcile_skills()
        .await
        .map_err(|error| StartError::ReconcileSkills(Box::new(error)))?;
    hub.ensure_scratch_project()
        .await
        .map_err(|error| StartError::ScratchProject(Box::new(error)))?;
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
) -> Result<axum::Router, StartError> {
    let routes_app = routes::App {
        hub: hub.clone(),
        forge_version: env!("CARGO_PKG_VERSION").to_owned(),
        shutdown: shutdown.clone(),
        tasks: tasks.clone(),
        repositories: Arc::new(repository::Repositories::new().map_err(StartError::Repositories)?),
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
    let traces = forge_engines::trace::folder(data_dir);
    tasks.spawn_blocking(move || {
        if let Err(failure) = forge_engines::trace::prune(&traces, std::time::SystemTime::now()) {
            tracing::warn!(
                error = diagnostics::chain(&failure),
                "remove old engine traces"
            );
        }
    });
    let auth = hub.clone();
    tasks.spawn(async move { auth.mcp().check_auth().await });
    let agents = hub.clone();
    tasks.spawn(async move { agents.reconcile_agents().await });
    let watching = hub.mcp().watch_server_changes();
    let stop = shutdown.clone();
    tasks.spawn(async move {
        tokio::select! {
            () = stop.cancelled() => {}
            () = watching => {}
        }
    });
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
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    #[must_use]
    pub const fn store(&self) -> &Store {
        &self.store
    }

    pub const fn hub(&self) -> &Hub {
        &self.hub
    }

    pub async fn run(self) {
        self.run_until(std::future::pending()).await;
    }

    pub async fn run_until(self, stop_requested: impl Future<Output = ()>) {
        let connections = TaskTracker::new();
        let handshakes = handshakes::Handshakes::new(handshakes::IN_FLIGHT, handshakes::PER_PEER);
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
                while !stop.is_cancelled() {
                    match store.gc_content().await {
                        Ok(0) => break,
                        Ok(_) => tokio::task::yield_now().await,
                        Err(error) => {
                            tracing::warn!(
                                error = diagnostics::chain(&error),
                                "source collection failed"
                            );
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
                    tracing::warn!("accept a connection: {error}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            };
            let handshake = match handshakes.admit(peer.ip()) {
                Ok(handshake) => handshake,
                Err(refused) => {
                    tracing::debug!(%peer, "refused a connection: {refused}");
                    drop(socket);
                    continue;
                }
            };
            let acceptor = self.acceptor.clone();
            let app = self.app.clone();
            let drain = self.drain.clone();
            connections.spawn(async move {
                serve_connection(socket, peer, acceptor, handshake, app, drain).await;
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
    }
}

async fn serve_connection<Socket>(
    socket: Socket,
    peer: SocketAddr,
    acceptor: tokio_rustls::TlsAcceptor,
    handshake: handshakes::Slot,
    app: axum::Router,
    drain: CancellationToken,
) where
    Socket: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let stream = tokio::select! {
        () = drain.cancelled() => return,
        result = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(socket)) => match result {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                tracing::debug!(%peer, "TLS handshake failed: {error}");
                return;
            }
            Err(elapsed) => {
                tracing::debug!(%peer, "TLS handshake failed: {elapsed}");
                return;
            }
        }
    };
    drop(handshake);
    let Some(certificate) = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
    else {
        return;
    };
    let client = routes::Client {
        certificate: forge_store::DeviceCertificate::from_der(certificate.as_ref()),
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
            tracing::warn!(
                error = diagnostics::chain(&error),
                "remove inactive threads"
            );
        }
        if let Err(error) = store.gc_content().await {
            tracing::warn!(
                error = diagnostics::chain(&error),
                "collect unreferenced text sources"
            );
        }
        if let Err(error) = store.gc_blobs().await {
            tracing::warn!(
                error = diagnostics::chain(&error),
                "collect unreferenced blobs"
            );
        }
        if let Err(error) = store.gc_resource_snapshots().await {
            tracing::warn!(
                error = diagnostics::chain(&error),
                "collect resource snapshots of removed threads"
            );
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(std::time::Duration::from_hours(1)) => {}
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("read Forge's stored identity")]
    Read(#[source] forge_store::ReadSecretError),
    #[error("save Forge's new identity")]
    Save(#[source] forge_store::SetSecretError),
    #[error("read Forge's stored identity")]
    ReadOffline(#[source] forge_store::admin::ReadSecretError),
    #[error("save Forge's new identity")]
    SaveOffline(#[source] forge_store::admin::SaveSecretError),
    #[error("generate Forge's identity")]
    Generate(#[source] artisan_api::tls::GenerateIdentityError),
    #[error("encode Forge's identity")]
    Encode(#[source] serde_json::Error),
    #[error("decode Forge's identity")]
    Decode(#[source] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum InviteError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("draw a pairing code")]
    Code(#[source] getrandom::Error),
    #[error("save the pairing code")]
    Save(#[source] forge_store::AddPairCodeError),
    #[error("save the pairing code")]
    SaveOffline(#[source] forge_store::admin::PairCodeError),
}

pub async fn identity(store: &Store) -> Result<Identity, IdentityError> {
    if let Some(stored) = store
        .secret(IDENTITY_KEY)
        .await
        .map_err(IdentityError::Read)?
    {
        return serde_json::from_str(&stored).map_err(IdentityError::Decode);
    }
    let stored = store
        .secret_or_insert(IDENTITY_KEY, fresh_identity()?)
        .await
        .map_err(IdentityError::Save)?;
    serde_json::from_str(&stored).map_err(IdentityError::Decode)
}

fn fresh_identity() -> Result<String, IdentityError> {
    let identity = Identity::generate("artisan-forge").map_err(IdentityError::Generate)?;
    serde_json::to_string(&identity).map_err(IdentityError::Encode)
}

pub async fn invite(store: &Store) -> Result<PairInvite, InviteError> {
    let identity = identity(store).await?;
    let (code, expires_at) = new_code(store.now())?;
    store
        .add_pair_code(forge_store::PairCodeHash::of(&code), expires_at)
        .await
        .map_err(InviteError::Save)?;
    Ok(PairInvite {
        code,
        forge_certificate: identity.certificate,
        expires_at,
    })
}

pub fn invite_offline(admin: &forge_store::admin::Admin) -> Result<PairInvite, InviteError> {
    let stored = if let Some(stored) = admin
        .secret(IDENTITY_KEY)
        .map_err(IdentityError::ReadOffline)?
    {
        stored
    } else {
        admin
            .secret_or_insert(IDENTITY_KEY, &fresh_identity()?)
            .map_err(IdentityError::SaveOffline)?
    };
    let identity: Identity = serde_json::from_str(&stored).map_err(IdentityError::Decode)?;
    let (code, expires_at) = new_code(artisan_api::now())?;
    admin
        .add_pair_code(&forge_store::PairCodeHash::of(&code), expires_at)
        .map_err(InviteError::SaveOffline)?;
    Ok(PairInvite {
        code,
        forge_certificate: identity.certificate,
        expires_at,
    })
}

fn new_code(now: Timestamp) -> Result<(String, Timestamp), InviteError> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(InviteError::Code)?;
    let alphabet = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let code = bytes
        .iter()
        .map(|byte| alphabet[usize::from(byte % 32)] as char)
        .collect();
    Ok((code, now + PAIR_CODE_LIFETIME_MS))
}

pub use forge_store::{DataLock, LockDataError};

#[derive(Debug)]
enum HeldLock {
    Generation { _shared: std::fs::File },
    Data { _exclusive: DataLock },
}

pub fn lock_data_dir(dir: &Path) -> Result<DataLock, LockDataError> {
    DataLock::acquire(dir)
}

fn lock_generation_dir(dir: &Path) -> Result<std::fs::File, StartError> {
    let path = dir.join("forge.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| StartError::OpenGenerationLock { path, source })?;
    fs2::FileExt::try_lock_shared(&file).map_err(|source| StartError::OlderForge {
        dir: dir.to_owned(),
        source,
    })?;
    Ok(file)
}

#[cfg(test)]
#[path = "../tests/events.rs"]
mod events_tests;
#[cfg(test)]
#[path = "../tests/generation.rs"]
mod generation_tests;
#[cfg(test)]
#[path = "../tests/replay.rs"]
mod replay;
#[cfg(test)]
#[path = "../tests/lib/unit.rs"]
mod tests;
