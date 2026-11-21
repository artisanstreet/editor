use super::Reserved;
use crate::Turn;
use crate::process::{
    self, EngineProcess, ReadError, SendError, SpawnError, StderrWritten, StdinWriter,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

const IDLE_UNLOAD: Duration = Duration::from_mins(30);
const SWEEP: Duration = Duration::from_secs(60);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const UNCLAIMED_BYTES: usize = 16 * 1024 * 1024;
const RUN_BACKLOG_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("the old Codex server didn't release thread {thread} in {}s", INITIALIZE_TIMEOUT.as_secs())]
    ReleaseTimedOut { thread: String },
    #[error("the old Codex server couldn't release thread {thread}")]
    Release {
        thread: String,
        #[source]
        source: ReleaseError,
    },
    #[error("start the Codex app-server")]
    Start(#[source] StartError),
    #[error("the Codex app-server ended while run {run} attached")]
    Ended { run: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("a run is still attached to the thread")]
    StillAttached,
    #[error("Codex refused thread/unsubscribe")]
    Refused(#[source] crate::rpc::RpcError),
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("create CODEX_HOME {}", path.display())]
    Home {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("spawn the app-server")]
    Spawn(#[source] SpawnError),
    #[error("the app-server didn't answer initialize in {}s; {stderr}", INITIALIZE_TIMEOUT.as_secs())]
    InitializeTimedOut { stderr: StderrWritten },
    #[error("initialize the app-server; {stderr}")]
    Initialize {
        stderr: StderrWritten,
        #[source]
        source: InitializeError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InitializeError {
    #[error("send initialize")]
    Send(#[source] SendError),
    #[error("read the answer to initialize")]
    Read(#[source] ReadError),
    #[error("Codex refused initialize")]
    Refused(#[source] crate::rpc::RpcError),
    #[error("Codex exited before answering initialize")]
    Exited,
}

#[derive(Debug, thiserror::Error)]
#[error("the Codex app-server has ended")]
pub struct Ended;

#[derive(Default)]
struct Backlog {
    bytes: AtomicUsize,
    drained: Notify,
}

impl Backlog {
    fn full(&self) -> bool {
        self.bytes.load(Ordering::Acquire) > RUN_BACKLOG_BYTES
    }

    fn take(&self, line: &str) {
        let before = self.bytes.fetch_sub(line.len(), Ordering::AcqRel);
        if before > RUN_BACKLOG_BYTES && before - line.len() <= RUN_BACKLOG_BYTES {
            self.drained.notify_one();
        }
    }

    async fn until_drained(&self) {
        loop {
            let drained = self.drained.notified();
            tokio::pin!(drained);
            drained.as_mut().enable();
            if !self.full() {
                return;
            }
            drained.await;
        }
    }
}

static SERVERS: LazyLock<tokio::sync::Mutex<Vec<Arc<Server>>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(Vec::new()));
static STARTS: LazyLock<Mutex<HashMap<u64, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) async fn attach(turn: &Turn) -> Result<Attachment, AttachError> {
    let key = launch_key(turn);
    let window = turn.config.context_window_override();
    let holders = retire_stale(turn, key, window).await;
    if let Some(thread) = &turn.session_id {
        for holder in holders {
            release(&holder, thread).await?;
        }
    }
    if let Some(attached) = attach_current(key, window).await {
        return attached;
    }
    let gate = start_gate(key);
    let starting = gate.lock().await;
    let attached = if let Some(attached) = attach_current(key, window).await {
        attached
    } else {
        let started = Server::start(turn, key).await.map_err(AttachError::Start)?;
        let mut servers = SERVERS.lock().await;
        servers.push(Arc::clone(&started));
        let attached = started.take_run(window);
        drop(servers);
        attached
    };
    drop(starting);
    STARTS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|_, waiting| Arc::strong_count(waiting) > 1);
    attached
}

fn start_gate(key: u64) -> Arc<tokio::sync::Mutex<()>> {
    Arc::clone(
        STARTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_default(),
    )
}

async fn retire_stale(turn: &Turn, key: u64, window: Option<u64>) -> Vec<Arc<Server>> {
    let mut servers = SERVERS.lock().await;
    servers.retain(|server| !server.commands.is_closed());
    for server in servers.iter() {
        let reload = turn.session_id.as_ref().is_some_and(|thread| {
            server
                .loaded()
                .get(thread)
                .is_some_and(|loaded| *loaded != window)
        });
        if server.home == turn.home && (server.key != key || reload) {
            server.retire();
        }
    }
    servers
        .iter()
        .filter(|server| server.home == turn.home && server.retired())
        .cloned()
        .collect()
}

async fn release(holder: &Server, thread: &str) -> Result<(), AttachError> {
    let (reply, released) = oneshot::channel();
    let asked = holder.commands.send(Command::Release {
        thread: thread.to_owned(),
        reply,
    });
    if let Err(_reaped) = asked {
        return Ok(());
    }
    match tokio::time::timeout(INITIALIZE_TIMEOUT, released).await {
        Err(_elapsed) => Err(AttachError::ReleaseTimedOut {
            thread: thread.to_owned(),
        }),
        Ok(Err(_reaped)) => Ok(()),
        Ok(Ok(Err(source))) => Err(AttachError::Release {
            thread: thread.to_owned(),
            source,
        }),
        Ok(Ok(Ok(()))) => Ok(()),
    }
}

async fn attach_current(key: u64, window: Option<u64>) -> Option<Result<Attachment, AttachError>> {
    let mut servers = SERVERS.lock().await;
    servers.retain(|server| !server.commands.is_closed());
    let current = servers
        .iter()
        .find(|server| server.key == key && !server.retired())
        .cloned()?;
    let attached = current.take_run(window);
    drop(servers);
    Some(attached)
}

fn launch_key(turn: &Turn) -> u64 {
    let mut hasher = DefaultHasher::new();
    (process::containment(turn) == forge_isolation::Containment::OwnCgroup).hash(&mut hasher);
    turn.binary.hash(&mut hasher);
    turn.home.hash(&mut hasher);
    turn.launch.args.hash(&mut hasher);
    turn.launch.env.hash(&mut hasher);
    let config = super::codex_home(turn).join("config.toml");
    match std::fs::read(&config) {
        Ok(bytes) => bytes.hash(&mut hasher),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "absent".hash(&mut hasher),
        Err(error) => {
            tracing::warn!(path = %config.display(), %error, "read the Codex config for the launch key; the key counts it as unreadable");
            error.kind().to_string().hash(&mut hasher);
        }
    }
    hasher.finish()
}

struct Server {
    key: u64,
    home: PathBuf,
    commands: mpsc::UnboundedSender<Command>,
    retired: AtomicBool,
    loaded: Mutex<HashMap<String, Option<u64>>>,
    ended: Mutex<String>,
    next_run: AtomicU64,
}

impl Server {
    async fn start(turn: &Turn, key: u64) -> Result<Arc<Self>, StartError> {
        let home = super::codex_home(turn);
        std::fs::create_dir_all(&home).map_err(|source| StartError::Home { path: home, source })?;
        let mut process = process::spawn_in(
            turn,
            &[
                "app-server".to_owned(),
                "-c".to_owned(),
                "thread_unload_delay_secs=0".to_owned(),
            ],
            &turn.home,
            "codex-server",
        )
        .await
        .map_err(StartError::Spawn)?
        .with_line_limit(super::MAX_LINE_BYTES);
        match tokio::time::timeout(INITIALIZE_TIMEOUT, initialize(&mut process)).await {
            Err(_elapsed) => {
                return Err(StartError::InitializeTimedOut {
                    stderr: process.stderr(),
                });
            }
            Ok(Err(source)) => {
                return Err(StartError::Initialize {
                    stderr: process.stderr(),
                    source,
                });
            }
            Ok(Ok(())) => {}
        }
        let stdin = process
            .take_stdin()
            .map_err(|source| StartError::Initialize {
                stderr: process.stderr(),
                source: InitializeError::Send(source),
            })?;
        let (outgoing, queue) = mpsc::unbounded_channel();
        let writer = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(write(stdin, queue)));
        let (commands, receiver) = mpsc::unbounded_channel();
        let server = Arc::new(Self {
            key,
            home: turn.home.clone(),
            commands,
            retired: AtomicBool::new(false),
            loaded: Mutex::new(HashMap::new()),
            ended: Mutex::new(String::new()),
            next_run: AtomicU64::new(1),
        });
        let actor = Actor {
            server: Arc::clone(&server),
            process,
            writes: outgoing,
            writer,
            commands: receiver,
            runs: HashMap::new(),
            owners: HashMap::new(),
            agents: AgentPaths::default(),
            pending: HashMap::new(),
            releasing: HashMap::new(),
            next_id: 1,
            idle: HashMap::new(),
            unclaimed: HashMap::new(),
            unclaimed_order: VecDeque::new(),
            unclaimed_bytes: 0,
            outbox: Vec::new(),
            reading: Reading::Codex,
        };
        tokio::spawn(actor.run());
        Ok(server)
    }

    fn take_run(self: &Arc<Self>, window: Option<u64>) -> Result<Attachment, AttachError> {
        let run = self.next_run.fetch_add(1, Ordering::Relaxed);
        let (lines, receiver) = mpsc::unbounded_channel();
        let backlog = Arc::new(Backlog::default());
        self.commands
            .send(Command::Attach {
                run,
                lines,
                backlog: Arc::clone(&backlog),
                window,
            })
            .map_err(|_unsent| AttachError::Ended { run })?;
        Ok(Attachment {
            server: Arc::clone(self),
            run,
            lines: receiver,
            backlog,
        })
    }

    fn retired(&self) -> bool {
        self.retired.load(Ordering::Relaxed)
    }

    fn retire(&self) {
        if !self.retired.swap(true, Ordering::Relaxed) {
            tracing::info!("retiring a Codex app-server; it ends once its runs have");
            let _server_stopped = self.commands.send(Command::Check);
        }
    }

    fn loaded(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<u64>>> {
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug, thiserror::Error)]
enum WriteError {
    #[error("write to the Codex app-server")]
    Send(#[source] SendError),
    #[error("the Codex app-server took no input for {}s", WRITE_TIMEOUT.as_secs())]
    Stalled,
}

async fn write(
    mut stdin: StdinWriter,
    mut queue: mpsc::UnboundedReceiver<Outgoing>,
) -> Result<(), WriteError> {
    while let Some(Outgoing { message, held }) = queue.recv().await {
        tokio::time::timeout(WRITE_TIMEOUT, stdin.send(&message))
            .await
            .map_err(|_elapsed| WriteError::Stalled)?
            .map_err(WriteError::Send)?;
        drop(held);
    }
    Ok(())
}

async fn initialize(process: &mut EngineProcess) -> Result<(), InitializeError> {
    process
        .send(&json!({
            "id": 0,
            "method": "initialize",
            "params": {
                "clientInfo": { "name": "artisan", "title": "Artisan", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": {
                    "experimentalApi": true,
                    "optOutNotificationMethods": [
                        "account/rateLimits/updated",
                        "mcpServer/startupStatus/updated",
                        "remoteControl/status/changed",
                    ],
                },
            },
        }))
        .await
        .map_err(InitializeError::Send)?;
    while let Some(line) = process.next_line().await.map_err(InitializeError::Read)? {
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if frame.get("method").is_none() && frame["id"] == json!(0) {
            if let Some(error) = frame.get("error") {
                return Err(InitializeError::Refused(crate::rpc::RpcError::refusal(
                    error,
                )));
            }
            return process
                .send(&json!({ "method": "initialized" }))
                .await
                .map_err(InitializeError::Send);
        }
    }
    Err(InitializeError::Exited)
}

pub(super) struct Attachment {
    server: Arc<Server>,
    run: u64,
    lines: mpsc::UnboundedReceiver<String>,
    backlog: Arc<Backlog>,
}

impl Attachment {
    pub(super) async fn next_line(&mut self) -> Option<String> {
        let line = self.lines.recv().await?;
        self.backlog.take(&line);
        Some(line)
    }

    pub(super) fn send(&self, message: Value) -> Result<(), Ended> {
        self.send_holding(message, Vec::new())
    }

    pub(super) fn send_holding(&self, message: Value, held: Vec<Reserved>) -> Result<(), Ended> {
        self.server
            .commands
            .send(Command::Send {
                run: self.run,
                outgoing: Outgoing { message, held },
            })
            .map_err(|_unsent| Ended)
    }

    pub(super) fn ended(&self) -> String {
        self.server
            .ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn retire(&self) {
        self.server.retire();
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        let _server_stopped = self.server.commands.send(Command::Detach { run: self.run });
    }
}

enum Command {
    Attach {
        run: u64,
        lines: mpsc::UnboundedSender<String>,
        backlog: Arc<Backlog>,
        window: Option<u64>,
    },
    Send {
        run: u64,
        outgoing: Outgoing,
    },
    Detach {
        run: u64,
    },
    Release {
        thread: String,
        reply: oneshot::Sender<Result<(), ReleaseError>>,
    },
    Check,
}

struct Outgoing {
    message: Value,
    held: Vec<Reserved>,
}

impl From<Value> for Outgoing {
    fn from(message: Value) -> Self {
        Self {
            message,
            held: Vec::new(),
        }
    }
}

struct Release {
    thread: String,
    reply: oneshot::Sender<Result<(), ReleaseError>>,
}

struct AttachedRun {
    lines: mpsc::UnboundedSender<String>,
    backlog: Arc<Backlog>,
    window: Option<u64>,
}

const SERVER_RUN: u64 = 0;

struct AgentPath {
    root: String,
    path: String,
}

#[derive(Default)]
struct AgentPaths(HashMap<String, AgentPath>);

impl AgentPaths {
    fn parent_path<'a>(&'a self, sender: &str, sender_is_root: bool) -> Option<&'a str> {
        self.0
            .get(sender)
            .map(|agent| agent.path.as_str())
            .or_else(|| sender_is_root.then_some("/root"))
    }

    fn adopt(&mut self, sender: &str, child: &str, path: &str) {
        let root = self
            .0
            .get(sender)
            .map_or_else(|| sender.to_owned(), |agent| agent.root.clone());
        self.0.insert(
            child.to_owned(),
            AgentPath {
                root,
                path: path.to_owned(),
            },
        );
    }

    fn forget(&mut self, thread: &str) {
        self.0
            .retain(|child, agent| child != thread && agent.root != thread);
    }
}

struct Actor {
    server: Arc<Server>,
    process: EngineProcess,
    writes: mpsc::UnboundedSender<Outgoing>,
    writer: tokio_util::task::AbortOnDropHandle<Result<(), WriteError>>,
    commands: mpsc::UnboundedReceiver<Command>,
    runs: HashMap<u64, AttachedRun>,
    owners: HashMap<String, u64>,
    agents: AgentPaths,
    pending: HashMap<u64, (u64, Value)>,
    releasing: HashMap<u64, Release>,
    next_id: u64,
    idle: HashMap<String, Instant>,
    unclaimed: HashMap<String, Vec<String>>,
    unclaimed_order: VecDeque<String>,
    unclaimed_bytes: usize,
    outbox: Vec<Value>,
    reading: Reading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    Codex,
    PausedFor(u64),
}

#[derive(Deserialize)]
struct Head {
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Params,
    result: Option<ResultHead>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Params {
    thread_id: Option<String>,
    thread: Option<ThreadHead>,
    item: Option<ItemHead>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadHead {
    id: String,
    parent_thread_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemHead {
    #[serde(default)]
    receiver_thread_ids: Vec<String>,
    agent_thread_id: Option<String>,
    agent_path: Option<String>,
    kind: Option<super::protocol::SubAgentActivityKind>,
}

#[derive(Deserialize)]
struct ResultHead {
    thread: Option<ThreadHead>,
}

impl Actor {
    async fn run(mut self) {
        let mut sweep = tokio::time::interval(SWEEP);
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let ended = loop {
            let behind = self.behind();
            self.pace(behind.as_ref().map(|(run, _)| *run));
            let behind = behind.map(|(_, backlog)| backlog);
            tokio::select! {
                () = until_drained(behind.as_deref()), if behind.is_some() => {}
                line = self.process.next_line(), if behind.is_none() => match line {
                    Ok(Some(line)) => {
                        self.route(line);
                        self.flush();
                    }
                    Ok(None) => break "Codex app-server exited".to_owned(),
                    Err(error) => break format!("reading the Codex app-server: {}", diagnostics::chain(&error)),
                },
                written = &mut self.writer => break match written {
                    Ok(Ok(())) => "the Codex app-server writer stopped".to_owned(),
                    Ok(Err(error)) => diagnostics::chain(&error),
                    Err(error) => format!("the Codex app-server writer ended: {}", diagnostics::chain(&error)),
                },
                command = self.commands.recv() => {
                    let Some(command) = command else { break "every handle is gone".to_owned() };
                    self.command(command);
                }
                _ = sweep.tick() => self.unload_idle(),
            }
            if self.runs.is_empty() && (self.server.retired() || self.idle.is_empty()) {
                let mut servers = SERVERS.lock().await;
                if !self.commands.is_empty() {
                    continue;
                }
                let why = if self.server.retired() {
                    "retired with no runs left"
                } else {
                    "no runs and no loaded threads"
                };
                self.end(why).await;
                servers.retain(|server| !Arc::ptr_eq(server, &self.server));
                return;
            }
        };
        self.end(&ended).await;
    }

    fn behind(&self) -> Option<(u64, Arc<Backlog>)> {
        self.runs
            .iter()
            .find(|(_, attached)| attached.backlog.full())
            .map(|(run, attached)| (*run, Arc::clone(&attached.backlog)))
    }

    fn pace(&mut self, behind: Option<u64>) {
        let reading = behind.map_or(Reading::Codex, Reading::PausedFor);
        if reading == self.reading {
            return;
        }
        match reading {
            Reading::PausedFor(run) => tracing::debug!(
                run,
                "Forge stopped reading Codex until run {run} catches up on its unread output"
            ),
            Reading::Codex => tracing::debug!("Forge reads Codex again"),
        }
        self.reading = reading;
    }

    fn write(&self, outgoing: impl Into<Outgoing>) {
        if self.writes.send(outgoing.into()).is_err() {
            tracing::debug!("the Codex app-server writer has ended; its failure ends the server");
        }
    }

    async fn end(&mut self, why: &str) {
        let stderr = self.process.stderr();
        let ended = format!("{why}; {stderr}");
        tracing::info!(why, stderr_bytes = stderr.bytes(), "Codex app-server ended");
        *self
            .server
            .ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = ended;
        self.server.retired.store(true, Ordering::Relaxed);
        self.process.note(&format!("ending: {why}"));
        self.process.kill().await;
        self.runs.clear();
        self.commands.close();
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Attach {
                run,
                lines,
                backlog,
                window,
            } => {
                self.runs.insert(
                    run,
                    AttachedRun {
                        lines,
                        backlog,
                        window,
                    },
                );
            }
            Command::Send { run, mut outgoing } => {
                if outgoing.message.get("method").is_some()
                    && let Some(id) = outgoing.message.get_mut("id")
                {
                    let ours = self.next_id;
                    self.next_id += 1;
                    let theirs = std::mem::replace(id, json!(ours));
                    self.pending.insert(ours, (run, theirs));
                }
                self.write(outgoing);
            }
            Command::Detach { run } => {
                self.runs.remove(&run);
                let now = Instant::now();
                let loaded = self.server.loaded();
                self.owners.retain(|thread, owner| {
                    if *owner != run {
                        return true;
                    }
                    if loaded.contains_key(thread) {
                        self.idle.insert(thread.clone(), now);
                    }
                    false
                });
            }
            Command::Release { thread, reply } => {
                if self.owners.contains_key(&thread) {
                    let _requester_gone = reply.send(Err(ReleaseError::StillAttached));
                } else if !self.server.loaded().contains_key(&thread) {
                    let _requester_gone = reply.send(Ok(()));
                } else {
                    let id = self.next_id;
                    self.releasing.insert(
                        id,
                        Release {
                            thread: thread.clone(),
                            reply,
                        },
                    );
                    self.unsubscribe(&thread);
                }
            }
            Command::Check => {}
        }
    }

    fn unload_idle(&mut self) {
        let now = Instant::now();
        let stale: Vec<String> = self
            .idle
            .iter()
            .filter(|(_, since)| now.duration_since(**since) >= IDLE_UNLOAD)
            .map(|(thread, _)| thread.clone())
            .collect();
        for thread in stale {
            self.idle.remove(&thread);
            self.unsubscribe(&thread);
        }
    }

    fn unsubscribe(&mut self, thread: &str) {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, (SERVER_RUN, json!(id)));
        self.write(json!({ "id": id, "method": "thread/unsubscribe",
            "params": { "threadId": thread } }));
    }

    fn closed(&mut self, thread: &str) {
        self.server.loaded().remove(thread);
        self.idle.remove(thread);
        self.agents.forget(thread);
        let ids: Vec<_> = self
            .releasing
            .iter()
            .filter(|(_, release)| release.thread == thread)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(release) = self.releasing.remove(&id) {
                let _requester_gone = release.reply.send(Ok(()));
            }
        }
    }

    fn route(&mut self, line: String) {
        let head = match serde_json::from_str::<Head>(&line) {
            Ok(head) => head,
            Err(error) => {
                tracing::warn!(%error, "Codex app-server wrote a frame Artisan can't read");
                self.deliver_all(&line);
                return;
            }
        };
        match head.method {
            None => self.response(head.id.as_ref(), head.result, &line),
            Some(method) => self.frame(&method, head.id, head.params, line),
        }
    }

    fn flush(&mut self) {
        for message in std::mem::take(&mut self.outbox) {
            self.write(message);
        }
    }

    fn response(&mut self, id: Option<&Value>, result: Option<ResultHead>, line: &str) {
        let Some((run, theirs)) = id
            .and_then(Value::as_u64)
            .and_then(|id| self.pending.remove(&id))
        else {
            tracing::debug!(?id, "Codex answered a request nobody is waiting for");
            return;
        };
        if run == SERVER_RUN {
            if let Some(id) = id.and_then(Value::as_u64) {
                let frame: Value = serde_json::from_str(line)
                    .expect("invariant: a Codex frame that parsed as a head parses as JSON");
                if frame["result"]["status"] == "notLoaded" {
                    if let Some(release) = self.releasing.get(&id) {
                        let thread = release.thread.clone();
                        self.closed(&thread);
                    }
                } else if let Some(error) = frame.get("error")
                    && let Some(release) = self.releasing.remove(&id)
                {
                    let _requester_gone = release.reply.send(Err(ReleaseError::Refused(
                        crate::rpc::RpcError::refusal(error),
                    )));
                }
            }
            return;
        }
        if let Some(thread) = result.and_then(|result| result.thread) {
            let window = self.runs.get(&run).and_then(|attached| attached.window);
            self.server.loaded().insert(thread.id.clone(), window);
            if self.runs.contains_key(&run) {
                self.idle.remove(&thread.id);
                self.claim(&thread.id, run);
            } else {
                self.idle.insert(thread.id, Instant::now());
            }
        }
        if !self.runs.contains_key(&run) {
            return;
        }
        let mut frame = match serde_json::from_str::<Value>(line) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(run, %error, "dropped a Codex answer that isn't JSON");
                return;
            }
        };
        frame["id"] = theirs;
        self.deliver(run, frame.to_string());
    }

    fn frame(&mut self, method: &str, id: Option<Value>, params: Params, line: String) {
        let thread = params
            .thread_id
            .clone()
            .or_else(|| params.thread.as_ref().map(|thread| thread.id.clone()));
        let Some(thread) = thread else {
            self.unthreaded(method, id, line);
            return;
        };
        if method == "thread/closed" {
            self.closed(&thread);
        }
        if let Some(parent) = params
            .thread
            .as_ref()
            .and_then(|thread| thread.parent_thread_id.as_ref())
            && let Some(&run) = self.owners.get(parent)
            && !self.owners.contains_key(&thread)
        {
            self.claim(&thread, run);
        }
        let Some(&run) = self.owners.get(&thread) else {
            if self.server.loaded().contains_key(&thread) {
                tracing::debug!(method, thread, "dropping a frame of an idle Codex thread");
            } else {
                self.hold(thread, line);
            }
            return;
        };
        self.deliver(run, line);
        if let Some(item) = params.item {
            if let Some(child) = self.native_child(&thread, &item)
                && !self.owners.contains_key(&child)
            {
                self.claim(&child, run);
            }
            for child in item.receiver_thread_ids {
                if !child.trim().is_empty() && !self.owners.contains_key(&child) {
                    self.claim(&child, run);
                }
            }
        }
    }

    fn native_child(&mut self, sender: &str, item: &ItemHead) -> Option<String> {
        use super::protocol::SubAgentActivityKind;
        let (child, path) = (item.agent_thread_id.as_ref()?, item.agent_path.as_ref()?);
        if child.trim().is_empty() || child == sender {
            return None;
        }
        let sender_is_root = self.server.loaded().contains_key(sender);
        let parent_path = self.agents.parent_path(sender, sender_is_root);
        let owns = match item.kind {
            Some(SubAgentActivityKind::Started) => true,
            Some(SubAgentActivityKind::Interacted) => parent_path.is_some_and(|parent| {
                path.strip_prefix(parent)
                    .is_some_and(|suffix| suffix.starts_with('/'))
            }),
            Some(
                SubAgentActivityKind::Interrupted
                | SubAgentActivityKind::Completed
                | SubAgentActivityKind::Unknown,
            )
            | None => false,
        };
        if !owns {
            return None;
        }
        self.agents.adopt(sender, child, path);
        Some(child.clone())
    }

    fn unthreaded(&mut self, method: &str, id: Option<Value>, line: String) {
        let Some(id) = id else {
            self.deliver_all(&line);
            return;
        };
        if let Some(run) = self.runs.keys().min().copied() {
            self.deliver(run, line);
        } else {
            tracing::debug!(
                method,
                "declining a Codex request made with no run attached"
            );
            let message = format!("no Artisan run is attached to answer {method}");
            self.outbox.push(json!({ "id": id, "error": { "code": crate::rpc::METHOD_NOT_FOUND, "message": message } }));
        }
    }

    fn claim(&mut self, thread: &str, run: u64) {
        self.owners.insert(thread.to_owned(), run);
        if let Some(frames) = self.unclaimed.remove(thread) {
            self.unclaimed_order.retain(|held| held != thread);
            for line in frames {
                self.unclaimed_bytes = self.unclaimed_bytes.saturating_sub(line.len());
                self.deliver(run, line);
            }
        }
    }

    fn hold(&mut self, thread: String, line: String) {
        self.unclaimed_bytes += line.len();
        if !self.unclaimed.contains_key(&thread) {
            self.unclaimed_order.push_back(thread.clone());
        }
        self.unclaimed.entry(thread).or_default().push(line);
        while self.unclaimed_bytes > UNCLAIMED_BYTES
            && let Some(oldest) = self.unclaimed_order.pop_front()
        {
            let frames = self.unclaimed.remove(&oldest).unwrap_or_default();
            let bytes: usize = frames.iter().map(String::len).sum();
            self.unclaimed_bytes = self.unclaimed_bytes.saturating_sub(bytes);
            tracing::warn!(
                thread = oldest,
                frames = frames.len(),
                bytes,
                "dropping frames of a Codex thread no run claimed"
            );
        }
    }

    fn deliver_all(&self, line: &str) {
        let runs: Vec<u64> = self.runs.keys().copied().collect();
        for run in runs {
            self.deliver(run, line.to_owned());
        }
    }

    fn deliver(&self, run: u64, line: String) {
        let Some(attached) = self.runs.get(&run) else {
            return;
        };
        let bytes = line.len();
        attached.backlog.bytes.fetch_add(bytes, Ordering::AcqRel);
        if attached.lines.send(line).is_err() {
            attached.backlog.bytes.fetch_sub(bytes, Ordering::AcqRel);
        }
    }
}

async fn until_drained(behind: Option<&Backlog>) {
    if let Some(backlog) = behind {
        backlog.until_drained().await;
    }
}

#[cfg(test)]
#[path = "../../tests/codex/server/agent_paths.rs"]
mod agent_paths_tests;
