#[cfg(all(test, unix))]
#[path = "../tests/opencode/cancellation.rs"]
mod cancellation_tests;
mod decoder;
mod goal;
#[cfg(test)]
#[path = "../tests/opencode/goal.rs"]
mod goal_tests;

#[cfg(test)]
#[path = "../tests/opencode/background.rs"]
mod background_tests;
#[cfg(test)]
#[path = "../tests/opencode/forms.rs"]
mod form_tests;
#[cfg(all(test, unix))]
#[path = "../tests/opencode/output.rs"]
mod output_tests;
mod side;
#[cfg(test)]
mod tests;

pub use decoder::DecodeError;
pub use goal::{ClearGoalError, GoalError};
pub use side::SideQueryError;

use crate::process::{self, EngineProcess, ReadError, SpawnError};
use crate::{EngineEvent, Harness, Input, Outcome, RunIo, Turn, UserInput};
use artisan_api::{ItemId, Permissions, ThreadId};
use base64::Engine as _;
use decoder::{Answered, Decoder, Method};
use futures::StreamExt as _;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use sse_core::{SseDecoder, SseEvent, SseStream};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

const READY_TIMEOUT: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_EVENT_BYTES: NonZeroUsize = NonZeroUsize::new(process::MAX_LINE_BYTES).unwrap();

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("no OS randomness for the OpenCode password")]
    Password(#[source] getrandom::Error),
    #[error("encode Forge's OpenCode agents")]
    EncodeAgents(#[source] serde_json::Error),
    #[error("write Forge's OpenCode agents to {}", path.display())]
    WriteAgents {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("start OpenCode")]
    Spawn(#[source] SpawnError),
    #[error("OpenCode did not start within {} s", READY_TIMEOUT.as_secs())]
    ReadyTimedOut,
    #[error("read OpenCode's startup output")]
    ReadyRead(#[source] ReadError),
    #[error("OpenCode exited before it was ready")]
    ExitedEarly,
    #[error("build the OpenCode HTTP client")]
    Client(#[source] reqwest::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("encode the body for OpenCode {path}")]
    Encode {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("call OpenCode {path}")]
    Send {
        path: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("read OpenCode's reply to {path}")]
    Read {
        path: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("OpenCode {path} answered {status}: {body}")]
    Status {
        path: String,
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("OpenCode {path} answered with invalid JSON")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SubscribeError {
    #[error("subscribe to OpenCode events")]
    Send(#[source] reqwest::Error),
    #[error("OpenCode refused the event stream: {status}")]
    Refused { status: reqwest::StatusCode },
    #[error("OpenCode's event stream sent nothing in {}s", REQUEST_TIMEOUT.as_secs())]
    Silent,
    #[error("OpenCode's event stream closed at once")]
    Closed,
}

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("read image {}", path.display())]
    Image {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("send the prompt")]
    Prompt(#[source] ApiError),
    #[error("OpenCode admitted the prompt without a message id")]
    NoMessageId,
    #[error("run /goal")]
    Goal(#[source] GoalError),
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("start the OpenCode server")]
    Server(#[source] ServerError),
    #[error("open OpenCode's event stream")]
    Subscribe(#[source] SubscribeError),
    #[error("switch OpenCode session {session} to agent {agent}")]
    Agent {
        session: String,
        agent: &'static str,
        #[source]
        source: Box<ApiError>,
    },
    #[error("switch OpenCode session {session} to model {model}")]
    Model {
        session: String,
        model: Value,
        #[source]
        source: Box<ApiError>,
    },
    #[error("create an OpenCode session")]
    Create(#[source] Box<ApiError>),
    #[error("OpenCode created a session without an id")]
    NoSessionId,
    #[error("interrupt OpenCode session {session}")]
    Interrupt {
        session: String,
        #[source]
        source: Box<ApiError>,
    },
    #[error("send message {item} to OpenCode session {session}")]
    Input {
        session: String,
        item: ItemId,
        #[source]
        source: Box<InputError>,
    },
    #[error("clear the goal of OpenCode session {session}")]
    ClearGoal {
        session: String,
        #[source]
        source: Box<GoalError>,
    },
    #[error("answer OpenCode request {path} for item {key}")]
    Answer {
        key: String,
        path: String,
        #[source]
        source: Box<ApiError>,
    },
    #[error("decode an OpenCode event")]
    Decode(#[source] DecodeError),
    #[error("OpenCode's event stream closed mid-run")]
    StreamClosed,
    #[error("OpenCode exited mid-run")]
    Exited,
    #[error("read OpenCode's output")]
    Stdout(#[source] ReadError),
}

#[derive(Debug, thiserror::Error)]
pub enum SetTitleError {
    #[error("thread {thread} has no OpenCode session")]
    NoSession { thread: ThreadId },
    #[error("start the OpenCode server")]
    Server(#[source] ServerError),
    #[error("rename OpenCode session {session}")]
    Rename {
        session: String,
        #[source]
        source: ApiError,
    },
}

pub struct OpenCodeHarness;

impl Harness for OpenCodeHarness {
    fn clear_goal(&self, turn: Turn) -> BoxFuture<'static, Result<(), crate::ClearGoalError>> {
        let thread = turn.thread_id.clone();
        Box::pin(async move {
            goal::clear_goal(turn)
                .await
                .map_err(|source| crate::ClearGoalError::OpenCode {
                    thread,
                    source: Box::new(source),
                })
        })
    }

    fn side_query(
        &self,
        turn: Turn,
        cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'static, Result<crate::SideReply, crate::SideQueryError>> {
        let thread = turn.thread_id.clone();
        Box::pin(async move {
            side::side_query(turn, cancel)
                .await
                .map_err(|source| crate::SideQueryError::OpenCode {
                    thread,
                    source: Box::new(source),
                })
        })
    }

    fn set_title(
        &self,
        turn: Turn,
        title: String,
    ) -> BoxFuture<'static, Result<(), crate::SetTitleError>> {
        let thread = turn.thread_id.clone();
        Box::pin(async move {
            set_title(turn, title)
                .await
                .map_err(|source| crate::SetTitleError::OpenCode {
                    thread,
                    source: Box::new(source),
                })
        })
    }

    fn compact(
        &self,
        turn: Turn,
        cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'static, Result<crate::SideReply, crate::SideQueryError>> {
        let thread = turn.thread_id.clone();
        Box::pin(async move {
            side::compact(turn, cancel)
                .await
                .map_err(|source| crate::SideQueryError::OpenCode {
                    thread,
                    source: Box::new(source),
                })
        })
    }

    fn run(&self, turn: Turn, mut io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            let (result, stderr) = run(&turn, &mut io).await;
            match result {
                Ok(outcome) => outcome,
                Err(failure) => {
                    {
                        let error: &dyn std::error::Error = &failure;
                        tracing::warn!(thread = %turn.thread_id, error, "OpenCode run failed");
                    }
                    let message = diagnostics::chain(&failure);
                    Outcome::failed(if stderr.is_empty() {
                        message
                    } else {
                        format!("{message}: OpenCode stderr: {stderr}")
                    })
                }
            }
        })
    }
}

async fn set_title(turn: Turn, title: String) -> Result<(), SetTitleError> {
    let session = turn
        .session_id
        .clone()
        .ok_or_else(|| SetTitleError::NoSession {
            thread: turn.thread_id.clone(),
        })?;
    let (mut server, password) = prepare(&turn).map_err(SetTitleError::Server)?;
    let result = async {
        let api = connect(&mut server, password)
            .await
            .map_err(SetTitleError::Server)?;
        let path = format!("/api/session/{session}");
        let body = serde_json::to_vec(&json!({ "title": title })).map_err(|source| {
            SetTitleError::Rename {
                session: session.clone(),
                source: ApiError::Encode {
                    path: path.clone(),
                    source,
                },
            }
        })?;
        let request = api
            .client
            .patch(format!("{}{path}", api.base))
            .timeout(REQUEST_TIMEOUT)
            .header("content-type", "application/json")
            .body(body);
        api.send(request, &path)
            .await
            .map(drop)
            .map_err(|source| SetTitleError::Rename {
                session: session.clone(),
                source,
            })
    }
    .await;
    server.kill().await;
    server.wait().await;
    result
}

fn prepare(turn: &Turn) -> Result<(EngineProcess, String), ServerError> {
    let password = password()?;
    let config_dir = write_agents(&turn.home, &turn.launch.readable)?;
    let server = spawn(turn, &password, &config_dir)?;
    Ok((server, password))
}

async fn connect(server: &mut EngineProcess, password: String) -> Result<Api, ServerError> {
    let base = tokio::time::timeout(READY_TIMEOUT, ready(server))
        .await
        .map_err(|_elapsed| ServerError::ReadyTimedOut)??;
    Api::new(base, password)
}

async fn run(turn: &Turn, io: &mut RunIo) -> (Result<Outcome, RunError>, String) {
    let (mut server, password) = match prepare(turn) {
        Ok(prepared) => prepared,
        Err(source) => return (Err(RunError::Server(source)), String::new()),
    };
    let cancel = io.cancel.clone();
    let result = async {
        let api = tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(Outcome::Cancelled),
            api = connect(&mut server, password) => api.map_err(RunError::Server)?,
        };
        drive(turn, io, &api, &mut server).await
    }
    .await;
    server.kill().await;
    server.wait().await;
    (result, server.stderr_tail())
}

fn spawn(turn: &Turn, password: &str, config_dir: &Path) -> Result<EngineProcess, ServerError> {
    let mut launch = turn.clone();
    let mut env = turn.launch.env.clone();
    env.extend(
        [
            ("OPENCODE_SERVER_PASSWORD", password),
            ("OPENCODE_CONFIG_DIR", &config_dir.to_string_lossy()),
            ("OPENCODE_ENABLE_QUESTION_TOOL", "1"),
            ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
            ("OPENCODE_CLIENT", "artisan-editor"),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned())),
    );
    launch.launch.env = env;
    let args = ["serve", "--hostname", "127.0.0.1", "--port", "0"].map(str::to_owned);
    process::spawn(&launch, &args).map_err(ServerError::Spawn)
}

async fn ready(server: &mut EngineProcess) -> Result<String, ServerError> {
    while let Some(line) = server.next_line().await.map_err(ServerError::ReadyRead)? {
        if let Some((_, url)) = line.split_once("listening on ") {
            return Ok(url.trim().trim_end_matches('/').to_owned());
        }
        tracing::debug!(target: "engine_stdout", bytes = line.len(), "OpenCode wrote to stdout");
    }
    Err(ServerError::ExitedEarly)
}

async fn drive(
    turn: &Turn,
    io: &mut RunIo,
    api: &Api,
    server: &mut EngineProcess,
) -> Result<Outcome, RunError> {
    let mut events = subscribe(api).await.map_err(RunError::Subscribe)?;
    let session = open_session(turn, api).await?;
    io.emit(EngineEvent::Session {
        id: session.clone(),
    })
    .await;
    let cancel = io.cancel.clone();
    let mut decoder = Decoder::new(Some(session.clone()));
    tokio::select! {
        biased;
        () = cancel.cancelled() => {
            api.post(&format!("/api/session/{session}/interrupt"), None)
                .await
                .map_err(|source| RunError::Interrupt { session: session.clone(), source: Box::new(source) })?;
            emit_all(io, decoder.cancel()).await;
            Ok(Outcome::Cancelled)
        }
        result = async {
            if let Some(commands) = commands(api).await {
                io.emit(EngineEvent::Commands(commands)).await;
            }
            if let Err(failure) = goal::refresh(api, &session, &mut decoder, io, false).await {
                let error: &dyn std::error::Error = &failure;
                tracing::debug!(session, error, "OpenCode goal unavailable");
            }
            send_input(api, &session, &turn.input, &mut decoder, io).await?;
            loop {
                if decoder.idle() {
                    match io.inbox.try_recv() {
                        Ok(input) => {
                            handle_input(api, &session, &mut decoder, io, input).await?;
                            continue;
                        }
                        Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }
                tokio::select! {
                    biased;
                    Some(input) = io.inbox.recv() => handle_input(api, &session, &mut decoder, io, input).await?,
                    event = events.recv() => match event {
                        Some(data) => emit_all(io, decoder.feed(&data).map_err(RunError::Decode)?).await,
                        None => return Err(RunError::StreamClosed),
                    },
                    line = server.next_line() => match line {
                        Ok(Some(line)) => tracing::debug!(target: "engine_stdout", bytes = line.len(), "OpenCode wrote to stdout"),
                        Ok(None) => return Err(RunError::Exited),
                        Err(source) => return Err(RunError::Stdout(source)),
                    },
                }
            }
            Ok(decoder
                .error
                .take()
                .map_or(Outcome::Done, Outcome::failed))
        } => result,
    }
}

async fn open_session(turn: &Turn, api: &Api) -> Result<String, RunError> {
    let agent = agent_name(turn.config.permissions);
    let variant = turn.config.variant.as_deref();
    let model = model_ref(turn.config.model.as_deref(), variant);
    Ok(if let Some(session) = &turn.session_id {
        api.post(
            &format!("/api/session/{session}/agent"),
            Some(&json!({ "agent": agent })),
        )
        .await
        .map_err(|source| RunError::Agent {
            session: session.clone(),
            agent,
            source: Box::new(source),
        })?;
        if let Some(model) = &model {
            api.post(
                &format!("/api/session/{session}/model"),
                Some(&json!({ "model": model })),
            )
            .await
            .map_err(|source| RunError::Model {
                session: session.clone(),
                model: model.clone(),
                source: Box::new(source),
            })?;
        }
        session.clone()
    } else {
        let mut body = json!({
            "agent": agent,
            "location": { "directory": turn.cwd.to_string_lossy() },
        });
        if let Some(model) = &model {
            body["model"] = model.clone();
        }
        let created = api
            .post("/api/session", Some(&body))
            .await
            .map_err(|source| RunError::Create(Box::new(source)))?;
        created["data"]["id"]
            .as_str()
            .ok_or(RunError::NoSessionId)?
            .to_owned()
    })
}

async fn handle_input(
    api: &Api,
    session: &str,
    decoder: &mut Decoder,
    io: &RunIo,
    input: Input,
) -> Result<(), RunError> {
    match input {
        Input::Steer(input) => {
            send_input(api, session, &input, decoder, io).await?;
        }
        Input::ClearGoal => {
            goal::command(api, session, "clear", &[])
                .await
                .map_err(|source| RunError::ClearGoal {
                    session: session.to_owned(),
                    source: Box::new(GoalError::Command(source)),
                })?;
            emit_all(io, decoder.goal_changed(session, None)).await;
        }
        Input::SetPermissions(permissions) => {
            let agent = agent_name(permissions);
            api.post(
                &format!("/api/session/{session}/agent"),
                Some(&json!({ "agent": agent })),
            )
            .await
            .map_err(|source| RunError::Agent {
                session: session.to_owned(),
                agent,
                source: Box::new(source),
            })?;
        }
        Input::Answer { key, answer } => match decoder.answer(&key, &answer) {
            Answered::Reply { reply, events } => {
                let request = match reply.method {
                    Method::Post => api.post(&reply.path, reply.body.as_ref()).await,
                    Method::Delete => api.delete(&reply.path).await,
                };
                request.map_err(|source| RunError::Answer {
                    key: key.clone(),
                    path: reply.path.clone(),
                    source: Box::new(source),
                })?;
                emit_all(io, events).await;
            }
            Answered::Invalid(events) => emit_all(io, events).await,
            Answered::Unexpected => {
                tracing::warn!(session, key, "no pending OpenCode request for this answer");
            }
        },
    }
    Ok(())
}

async fn send_input(
    api: &Api,
    session: &str,
    input: &UserInput,
    decoder: &mut Decoder,
    io: &RunIo,
) -> Result<(), RunError> {
    let failed = |source| RunError::Input {
        session: session.to_owned(),
        item: input.id.clone(),
        source: Box::new(source),
    };
    if let Some(text) = goal::argument(&input.text) {
        let files = input_files(input).await.map_err(failed)?;
        goal::command(api, session, text, &files)
            .await
            .map_err(|source| failed(InputError::Goal(GoalError::Command(source))))?;
        goal::refresh(api, session, decoder, io, true)
            .await
            .map_err(|source| failed(InputError::Goal(source)))?;
        io.emit(EngineEvent::InputAccepted {
            id: input.id.clone(),
        })
        .await;
        return Ok(());
    }
    let message = send_prompt(api, session, input).await.map_err(failed)?;
    emit_all(io, decoder.prompt_sent(&message, Some(input.id.clone()))).await;
    Ok(())
}

async fn input_files(input: &UserInput) -> Result<Vec<Value>, InputError> {
    let mut files = Vec::new();
    for path in &input.images {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|source| InputError::Image {
                path: path.clone(),
                source,
            })?;
        let mime = match path.extension().and_then(|extension| extension.to_str()) {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            _ => "image/webp",
        };
        files.push(json!({
            "uri": format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)),
            "name": path.file_name().map(|name| name.to_string_lossy().into_owned()),
        }));
    }
    Ok(files)
}

async fn send_prompt(api: &Api, session: &str, input: &UserInput) -> Result<String, InputError> {
    let files = input_files(input).await?;
    let body = json!({ "text": input.text, "files": files, "delivery": "steer" });
    let admitted = api
        .post(&format!("/api/session/{session}/prompt"), Some(&body))
        .await
        .map_err(InputError::Prompt)?;
    let message = admitted["data"]["id"]
        .as_str()
        .ok_or(InputError::NoMessageId)?;
    Ok(message.to_owned())
}

async fn emit_all(io: &RunIo, events: Vec<EngineEvent>) {
    for event in events {
        io.emit(event).await;
    }
}

/// `provider/model` (the model part may contain more slashes) plus the
/// effort as OpenCode's variant.
fn model_ref(model: Option<&str>, variant: Option<&str>) -> Option<Value> {
    let (provider, id) = model?.split_once('/')?;
    let mut model = json!({ "providerID": provider, "id": id });
    if let Some(variant) = variant.filter(|variant| !variant.is_empty()) {
        model["variant"] = json!(variant);
    }
    Some(model)
}

const fn agent_name(permissions: Permissions) -> &'static str {
    match permissions {
        Permissions::Auto => "artisan-auto",
        Permissions::Full => "artisan-full",
    }
}

fn permission(permissions: Permissions, readable: &[PathBuf]) -> Value {
    match permissions {
        Permissions::Auto => {
            let mut external = serde_json::Map::from_iter([("*".to_owned(), json!("ask"))]);
            for dir in readable {
                let pattern = format!("{}/*", dir.to_string_lossy().replace('\\', "/"));
                external.insert(pattern, json!("allow"));
            }
            json!({ "*": "allow", "bash": "ask", "external_directory": external })
        }
        Permissions::Full => {
            json!({ "*": "allow", "external_directory": "allow", "doom_loop": "allow" })
        }
    }
}

/// Writes Forge's agents into a directory of its own in the engine home.
/// OpenCode installs its plugin package there on first use, so it persists.
fn write_agents(home: &Path, readable: &[PathBuf]) -> Result<PathBuf, ServerError> {
    let dir = home.join("artisan/opencode");
    let agents: serde_json::Map<String, Value> = [Permissions::Auto, Permissions::Full]
        .into_iter()
        .map(|permissions| {
            let agent = json!({
                "mode": "primary",
                "hidden": true,
                "description": "Artisan's execution policy.",
                "permission": permission(permissions, readable),
            });
            (agent_name(permissions).to_owned(), agent)
        })
        .collect();
    let mut agents = agents;
    agents.insert(
        "artisan-summary".to_owned(),
        json!({
            "mode": "primary", "hidden": true,
            "description": "Summarize historical conversation without taking actions.",
            "permission": { "*": "deny" },
        }),
    );
    let config = json!({ "$schema": "https://opencode.ai/config.json", "agent": agents });
    let bytes = serde_json::to_vec_pretty(&config).map_err(ServerError::EncodeAgents)?;
    let path = dir.join("opencode.json");
    let current = match std::fs::read(&path) {
        Ok(current) => Some(current),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(ServerError::WriteAgents { path, source }),
    };
    if current.as_deref() != Some(bytes.as_slice()) {
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, bytes))
            .map_err(|source| ServerError::WriteAgents { path, source })?;
    }
    Ok(dir)
}

fn password() -> Result<String, ServerError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(ServerError::Password)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

struct Api {
    client: reqwest::Client,
    base: String,
    password: String,
}

/// The `/` commands this server reads, for the composer's menu; `None` when
/// it won't say, which leaves the last list in place.
async fn commands(api: &Api) -> Option<Vec<artisan_api::SlashCommand>> {
    let mut listed = None;
    for path in ["/api/command", "/command"] {
        match api.get(path).await {
            Ok(value) => {
                listed = Some(value);
                break;
            }
            Err(failure) => {
                let error: &dyn std::error::Error = &failure;
                tracing::debug!(path, error, "OpenCode didn't list its commands");
            }
        }
    }
    let listed = listed?;
    let rows = listed.as_array().or_else(|| listed["data"].as_array())?;
    Some(
        rows.iter()
            .filter_map(|command| {
                Some(artisan_api::SlashCommand {
                    name: command["name"].as_str()?.to_owned(),
                    description: command["description"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    hint: None,
                })
            })
            .collect(),
    )
}

impl Api {
    fn new(base: String, password: String) -> Result<Self, ServerError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(ServerError::Client)?;
        Ok(Self {
            client,
            base,
            password,
        })
    }

    async fn get(&self, path: &str) -> Result<Value, ApiError> {
        let request = self
            .client
            .get(format!("{}{path}", self.base))
            .timeout(REQUEST_TIMEOUT);
        self.send(request, path).await
    }

    async fn delete(&self, path: &str) -> Result<Value, ApiError> {
        let request = self
            .client
            .delete(format!("{}{path}", self.base))
            .timeout(REQUEST_TIMEOUT);
        self.send(request, path).await
    }

    async fn post(&self, path: &str, body: Option<&Value>) -> Result<Value, ApiError> {
        let mut request = self
            .client
            .post(format!("{}{path}", self.base))
            .timeout(REQUEST_TIMEOUT);
        if let Some(body) = body {
            request = request.header("content-type", "application/json").body(
                serde_json::to_vec(body).map_err(|source| ApiError::Encode {
                    path: path.to_owned(),
                    source,
                })?,
            );
        }
        self.send(request, path).await
    }

    async fn send(&self, request: reqwest::RequestBuilder, path: &str) -> Result<Value, ApiError> {
        let response = request
            .basic_auth("opencode", Some(&self.password))
            .send()
            .await
            .map_err(|source| ApiError::Send {
                path: path.to_owned(),
                source,
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|source| ApiError::Read {
            path: path.to_owned(),
            source,
        })?;
        if !status.is_success() {
            let text = String::from_utf8_lossy(&bytes);
            return Err(ApiError::Status {
                path: path.to_owned(),
                status,
                body: text.chars().take(500).collect(),
            });
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|source| ApiError::Json {
            path: path.to_owned(),
            source,
        })
    }
}

/// Opens the global event stream and waits for its first event, so nothing
/// the session does afterwards is missed. Yields each event's `data`.
async fn subscribe(api: &Api) -> Result<mpsc::Receiver<String>, SubscribeError> {
    let response = api
        .client
        .get(format!("{}/api/event", api.base))
        .header("accept", "text/event-stream")
        .basic_auth("opencode", Some(&api.password))
        .send()
        .await
        .map_err(SubscribeError::Send)?;
    if !response.status().is_success() {
        return Err(SubscribeError::Refused {
            status: response.status(),
        });
    }
    let (sender, mut receiver) = mpsc::channel(256);
    // The reader keeps the run's span, so its log names the thread.
    tokio::spawn(tracing::Instrument::in_current_span(read_events(
        response, sender,
    )));
    let first = tokio::time::timeout(REQUEST_TIMEOUT, receiver.recv())
        .await
        .map_err(|_elapsed| SubscribeError::Silent)?
        .ok_or(SubscribeError::Closed)?;
    tracing::debug!(target: "engine_events", bytes = first.len(), "OpenCode's event stream opened");
    Ok(receiver)
}

async fn read_events(response: reqwest::Response, sender: mpsc::Sender<String>) {
    let mut stream = SseStream::with_decoder(SseDecoder::with_limit(MAX_EVENT_BYTES));
    stream.attach(response.bytes_stream());
    loop {
        let event = tokio::select! {
            event = stream.next() => event,
            () = sender.closed() => return,
        };
        match event {
            Some(Ok(SseEvent::Message(message))) => {
                if sender.send(message.data).await.is_err() {
                    return;
                }
            }
            Some(Ok(SseEvent::Retry(_))) => {}
            Some(Err(error)) => {
                tracing::warn!("OpenCode event stream failed: {error}");
                return;
            }
            None => return,
        }
    }
}
