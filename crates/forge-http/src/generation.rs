use artisan_api::Envelope;
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use forge_runs::{
    Hub,
    runtime::{
        Command, Member, Overlap, PROTOCOL, Phase, Registry, RegistryFileError, Reply, ReplyFailure,
    },
};
use futures::StreamExt as _;
use std::{collections::HashSet, time::Duration};
use tokio::net::TcpListener;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Debug, thiserror::Error)]
pub enum PrepareGenerationError {
    #[error("listen for generation control on loopback")]
    Bind(#[source] std::io::Error),
    #[error("read the generation control address")]
    Address(#[source] std::io::Error),
    #[error("draw the generation control token")]
    Token(#[source] getrandom::Error),
    #[error("register the generation serving {address}")]
    Register {
        address: std::net::SocketAddr,
        #[source]
        source: RegistryFileError,
    },
}

pub async fn prepare(
    registry: &Registry,
    address: std::net::SocketAddr,
) -> Result<(forge_runs::runtime::Generation, TcpListener), PrepareGenerationError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(PrepareGenerationError::Bind)?;
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(PrepareGenerationError::Token)?;
    let control = listener
        .local_addr()
        .map_err(PrepareGenerationError::Address)?;
    let generation = registry
        .register(Member {
            id: String::new(),
            control: format!("http://{control}"),
            token: hex::encode(secret),
            address,
            pid: std::process::id(),
            protocol: PROTOCOL,
            schema: forge_store::Store::schema_version(),
        })
        .map_err(|source| PrepareGenerationError::Register { address, source })?;
    Ok((generation, listener))
}

#[derive(Clone)]
struct Service {
    hub: Hub,
    stop: CancellationToken,
}

fn authorized(service: &Service, headers: &HeaderMap) -> bool {
    service.hub.generation().is_some_and(|generation| {
        headers.get("authorization").is_some_and(|header| {
            header.as_bytes() == format!("Bearer {}", generation.member.token).as_bytes()
        })
    })
}

async fn control(
    State(service): State<Service>,
    headers: HeaderMap,
    Json(command): Json<Command>,
) -> Response {
    if !authorized(&service, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let result = match command {
        Command::Retire => {
            if service
                .hub
                .generation()
                .is_some_and(forge_runs::runtime::Generation::draining)
                && service.hub.local_active_count() == 0
            {
                let stop = service.stop.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    stop.cancel();
                });
                Ok(serde_json::Value::Bool(true))
            } else {
                Ok(serde_json::Value::Bool(false))
            }
        }
        Command::Stop => {
            if let Some(generation) = service.hub.generation() {
                generation.set_phase(Phase::Draining);
            }
            let stop = service.stop.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                stop.cancel();
            });
            Ok(serde_json::Value::Null)
        }
        command @ (Command::Health
        | Command::Activate
        | Command::Drain
        | Command::Accept { .. }
        | Command::Answer { .. }
        | Command::Cancel { .. }
        | Command::Update { .. }
        | Command::ClearGoal { .. }
        | Command::EditorReply { .. }
        | Command::Snapshot) => service.hub.control(command).await,
    };
    Json(match result {
        Ok(value) => Reply::Ok(value),
        Err(error) => Reply::Error(ReplyFailure::Typed(error.failure())),
    })
    .into_response()
}

async fn live(State(service): State<Service>, headers: HeaderMap) -> Response {
    if !authorized(&service, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let receiver = service.hub.store().subscribe_local_live();
    let stream = futures::stream::unfold(receiver, |mut receiver| async move {
        let envelope = match receiver.recv().await {
            Ok(envelope) => envelope,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Envelope {
                seq: None,
                live_revision: 0,
                event: artisan_api::Event::Reset,
            },
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
        };
        let bytes = serde_json::to_vec(&envelope).map(|mut bytes| {
            bytes.push(b'\n');
            bytes
        });
        Some((bytes, receiver))
    });
    Body::from_stream(stream).into_response()
}

pub fn start(
    hub: &Hub,
    listener: TcpListener,
    stop: &CancellationToken,
    drain: &CancellationToken,
    tasks: &TaskTracker,
) {
    let service = Service {
        hub: hub.clone(),
        stop: drain.clone(),
    };
    let app = Router::new()
        .route("/control", post(control))
        .route("/live", get(live))
        .layer(DefaultBodyLimit::max(forge_store::MAX_BLOB_BYTES + 1024))
        .with_state(service);
    let server_stop = stop.clone();
    tasks.spawn(async move {
        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(server_stop.cancelled_owned())
            .await
        {
            tracing::error!(error = %error, "generation control stopped");
        }
    });
    tasks.spawn(synchronize(hub.clone(), stop.clone(), tasks.clone()));
}

async fn synchronize(hub: Hub, stop: CancellationToken, tasks: TaskTracker) {
    let Some(generation) = hub.generation().cloned() else {
        return;
    };
    let mut overlap = generation.watch();
    let mut cursor = hub.store().head();
    let mut peers = HashSet::new();
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    loop {
        if !overlap.borrow_and_update().overlapping() {
            tokio::select! {
                () = stop.cancelled() => break,
                watching = settled(&mut overlap, Overlap::overlapping) => if !watching { break },
            }
            interval.reset_immediately();
        }
        tokio::select! {
            () = stop.cancelled() => break,
            _tick = interval.tick() => {}
        }
        if let Err(error) = hub.store().synchronize(&mut cursor).await {
            tracing::warn!(
                generation = %generation.member.id,
                error = diagnostics::chain(&error),
                "synchronize generation store"
            );
        }
        if let Err(error) = hub.follow_peers().await {
            tracing::warn!(
                generation = %generation.member.id,
                error = diagnostics::chain(&error),
                "follow the other generations"
            );
        }
        hub.runtime_changed();
        let members = match generation.registry.list_peers().await {
            Ok(members) => members,
            Err(error) => {
                tracing::debug!(
                    generation = %generation.member.id,
                    error = diagnostics::chain(&error),
                    "list the generations to relay"
                );
                continue;
            }
        };
        peers.retain(|id| members.iter().any(|member| member.id == *id));
        for member in members {
            if member.id == generation.member.id || !peers.insert(member.id.clone()) {
                continue;
            }
            tasks.spawn(relay_peer(hub.clone(), member, stop.clone()));
        }
    }
}

async fn relay_peer(hub: Hub, member: Member, stop: CancellationToken) {
    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            result = relay(&hub, &member) => {
                if let Err(error) = result { tracing::debug!(generation = %member.id, error = diagnostics::chain(&error), "live relay"); }
            }
        }
        tokio::select! {
            () = stop.cancelled() => break,
            () = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        let Some(generation) = hub.generation() else {
            break;
        };
        if generation
            .registry
            .list_peers()
            .await
            .is_ok_and(|members| !members.iter().any(|candidate| candidate.id == member.id))
        {
            break;
        }
    }
}

pub fn start_primary_services(
    data: std::path::PathBuf,
    app: crate::routes::App,
    stop: &CancellationToken,
    tasks: &TaskTracker,
) {
    tasks.spawn(primary_services(data, app, stop.clone()));
}

async fn settled(
    overlap: &mut tokio::sync::watch::Receiver<Overlap>,
    condition: impl FnMut(&Overlap) -> bool,
) -> bool {
    overlap.wait_for(condition).await.is_ok()
}

async fn primary_services(
    data: std::path::PathBuf,
    app: crate::routes::App,
    stop: CancellationToken,
) {
    let Some(generation) = app.hub.generation().cloned() else {
        return;
    };
    let mut overlap = generation.watch();
    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            watching = settled(&mut overlap, |overlap| overlap.phase == Phase::Active) => if !watching { break },
        }
        let leadership = stop.child_token();
        let services = TaskTracker::new();
        let account = crate::account_service::run(
            data.clone(),
            app.clone(),
            leadership.clone(),
            services.clone(),
        );
        let retention = crate::remove_inactive_threads(
            app.hub.clone(),
            app.hub.store().clone(),
            leadership.clone(),
        );
        let service = async {
            tokio::join!(account, retention);
        };
        tokio::pin!(service);
        let watching = tokio::select! {
            () = &mut service => true,
            watching = settled(&mut overlap, |overlap| overlap.phase != Phase::Active) => {
                leadership.cancel();
                service.await;
                watching
            }
        };
        leadership.cancel();
        services.close();
        services.wait().await;
        if !watching {
            break;
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum RelayLiveError {
    #[error("build the client that follows generation {generation}")]
    Client {
        generation: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("reach the live events of generation {generation}")]
    Reach {
        generation: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("read the live events of generation {generation}")]
    Read {
        generation: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("a live frame of generation {generation} exceeds {bytes} bytes")]
    FrameTooLarge { generation: String, bytes: usize },
    #[error("decode a live event of generation {generation}")]
    Decode {
        generation: String,
        #[source]
        source: serde_json::Error,
    },
}

async fn relay(hub: &Hub, member: &Member) -> Result<(), RelayLiveError> {
    let generation = || member.id.clone();
    let reach = |source| RelayLiveError::Reach {
        generation: generation(),
        source,
    };
    let response = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .map_err(|source| RelayLiveError::Client {
            generation: generation(),
            source,
        })?
        .get(format!("{}/live", member.control))
        .bearer_auth(&member.token)
        .send()
        .await
        .map_err(reach)?
        .error_for_status()
        .map_err(reach)?;
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    hub.store().relay_live(artisan_api::Event::Reset);
    while let Some(chunk) = stream.next().await {
        pending.extend_from_slice(&chunk.map_err(|source| RelayLiveError::Read {
            generation: generation(),
            source,
        })?);
        if pending.len() > LIVE_FRAME_BYTES {
            return Err(RelayLiveError::FrameTooLarge {
                generation: generation(),
                bytes: LIVE_FRAME_BYTES,
            });
        }
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let envelope: Envelope = serde_json::from_slice(&pending[..end]).map_err(|source| {
                RelayLiveError::Decode {
                    generation: generation(),
                    source,
                }
            })?;
            hub.store().relay_live(envelope.event);
            pending.drain(..=end);
        }
    }
    Ok(())
}

const LIVE_FRAME_BYTES: usize = 8 * 1024 * 1024;
