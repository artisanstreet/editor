use super::{Gateway, Outcome, Ready, Upgrade, process, read, write};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use forge_runs::runtime::{self, Command, PROTOCOL, Registry};
use fs2::FileExt as _;
use std::{fs::File, net::TcpListener as StdListener, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Service {
    gateway: Gateway,
    registry: Registry,
    data: PathBuf,
    listener: Arc<StdListener>,
    lock: Arc<File>,
    replaced: Arc<Notify>,
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
    if let Ok(Some(current)) = read::<Gateway>(&service.registry.root().join("gateway.json"))
        && current.id == gateway.id
    {
        gateway = current;
    }

    match process::healthy(&gateway.backend).await {
        Ok(()) => Json(Ready { gateway }).into_response(),
        Err(error) => (StatusCode::SERVICE_UNAVAILABLE, format!("{error:#}")).into_response(),
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
    Json(match result {
        Ok(gateway) => Outcome::Ready(Box::new(gateway)),
        Err(error) => Outcome::Error(format!("{error:#}")),
    })
    .into_response()
}

async fn replace(service: &Service, upgrade: Upgrade) -> Result<Gateway> {
    let _upgrade = service
        .registry
        .upgrade_lock()?
        .context("another Forge upgrade is in progress")?;
    let current: Gateway = read(&service.registry.root().join("gateway.json"))?
        .context("active gateway is missing")?;
    ensure!(
        current.id == service.gateway.id,
        "this gateway was already replaced; retry the current gateway"
    );
    let replacement = process::candidate(
        &service.registry,
        &service.data,
        &upgrade.binary,
        service.listener.try_clone()?,
        service.lock.try_clone()?,
    )
    .await?;
    let previous = service
        .registry
        .active()?
        .context("active backend is missing")?;
    let commit = async {
        runtime::request(&replacement.backend, &Command::Activate).await?;
        process::notify(replacement.pid)?;
        write(&service.registry.root().join("gateway.json"), &replacement)?;
        write(&service.data.join("forge.pid"), &replacement.pid)?;
        service.registry.promote(&replacement.backend)?;
        Ok(())
    }
    .await;
    if let Err(error) = commit {
        service.registry.promote(&previous)?;
        write(&service.registry.root().join("gateway.json"), &current)?;
        write(&service.data.join("forge.pid"), &current.pid)?;
        let restored = process::notify(current.pid);
        super::stop_member(&replacement).await.ok();
        restored?;
        return Err(error);
    }
    runtime::request(&previous, &Command::Drain).await.ok();
    service.replaced.notify_one();
    Ok(replacement)
}

struct Binding {
    listener: StdListener,
    lock: File,
    id: String,
    directory: PathBuf,
}

fn binding(
    config: &crate::ServeConfig,
    registry: &Registry,
    handoff: Option<PathBuf>,
) -> Result<Binding> {
    let (listener, lock, id, directory) = if let Some(handoff) = handoff {
        let value: serde_json::Value =
            read(&handoff)?.context("handoff configuration is missing")?;
        let id = value["id"]
            .as_str()
            .context("handoff id missing")?
            .to_owned();
        let directory = handoff
            .parent()
            .context("handoff directory missing")?
            .to_path_buf();
        (
            process::listener()?,
            process::inherited_lock()?,
            id,
            directory,
        )
    } else {
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(registry.root().join("gateway.lock"))?;
        lock.try_lock_exclusive()
            .context("another Forge gateway is running")?;
        if registry.peers()?.is_empty() {
            let _exclusive = crate::lock_data_dir(&config.data_dir)?;
            drop(forge_store::Store::open(&config.data_dir)?);
        }
        let listener = StdListener::bind(config.listen)?;
        listener.set_nonblocking(true)?;
        let id = artisan_api::ItemId::new().to_string();
        let directory = registry.root().join("binaries").join(&id);
        std::fs::create_dir(&directory)?;
        (listener, lock, id, directory)
    };
    Ok(Binding {
        listener,
        lock,
        id,
        directory,
    })
}

pub async fn serve(
    config: crate::ServeConfig,
    handoff: Option<PathBuf>,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let registry = Registry::open(&config.data_dir)?;
    let binary = std::env::current_exe()?.canonicalize()?;
    let Binding {
        listener,
        lock,
        id,
        directory,
    } = binding(&config, &registry, handoff)?;
    let mut backend =
        process::spawn_backend(&binary, &config.data_dir, &directory.join("backend.json"))?;
    let member = match process::ready(&mut backend, &directory.join("backend.json")).await {
        Ok(member) => member,
        Err(error) => {
            process::terminate(&mut backend).ok();
            return Err(error);
        }
    };
    process::healthy(&member).await?;
    let control = TcpListener::bind("127.0.0.1:0").await?;
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret)?;
    let gateway = Gateway {
        id: id.clone(),
        pid: std::process::id(),
        control: format!("http://{}", control.local_addr()?),
        token: hex::encode(secret),
        address: listener.local_addr()?,
        binary,
        protocol: PROTOCOL,
        backend: member,
    };
    let stop_token = CancellationToken::new();
    let replaced = Arc::new(Notify::new());
    let service = Service {
        gateway: gateway.clone(),
        registry: registry.clone(),
        data: config.data_dir.clone(),
        listener: Arc::new(listener),
        lock: Arc::new(lock),
        replaced: Arc::clone(&replaced),
        stop: stop_token.clone(),
    };
    let control_task = start_control(control, service.clone());
    let inherited = directory.join("handoff.json").exists();
    if inherited {
        write(
            &directory.join("ready.json"),
            &Ready {
                gateway: gateway.clone(),
            },
        )?;
    } else {
        runtime::request(&gateway.backend, &Command::Activate).await?;
        registry.promote(&gateway.backend)?;
        write(&registry.root().join("gateway.json"), &gateway)?;
        write(&config.data_dir.join("forge.pid"), &gateway.pid)?;
        process::notify(gateway.pid)?;
    }
    let tcp = TcpListener::from_std(service.listener.try_clone()?)?;
    let maintenance = service.clone();
    let maintenance_stop = stop_token.clone();
    let maintenance_task = tokio::spawn(async move {
        loop {
            tokio::select! {()=maintenance_stop.cancelled()=>break,()=tokio::time::sleep(Duration::from_millis(250))=>{}}
            if let Err(error) = maintain(&maintenance).await {
                tracing::warn!("generation maintenance: {error:#}");
            }
        }
    });
    backend.release();
    let mut connections = JoinSet::new();
    tokio::pin!(stop);
    let replaced = loop {
        tokio::select! {
            ()=&mut stop=>break false,
            ()=stop_token.cancelled()=>break false,
            ()=replaced.notified()=>break true,
            accepted=tcp.accept()=>{
                let (mut client,_)=accepted?;let registry=registry.clone();
                connections.spawn(async move {
                    let active=registry.active()?.context("no active Forge generation")?;
                    let mut backend=TcpStream::connect(active.address).await?;
                    tokio::io::copy_bidirectional(&mut client,&mut backend).await?;
                    anyhow::Ok(())
                });
            }
            Some(result)=connections.join_next(),if !connections.is_empty()=>{if let Err(error)=result {tracing::debug!("gateway connection: {error}");}}
        }
    };
    drop(tcp);
    drain(&service, replaced, &mut connections).await?;
    stop_token.cancel();
    maintenance_task.await?;
    control_task.await??;
    Ok(())
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

async fn drain(
    service: &Service,
    replaced: bool,
    connections: &mut JoinSet<Result<()>>,
) -> Result<()> {
    if !replaced {
        if read::<Gateway>(&service.registry.root().join("gateway.json"))?
            .is_some_and(|current| current.id == service.gateway.id)
        {
            let peers = service.registry.peers()?;
            for member in &peers {
                runtime::request(member, &Command::Drain).await.ok();
            }
            for member in &peers {
                runtime::request(member, &Command::Stop).await.ok();
            }
        } else {
            runtime::request(&service.gateway.backend, &Command::Stop)
                .await
                .ok();
        }
    }
    while !connections.is_empty() {
        tokio::select! {
            _result = connections.join_next() => {},
            () = tokio::time::sleep(Duration::from_millis(250)) => {
                if !service.registry.peers()?.iter().any(|member| member.id == service.gateway.backend.id) {
                    connections.abort_all();
                }
            }
        }
    }
    Ok(())
}

async fn maintain(service: &Service) -> Result<()> {
    if read::<Gateway>(&service.registry.root().join("gateway.json"))?
        .is_none_or(|current| current.id != service.gateway.id)
    {
        return Ok(());
    }
    let Some(_upgrade) = service.registry.upgrade_lock()? else {
        return Ok(());
    };
    let members = service.registry.peers()?;
    let active = service
        .registry
        .active()?
        .context("active generation missing")?;
    if !members.iter().any(|member| member.id == active.id) {
        let ready = service
            .registry
            .root()
            .join("binaries")
            .join(&service.gateway.id)
            .join(format!("restart-{}.json", artisan_api::ItemId::new()));
        let mut child = process::spawn_backend(&service.gateway.binary, &service.data, &ready)?;
        let member = process::ready(&mut child, &ready).await?;
        process::healthy(&member).await?;
        runtime::request(&member, &Command::Activate).await?;
        service.registry.promote(&member)?;
        let mut gateway = service.gateway.clone();
        gateway.backend = member;
        write(&service.registry.root().join("gateway.json"), &gateway)?;
        child.release();
        return Ok(());
    }
    for member in &members {
        if member.id == active.id {
            continue;
        }
        runtime::request(member, &Command::Drain).await.ok();
        runtime::request(member, &Command::Retire).await.ok();
    }
    prune(service, &members)?;
    Ok(())
}

fn prune(service: &Service, members: &[runtime::Member]) -> Result<()> {
    let mut retired = Vec::new();
    for entry in std::fs::read_dir(service.registry.root().join("binaries"))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || entry.file_name() == service.gateway.id.as_str() {
            continue;
        }
        let directory = entry.path();
        let mut live = false;
        for metadata in std::fs::read_dir(&directory)? {
            let path = metadata?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if (name == "backend.json" || name.starts_with("restart-"))
                && read::<runtime::Member>(&path)?
                    .is_some_and(|candidate| members.iter().any(|member| member.id == candidate.id))
            {
                live = true;
                break;
            }
        }
        if !live {
            retired.push((entry.metadata()?.modified()?, directory));
        }
    }
    retired.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, directory) in retired.into_iter().skip(2) {
        std::fs::remove_dir_all(directory)?;
    }
    Ok(())
}
