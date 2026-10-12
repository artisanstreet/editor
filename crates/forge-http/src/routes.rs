//! JSON routes. See `artisan_api::request` for the list.

use crate::events;
use crate::links::{self, LinkTitles};
use anyhow::Context as _;
use artisan_api::{
    Answer, ApiError, BlobHash, BlobRef, CreateProject, CreateThread, DirEntry, DirListing,
    EngineId, EngineStatus, ErrorCode, Item, ItemId, ItemsPage, ItemsQuery, LinkTitle,
    LinkTitleQuery, PairRequest, PairResponse, Project, ProjectId, SendMessage, State, Thread,
    ThreadId, UpdateThread,
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
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct App {
    pub hub: Hub,
    pub forge_version: String,
    pub links: Arc<LinkTitles>,
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
        .route("/threads", post(create_thread))
        .route("/threads/{id}", patch(update_thread))
        .route("/threads/{id}/items", get(items))
        .route("/threads/{id}/messages", post(send_message))
        .route("/threads/{id}/cancel", post(cancel))
        .route("/items/{id}/answer", post(answer))
        .route("/settings/{key}", put(set_setting))
        .route("/drafts/{scope}", get(draft).put(set_draft))
        .route("/blobs", put(put_blob))
        .route("/blobs/{hash}", get(get_blob))
        .route("/engines", get(engines))
        .route("/engines/{engine}/{action}", post(engine_action))
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
        .route("/links/title", get(link_title))
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
    request: Request,
    next: Next,
) -> Response {
    let store = app.hub.store();
    match store.is_authorized(&client.certificate).await {
        Ok(Some(device)) => {
            let _ = store.touch_device(&device).await;
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
        Self(
            error,
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
        )
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

type Result<T> = std::result::Result<T, Failure>;

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
                .find_map(|name| std::env::var(name).ok())
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .or_else(login_name)
                .unwrap_or_else(|| "Artisan".to_owned());
            let host = std::fs::read_to_string("/etc/hostname")
                .ok()
                .or_else(|| std::env::var("HOSTNAME").ok())
                .or_else(|| std::env::var("COMPUTERNAME").ok())
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "This host".to_owned());
            artisan_api::Account { user, host }
        })
        .clone()
}

/// The process owner's login name from the password file, for a Forge
/// started without `USER` (a service manager or a bare spawn).
#[cfg(unix)]
fn login_name() -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;
    let uid = std::fs::metadata("/proc/self").ok()?.uid().to_string();
    std::fs::read_to_string("/etc/passwd")
        .ok()?
        .lines()
        .map(|line| line.split(':').collect::<Vec<_>>())
        .find(|fields| fields.get(2) == Some(&uid.as_str()))
        .and_then(|fields| fields.first().map(|name| (*name).to_owned()))
        .filter(|name| !name.is_empty())
}

#[cfg(not(unix))]
fn login_name() -> Option<String> {
    None
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
        let Ok(bytes) = tokio::fs::read(&path).await else {
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
    let repository = tokio::task::spawn_blocking(move || crate::repository::read(&root))
        .await
        .context("reading the project's repository")?;
    let avatar = match repository {
        Some(repository) => tokio::time::timeout(
            std::time::Duration::from_secs(8),
            crate::repository::owner_avatar(&repository),
        )
        .await
        .ok()
        .flatten(),
        None => None,
    };
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
    let repository = tokio::task::spawn_blocking(move || crate::repository::read(&path))
        .await
        .context("reading the project's repository")?;
    Ok(Json(repository))
}

async fn create_thread(
    AppState(app): AppState<App>,
    Json(request): Json<CreateThread>,
) -> Result<Json<Thread>> {
    if app
        .hub
        .store()
        .project(&request.project_id)
        .await?
        .is_none()
    {
        return Err(not_found(format!("project {}", request.project_id)));
    }
    Ok(Json(app.hub.store().create_thread(request).await?))
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

async fn items(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<ItemsPage>> {
    Ok(Json(
        app.hub.store().items_page(&ThreadId(id), query).await?,
    ))
}

async fn send_message(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Json(message): Json<SendMessage>,
) -> Result<Json<Item>> {
    let id = ThreadId(id);
    if app.hub.store().thread(&id).await?.is_none() {
        return Err(not_found(format!("thread {id}")));
    }
    Ok(Json(app.hub.send_message(&id, message).await?))
}

async fn cancel(AppState(app): AppState<App>, Path(id): Path<String>) -> StatusCode {
    app.hub.cancel(&ThreadId(id));
    StatusCode::NO_CONTENT
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

async fn draft(
    AppState(app): AppState<App>,
    Path(scope): Path<String>,
) -> Result<Json<serde_json::Value>> {
    Ok(Json(
        app.hub
            .store()
            .draft(&scope)
            .await?
            .unwrap_or(serde_json::Value::Null),
    ))
}

async fn set_draft(
    AppState(app): AppState<App>,
    Path(scope): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Result<StatusCode> {
    app.hub.store().set_draft(scope, value).await?;
    Ok(StatusCode::NO_CONTENT)
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

#[derive(Deserialize, Default)]
#[serde(default)]
struct EngineActionBody {
    code: Option<String>,
    version: Option<String>,
}

/// install | update | rollback | use | login | login-code | cancel-login | logout | refresh |
/// versions
async fn engine_action(
    AppState(app): AppState<App>,
    Path((engine, action)): Path<(String, String)>,
    body: Option<Json<EngineActionBody>>,
) -> Result<Json<EngineStatus>> {
    let engine: EngineId = engine
        .parse()
        .map_err(|error: String| bad_request(anyhow::anyhow!(error)))?;
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let engines = app.hub.engines().clone();
    match action.as_str() {
        // Installs take a while; their progress arrives as events.
        "install" | "update" | "rollback" | "use" => {
            tokio::spawn(async move {
                let result = match action.as_str() {
                    "install" => engines.install(engine).await.map(drop),
                    "update" => engines.update(engine).await.map(drop),
                    "rollback" => engines.rollback(engine).await.map(drop),
                    _ => match body.version {
                        Some(version) => engines.use_version(engine, &version).await,
                        None => Err(anyhow::anyhow!("which version?")),
                    },
                };
                match result {
                    Ok(()) => engines.refresh(engine).await,
                    Err(error) => tracing::warn!(%engine, "{action} failed: {error:#}"),
                }
            });
        }
        "login" => engines.login(engine).map_err(bad_request)?,
        "login-code" => {
            let code = body
                .code
                .ok_or_else(|| bad_request(anyhow::anyhow!("missing code")))?;
            engines
                .submit_login_code(engine, &code)
                .await
                .map_err(bad_request)?;
        }
        "cancel-login" => engines.cancel_login(engine),
        "logout" => engines.logout(engine).map_err(bad_request)?,
        "refresh" => engines.refresh(engine).await,
        // The list arrives in the status this returns (and its event).
        "versions" => {
            engines.list_versions(engine).await.map_err(bad_request)?;
        }
        other => return Err(not_found(format!("engine action {other:?}"))),
    }
    Ok(Json(app.hub.engines().status(engine)))
}

#[derive(Deserialize)]
struct ListQuery {
    path: Option<String>,
}

/// Directory listing for picking a project folder on this host.
async fn list_dir(Query(query): Query<ListQuery>) -> Result<Json<DirListing>> {
    let path = match query.path {
        Some(path) if !path.is_empty() => std::path::PathBuf::from(path),
        _ => std::env::var_os("HOME").map_or_else(|| "/".into(), std::path::PathBuf::from),
    };
    let reader = std::fs::read_dir(&path)
        .with_context(|| format!("list {}", path.display()))
        .map_err(bad_request)?;
    let mut entries: Vec<DirEntry> = reader
        .filter_map(std::result::Result::ok)
        .map(|entry| DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: entry.file_type().is_ok_and(|kind| kind.is_dir()),
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(Json(DirListing {
        parent: path.parent().map(|parent| parent.display().to_string()),
        path: path.display().to_string(),
        entries,
    }))
}

/// The page title of an `http(s)` link; `null` when it has none or can't be
/// fetched.
async fn link_title(
    AppState(app): AppState<App>,
    Query(query): Query<LinkTitleQuery>,
) -> Result<Json<LinkTitle>> {
    let url = links::link_url(&query.url).map_err(bad_request)?;
    let title = app.links.title(url).await;
    Ok(Json(LinkTitle {
        url: query.url,
        title,
    }))
}

/// Authorizes the connection's certificate when the code is right.
async fn pair(
    AppState(app): AppState<App>,
    Extension(client): Extension<Client>,
    Json(request): Json<PairRequest>,
) -> Result<Json<PairResponse>> {
    let store = app.hub.store();
    if request.public_key != client.certificate {
        return Err(bad_request(anyhow::anyhow!(
            "the key in the request isn't this connection's certificate"
        )));
    }
    if !store
        .take_pair_code(crate::hash_code(&request.code), artisan_api::now())
        .await?
    {
        return Err(Failure(
            anyhow::anyhow!("that pairing code is wrong or expired"),
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
        ));
    }
    let device_id = store
        .add_device(request.device_name, client.certificate)
        .await?;
    let identity = crate::identity(store).await?;
    Ok(Json(PairResponse {
        device_id,
        forge_public_key: identity.certificate,
    }))
}
