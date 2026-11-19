use crate::Turn;
use crate::sandbox::Sandbox;
use crate::trace::{Kind, Secrets, Trace};
use artisan_api::{EngineId, ThreadId};
use process_wrap::tokio::{ChildWrapper, CommandWrap, CommandWrapper, KillOnDrop};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

pub const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 8 * 1024;

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
    #[error("start {}", program.display())]
    Start {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },
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

pub struct EngineProcess {
    program: PathBuf,
    child: OwnedChild,
    stdin: Option<StdinWriter>,
    stdout: BufReader<ChildStdout>,
    stderr_tail: Arc<Mutex<String>>,
    line: Vec<u8>,
    oversized: bool,
    trace: Trace,
    stderr_task: Option<tokio_util::task::AbortOnDropHandle<()>>,
}

pub fn spawn(turn: &Turn, args: &[String]) -> Result<EngineProcess, SpawnError> {
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
            Trace::start(dir, name, Secrets::of(&turn.launch)).map_or_else(
                |failure| {
                    let error: &dyn std::error::Error = &failure;
                    tracing::warn!(error, "this engine process runs without a diagnostic trace");
                    Trace::default()
                },
                |started| started.trace,
            )
        });
    let sandbox =
        crate::sandbox::Sandbox::for_turn(turn).map_err(|source| SpawnError::Sandbox {
            engine: turn.engine,
            thread: turn.thread_id.clone(),
            source: Box::new(source),
        })?;
    spawn_command(command, trace, sandbox)
}

pub(crate) fn spawn_probe(command: Command) -> Result<EngineProcess, SpawnError> {
    spawn_command(command, Trace::default(), Sandbox::Open)
}

pub(crate) struct OwnedChild {
    child: Box<dyn ChildWrapper>,
    armed: bool,
    _sandbox: Sandbox,
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
            self.child.start_kill().ok();
            self.armed = false;
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Debug)]
struct Isolated(Sandbox);

impl CommandWrapper for Isolated {
    fn pre_spawn(&mut self, command: &mut Command, _core: &CommandWrap) -> std::io::Result<()> {
        forge_isolation::contain(command.as_std_mut(), self.0.ruleset());
        Ok(())
    }

    fn post_spawn(
        &mut self,
        _command: &mut Command,
        child: &mut tokio::process::Child,
        _core: &CommandWrap,
    ) -> std::io::Result<()> {
        if let Some(process_id) = child.id() {
            forge_isolation::started(process_id);
        }
        Ok(())
    }
}

pub(crate) fn owned_command(
    mut command: Command,
    sandbox: Sandbox,
) -> Result<OwnedChild, SpawnError> {
    let program = PathBuf::from(command.as_std().get_program());
    sandbox.prepare(&mut command);
    let mut command = CommandWrap::from(command);
    command.wrap(KillOnDrop);
    command.wrap(Isolated(sandbox.clone()));
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::tokio::JobObject);
    command
        .spawn()
        .map(|child| OwnedChild {
            child,
            armed: true,
            _sandbox: sandbox,
        })
        .map_err(|source| SpawnError::Start { program, source })
}

fn spawn_command(
    mut command: Command,
    trace: Trace,
    sandbox: Sandbox,
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
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    let stderr_task = child.stderr().take().map(|mut stderr| {
        let tail = Arc::clone(&stderr_tail);
        let stderr_trace = trace.clone();
        tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            let mut buffer = vec![0_u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buffer[..read]);
                tracing::debug!(target: "engine_stderr", bytes = read, "the engine wrote to stderr");
                stderr_trace.record(Kind::Stderr, &text);
                let mut tail = tail
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                tail.push_str(&text);
                if tail.len() > STDERR_TAIL_BYTES {
                    let mut cut = tail.len() - STDERR_TAIL_BYTES;
                    while !tail.is_char_boundary(cut) {
                        cut += 1;
                    }
                    tail.drain(..cut);
                }
            }
        }))
    });
    Ok(EngineProcess {
        program,
        child,
        stdin,
        stdout,
        stderr_tail,
        line: Vec::new(),
        oversized: false,
        trace,
        stderr_task,
    })
}

impl EngineProcess {
    pub async fn send(&mut self, value: &serde_json::Value) -> Result<(), SendError> {
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

    pub fn close_stdin(&mut self, why: &str) {
        self.trace
            .record(Kind::Note, &format!("closing stdin: {why}"));
        self.stdin = None;
    }

    pub fn note(&self, text: &str) {
        self.trace.record(Kind::Note, text);
    }

    pub async fn next_line(&mut self) -> Result<Option<String>, ReadError> {
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
    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .trim()
            .to_owned()
    }

    pub async fn kill(&mut self) {
        self.trace.record(Kind::Note, "killing the process");
        self.child.shutdown();
        if let Err(failure) = self.child.wait().await {
            let error: &dyn std::error::Error = &failure;
            tracing::debug!(program = %self.program.display(), error, "couldn't reap a killed engine process");
        }
    }

    pub async fn wait(&mut self) -> Option<i32> {
        let status = self.child.wait().await;
        self.child.shutdown();
        self.trace
            .record(Kind::Note, &format!("exited: {status:?}"));
        status.ok().and_then(|status| status.code())
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
    let mut process = spawn_probe(command).map_err(ProbeError::Spawn)?;
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
            stderr: process.stderr_tail().into_bytes(),
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
