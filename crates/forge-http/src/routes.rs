use crate::events;
use crate::repository::{Icon, IconSource};
use artisan_api::{
    Answer, ApiError, BlobHash, BlobRef, ContentId, CreateProject, CreateThread, DirEntry,
    DirListing, EngineId, EngineStatus, ErrorCode, Item, ItemId, ItemsPage, ItemsQuery,
    PairRequest, PairResponse, Project, ProjectId, SendMessage, State, Thread, ThreadId,
    UpdateThread,
};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State as AppState};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use forge_runs::Hub;
use forge_runs::failure as wire;
use serde::Deserialize;
use sha2::Digest as _;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::CompressionLevel;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{NotForContentType, Predicate, SizeAbove};
use tracing::Instrument as _;

#[derive(Clone)]
pub(crate) struct App {
    pub hub: Hub,
    pub forge_version: String,
    pub shutdown: tokio_util::sync::CancellationToken,
    pub tasks: tokio_util::task::TaskTracker,
    pub repositories: Arc<crate::repository::Repositories>,
    pub icons: Arc<crate::repository::Icons>,
}

#[derive(Clone)]
pub(crate) struct Client {
    pub certificate: forge_store::DeviceCertificate,
}

#[derive(Clone)]
pub(crate) struct TunnelAuth {
    pub claims: artisan_account::GrantClaims,
}

const PAIR_BODY_BYTES: usize = 4 * 1024;

pub(crate) fn router(app: App) -> Router {
    let authorized = Router::new()
        .route("/state", get(state))
        .route("/database-rollback", delete(dismiss_database_rollback))
        .route("/events", get(events::stream))
        .route("/projects", post(create_project))
        .route("/projects/{id}", delete(remove_project))
        .route("/projects/{id}/icon", get(project_icon))
        .route("/projects/{id}/repository", get(project_repository))
        .route("/threads", post(create_thread).get(threads_page))
        .route("/search/text", get(text_search))
        .route("/threads/{id}", patch(update_thread).get(thread))
        .route("/threads/{id}/settle", post(settle_thread))
        .route("/threads/{id}/unread", post(mark_unread))
        .route("/threads/{id}/items", get(items))
        .route("/threads/{id}/transcript", get(transcript))
        .route("/threads/{id}/work", get(work))
        .route("/threads/{id}/agents/{agent}/items", get(agent_items))
        .route("/threads/{id}/inspector", get(inspector))
        .route("/threads/{id}/resource", get(resource))
        .route("/threads/{id}/messages", post(send_message))
        .route("/threads/{id}/cancel", post(cancel))
        .route("/threads/{id}/continue", post(continue_turn))
        .route("/threads/{id}/goal/clear", post(clear_goal))
        .route("/items/{id}/answer", post(answer))
        .route("/editor/calls/{id}", post(editor_reply))
        .route("/items/{id}", get(item))
        .route("/content/{id}", get(content_range))
        .route("/content/{id}/index", get(source_index))
        .route("/content/{id}/records", get(collection_range))
        .route("/settings/{key}", put(set_setting))
        .route("/mcp", patch(mutate_mcp))
        .route(
            "/mcp/sign-in/start",
            post(crate::extensions::mcp_sign_in_start),
        )
        .route(
            "/mcp/sign-in/finish",
            post(crate::extensions::mcp_sign_in_finish),
        )
        .route(
            "/mcp/sign-in/cancel",
            post(crate::extensions::mcp_sign_in_cancel),
        )
        .route("/mcp/sign-out", post(crate::extensions::mcp_sign_out))
        .route("/engines/{engine}/default-model", put(set_default_model))
        .route("/engines/{engine}/model-config", put(set_model_config))
        .route("/engines/{engine}/model-groups", put(set_model_group))
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
            "/marketplace/search",
            get(crate::extensions::marketplace_search),
        )
        .route(
            "/marketplace/install",
            post(crate::extensions::marketplace_install),
        )
        .route(
            "/prompt",
            get(crate::extensions::prompt).put(crate::extensions::save_prompt),
        )
        .route("/prompt/resolve", post(crate::extensions::resolve_prompt))
        .route("/fs/list", get(list_dir))
        .layer(middleware::from_fn_with_state(app.clone(), authorize));
    Router::new()
        .route(
            "/pair",
            post(pair).layer(DefaultBodyLimit::max(PAIR_BODY_BYTES)),
        )
        .merge(authorized)
        .layer(DefaultBodyLimit::max(forge_store::MAX_BLOB_BYTES + 1024))
        .layer(compression())
        .layer(middleware::from_fn(log_internal))
        .with_state(app)
}

fn compression() -> CompressionLayer<impl Predicate> {
    CompressionLayer::new()
        .quality(CompressionLevel::Fastest)
        .compress_when(
            SizeAbove::new(1024)
                .and(NotForContentType::IMAGES)
                .and(NotForContentType::SSE)
                .and(NotForContentType::GRPC),
        )
}

async fn authorize(
    AppState(app): AppState<App>,
    client: Option<Extension<Client>>,
    tunnel: Option<Extension<TunnelAuth>>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(Extension(tunnel)) = tunnel {
        let now_sec = artisan_api::now().div_euclid(1000);
        let live = tunnel.claims.exp > now_sec
            && tunnel
                .claims
                .scope
                .contains(&artisan_account::Scope::ForgeExecute);
        if live {
            request
                .extensions_mut()
                .insert(artisan_api::DeviceId(format!(
                    "tunnel-{}",
                    tunnel.claims.sub
                )));
            return next.run(request).await;
        }
        return Failure::from(Unauthorized::Tunnel).into_response();
    }
    let Some(Extension(client)) = client else {
        return Failure::from(Unauthorized::NoCertificate).into_response();
    };
    match app.hub.store().is_authorized(&client.certificate).await {
        Ok(Some(device)) => {
            request.extensions_mut().insert(device);
            next.run(request).await
        }
        Ok(None) => Failure::from(Unauthorized::NotPaired).into_response(),
        Err(failure) => Failure::from(failure).into_response(),
    }
}

const fn status(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::BadRequest => StatusCode::BAD_REQUEST,
        ErrorCode::Conflict | ErrorCode::VersionMismatch => StatusCode::CONFLICT,
        ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorCode::EngineUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Missing {
    #[error("thread {0} not found")]
    Thread(ThreadId),
    #[error("project {0} not found")]
    Project(ProjectId),
    #[error("item {0} not found")]
    Item(ItemId),
    #[error("agent {0} not found")]
    Agent(ItemId),
    #[error("agent {agent} in thread {thread} not found")]
    AgentInThread { thread: ThreadId, agent: ItemId },
    #[error("text source {0} not found")]
    TextSource(ContentId),
    #[error("canonical collection {0} not found")]
    Collection(ContentId),
    #[error("blob {0} not found")]
    Blob(BlobHash),
}

impl Missing {
    fn wire(&self) -> artisan_api::Failure {
        use artisan_api::{AgentFailure, Failure, ProjectFailure, ResourceFailure, ThreadFailure};
        match self {
            Self::Thread(thread) => Failure::Thread(ThreadFailure::NotFound {
                thread: thread.clone(),
            }),
            Self::Project(project) => Failure::Project(ProjectFailure::NotFound {
                project: project.clone(),
            }),
            Self::Item(item) => Failure::Thread(ThreadFailure::ItemNotFound { item: item.clone() }),
            Self::Agent(agent) => Failure::Agent(AgentFailure::NotFound {
                agent: agent.clone(),
            }),
            Self::AgentInThread { thread, agent } => Failure::Agent(AgentFailure::NotInThread {
                thread: thread.clone(),
                agent: agent.clone(),
            }),
            Self::TextSource(content) | Self::Collection(content) => {
                Failure::Resource(ResourceFailure::ContentNotFound {
                    content: content.clone(),
                })
            }
            Self::Blob(blob) => {
                Failure::Resource(ResourceFailure::BlobNotFound { blob: blob.clone() })
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Rejected {
    #[error("{path} is not a directory on this host")]
    NotADirectory { path: String },
    #[error("invalid source identity or range limit for {content}")]
    ContentQuery { content: ContentId },
    #[error("invalid collection identity or range limit for {content}")]
    CollectionQuery { content: ContentId },
    #[error(
        "setting {key} is {} or {}",
        artisan_api::CLAUDE_CODE,
        artisan_api::CLAUDE_AI
    )]
    Transport { key: String },
    #[error(transparent)]
    SettingValue(#[from] artisan_api::SettingValueError),
    #[error("setting {key} is not one clients can change")]
    UnknownSetting { key: String },
    #[error("setting {key} is saved through {route}")]
    OwnRoute { key: String, route: &'static str },
    #[error("{engine} model {model} is unavailable")]
    ModelUnavailable { engine: EngineId, model: String },
    #[error("config for {sent} sent to {engine}")]
    ConfigEngine { engine: EngineId, sent: EngineId },
    #[error("the {engine} config names no model")]
    ConfigWithoutModel { engine: EngineId },
    #[error("only Claude signs in to claude.ai, not {engine}")]
    ClaudeAiEngine { engine: EngineId },
    #[error("host home directory unavailable; provide a path")]
    NoHome,
}

impl Rejected {
    fn wire(&self) -> artisan_api::Failure {
        use artisan_api::{
            EngineFailure, Failure, ProjectFailure, ResourceFailure, SettingFailure,
            SettingValueError as Value,
        };
        match self {
            Self::NotADirectory { path } => {
                Failure::Project(ProjectFailure::NotADirectory { path: path.clone() })
            }
            Self::ContentQuery { content } | Self::CollectionQuery { content } => {
                Failure::Resource(ResourceFailure::InvalidContent {
                    content: content.clone(),
                })
            }
            Self::Transport { key } => Failure::Setting(SettingFailure::InvalidValue {
                key: key.clone(),
                detail: self.to_string(),
            }),
            Self::SettingValue(
                error @ (Value::NotBool { key }
                | Value::NotText { key, .. }
                | Value::NotAChoice { key, .. }
                | Value::NotAnOption { key, .. }
                | Value::ModelCustomizations { key, .. }),
            ) => Failure::Setting(SettingFailure::InvalidValue {
                key: (*key).to_owned(),
                detail: diagnostics::chain(error),
            }),
            Self::UnknownSetting { key } => {
                Failure::Setting(SettingFailure::Unknown { key: key.clone() })
            }
            Self::OwnRoute { key, route } => Failure::Setting(SettingFailure::OwnRoute {
                key: key.clone(),
                route: (*route).to_owned(),
            }),
            Self::ModelUnavailable { engine, model } => {
                Failure::Setting(SettingFailure::ModelUnavailable {
                    engine: *engine,
                    model: model.clone(),
                })
            }
            Self::ConfigEngine { engine, sent } => Failure::Setting(SettingFailure::ConfigEngine {
                engine: *engine,
                sent: *sent,
            }),
            Self::ConfigWithoutModel { engine } => {
                Failure::Setting(SettingFailure::ConfigWithoutModel { engine: *engine })
            }
            Self::ClaudeAiEngine { engine } => {
                Failure::Engine(EngineFailure::ClaudeAiEngine { engine: *engine })
            }
            Self::NoHome => Failure::Resource(ResourceFailure::NoHome),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Unauthorized {
    #[error("the connection carries no client certificate")]
    NoCertificate,
    #[error("this device is not paired with Forge")]
    NotPaired,
    #[error("that pairing code is wrong or expired")]
    PairCode,
    #[error("this tunneled call is not authorized")]
    Tunnel,
}

impl Unauthorized {
    const fn wire(&self) -> artisan_api::Failure {
        use artisan_api::{AuthFailure, Failure};
        Failure::Auth(match self {
            Self::NoCertificate => AuthFailure::NoCertificate,
            Self::NotPaired => AuthFailure::NotPaired,
            Self::PairCode => AuthFailure::PairCode,
            Self::Tunnel => AuthFailure::Tunnel,
        })
    }
}

const LISTING_ENTRIES: usize = 32_768;
const LISTING_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ListDirectoryError {
    #[error("wait for a directory listing slot")]
    Slot(#[source] tokio::sync::AcquireError),
    #[error("run the listing of {}", path.display())]
    Task {
        path: PathBuf,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("list {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read an entry in {}", path.display())]
    Entry {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("inspect {entry}")]
    Inspect {
        entry: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the listing of {} exceeds {LISTING_ENTRIES} entries or {LISTING_BYTES} bytes", path.display())]
    TooLarge { path: PathBuf },
}

impl ListDirectoryError {
    fn wire(&self) -> artisan_api::Failure {
        use artisan_api::{Failure, Limited, ResourceFailure};
        match self {
            Self::Slot(_) | Self::Task { .. } => Failure::Internal,
            Self::Read { path, .. } | Self::Entry { path, .. } => {
                Failure::Resource(ResourceFailure::Unlistable {
                    path: path.display().to_string(),
                })
            }
            Self::Inspect { entry, .. } => Failure::Resource(ResourceFailure::Unlistable {
                path: entry.clone(),
            }),
            Self::TooLarge { .. } => Failure::Resource(ResourceFailure::TooLarge {
                subject: Limited::Listing,
                actual: None,
                limit: u64::try_from(LISTING_ENTRIES)
                    .expect("invariant: the listing entry limit fits in u64"),
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("receive an uploaded blob")]
pub(crate) struct ReceiveBlobError(#[source] axum::Error);

#[derive(Debug, thiserror::Error)]
#[error("read blob {hash} at {}", path.display())]
pub(crate) struct ReadBlobError {
    hash: BlobHash,
    path: PathBuf,
    #[source]
    source: std::io::Error,
}

#[derive(Debug, thiserror::Error)]
#[error("read resource {path}")]
pub(crate) struct ReadResourceError {
    path: String,
    #[source]
    source: forge_runs::resources::ResourceError,
}

fn engines_failure(engine: EngineId, kind: EngineKind<'_>) -> artisan_api::Failure {
    use artisan_api::{EngineFailure, Failure};
    Failure::Engine(match kind {
        EngineKind::NotInstalled => EngineFailure::NotInstalled { engine },
        EngineKind::NoBuild => EngineFailure::NoBuild { engine },
        EngineKind::Versions(detail) => EngineFailure::Versions { engine, detail },
        EngineKind::NoSignIn => EngineFailure::NoSignIn { engine },
        EngineKind::SignInClosed => EngineFailure::SignInClosed { engine },
        EngineKind::Unusable(version) => EngineFailure::UnusableVersion {
            engine,
            version: version.to_owned(),
        },
    })
}

enum EngineKind<'a> {
    NotInstalled,
    NoBuild,
    Versions(String),
    NoSignIn,
    SignInClosed,
    Unusable(&'a str),
}

fn validate_version(
    error: &forge_engines::ValidateVersionError,
    engine: EngineId,
) -> artisan_api::Failure {
    use forge_engines::ValidateVersionError as Validate;
    match error {
        Validate::Unusable(unusable) => {
            engines_failure(engine, EngineKind::Unusable(&unusable.version))
        }
        Validate::TooOld {
            engine,
            version,
            minimum,
        } => artisan_api::Failure::Engine(artisan_api::EngineFailure::VersionTooOld {
            engine: *engine,
            version: version.clone(),
            minimum: (*minimum).to_owned(),
        }),
    }
}

fn submit_code(error: &forge_engines::SubmitCodeError) -> artisan_api::Failure {
    use forge_engines::SubmitCodeError as Submit;
    match error {
        Submit::NoSignIn { engine } => engines_failure(*engine, EngineKind::NoSignIn),
        Submit::Closed { engine } => engines_failure(*engine, EngineKind::SignInClosed),
        Submit::Write { .. } => artisan_api::Failure::Internal,
    }
}

fn claude_ai_sign_in(error: &forge_engines::ClaudeAiSignInError) -> artisan_api::Failure {
    use artisan_api::{EngineFailure, Failure};
    use forge_engines::ClaudeAiSignInError as SignIn;
    match error {
        SignIn::NotSignedIn | SignIn::SignedOutMeanwhile => {
            Failure::Engine(EngineFailure::ClaudeAiSignedOut)
        }
        SignIn::NoUserAgent => Failure::Engine(EngineFailure::ClaudeAiNoUserAgent),
        SignIn::Bootstrap(source) => Failure::Engine(EngineFailure::ClaudeAiRefused {
            detail: diagnostics::chain(source),
        }),
        SignIn::NoOrganization => Failure::Engine(EngineFailure::ClaudeAiNoOrganization),
        SignIn::Client(_) | SignIn::Save(_) => Failure::Internal,
    }
}

fn list_versions(error: &forge_engines::ListVersionsError) -> artisan_api::Failure {
    use forge_engines::ListVersionsError as List;
    match error {
        List::NoBuild(no_build) => engines_failure(no_build.engine, EngineKind::NoBuild),
        List::Get { engine, source } => {
            engines_failure(*engine, EngineKind::Versions(diagnostics::chain(source)))
        }
        List::Pointer { engine, source } => {
            engines_failure(*engine, EngineKind::Versions(diagnostics::chain(source)))
        }
        List::Task { .. } => artisan_api::Failure::Internal,
    }
}

const fn internal<E>(_: &E) -> artisan_api::Failure {
    artisan_api::Failure::Internal
}

macro_rules! route_errors {
    ($($variant:ident($error:ty) => $wire:expr,)*) => {
        #[derive(Debug, thiserror::Error)]
        pub(crate) enum RouteError {
            $(
                #[error(transparent)]
                $variant(#[from] $error),
            )*
        }

        impl RouteError {
            fn wire(&self) -> artisan_api::Failure {
                match self {
                    $(Self::$variant(error) => {
                        let wire: fn(&$error) -> artisan_api::Failure = $wire;
                        wire(error)
                    })*
                }
            }
        }

        $(
            impl From<$error> for Failure {
                fn from(error: $error) -> Self {
                    Self::new(RouteError::$variant(error))
                }
            }
        )*
    };
}

route_errors! {
    Missing(Missing) => Missing::wire,
    Rejected(Rejected) => Rejected::wire,
    Unauthorized(Unauthorized) => Unauthorized::wire,
    ListDirectory(ListDirectoryError) => ListDirectoryError::wire,
    ReadBlob(ReadBlobError) => internal,
    ReceiveBlob(ReceiveBlobError) => |_| artisan_api::Failure::Resource(artisan_api::ResourceFailure::UploadInterrupted),
    ReadResource(ReadResourceError) => |error| error.source.wire(&error.path),
    Authorize(forge_store::AuthorizeError) => internal,
    AwaitingItems(forge_store::AwaitingItemsError) => internal,
    DismissRollback(forge_store::DismissRollbackError) => internal,
    CreateProject(forge_store::CreateProjectError) => |error| wire::create_project(&error.cause),
    CreateThread(forge_store::CreateThreadError) => wire::create_thread,
    Inspector(forge_store::InspectorError) => |error| wire::content(&error.cause),
    MarkUnread(forge_store::MarkUnreadError) => |error| wire::write(&error.cause),
    PairDevice(forge_store::PairDeviceError) => internal,
    PutBlob(forge_store::PutBlobError) => |error| wire::blob(&error.cause),
    ReadDraft(forge_store::ReadDraftError) => internal,
    ReadItem(forge_store::ReadItemError) => internal,
    ReadProject(forge_store::ReadProjectError) => internal,
    ReadThread(forge_store::ReadThreadError) => internal,
    RemoveProject(forge_store::RemoveProjectError) => internal,
    SetDefaultModel(forge_store::SetDefaultModelError) => |error| wire::setting(&error.cause),
    SetDraft(forge_store::SetDraftError) => |error| wire::draft(&error.scope, &error.cause),
    SetModelConfig(forge_store::SetModelConfigError) => |error| wire::setting(&error.cause),
    SetModelGroup(forge_store::SetModelGroupError) => |error| wire::write(&error.cause),
    SetSetting(forge_store::SetSettingError) => |error| wire::setting(&error.cause),
    SettleThread(forge_store::SettleThreadError) => |error| wire::write(&error.cause),
    State(forge_store::StateError) => internal,
    TextSearch(forge_store::TextSearchError) => internal,
    TranscriptPage(forge_store::TranscriptPageError) => internal,
    TranscriptRows(forge_store::TranscriptRowsError) => internal,
    WorkPage(forge_store::WorkPageError) => internal,
    ThreadsPage(forge_store::ThreadsPageError) => |error| wire::threads_page(&error.cause),
    ContentRange(forge_store::ContentRangeError) => |error| wire::content(&error.cause),
    CollectionRange(forge_store::CollectionRangeError) => |error| wire::content(&error.cause),
    SourceIndex(forge_store::SourceIndexPageError) => |error| wire::source_index(&error.cause),
    MutateMcp(forge_store::MutateMcpError) => |error| wire::setting(&error.cause),
    AdoptSkill(forge_runs::AdoptSkillError) => forge_runs::AdoptSkillError::wire,
    DiscardSkill(forge_runs::DiscardSkillError) => forge_runs::DiscardSkillError::wire,
    EnabledSkills(forge_runs::EnabledSkillsError) => forge_runs::EnabledSkillsError::wire,
    GlobalPrompt(forge_runs::GlobalPromptError) => forge_runs::GlobalPromptError::wire,
    MarketplaceInstall(forge_runs::MarketplaceInstallError) => forge_runs::MarketplaceInstallError::wire,
    MarketplaceSearch(forge_runs::MarketplaceSearchError) => forge_runs::MarketplaceSearchError::wire,
    ResolveGlobalPrompt(forge_runs::ResolveGlobalPromptError) => forge_runs::ResolveGlobalPromptError::wire,
    SaveGlobalPrompt(forge_runs::SaveGlobalPromptError) => forge_runs::SaveGlobalPromptError::wire,
    SetSkillEnabled(forge_runs::SetSkillEnabledError) => forge_runs::SetSkillEnabledError::wire,
    SkillCatalog(forge_runs::SkillCatalogError) => forge_runs::SkillCatalogError::wire,
    McpStore(forge_runs::McpStoreError) => forge_runs::McpStoreError::wire,
    McpSignInStart(forge_runs::McpSignInStartError) => forge_runs::McpSignInStartError::wire,
    McpSignInFinish(forge_runs::McpSignInFinishError) => forge_runs::McpSignInFinishError::wire,
    Send(forge_runs::SendError) => forge_runs::SendError::wire,
    Answer(forge_runs::AnswerError) => forge_runs::AnswerError::wire,
    Continue(forge_runs::ContinueError) => forge_runs::ContinueError::wire,
    Update(forge_runs::UpdateError) => forge_runs::UpdateError::wire,
    ClearGoal(forge_runs::ClearGoalError) => forge_runs::ClearGoalError::wire,
    Cancel(forge_runs::CancelError) => forge_runs::CancelError::wire,
    EditorReply(forge_runs::runtime::EditorReplyError) => forge_runs::runtime::EditorReplyError::wire,
    ValidateVersion(VersionError) => |error| validate_version(&error.source, error.engine),
    NotInstalled(forge_engines::NotInstalled) => |error| engines_failure(error.engine, EngineKind::NotInstalled),
    SubmitCode(forge_engines::SubmitCodeError) => submit_code,
    ClaudeAiSignIn(forge_engines::ClaudeAiSignInError) => claude_ai_sign_in,
    ClaudeAiSignOut(forge_engines::claudeai::RemoveSessionError) => internal,
    ListVersions(forge_engines::ListVersionsError) => list_versions,
}

#[derive(Debug, thiserror::Error)]
#[error("use {engine} version")]
pub(crate) struct VersionError {
    engine: EngineId,
    #[source]
    source: forge_engines::ValidateVersionError,
}

pub(crate) struct Failure {
    wire: artisan_api::Failure,
    error: Box<RouteError>,
    spans: diagnostics::Spans,
}

impl Failure {
    fn new(error: RouteError) -> Self {
        Self {
            wire: error.wire(),
            error: Box::new(error),
            spans: diagnostics::Spans::capture(),
        }
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let message = forge_runs::failure::message(&*self.error, &self.wire);
        let mut response = refusal(ApiError::new(self.wire, message));
        response.extensions_mut().insert(Refusal {
            error: Arc::from(self.error),
            spans: self.spans,
        });
        response
    }
}

fn refusal(error: ApiError) -> Response {
    (status(error.code), Json(error)).into_response()
}

#[derive(Clone)]
struct Refusal {
    error: Arc<RouteError>,
    spans: diagnostics::Spans,
}

async fn log_internal(request: Request, next: Next) -> Response {
    let span = tracing::info_span!(
        "request",
        method = %request.method(),
        path = request.uri().path()
    );
    async move {
        let response = next.run(request).await;
        if let Some(Refusal { error, spans }) = response.extensions().get::<Refusal>() {
            if response.status().is_server_error() {
                tracing::error!(error = diagnostics::chain(&**error), %spans, "request failed");
            } else {
                tracing::info!(error = diagnostics::chain(&**error), "request refused");
            }
        }
        response
    }
    .instrument(span)
    .await
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
        database_rollback: snapshot.database_rollback,
    }))
}

async fn dismiss_database_rollback(AppState(app): AppState<App>) -> Result<StatusCode> {
    app.hub.store().dismiss_rollback().await?;
    Ok(StatusCode::NO_CONTENT)
}

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
            let wsl_distro = std::env::var("WSL_DISTRO_NAME")
                .ok()
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty());
            artisan_api::Account {
                user,
                host,
                wsl_distro,
            }
        })
        .clone()
}

async fn create_project(
    AppState(app): AppState<App>,
    Json(request): Json<CreateProject>,
) -> Result<Json<Project>> {
    let path = std::path::Path::new(&request.path);
    if !path.is_dir() {
        return Err(Rejected::NotADirectory { path: request.path }.into());
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
        .ok_or(Missing::Project(id))?;
    Ok(StatusCode::NO_CONTENT)
}

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
        .ok_or_else(|| Missing::Project(id.clone()))?;
    let store = app.hub.store();
    for candidate in CANDIDATES {
        let path = std::path::Path::new(&project.path).join(candidate);
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                tracing::info!(project = %id, path = %path.display(), %error, "unreadable project icon");
                continue;
            }
        };
        let source = metadata.modified().ok().map(|modified| IconSource::File {
            path: path.clone(),
            modified,
            len: metadata.len(),
        });
        match source
            .as_ref()
            .and_then(|source| app.icons.get(source, store))
        {
            Some(Icon::Blob(blob)) => return Ok(Json(Some(blob))),
            Some(Icon::Unusable) => continue,
            None => {}
        }
        let bytes = match crate::repository::icon_file(&path).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(error) => {
                tracing::info!(project = %id, error = diagnostics::chain(&error), "unreadable project icon");
                continue;
            }
        };
        let icon = match store.put_image(bytes).await {
            Ok(blob) => Icon::Blob(blob),
            Err(error) => {
                tracing::info!(project = %id, path = %path.display(), error = diagnostics::chain(&error), "unusable project icon");
                Icon::Unusable
            }
        };
        if let Some(source) = source {
            app.icons.insert(source, icon.clone());
        }
        if let Icon::Blob(blob) = icon {
            return Ok(Json(Some(blob)));
        }
    }
    let root = PathBuf::from(&project.path);
    let Some(bytes) = app.repositories.avatar(&root).await else {
        return Ok(Json(None));
    };
    let source = IconSource::Avatar(sha2::Sha256::digest(&bytes).into());
    let icon = if let Some(icon) = app.icons.get(&source, store) {
        icon
    } else {
        let icon = match store.put_image(bytes).await {
            Ok(blob) => Icon::Blob(blob),
            Err(error) => {
                tracing::info!(project = %id, error = diagnostics::chain(&error), "unusable repository avatar");
                Icon::Unusable
            }
        };
        app.icons.insert(source, icon.clone());
        icon
    };
    Ok(Json(match icon {
        Icon::Blob(blob) => Some(blob),
        Icon::Unusable => None,
    }))
}

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
        .ok_or(Missing::Project(id))?;
    let path = PathBuf::from(project.path);
    let repository = app.repositories.read(&path).await;
    Ok(Json(repository))
}

async fn create_thread(
    AppState(app): AppState<App>,
    Json(request): Json<CreateThread>,
) -> Result<Json<Thread>> {
    let started_by_user = request.parent_thread_id.is_none() && request.spawned_by.is_none();
    let thread = app.hub.store().create_thread(request).await?;
    if started_by_user {
        app.hub
            .store()
            .set_setting(
                artisan_api::LAST_PROJECT_KEY.into(),
                serde_json::Value::String(thread.project_id.as_str().to_owned()),
            )
            .await?;
    }
    Ok(Json(thread))
}

async fn threads_page(
    AppState(app): AppState<App>,
    Query(query): Query<artisan_api::ThreadsQuery>,
) -> Result<Json<artisan_api::ThreadsPage>> {
    Ok(Json(app.hub.store().threads_page(query).await?))
}

async fn thread(AppState(app): AppState<App>, Path(id): Path<ThreadId>) -> Result<Json<Thread>> {
    app.hub.mcp().warm_servers();
    app.hub
        .store()
        .thread(&id)
        .await?
        .map(Json)
        .ok_or_else(|| Missing::Thread(id).into())
}

async fn update_thread(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Json(update): Json<UpdateThread>,
) -> Result<Json<Thread>> {
    let id = ThreadId(id);
    let thread = app.hub.update_thread(&id, update).await?;
    Ok(Json(thread.ok_or(Missing::Thread(id))?))
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
        .ok_or_else(|| Missing::Thread(id).into())
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
        .ok_or_else(|| Missing::Thread(id).into())
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

async fn transcript(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<ItemsPage>> {
    Ok(Json(
        app.hub
            .store()
            .transcript_rows(&ThreadId(id), query)
            .await?,
    ))
}

async fn text_search(
    AppState(app): AppState<App>,
    Query(query): Query<artisan_api::TextSearchQuery>,
) -> Result<Json<Vec<artisan_api::TextHit>>> {
    Ok(Json(app.hub.store().text_search(query).await?))
}

async fn work(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<ItemsPage>> {
    Ok(Json(app.hub.store().work_page(&ThreadId(id), query).await?))
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
        .ok_or_else(|| Missing::Agent(agent.clone()))?;
    if owner.thread_id != thread || !matches!(owner.kind, artisan_api::ItemKind::Agent { .. }) {
        return Err(Missing::AgentInThread { thread, agent }.into());
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
    Query(query): Query<artisan_api::InspectorQuery>,
) -> Result<Json<artisan_api::InspectorSummary>> {
    Ok(Json(
        app.hub.store().inspector_since(&id, query.outline).await?,
    ))
}

async fn source_index(
    AppState(app): AppState<App>,
    Path(id): Path<ContentId>,
    Query(query): Query<artisan_api::SourceIndexQuery>,
) -> Result<Json<artisan_api::SourceIndexRange>> {
    app.hub
        .store()
        .source_index(&id, query)
        .await?
        .map(Json)
        .ok_or_else(|| Missing::TextSource(id).into())
}

async fn content_range(
    AppState(app): AppState<App>,
    Path(id): Path<ContentId>,
    Query(query): Query<artisan_api::ContentQuery>,
) -> Result<Json<artisan_api::ContentRange>> {
    if !id.is_valid()
        || !query
            .limit
            .is_none_or(|limit| (4..=artisan_api::CONTENT_RANGE_BYTES).contains(&limit))
    {
        return Err(Rejected::ContentQuery { content: id }.into());
    }
    app.hub
        .store()
        .content_range(&id, query)
        .await?
        .map(Json)
        .ok_or_else(|| Missing::TextSource(id).into())
}

async fn collection_range(
    AppState(app): AppState<App>,
    Path(id): Path<ContentId>,
    Query(query): Query<artisan_api::CollectionQuery>,
) -> Result<Json<artisan_api::CollectionRange<serde_json::Value>>> {
    if !id.is_valid()
        || !query
            .limit
            .is_none_or(|limit| (1..=artisan_api::COLLECTION_RANGE_ENTRIES).contains(&limit))
    {
        return Err(Rejected::CollectionQuery { content: id }.into());
    }
    app.hub
        .store()
        .collection_range(&id, query)
        .await?
        .map(Json)
        .ok_or_else(|| Missing::Collection(id).into())
}

async fn item(AppState(app): AppState<App>, Path(id): Path<ItemId>) -> Result<Json<Item>> {
    app.hub
        .store()
        .item(&id)
        .await?
        .map(Json)
        .ok_or_else(|| Missing::Item(id).into())
}

#[derive(Deserialize)]
struct ResourceQuery {
    path: String,
    #[serde(default)]
    from_home: bool,
    item: Option<ItemId>,
}

async fn resource(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Query(query): Query<ResourceQuery>,
) -> Result<Bytes> {
    use forge_runs::resources::Written;
    let thread = app
        .hub
        .store()
        .thread(&ThreadId(id.clone()))
        .await?
        .ok_or(Missing::Thread(ThreadId(id)))?;
    let written = Written {
        path: query.path,
        from_home: query.from_home,
    };
    app.hub
        .read_resource(&thread, query.item.as_ref(), &written)
        .await
        .map(Bytes::from)
        .map_err(|source| {
            ReadResourceError {
                path: written.path,
                source,
            }
            .into()
        })
}

async fn send_message(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Json(message): Json<SendMessage>,
) -> Result<Json<Item>> {
    Ok(Json(app.hub.send_message(&ThreadId(id), message).await?))
}

async fn cancel(AppState(app): AppState<App>, Path(id): Path<String>) -> Result<StatusCode> {
    app.hub.cancel(&ThreadId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn continue_turn(AppState(app): AppState<App>, Path(id): Path<String>) -> Result<StatusCode> {
    app.hub.continue_turn(&ThreadId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_goal(AppState(app): AppState<App>, Path(id): Path<String>) -> Result<StatusCode> {
    app.hub.clear_goal(&ThreadId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn editor_reply(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Json(reply): Json<artisan_api::EditorReply>,
) -> Result<StatusCode> {
    Ok(
        match app.hub.answer_distributed_editor_call(&id, reply).await? {
            forge_runs::runtime::EditorCall::Answered => StatusCode::NO_CONTENT,
            forge_runs::runtime::EditorCall::NotWaiting => StatusCode::GONE,
        },
    )
}

async fn answer(
    AppState(app): AppState<App>,
    Path(id): Path<String>,
    Json(answer): Json<Answer>,
) -> Result<StatusCode> {
    app.hub.answer(&ItemId(id), answer).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_setting(
    AppState(app): AppState<App>,
    Path(key): Path<String>,
    Json(value): Json<serde_json::Value>,
) -> Result<StatusCode> {
    if key == artisan_api::TRANSPORT_KEY {
        let Some(transport) = value.as_str().filter(|transport| {
            [artisan_api::CLAUDE_CODE, artisan_api::CLAUDE_AI].contains(transport)
        }) else {
            return Err(Rejected::Transport { key }.into());
        };
        app.hub
            .store()
            .set_setting(key, serde_json::Value::String(transport.to_owned()))
            .await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    if key == artisan_api::THREAD_RETENTION_DAYS_KEY {
        artisan_api::setting_decl(&key)
            .expect("invariant: thread retention is a declared setting")
            .check(&value)
            .map_err(Rejected::from)?;
        app.hub.store().set_setting(key, value).await?;
        let hub = app.hub.clone();
        app.tasks.spawn(async move {
            if let Err(error) = hub.remove_inactive_threads().await {
                tracing::warn!(
                    error = diagnostics::chain(&error),
                    "remove inactive threads"
                );
            }
        });
        return Ok(StatusCode::NO_CONTENT);
    }
    if let Some(switch) = artisan_api::setting_decl(&key).filter(|decl| {
        decl.owner == artisan_api::SettingOwner::Forge
            && matches!(decl.kind, artisan_api::SettingKind::Bool)
    }) {
        switch.check(&value).map_err(Rejected::from)?;
        app.hub.store().set_setting(key, value).await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let route = match key.as_str() {
        "global_prompt" => "PUT /prompt",
        "mcp_servers" => "PATCH /mcp",
        "default_engine_config" => "PUT /engines/{engine}/default-model",
        artisan_api::MODEL_CONFIGS_KEY => "PUT /engines/{engine}/model-config",
        artisan_api::MODEL_GROUPS_KEY => "PUT /engines/{engine}/model-groups",
        artisan_api::LAST_PROJECT_KEY => "POST /threads",
        "disabled_skills" => "POST /skills/{id}/{action}",
        _ => return Err(Rejected::UnknownSetting { key }.into()),
    };
    Err(Rejected::OwnRoute { key, route }.into())
}

async fn mutate_mcp(
    AppState(app): AppState<App>,
    Json(mutation): Json<artisan_api::McpMutation>,
) -> Result<Json<Vec<artisan_api::McpServer>>> {
    let servers = app.hub.mcp().mutate(mutation).await?;
    let hub = app.hub.clone();
    app.tasks.spawn(async move { hub.mcp().check_auth().await });
    Ok(Json(servers))
}

async fn set_default_model(
    AppState(app): AppState<App>,
    Path(engine): Path<EngineId>,
    Json(id): Json<String>,
) -> Result<Json<artisan_api::EngineConfig>> {
    let model = available_model(&app, engine, id)?;
    Ok(Json(
        app.hub.store().set_default_model(engine, model).await?,
    ))
}

fn available_model(
    app: &App,
    engine: EngineId,
    id: String,
) -> std::result::Result<artisan_api::Model, Rejected> {
    app.hub
        .engines()
        .status(engine)
        .models
        .iter()
        .find(|model| model.id == id)
        .cloned()
        .ok_or(Rejected::ModelUnavailable { engine, model: id })
}

async fn set_model_config(
    AppState(app): AppState<App>,
    Path(engine): Path<EngineId>,
    Json(config): Json<artisan_api::EngineConfig>,
) -> Result<Json<artisan_api::EngineConfig>> {
    if config.engine != engine {
        return Err(Rejected::ConfigEngine {
            engine,
            sent: config.engine,
        }
        .into());
    }
    let id = config
        .model
        .clone()
        .ok_or(Rejected::ConfigWithoutModel { engine })?;
    let model = available_model(&app, engine, id)?;
    Ok(Json(
        app.hub.store().set_model_config(config, &model).await?,
    ))
}

async fn set_model_group(
    AppState(app): AppState<App>,
    Path(engine): Path<EngineId>,
    Json(group): Json<artisan_api::ModelGroup>,
) -> Result<StatusCode> {
    app.hub.store().set_model_group(engine, group).await?;
    Ok(StatusCode::NO_CONTENT)
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
    app.hub.mcp().warm_servers();
    Ok(Json(app.hub.store().set_draft(scope, request).await?))
}

async fn put_blob(AppState(app): AppState<App>, body: axum::body::Body) -> Result<Json<BlobRef>> {
    let upload = app.hub.store().admit_blob().await;
    let bytes = axum::body::to_bytes(body, forge_store::MAX_BLOB_BYTES + 1024)
        .await
        .map_err(ReceiveBlobError)?;
    Ok(Json(upload.put(bytes.to_vec()).await?))
}

async fn get_blob(AppState(app): AppState<App>, Path(hash): Path<String>) -> Result<Response> {
    let hash = BlobHash(hash);
    let Some(path) = app.hub.store().blob_path(&hash) else {
        return Err(Missing::Blob(hash).into());
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(source) => return Err(ReadBlobError { hash, path, source }.into()),
    };
    Ok((
        [
            (header::CONTENT_TYPE, "image/webp"),
            (header::CACHE_CONTROL, "private, no-store"),
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

async fn engine_action(
    AppState(app): AppState<App>,
    Path(engine): Path<EngineId>,
    Json(command): Json<artisan_api::EngineCommand>,
) -> Result<Json<EngineStatus>> {
    use artisan_api::EngineCommand as Command;
    let engines = app.hub.engines().clone();
    if let Command::Use { version } = &command {
        forge_engines::Engines::validate_version(engine, version)
            .map_err(|source| VersionError { engine, source })?;
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
                        Command::Login { .. }
                        | Command::LoginCode { .. }
                        | Command::CancelLogin { .. }
                        | Command::Logout { .. }
                        | Command::Refresh { .. }
                        | Command::Versions { .. }
                        | Command::ClaudeAiSignIn { .. }
                        | Command::ClaudeAiSignOut { .. } => {
                            unreachable!("only installation commands spawn a worker")
                        }
                    } } => result,
                };
                match result {
                    Ok(()) => {
                        if let Err(error) = hub.extensions().reconcile_global_prompt().await {
                            tracing::warn!(%engine, error = diagnostics::chain(&error), "reconcile prompt");
                        }
                        if let Err(error) = hub.extensions().reconcile_skills().await {
                            tracing::warn!(%engine, error = diagnostics::chain(&error), "reconcile skills");
                        }
                        engines.refresh(engine).await;
                    }
                    Err(error) => {
                        tracing::warn!(%engine, action = label, error = diagnostics::chain(&error), "engine command failed");
                    }
                }
            });
        }
        Command::Login {} => engines.login(engine)?,
        Command::LoginCode { code } => engines.submit_login_code(engine, &code).await?,
        Command::CancelLogin {} => engines.cancel_login(engine),
        Command::Logout {} => engines.logout(engine)?,
        Command::ClaudeAiSignIn { session } => {
            if engine != EngineId::Claude {
                return Err(Rejected::ClaudeAiEngine { engine }.into());
            }
            engines.claude_ai_sign_in(session).await?;
        }
        Command::ClaudeAiSignOut {} => {
            if engine != EngineId::Claude {
                return Err(Rejected::ClaudeAiEngine { engine }.into());
            }
            engines.claude_ai_sign_out()?;
        }
        Command::Refresh {} => engines.refresh(engine).await,
        Command::Versions {} => {
            engines.list_versions(engine).await?;
        }
    }
    Ok(Json(app.hub.engines().status(engine)))
}

#[derive(Deserialize)]
struct ListQuery {
    path: Option<String>,
}

async fn list_dir(Query(query): Query<ListQuery>) -> Result<Json<DirListing>> {
    static READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let path = match query.path {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => dirs::home_dir().ok_or(Rejected::NoHome)?,
    };
    let permit = READS.acquire().await.map_err(ListDirectoryError::Slot)?;
    let listed = path.clone();
    let listing = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        read_directory(&listed)
    })
    .await
    .map_err(|source| ListDirectoryError::Task { path, source })??;
    Ok(Json(listing))
}

fn read_directory(path: &std::path::Path) -> std::result::Result<DirListing, ListDirectoryError> {
    let reader = std::fs::read_dir(path).map_err(|source| ListDirectoryError::Read {
        path: path.to_owned(),
        source,
    })?;
    let mut entries = Vec::new();
    let mut bytes = 0usize;
    for entry in reader {
        let entry = entry.map_err(|source| ListDirectoryError::Entry {
            path: path.to_owned(),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let entry_path = entry.path().to_string_lossy().into_owned();
        bytes = bytes
            .saturating_add(name.len())
            .saturating_add(entry_path.len());
        if entries.len() >= LISTING_ENTRIES || bytes > LISTING_BYTES {
            return Err(ListDirectoryError::TooLarge {
                path: path.to_owned(),
            });
        }
        let is_dir = entry
            .file_type()
            .map_err(|source| ListDirectoryError::Inspect {
                entry: entry_path.clone(),
                source,
            })?
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

async fn pair(
    AppState(app): AppState<App>,
    Extension(client): Extension<Client>,
    Json(request): Json<PairRequest>,
) -> Result<Json<PairResponse>> {
    let store = app.hub.store();
    let Some(device_id) = store
        .pair_device(
            forge_store::PairCodeHash::of(&request.code),
            request.device_name,
            client.certificate,
        )
        .await?
    else {
        return Err(Unauthorized::PairCode.into());
    };
    Ok(Json(PairResponse { device_id }))
}
