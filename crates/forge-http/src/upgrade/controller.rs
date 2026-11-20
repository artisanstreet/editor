use super::process::Exited;
use super::process::{
    CandidateError, HealthyError, InheritError, NotifyError, ReadyError, SpawnError,
};
use super::{Gateway, Outcome, Ready, Upgrade, process, read, write};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use forge_runs::runtime::{self, Command, PROTOCOL, PeerError, Registry, RegistryFileError};
use fs2::FileExt as _;
use std::{
    fs::File, net::SocketAddr, net::TcpListener as StdListener, path::PathBuf, sync::Arc,
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Notify, oneshot},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Service {
    gateway: Gateway,
    registry: Registry,
    data: PathBuf,
    log: diagnostics::Destination,
    listener: Arc<StdListener>,
    lock: Arc<File>,
    replaced: Arc<Notify>,
    wake: Arc<Notify>,
    stop: CancellationToken,
}

fn authorized(service: &Service, headers: &HeaderMap) -> bool {
    headers.get("authorization").is_some_and(|value| {
        value.as_bytes() == format!("Bearer {}", service.gateway.token).as_bytes()
    })
}

async fn health(State(service): State<Service>, headers: HeaderMap) -> Response {
    if !authorized(&service, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut gateway = service.gateway.clone();
    match read::<Gateway>(&service.registry.root().join("gateway.json")) {
        Ok(Some(current)) if current.id == gateway.id => gateway = current,
        Ok(Some(_) | None) => {}
        Err(error) => {
            tracing::warn!(gateway = %gateway.id, error = diagnostics::chain(&error), "read the gateway record for a health check");
        }
    }
    match process::healthy(&gateway.backend).await {
        Ok(()) => Json(Ready { gateway }).into_response(),
        Err(error) => (StatusCode::SERVICE_UNAVAILABLE, diagnostics::chain(&error)).into_response(),
    }
}

async fn stop_gateway(State(service): State<Service>, headers: HeaderMap) -> Response {
    if !authorized(&service, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        service.stop.cancel();
    });
    StatusCode::ACCEPTED.into_response()
}

async fn upgrade(
    State(service): State<Service>,
    headers: HeaderMap,
    Json(upgrade): Json<Upgrade>,
) -> Response {
    if !authorized(&service, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let result = replace(&service, upgrade).await;
    service.wake.notify_one();
    Json(match result {
        Ok(gateway) => Outcome::Ready(Box::new(gateway)),
        Err(error) => {
            tracing::warn!(gateway = %service.gateway.id, error = diagnostics::chain(&error), "upgrade failed");
            Outcome::Error(diagnostics::chain(&error))
        }
    })
    .into_response()
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ReplaceGatewayError {
    #[error("take the upgrade lock")]
    Lock(#[source] RegistryFileError),
    #[error("another Forge upgrade is in progress")]
    Busy,
    #[error("read the active gateway record")]
    Record(#[source] RegistryFileError),
    #[error("the active gateway record is missing")]
    RecordMissing,
    #[error("gateway {gateway} was already replaced by {current}; retry the current gateway")]
    Replaced { gateway: String, current: String },
    #[error("hand the listener and lock of gateway {gateway} to the replacement")]
    Handover {
        gateway: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Candidate(#[from] CandidateError),
    #[error("read the active generation")]
    Active(#[source] RegistryFileError),
    #[error("the active generation is missing")]
    ActiveMissing,
    #[error("commit replacement gateway {replacement}")]
    Commit {
        replacement: String,
        #[source]
        source: CommitError,
    },
    #[error("restore gateway {gateway} after replacement {replacement} failed to commit")]
    Restore {
        gateway: String,
        replacement: String,
        #[source]
        source: RestoreError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(super) enum CommitError {
    #[error("activate generation {generation}")]
    Activate {
        generation: String,
        #[source]
        source: PeerError,
    },
    #[error(transparent)]
    Notify(#[from] NotifyError),
    #[error("record the replacement")]
    Record(#[source] RegistryFileError),
}

#[derive(Debug, thiserror::Error)]
pub(super) enum RestoreError {
    #[error("promote generation {generation} again")]
    Promote {
        generation: String,
        #[source]
        source: RegistryFileError,
    },
    #[error("record the previous gateway again")]
    Record(#[source] RegistryFileError),
    #[error(transparent)]
    Notify(#[from] NotifyError),
}

async fn replace(service: &Service, upgrade: Upgrade) -> Result<Gateway, ReplaceGatewayError> {
    let _upgrade = service
        .registry
        .upgrade_lock()
        .map_err(ReplaceGatewayError::Lock)?
        .ok_or(ReplaceGatewayError::Busy)?;
    let current: Gateway = read(&service.registry.root().join("gateway.json"))
        .map_err(ReplaceGatewayError::Record)?
        .ok_or(ReplaceGatewayError::RecordMissing)?;
    if current.id != service.gateway.id {
        return Err(ReplaceGatewayError::Replaced {
            gateway: service.gateway.id.clone(),
            current: current.id,
        });
    }
    let handover = |source| ReplaceGatewayError::Handover {
        gateway: service.gateway.id.clone(),
        source,
    };
    let replacement = process::candidate(
        &service.registry,
        &service.data,
        &service.log,
        &upgrade.binary,
        service.listener.try_clone().map_err(handover)?,
        service.lock.try_clone().map_err(handover)?,
    )
    .await?;
    let previous = service
        .registry
        .active()
        .map_err(ReplaceGatewayError::Active)?
        .ok_or(ReplaceGatewayError::ActiveMissing)?;
    if let Err(failed) = commit(service, &replacement).await {
        if let Err(source) = restore(service, &current, &previous, &replacement).await {
            tracing::error!(
                gateway = %current.id,
                replacement = %replacement.id,
                error = diagnostics::chain(&failed),
                "replacement failed to commit"
            );
            return Err(ReplaceGatewayError::Restore {
                gateway: current.id,
                replacement: replacement.id,
                source,
            });
        }
        return Err(ReplaceGatewayError::Commit {
            replacement: replacement.id,
            source: failed,
        });
    }
    if let Err(error) = runtime::request(&previous, &Command::Drain).await {
        tracing::warn!(generation = %previous.id, error = diagnostics::chain(&error), "drain the previous generation");
    }
    service.replaced.notify_one();
    Ok(replacement)
}

async fn commit(service: &Service, replacement: &Gateway) -> Result<(), CommitError> {
    runtime::request(&replacement.backend, &Command::Activate)
        .await
        .map_err(|source| CommitError::Activate {
            generation: replacement.backend.id.clone(),
            source,
        })?;
    process::notify(replacement.pid)?;
    write(&service.registry.root().join("gateway.json"), replacement)
        .map_err(CommitError::Record)?;
    write(&service.data.join("forge.pid"), &replacement.pid).map_err(CommitError::Record)?;
    service
        .registry
        .promote(&replacement.backend)
        .map_err(CommitError::Record)?;
    Ok(())
}

async fn restore(
    service: &Service,
    current: &Gateway,
    previous: &runtime::Member,
    replacement: &Gateway,
) -> Result<(), RestoreError> {
    if let Err(error) = runtime::request(previous, &Command::Activate).await {
        tracing::error!(generation = %previous.id, error = diagnostics::chain(&error), "hand new work back to the previous generation");
    }
    service
        .registry
        .promote(previous)
        .map_err(|source| RestoreError::Promote {
            generation: previous.id.clone(),
            source,
        })?;
    write(&service.registry.root().join("gateway.json"), current).map_err(RestoreError::Record)?;
    write(&service.data.join("forge.pid"), &current.pid).map_err(RestoreError::Record)?;
    let restored = process::notify(current.pid);
    if let Err(error) = super::stop_member(replacement).await {
        tracing::warn!(gateway = %replacement.id, error = diagnostics::chain(&error), "stop the replacement gateway");
    }
    Ok(restored?)
}

struct Binding {
    listener: StdListener,
    lock: File,
    id: String,
    directory: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    #[error("read the handoff {}", path.display())]
    Handoff {
        path: PathBuf,
        #[source]
        source: RegistryFileError,
    },
    #[error("the handoff {} is missing or names no id", path.display())]
    HandoffId { path: PathBuf },
    #[error("the handoff {} has no folder", path.display())]
    HandoffFolder { path: PathBuf },
    #[error(transparent)]
    Inherit(#[from] InheritError),
    #[error("open the gateway lock {}", path.display())]
    OpenLock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("another Forge gateway is running")]
    Locked(#[source] std::io::Error),
    #[error("list the generations")]
    Peers(#[source] RegistryFileError),
    #[error("retire the unused registry entries")]
    Retire(#[source] RegistryFileError),
    #[error(transparent)]
    DataLock(#[from] crate::LockDataError),
    #[error("migrate the store in {}", dir.display())]
    Store {
        dir: PathBuf,
        #[source]
        source: Box<forge_store::OpenStoreError>,
    },
    #[error("listen on {address}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("make the listener on {address} non-blocking")]
    NonBlocking {
        address: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("create the binaries folder {}", path.display())]
    Folder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn binding(
    config: &crate::ServeConfig,
    registry: &Registry,
    handoff: Option<PathBuf>,
) -> Result<Binding, BindingError> {
    if let Some(handoff) = handoff {
        let value: Option<serde_json::Value> =
            read(&handoff).map_err(|source| BindingError::Handoff {
                path: handoff.clone(),
                source,
            })?;
        let Some(id) = value
            .as_ref()
            .and_then(|value| value["id"].as_str())
            .map(str::to_owned)
        else {
            return Err(BindingError::HandoffId { path: handoff });
        };
        let Some(directory) = handoff.parent().map(std::path::Path::to_path_buf) else {
            return Err(BindingError::HandoffFolder { path: handoff });
        };
        return Ok(Binding {
            listener: process::listener()?,
            lock: process::inherited_lock()?,
            id,
            directory,
        });
    }
    let lock_path = registry.root().join("gateway.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|source| BindingError::OpenLock {
            path: lock_path,
            source,
        })?;
    lock.try_lock_exclusive().map_err(BindingError::Locked)?;
    if registry.peers().map_err(BindingError::Peers)?.is_empty() {
        registry.retire_unused().map_err(BindingError::Retire)?;
        let exclusive = crate::lock_data_dir(&config.data_dir)?;
        let system = forge_store::Environment::system();
        let environment = forge_store::Environment {
            progress: system.progress.clone().with_heartbeat(|| {
                if let Err(error) = process::extend_start() {
                    tracing::warn!(
                        error = %diagnostics::chain(&error),
                        "systemd may stop Forge for starting too slowly"
                    );
                }
            }),
            ..system
        };
        drop(
            forge_store::Store::open_restoring_in(&exclusive, environment).map_err(|source| {
                BindingError::Store {
                    dir: config.data_dir.clone(),
                    source: Box::new(source),
                }
            })?,
        );
    }
    let address = config.listen;
    let listener =
        StdListener::bind(address).map_err(|source| BindingError::Bind { address, source })?;
    listener
        .set_nonblocking(true)
        .map_err(|source| BindingError::NonBlocking { address, source })?;
    let id = artisan_api::ItemId::new().to_string();
    let directory = registry.root().join("binaries").join(&id);
    std::fs::create_dir(&directory).map_err(|source| BindingError::Folder {
        path: directory.clone(),
        source,
    })?;
    Ok(Binding {
        listener,
        lock,
        id,
        directory,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("open the runtime registry in {}", data.display())]
    Registry {
        data: PathBuf,
        #[source]
        source: RegistryFileError,
    },
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error(transparent)]
    Ready(#[from] ReadyError),
    #[error(transparent)]
    Healthy(#[from] HealthyError),
    #[error("listen for gateway control on loopback")]
    Control(#[source] std::io::Error),
    #[error("draw the gateway token")]
    Token(#[source] getrandom::Error),
    #[error("read the address of gateway {gateway}")]
    Address {
        gateway: String,
        #[source]
        source: std::io::Error,
    },
    #[error("record gateway {gateway}")]
    Record {
        gateway: String,
        #[source]
        source: RegistryFileError,
    },
    #[error("activate generation {generation}")]
    Activate {
        generation: String,
        #[source]
        source: PeerError,
    },
    #[error(transparent)]
    Notify(#[from] NotifyError),
    #[error("serve connections on gateway {gateway}")]
    Listener {
        gateway: String,
        #[source]
        source: std::io::Error,
    },
    #[error("accept a connection on gateway {gateway}")]
    Accept {
        gateway: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the maintenance of gateway {gateway} stopped")]
    Maintenance {
        gateway: String,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error(transparent)]
    Drain(#[from] DrainError),
    #[error("the control server of gateway {gateway} stopped")]
    ControlTask {
        gateway: String,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("serve the control endpoint of gateway {gateway}")]
    ControlServe {
        gateway: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ConnectionError {
    #[error("find the active generation")]
    Active(#[source] RegistryFileError),
    #[error("no generation is active")]
    NoActive,
    #[error("forward a connection to generation {backend}")]
    Forward {
        backend: String,
        #[source]
        source: crate::forwarded::ForwardError,
    },
}

impl ConnectionError {
    const fn reached_backend(&self) -> bool {
        match self {
            Self::Forward { source, .. } => source.reached_backend(),
            Self::Active(_) | Self::NoActive => false,
        }
    }
}

async fn forward(
    registry: Registry,
    client: TcpStream,
    peer: SocketAddr,
) -> Result<(), ConnectionError> {
    let active = registry
        .active()
        .map_err(ConnectionError::Active)?
        .ok_or(ConnectionError::NoActive)?;
    crate::forwarded::forward(client, peer, active.address)
        .await
        .map_err(|source| ConnectionError::Forward {
            backend: active.id,
            source,
        })
}

fn report(
    gateway: &str,
    wake: &Notify,
    finished: Result<Result<(), ConnectionError>, tokio::task::JoinError>,
) {
    match finished {
        Ok(Ok(())) => {}
        Ok(Err(error @ ConnectionError::Forward { .. })) if error.reached_backend() => {
            tracing::debug!(
                gateway,
                error = diagnostics::chain(&error),
                "gateway connection ended"
            );
        }
        Ok(Err(
            error @ (ConnectionError::Active(_)
            | ConnectionError::NoActive
            | ConnectionError::Forward { .. }),
        )) => {
            tracing::warn!(
                gateway,
                error = diagnostics::chain(&error),
                "gateway connection failed"
            );
            wake.notify_one();
        }
        Err(error) => {
            tracing::warn!(gateway, error = %error, "gateway connection task stopped");
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one gateway lifetime from binding to drain"
)]
pub(super) async fn serve(
    config: crate::ServeConfig,
    binary: PathBuf,
    handoff: Option<PathBuf>,
    log: diagnostics::Destination,
    stop: impl Future<Output = ()>,
) -> Result<(), ServeError> {
    tokio::pin!(stop);
    if futures::poll!(&mut stop).is_ready() {
        return Ok(());
    }
    let registry = Registry::open(&config.data_dir).map_err(|source| ServeError::Registry {
        data: config.data_dir.clone(),
        source,
    })?;
    let Binding {
        listener,
        lock,
        id,
        directory,
    } = binding(&config, &registry, handoff)?;
    let mut backend = process::spawn_backend(
        &binary,
        &config.data_dir,
        &log,
        &directory.join("backend.json"),
    )?;
    let member = match process::ready(&mut backend, &directory.join("backend.json")).await {
        Ok(member) => member,
        Err(error) => {
            if let Err(stopped) = process::terminate(&mut backend) {
                tracing::warn!(gateway = %id, error = diagnostics::chain(&stopped), "stop the backend that never became ready");
            }
            return Err(error.into());
        }
    };
    process::healthy(&member).await?;
    let control = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(ServeError::Control)?;
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(ServeError::Token)?;
    let address = |source| ServeError::Address {
        gateway: id.clone(),
        source,
    };
    let gateway = Gateway {
        id: id.clone(),
        pid: std::process::id(),
        control: format!("http://{}", control.local_addr().map_err(address)?),
        token: hex::encode(secret),
        address: listener.local_addr().map_err(address)?,
        binary,
        protocol: PROTOCOL,
        backend: member,
    };
    let stop_token = CancellationToken::new();
    let replaced = Arc::new(Notify::new());
    let wake = Arc::new(Notify::new());
    let service = Service {
        gateway: gateway.clone(),
        registry: registry.clone(),
        data: config.data_dir.clone(),
        log,
        listener: Arc::new(listener),
        lock: Arc::new(lock),
        replaced: Arc::clone(&replaced),
        wake: Arc::clone(&wake),
        stop: stop_token.clone(),
    };
    let control_task = start_control(control, service.clone());
    let record = |source| ServeError::Record {
        gateway: id.clone(),
        source,
    };
    if directory.join("handoff.json").exists() {
        write(
            &directory.join("ready.json"),
            &Ready {
                gateway: gateway.clone(),
            },
        )
        .map_err(record)?;
    } else {
        runtime::request(&gateway.backend, &Command::Activate)
            .await
            .map_err(|source| ServeError::Activate {
                generation: gateway.backend.id.clone(),
                source,
            })?;
        registry.promote(&gateway.backend).map_err(record)?;
        write(&registry.root().join("gateway.json"), &gateway).map_err(record)?;
        write(&config.data_dir.join("forge.pid"), &gateway.pid).map_err(record)?;
        process::notify(gateway.pid)?;
    }
    let tcp = service
        .listener
        .try_clone()
        .and_then(TcpListener::from_std)
        .map_err(|source| ServeError::Listener {
            gateway: id.clone(),
            source,
        })?;
    let maintenance_task = spawn_maintenance(
        service.clone(),
        stop_token.clone(),
        Backend::Serving(backend.release()),
    );
    let mut connections = JoinSet::new();
    let replaced = loop {
        tokio::select! {
            ()=&mut stop=>break false,
            ()=stop_token.cancelled()=>break false,
            ()=replaced.notified()=>break true,
            accepted=tcp.accept()=>match accepted {
                Ok((client, peer)) => {
                    connections.spawn(forward(registry.clone(), client, peer));
                }
                Err(source) => {
                    let error = ServeError::Accept { gateway: id.clone(), source };
                    tracing::warn!(gateway = %id, error = diagnostics::chain(&error), "the gateway keeps serving");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
            Some(finished)=connections.join_next(),if !connections.is_empty()=>report(&id, &wake, finished),
        }
    };
    drop(tcp);
    stop_token.cancel();
    let backend = maintenance_task
        .await
        .map_err(|source| ServeError::Maintenance {
            gateway: id.clone(),
            source,
        })?;
    drain(&service, replaced, &mut connections, backend).await?;
    control_task
        .await
        .map_err(|source| ServeError::ControlTask {
            gateway: id.clone(),
            source,
        })?
        .map_err(|source| ServeError::ControlServe {
            gateway: id.clone(),
            source,
        })?;
    Ok(())
}

const RETRY_AFTER: Duration = Duration::from_secs(10);

const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

enum Backend {
    Serving(Exited),
    Gone,
}

impl Backend {
    async fn exited(&mut self) {
        match self {
            Self::Serving(exited) => {
                exited.wait().await;
                *self = Self::Gone;
            }
            Self::Gone => std::future::pending().await,
        }
    }
}

enum Pass {
    Settled,
    Retiring,
    Upgrading,
}

enum Wait {
    Event,
    Retry,
    Release(oneshot::Receiver<std::io::Result<()>>),
}

impl Wait {
    async fn elapsed(self, gateway: &str) {
        match self {
            Self::Event => std::future::pending().await,
            Self::Retry => tokio::time::sleep(RETRY_AFTER).await,
            Self::Release(released) => match released.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(gateway, error = %error, "wait for the upgrade lock");
                    tokio::time::sleep(RETRY_AFTER).await;
                }
                Err(error) => {
                    tracing::warn!(gateway, error = %error, "the upgrade lock waiter stopped");
                    tokio::time::sleep(RETRY_AFTER).await;
                }
            },
        }
    }
}

fn upgrade_released(
    registry: &Registry,
) -> std::io::Result<oneshot::Receiver<std::io::Result<()>>> {
    let path = registry.root().join("upgrade.lock");
    let (released, waiting) = oneshot::channel();
    std::thread::Builder::new()
        .name("forge-upgrade-lock".into())
        .spawn(move || {
            let locked = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)
                .and_then(|lock| lock.lock_exclusive());
            if released.send(locked).is_err() {
                tracing::debug!(path = %path.display(), "the upgrade lock was released after maintenance moved on");
            }
        })?;
    Ok(waiting)
}

fn spawn_maintenance(
    maintenance: Service,
    maintenance_stop: CancellationToken,
    mut backend: Backend,
) -> tokio::task::JoinHandle<Backend> {
    tokio::spawn(async move {
        let gateway = maintenance.gateway.id.as_str();
        loop {
            let wait = match maintain(&maintenance, &mut backend).await {
                Ok(Pass::Settled) => Wait::Event,
                Ok(Pass::Retiring) => Wait::Retry,
                Ok(Pass::Upgrading) => match upgrade_released(&maintenance.registry) {
                    Ok(released) => Wait::Release(released),
                    Err(error) => {
                        tracing::warn!(gateway, error = %error, "start waiting for the upgrade lock");
                        Wait::Retry
                    }
                },
                Err(error) => {
                    tracing::warn!(
                        gateway,
                        backend = %maintenance.gateway.backend.id,
                        error = diagnostics::chain(&error),
                        "generation maintenance"
                    );
                    Wait::Retry
                }
            };
            tokio::select! {
                () = maintenance_stop.cancelled() => break,
                () = maintenance.wake.notified() => {}
                () = backend.exited() => {}
                () = wait.elapsed(gateway) => {}
            }
        }
        backend
    })
}

fn start_control(
    control: TcpListener,
    service: Service,
) -> tokio::task::JoinHandle<std::io::Result<()>> {
    let control_stop = service.stop.clone();
    let app = Router::new()
        .route("/health", get(health))
        .route("/upgrade", post(upgrade))
        .route("/stop", post(stop_gateway))
        .with_state(service);
    tokio::spawn(async move {
        axum::serve(control, app)
            .with_graceful_shutdown(control_stop.cancelled_owned())
            .await
    })
}

#[derive(Debug, thiserror::Error)]
pub enum DrainError {
    #[error("read the record of gateway {gateway}")]
    Record {
        gateway: String,
        #[source]
        source: RegistryFileError,
    },
    #[error("list the generations gateway {gateway} drains")]
    Peers {
        gateway: String,
        #[source]
        source: RegistryFileError,
    },
}

async fn request_logged(member: &runtime::Member, command: &Command) {
    if let Err(error) = runtime::request(member, command).await {
        tracing::debug!(generation = %member.id, error = diagnostics::chain(&error), "generation did not take the request");
    }
}

async fn drain(
    service: &Service,
    replaced: bool,
    connections: &mut JoinSet<Result<(), ConnectionError>>,
    mut backend: Backend,
) -> Result<(), DrainError> {
    let gateway = || service.gateway.id.clone();
    let peers = |source| DrainError::Peers {
        gateway: gateway(),
        source,
    };
    if !replaced {
        if read::<Gateway>(&service.registry.root().join("gateway.json"))
            .map_err(|source| DrainError::Record {
                gateway: gateway(),
                source,
            })?
            .is_some_and(|current| current.id == service.gateway.id)
        {
            let members = service.registry.peers().map_err(peers)?;
            for member in &members {
                request_logged(member, &Command::Drain).await;
            }
            for member in &members {
                request_logged(member, &Command::Stop).await;
            }
        } else {
            request_logged(&service.gateway.backend, &Command::Stop).await;
        }
    }
    if matches!(backend, Backend::Gone) {
        connections.abort_all();
    }
    loop {
        tokio::select! {
            finished = connections.join_next() => match finished {
                Some(finished) => report(&service.gateway.id, &service.wake, finished),
                None => break,
            },
            () = backend.exited() => connections.abort_all(),
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(super) enum MaintainError {
    #[error("read the gateway record")]
    Record(#[source] RegistryFileError),
    #[error("take the upgrade lock")]
    Lock(#[source] RegistryFileError),
    #[error("list the generations")]
    Peers(#[source] RegistryFileError),
    #[error("read the active generation")]
    Active(#[source] RegistryFileError),
    #[error("the active generation is missing")]
    ActiveMissing,
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error(transparent)]
    Ready(#[from] ReadyError),
    #[error(transparent)]
    Healthy(#[from] HealthyError),
    #[error("activate the restarted generation {generation}")]
    Activate {
        generation: String,
        #[source]
        source: PeerError,
    },
    #[error("record the restarted generation {generation}")]
    Promote {
        generation: String,
        #[source]
        source: RegistryFileError,
    },
    #[error("prune {}", path.display())]
    Prune {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read {} while pruning", path.display())]
    PruneRead {
        path: PathBuf,
        #[source]
        source: RegistryFileError,
    },
}

async fn maintain(service: &Service, backend: &mut Backend) -> Result<Pass, MaintainError> {
    let Some(_upgrade) = service
        .registry
        .upgrade_lock()
        .map_err(MaintainError::Lock)?
    else {
        return Ok(Pass::Upgrading);
    };
    if read::<Gateway>(&service.registry.root().join("gateway.json"))
        .map_err(MaintainError::Record)?
        .is_none_or(|current| current.id != service.gateway.id)
    {
        return Ok(Pass::Settled);
    }
    let members = service.registry.peers().map_err(MaintainError::Peers)?;
    let active = service
        .registry
        .active()
        .map_err(MaintainError::Active)?
        .ok_or(MaintainError::ActiveMissing)?;
    let active = if members.iter().any(|member| member.id == active.id) {
        active.id
    } else {
        restart(service, backend).await?
    };
    let mut retiring = false;
    for member in members.iter().filter(|member| member.id != active) {
        request_logged(member, &Command::Drain).await;
        request_logged(member, &Command::Retire).await;
        retiring = true;
    }
    prune(service, &members)?;
    Ok(if retiring {
        Pass::Retiring
    } else {
        Pass::Settled
    })
}

async fn restart(service: &Service, backend: &mut Backend) -> Result<String, MaintainError> {
    let ready = service
        .registry
        .root()
        .join("binaries")
        .join(&service.gateway.id)
        .join(format!("restart-{}.json", artisan_api::ItemId::new()));
    let mut child =
        process::spawn_backend(&service.gateway.binary, &service.data, &service.log, &ready)?;
    let member: runtime::Member = process::ready(&mut child, &ready).await?;
    process::healthy(&member).await?;
    runtime::request(&member, &Command::Activate)
        .await
        .map_err(|source| MaintainError::Activate {
            generation: member.id.clone(),
            source,
        })?;
    let promote = |source| MaintainError::Promote {
        generation: member.id.clone(),
        source,
    };
    service.registry.promote(&member).map_err(promote)?;
    let mut gateway = service.gateway.clone();
    gateway.backend = member.clone();
    write(&service.registry.root().join("gateway.json"), &gateway).map_err(promote)?;
    *backend = Backend::Serving(child.release());
    Ok(member.id)
}

fn prune(service: &Service, members: &[runtime::Member]) -> Result<(), MaintainError> {
    let failed = |path: &std::path::Path| {
        let path = path.to_owned();
        move |source| MaintainError::Prune { path, source }
    };
    let binaries = service.registry.root().join("binaries");
    let mut retired = Vec::new();
    for entry in std::fs::read_dir(&binaries).map_err(failed(&binaries))? {
        let entry = entry.map_err(failed(&binaries))?;
        let directory = entry.path();
        if !entry.file_type().map_err(failed(&directory))?.is_dir()
            || entry.file_name() == service.gateway.id.as_str()
        {
            continue;
        }
        let mut live = false;
        for metadata in std::fs::read_dir(&directory).map_err(failed(&directory))? {
            let path = metadata.map_err(failed(&directory))?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if (name == "backend.json" || name.starts_with("restart-"))
                && read::<runtime::Member>(&path)
                    .map_err(|source| MaintainError::PruneRead {
                        path: path.clone(),
                        source,
                    })?
                    .is_some_and(|candidate| members.iter().any(|member| member.id == candidate.id))
            {
                live = true;
                break;
            }
        }
        if !live {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(failed(&directory))?;
            retired.push((modified, directory));
        }
    }
    retired.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, directory) in retired.into_iter().skip(2) {
        std::fs::remove_dir_all(&directory).map_err(failed(&directory))?;
    }
    Ok(())
}
