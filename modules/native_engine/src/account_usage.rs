//! Bounded non-billable provider-account usage readers.
//!
//! This module owns the process custody shared by the Codex and Claude usage
//! readers: spawning one provider CLI, draining its stdout/stderr
//! concurrently, enforcing byte bounds and deadlines on every path, and
//! always killing plus reaping the child. Provider payloads never leave this
//! module: failures are payload-free categories, and successful reads return
//! only validated domain windows with Artisan-owned authentication state.
//!
//! The wire framing mirrors the TypeScript adapters: Codex speaks
//! newline-delimited JSON-RPC over stdio (`stdio-jsonl`), Claude answers one
//! shot on stdout. `tokio` is deliberately not used here; custody runs on
//! `std` threads so a deadline can always preempt a blocked provider.

use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
#[cfg(not(windows))]
use std::process::Child;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use artisan_domain::{EngineUsageAuth, EngineUsageAuthentication, QuotaSurface};

#[cfg(windows)]
use command_group::CommandGroup as _;

pub use super::account_usage_resolve::{
    CliLaunch, CliResolveInput, resolve_claude_cli, resolve_cli_with, resolve_codex_cli,
};

/// Default per-line byte ceiling for provider stdio frames (1 MiB).
///
/// Measured over line content without its trailing newline; one extra probe
/// byte is read to distinguish a complete maximum-length line from an
/// overlong one.
pub const USAGE_MAX_LINE_BYTES: usize = 1_048_576;
/// Default total byte ceiling for one usage exchange (8 MiB).
pub const USAGE_MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
/// Default ceiling for skipped server-initiated frames per exchange.
pub const USAGE_MAX_SKIPPED_FRAMES: usize = 1_024;
/// Default capacity of the bounded stdout line queue.
pub const USAGE_MAX_QUEUED_LINES: usize = 64;
/// Default grace for each bounded teardown step (child reap, thread join).
pub const USAGE_TEARDOWN_GRACE: Duration = Duration::from_secs(2);
/// Poll interval for bounded teardown waits.
pub const USAGE_TEARDOWN_POLL: Duration = Duration::from_millis(10);

/// Byte and frame bounds for one provider usage exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExchangeBounds {
    /// Maximum bytes accepted for one stdio line, including its newline.
    pub max_line_bytes: usize,
    /// Maximum total stdout bytes accepted for the whole exchange.
    pub max_total_bytes: usize,
    /// Maximum server-initiated frames skipped while awaiting a response.
    pub max_skipped_frames: usize,
}

impl ExchangeBounds {
    /// Returns the documented default bounds.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            max_line_bytes: USAGE_MAX_LINE_BYTES,
            max_total_bytes: USAGE_MAX_TOTAL_BYTES,
            max_skipped_frames: USAGE_MAX_SKIPPED_FRAMES,
        }
    }
}

impl Default for ExchangeBounds {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Payload-free failure of one bounded provider usage read.
///
/// No variant retains provider output, credentials, paths, or process
/// diagnostics; callers map these categories to Artisan-owned reasons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageReaderError {
    /// The provider child could not be spawned.
    Spawn,
    /// The overall deadline elapsed; the child was killed and reaped.
    Timeout,
    /// The provider closed stdio before answering.
    Closed,
    /// A line or the total exchange exceeded its byte bound.
    TooLarge,
    /// Provider output was not valid JSON of the expected shape.
    Malformed,
    /// The provider violated the JSON-RPC routing contract.
    Protocol,
    /// The provider exited unsuccessfully before answering.
    ExitStatus,
    /// The provider answered successfully but reported no usable data.
    Empty,
}

impl fmt::Display for UsageReaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Spawn => "provider usage child could not be spawned",
            Self::Timeout => "provider usage read timed out",
            Self::Closed => "provider usage child closed stdio before answering",
            Self::TooLarge => "provider usage output exceeded its byte bound",
            Self::Malformed => "provider usage output was malformed",
            Self::Protocol => "provider usage child violated its protocol contract",
            Self::ExitStatus => "provider usage child exited unsuccessfully",
            Self::Empty => "provider usage child reported no usable data",
        })
    }
}

impl std::error::Error for UsageReaderError {}

/// Provider-side JSON-RPC error with its message custody contained.
///
/// The message is bounded at construction and never formatted: it exists
/// only so readers can recognize login-gated failures. `Debug` and
/// `Display` are redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct ProviderError {
    message: String,
}

impl ProviderError {
    /// Bounds one provider error message for internal classification.
    #[must_use]
    pub fn new(message: &str) -> Self {
        Self {
            message: message.chars().take(1_024).collect::<String>(),
        }
    }

    /// Returns whether the provider message reports a missing login.
    ///
    /// Mirrors `is_codex_login_error_message`: the patterns are matched
    /// case-insensitively over whitespace-normalized text.
    #[must_use]
    pub fn is_login_error(&self) -> bool {
        let normalized = self
            .message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        const PATTERNS: &[&str] = &[
            "not logged in",
            "login required",
            "log in required",
            "log-in required",
            "please sign in",
            "please sign-in",
            "please log in",
            "please log-in",
            "unauthenticated",
            "not authenticated",
            "no active account",
            "authentication required",
        ];
        PATTERNS.iter().any(|pattern| normalized.contains(pattern))
    }
}

impl fmt::Debug for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderError { <redacted> }")
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("provider request failed")
    }
}

impl std::error::Error for ProviderError {}

/// Failure of one JSON-RPC call: transport custody or a provider error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallError {
    /// Bounded child, time, or framing failure.
    Transport(UsageReaderError),
    /// The provider answered with a JSON-RPC error envelope.
    Provider(ProviderError),
}

impl fmt::Display for CallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "{error}"),
            Self::Provider(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CallError {}

impl From<UsageReaderError> for CallError {
    fn from(error: UsageReaderError) -> Self {
        Self::Transport(error)
    }
}

/// Validated provider-account usage: authentication plus quota windows.
///
/// Readers build this from provider output through the domain constructors,
/// so every value crossing into the backend is already bounded, clamped,
/// and timestamp-validated.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderUsage {
    /// Provider-account authentication state with an Artisan-owned reason.
    pub auth: EngineUsageAuth,
    /// Provider account email, when the transport discloses a valid one.
    pub account_email: Option<String>,
    /// Explicit quota-surface evidence for this read.
    pub quota_surface: QuotaSurface,
    /// Bounded validated quota windows.
    pub windows: Vec<artisan_domain::EngineUsageWindow>,
}

impl ProviderUsage {
    /// Returns an authenticated read with no email.
    #[must_use]
    pub fn authenticated(windows: Vec<artisan_domain::EngineUsageWindow>) -> Self {
        Self {
            auth: EngineUsageAuth::new(EngineUsageAuthentication::Authenticated, None)
                .expect("static auth state is valid"),
            account_email: None,
            quota_surface: QuotaSurface::Supported,
            windows,
        }
    }

    /// Returns an unauthenticated read with an Artisan-owned reason.
    #[must_use]
    pub fn unauthenticated(reason: &'static str) -> Self {
        Self {
            auth: EngineUsageAuth::new(
                EngineUsageAuthentication::Unauthenticated,
                Some(reason.to_owned()),
            )
            .expect("static auth reason is valid"),
            account_email: None,
            quota_surface: QuotaSurface::Supported,
            windows: Vec::new(),
        }
    }
}

/// Process-group custody for one provider child.
///
/// Windows holds a `command-group` Job Object exactly like the owned
/// engine-owner launches: killing it terminates the whole descendant tree
/// and closes inherited pipes, so drain threads always observe EOF and no
/// reader can stay blocked past teardown. Other platforms hold the direct
/// child, matching the existing owned custody split.
pub(crate) struct ChildCustody {
    #[cfg(windows)]
    grouped: command_group::GroupChild,
    #[cfg(not(windows))]
    direct: Child,
}

impl ChildCustody {
    pub(crate) fn spawn(command: &mut Command) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let grouped = command
                .group()
                .kill_on_drop(true)
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()?;
            Ok(Self { grouped })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {
                direct: command.spawn()?,
            })
        }
    }

    pub(crate) fn take_pipes(
        &mut self,
    ) -> (Option<ChildStdin>, Option<ChildStdout>, Option<ChildStderr>) {
        #[cfg(windows)]
        {
            let inner = self.grouped.inner();
            (inner.stdin.take(), inner.stdout.take(), inner.stderr.take())
        }
        #[cfg(not(windows))]
        {
            (
                self.direct.stdin.take(),
                self.direct.stdout.take(),
                self.direct.stderr.take(),
            )
        }
    }

    pub(crate) fn kill(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            self.grouped.kill()
        }
        #[cfg(not(windows))]
        {
            self.direct.kill()
        }
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        #[cfg(windows)]
        {
            self.grouped.try_wait()
        }
        #[cfg(not(windows))]
        {
            self.direct.try_wait()
        }
    }
}

/// Waits for one provider child to exit within a bounded grace.
///
/// A killed direct child (or, on Windows, a killed job tree) reaps
/// promptly; the poll loop only bounds the pathological case. Errors from
/// `try_wait` end the wait: there is no handle left worth blocking on.
pub(crate) fn wait_child_bounded(child: &mut ChildCustody, grace: Duration) {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => {
                if Instant::now() >= deadline {
                    return;
                }
                thread::sleep(USAGE_TEARDOWN_POLL);
            }
        }
    }
}

/// Joins one drain thread within a bounded grace.
///
/// A finished thread is joined and its panic (a fixture bug, never provider
/// data) is discarded. An unfinished thread is detached by dropping its
/// handle; teardown already killed the process tree and dropped the queue
/// receiver, so the thread exits on pipe EOF or channel disconnect and only
/// a failed group kill could linger it, bounded by the descendant's own
/// lifetime and never blocking the caller.
pub(crate) fn join_thread_bounded(handle: JoinHandle<()>, grace: Duration) {
    let deadline = Instant::now() + grace;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return;
        }
        thread::sleep(USAGE_TEARDOWN_POLL);
    }
    let _join_result = handle.join();
}

enum LineEvent {
    Line(String),
    Oversize,
    Finished,
}

/// One bounded newline-delimited JSON-RPC session over a provider child.
///
/// Stdout is drained on a dedicated thread into a bounded queue and stderr
/// on another, both concurrent with the caller's writes. The queue applies
/// backpressure instead of dropping frames: a full queue parks the reader
/// until the caller consumes, and teardown drops the receiver first so a
/// parked sender wakes instead of deadlocking. Teardown
/// ([`JsonRpcSession::shutdown`], also run from [`Drop`]) closes stdin,
/// drops the receiver, kills the whole process tree, reaps it within a
/// grace, and joins both drains within the same grace: no step blocks past
/// its bound, even when a descendant holds inherited pipes open.
pub struct JsonRpcSession {
    stdin: Option<ChildStdin>,
    lines: Option<Receiver<LineEvent>>,
    custody: Option<ChildCustody>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_drain: Option<JoinHandle<()>>,
    next_id: u64,
    total_bytes: usize,
    skipped_frames: usize,
    bounds: ExchangeBounds,
    teardown_grace: Duration,
    torn_down: bool,
}

impl JsonRpcSession {
    /// Spawns one provider child with piped stdio and starts both drains.
    ///
    /// # Errors
    ///
    /// Returns [`UsageReaderError::Spawn`] when the child cannot be started.
    pub fn spawn(
        executable: &Path,
        args: &[String],
        env: &[(String, String)],
        bounds: ExchangeBounds,
    ) -> Result<Self, UsageReaderError> {
        let mut command = Command::new(executable);
        command
            .args(args)
            .envs(env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut custody = ChildCustody::spawn(&mut command).map_err(|_| UsageReaderError::Spawn)?;
        let (stdin, stdout, stderr) = custody.take_pipes();
        let (sender, lines) = mpsc::sync_channel(USAGE_MAX_QUEUED_LINES);
        let max_line_bytes = bounds.max_line_bytes;
        let mut stdout_reader = None;
        if let Some(stdout) = stdout {
            match thread::Builder::new()
                .name("usage-stdout-drain".to_owned())
                .spawn(move || drain_stdout_lines(stdout, sender, max_line_bytes))
            {
                Ok(handle) => stdout_reader = Some(handle),
                Err(_) => {
                    let _kill_result = custody.kill();
                    wait_child_bounded(&mut custody, USAGE_TEARDOWN_GRACE);
                    return Err(UsageReaderError::Spawn);
                }
            }
        }
        let mut stderr_drain = None;
        if let Some(stderr) = stderr {
            match thread::Builder::new()
                .name("usage-stderr-drain".to_owned())
                .spawn(move || drain_to_void(stderr))
            {
                Ok(handle) => stderr_drain = Some(handle),
                Err(_) => {
                    let _kill_result = custody.kill();
                    wait_child_bounded(&mut custody, USAGE_TEARDOWN_GRACE);
                    if let Some(reader) = stdout_reader.take() {
                        join_thread_bounded(reader, USAGE_TEARDOWN_GRACE);
                    }
                    return Err(UsageReaderError::Spawn);
                }
            }
        }
        Ok(Self {
            stdin,
            lines: Some(lines),
            custody: Some(custody),
            stdout_reader,
            stderr_drain,
            next_id: 1,
            total_bytes: 0,
            skipped_frames: 0,
            bounds,
            teardown_grace: USAGE_TEARDOWN_GRACE,
            torn_down: false,
        })
    }

    /// Tears the session down within a bounded grace: closes stdin, drops
    /// the line receiver so a backpressured sender wakes, kills the whole
    /// process tree, reaps it, and joins both drains. Idempotent. Every step
    /// carries its own deadline, so cleanup returns even when a descendant
    /// holds inherited pipes past the grace; killing the tree closes those
    /// pipes, which lets the drains observe EOF and exit.
    pub fn shutdown(&mut self) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        drop(self.stdin.take());
        drop(self.lines.take());
        if let Some(mut custody) = self.custody.take() {
            let _kill_result = custody.kill();
            wait_child_bounded(&mut custody, self.teardown_grace);
        }
        if let Some(reader) = self.stdout_reader.take() {
            join_thread_bounded(reader, self.teardown_grace);
        }
        if let Some(drain) = self.stderr_drain.take() {
            join_thread_bounded(drain, self.teardown_grace);
        }
    }

    /// Sends one fire-and-forget JSON-RPC notification line.
    ///
    /// # Errors
    ///
    /// Returns [`UsageReaderError::Closed`] when stdin is gone.
    pub fn notify(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), UsageReaderError> {
        let line = serde_json::json!({"method": method, "params": params}).to_string() + "\n";
        self.write_line(line.as_bytes())
    }

    /// Sends one JSON-RPC request and awaits the matching response id.
    ///
    /// Server-initiated frames and unknown ids are skipped within the frame
    /// bound. Provider error envelopes return [`CallError::Provider`] with
    /// the message retained only for login classification.
    ///
    /// # Errors
    ///
    /// Returns [`CallError::Transport`] for custody, bound, framing, or
    /// deadline failures, or [`CallError::Provider`] for a provider error
    /// envelope.
    pub fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
        deadline: Instant,
    ) -> Result<serde_json::Value, CallError> {
        if self.torn_down {
            return Err(CallError::Transport(UsageReaderError::Closed));
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let line =
            serde_json::json!({"id": id, "method": method, "params": params}).to_string() + "\n";
        self.write_line(line.as_bytes())?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CallError::Transport(UsageReaderError::Timeout));
            }
            let received = self
                .lines
                .as_ref()
                .ok_or(CallError::Transport(UsageReaderError::Closed))?
                .recv_timeout(remaining);
            match received {
                Err(RecvTimeoutError::Timeout) => {
                    return Err(CallError::Transport(UsageReaderError::Timeout));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(CallError::Transport(UsageReaderError::Closed));
                }
                Ok(LineEvent::Oversize) => {
                    return Err(CallError::Transport(UsageReaderError::TooLarge));
                }
                Ok(LineEvent::Finished) => {
                    return Err(CallError::Transport(UsageReaderError::Closed));
                }
                Ok(LineEvent::Line(line)) => {
                    self.total_bytes = self.total_bytes.saturating_add(line.len());
                    if self.total_bytes > self.bounds.max_total_bytes {
                        return Err(CallError::Transport(UsageReaderError::TooLarge));
                    }
                    if let Some(response) = self.route_line(&line, id)? {
                        return response;
                    }
                }
            }
        }
    }

    fn write_line(&mut self, bytes: &[u8]) -> Result<(), UsageReaderError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(UsageReaderError::Closed);
        };
        stdin
            .write_all(bytes)
            .and_then(|()| stdin.flush())
            .map_err(|_| UsageReaderError::Closed)
    }

    fn route_line(
        &mut self,
        line: &str,
        awaited: u64,
    ) -> Result<Option<Result<serde_json::Value, CallError>>, CallError> {
        // Non-JSON lines are skipped within the frame bound rather than
        // rejected: real CLIs print version banners to stdout, and the same
        // tolerance keeps fixture harnesses honest. Valid JSON with the wrong
        // shape still fails as malformed below, and unbounded chatter fails
        // as a protocol violation.
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                self.skip_frame()?;
                return Ok(None);
            }
        };
        let object = value
            .as_object()
            .ok_or(CallError::Transport(UsageReaderError::Malformed))?;
        if object.contains_key("method") {
            self.skip_frame()?;
            return Ok(None);
        }
        let matches = match object.get("id") {
            Some(serde_json::Value::Number(id)) => id.as_u64() == Some(awaited),
            Some(serde_json::Value::String(id)) => id == &awaited.to_string(),
            _ => false,
        };
        if !matches {
            self.skip_frame()?;
            return Ok(None);
        }
        if let Some(error) = object.get("error") {
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            return Ok(Some(Err(CallError::Provider(ProviderError::new(message)))));
        }
        match object.get("result") {
            Some(result) => Ok(Some(Ok(result.clone()))),
            None => Err(CallError::Transport(UsageReaderError::Malformed)),
        }
    }

    fn skip_frame(&mut self) -> Result<(), CallError> {
        self.skipped_frames = self.skipped_frames.saturating_add(1);
        if self.skipped_frames > self.bounds.max_skipped_frames {
            return Err(CallError::Transport(UsageReaderError::Protocol));
        }
        Ok(())
    }
}

impl Drop for JsonRpcSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn drain_stdout_lines(
    stdout: ChildStdout,
    sender: mpsc::SyncSender<LineEvent>,
    max_line_bytes: usize,
) {
    let mut reader = BufReader::new(stdout);
    // `take` bounds every read to one line plus a probe margin: a no-newline
    // flood can never grow this allocation past the bound, and the overlong
    // remainder stays in the pipe for tree-kill teardown. Content is measured
    // without its single trailing newline, so a maximum-length line still
    // fits; an EOF-terminated tail without a newline counts fully.
    let limit = max_line_bytes.saturating_add(2) as u64;
    loop {
        let mut line = Vec::new();
        match reader.by_ref().take(limit).read_until(b'\n', &mut line) {
            Ok(0) => {
                let _send_result = sender.send(LineEvent::Finished);
                break;
            }
            Ok(_) => {
                let complete = line.ends_with(b"\n");
                let content_len = line.len() - usize::from(complete);
                if content_len > max_line_bytes {
                    let _send_result = sender.send(LineEvent::Oversize);
                    break;
                }
                let event = match String::from_utf8(line) {
                    Ok(text) => LineEvent::Line(text),
                    Err(_) => LineEvent::Oversize,
                };
                // Backpressure, never silent drops: a full queue parks the
                // reader until the caller consumes, and teardown drops the
                // receiver first so a parked sender wakes instead of
                // deadlocking. Every received line still counts downstream.
                let oversize = matches!(event, LineEvent::Oversize);
                if sender.send(event).is_err() {
                    break;
                }
                if oversize {
                    break;
                }
            }
            Err(_) => {
                let _send_result = sender.send(LineEvent::Finished);
                break;
            }
        }
    }
}

fn drain_to_void(stderr: std::process::ChildStderr) {
    let mut reader = BufReader::new(stderr);
    let mut chunk = [0_u8; 8_192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_patterns_match_without_retaining_provider_text() {
        let error = ProviderError::new(
            "Not logged in: please run `codex login` first, this message is deliberately long and must never be formatted anywhere in diagnostics or reports.",
        );
        assert!(error.is_login_error());
        assert_eq!(error.to_string(), "provider request failed");
        assert_eq!(format!("{error:?}"), "ProviderError { <redacted> }");
        assert!(!format!("{error:?}").contains("logged in"));

        let other = ProviderError::new("rateLimits/read timed out");
        assert!(!other.is_login_error());
        let expired = ProviderError::new("CURSOR SIGN-IN IS NO LONGER VALID");
        assert!(!expired.is_login_error());
    }

    #[test]
    fn reader_errors_and_provider_usage_constructors_are_bounded() {
        assert_eq!(
            UsageReaderError::TooLarge.to_string(),
            "provider usage output exceeded its byte bound"
        );
        let usage = ProviderUsage::unauthenticated("Sign in to Cursor from Settings.");
        assert_eq!(
            usage.auth.state(),
            EngineUsageAuthentication::Unauthenticated
        );
        assert!(usage.windows.is_empty());
        assert_eq!(usage.quota_surface, QuotaSurface::Supported);
    }
}
