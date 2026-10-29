//! OpenCode: `opencode serve` per run, driven over its v2 HTTP API.
//!
//! One server per run, killed when the run ends. Everything the run shows
//! comes from the global event stream (`GET /api/event`), which, unlike the
//! per-session stream, carries text deltas, permission and question requests
//! and child sessions. Permissions come from agents Forge defines in its own
//! config directory (one per access × approval mode) and are answered through
//! the API, questions through the forms OpenCode asks them with. The root
//! session's execution events (`started`, `succeeded`, `failed`,
//! `interrupted`) bound the turn; OpenCode compacts a full context itself.

mod decoder;
mod goal;
#[cfg(test)]
#[path = "../tests/opencode/goal.rs"]
mod goal_tests;
mod side;
#[cfg(test)]
mod tests;

use crate::process::{self, EngineProcess};
use crate::{EngineEvent, Harness, Input, Outcome, RunIo, Turn, UserInput};
use anyhow::{Context, Result, anyhow, bail, ensure};
use artisan_api::Permissions;
use base64::Engine as _;
use decoder::{Decoder, Method};
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
/// Oversized events terminate the run instead of silently skipping state.
const MAX_EVENT_BYTES: NonZeroUsize = NonZeroUsize::new(process::MAX_LINE_BYTES).unwrap();

pub struct OpenCodeHarness;

impl Harness for OpenCodeHarness {
    fn clear_goal(&self, turn: Turn) -> BoxFuture<'static, Result<()>> {
        Box::pin(goal::clear_goal(turn))
    }

    fn side_query(
        &self,
        turn: Turn,
        cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'static, Result<crate::SideReply>> {
        Box::pin(side::side_query(turn, cancel))
    }

    fn set_title(&self, turn: Turn, title: String) -> BoxFuture<'static, Result<()>> {
        Box::pin(async move {
            let session = turn
                .session_id
                .as_deref()
                .context("naming OpenCode needs a session")?;
            let password = password()?;
            let config_dir = write_agents(&turn.home)?;
            let mut server = spawn(&turn, &password, &config_dir)?;
            let result = async {
                let base = tokio::time::timeout(READY_TIMEOUT, ready(&mut server)).await??;
                let api = Api::new(base, password)?;
                let path = format!("/api/session/{session}");
                let request = api
                    .client
                    .patch(format!("{}{path}", api.base))
                    .timeout(REQUEST_TIMEOUT)
                    .header("content-type", "application/json")
                    .body(serde_json::to_vec(&json!({ "title": title }))?);
                api.send(request, &path).await.map(drop)
            }
            .await;
            server.kill().await;
            server.wait().await;
            result
        })
    }

    fn compact(
        &self,
        turn: Turn,
        cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'static, Result<crate::SideReply>> {
        Box::pin(side::compact(turn, cancel))
    }

    fn run(&self, turn: Turn, mut io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            match run(&turn, &mut io).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    tracing::warn!(thread = %turn.thread_id, "OpenCode run failed: {error:#}");
                    Outcome::failed(format!("{error:#}"))
                }
            }
        })
    }
}

async fn run(turn: &Turn, io: &mut RunIo) -> Result<Outcome> {
    let password = password()?;
    let config_dir = write_agents(&turn.home)?;
    let mut server = spawn(turn, &password, &config_dir)?;
    let cancel = io.cancel.clone();
    let result = tokio::select! {
        biased;
        () = cancel.cancelled() => Ok(Outcome::cancelled()),
        result = async {
        let base = tokio::time::timeout(READY_TIMEOUT, ready(&mut server))
            .await
            .map_err(|_elapsed| {
                anyhow!(
                    "OpenCode did not start within {} s",
                    READY_TIMEOUT.as_secs()
                )
            })??;
        let api = Api::new(base, password)?;
        drive(turn, io, &api, &mut server).await
        } => result,
    };
    server.kill().await;
    let _ = server.wait().await;
    result.map_err(|error| match server.stderr_tail() {
        tail if tail.is_empty() => error,
        tail => error.context(format!("OpenCode stderr: {tail}")),
    })
}

fn spawn(turn: &Turn, password: &str, config_dir: &Path) -> Result<EngineProcess> {
    let mut launch = turn.clone();
    // The launch environment already roots OpenCode's config, data (auth,
    // sessions) and caches in the engine home (`engines::environment`).
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
    process::spawn(&launch, &args)
}

/// Waits for `opencode server listening on http://127.0.0.1:<port>`.
async fn ready(server: &mut EngineProcess) -> Result<String> {
    while let Some(line) = server.next_line().await? {
        if let Some((_, url)) = line.split_once("listening on ") {
            return Ok(url.trim().trim_end_matches('/').to_owned());
        }
        tracing::debug!(target: "engine_stdout", "{line}");
    }
    bail!("OpenCode exited before it was ready")
}

async fn drive(
    turn: &Turn,
    io: &mut RunIo,
    api: &Api,
    server: &mut EngineProcess,
) -> Result<Outcome> {
    let mut events = subscribe(api).await?;
    let agent = agent_name(turn.config.permissions);
    let variant = turn.config.variant.as_deref();
    let model = model_ref(turn.config.model.as_deref(), variant);
    let session = if let Some(session) = &turn.session_id {
        api.post(
            &format!("/api/session/{session}/agent"),
            Some(&json!({ "agent": agent })),
        )
        .await
        .with_context(|| format!("switching OpenCode session {session} to agent {agent}"))?;
        if let Some(model) = &model {
            api.post(
                &format!("/api/session/{session}/model"),
                Some(&json!({ "model": model })),
            )
            .await
            .with_context(|| format!("switching OpenCode session {session} to {model}"))?;
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
            .context("creating an OpenCode session")?;
        created["data"]["id"]
            .as_str()
            .context("OpenCode created a session without an id")?
            .to_owned()
    };
    io.emit(EngineEvent::Session {
        id: session.clone(),
    })
    .await;
    if let Some(commands) = commands(api).await {
        io.emit(EngineEvent::Commands(commands)).await;
    }

    let mut decoder = Decoder::new(Some(session.clone()));
    if let Err(error) = goal::refresh(api, &session, &mut decoder, io, false).await {
        tracing::debug!(session, %error, "OpenCode goal unavailable");
    }
    send_input(api, &session, &turn.input, &mut decoder, io).await?;
    loop {
        if decoder.idle() {
            // A steer that raced the end of the turn starts the next one.
            match io.inbox.try_recv() {
                Ok(input) => {
                    handle_input(api, &session, &mut decoder, io, input).await?;
                    continue;
                }
                Err(_) => break,
            }
        }
        tokio::select! {
            biased;
            () = io.cancel.cancelled() => {
                if let Err(error) = api.post(&format!("/api/session/{session}/interrupt"), None).await {
                    tracing::warn!(session, "interrupting OpenCode failed: {error:#}");
                }
                emit_all(io, decoder.cancel()).await;
                return Ok(Outcome::cancelled());
            }
            Some(input) = io.inbox.recv() => handle_input(api, &session, &mut decoder, io, input).await?,
            event = events.recv() => match event {
                Some(data) => emit_all(io, decoder.feed(&data)?).await,
                None => bail!("OpenCode's event stream closed mid-run"),
            },
            line = server.next_line() => match line {
                Ok(Some(line)) => tracing::debug!(target: "engine_stdout", "{line}"),
                _ => bail!("OpenCode exited mid-run"),
            },
        }
    }
    Ok(decoder
        .error
        .take()
        .map_or_else(Outcome::done, Outcome::failed))
}

async fn handle_input(
    api: &Api,
    session: &str,
    decoder: &mut Decoder,
    io: &RunIo,
    input: Input,
) -> Result<()> {
    match input {
        Input::Steer(input) => {
            send_input(api, session, &input, decoder, io).await?;
        }
        Input::ClearGoal => {
            goal::command(api, session, "clear", &[]).await?;
            emit_all(io, decoder.goal_changed(session, None)).await;
        }
        // The session's agent carries its rules; its next step asks by them.
        Input::SetPermissions(permissions) => {
            let agent = agent_name(permissions);
            api.post(
                &format!("/api/session/{session}/agent"),
                Some(&json!({ "agent": agent })),
            )
            .await
            .with_context(|| format!("switching OpenCode session {session} to agent {agent}"))?;
        }
        Input::Answer { key, answer } => {
            let Some((reply, events)) = decoder.answer(&key, &answer) else {
                tracing::warn!(session, key, "no pending OpenCode request for this answer");
                return Ok(());
            };
            emit_all(io, events).await;
            let request = match reply.method {
                Method::Post => api.post(&reply.path, reply.body.as_ref()).await,
                Method::Delete => api.delete(&reply.path).await,
            };
            request.with_context(|| {
                format!("answering OpenCode request {} for item {key}", reply.path)
            })?;
        }
    }
    Ok(())
}

async fn send_input(
    api: &Api,
    session: &str,
    input: &UserInput,
    decoder: &mut Decoder,
    io: &RunIo,
) -> Result<()> {
    if let Some(text) = goal::argument(&input.text) {
        let files = input_files(input).await?;
        goal::command(api, session, text, &files).await?;
        goal::refresh(api, session, decoder, io, true).await?;
        io.emit(EngineEvent::InputAccepted {
            id: input.id.clone(),
        })
        .await;
        return Ok(());
    }
    let message = send_prompt(api, session, input).await?;
    emit_all(io, decoder.prompt_sent(&message, Some(input.id.clone()))).await;
    Ok(())
}

async fn input_files(input: &UserInput) -> Result<Vec<Value>> {
    let mut files = Vec::new();
    for path in &input.images {
        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("reading image {}", path.display()))?;
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

async fn send_prompt(api: &Api, session: &str, input: &UserInput) -> Result<String> {
    let files = input_files(input).await?;
    let body = json!({ "text": input.text, "files": files, "delivery": "steer" });
    let admitted = api
        .post(&format!("/api/session/{session}/prompt"), Some(&body))
        .await
        .with_context(|| format!("sending message {} to OpenCode session {session}", input.id))?;
    let message = admitted["data"]["id"]
        .as_str()
        .context("OpenCode admitted a prompt without a message id")?;
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

/// OpenCode permission rules (later keys win; `*` sorts first). OpenCode
/// has no sandbox or automatic mode: Auto edits inside the project and asks
/// before shell commands and other folders.
fn permission(permissions: Permissions) -> Value {
    match permissions {
        Permissions::Auto => json!({ "*": "allow", "bash": "ask", "external_directory": "ask" }),
        Permissions::Full => {
            json!({ "*": "allow", "external_directory": "allow", "doom_loop": "allow" })
        }
    }
}

/// Writes Forge's agents into a directory of its own in the engine home.
/// OpenCode installs its plugin package there on first use, so it persists.
fn write_agents(home: &Path) -> Result<PathBuf> {
    let dir = home.join("artisan/opencode");
    let agents: serde_json::Map<String, Value> = [Permissions::Auto, Permissions::Full]
        .into_iter()
        .map(|permissions| {
            let agent = json!({
                "mode": "primary",
                "hidden": true,
                "description": "Artisan's execution policy.",
                "permission": permission(permissions),
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
    let bytes = serde_json::to_vec_pretty(&config)?;
    let path = dir.join("opencode.json");
    if std::fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(dir)
}

fn password() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow!("no OS randomness for the OpenCode password: {error}"))?;
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
            Err(error) => tracing::debug!(path, error = format!("{error:#}"), "OpenCode commands"),
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
    fn new(base: String, password: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .context("building the OpenCode HTTP client")?;
        Ok(Self {
            client,
            base,
            password,
        })
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let request = self
            .client
            .get(format!("{}{path}", self.base))
            .timeout(REQUEST_TIMEOUT);
        self.send(request, path).await
    }

    async fn delete(&self, path: &str) -> Result<Value> {
        let request = self
            .client
            .delete(format!("{}{path}", self.base))
            .timeout(REQUEST_TIMEOUT);
        self.send(request, path).await
    }

    async fn post(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        let mut request = self
            .client
            .post(format!("{}{path}", self.base))
            .timeout(REQUEST_TIMEOUT);
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(serde_json::to_vec(body)?);
        }
        self.send(request, path).await
    }

    async fn send(&self, request: reqwest::RequestBuilder, path: &str) -> Result<Value> {
        let response = request
            .basic_auth("opencode", Some(&self.password))
            .send()
            .await
            .with_context(|| format!("calling OpenCode {path}"))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("reading OpenCode's reply to {path}"))?;
        if !status.is_success() {
            let text = String::from_utf8_lossy(&bytes);
            let text: String = text.chars().take(500).collect();
            bail!("OpenCode {path} answered {status}: {text}");
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes)
            .with_context(|| format!("OpenCode {path} answered with invalid JSON"))
    }
}

/// Opens the global event stream and waits for its first event, so nothing
/// the session does afterwards is missed. Yields each event's `data`.
async fn subscribe(api: &Api) -> Result<mpsc::Receiver<String>> {
    let response = api
        .client
        .get(format!("{}/api/event", api.base))
        .header("accept", "text/event-stream")
        .basic_auth("opencode", Some(&api.password))
        .send()
        .await
        .context("subscribing to OpenCode events")?;
    ensure!(
        response.status().is_success(),
        "OpenCode refused the event stream: {}",
        response.status()
    );
    let (sender, mut receiver) = mpsc::channel(256);
    // The reader keeps the run's span, so its log names the thread.
    tokio::spawn(tracing::Instrument::in_current_span(read_events(
        response, sender,
    )));
    let first = tokio::time::timeout(REQUEST_TIMEOUT, receiver.recv())
        .await
        .map_err(|_elapsed| anyhow!("OpenCode's event stream sent nothing"))?
        .context("OpenCode's event stream closed at once")?;
    tracing::debug!(target: "engine_stdout", "{first}");
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
