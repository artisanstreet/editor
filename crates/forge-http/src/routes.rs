//! JSON routes. See `artisan_api::request` for the list.

use crate::events;
use anyhow::Context as _;
use artisan_api::{
    Answer, ApiError, BlobHash, BlobRef, CreateProject, CreateThread, DirEntry, DirListing,
    EngineId, EngineStatus, ErrorCode, Item, ItemId, ItemsPage, ItemsQuery, PairRequest,
    PairResponse, Project, ProjectId, SendMessage, State, Thread, ThreadId, UpdateThread,
};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State as AppState};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use forge_runs::Hub;
use serde::Deserialize;

#[derive(Clone)]
pub(crate) struct App {
    pub hub: Hub,
    pub forge_version: String,
    pub shutdown: tokio_util::sync::CancellationToken,
    pub tasks: tokio_util::task::TaskTracker,
    pub repositories: std::sync::Arc<crate::repository::Repositories>,
}

/// The connection's client certificate (base64 DER).
#[derive(Clone)]
pub(crate) struct Client {
    pub certificate: String,
}

pub(crate) fn router(app: App) -> Router {
    let authorized = Router::new()
        .route("/state", get(state))
        .route("/events", get(events::stream))
        .route("/projects", post(create_project))
        .route("/projects/{id}", delete(remove_project))
        .route("/projects/{id}/icon", get(project_icon))
        .route("/projects/{id}/repository", get(project_repository))
        .route("/threads", post(create_thread).get(threads_page))
        .route("/threads/{id}", patch(update_thread).get(thread))
        .route("/threads/{id}/settle", post(settle_thread))
        .route("/threads/{id}/unread", post(mark_unread))
        .route("/threads/{id}/items", get(items))
        .route("/threads/{id}/agents/{agent}/items", get(agent_items))
        .route("/threads/{id}/inspector", get(inspector))
        .route("/threads/{id}/resource", get(resource))
        .route("/threads/{id}/messages", post(send_message))
        .route("/threads/{id}/cancel", post(cancel))
        .route("/items/{id}/answer", post(answer))
        .route("/items/{id}", get(item))
        .route("/content/{id}", get(content_range))
        .route("/content/{id}/index", get(source_index))
        .route("/content/{id}/records", get(collection_range))
        .route("/settings/{key}", put(set_setting))
        .route("/mcp", patch(mutate_mcp))
        .route("/engines/{engine}/default-model", put(set_default_model))
        .route("/drafts/{scope}", get(draft).put(set_draft))
        .route("/blobs", put(put_blob))
        .route("/blobs/{hash}", get(get_blob))
        .route("/engines", get(engines))
        .route("/engines/{engine}/commands", post(engine_action))
        .route("/skills", get(crate::extensions::skills))
        .route("/skills/catalog", get(crate::extensions::catalog))
        .route(
            "/skills/{id}/{action}",
            post(crate::extensions::skill_action),
        )
        .route(
            "/prompt",
            get(crate::extensions::prompt).put(crate::extensions::save_prompt),
        )
        .route("/prompt/resolve", post(crate::extensions::resolve_prompt))
        .route("/fs/list", get(list_dir))
        .layer(middleware::from_fn_with_state(app.clone(), authorize));
    Router::new()
        .route("/pair", post(pair))
        .merge(authorized)
        .layer(DefaultBodyLimit::max(forge_store::MAX_BLOB_BYTES + 1024))
        .with_state(app)
}

/// Only paired devices get past here.
async fn authorize(
    AppState(app): AppState<App>,
    Extension(client): Extension<Client>,
    mut request: Request,
    next: Next,
) -> Response {
    let store = app.hub.store();
    match store.is_authorized(&client.certificate).await {
        Ok(Some(device)) => {
            request.extensions_mut().insert(device);
            next.run(request).await
        }
        Ok(None) => error(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "this device is not paired with Forge",
        ),
        Err(failure) => internal(&failure),
    }
}

pub(crate) struct Failure(anyhow::Error, StatusCode, ErrorCode);

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        let (status, code) = if error
            .downcast_ref::<forge_store::ProjectNotFound>()
            .is_some()
        {
            (StatusCode::NOT_FOUND, ErrorCode::NotFound)
        } else if error.downcast_ref::<forge_runs::Refused>().is_some() {
            (StatusCode::CONFLICT, ErrorCode::Conflict)
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal)
        };
        Self(error, status, code)
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        if self.1 == StatusCode::INTERNAL_SERVER_ERROR {
            internal(&self.0)
        } else {
            error(self.1, self.2, &format!("{:#}", self.0))
        }
    }
}

fn not_found(what: impl std::fmt::Display) -> Failure {
    Failure(
        anyhow::anyhow!("{what} not found"),
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
    )
}

fn bad_request(error: anyhow::Error) -> Failure {
    Failure(error, StatusCode::BAD_REQUEST, ErrorCode::BadRequest)
}

fn error(status: StatusCode, code: ErrorCode, message: &str) -> Response {
    (
        status,
        Json(ApiError {
            code,
            message: message.to_owned(),
        }),
    )
        .into_response()
}

fn internal(failure: &anyhow::Error) -> Response {
    tracing::error!("request failed: {failure:#}");
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::Internal,
        &format!("{failure:#}"),
    )
}

pub(crate) type Result<T> = std::result::Result<T, Failure>;

async fn state(AppState(app): AppState<App>) -> Result<Json<State>> {
    let snapshot = app.hub.store().state().await?;
    let engines = EngineId::ALL
        .into_iter()
        .map(|engine| app.hub.engines().status(engine))
        .collect();
    let waiting = app.hub.store().items_awaiting_user().await?;
    Ok(Json(State {
        waiting,
        account: Some(account()),
        head: snapshot.head,
        forge_version: app.forge_version.clone(),
        protocol: artisan_api::PROTOCOL_VERSION,
        projects: snapshot.projects,
        threads: snapshot.threads,
        threads_next: snapshot.threads_next,
        runs: snapshot.runs,
        settings: snapshot.settings,
        engines,
    }))
}

/// The user Forge runs as and its host's name, read once.
fn account() -> artisan_api::Account {
    static ACCOUNT: std::sync::OnceLock<artisan_api::Account> = std::sync::OnceLock::new();
    ACCOUNT
        .get_or_init(|| {
            let user = ["USER", "LOGNAME", "USERNAME"]
                .into_iter()
                .filter_map(|name| std::env::var(name).ok())
                .map(|name| name.trim().to_owned())
                .find(|name| !name.is_empty())
                .unwrap_or_else(|| "Artisan".to_owned());
            let host = gethostname::gethostname()
                .to_string_lossy()
                .trim()
                .to_owned();
            let host = if host.is_empty() {
                "This host".to_owned()
            } else {
                host
            };
            artisan_api::Account { user, host }
        })
        .clone()
}

async fn create_project(
    AppState(app): AppState<App>,
    Json(request): Json<CreateProject>,
) -> Result<Json<Project>> {
    let path = std::path::Path::new(&request.path);
    if !path.is_dir() {
        return Err(bad_request(anyhow::anyhow!(
            "{} is not a directory on this host",
            request.path
        )));
    }
    Ok(Json(app.hub.store().create_project(request).await?))
}

async fn remove_project(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let id = ProjectId(id);
    app.hub
        .store()
        .remove_project(&id)
        .await?
        .ok_or_else(|| not_found(format!("project {id}")))?;
    Ok(StatusCode::NO_CONTENT)
}

/// The project's own icon file, stored as a blob; `null` when it has none.
async fn project_icon(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
) -> Result<Json<Option<BlobRef>>> {
    const CANDIDATES: [&str; 5] = [
        ".artisan/icon.png",
        ".artisan/icon.webp",
        "icon.png",
        "favicon.png",
        "public/favicon.png",
    ];
    let id = ProjectId(id);
    let project = app
        .hub
        .store()
        .project(&id)
        .await?
        .ok_or_else(|| not_found(format!("project {id}")))?;
    for candidate in CANDIDATES {
        let path = std::path::Path::new(&project.path).join(candidate);
        let Some(bytes) = crate::repository::icon_file(&path).await else {
            continue;
        };
        match app.hub.store().put_image(bytes).await {
            Ok(blob) => return Ok(Json(Some(blob))),
            Err(error) => {
                tracing::info!(project = %id, path = %path.display(), "unusable project icon: {error:#}");
            }
        }
    }
    // Else the repository owner's avatar, as its host shows it.
    let root = std::path::PathBuf::from(&project.path);
    let avatar = app.repositories.avatar(&root).await;
    if let Some(bytes) = avatar {
        match app.hub.store().put_image(bytes).await {
            Ok(blob) => return Ok(Json(Some(blob))),
            Err(error) => {
                tracing::info!(project = %id, "unusable repository avatar: {error:#}");
            }
        }
    }
    Ok(Json(None))
}

/// The project's git branch and remote page; `null` outside a repository.
async fn project_repository(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
) -> Result<Json<Option<artisan_api::ProjectRepository>>> {
    let id = ProjectId(id);
    let project = app
        .hub
        .store()
        .project(&id)
        .await?
        .ok_or_else(|| not_found(format!("project {id}")))?;
    let path = std::path::PathBuf::from(project.path);
    let repository = app.repositories.read(&path).await;
    Ok(Json(repository))
}

async fn create_thread(
    AppState(app): AppState<App>,
    Json(request): Json<CreateThread>,
) -> Result<Json<Thread>> {
    Ok(Json(app.hub.store().create_thread(request).await?))
}

async fn threads_page(
    AppState(app): AppState<App>,
    Query(query): Query<artisan_api::ThreadsQuery>,
) -> Result<Json<artisan_api::ThreadsPage>> {
    Ok(Json(
        app.hub
            .store()
            .threads_page(query)
            .await
            .map_err(bad_request)?,
    ))
}

async fn thread(AppState(app): AppState<App>, Path(id): Path<ThreadId>) -> Result<Json<Thread>> {
    app.hub
        .store()
        .thread(&id)
        .await?
        .map(Json)
        .ok_or_else(|| not_found("thread not found"))
}

async fn update_thread(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Json(update): Json<UpdateThread>,
) -> Result<Json<Thread>> {
    let id = ThreadId(id);
    let thread = app.hub.store().update_thread(&id, update).await?;
    Ok(Json(
        thread.ok_or_else(|| not_found(format!("thread {id}")))?,
    ))
}

async fn settle_thread(
    AppState(app): AppState<App>,
    Path(id): Path<ThreadId>,
    Json(request): Json<artisan_api::SettleThread>,
) -> Result<Json<Thread>> {
    app.hub
        .store()
        .settle_thread(&id, request.observed_seq)
        .await?
        .map(Json)
        .ok_or_else(|| not_found("thread not found"))
}

async fn mark_unread(
    AppState(app): AppState<App>,
    Path(id): Path<ThreadId>,
) -> Result<Json<Thread>> {
    app.hub
        .store()
        .mark_unread(&id)
        .await?
        .map(Json)
        .ok_or_else(|| not_found("thread not found"))
}

async fn items(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<ItemsPage>> {
    Ok(Json(
        app.hub
            .store()
            .transcript_page(&ThreadId(id), None, query)
            .await?,
    ))
}

async fn agent_items(
    AppState(app): AppState<App>,
    Path((thread, agent)): Path<(ThreadId, ItemId)>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<ItemsPage>> {
    let owner = app
        .hub
        .store()
        .item(&agent)
        .await?
        .ok_or_else(|| not_found("agent not found"))?;
    if owner.thread_id != thread || !matches!(owner.kind, artisan_api::ItemKind::Agent { .. }) {
        return Err(not_found("agent not found in this thread"));
    }
    Ok(Json(
        app.hub
            .store()
            .transcript_page(&thread, Some(&agent), query)
            .await?,
    ))
}

async fn inspector(
    AppState(app): AppState<App>,
    Path(id): Path<ThreadId>,
) -> Result<Json<artisan_api::InspectorSummary>> {
    Ok(Json(app.hub.store().inspector(&id).await?))
}

async fn source_index(
    AppState(app): AppState<App>,
    Path(id): Path<artisan_api::ContentId>,
    Query(query): Query<artisan_api::SourceIndexQuery>,
) -> Result<Json<artisan_api::SourceIndexRange>> {
    app.hub
        .store()
        .source_index(&id, query)
        .await
        .map_err(bad_request)?
        .map(Json)
        .ok_or_else(|| not_found("text source"))
}

async fn content_range(
    AppState(app): AppState<App>,
    Path(id): Path<artisan_api::ContentId>,
    Query(query): Query<artisan_api::ContentQuery>,
) -> Result<Json<artisan_api::ContentRange>> {
    if !id.is_valid()
        || !query
            .limit
            .is_none_or(|limit| (4..=artisan_api::CONTENT_RANGE_BYTES).contains(&limit))
    {
        return Err(bad_request(anyhow::anyhow!(
            "invalid source identity or range limit"
        )));
    }
    app.hub
        .store()
        .content_range(&id, query)
        .await
        .map_err(bad_request)?
        .map(Json)
        .ok_or_else(|| not_found("text source"))
}

async fn collection_range(
    AppState(app): AppState<App>,
    Path(id): Path<artisan_api::ContentId>,
    Query(query): Query<artisan_api::CollectionQuery>,
) -> Result<Json<artisan_api::CollectionRange<serde_json::Value>>> {
    if !id.is_valid()
        || !query
            .limit
            .is_none_or(|limit| (1..=artisan_api::COLLECTION_RANGE_ENTRIES).contains(&limit))
    {
        return Err(bad_request(anyhow::anyhow!(
            "invalid collection identity or range limit"
        )));
    }
    app.hub
        .store()
        .collection_range(&id, query)
        .await
        .map_err(bad_request)?
        .map(Json)
        .ok_or_else(|| not_found("canonical collection"))
}

async fn item(AppState(app): AppState<App>, Path(id): Path<ItemId>) -> Result<Json<Item>> {
    app.hub
        .store()
        .item(&id)
        .await?
        .map(Json)
        .ok_or_else(|| not_found("item not found"))
}

#[derive(Deserialize)]
struct ResourceQuery {
    path: String,
    #[serde(default)]
    from_home: bool,
}

/// Relative agent resources resolve on their Forge, never on the paired client.
async fn resource(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Query(query): Query<ResourceQuery>,
) -> Result<Bytes> {
    use tokio::io::AsyncReadExt as _;
    let thread = app
        .hub
        .store()
        .thread(&ThreadId(id.clone()))
        .await?
        .ok_or_else(|| not_found(format!("thread {id}")))?;
    let path = if query
        .path
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file://"))
    {
        url::Url::parse(&query.path)
            .map_err(|error| bad_request(error.into()))?
            .to_file_path()
            .map_err(|()| bad_request(anyhow::anyhow!("Invalid file URI for this Forge host.")))?
    } else {
        std::path::PathBuf::from(&query.path)
    };
    let path = if query.from_home {
        dirs::home_dir()
            .ok_or_else(|| bad_request(anyhow::anyhow!("Forge has no home directory")))?
            .join(path)
    } else if path.is_absolute() {
        path
    } else {
        let project = app
            .hub
            .store()
            .project(&thread.project_id)
            .await?
            .ok_or_else(|| not_found(format!("project {}", thread.project_id)))?;
        std::path::Path::new(&project.path).join(path)
    };
    const LIMIT: usize = 32 * 1024 * 1024;
    // Reject directories, devices and FIFOs before open. Opening a FIFO can
    // otherwise wait indefinitely for a writer before the byte limit applies.
    let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            not_found(query.path.clone())
        } else {
            Failure::from(anyhow::Error::from(error))
        }
    })?;
    if !metadata.is_file() || metadata.len() > LIMIT as u64 {
        return Err(bad_request(anyhow::anyhow!(
            "resource must be a regular file no larger than 32 MiB"
        )));
    }
    let file = tokio::fs::File::open(&path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            not_found(query.path)
        } else {
            Failure::from(anyhow::Error::from(error))
        }
    })?;
    let metadata = file.metadata().await.map_err(anyhow::Error::from)?;
    if !metadata.is_file() || metadata.len() > LIMIT as u64 {
        return Err(bad_request(anyhow::anyhow!(
            "resource must be a regular file no larger than 32 MiB"
        )));
    }
    let mut bytes = Vec::new();
    file.take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(anyhow::Error::from)?;
    if bytes.len() > LIMIT {
        return Err(bad_request(anyhow::anyhow!("resource exceeds 32 MiB")));
    }
    Ok(Bytes::from(bytes))
}

async fn send_message(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Json(message): Json<SendMessage>,
) -> Result<Json<Item>> {
    let id = ThreadId(id);
    match app.hub.send_message(&id, message).await {
        Ok(item) => Ok(Json(item)),
        Err(error) => match error.downcast_ref::<forge_store::SendRejected>() {
            Some(forge_store::SendRejected::ThreadNotFound(_)) => {
                Err(not_found(format!("thread {id}")))
            }
            Some(
                forge_store::SendRejected::IdConflict(_)
                | forge_store::SendRejected::Payload(_)
                | forge_store::SendRejected::MissingBlob(_),
            ) => Err(bad_request(error)),
            None => Err(error.into()),
        },
    }
}

async fn cancel(AppState(app): AppState<App>, Path(id): Path<String>) -> Result<StatusCode> {
    app.hub.cancel(&ThreadId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn answer(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Json(answer): Json<Answer>,
) -> Result<StatusCode> {
    app.hub
        .answer(&ItemId(id), answer)
        .await
        .map_err(|error| Failure(error, StatusCode::CONFLICT, ErrorCode::Conflict))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_setting(
    AppState(app): AppState<App>,
    Path(key): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Result<StatusCode> {
    app.hub.store().set_setting(key, value).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mutate_mcp(
    AppState(app): AppState<App>,
    Json(mutation): Json<artisan_api::McpMutation>,
) -> Result<Json<Vec<artisan_api::McpServer>>> {
    app.hub
        .store()
        .mutate_mcp(mutation)
        .await
        .map(Json)
        .map_err(bad_request)
}

async fn set_default_model(
    AppState(app): AppState<App>,
    Path(engine): Path<EngineId>,
    Json(id): Json<String>,
) -> Result<Json<artisan_api::EngineConfig>> {
    let model = app
        .hub
        .engines()
        .status(engine)
        .models
        .iter()
        .find(|model| model.id == id)
        .cloned()
        .ok_or_else(|| bad_request(anyhow::anyhow!("model {id} is unavailable")))?;
    Ok(Json(
        app.hub.store().set_default_model(engine, model).await?,
    ))
}

async fn draft(
    AppState(app): AppState<App>,
    Path(scope): Path<String>,
) -> Result<Json<artisan_api::DraftSnapshot>> {
    Ok(Json(app.hub.store().draft(&scope).await?))
}

async fn set_draft(
    AppState(app): AppState<App>,
    Path(scope): Path<String>,
    Json(request): Json<artisan_api::WriteDraft>,
) -> Result<Json<artisan_api::DraftWriteResult>> {
    Ok(Json(app.hub.store().set_draft(scope, request).await?))
}

async fn put_blob(AppState(app): AppState<App>, body: Bytes) -> Result<Json<artisan_api::BlobRef>> {
    app.hub
        .store()
        .put_blob(body.to_vec())
        .await
        .map(Json)
        .map_err(|error| {
            if error.downcast_ref::<forge_store::BlobRejected>().is_some() {
                bad_request(error)
            } else {
                Failure::from(error)
            }
        })
}

async fn get_blob(AppState(app): AppState<App>, Path(hash): Path<String>) -> Result<Response> {
    let hash = BlobHash(hash);
    let path = app
        .hub
        .store()
        .blob_path(&hash)
        .ok_or_else(|| not_found(format!("blob {hash}")))?;
    let bytes = tokio::fs::read(&path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/webp"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        bytes,
    )
        .into_response())
}

async fn engines(AppState(app): AppState<App>) -> Json<Vec<EngineStatus>> {
    Json(
        EngineId::ALL
            .into_iter()
            .map(|engine| app.hub.engines().status(engine))
            .collect(),
    )
}

/// Executes a typed engine command; required arguments belong to its variant.
async fn engine_action(
    AppState(app): AppState<App>,
    Path(engine): Path<EngineId>,
    Json(command): Json<artisan_api::EngineCommand>,
) -> Result<Json<EngineStatus>> {
    let engines = app.hub.engines().clone();
    use artisan_api::EngineCommand as Command;
    if let Command::Use { version } = &command {
        forge_engines::Engines::validate_version(engine, version).map_err(bad_request)?;
    }
    match command {
        Command::Install {} | Command::Update {} | Command::Rollback {} | Command::Use { .. } => {
            let stop = app.shutdown.clone();
            let hub = app.hub.clone();
            app.tasks.spawn(async move {
                let label = command.as_str();
                let result = tokio::select! {
                    () = stop.cancelled() => return,
                    result = async { match command {
                        Command::Install {} => engines.install(engine).await.map(drop),
                        Command::Update {} => engines.update(engine).await.map(drop),
                        Command::Rollback {} => engines.rollback(engine).await.map(drop),
                        Command::Use { version } => engines.use_version(engine, &version).await,
                        _ => unreachable!("only installation commands spawn a worker"),
                    } } => result,
                };
                match result {
                    Ok(()) => {
                        if let Err(error) = hub.reconcile_global_prompt().await {
                            tracing::warn!(%engine, "reconcile prompt: {error:#}");
                        }
                        if let Err(error) = hub.reconcile_skills().await {
                            tracing::warn!(%engine, "reconcile skills: {error:#}");
                        }
                        engines.refresh(engine).await;
                    }
                    Err(error) => {
                        tracing::warn!(%engine, action = label, "engine command failed: {error:#}")
                    }
                }
            });
        }
        Command::Login {} => engines.login(engine).map_err(bad_request)?,
        Command::LoginCode { code } => engines
            .submit_login_code(engine, &code)
            .await
            .map_err(bad_request)?,
        Command::CancelLogin {} => engines.cancel_login(engine),
        Command::Logout {} => engines.logout(engine).map_err(bad_request)?,
        Command::Refresh {} => engines.refresh(engine).await,
        Command::Versions {} => {
            engines.list_versions(engine).await.map_err(bad_request)?;
        }
    }
    Ok(Json(app.hub.engines().status(engine)))
}

#[derive(Deserialize)]
struct ListQuery {
    path: Option<String>,
}

/// Directory listing for picking a project folder on this host.
async fn list_dir(Query(query): Query<ListQuery>) -> Result<Json<DirListing>> {
    static READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let path = match query.path {
        Some(path) if !path.is_empty() => std::path::PathBuf::from(path),
        _ => dirs::home_dir().ok_or_else(|| {
            bad_request(anyhow::anyhow!(
                "host home directory unavailable; provide a path"
            ))
        })?,
    };
    let permit = READS.acquire().await.map_err(|error| {
        Failure(
            error.into(),
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
        )
    })?;
    tokio::task::spawn_blocking(move || {
        // Keep admission until the blocking job actually exits, including client cancellation.
        let _permit = permit;
        read_directory(&path).map(Json).map_err(bad_request)
    })
    .await
    .map_err(|error| {
        Failure(
            error.into(),
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
        )
    })?
}

fn read_directory(path: &std::path::Path) -> anyhow::Result<DirListing> {
    let reader = std::fs::read_dir(path).with_context(|| format!("list {}", path.display()))?;
    let mut entries = Vec::new();
    let mut bytes = 0usize;
    for entry in reader {
        let entry = entry.with_context(|| format!("read entry in {}", path.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let entry_path = entry.path().to_string_lossy().into_owned();
        bytes = bytes
            .saturating_add(name.len())
            .saturating_add(entry_path.len());
        anyhow::ensure!(
            entries.len() < 32_768 && bytes <= 4 * 1024 * 1024,
            "directory listing exceeds 32768 entries or 4 MiB"
        );
        let is_dir = entry
            .file_type()
            .with_context(|| format!("inspect {entry_path}"))?
            .is_dir();
        let sort_key = name.to_lowercase();
        entries.push((
            DirEntry {
                name,
                path: entry_path,
                is_dir,
            },
            sort_key,
        ));
    }
    entries.sort_by(|(a, ak), (b, bk)| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| ak.cmp(bk))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(DirListing {
        parent: path.parent().map(|parent| parent.display().to_string()),
        path: path.display().to_string(),
        entries: entries.into_iter().map(|(entry, _)| entry).collect(),
    })
}

/// Authorizes the connection's certificate when the code is right.
async fn pair(
    AppState(app): AppState<App>,
    Extension(client): Extension<Client>,
    Json(request): Json<PairRequest>,
) -> Result<Json<PairResponse>> {
    let store = app.hub.store();
    let Some(device_id) = store
        .pair_device(
            crate::hash_code(&request.code),
            request.device_name,
            client.certificate,
        )
        .await?
    else {
        return Err(Failure(
            anyhow::anyhow!("that pairing code is wrong or expired"),
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
        ));
    };
    Ok(Json(PairResponse { device_id }))
}
