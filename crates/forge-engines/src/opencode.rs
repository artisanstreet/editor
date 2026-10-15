//! OpenCode: `opencode serve` per run, driven over its v2 HTTP API.
//!
//! One server per run, killed when the run ends. Everything the run shows
//! comes from the global event stream (`GET /api/event`), which, unlike the
//! per-session stream, carries text deltas, permission and question requests
//! and child sessions. Permissions come from agents Forge defines in its own
//! config directory (one per access × approval mode) and are answered through
//! the API. Native idle and step endings settle the root turn; typed context
//! overflow stays recoverable while compaction starts.

mod decoder;
#[cfg(test)]
mod tests;

use crate::process::{self, EngineProcess};
use crate::{EngineEvent, Harness, Input, Outcome, RunIo, Turn, UserInput};
use anyhow::{Context, Result, anyhow, bail, ensure};
use artisan_api::Permissions;
use base64::Engine as _;
use decoder::Decoder;
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
const MAX_EVENT_BYTES: usize = process::MAX_LINE_BYTES;

pub(crate) struct OpenCodeHarness;

impl Harness for OpenCodeHarness {
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
            .map_err(|_| {
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
    let home = &turn.home;
    let mut launch = turn.clone();
    // The engine home holds OpenCode's config, data (auth, sessions) and
    // caches; customisation's environment may override these.
    let mut env: Vec<(String, String)> = [
        ("XDG_CONFIG_HOME", home.join("config")),
        ("XDG_DATA_HOME", home.join("data")),
        ("XDG_CACHE_HOME", home.join("cache")),
        ("XDG_STATE_HOME", home.join("state")),
    ]
    .into_iter()
    .map(|(key, path)| (key.to_owned(), path.to_string_lossy().into_owned()))
    .collect();
    env.extend(turn.launch.env.iter().cloned());
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
    check_model(api, &session).await?;

    let mut decoder = Decoder::new(Some(session.clone()));
    let message = send_prompt(api, &session, &turn.input).await?;
    emit_all(
        io,
        decoder.prompt_sent(&message, Some(turn.input.id.clone())),
    )
    .await;
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
    Ok(match decoder.error.take() {
        Some(error) => Outcome::failed(error),
        None => Outcome::done(),
    })
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
            // `steer` folds the message in at the next step boundary, or
            // starts a new turn when the session is idle.
            let message = send_prompt(api, session, &input).await?;
            emit_all(io, decoder.prompt_sent(&message, Some(input.id))).await;
        }
        Input::Answer { key, answer } => {
            let Some((reply, events)) = decoder.answer(&key, &answer) else {
                tracing::warn!(session, key, "no pending OpenCode request for this answer");
                return Ok(());
            };
            emit_all(io, events).await;
            api.post(&reply.path, reply.body.as_ref())
                .await
                .with_context(|| {
                    format!("answering OpenCode request {} for item {key}", reply.path)
                })?;
        }
    }
    Ok(())
}

async fn send_prompt(api: &Api, session: &str, input: &UserInput) -> Result<String> {
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
    let body = json!({ "prompt": { "text": input.text, "files": files }, "delivery": "steer" });
    let admitted = api
        .post(&format!("/api/session/{session}/prompt"), Some(&body))
        .await
        .with_context(|| format!("sending message {} to OpenCode session {session}", input.id))?;
    Ok(admitted["data"]["id"]
        .as_str()
        .context("OpenCode admitted a prompt without a message id")?
        .to_owned())
}

/// OpenCode logs an unavailable model and goes idle without an event, which
/// would leave the run waiting forever, so check the session's model first.
async fn check_model(api: &Api, session: &str) -> Result<()> {
    let info = api
        .get(&format!("/api/session/{session}"))
        .await
        .context("reading the OpenCode session")?;
    let (Some(id), Some(provider)) = (
        info["data"]["model"]["id"].as_str(),
        info["data"]["model"]["providerID"].as_str(),
    ) else {
        return Ok(());
    };
    let catalog = api
        .get("/api/model")
        .await
        .context("listing OpenCode models")?;
    let models = catalog["data"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    // A fresh server loads its catalog in the background; an empty one
    // says nothing about the model.
    if models.is_empty() {
        return Ok(());
    }
    let available = models.iter().any(|model| {
        model["id"].as_str() == Some(id)
            && model["providerID"].as_str() == Some(provider)
            && model["enabled"].as_bool() != Some(false)
    });
    ensure!(
        available,
        "OpenCode can't run {provider}/{id}: it is not among the {} models its session runner offers",
        models.len()
    );
    Ok(())
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

fn agent_name(permissions: Permissions) -> &'static str {
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
    tokio::spawn(read_events(response, sender));
    let first = tokio::time::timeout(REQUEST_TIMEOUT, receiver.recv())
        .await
        .map_err(|_| anyhow!("OpenCode's event stream sent nothing"))?
        .context("OpenCode's event stream closed at once")?;
    tracing::debug!(target: "engine_stdout", "{first}");
    Ok(receiver)
}

async fn read_events(response: reqwest::Response, sender: mpsc::Sender<String>) {
    let mut stream = SseStream::with_decoder(SseDecoder::with_limit(
        NonZeroUsize::new(MAX_EVENT_BYTES).unwrap(),
    ));
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
