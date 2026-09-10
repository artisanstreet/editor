//! Bounded child-process plumbing shared by the Codex probes.
//!
//! One focused helper set: pre-spawn deadline validation, shell-free argv
//! spawns, concurrent pipe pumps with byte bounds, and kill-then-reap
//! shutdown. Callers drive their own request/response exchange on top.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::probe::CodexProbeError;

/// Poll interval for child-exit and drain-channel checks.
pub(crate) const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Grace period for a drain thread to observe end of stream after its child
/// is reaped before the thread is detached.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// Validates a probe time budget before spawning.
///
/// # Errors
///
/// Returns [`CodexProbeError::Timeout`] for a zero or overflowing deadline so
/// no child starts without a usable budget.
pub(crate) fn probe_deadline(timeout: Duration) -> Result<Instant, CodexProbeError> {
    if timeout.is_zero() {
        return Err(CodexProbeError::Timeout);
    }
    Instant::now()
        .checked_add(timeout)
        .ok_or(CodexProbeError::Timeout)
}

/// Spawns a probe child with piped stdio and no shell.
///
/// Array entries pass paths containing spaces as single arguments.
/// `CODEX_HOME` is set only when provided; the rest of the environment is
/// inherited.
///
/// # Errors
///
/// Returns [`CodexProbeError::InvalidBinary`] when the child cannot start.
pub(crate) fn spawn_probe_child(
    executable: &Path,
    argv: &[String],
    codex_home: Option<&Path>,
) -> Result<Child, CodexProbeError> {
    let mut command = Command::new(executable);
    command.args(argv);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if let Some(home) = codex_home {
        command.env("CODEX_HOME", home);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn().map_err(|_| CodexProbeError::InvalidBinary)
}

/// Takes a piped stdout handle from a freshly spawned probe child.
///
/// # Errors
///
/// Returns [`CodexProbeError::Unavailable`] when the handle is missing.
pub(crate) fn take_child_stdout(child: &mut Child) -> Result<ChildStdout, CodexProbeError> {
    child.stdout.take().ok_or(CodexProbeError::Unavailable)
}

/// Takes a piped stderr handle from a freshly spawned probe child.
///
/// # Errors
///
/// Returns [`CodexProbeError::Unavailable`] when the handle is missing.
pub(crate) fn take_child_stderr(child: &mut Child) -> Result<ChildStderr, CodexProbeError> {
    child.stderr.take().ok_or(CodexProbeError::Unavailable)
}

/// Events from one concurrently drained pipe.
pub(crate) enum PipeEvent {
    /// One newline-terminated (or final partial) chunk.
    Line(Vec<u8>),
    /// Every write end is closed.
    Eof,
    /// The stream exceeded its byte bound.
    TooLarge,
    /// The stream failed while reading.
    Io,
}

/// One pipe-drain thread plus its event channel.
pub(crate) struct PipeDrain {
    events: Receiver<PipeEvent>,
    handle: Option<JoinHandle<()>>,
}

impl PipeDrain {
    /// Starts draining `pipe` with a total byte bound.
    pub(crate) fn spawn_pipe<R>(pipe: R, max_bytes: usize) -> Self
    where
        R: Read + Send + 'static,
    {
        let (sender, events) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut total = 0_usize;
            loop {
                let mut line = Vec::new();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => {
                        let _ = sender.send(PipeEvent::Eof);
                        break;
                    }
                    Ok(_) => {
                        total = total.saturating_add(line.len());
                        if total > max_bytes {
                            let _ = sender.send(PipeEvent::TooLarge);
                            break;
                        }
                        if sender.send(PipeEvent::Line(line)).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = sender.send(PipeEvent::Io);
                        break;
                    }
                }
            }
        });
        Self {
            events,
            handle: Some(handle),
        }
    }

    /// Returns the event channel.
    pub(crate) const fn events(&self) -> &Receiver<PipeEvent> {
        &self.events
    }

    /// Joins the drain thread, detaching it after a bounded grace period.
    ///
    /// Detachment only happens when another process inherited the pipe and
    /// holds it open; normally exited children always join.
    pub(crate) fn join_or_detach(&mut self) {
        if let Some(handle) = self.handle.take() {
            let grace = Instant::now().checked_add(DRAIN_GRACE);
            while !handle.is_finished() {
                if grace.is_none_or(|deadline| Instant::now() >= deadline) {
                    return;
                }
                thread::sleep(PROBE_POLL_INTERVAL);
            }
            let _ = handle.join();
        }
    }
}

/// Kills and reaps a probe child, ignoring the outcome.
pub(crate) fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Reaps an exited child, killing it first when the deadline passes.
///
/// # Errors
///
/// Returns [`CodexProbeError::Timeout`] when the deadline passes first; the
/// child is killed and reaped before returning.
/// Returns [`CodexProbeError::Unavailable`] when the child handle is broken.
pub(crate) fn reap_or_kill(
    child: &mut Child,
    deadline: Instant,
) -> Result<ExitStatus, CodexProbeError> {
    loop {
        match child.try_wait().map_err(|_| CodexProbeError::Unavailable)? {
            Some(status) => return Ok(status),
            None => {
                if Instant::now() >= deadline {
                    stop_child(child);
                    return Err(CodexProbeError::Timeout);
                }
                thread::sleep(PROBE_POLL_INTERVAL);
            }
        }
    }
}
