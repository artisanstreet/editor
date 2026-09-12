#![allow(clippy::module_name_repetitions)]
#![allow(dead_code)]

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::time::{Instant, timeout_at};

#[cfg(windows)]
use command_group::AsyncCommandGroup;

#[cfg(windows)]
use super::super::consts::CREATE_NO_WINDOW;

use super::super::process::POST_KILL_GRACE;
use super::{
    ACP_CLIENT_NAME, ACP_CLIENT_VERSION, ACP_PROTOCOL_VERSION, AcpBounds, AcpEnvelope, AcpError,
    AcpFramer, AcpId, AcpResponsePayload, InitializeResult, METHOD_AUTHENTICATE, METHOD_INITIALIZE,
    METHOD_SESSION_CANCEL, METHOD_SESSION_LOAD, METHOD_SESSION_NEW, METHOD_SESSION_PROMPT,
    METHOD_SESSION_UPDATE, PromptOutcome, SessionId, SessionUpdate, parse_envelope,
    parse_initialize_result, parse_prompt_result, parse_session_update,
};

// ---------------------------------------------------------------------------
// Exit classification: interruption vs failure vs cancel
// ---------------------------------------------------------------------------

/// The platform exit facts relevant to classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ObservedExit {
    pub(crate) code: Option<i32>,
    pub(crate) signaled: bool,
}

impl From<ExitStatus> for ObservedExit {
    fn from(status: ExitStatus) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            Self {
                code: status.code(),
                signaled: status.signal().is_some(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                code: status.code(),
                signaled: false,
            }
        }
    }
}

/// How an ACP child ending maps to run fate, mirroring
/// `engine_exit_is_interruption`: a signal, or no code at all, means the
/// process never chose to stop. On Windows there are no signals, so a
/// `TerminateProcess` kill surfaces as an ordinary non-zero exit; the
/// durable recovery path covers that case instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClassifiedExit {
    Cancelled,
    Interrupted,
    Failed { code: Option<i32> },
}

/// Classifies one observed exit. An explicit caller cancellation wins over
/// every platform fact.
///
/// # Panics
///
/// Never panics; the mapping is total over the input facts.
#[must_use = "classification must drive the terminal observation"]
pub(crate) fn classify_exit(exit: ObservedExit, cancelled: bool) -> ClassifiedExit {
    if cancelled {
        return ClassifiedExit::Cancelled;
    }
    if exit.signaled || exit.code.is_none() {
        return ClassifiedExit::Interrupted;
    }
    ClassifiedExit::Failed { code: exit.code }
}

// ---------------------------------------------------------------------------
// Child spawn and teardown: kill-then-reap with quarantine
// ---------------------------------------------------------------------------

/// The taken stdio pipes of one ACP child. The stderr handle is handed to
/// the dispatch arm, which owns count-only draining; this core never logs
/// provider bytes.
pub(crate) struct AcpPipes {
    pub(crate) stdin: ChildStdin,
    pub(crate) stdout: ChildStdout,
    pub(crate) stderr: ChildStderr,
}

enum ChildInner {
    #[cfg(windows)]
    Grouped(command_group::AsyncGroupChild),
    #[cfg(not(windows))]
    Direct(tokio::process::Child),
}

impl ChildInner {
    async fn wait(&mut self) -> io::Result<ExitStatus> {
        match self {
            #[cfg(windows)]
            Self::Grouped(grouped) => grouped.wait().await,
            #[cfg(not(windows))]
            Self::Direct(direct) => direct.wait().await,
        }
    }

    fn start_kill(&mut self) -> io::Result<()> {
        match self {
            #[cfg(windows)]
            Self::Grouped(grouped) => grouped.start_kill(),
            #[cfg(not(windows))]
            Self::Direct(direct) => direct.start_kill(),
        }
    }

    fn id(&self) -> Option<u32> {
        match self {
            #[cfg(windows)]
            Self::Grouped(grouped) => grouped.id(),
            #[cfg(not(windows))]
            Self::Direct(direct) => direct.id(),
        }
    }
}

/// One ACP child: the process handle plus its pipes until taken.
pub(crate) struct AcpChild {
    inner: ChildInner,
    pipes: Option<AcpPipes>,
}

impl AcpChild {
    /// Takes the piped stdio for the transport. The dispatch arm must close
    /// the writer (lifeline EOF) before [`shutdown_acp_child`].
    #[must_use = "taken pipes must feed the transport"]
    pub(crate) fn take_pipes(&mut self) -> Option<AcpPipes> {
        self.pipes.take()
    }

    /// Returns the leader process identifier while it remains available.
    #[allow(dead_code)]
    pub(crate) fn id(&self) -> Option<u32> {
        self.inner.id()
    }
}

/// Spawns one ACP agent with piped stdio and no shell.
///
/// The environment is inherited (PATH resolution plus ambient auth),
/// mirroring the TypeScript factory's `env: process.env` evidence; no
/// command interpreter is ever inserted between the caller and the agent.
///
/// # Errors
///
/// Returns the raw spawn failure; the caller reduces it to a typed,
/// payload-free cause.
pub(crate) fn spawn_acp_child(
    program: &OsStr,
    args: &[OsString],
    cwd: Option<&Path>,
) -> io::Result<AcpChild> {
    let mut command = tokio::process::Command::new(program);
    for arg in args {
        command.arg(arg);
    }
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        let mut grouped = command
            .group()
            .kill_on_drop(true)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        let pipes = {
            let raw = grouped.inner();
            let stdin = raw.stdin.take();
            let stdout = raw.stdout.take();
            let stderr = raw.stderr.take();
            if let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) {
                AcpPipes {
                    stdin,
                    stdout,
                    stderr,
                }
            } else {
                let _ignored = grouped.start_kill();
                return Err(io::Error::other("acp stdio unavailable"));
            }
        };
        Ok(AcpChild {
            inner: ChildInner::Grouped(grouped),
            pipes: Some(pipes),
        })
    }

    #[cfg(not(windows))]
    {
        command.kill_on_drop(true);
        let mut direct = command.spawn()?;
        let pipes = match (
            direct.stdin.take(),
            direct.stdout.take(),
            direct.stderr.take(),
        ) {
            (Some(stdin), Some(stdout), Some(stderr)) => AcpPipes {
                stdin,
                stdout,
                stderr,
            },
            _ => {
                let _ignored = direct.start_kill();
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "acp stdio unavailable",
                ));
            }
        };
        Ok(AcpChild {
            inner: ChildInner::Direct(direct),
            pipes: Some(pipes),
        })
    }
}

/// Exact custody retained when an ACP child's death could not be observed.
/// The single owner task quarantines this value; it is never dropped
/// silently while the child may still be alive.
pub(crate) struct AcpRetainedChild {
    inner: ChildInner,
}

impl AcpRetainedChild {
    /// Returns the leader process identifier while it remains available.
    #[allow(dead_code)]
    pub(crate) fn id(&self) -> Option<u32> {
        self.inner.id()
    }
}

impl fmt::Debug for AcpRetainedChild {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AcpRetainedChild { <redacted> }")
    }
}

/// How ACP teardown ended, mirroring the `process.rs` cleanup order:
/// close the lifeline first, bounded wait, `start_kill` only when no exit
/// was observed, one more wait on the remaining budget (never less than the
/// defined post-kill grace), else quarantine.
#[derive(Debug)]
pub(crate) enum AcpShutdown {
    ReapedWithoutKill(ExitStatus),
    ReapedAfterKill(ExitStatus),
    Retained(Box<AcpRetainedChild>),
}

async fn wait_for_child(inner: &mut ChildInner, deadline: Instant) -> io::Result<ExitStatus> {
    match timeout_at(deadline, inner.wait()).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "bounded acp wait elapsed",
        )),
    }
}

/// Runs the fixed teardown for one ACP child.
///
/// The caller must have closed the stdin lifeline (drop the transport
/// writer or call `shutdown_writer`) so an EOF-clean agent can exit on its
/// own before any kill is requested.
pub(crate) async fn shutdown_acp_child(child: AcpChild, close_budget: Duration) -> AcpShutdown {
    let AcpChild { mut inner, pipes } = child;
    drop(pipes);
    let start = Instant::now();
    let deadline = match start.checked_add(close_budget) {
        Some(deadline) => deadline,
        None => start,
    };
    if let Ok(status) = wait_for_child(&mut inner, deadline).await {
        return AcpShutdown::ReapedWithoutKill(status);
    }
    let _ignored = inner.start_kill();
    // The kill always gets the defined observation grace, even when the
    // caller supplied ZERO or the close budget is already consumed, so a
    // terminated child cannot decay into quarantine on a bare poll.
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .unwrap_or(Duration::ZERO)
        .max(POST_KILL_GRACE);
    let second = Instant::now().checked_add(remaining).unwrap_or(deadline);
    let settled = wait_for_child(&mut inner, second).await;
    match settled {
        Ok(status) => AcpShutdown::ReapedAfterKill(status),
        Err(_) => AcpShutdown::Retained(Box::new(AcpRetainedChild { inner })),
    }
}

// ---------------------------------------------------------------------------
// Transport: handshake, sessions, update loop, cancel/close
// ---------------------------------------------------------------------------

/// One inbound item: a correlated response, an agent-initiated request for
/// the A2 bridges, or a notification (including `session/update`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AcpInbound {
    Response {
        id: AcpId,
        payload: AcpResponsePayload,
    },
    AgentRequest {
        id: AcpId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

/// One update-loop item: a validated session update for our session, the
/// prompt round result, or an agent-initiated request the A2 bridges own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UpdateEvent {
    SessionUpdate(SessionUpdate),
    PromptResult(PromptOutcome),
    AgentRequest {
        id: AcpId,
        method: String,
        params: Value,
    },
}

/// The finite ACP transport over caller-provided async pipes.
///
/// Generic over the pipe halves so fixture tests drive the exact stdio byte
/// protocol over in-memory pipes while production passes real child pipes.
/// No tasks are spawned; every method runs on the dispatch arm's task.
pub(crate) struct AcpTransport<R, W> {
    reader: BufReader<R>,
    writer: W,
    framer: AcpFramer,
    pending_lines: VecDeque<String>,
    max_envelope: usize,
    max_session_id: usize,
    handshake: Duration,
    inactivity: Duration,
    next_id: i64,
    eof_seen: bool,
}

impl<R: AsyncRead + Unpin + Send, W: AsyncWrite + Unpin + Send> AcpTransport<R, W> {
    /// Wraps async pipes with the caller-supplied bounds.
    pub(crate) fn new(reader: R, writer: W, bounds: AcpBounds) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            framer: AcpFramer::new(bounds.max_line_bytes),
            pending_lines: VecDeque::new(),
            max_envelope: bounds.max_envelope_bytes,
            max_session_id: bounds.max_session_id_bytes,
            handshake: bounds.handshake_timeout,
            inactivity: bounds.inactivity_timeout,
            next_id: 1,
            eof_seen: false,
        }
    }

    /// Returns the rolling deadline for the next expected frame.
    #[must_use = "deadlines must bound the update loop"]
    pub(crate) fn activity_deadline(&self) -> Instant {
        Instant::now()
            .checked_add(self.inactivity)
            .unwrap_or_else(Instant::now)
    }

    async fn write_line(&mut self, line: &str) -> Result<(), AcpError> {
        if line.len() > self.max_envelope {
            return Err(AcpError::EnvelopeTooLarge);
        }
        let mut framed = line.to_owned();
        framed.push('\n');
        self.writer
            .write_all(framed.as_bytes())
            .await
            .map_err(|_| AcpError::IoFailed)?;
        self.writer.flush().await.map_err(|_| AcpError::IoFailed)?;
        Ok(())
    }

    pub(super) async fn send_request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<AcpId, AcpError> {
        let id_number = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(AcpError::Poisoned)?;
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id_number,
            "method": method,
            "params": params,
        })
        .to_string();
        self.write_line(&line).await?;
        Ok(AcpId::Number(id_number))
    }

    async fn send_notification(&mut self, method: &str, params: Value) -> Result<(), AcpError> {
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        })
        .to_string();
        self.write_line(&line).await
    }

    async fn read_frame(&mut self, deadline: Instant) -> Result<String, AcpError> {
        if let Some(line) = self.pending_lines.pop_front() {
            return Ok(line);
        }
        if self.eof_seen {
            return Err(AcpError::PeerClosed);
        }
        let mut chunk = [0_u8; 4096];
        loop {
            match timeout_at(deadline, self.reader.read(&mut chunk)).await {
                Ok(Ok(0)) => {
                    self.eof_seen = true;
                    self.framer.finish()?;
                    return Err(AcpError::PeerClosed);
                }
                Ok(Ok(count)) => {
                    let mut lines = self.framer.push(&chunk[..count])?;
                    if lines.is_empty() {
                        continue;
                    }
                    let first = lines.remove(0);
                    self.pending_lines.extend(lines);
                    return Ok(first);
                }
                Ok(Err(_)) => return Err(AcpError::IoFailed),
                Err(_) => return Err(AcpError::InactivityStall),
            }
        }
    }

    /// Reads one typed inbound item before `deadline`.
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::InactivityStall`] when no frame arrives in time,
    /// [`AcpError::PeerClosed`] on clean EOF, or the framing/envelope error
    /// for malformed bytes.
    pub(crate) async fn next_inbound(&mut self, deadline: Instant) -> Result<AcpInbound, AcpError> {
        let line = self.read_frame(deadline).await?;
        match parse_envelope(&line, self.max_envelope)? {
            AcpEnvelope::Response { id, payload } => Ok(AcpInbound::Response { id, payload }),
            AcpEnvelope::Request { id, method, params } => {
                Ok(AcpInbound::AgentRequest { id, method, params })
            }
            AcpEnvelope::Notification { method, params } => {
                Ok(AcpInbound::Notification { method, params })
            }
        }
    }

    async fn await_response(
        &mut self,
        id: &AcpId,
        deadline: Instant,
    ) -> Result<AcpResponsePayload, AcpError> {
        loop {
            match self.next_inbound(deadline).await? {
                AcpInbound::Response {
                    id: actual,
                    payload,
                } if actual == *id => return Ok(payload),
                AcpInbound::Response { .. }
                | AcpInbound::AgentRequest { .. }
                | AcpInbound::Notification { .. } => {}
            }
        }
    }

    fn phase_deadline(&self) -> Result<Instant, AcpError> {
        Instant::now()
            .checked_add(self.handshake)
            .ok_or(AcpError::UnrepresentableDeadline)
    }

    /// Runs the bounded `initialize` handshake: send, await the matching
    /// response on an absolute deadline, and check the protocol version.
    /// Any complete frame ends the wait only when it is our response;
    /// silence past the deadline is [`AcpError::HandshakeTimeout`].
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::HandshakeTimeout`], [`AcpError::HandshakeRejected`],
    /// [`AcpError::UnsupportedVersion`], or the framing/envelope error.
    pub(crate) async fn initialize(&mut self) -> Result<InitializeResult, AcpError> {
        let deadline = self.phase_deadline()?;
        let id = self
            .send_request(
                METHOD_INITIALIZE,
                serde_json::json!({
                    "protocolVersion": ACP_PROTOCOL_VERSION,
                    "clientCapabilities": { "plan": {}, "session": { "compaction": {} } },
                    "clientInfo": { "name": ACP_CLIENT_NAME, "version": ACP_CLIENT_VERSION },
                }),
            )
            .await?;
        let payload = match self.await_response(&id, deadline).await {
            Ok(payload) => payload,
            Err(AcpError::InactivityStall | AcpError::PeerClosed) => {
                return Err(AcpError::HandshakeTimeout);
            }
            Err(other) => return Err(other),
        };
        match payload {
            AcpResponsePayload::Error { .. } => Err(AcpError::HandshakeRejected),
            AcpResponsePayload::Result(result) => {
                let parsed = parse_initialize_result(&result)?;
                if parsed.protocol_version != ACP_PROTOCOL_VERSION {
                    return Err(AcpError::UnsupportedVersion);
                }
                Ok(parsed)
            }
        }
    }

    /// Sends `authenticate` for a row-selected method and awaits the
    /// matching response inside a fresh handshake window.
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::AuthUnavailable`] on an error payload or silence,
    /// or the framing/envelope error.
    pub(crate) async fn authenticate(&mut self, method_id: &str) -> Result<(), AcpError> {
        let deadline = self.phase_deadline()?;
        let id = self
            .send_request(
                METHOD_AUTHENTICATE,
                serde_json::json!({ "_meta": { "headless": true }, "methodId": method_id }),
            )
            .await?;
        match self.await_response(&id, deadline).await {
            Ok(AcpResponsePayload::Result(_)) => Ok(()),
            Ok(AcpResponsePayload::Error { .. })
            | Err(AcpError::InactivityStall | AcpError::PeerClosed) => {
                Err(AcpError::AuthUnavailable)
            }
            Err(other) => Err(other),
        }
    }

    /// Creates one session rooted at `cwd` and returns its validated id.
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::ChildFailed`] on an error payload or silence, or
    /// the framing/envelope error.
    pub(crate) async fn new_session(&mut self, cwd: &str) -> Result<SessionId, AcpError> {
        let deadline = self.phase_deadline()?;
        let id = self
            .send_request(
                METHOD_SESSION_NEW,
                serde_json::json!({ "cwd": cwd, "mcpServers": [] }),
            )
            .await?;
        let payload = match self.await_response(&id, deadline).await {
            Ok(payload) => payload,
            Err(AcpError::InactivityStall | AcpError::PeerClosed) => {
                return Err(AcpError::ChildFailed);
            }
            Err(other) => return Err(other),
        };
        match payload {
            AcpResponsePayload::Error { .. } => Err(AcpError::ChildFailed),
            AcpResponsePayload::Result(result) => {
                let session_id = result
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .ok_or(AcpError::MalformedEnvelope)?;
                SessionId::parse(session_id, self.max_session_id)
            }
        }
    }

    /// Reopens one session rooted at `cwd`. The result carries no typed
    /// fields the lifecycle needs; any result value resumes.
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::ChildFailed`] on an error payload or silence, or
    /// the framing/envelope error.
    pub(crate) async fn load_session(
        &mut self,
        session: &SessionId,
        cwd: &str,
    ) -> Result<(), AcpError> {
        let deadline = self.phase_deadline()?;
        let id = self
            .send_request(
                METHOD_SESSION_LOAD,
                serde_json::json!({ "cwd": cwd, "mcpServers": [], "sessionId": session.as_str() }),
            )
            .await?;
        match self.await_response(&id, deadline).await {
            Ok(AcpResponsePayload::Result(_)) => Ok(()),
            Ok(AcpResponsePayload::Error { .. })
            | Err(AcpError::InactivityStall | AcpError::PeerClosed) => Err(AcpError::ChildFailed),
            Err(other) => Err(other),
        }
    }

    /// Sends one prompt round and returns its request id for [`Self::next_update`].
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::EnvelopeTooLarge`] before any byte is written, or
    /// [`AcpError::IoFailed`] when the pipe is gone.
    pub(crate) async fn prompt(
        &mut self,
        session: &SessionId,
        content: Vec<Value>,
    ) -> Result<AcpId, AcpError> {
        self.send_request(
            METHOD_SESSION_PROMPT,
            serde_json::json!({ "sessionId": session.as_str(), "prompt": content }),
        )
        .await
    }

    /// Reads the next update-loop item for one prompt round.
    ///
    /// The inactivity deadline is recomputed from the last received frame,
    /// so an active agent re-arms it while a silent one stalls. Foreign
    /// session frames and unrelated responses are skipped; agent-initiated
    /// requests surface untouched for the A2 bridges.
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::InactivityStall`] on a silent window,
    /// [`AcpError::PeerClosed`] on EOF, [`AcpError::ChildFailed`] for a
    /// prompt error payload, or the framing/envelope error.
    pub(crate) async fn next_update(
        &mut self,
        session: &SessionId,
        prompt: &AcpId,
    ) -> Result<UpdateEvent, AcpError> {
        loop {
            let deadline = Instant::now()
                .checked_add(self.inactivity)
                .ok_or(AcpError::UnrepresentableDeadline)?;
            match self.next_inbound(deadline).await? {
                AcpInbound::Response { id, payload } if id == *prompt => {
                    return parse_prompt_result(&payload).map(UpdateEvent::PromptResult);
                }
                AcpInbound::Notification { method, params } if method == METHOD_SESSION_UPDATE => {
                    if let Some(update) =
                        parse_session_update(&params, session, self.max_session_id)?
                    {
                        return Ok(UpdateEvent::SessionUpdate(update));
                    }
                }
                AcpInbound::AgentRequest { id, method, params } => {
                    return Ok(UpdateEvent::AgentRequest { id, method, params });
                }
                AcpInbound::Response { .. } | AcpInbound::Notification { .. } => {}
            }
        }
    }

    /// Notifies `session/cancel` for one session. Delivery failures are
    /// reported so the caller can still settle the run deterministically.
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::EnvelopeTooLarge`] or [`AcpError::IoFailed`].
    pub(crate) async fn cancel(&mut self, session: &SessionId) -> Result<(), AcpError> {
        self.send_notification(
            METHOD_SESSION_CANCEL,
            serde_json::json!({ "sessionId": session.as_str() }),
        )
        .await
    }

    /// Closes the stdin lifeline so an EOF-clean agent can exit on its own.
    /// Call before [`shutdown_acp_child`].
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::IoFailed`] when the pipe is already gone.
    pub(crate) async fn shutdown_writer(&mut self) -> Result<(), AcpError> {
        self.writer.shutdown().await.map_err(|_| AcpError::IoFailed)
    }
}
