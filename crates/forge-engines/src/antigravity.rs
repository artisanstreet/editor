mod brain;
mod decoder;
mod protocol;
#[cfg(test)]
#[path = "../tests/antigravity/unit.rs"]
mod tests;
mod wake;

pub use protocol::FrameError;

use crate::process::{self, EngineProcess, ReadError, SendError, SpawnError, StderrWritten};
use crate::rpc::RpcError;
use crate::{EngineEvent, Harness, Input, Outcome, Retry, RunIo, Turn, UserInput};
use artisan_api::{ItemId, Permissions, SignInState};
use base64::Engine as _;
use decoder::{Answered, Decoder, Formed, Permitted};
use futures::future::BoxFuture;
use protocol::{ConfigOption, Frame, Opened, Prompted, StopReason};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wake::{Due, Wake};

pub(crate) const GEMINI_DIR: &str = ".gemini";
pub(crate) const LOGIN_ARGS: &[&str] = &["acp", "--login"];
const SERVE: &str = "acp";
const OPEN_TIMEOUT: Duration = Duration::from_secs(90);
const CANCEL_GRACE: Duration = Duration::from_secs(5);
const AUTO_MODE: &str = "auto";
const ASKING_MODE: &str = "default";
const QUOTA_MARKERS: [&str; 3] = ["QUOTA_EXHAUSTED", "RESOURCE_EXHAUSTED", "quota"];
const SIGN_IN: &str = "Sign in to Antigravity in Settings, then send the message again.";
const SILENT: &str = "Antigravity stopped responding for 15 minutes, so Forge stopped the turn.";

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("start Antigravity")]
    Spawn(#[source] SpawnError),
    #[error("write {method} to Antigravity")]
    Send {
        method: &'static str,
        #[source]
        source: SendError,
    },
    #[error("read Antigravity's output")]
    Read(#[source] ReadError),
    #[error("decode Antigravity's output")]
    Frame(#[source] FrameError),
    #[error("Antigravity exited; {stderr}")]
    Exited { stderr: StderrWritten },
    #[error("Antigravity didn't answer {method} within {}s", OPEN_TIMEOUT.as_secs())]
    TimedOut { method: &'static str },
    #[error("Antigravity refused {method}")]
    Refused {
        method: &'static str,
        #[source]
        source: RpcError,
    },
    #[error("Antigravity answered {method} in a shape Forge can't read")]
    Answer {
        method: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("Antigravity opened a session without an id")]
    NoSession,
    #[error("read image {}", path.display())]
    Image {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl RunError {
    fn signed_out(&self) -> bool {
        matches!(self, Self::Refused { source, .. } if source.code() == Some(protocol::SIGN_IN_REQUIRED))
    }
}

#[derive(Debug)]
pub struct AntigravityHarness;

impl Harness for AntigravityHarness {
    fn run(&self, turn: Turn, mut io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            let mut decoder = Decoder::default();
            let outcome = match run(&turn, &mut io, &mut decoder).await {
                Ok(outcome) => outcome,
                Err(failure) if failure.signed_out() => Outcome::failed(SIGN_IN),
                Err(failure) => {
                    let error: &dyn std::error::Error = &failure;
                    tracing::warn!(thread = %turn.thread_id, session = ?turn.session_id, error, "Antigravity run failed");
                    Outcome::failed(crate::rpc::told(&failure))
                }
            };
            for event in decoder.finish() {
                io.emit(event).await;
            }
            outcome
        })
    }
}

pub(crate) fn environment() -> Vec<(String, String)> {
    vec![("AGY_CLI_DISABLE_AUTO_UPDATE".into(), "1".into())]
}

pub(crate) fn profile_args(home: &Path) -> Vec<String> {
    vec![
        "--gemini-dir".to_owned(),
        home.join(GEMINI_DIR).display().to_string(),
    ]
}

struct Connection {
    process: EngineProcess,
    next: u64,
    backlog: VecDeque<Frame>,
}

impl Connection {
    async fn initialize(process: EngineProcess) -> Result<Self, RunError> {
        let mut connection = Self {
            process,
            next: 0,
            backlog: VecDeque::new(),
        };
        connection
            .call("initialize", protocol::initialize())
            .await?;
        Ok(connection)
    }

    async fn write(&mut self, method: &'static str, message: &Value) -> Result<(), RunError> {
        self.process
            .send(message)
            .await
            .map_err(|source| RunError::Send { method, source })
    }

    async fn request(&mut self, method: &'static str, params: Value) -> Result<u64, RunError> {
        self.next += 1;
        let id = self.next;
        self.write(
            method,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .await?;
        Ok(id)
    }

    async fn notify(&mut self, method: &'static str, params: Value) -> Result<(), RunError> {
        self.write(
            method,
            &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
        .await
    }

    async fn reply(&mut self, id: Value, result: Value) -> Result<(), RunError> {
        self.write(
            "a reply",
            &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        )
        .await
    }

    async fn refuse(&mut self, id: Value, method: &str) -> Result<(), RunError> {
        let message = format!("Artisan doesn't offer {method}");
        self.write(
            "a refusal",
            &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": crate::rpc::METHOD_NOT_FOUND, "message": message } }),
        )
        .await
    }

    async fn read(&mut self) -> Result<Frame, RunError> {
        let line = self.process.next_line().await.map_err(RunError::Read)?;
        let Some(line) = line else {
            return Err(RunError::Exited {
                stderr: self.process.stderr(),
            });
        };
        Frame::parse(&line).map_err(RunError::Frame)
    }

    async fn frame(&mut self) -> Result<Frame, RunError> {
        match self.backlog.pop_front() {
            Some(frame) => Ok(frame),
            None => self.read().await,
        }
    }

    async fn call(&mut self, method: &'static str, params: Value) -> Result<Value, RunError> {
        let id = self.request(method, params).await?;
        tokio::time::timeout(OPEN_TIMEOUT, async {
            loop {
                match self.read().await? {
                    Frame::Response {
                        id: answered,
                        result,
                    } if answered == id => {
                        return result.map_err(|source| RunError::Refused { method, source });
                    }
                    Frame::Unsupported { id, method } => self.refuse(id, &method).await?,
                    frame @ (Frame::Response { .. }
                    | Frame::Update { .. }
                    | Frame::Notice { .. }
                    | Frame::Permission { .. }
                    | Frame::Form { .. }) => self.backlog.push_back(frame),
                }
            }
        })
        .await
        .map_err(|_elapsed| RunError::TimedOut { method })?
    }

    async fn end(mut self) {
        self.process.kill().await;
        self.process.finish().await;
    }
}

fn answer<T: serde::de::DeserializeOwned>(
    method: &'static str,
    value: Value,
) -> Result<T, RunError> {
    serde_json::from_value(value).map_err(|source| RunError::Answer { method, source })
}

async fn start(turn: &Turn) -> Result<Connection, RunError> {
    let mut args = vec![SERVE.to_owned()];
    args.extend(profile_args(&turn.home));
    let process = process::spawn(turn, &args).await.map_err(RunError::Spawn)?;
    Connection::initialize(process).await
}

struct Session {
    id: String,
    modes: Vec<String>,
    mode: Option<String>,
    options: Vec<ConfigOption>,
}

async fn open(turn: &Turn, connection: &mut Connection, io: &RunIo) -> Result<Session, RunError> {
    let servers = protocol::mcp_servers(&turn.launch.mcp_servers);
    let cwd = turn.cwd.display().to_string();
    if let Some(id) = turn.session_id.as_ref().filter(|id| !id.is_empty()) {
        let params = json!({ "sessionId": id, "cwd": cwd, "mcpServers": servers });
        match connection.call("session/resume", params).await {
            Ok(value) => return session(answer("session/resume", value)?, Some(id.clone())),
            Err(failure) if failure.signed_out() => return Err(failure),
            Err(RunError::Refused { source, .. }) => {
                tracing::info!(thread = %turn.thread_id, session = id, error = %source, said = ?source.said(), "Antigravity couldn't resume the session; starting a new one");
                io.emit(EngineEvent::SessionGone).await;
            }
            Err(failure) => return Err(failure),
        }
    }
    let value = connection
        .call("session/new", json!({ "cwd": cwd, "mcpServers": servers }))
        .await?;
    session(answer("session/new", value)?, None)
}

fn session(opened: Opened, resumed: Option<String>) -> Result<Session, RunError> {
    let id = resumed
        .or(opened.session_id)
        .filter(|id| !id.is_empty())
        .ok_or(RunError::NoSession)?;
    let (mode, modes) = opened.modes.map_or((None, Vec::new()), |modes| {
        (
            modes.current_mode_id,
            modes
                .available_modes
                .into_iter()
                .map(|mode| mode.id)
                .collect(),
        )
    });
    Ok(Session {
        id,
        modes,
        mode,
        options: opened.config_options,
    })
}

fn mode_for(permissions: Permissions, modes: &[String]) -> Option<&'static str> {
    let wanted = match permissions {
        Permissions::Auto if modes.iter().any(|mode| mode == AUTO_MODE) => AUTO_MODE,
        Permissions::Auto | Permissions::Full => ASKING_MODE,
    };
    modes.iter().any(|mode| mode == wanted).then_some(wanted)
}

async fn apply_mode(
    connection: &mut Connection,
    session: &mut Session,
    permissions: Permissions,
) -> Result<(), RunError> {
    let Some(wanted) = mode_for(permissions, &session.modes) else {
        return Ok(());
    };
    if session.mode.as_deref() == Some(wanted) {
        return Ok(());
    }
    connection
        .call(
            "session/set_mode",
            json!({ "sessionId": session.id, "modeId": wanted }),
        )
        .await?;
    session.mode = Some(wanted.to_owned());
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Configured {
    #[serde(default)]
    config_options: Option<Vec<ConfigOption>>,
}

async fn apply_option(
    connection: &mut Connection,
    session: &mut Session,
    category: &str,
    wanted: Option<&str>,
) -> Result<(), RunError> {
    let Some(wanted) = wanted.filter(|wanted| !wanted.is_empty()) else {
        return Ok(());
    };
    let Some(option) = session.options.iter().find(|option| option.is(category)) else {
        return Ok(());
    };
    if option.current_value.as_deref() == Some(wanted)
        || !option.choices().iter().any(|choice| choice.value == wanted)
    {
        return Ok(());
    }
    let value = connection
        .call(
            "session/set_config_option",
            json!({ "sessionId": session.id, "configId": option.id, "value": wanted }),
        )
        .await?;
    let configured: Configured = answer("session/set_config_option", value)?;
    match configured.config_options {
        Some(options) => session.options = options,
        None => {
            if let Some(option) = session
                .options
                .iter_mut()
                .find(|option| option.is(category))
            {
                option.current_value = Some(wanted.to_owned());
            }
        }
    }
    Ok(())
}

async fn run(turn: &Turn, io: &mut RunIo, decoder: &mut Decoder) -> Result<Outcome, RunError> {
    let cancel = io.cancel.clone();
    let mut connection = tokio::select! {
        biased;
        () = cancel.cancelled() => return Ok(Outcome::Cancelled),
        connection = start(turn) => connection?,
    };
    let result = tokio::select! {
        biased;
        () = cancel.cancelled() => Ok(Outcome::Cancelled),
        result = drive(turn, io, decoder, &mut connection) => result,
    };
    connection.end().await;
    result
}

struct Turnstile {
    prompts: HashMap<u64, ItemId>,
    permissions: Permissions,
    outcome: Outcome,
}

async fn drive(
    turn: &Turn,
    io: &mut RunIo,
    decoder: &mut Decoder,
    connection: &mut Connection,
) -> Result<Outcome, RunError> {
    let mut session = open(turn, connection, io).await?;
    io.emit(EngineEvent::Session {
        id: session.id.clone(),
    })
    .await;
    let levels = thought_levels(&session.options);
    apply_mode(connection, &mut session, turn.config.permissions).await?;
    apply_option(
        connection,
        &mut session,
        protocol::MODEL,
        turn.config.model.as_deref(),
    )
    .await?;
    let effort = match current_thinking(&session.options, &levels) {
        Thinking::Fixed(_) => None,
        Thinking::Orchestrator | Thinking::Adjustable => turn.config.effort.as_deref(),
    };
    apply_option(connection, &mut session, protocol::THOUGHT_LEVEL, effort).await?;
    let mut configured = Vec::new();
    decoder.configured(&session.options, &mut configured);
    emit_all(io, configured).await;
    let mut state = Turnstile {
        prompts: HashMap::new(),
        permissions: turn.config.permissions,
        outcome: Outcome::Done,
    };
    prompt(connection, &session, &mut state, decoder, io, &turn.input).await?;
    let cancel = io.cancel.clone();
    let gemini_dir = turn.home.join(GEMINI_DIR);
    loop {
        let settled = state.prompts.is_empty() && !decoder.waiting();
        if settled && decoder.wake() == Wake::Quiet {
            match io.inbox.try_recv() {
                Ok(input) => {
                    input_arrived(connection, &mut session, &mut state, decoder, io, input).await?;
                    continue;
                }
                Err(_) => return Ok(state.outcome),
            }
        }
        let due = decoder.due();
        let lull = async {
            match due {
                Some((at, due)) => {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await;
                    due
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => return stop(connection, &session, &mut state, decoder, io).await,
            Some(input) = io.inbox.recv() => {
                input_arrived(connection, &mut session, &mut state, decoder, io, input).await?;
            }
            frame = connection.frame() => {
                frame_arrived(connection, &mut state, decoder, io, frame?).await?;
            }
            due = lull => match due {
                Due::Probe => {
                    let pending = pending_wake(&gemini_dir, &session.id).await;
                    emit_all(io, decoder.probed(pending, Instant::now())).await;
                }
                Due::Lapse => emit_all(io, decoder.lapse(Instant::now())).await,
                Due::Abandon => {
                    tracing::warn!(
                        thread = %turn.thread_id,
                        session = session.id,
                        silence_secs = wake::PROMPT_SILENCE.as_secs(),
                        "Antigravity went silent mid-prompt; stopping it"
                    );
                    stop(connection, &session, &mut state, decoder, io).await?;
                    return Ok(Outcome::failed(SILENT));
                }
            },
        }
    }
}

async fn pending_wake(gemini_dir: &Path, session: &str) -> bool {
    match brain::pending_wake(gemini_dir, session).await {
        Ok(pending) => pending,
        Err(failure) => {
            let error: &dyn std::error::Error = &failure;
            tracing::warn!(
                session,
                error,
                "couldn't read Antigravity's queued wakes; waiting for one"
            );
            true
        }
    }
}

async fn prompt(
    connection: &mut Connection,
    session: &Session,
    state: &mut Turnstile,
    decoder: &mut Decoder,
    io: &RunIo,
    input: &UserInput,
) -> Result<(), RunError> {
    let blocks = blocks(input).await?;
    let id = connection
        .request(
            "session/prompt",
            json!({ "sessionId": session.id, "prompt": blocks }),
        )
        .await?;
    state.prompts.insert(id, input.id.clone());
    emit_all(io, decoder.prompted(Instant::now())).await;
    io.emit(EngineEvent::InputAccepted {
        id: input.id.clone(),
    })
    .await;
    Ok(())
}

async fn blocks(input: &UserInput) -> Result<Vec<Value>, RunError> {
    let mut blocks = Vec::with_capacity(input.images.len() + 1);
    for path in &input.images {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|source| RunError::Image {
                path: path.clone(),
                source,
            })?;
        let mime = match path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("bmp") => "image/bmp",
            _ => "image/webp",
        };
        blocks.push(json!({
            "type": "image",
            "mimeType": mime,
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        }));
    }
    if !input.text.is_empty() || blocks.is_empty() {
        blocks.insert(0, json!({ "type": "text", "text": input.text }));
    }
    Ok(blocks)
}

async fn input_arrived(
    connection: &mut Connection,
    session: &mut Session,
    state: &mut Turnstile,
    decoder: &mut Decoder,
    io: &RunIo,
    input: Input,
) -> Result<(), RunError> {
    match input {
        Input::Steer(input) => prompt(connection, session, state, decoder, io, &input).await?,
        Input::Answer { key, answer } => match decoder.answer(&key, &answer, Instant::now()) {
            Answered::Reply {
                rpc,
                result,
                events,
            } => {
                connection.reply(rpc, result).await?;
                emit_all(io, events).await;
            }
            Answered::Invalid(events) => emit_all(io, events).await,
            Answered::Unexpected => {
                tracing::warn!(
                    session = session.id,
                    key,
                    "no pending Antigravity request for this answer"
                );
            }
        },
        Input::SetPermissions(permissions) => {
            state.permissions = permissions;
            let wanted = mode_for(permissions, &session.modes);
            if let Some(wanted) = wanted.filter(|wanted| session.mode.as_deref() != Some(wanted)) {
                connection
                    .request(
                        "session/set_mode",
                        json!({ "sessionId": session.id, "modeId": wanted }),
                    )
                    .await?;
                session.mode = Some(wanted.to_owned());
            }
        }
        Input::ClearGoal => {
            tracing::debug!(session = session.id, "Antigravity has no goals to clear");
        }
    }
    Ok(())
}

async fn frame_arrived(
    connection: &mut Connection,
    state: &mut Turnstile,
    decoder: &mut Decoder,
    io: &RunIo,
    frame: Frame,
) -> Result<(), RunError> {
    match frame {
        Frame::Response { id, result } => {
            if state.prompts.remove(&id).is_none() {
                if let Err(error) = result {
                    tracing::warn!(id, %error, said = ?error.said(), "Antigravity refused a request");
                }
                return Ok(());
            }
            match result {
                Ok(value) => {
                    let prompted: Prompted = answer("session/prompt", value)?;
                    let superseded = !state.prompts.is_empty();
                    emit_all(io, decoder.ended(&prompted, superseded, Instant::now())).await;
                    if !superseded {
                        state.outcome = stopped(prompted.stop_reason);
                    }
                }
                Err(source) => {
                    let failure = RunError::Refused {
                        method: "session/prompt",
                        source,
                    };
                    state.outcome = refused(&failure);
                    if state.prompts.is_empty() {
                        emit_all(io, decoder.abandoned()).await;
                    }
                }
            }
        }
        Frame::Update { update, .. } => emit_all(io, decoder.update(*update, Instant::now())).await,
        Frame::Permission { id, request } => {
            match decoder.permission(id, *request, state.permissions, Instant::now()) {
                Permitted::Ask(events) => emit_all(io, events).await,
                Permitted::Reply {
                    rpc,
                    result,
                    events,
                } => {
                    connection.reply(rpc, result).await?;
                    emit_all(io, events).await;
                }
            }
        }
        Frame::Form { id, request } => match decoder.form(id, *request, Instant::now()) {
            Formed::Asked(events) => emit_all(io, events).await,
            Formed::Refused {
                rpc,
                result,
                events,
            } => {
                emit_all(io, events).await;
                connection.reply(rpc, result).await?;
            }
        },
        Frame::Unsupported { id, method } => connection.refuse(id, &method).await?,
        Frame::Notice { method } => {
            tracing::debug!(method, "Antigravity sent a notice Forge doesn't use");
        }
    }
    Ok(())
}

fn stopped(reason: StopReason) -> Outcome {
    match reason {
        StopReason::EndTurn => Outcome::Done,
        StopReason::Cancelled => Outcome::Cancelled,
        StopReason::Refusal => Outcome::Refused,
        StopReason::MaxTokens => {
            Outcome::failed("Antigravity stopped: the reply reached the model's output limit.")
        }
        StopReason::MaxTurnRequests => {
            Outcome::failed("Antigravity stopped: the turn reached its limit of model requests.")
        }
    }
}

fn refused(failure: &RunError) -> Outcome {
    if failure.signed_out() {
        return Outcome::failed(SIGN_IN);
    }
    let told = crate::rpc::told(failure);
    if QUOTA_MARKERS.iter().any(|marker| told.contains(marker)) {
        Outcome::retryable(told, Retry::UsageLimit { resets_at: None })
    } else {
        Outcome::failed(told)
    }
}

async fn stop(
    connection: &mut Connection,
    session: &Session,
    state: &mut Turnstile,
    decoder: &mut Decoder,
    io: &RunIo,
) -> Result<Outcome, RunError> {
    let (replies, events) = decoder.cancel();
    for (rpc, result) in replies {
        connection.reply(rpc, result).await?;
    }
    emit_all(io, events).await;
    connection
        .notify("session/cancel", json!({ "sessionId": session.id }))
        .await?;
    let settled = tokio::time::timeout(CANCEL_GRACE, async {
        while !state.prompts.is_empty() {
            match connection.frame().await? {
                Frame::Response { id, .. } => {
                    state.prompts.remove(&id);
                }
                Frame::Permission { id, .. } => connection.reply(id, protocol::dismissed()).await?,
                Frame::Form { id, .. } => connection.reply(id, protocol::withdrawn()).await?,
                Frame::Unsupported { id, method } => connection.refuse(id, &method).await?,
                Frame::Update { .. } | Frame::Notice { .. } => {}
            }
        }
        Ok::<(), RunError>(())
    })
    .await;
    match settled {
        Ok(result) => result?,
        Err(_elapsed) => {
            tracing::info!(
                session = session.id,
                "Antigravity didn't confirm the stop; ending it"
            );
        }
    }
    Ok(Outcome::Cancelled)
}

async fn emit_all(io: &RunIo, events: Vec<EngineEvent>) {
    for event in events {
        io.emit(event).await;
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProbeError {
    #[error("start Antigravity")]
    Spawn(#[source] SpawnError),
    #[error("talk to Antigravity")]
    Open(#[source] RunError),
}

pub(crate) enum Probed {
    SignedOut,
    SignedIn(Lineup),
}

pub(crate) struct Lineup {
    pub listing: crate::catalog::Listing,
    pub hidden: usize,
}

pub(crate) async fn probe(
    mut command: tokio::process::Command,
    home: &Path,
) -> Result<Probed, ProbeError> {
    let scratch = crate::home::forge_records(home).join("antigravity-probe");
    if let Err(error) = std::fs::create_dir_all(&scratch) {
        tracing::warn!(path = %scratch.display(), %error, "couldn't create the Antigravity probe folder");
    }
    command
        .arg(SERVE)
        .args(profile_args(home))
        .stdin(std::process::Stdio::piped());
    let process = process::spawn_probe(command).map_err(ProbeError::Spawn)?;
    let mut connection = Connection::initialize(process)
        .await
        .map_err(ProbeError::Open)?;
    let opened = connection
        .call(
            "session/new",
            json!({ "cwd": scratch.display().to_string(), "mcpServers": [] }),
        )
        .await;
    let probed = match opened {
        Ok(value) => {
            let opened: Opened = answer("session/new", value).map_err(ProbeError::Open)?;
            if let Some(id) = &opened.session_id
                && let Err(error) = connection
                    .call("session/close", json!({ "sessionId": id }))
                    .await
            {
                tracing::debug!(
                    error = diagnostics::chain(&error),
                    "Antigravity kept its probe session open"
                );
            }
            Ok(Probed::SignedIn(models(&opened)))
        }
        Err(failure) if failure.signed_out() => Ok(Probed::SignedOut),
        Err(failure) => Err(ProbeError::Open(failure)),
    };
    connection.end().await;
    probed
}

pub(crate) async fn sign_in(binary: &Path, home: &Path) -> SignInState {
    let command = crate::account::engine_command(artisan_api::EngineId::Antigravity, binary, home);
    match probe(command, home).await {
        Ok(Probed::SignedIn(_)) => SignInState::SignedIn {
            account: account(home),
            plan: None,
        },
        Ok(Probed::SignedOut) => SignInState::SignedOut,
        Err(failure) => {
            let error = crate::rpc::told(&failure);
            tracing::warn!(home = %home.display(), error, "Antigravity sign-in check failed");
            SignInState::Failed {
                message: format!("Couldn't check the Antigravity sign-in: {error}"),
            }
        }
    }
}

const TOKEN_FILE: [&str; 2] = ["antigravity-cli", "antigravity-oauth-token"];

#[derive(Debug, thiserror::Error)]
enum AccountError {
    #[error("read the Antigravity token file {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse the Antigravity token file {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(serde::Deserialize)]
struct TokenAccount {
    id_token_email: Option<String>,
}

pub(crate) fn token_path(home: &Path) -> PathBuf {
    TOKEN_FILE
        .iter()
        .fold(home.join(GEMINI_DIR), |path, part| path.join(part))
}

fn account(home: &Path) -> Option<String> {
    let path = token_path(home);
    match read_account(&path) {
        Ok(account) => account,
        Err(AccountError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            None
        }
        Err(error) => {
            tracing::warn!(
                error = diagnostics::chain(&error),
                "couldn't read the Antigravity account email"
            );
            None
        }
    }
}

fn read_account(path: &Path) -> Result<Option<String>, AccountError> {
    let text = std::fs::read(path).map_err(|source| AccountError::Read {
        path: path.to_owned(),
        source,
    })?;
    let token: TokenAccount =
        serde_json::from_slice(&text).map_err(|source| AccountError::Parse {
            path: path.to_owned(),
            source,
        })?;
    Ok(token
        .id_token_email
        .filter(|email| !email.trim().is_empty()))
}

fn models(opened: &Opened) -> Lineup {
    let options = &opened.config_options;
    let thinking_option = options
        .iter()
        .find(|option| option.is(protocol::THOUGHT_LEVEL));
    let levels = thought_levels(options);
    let listed: Vec<&str> = opened
        .models
        .iter()
        .flat_map(|models| &models.available_models)
        .map(|listed| listed.model_id.as_str())
        .collect();
    let Some(model) = options.iter().find(|option| option.is(protocol::MODEL)) else {
        return Lineup {
            listing: crate::catalog::Listing::default(),
            hidden: 0,
        };
    };
    let current = model.current_value.as_deref();
    let choices = model.choices();
    let offered = choices.len();
    let mut models: Vec<artisan_api::Model> = choices
        .into_iter()
        .filter_map(|choice| {
            let shown = choice.name.clone().unwrap_or_else(|| choice.value.clone());
            let (name, efforts, default_effort) = match thinking(&choice.value, &levels) {
                Thinking::Orchestrator => return None,
                Thinking::Fixed(level) => (
                    without_level(&shown).to_owned(),
                    vec![level.to_owned()],
                    Some(level.to_owned()),
                ),
                Thinking::Adjustable => {
                    let prefix = format!("{}-", choice.value);
                    let efforts: Vec<String> = levels
                        .iter()
                        .filter(|level| {
                            listed
                                .iter()
                                .any(|id| id.strip_prefix(&prefix) == Some(level.as_str()))
                        })
                        .cloned()
                        .collect();
                    let default_effort = (current == Some(choice.value.as_str()))
                        .then(|| thinking_option.and_then(|option| option.current_value.clone()))
                        .flatten()
                        .filter(|effort| efforts.contains(effort));
                    (shown, efforts, default_effort)
                }
            };
            let id = &choice.value;
            let observed_at = artisan_api::now();
            let mut sources = vec![
                artisan_api::FieldSource {
                    field: "id".into(),
                    source: "antigravity.session/new".into(),
                    observed_at,
                },
                artisan_api::FieldSource {
                    field: "name".into(),
                    source: "antigravity.session/new".into(),
                    observed_at,
                },
                artisan_api::FieldSource {
                    field: "images".into(),
                    source: "antigravity.session/new".into(),
                    observed_at,
                },
            ];
            if !efforts.is_empty() {
                sources.push(artisan_api::FieldSource {
                    field: "efforts".into(),
                    source: "antigravity.session/new".into(),
                    observed_at,
                });
            }
            Some(artisan_api::Model {
                evidence: artisan_api::ModelEvidence { sources },
                name,
                id: choice.value.clone(),
                description: choice.description,
                efforts,
                default_effort,
                supports_images: true,
                lab: lab(id).map(str::to_owned),
                section: Some(section(id).to_owned()),
                ..artisan_api::Model::default()
            })
        })
        .collect();
    models.sort_by_key(|model| model.section.as_deref() != Some(GEMINI_SECTION));
    let hidden = offered - models.len();
    Lineup {
        listing: crate::catalog::Listing {
            models,
            default: model.current_value.clone(),
            commands: None,
        },
        hidden,
    }
}

const ORCHESTRATOR_PREFIX: &str = "gemini-pro-orchestrator-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Thinking<'a> {
    Orchestrator,
    Fixed(&'a str),
    Adjustable,
}

fn thinking<'a>(id: &str, levels: &'a [String]) -> Thinking<'a> {
    if id.starts_with(ORCHESTRATOR_PREFIX) {
        return Thinking::Orchestrator;
    }
    levels
        .iter()
        .find(|level| {
            id.strip_suffix(level.as_str())
                .is_some_and(|model| model.ends_with('-'))
        })
        .map_or(Thinking::Adjustable, |level| Thinking::Fixed(level))
}

fn thought_levels(options: &[ConfigOption]) -> Vec<String> {
    options
        .iter()
        .find(|option| option.is(protocol::THOUGHT_LEVEL))
        .map(|option| {
            option
                .choices()
                .into_iter()
                .map(|choice| choice.value)
                .collect()
        })
        .unwrap_or_default()
}

fn current_thinking<'a>(options: &[ConfigOption], levels: &'a [String]) -> Thinking<'a> {
    options
        .iter()
        .find(|option| option.is(protocol::MODEL))
        .and_then(|option| option.current_value.as_deref())
        .map_or(Thinking::Adjustable, |id| thinking(id, levels))
}

fn without_level(name: &str) -> &str {
    name.strip_suffix(')')
        .and_then(|name| name.rsplit_once(" ("))
        .map_or(name, |(bare, _)| bare.trim_end())
}

const GEMINI_SECTION: &str = "Gemini";
const OTHER_SECTION: &str = "Other";

fn lab(model: &str) -> Option<&'static str> {
    [
        ("gemini-", "Google"),
        ("claude-", "Anthropic"),
        ("gpt-", "OpenAI"),
    ]
    .into_iter()
    .find_map(|(prefix, lab)| model.starts_with(prefix).then_some(lab))
}

fn section(model: &str) -> &'static str {
    if model.starts_with("gemini-") {
        GEMINI_SECTION
    } else {
        OTHER_SECTION
    }
}

pub(crate) async fn logout(binary: &Path, home: &Path) -> Result<(), ProbeError> {
    let mut command =
        crate::account::engine_command(artisan_api::EngineId::Antigravity, binary, home);
    command
        .arg(SERVE)
        .args(profile_args(home))
        .stdin(std::process::Stdio::piped());
    let process = process::spawn_probe(command).map_err(ProbeError::Spawn)?;
    let mut connection = Connection::initialize(process)
        .await
        .map_err(ProbeError::Open)?;
    let result = connection.call("logout", json!({})).await;
    connection.end().await;
    result.map(drop).map_err(ProbeError::Open)
}
