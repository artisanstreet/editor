//! Bounded Codex app-server account probe.
//!
//! A real non-billable transport probe using the actual TypeScript protocol:
//! spawn `codex app-server --stdio`, send the JSON-RPC `initialize` request,
//! send `account/read`, then shut the child down. No prompt is sent, no
//! thread or turn starts, and no account or session state is changed.
//!
//! Request shapes mirror `make_codex_initialize_request` and
//! `make_codex_account_read_request`; envelope routing mirrors
//! `DecodeCodexInboundEnvelope` (unambiguous discriminants only); the
//! initialize result mirrors `CodexInitializeResult`; the account result
//! reuses the shared account decoder. One end-to-end deadline and a total
//! byte bound cover the whole exchange.

use std::io::Write;
use std::path::Path;
use std::process::{Child, ChildStdin};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::probe::{CodexAccountRead, CodexAuthState, CodexProbeError, decode_account_value};
use super::process::{
    PROBE_POLL_INTERVAL, PipeDrain, PipeEvent, probe_deadline, spawn_probe_child, stop_child,
    take_child_stderr, take_child_stdout,
};

/// Arguments appended after caller-supplied args to open the app-server.
///
/// Mirrors the TypeScript spawn: `[...executable_args, "app-server", "--stdio"]`.
pub const CODEX_APP_SERVER_ARGS: [&str; 2] = ["app-server", "--stdio"];

/// Notifications the initialize request opts out of.
///
/// Mirrors `codex_opt_out_notification_methods`.
pub const CODEX_OPT_OUT_NOTIFICATION_METHODS: [&str; 3] = [
    "account/rateLimits/updated",
    "mcpServer/startupStatus/updated",
    "remoteControl/status/changed",
];

/// Default deadline for the whole initialize/account-read/shutdown exchange.
pub const CODEX_SESSION_TIMEOUT: Duration = Duration::from_secs(20);

/// Default total byte bound for the session stdout/stderr streams.
pub const CODEX_SESSION_OUTPUT_BOUND_BYTES: usize = 64 * 1024;

/// Default cap on inbound envelopes per awaited response.
pub const CODEX_MAX_INBOUND_ENVELOPES: usize = 1024;

/// Grace period for a clean child exit after stdin closes before it is stopped.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Validated client identity reported to the app-server.
///
/// Both fields must be non-empty, mirroring `CodexClientInfo`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexClientIdentity {
    name: String,
    version: String,
}

impl CodexClientIdentity {
    /// Builds an identity, returning `None` when either field is empty.
    #[must_use]
    pub fn new(name: &str, version: &str) -> Option<Self> {
        if name.is_empty() || version.is_empty() {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
            version: version.to_owned(),
        })
    }

    /// Returns the client name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the client version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Validated app-server handshake metadata.
///
/// Mirrors `CodexInitializeResult`; unknown fields are ignored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexServerInfo {
    codex_home: String,
    platform_family: String,
    platform_os: String,
    user_agent: String,
}

impl CodexServerInfo {
    /// Returns the server-reported Codex home.
    #[must_use]
    pub fn codex_home(&self) -> &str {
        &self.codex_home
    }

    /// Returns the server-reported platform family.
    #[must_use]
    pub fn platform_family(&self) -> &str {
        &self.platform_family
    }

    /// Returns the server-reported operating system.
    #[must_use]
    pub fn platform_os(&self) -> &str {
        &self.platform_os
    }

    /// Returns the server-reported user agent.
    #[must_use]
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }
}

/// Non-billable account probe result: handshake metadata plus auth state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexAccountProbe {
    account: CodexAccountRead,
    server: CodexServerInfo,
}

impl CodexAccountProbe {
    /// Returns the decoded account result.
    #[must_use]
    pub const fn account(&self) -> CodexAccountRead {
        self.account
    }

    /// Returns the handshake metadata.
    #[must_use]
    pub const fn server(&self) -> &CodexServerInfo {
        &self.server
    }

    /// Returns the authentication readiness.
    #[must_use]
    pub fn authentication(&self) -> CodexAuthState {
        super::probe::classify_codex_auth(self.account)
    }

    /// Returns whether an account is active.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.authentication().is_authenticated()
    }
}

/// One validated inbound app-server envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerEnvelope {
    /// Successful response correlated to a client request.
    Response {
        /// Echoed request identifier.
        id: Value,
        /// Raw result payload.
        result: Value,
    },
    /// Error response correlated to a client request.
    ErrorResponse {
        /// Echoed request identifier.
        id: Value,
    },
    /// Server-initiated request awaiting a response.
    ServerRequest {
        /// Server request identifier.
        id: Value,
        /// Server method name.
        method: String,
    },
    /// Server-initiated notification without an identifier.
    Notification {
        /// Server method name.
        method: String,
    },
}

/// Builds the `initialize` request value for a request identifier.
///
/// Mirrors `make_codex_initialize_request` exactly, including the capability
/// flags and opt-out list.
#[must_use]
pub fn make_initialize_request(id: u64, client: &CodexClientIdentity) -> Value {
    serde_json::json!({
        "id": id,
        "method": "initialize",
        "params": {
            "capabilities": {
                "experimentalApi": false,
                "optOutNotificationMethods": CODEX_OPT_OUT_NOTIFICATION_METHODS,
                "requestAttestation": false,
            },
            "clientInfo": {
                "name": client.name,
                "version": client.version,
            },
        },
    })
}

/// Builds the minimal `account/read` request value.
///
/// Mirrors `make_codex_account_read_request`.
#[must_use]
pub fn make_account_read_request(id: u64) -> Value {
    serde_json::json!({ "id": id, "method": "account/read", "params": {} })
}

/// Decodes one inbound app-server envelope with unambiguous routing.
///
/// A method envelope must carry no `result`/`error`; a response envelope
/// must carry an identifier plus exactly one of `result`/`error`;
/// identifiers are integers or strings.
///
/// # Errors
///
/// Returns [`CodexProbeError::Protocol`] for malformed JSON or ambiguous
/// routing discriminants.
pub fn decode_server_envelope(line: &[u8]) -> Result<ServerEnvelope, CodexProbeError> {
    let value: Value = serde_json::from_slice(line).map_err(|_| CodexProbeError::Protocol)?;
    let object = value.as_object().ok_or(CodexProbeError::Protocol)?;
    match object.get("method") {
        Some(Value::String(method)) if !method.is_empty() => {
            if object.contains_key("result") || object.contains_key("error") {
                return Err(CodexProbeError::Protocol);
            }
            match object.get("id") {
                None => Ok(ServerEnvelope::Notification {
                    method: method.clone(),
                }),
                Some(id) => {
                    check_request_id(id)?;
                    Ok(ServerEnvelope::ServerRequest {
                        id: id.clone(),
                        method: method.clone(),
                    })
                }
            }
        }
        Some(_) => Err(CodexProbeError::Protocol),
        None => {
            let id = object.get("id").ok_or(CodexProbeError::Protocol)?;
            check_request_id(id)?;
            match (object.contains_key("result"), object.contains_key("error")) {
                (true, false) => Ok(ServerEnvelope::Response {
                    id: id.clone(),
                    result: object.get("result").cloned().unwrap_or(Value::Null),
                }),
                (false, true) => Ok(ServerEnvelope::ErrorResponse { id: id.clone() }),
                _ => Err(CodexProbeError::Protocol),
            }
        }
    }
}

fn check_request_id(id: &Value) -> Result<(), CodexProbeError> {
    match id {
        Value::Number(number) if number.is_i64() || number.is_u64() => Ok(()),
        Value::String(_) => Ok(()),
        _ => Err(CodexProbeError::Protocol),
    }
}

/// Validates an `initialize` result into handshake metadata.
///
/// All four fields are required non-empty strings; unknown fields are
/// ignored for forward compatibility.
///
/// # Errors
///
/// Returns [`CodexProbeError::Protocol`] when the result is not an object
/// with the required fields.
pub fn validate_initialize_result(result: &Value) -> Result<CodexServerInfo, CodexProbeError> {
    let object = result.as_object().ok_or(CodexProbeError::Protocol)?;
    let field = |name: &str| -> Result<String, CodexProbeError> {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .ok_or(CodexProbeError::Protocol)
    };
    Ok(CodexServerInfo {
        codex_home: field("codexHome")?,
        platform_family: field("platformFamily")?,
        platform_os: field("platformOs")?,
        user_agent: field("userAgent")?,
    })
}

/// Awaits the successful response for one request identifier.
///
/// Server requests, notifications, and foreign responses are skipped; a
/// matching error response or an exhausted envelope budget fails with
/// [`CodexProbeError::Protocol`]; a closed stream fails with
/// [`CodexProbeError::Unavailable`].
///
/// # Errors
///
/// Returns the first terminal envelope failure, [`CodexProbeError::Protocol`]
/// on budget exhaustion or error responses, and
/// [`CodexProbeError::Unavailable`] when the stream ends first.
pub fn await_response_id(
    envelopes: &mut dyn Iterator<Item = Result<ServerEnvelope, CodexProbeError>>,
    want_id: u64,
    budget: &mut usize,
) -> Result<Value, CodexProbeError> {
    let want = Value::from(want_id);
    loop {
        if *budget == 0 {
            return Err(CodexProbeError::Protocol);
        }
        match envelopes.next() {
            None => return Err(CodexProbeError::Unavailable),
            Some(Err(error)) => return Err(error),
            Some(Ok(envelope)) => {
                *budget -= 1;
                match envelope {
                    ServerEnvelope::Response { id, result } if id == want => return Ok(result),
                    ServerEnvelope::ErrorResponse { id } if id == want => {
                        return Err(CodexProbeError::Protocol);
                    }
                    ServerEnvelope::Response { .. }
                    | ServerEnvelope::ErrorResponse { .. }
                    | ServerEnvelope::ServerRequest { .. }
                    | ServerEnvelope::Notification { .. } => {}
                }
            }
        }
    }
}

/// Input for the bounded app-server account probe.
pub struct CodexSessionProbeInput<'a> {
    /// Resolved Codex executable.
    pub executable: &'a Path,
    /// Extra arguments prepended before `app-server --stdio`.
    pub extra_args: &'a [String],
    /// Optional `CODEX_HOME` for the child; the environment is otherwise inherited.
    pub codex_home: Option<&'a Path>,
    /// Validated client identity.
    pub client: &'a CodexClientIdentity,
    /// End-to-end deadline for spawn, both round trips, and shutdown.
    pub timeout: Duration,
    /// Total byte bound per captured stream.
    pub max_bytes: usize,
    /// Inbound envelope cap per awaited response.
    pub max_envelopes: usize,
}

/// Runs the bounded `initialize` + `account/read` + shutdown probe.
///
/// One end-to-end deadline covers everything; both streams drain
/// concurrently with byte bounds; the child is always shut down (stdin
/// closed, brief exit window, then stopped) and its drain threads joined, so
/// no path hangs or leaks the child.
///
/// # Errors
///
/// Returns [`CodexProbeError::InvalidBinary`] when the child cannot start,
/// [`CodexProbeError::Timeout`] on deadline expiry,
/// [`CodexProbeError::OutputTooLarge`] on stream excess,
/// [`CodexProbeError::Unavailable`] when the child or its streams fail, and
/// [`CodexProbeError::Protocol`] or [`CodexProbeError::AccountInvalid`] for
/// malformed exchanges or envelope excess. Captured bytes are never echoed.
pub fn probe_codex_account(
    input: &CodexSessionProbeInput<'_>,
) -> Result<CodexAccountProbe, CodexProbeError> {
    let deadline = probe_deadline(input.timeout)?;
    let mut argv = input.extra_args.to_vec();
    argv.extend(CODEX_APP_SERVER_ARGS.iter().map(|arg| (*arg).to_owned()));
    let mut child = spawn_probe_child(input.executable, &argv, input.codex_home)?;
    let mut stdout_drain = PipeDrain::spawn_pipe(take_child_stdout(&mut child)?, input.max_bytes);
    let mut stderr_drain = PipeDrain::spawn_pipe(take_child_stderr(&mut child)?, input.max_bytes);
    let mut stdin = child.stdin.take();
    let outcome = drive_session_exchange(
        &mut child,
        &mut stdin,
        &stdout_drain,
        &mut stderr_drain,
        input,
        deadline,
    );
    drop(stdin);
    shutdown_probe_child(&mut child, deadline);
    stdout_drain.join_or_detach();
    stderr_drain.join_or_detach();
    outcome
}

fn drive_session_exchange(
    child: &mut Child,
    stdin: &mut Option<ChildStdin>,
    stdout_drain: &PipeDrain,
    stderr_drain: &mut PipeDrain,
    input: &CodexSessionProbeInput<'_>,
    deadline: Instant,
) -> Result<CodexAccountProbe, CodexProbeError> {
    send_request_line(child, stdin, &make_initialize_request(1, input.client))?;
    let server = {
        let mut live = LiveEnvelopes::new(stdout_drain, stderr_drain, child, deadline);
        let mut budget = input.max_envelopes;
        let result = await_response_id(&mut live, 1, &mut budget)?;
        validate_initialize_result(&result)?
    };
    send_request_line(child, stdin, &make_account_read_request(2))?;
    let account = {
        let mut live = LiveEnvelopes::new(stdout_drain, stderr_drain, child, deadline);
        let mut budget = input.max_envelopes;
        let result = await_response_id(&mut live, 2, &mut budget)?;
        decode_account_value(&result)?
    };
    Ok(CodexAccountProbe { account, server })
}

fn send_request_line(
    child: &mut Child,
    stdin: &mut Option<ChildStdin>,
    request: &Value,
) -> Result<(), CodexProbeError> {
    let line = serde_json::to_string(request).map_err(|_| {
        stop_child(child);
        CodexProbeError::Protocol
    })?;
    let handle = stdin.as_mut().ok_or(CodexProbeError::Unavailable)?;
    writeln!(handle, "{line}").map_err(|_| {
        stop_child(child);
        CodexProbeError::Unavailable
    })
}

fn shutdown_probe_child(child: &mut Child, deadline: Instant) {
    let end = Instant::now()
        .checked_add(SHUTDOWN_GRACE)
        .unwrap_or(deadline)
        .min(deadline);
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => {
                if Instant::now() >= end {
                    break;
                }
                thread::sleep(PROBE_POLL_INTERVAL);
            }
        }
    }
    stop_child(child);
}

/// Live envelope source over a draining child stdout with deadline.
///
/// Blank lines are skipped. Terminal stream conditions surface once as an
/// error item, after which the iterator ends.
struct LiveEnvelopes<'a> {
    stdout: &'a PipeDrain,
    stderr: &'a mut PipeDrain,
    child: &'a mut Child,
    deadline: Instant,
    terminal: bool,
}

impl<'a> LiveEnvelopes<'a> {
    const fn new(
        stdout: &'a PipeDrain,
        stderr: &'a mut PipeDrain,
        child: &'a mut Child,
        deadline: Instant,
    ) -> Self {
        Self {
            stdout,
            stderr,
            child,
            deadline,
            terminal: false,
        }
    }

    fn fail(&mut self, error: CodexProbeError) -> Result<ServerEnvelope, CodexProbeError> {
        self.terminal = true;
        Err(error)
    }
}

impl Iterator for LiveEnvelopes<'_> {
    type Item = Result<ServerEnvelope, CodexProbeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.terminal {
            return None;
        }
        loop {
            while let Ok(event) = self.stderr.events().try_recv() {
                match event {
                    PipeEvent::Line(_) | PipeEvent::Eof => {}
                    PipeEvent::TooLarge => return Some(self.fail(CodexProbeError::OutputTooLarge)),
                    PipeEvent::Io => return Some(self.fail(CodexProbeError::Unavailable)),
                }
            }
            match self.stdout.events().recv_timeout(PROBE_POLL_INTERVAL) {
                Ok(PipeEvent::Line(line)) => {
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    match decode_server_envelope(&line) {
                        Ok(envelope) => return Some(Ok(envelope)),
                        Err(error) => return Some(self.fail(error)),
                    }
                }
                Ok(PipeEvent::Eof | PipeEvent::Io)
                | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Some(self.fail(CodexProbeError::Unavailable));
                }
                Ok(PipeEvent::TooLarge) => return Some(self.fail(CodexProbeError::OutputTooLarge)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if Instant::now() >= self.deadline {
                        return Some(self.fail(CodexProbeError::Timeout));
                    }
                    if self.child.try_wait().is_err() {
                        return Some(self.fail(CodexProbeError::Unavailable));
                    }
                }
            }
        }
    }
}
