use crate::Turn;
use crate::sandbox::Sandbox;
use crate::trace::{Kind, Secrets, Trace};
use artisan_api::{EngineId, ThreadId};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

pub(crate) const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;
const PROBE_STDERR_BYTES: usize = 8 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("restore {engine}'s configuration before starting thread {thread}")]
    Guard {
        engine: EngineId,
        thread: ThreadId,
        #[source]
        source: crate::sandbox::GuardError,
    },
    #[error("sandbox {engine} thread {thread}")]
    Sandbox {
        engine: EngineId,
        thread: ThreadId,
        #[source]
        source: Box<crate::sandbox::SandboxError>,
    },
    #[error("make the process tree of {}", program.display())]
    Tree {
        program: PathBuf,
        #[source]
        source: forge_isolation::RunGroupError,
    },
    #[error("start {}", program.display())]
    Start {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum Exit {
    #[error("exited with code {0}")]
    Code(i32),
    #[error("was ended by a signal")]
    Signal,
    #[error("couldn't be waited for")]
    Unreaped(#[source] std::io::Error),
    #[error("was stopped by Forge after its output closed")]
    Killed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StderrWritten {
    bytes: u64,
    trace: Option<Arc<str>>,
}

impl StderrWritten {
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl std::fmt::Display for StderrWritten {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.bytes, &self.trace) {
            (0, _) => write!(f, "nothing on stderr"),
            (bytes, Some(trace)) => write!(
                f,
                "{bytes} bytes on stderr, kept in the engine trace {trace}"
            ),
            (bytes, None) => write!(f, "{bytes} bytes on stderr"),
        }
    }
}

#[derive(Debug)]
enum StderrUse {
    Traced,
    Captured(Arc<Mutex<Vec<u8>>>),
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("stdin of {} is closed", program.display())]
    StdinClosed { program: PathBuf },
    #[error("encode a message for {}", program.display())]
    Encode {
        program: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("write to {}", program.display())]
    Write {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("read the output of {}", program.display())]
    Read {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} wrote a line over {limit} bytes", program.display())]
    Oversized { program: PathBuf, limit: usize },
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("start the probe")]
    Spawn(#[source] SpawnError),
    #[error("read the output of probe {}", program.display())]
    Read {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("probe {} wrote over {limit} bytes", program.display())]
    Oversized { program: PathBuf, limit: usize },
    #[error("wait for probe {}", program.display())]
    Wait {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("probe {} gave no answer in {}s", program.display(), after.as_secs())]
    TimedOut {
        program: PathBuf,
        after: std::time::Duration,
    },
}

#[derive(Debug)]
pub(crate) struct StdinWriter {
    program: PathBuf,
    stdin: ChildStdin,
    trace: Trace,
}

impl StdinWriter {
    pub(crate) async fn send(&mut self, value: &serde_json::Value) -> Result<(), SendError> {
        let mut bytes = serde_json::to_vec(value).map_err(|source| SendError::Encode {
            program: self.program.clone(),
            source,
        })?;
        self.trace
            .record(Kind::In, &String::from_utf8_lossy(&bytes));
        bytes.push(b'\n');
        let written = match self.stdin.write_all(&bytes).await {
            Ok(()) => self.stdin.flush().await,
            Err(error) => Err(error),
        };
        written.map_err(|source| SendError::Write {
            program: self.program.clone(),
            source,
        })
    }
}

#[derive(Debug)]
pub struct EngineProcess {
    program: PathBuf,
    child: OwnedChild,
    stdin: Option<StdinWriter>,
    stdout: BufReader<ChildStdout>,
    stderr_bytes: Arc<AtomicU64>,
    line: Vec<u8>,
    oversized: bool,
    trace: Trace,
    stderr_task: Option<tokio_util::task::AbortOnDropHandle<()>>,
}

pub(crate) fn spawn(turn: &Turn, args: &[String]) -> Result<EngineProcess, SpawnError> {
    spawn_in(
        turn,
        args,
        &turn.cwd,
        &format!("{}-{}", turn.engine, turn.thread_id),
    )
}

pub(crate) fn spawn_in(
    turn: &Turn,
    args: &[String],
    cwd: &std::path::Path,
    name: &str,
) -> Result<EngineProcess, SpawnError> {
    let gateway = match turn.engine {
        EngineId::Claude if !turn.launch.gateway_mcp.is_empty() => Some(
            crate::customise::GatewayConfig::for_claude(&turn.launch.gateway_mcp),
        ),
        EngineId::Claude | EngineId::Codex | EngineId::OpenCode => None,
    };
    let launch_args = gateway.as_ref().map_or_else(
        || turn.launch.args.clone(),
        |gateway| gateway.claude_args(&turn.launch.args),
    );
    let mut command = crate::account::engine_command(turn.engine, &turn.binary, &turn.home)
        .map_err(|source| SpawnError::Guard {
            engine: turn.engine,
            thread: turn.thread_id.clone(),
            source,
        })?;
    command
        .args(args)
        .args(&launch_args)
        .current_dir(cwd)
        .envs(turn.launch.env.iter().map(|(key, value)| (key, value)))
        .envs(
            gateway
                .iter()
                .flat_map(crate::customise::GatewayConfig::env),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let trace = turn
        .trace_dir
        .as_deref()
        .map_or_else(Trace::default, |dir| {
            let environment = crate::trace::effective_environment(command.as_std());
            Trace::start(dir, name, Secrets::of(&turn.launch, &environment)).map_or_else(
                |failure| {
                    let error: &dyn std::error::Error = &failure;
                    tracing::warn!(error, "this engine process runs without a diagnostic trace");
                    Trace::default()
                },
                |started| started.trace,
            )
        });
    let sandbox = Sandbox::for_turn(turn).map_err(|source| SpawnError::Sandbox {
        engine: turn.engine,
        thread: turn.thread_id.clone(),
        source: Box::new(source),
    })?;
    spawn_command(command, trace, sandbox, StderrUse::Traced)
}

pub(crate) fn spawn_probe(command: Command) -> Result<EngineProcess, SpawnError> {
    spawn_command(command, Trace::default(), Sandbox::Open, StderrUse::Traced)
}

#[derive(Debug)]
pub(crate) struct OwnedChild {
    child: Box<dyn ChildWrapper>,
    armed: bool,
    tree: forge_isolation::EngineTree,
    sandbox: Sandbox,
}
impl std::ops::Deref for OwnedChild {
    type Target = dyn ChildWrapper;
    fn deref(&self) -> &Self::Target {
        self.child.as_ref()
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.child.as_mut()
    }
}
impl OwnedChild {
    pub(crate) fn shutdown(&mut self) {
        if self.armed {
            if let Err(error) = self.child.start_kill() {
                tracing::debug!(%error, "the engine process had already exited when it was dropped");
            }
            self.armed = false;
            end_tree(&self.tree, &self.sandbox);
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn end_tree(tree: &forge_isolation::EngineTree, sandbox: &Sandbox) {
    if let Err(failure) = tree.end() {
        let error: &dyn std::error::Error = &failure;
        tracing::warn!(
            error,
            "an engine run's processes may outlive it; its engine's configuration stays guarded until Forge exits"
        );
        sandbox.outlived();
    }
}

pub(crate) fn owned_command(
    mut command: Command,
    sandbox: Sandbox,
) -> Result<OwnedChild, SpawnError> {
    let program = PathBuf::from(command.as_std().get_program());
    sandbox.prepare(&mut command);
    let tree = forge_isolation::EngineTree::new().map_err(|source| SpawnError::Tree {
        program: program.clone(),
        source,
    })?;
    forge_isolation::contain(command.as_std_mut(), sandbox.ruleset(), &tree);
    let mut command = CommandWrap::from(command);
    command.wrap(KillOnDrop);
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::tokio::JobObject);
    match command.spawn() {
        Ok(child) => {
            if let Some(process_id) = child.id() {
                forge_isolation::started(&tree, process_id);
            }
            Ok(OwnedChild {
                child,
                armed: true,
                tree,
                sandbox,
            })
        }
        Err(source) => {
            end_tree(&tree, &sandbox);
            Err(SpawnError::Start { program, source })
        }
    }
}

fn spawn_command(
    mut command: Command,
    trace: Trace,
    sandbox: Sandbox,
    stderr_use: StderrUse,
) -> Result<EngineProcess, SpawnError> {
    let program = PathBuf::from(command.as_std().get_program());
    command.stdout(Stdio::piped());
    let mut child = owned_command(command, sandbox)?;
    trace.record(Kind::Note, &format!("started (pid {:?})", child.id()));
    let stdin = child.stdin().take().map(|stdin| StdinWriter {
        program: program.clone(),
        stdin,
        trace: trace.clone(),
    });
    let stdout = BufReader::new(child.stdout().take().unwrap_or_else(|| {
        unreachable!(
            "invariant: {} was spawned with a piped stdout",
            program.display()
        )
    }));
    let stderr_bytes = Arc::new(AtomicU64::new(0));
    let captured = match stderr_use {
        StderrUse::Traced => None,
        StderrUse::Captured(captured) => Some(captured),
    };
    let stderr_task = child.stderr().take().map(|mut stderr| {
        let counted = Arc::clone(&stderr_bytes);
        let stderr_trace = trace.clone();
        let reader_program = program.clone();
        tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            let mut buffer = vec![0_u8; 4096];
            let mut traced = stderr_trace.stream(Kind::Stderr);
            loop {
                let read = match stderr.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) => {
                        tracing::debug!(program = %reader_program.display(), %error, "stopped reading the engine's stderr");
                        break;
                    }
                };
                counted.fetch_add(read as u64, Ordering::Relaxed);
                traced.write(&buffer[..read]);
                if let Some(captured) = &captured {
                    let mut captured = captured
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    captured.extend_from_slice(&buffer[..read]);
                    let excess = captured.len().saturating_sub(PROBE_STDERR_BYTES);
                    captured.drain(..excess);
                }
            }
            tracing::debug!(
                target: "engine_stderr",
                program = %reader_program.display(),
                bytes = counted.load(Ordering::Relaxed),
                "the engine's stderr closed"
            );
        }))
    });
    Ok(EngineProcess {
        program,
        child,
        stdin,
        stdout,
        stderr_bytes,
        line: Vec::new(),
        oversized: false,
        trace,
        stderr_task,
    })
}

impl EngineProcess {
    pub(crate) async fn send(&mut self, value: &serde_json::Value) -> Result<(), SendError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(SendError::StdinClosed {
                program: self.program.clone(),
            });
        };
        stdin.send(value).await
    }

    pub(crate) fn take_stdin(&mut self) -> Result<StdinWriter, SendError> {
        self.stdin.take().ok_or_else(|| SendError::StdinClosed {
            program: self.program.clone(),
        })
    }

    pub(crate) fn close_stdin(&mut self, why: &str) {
        self.trace
            .record(Kind::Note, &format!("closing stdin: {why}"));
        self.stdin = None;
    }

    pub(crate) fn note(&self, text: &str) {
        self.trace.record(Kind::Note, text);
    }

    pub(crate) async fn next_line(&mut self) -> Result<Option<String>, ReadError> {
        loop {
            loop {
                let available = self
                    .stdout
                    .fill_buf()
                    .await
                    .map_err(|source| ReadError::Read {
                        program: self.program.clone(),
                        source,
                    })?;
                if available.is_empty() {
                    if self.line.is_empty() && !self.oversized {
                        return Ok(None);
                    }
                    break;
                }
                let (chunk, found) = available
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or((available, false), |index| (&available[..=index], true));
                let used = chunk.len();
                if !self.oversized && self.line.len() + used <= MAX_LINE_BYTES {
                    self.line.extend_from_slice(chunk);
                } else {
                    self.oversized = true;
                    self.line.clear();
                }
                self.stdout.consume(used);
                if found {
                    break;
                }
            }
            let mut line = std::mem::take(&mut self.line);
            if std::mem::take(&mut self.oversized) {
                return Err(ReadError::Oversized {
                    program: self.program.clone(),
                    limit: MAX_LINE_BYTES,
                });
            }
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            let text = String::from_utf8(line)
                .unwrap_or_else(|invalid| String::from_utf8_lossy(invalid.as_bytes()).into_owned());
            self.trace.record(Kind::Out, &text);
            if text.trim().is_empty() {
                self.line = text.into_bytes();
                self.line.clear();
                continue;
            }
            return Ok(Some(text));
        }
    }

    #[must_use]
    pub(crate) fn stderr(&self) -> StderrWritten {
        StderrWritten {
            bytes: self.stderr_bytes.load(Ordering::Relaxed),
            trace: self.trace.name(),
        }
    }

    pub(crate) async fn kill(&mut self) {
        self.trace.record(Kind::Note, "killing the process");
        self.child.shutdown();
        if let Err(failure) = self.child.wait().await {
            let error: &dyn std::error::Error = &failure;
            tracing::debug!(program = %self.program.display(), error, "couldn't reap a killed engine process");
        }
    }

    pub(crate) async fn finish(&mut self) {
        if let Exit::Unreaped(error) = self.wait().await {
            tracing::debug!(program = %self.program.display(), %error, "couldn't reap a finished engine process");
        }
    }

    pub(crate) async fn wait(&mut self) -> Exit {
        let status = self.child.wait().await;
        self.child.shutdown();
        self.trace
            .record(Kind::Note, &format!("exited: {status:?}"));
        match status {
            Ok(status) => status.code().map_or(Exit::Signal, Exit::Code),
            Err(error) => Exit::Unreaped(error),
        }
    }
}

pub async fn probe(
    mut command: Command,
    timeout: std::time::Duration,
    limit: usize,
) -> Result<std::process::Output, ProbeError> {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut process = spawn_command(
        command,
        Trace::default(),
        Sandbox::Open,
        StderrUse::Captured(Arc::clone(&captured)),
    )
    .map_err(ProbeError::Spawn)?;
    let program = process.program.clone();
    let result = tokio::time::timeout(timeout, async {
        let mut stdout = Vec::new();
        (&mut process.stdout)
            .take(limit as u64 + 1)
            .read_to_end(&mut stdout)
            .await
            .map_err(|source| ProbeError::Read {
                program: program.clone(),
                source,
            })?;
        if stdout.len() > limit {
            return Err(ProbeError::Oversized {
                program: program.clone(),
                limit,
            });
        }
        let status = process
            .child
            .wait()
            .await
            .map_err(|source| ProbeError::Wait {
                program: program.clone(),
                source,
            })?;
        process.child.shutdown();
        if let Some(stderr) = process.stderr_task.take()
            && let Err(failure) = stderr.await
        {
            let error: &dyn std::error::Error = &failure;
            tracing::debug!(program = %program.display(), error, "a probe's stderr reader ended early");
        }
        Ok(std::process::Output {
            stdout,
            stderr: std::mem::take(
                &mut *captured
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ),
            status,
        })
    })
    .await
    .unwrap_or_else(|_elapsed| {
        Err(ProbeError::TimedOut {
            program: program.clone(),
            after: timeout,
        })
    });
    if result.is_err() {
        process.kill().await;
    }
    result
}

#[cfg(test)]
#[cfg(unix)]
#[path = "../tests/process/gateway.rs"]
mod gateway_tests;

#[cfg(test)]
#[cfg(unix)]
#[path = "../tests/process/logs.rs"]
mod log_tests;

#[cfg(test)]
#[cfg(unix)]
#[path = "../tests/process/containment.rs"]
mod containment_tests;

#[cfg(test)]
#[cfg(unix)]
#[path = "../tests/process/reform.rs"]
mod reform_tests;

#[cfg(test)]
#[cfg(windows)]
#[path = "../tests/process/windows_owner.rs"]
mod windows_owner_tests;

#[cfg(test)]
#[cfg(unix)]
#[path = "../tests/process/redaction.rs"]
mod redaction_tests;
