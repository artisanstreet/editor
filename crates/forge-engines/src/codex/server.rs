//! One `codex app-server` shared by every Codex thread.
//!
//! Codex keeps per-thread state in the process. The visible one: a thread
//! whose Responses WebSocket keeps getting closed falls back to HTTPS and
//! stays there. A process per turn forgot that and sat through five
//! reconnects on every turn, so runs attach to one long-lived server instead,
//! the way Codex's own desktop daemon serves its threads.
//!
//! The server's task owns the process. Each run keeps its own request ids
//! (rewritten on the way through) and gets the frames of its threads: the
//! root thread `thread/start` or `thread/resume` answered with, and every
//! child thread its agents spawn (`parentThreadId`, `receiverThreadIds`).
//! Frames for a thread no run has claimed wait for their owner. A thread no
//! run has used for [`IDLE_UNLOAD`] is unsubscribed; the process ends once it
//! has no runs and no loaded threads, or once it is retired and its runs are
//! over.
//!
//! A server serves one launch: binary, home, arguments, environment and the
//! managed `config.toml`. When any of them changes, the next run starts a new
//! server and the old one retires. So does a thread whose context window
//! changed, since Codex reads that only when it loads a thread. Before a
//! replacement resumes a thread, its previous server must acknowledge that
//! the thread is closed and its persistence writer has been released.

use crate::Turn;
use crate::process::{self, EngineProcess};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

/// How long a thread stays loaded after its last run.
const IDLE_UNLOAD: Duration = Duration::from_mins(30);
const SWEEP: Duration = Duration::from_secs(60);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
/// Frames held for threads no run has claimed yet.
const UNCLAIMED_BYTES: usize = 16 * 1024 * 1024;

static SERVERS: LazyLock<tokio::sync::Mutex<Vec<Arc<Server>>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(Vec::new()));

/// Attaches a run to the server for `turn`'s launch, starting one if needed.
#[expect(
    clippy::significant_drop_tightening,
    reason = "the pool stays locked until the run is attached; see below"
)]
pub(super) async fn attach(turn: &Turn) -> Result<Attachment> {
    let key = launch_key(turn);
    let window = turn.config.context_window_override();
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
    // A retired process may still serve other runs and retain this thread's
    // writer lock. Wait for its explicit close before loading it elsewhere.
    if let Some(thread) = &turn.session_id {
        for server in servers
            .iter()
            .filter(|server| server.home == turn.home && server.retired())
        {
            let (reply, released) = oneshot::channel();
            server
                .commands
                .send(Command::Release {
                    thread: thread.clone(),
                    reply,
                })
                .map_err(|_unsent| anyhow!("the old Codex server ended during thread handoff"))?;
            tokio::time::timeout(INITIALIZE_TIMEOUT, released)
                .await
                .context("waiting for the old Codex server to release the thread timed out")?
                .context("the old Codex server ended before releasing the thread")??;
        }
    }
    let current = servers
        .iter()
        .find(|server| server.key == key && !server.retired())
        .cloned();
    let server = if let Some(server) = current {
        server
    } else {
        let server = Server::start(turn, key).await?;
        servers.push(Arc::clone(&server));
        server
    };
    let run = server.next_run.fetch_add(1, Ordering::Relaxed);
    let (lines, receiver) = mpsc::unbounded_channel();
    // Under the pool lock, so an idle server can't end between being picked
    // and taking the run (it removes itself under the same lock).
    server
        .commands
        .send(Command::Attach { run, lines, window })
        .map_err(|_unsent| anyhow!("the Codex app-server ended while a run attached"))?;
    Ok(Attachment {
        server,
        run,
        lines: receiver,
    })
}

/// What a server is keyed by: everything that shapes the process.
fn launch_key(turn: &Turn) -> u64 {
    let mut hasher = DefaultHasher::new();
    turn.binary.hash(&mut hasher);
    turn.home.hash(&mut hasher);
    turn.launch.args.hash(&mut hasher);
    turn.launch.env.hash(&mut hasher);
    // Codex reads its config (MCP servers) when the process starts.
    std::fs::read(super::codex_home(turn).join("config.toml"))
        .ok()
        .hash(&mut hasher);
    hasher.finish()
}

struct Server {
    key: u64,
    home: PathBuf,
    commands: mpsc::UnboundedSender<Command>,
    /// Takes no new runs; ends once its runs have.
    retired: AtomicBool,
    /// Loaded root threads and the context window each was loaded with.
    loaded: Mutex<HashMap<String, Option<u64>>>,
    /// Why the process ended, with the tail of its stderr.
    ended: Mutex<String>,
    next_run: AtomicU64,
}

impl Server {
    async fn start(turn: &Turn, key: u64) -> Result<Arc<Self>> {
        std::fs::create_dir_all(super::codex_home(turn))
            .context("creating CODEX_HOME for the Codex app-server")?;
        // Artisan already retains idle threads for IDLE_UNLOAD. An explicit
        // unsubscribe must release persistence immediately for server handoffs,
        // rather than starting Codex's separate idle retention timer.
        let mut process = process::spawn_in(
            turn,
            &[
                "app-server".to_owned(),
                "-c".to_owned(),
                "thread_unload_delay_secs=0".to_owned(),
            ],
            &turn.home,
            "codex-server",
        )?;
        tokio::time::timeout(INITIALIZE_TIMEOUT, initialize(&mut process))
            .await
            .context("Codex app-server did not answer initialize")?
            .with_context(|| {
                let tail = process.stderr_tail();
                format!("initializing the Codex app-server: {tail}")
            })?;
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
            commands: receiver,
            runs: HashMap::new(),
            owners: HashMap::new(),
            pending: HashMap::new(),
            releasing: HashMap::new(),
            next_id: 1,
            idle: HashMap::new(),
            unclaimed: HashMap::new(),
            unclaimed_order: VecDeque::new(),
            unclaimed_bytes: 0,
            outbox: Vec::new(),
        };
        tokio::spawn(actor.run());
        Ok(server)
    }

    fn retired(&self) -> bool {
        self.retired.load(Ordering::Relaxed)
    }

    fn retire(&self) {
        if !self.retired.swap(true, Ordering::Relaxed) {
            tracing::info!("retiring a Codex app-server; it ends once its runs have");
            // An ended server has nothing left to check.
            self.commands.send(Command::Check).ok();
        }
    }

    fn loaded(&self) -> std::sync::MutexGuard<'_, HashMap<String, Option<u64>>> {
        self.loaded.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

async fn initialize(process: &mut EngineProcess) -> Result<()> {
    process
        .send(&json!({
            "id": 0,
            "method": "initialize",
            "params": {
                "clientInfo": { "name": "artisan", "title": "Artisan", "version": env!("CARGO_PKG_VERSION") },
                // Experimental: `item/tool/requestUserInput`, `turn/steer` fields.
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
        .await?;
    while let Some(line) = process.next_line().await? {
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if frame.get("method").is_none() && frame["id"] == json!(0) {
            if let Some(error) = frame.get("error") {
                bail!("{error}");
            }
            return process.send(&json!({ "method": "initialized" })).await;
        }
    }
    bail!("Codex exited before answering initialize")
}

/// One run's connection to its server. Dropping it detaches the run.
pub(super) struct Attachment {
    server: Arc<Server>,
    run: u64,
    lines: mpsc::UnboundedReceiver<String>,
}

impl Attachment {
    /// The next line for this run, or `None` once the process has ended.
    pub async fn next_line(&mut self) -> Option<String> {
        self.lines.recv().await
    }

    pub fn send(&self, message: Value) -> Result<()> {
        self.server
            .commands
            .send(Command::Send {
                run: self.run,
                message,
            })
            .map_err(|_unsent| anyhow!("the Codex app-server has ended"))
    }

    /// Why the process ended, with the tail of its stderr.
    pub fn ended(&self) -> String {
        self.server
            .ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Sends later runs to a fresh process, for one that stopped answering.
    pub fn retire(&self) {
        self.server.retire();
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        // An ended server has no runs left to detach.
        self.server
            .commands
            .send(Command::Detach { run: self.run })
            .ok();
    }
}

enum Command {
    Attach {
        run: u64,
        lines: mpsc::UnboundedSender<String>,
        window: Option<u64>,
    },
    Send {
        run: u64,
        message: Value,
    },
    Detach {
        run: u64,
    },
    Release {
        thread: String,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Look again at whether the server should end.
    Check,
}

struct Release {
    thread: String,
    reply: oneshot::Sender<Result<()>>,
}

struct AttachedRun {
    lines: mpsc::UnboundedSender<String>,
    window: Option<u64>,
}

/// The server's own requests (`thread/unsubscribe`) belong to run 0.
const SERVER_RUN: u64 = 0;

struct Actor {
    server: Arc<Server>,
    process: EngineProcess,
    commands: mpsc::UnboundedReceiver<Command>,
    runs: HashMap<u64, AttachedRun>,
    /// Thread → the run its frames go to.
    owners: HashMap<String, u64>,
    /// Requests in flight: the id Codex sees → the run and the id it used.
    pending: HashMap<u64, (u64, Value)>,
    releasing: HashMap<u64, Release>,
    next_id: u64,
    /// Loaded root threads no run is using, since when.
    idle: HashMap<String, Instant>,
    /// Frames of threads no run has claimed yet, oldest thread first.
    unclaimed: HashMap<String, Vec<String>>,
    unclaimed_order: VecDeque<String>,
    unclaimed_bytes: usize,
    /// Answers routing gives on the server's behalf, for [`Actor::flush`].
    outbox: Vec<Value>,
}

/// The parts of a frame routing needs.
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
            tokio::select! {
                line = self.process.next_line() => match line {
                    Ok(Some(line)) => {
                        self.route(line);
                        if let Err(error) = self.flush().await {
                            break format!("writing to the Codex app-server: {error:#}");
                        }
                    }
                    Ok(None) => break "Codex app-server exited".to_owned(),
                    Err(error) => break format!("reading the Codex app-server: {error:#}"),
                },
                command = self.commands.recv() => {
                    let Some(command) = command else { break "every handle is gone".to_owned() };
                    if let Err(error) = self.command(command).await {
                        break format!("writing to the Codex app-server: {error:#}");
                    }
                }
                _ = sweep.tick() => {
                    if let Err(error) = self.unload_idle().await {
                        break format!("writing to the Codex app-server: {error:#}");
                    }
                }
            }
            if self.runs.is_empty() && (self.server.retired() || self.idle.is_empty()) {
                // Under the pool lock, so no run attaches meanwhile.
                // An attaching run may hold the pool while waiting for us to
                // release a thread. Keep servicing commands in that case.
                let Ok(mut servers) = SERVERS.try_lock() else {
                    continue;
                };
                if !self.commands.is_empty() {
                    continue;
                }
                let why = if self.server.retired() {
                    "retired with no runs left"
                } else {
                    "no runs and no loaded threads"
                };
                // Keep the pool locked until the process is reaped: removing
                // it first lets a replacement race its remaining writer locks.
                self.end(why).await;
                servers.retain(|server| !Arc::ptr_eq(server, &self.server));
                return;
            }
        };
        self.end(&ended).await;
    }

    async fn end(&mut self, why: &str) {
        let tail = self.process.stderr_tail();
        let ended = if tail.is_empty() {
            why.to_owned()
        } else {
            format!("{why}: {tail}")
        };
        tracing::info!(why, "Codex app-server ended");
        *self
            .server
            .ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = ended;
        self.server.retired.store(true, Ordering::Relaxed);
        self.process.note(&format!("ending: {why}"));
        self.process.kill().await;
        // Dropping the senders ends every attached run's stream.
        self.runs.clear();
        self.commands.close();
    }

    async fn command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Attach { run, lines, window } => {
                self.runs.insert(run, AttachedRun { lines, window });
            }
            Command::Send { run, mut message } => {
                if message.get("method").is_some()
                    && let Some(id) = message.get_mut("id")
                {
                    let ours = self.next_id;
                    self.next_id += 1;
                    let theirs = std::mem::replace(id, json!(ours));
                    self.pending.insert(ours, (run, theirs));
                }
                self.process.send(&message).await?;
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
                    reply
                        .send(Err(anyhow!("the Codex thread still has an attached run")))
                        .ok();
                } else if !self.server.loaded().contains_key(&thread) {
                    reply.send(Ok(())).ok();
                } else {
                    let id = self.next_id;
                    self.releasing.insert(
                        id,
                        Release {
                            thread: thread.clone(),
                            reply,
                        },
                    );
                    self.unsubscribe(&thread).await?;
                }
            }
            Command::Check => {}
        }
        Ok(())
    }

    /// Unsubscribes threads no run has used for [`IDLE_UNLOAD`].
    async fn unload_idle(&mut self) -> Result<()> {
        let now = Instant::now();
        let stale: Vec<String> = self
            .idle
            .iter()
            .filter(|(_, since)| now.duration_since(**since) >= IDLE_UNLOAD)
            .map(|(thread, _)| thread.clone())
            .collect();
        for thread in stale {
            self.idle.remove(&thread);
            self.unsubscribe(&thread).await?;
        }
        Ok(())
    }

    async fn unsubscribe(&mut self, thread: &str) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, (SERVER_RUN, json!(id)));
        self.process
            .send(&json!({ "id": id, "method": "thread/unsubscribe",
            "params": { "threadId": thread } }))
            .await
    }

    fn closed(&mut self, thread: &str) {
        self.server.loaded().remove(thread);
        self.idle.remove(thread);
        let ids: Vec<_> = self
            .releasing
            .iter()
            .filter(|(_, release)| release.thread == thread)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(release) = self.releasing.remove(&id) {
                release.reply.send(Ok(())).ok();
            }
        }
    }

    fn route(&mut self, line: String) {
        let head = match serde_json::from_str::<Head>(&line) {
            Ok(head) => head,
            Err(error) => {
                // Every run's decoder fails on it, as it would have alone.
                tracing::warn!(%error, "Codex app-server wrote a frame Artisan can't read");
                for run in self.runs.values() {
                    // A run that stopped reading is detaching.
                    run.lines.send(line.clone()).ok();
                }
                return;
            }
        };
        match head.method {
            None => self.response(head.id.as_ref(), head.result, &line),
            Some(method) => self.frame(&method, head.id, head.params, line),
        }
    }

    /// Writes what routing answered on the server's behalf.
    async fn flush(&mut self) -> Result<()> {
        for message in std::mem::take(&mut self.outbox) {
            self.process.send(&message).await?;
        }
        Ok(())
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
                let frame: Value = serde_json::from_str(line).unwrap_or_default();
                if frame["result"]["status"] == "notLoaded" {
                    if let Some(release) = self.releasing.get(&id) {
                        let thread = release.thread.clone();
                        self.closed(&thread);
                    }
                } else if frame.get("error").is_some()
                    && let Some(release) = self.releasing.remove(&id)
                {
                    release
                        .reply
                        .send(Err(anyhow!("Codex could not release the thread: {line}")))
                        .ok();
                }
            }
            return;
        }
        // `thread/start` and `thread/resume` answer with the run's root thread.
        if let Some(thread) = result.and_then(|result| result.thread) {
            let window = self.runs.get(&run).and_then(|attached| attached.window);
            self.server.loaded().insert(thread.id.clone(), window);
            if self.runs.contains_key(&run) {
                self.idle.remove(&thread.id);
                self.claim(&thread.id, run);
            } else {
                // Loaded for a run that has already gone.
                self.idle.insert(thread.id, Instant::now());
            }
        }
        if !self.runs.contains_key(&run) {
            return;
        }
        let Ok(mut frame) = serde_json::from_str::<Value>(line) else {
            return;
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
        // A child thread belongs to the run of the thread that spawned it.
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
                // An idle thread winding down; nobody is listening.
                tracing::debug!(method, thread, "dropping a frame of an idle Codex thread");
            } else {
                self.hold(thread, line);
            }
            return;
        };
        for child in params
            .item
            .map(|item| item.receiver_thread_ids)
            .unwrap_or_default()
        {
            if !child.trim().is_empty() && !self.owners.contains_key(&child) {
                self.claim(&child, run);
            }
        }
        self.deliver(run, line);
    }

    /// A frame about no thread: notifications go to every run; a request
    /// gets one answer, from the oldest run.
    fn unthreaded(&mut self, method: &str, id: Option<Value>, line: String) {
        let Some(id) = id else {
            for run in self.runs.values() {
                // A run that stopped reading is detaching.
                run.lines.send(line.clone()).ok();
            }
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

    fn deliver(&self, run: u64, line: String) {
        if let Some(attached) = self.runs.get(&run) {
            // A run that stopped reading is detaching.
            attached.lines.send(line).ok();
        }
    }
}
