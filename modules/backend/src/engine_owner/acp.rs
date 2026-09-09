//! Finite ACP shared-transport core A1: stdio lifecycle plus framing.
//!
//! This leaf owns the native JSON-RPC-over-stdio transport spoken directly on
//! the wire: piped-`stdio` spawn with no shell, a bounded `initialize`
//! handshake with timeout, `session/new` and `session/load`, the streaming
//! update-loop framing with byte and line bounds, `cancel`/`close` teardown
//! with kill-then-reap and quarantine on failure, exit classification
//! (`interruption` vs `failure` vs `cancel`), and an inactivity deadline
//! measured after the last received frame. Every bound is caller-supplied
//! through [`AcpBounds`]; nothing here hardcodes a transport tuning value.
//!
//! The core is called from per-engine dispatch arms and adds no tasks: one
//! `EngineOwner` task and queue stays the authority for admission, custody,
//! and quarantine. Spawning mirrors the `process.rs` custody contract
//! (piped stdio, `kill_on_drop`, whole-job termination on Windows, close
//! lifeline first, bounded wait, `start_kill`, observed reap or retained
//! custody) without forking its OpenCode-specific recipe or types.
//!
//! Envelopes are typed at the boundary ([`AcpEnvelope`]); provider error text
//! is never retained (only the numeric code), update payloads cross as the
//! explicitly redacted [`WireUpdate`] boundary type for the A2 bridges to
//! interpret, and image payloads pass through as typed [`PromptPart`] blocks
//! per the row's [`ImageMode`], never as file paths. The TypeScript
//! `@agentclientprotocol/sdk` is behavior evidence only; this transport
//! speaks the wire directly and takes no SDK dependency.
//!
//! Explicit non-goals for A1: permission/elicitation bridges (A2 packet),
//! probe harness (A3), dispatcher admission, catalog, frontend, and any
//! engine beyond the grok and cursor rows ([`GROK_ACP`], [`CURSOR_ACP`]).
//!
//! `GrokSettings`/`CursorSettings` do not exist yet, so the row arg builders
//! take the explicit [`LaunchArgs`] params mirroring `GrokAcpArgs` and
//! `CursorAcpArgs` from the TypeScript evidence.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]
// A1 has no in-crate callers yet: per-engine dispatch arms land in a later
// packet. Every item below is covered by the inline fixture-agent tests.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};
use tokio::time::{Instant, timeout, timeout_at};

#[cfg(windows)]
use command_group::AsyncCommandGroup;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Wire protocol version this core accepts, mirroring the TypeScript
/// `Acp.PROTOCOL_VERSION` evidence. The A3 probe harness re-validates this
/// against the pinned SDK before any dispatcher trusts it.
pub(crate) const ACP_PROTOCOL_VERSION: u32 = 1;

/// Client identity sent in `initialize` params, mirroring the TypeScript
/// evidence. A later packet should source the version from crate metadata.
pub(crate) const ACP_CLIENT_NAME: &str = "Artisan Editor";
pub(crate) const ACP_CLIENT_VERSION: &str = "0.2.2";

pub(crate) const METHOD_INITIALIZE: &str = "initialize";
pub(crate) const METHOD_AUTHENTICATE: &str = "authenticate";
pub(crate) const METHOD_SESSION_NEW: &str = "session/new";
pub(crate) const METHOD_SESSION_LOAD: &str = "session/load";
pub(crate) const METHOD_SESSION_PROMPT: &str = "session/prompt";
pub(crate) const METHOD_SESSION_CANCEL: &str = "session/cancel";
pub(crate) const METHOD_SESSION_UPDATE: &str = "session/update";

// ---------------------------------------------------------------------------
// Caller-supplied bounds
// ---------------------------------------------------------------------------

/// Raw ACP transport bounds.
///
/// All fields are caller-supplied literals, validated only through
/// [`AcpBounds::new`]. Zero sizes are rejected; zero durations are allowed
/// and expire before the operation they bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AcpBounds {
    pub(crate) max_line_bytes: usize,
    pub(crate) max_envelope_bytes: usize,
    pub(crate) max_session_id_bytes: usize,
    pub(crate) handshake_timeout: Duration,
    pub(crate) inactivity_timeout: Duration,
    pub(crate) close_budget: Duration,
}

/// Typed validation failure for [`AcpBounds::new`].
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum AcpBoundsError {
    /// A byte bound was zero.
    #[error("acp bound {field} must be greater than zero")]
    ZeroBound { field: &'static str },
    /// A duration cannot be represented as a future instant.
    #[error("acp duration for {field} is not representable as a future instant")]
    UnrepresentableDuration { field: &'static str },
}

impl AcpBounds {
    /// Validates and constructs the transport bounds.
    ///
    /// # Errors
    ///
    /// Returns [`AcpBoundsError::ZeroBound`] when any byte bound is zero, or
    /// [`AcpBoundsError::UnrepresentableDuration`] when any duration cannot
    /// be represented as a future instant.
    #[must_use = "validated bounds must be stored by the caller"]
    pub(crate) fn new(
        max_line_bytes: usize,
        max_envelope_bytes: usize,
        max_session_id_bytes: usize,
        handshake_timeout: Duration,
        inactivity_timeout: Duration,
        close_budget: Duration,
    ) -> Result<Self, AcpBoundsError> {
        if max_line_bytes == 0 {
            return Err(AcpBoundsError::ZeroBound {
                field: "max_line_bytes",
            });
        }
        if max_envelope_bytes == 0 {
            return Err(AcpBoundsError::ZeroBound {
                field: "max_envelope_bytes",
            });
        }
        if max_session_id_bytes == 0 {
            return Err(AcpBoundsError::ZeroBound {
                field: "max_session_id_bytes",
            });
        }
        let reference = Instant::now();
        for (duration, field) in [
            (handshake_timeout, "handshake_timeout"),
            (inactivity_timeout, "inactivity_timeout"),
            (close_budget, "close_budget"),
        ] {
            if reference.checked_add(duration).is_none() {
                return Err(AcpBoundsError::UnrepresentableDuration { field });
            }
        }
        Ok(Self {
            max_line_bytes,
            max_envelope_bytes,
            max_session_id_bytes,
            handshake_timeout,
            inactivity_timeout,
            close_budget,
        })
    }
}

// ---------------------------------------------------------------------------
// Payload-free errors
// ---------------------------------------------------------------------------

/// Typed, payload-free ACP transport failure.
///
/// `Debug` and `Display` are constant strings; no frame, prompt, session,
/// or credential bytes are embedded.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum AcpError {
    /// The child process could not be spawned. The dispatch arms map the raw
    /// spawn failure to this cause.
    #[error("acp spawn failed")]
    SpawnFailed,
    /// The handshake did not complete before its deadline.
    #[error("acp handshake timed out")]
    HandshakeTimeout,
    /// The agent answered the handshake with an error.
    #[error("acp handshake rejected")]
    HandshakeRejected,
    /// The agent reported an unsupported protocol version.
    #[error("acp protocol version unsupported")]
    UnsupportedVersion,
    /// No usable authentication method was offered.
    #[error("acp authentication unavailable")]
    AuthUnavailable,
    /// One stdio line exceeded the caller-supplied line bound.
    #[error("acp line exceeded the configured limit")]
    LineTooLong,
    /// One serialized envelope exceeded the caller-supplied envelope bound.
    #[error("acp envelope exceeded the configured limit")]
    EnvelopeTooLarge,
    /// A complete stdio line was not valid UTF-8.
    #[error("acp bytes were not valid utf-8")]
    InvalidUtf8,
    /// A complete line was not a well-formed JSON-RPC 2.0 envelope.
    #[error("acp envelope malformed")]
    MalformedEnvelope,
    /// A deadline could not be represented as a future instant.
    #[error("acp deadline is unrepresentable")]
    UnrepresentableDeadline,
    /// The peer closed stdout; the transport is done.
    #[error("acp peer closed the stream")]
    PeerClosed,
    /// No frame arrived before the inactivity deadline.
    #[error("acp inactivity deadline exceeded")]
    InactivityStall,
    /// The operation was cancelled by the caller. The dispatch arms map
    /// their cancellation signal to this cause.
    #[error("acp operation cancelled")]
    Cancelled,
    /// The agent answered a session operation with an error, or exited badly.
    #[error("acp child failed")]
    ChildFailed,
    /// Prompt content could not be represented for this engine row.
    #[error("acp prompt content is invalid")]
    InvalidContent,
    /// A transport I/O operation failed.
    #[error("acp transport io failed")]
    IoFailed,
    /// The framer already failed and refuses further input.
    #[error("acp framer is poisoned")]
    Poisoned,
}

// ---------------------------------------------------------------------------
// Byte-oriented NDJSON framer
// ---------------------------------------------------------------------------

/// Incremental byte-oriented NDJSON framer with a caller-supplied line bound.
///
/// Accepts arbitrarily fragmented chunks without converting incomplete
/// UTF-8, recognizes LF lines with one optional CR trimmed, validates UTF-8
/// only after a complete bounded line, and poisons deterministically after
/// the first hard error. Mirrors the `SseFramer` accumulation contract for
/// the ACP newline-delimited wire.
pub(crate) struct AcpFramer {
    max_line: usize,
    pending: Vec<u8>,
    poisoned: bool,
    finished: bool,
}

impl AcpFramer {
    /// Creates a framer bounded by the caller-supplied `max_line_bytes`.
    #[must_use = "framers must be driven by the caller"]
    pub(crate) fn new(max_line: usize) -> Self {
        Self {
            max_line,
            pending: Vec::new(),
            poisoned: false,
            finished: false,
        }
    }

    /// Feeds an arbitrarily fragmented chunk, returning complete lines.
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::Poisoned`] after a previous failure or `finish`,
    /// [`AcpError::LineTooLong`] when pending bytes cross the bound, or
    /// [`AcpError::InvalidUtf8`] for a complete non-UTF-8 line.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, AcpError> {
        if self.poisoned {
            return Err(AcpError::Poisoned);
        }
        if self.finished {
            self.poisoned = true;
            return Err(AcpError::Poisoned);
        }
        let mut out = Vec::new();
        let mut cursor = 0_usize;
        while cursor < chunk.len() {
            if let Some(relative) = chunk[cursor..].iter().position(|byte| *byte == b'\n') {
                let total = self
                    .pending
                    .len()
                    .checked_add(relative)
                    .ok_or(AcpError::LineTooLong)?;
                if total > self.max_line {
                    self.poisoned = true;
                    return Err(AcpError::LineTooLong);
                }
                let mut line_bytes = Vec::with_capacity(total);
                line_bytes.extend_from_slice(&self.pending);
                line_bytes.extend_from_slice(&chunk[cursor..cursor + relative]);
                self.pending.clear();
                if line_bytes.last() == Some(&b'\r') {
                    line_bytes.pop();
                }
                match String::from_utf8(line_bytes) {
                    Ok(line) => out.push(line),
                    Err(_) => {
                        self.poisoned = true;
                        return Err(AcpError::InvalidUtf8);
                    }
                }
                cursor += relative + 1;
            } else {
                let remaining = chunk.len() - cursor;
                let total = self
                    .pending
                    .len()
                    .checked_add(remaining)
                    .ok_or(AcpError::LineTooLong)?;
                if total > self.max_line {
                    self.poisoned = true;
                    return Err(AcpError::LineTooLong);
                }
                self.pending.extend_from_slice(&chunk[cursor..]);
                break;
            }
        }
        Ok(out)
    }

    /// Discards any incomplete pending line. Further `push` calls are
    /// poisoned; EOF never equates to a terminal observation.
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::Poisoned`] when already finished or poisoned.
    pub(crate) fn finish(&mut self) -> Result<(), AcpError> {
        if self.poisoned || self.finished {
            return Err(AcpError::Poisoned);
        }
        self.pending.clear();
        self.finished = true;
        Ok(())
    }
}

impl fmt::Debug for AcpFramer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcpFramer")
            .field("max_line", &self.max_line)
            .field("poisoned", &self.poisoned)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Typed JSON-RPC envelopes at the boundary
// ---------------------------------------------------------------------------

/// A bounded JSON-RPC 2.0 message identifier: integer or non-empty text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AcpId {
    Number(i64),
    Text(String),
}

/// The result half of a JSON-RPC response.
///
/// Provider error text is deliberately dropped; only the numeric code is
/// retained, keeping provenance sanitized at the boundary.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AcpResponsePayload {
    Result(Value),
    Error { code: i64 },
}

/// One typed JSON-RPC 2.0 envelope. Arbitrary `params`/`result` payloads
/// stay inside the typed envelope until a later packet interprets them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AcpEnvelope {
    Request {
        id: AcpId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: AcpId,
        payload: AcpResponsePayload,
    },
}

fn parse_envelope_id(value: Option<&Value>) -> Result<Option<AcpId>, AcpError> {
    match value {
        None => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(AcpId::Number)
            .map(Some)
            .ok_or(AcpError::MalformedEnvelope),
        Some(Value::String(text)) => {
            if text.is_empty() {
                return Err(AcpError::MalformedEnvelope);
            }
            Ok(Some(AcpId::Text(text.clone())))
        }
        Some(_) => Err(AcpError::MalformedEnvelope),
    }
}

/// Parses one complete line into a typed envelope.
///
/// The byte bound is enforced before parsing so no unbounded allocation can
/// precede rejection. Strict shape rules apply: `jsonrpc` must be exactly
/// `"2.0"`, requests carry an id plus a non-empty method and no
/// result/error, notifications carry a method with no id and no
/// result/error, and responses carry an id with exactly one of
/// result/error.
///
/// # Errors
///
/// Returns [`AcpError::EnvelopeTooLarge`] when the line crosses
/// `max_envelope_bytes`, or [`AcpError::MalformedEnvelope`] for any shape
/// violation.
pub(crate) fn parse_envelope(
    line: &str,
    max_envelope_bytes: usize,
) -> Result<AcpEnvelope, AcpError> {
    if line.len() > max_envelope_bytes {
        return Err(AcpError::EnvelopeTooLarge);
    }
    let value: Value = serde_json::from_str(line).map_err(|_| AcpError::MalformedEnvelope)?;
    let obj = value.as_object().ok_or(AcpError::MalformedEnvelope)?;
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(AcpError::MalformedEnvelope);
    }
    let id = parse_envelope_id(obj.get("id"))?;
    let method = match obj.get("method") {
        None => None,
        Some(Value::String(method)) => {
            if method.is_empty() {
                return Err(AcpError::MalformedEnvelope);
            }
            Some(method.clone())
        }
        Some(_) => return Err(AcpError::MalformedEnvelope),
    };
    let has_result = obj.contains_key("result");
    let has_error = obj.contains_key("error");
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    match (id, method, has_result, has_error) {
        (Some(id), Some(method), false, false) => Ok(AcpEnvelope::Request { id, method, params }),
        (None, Some(method), false, false) => Ok(AcpEnvelope::Notification { method, params }),
        (Some(id), None, true, false) => Ok(AcpEnvelope::Response {
            id,
            payload: AcpResponsePayload::Result(obj["result"].clone()),
        }),
        (Some(id), None, false, true) => {
            let code = obj
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_i64)
                .ok_or(AcpError::MalformedEnvelope)?;
            Ok(AcpEnvelope::Response {
                id,
                payload: AcpResponsePayload::Error { code },
            })
        }
        _ => Err(AcpError::MalformedEnvelope),
    }
}

// ---------------------------------------------------------------------------
// Typed handshake and session values
// ---------------------------------------------------------------------------

/// The validated `initialize` result: wire protocol version plus the offered
/// authentication method identities. The caller selects a method through its
/// engine row; this core never reads ambient credentials itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InitializeResult {
    pub(crate) protocol_version: u32,
    pub(crate) auth_methods: Vec<String>,
}

/// Parses an `initialize` result value. Array entries must be objects with a
/// non-empty string `id`; a missing `authMethods` means no methods, mirroring
/// the TypeScript `?? []` evidence. Total size stays bounded by the envelope
/// bound checked before parsing.
///
/// # Errors
///
/// Returns [`AcpError::MalformedEnvelope`] for any shape violation.
pub(crate) fn parse_initialize_result(result: &Value) -> Result<InitializeResult, AcpError> {
    let obj = result.as_object().ok_or(AcpError::MalformedEnvelope)?;
    let version_number = obj
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .ok_or(AcpError::MalformedEnvelope)?;
    let protocol_version =
        u32::try_from(version_number).map_err(|_| AcpError::MalformedEnvelope)?;
    let mut auth_methods = Vec::new();
    if let Some(methods) = obj.get("authMethods") {
        let items = methods.as_array().ok_or(AcpError::MalformedEnvelope)?;
        for item in items {
            let method_id = item
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or(AcpError::MalformedEnvelope)?;
            auth_methods.push(method_id.to_owned());
        }
    }
    Ok(InitializeResult {
        protocol_version,
        auth_methods,
    })
}

/// A validated provider session identity.
///
/// Rules mirror the owner continuation identity: non-empty, at most
/// `max_session_id_bytes`, and free of whitespace, controls, `/`, `?`,
/// `#`, and `%`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionId(String);

impl SessionId {
    /// Validates a session identity against the caller-supplied byte bound.
    ///
    /// # Errors
    ///
    /// Returns [`AcpError::MalformedEnvelope`] when the identity is empty,
    /// too long, or carries forbidden characters.
    pub(crate) fn parse(value: &str, max_bytes: usize) -> Result<Self, AcpError> {
        if value.is_empty() || value.len() > max_bytes {
            return Err(AcpError::MalformedEnvelope);
        }
        if value.contains('/')
            || value.contains('?')
            || value.contains('#')
            || value.contains('%')
            || value.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(AcpError::MalformedEnvelope);
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the validated identity text.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// An opaque session-update payload at the A1 boundary.
///
/// The value stays typed-as-envelope until the A2 bridges interpret update
/// kinds; `Debug` is redacted so provider content never lands in logs.
#[derive(Clone, PartialEq)]
pub(crate) struct WireUpdate(Value);

impl WireUpdate {
    /// Returns the opaque update value for A2 interpretation.
    #[must_use]
    pub(crate) fn value(&self) -> &Value {
        &self.0
    }
}

impl fmt::Debug for WireUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WireUpdate { <redacted> }")
    }
}

/// One validated `session/update` notification for the expected session.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionUpdate {
    pub(crate) session: SessionId,
    pub(crate) update: WireUpdate,
}

/// Parses a `session/update` notification's params.
///
/// A params `sessionId` that does not match the expected session yields
/// `Ok(None)` so the update loop keeps waiting, mirroring the TypeScript
/// early-return evidence for foreign session frames.
///
/// # Errors
///
/// Returns [`AcpError::MalformedEnvelope`] for structural violations.
pub(crate) fn parse_session_update(
    params: &Value,
    expected: &SessionId,
    max_session_id_bytes: usize,
) -> Result<Option<SessionUpdate>, AcpError> {
    let obj = params.as_object().ok_or(AcpError::MalformedEnvelope)?;
    let session_id = obj
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or(AcpError::MalformedEnvelope)?;
    if session_id != expected.as_str() {
        return Ok(None);
    }
    let session = SessionId::parse(session_id, max_session_id_bytes)?;
    let update = obj.get("update").cloned().unwrap_or(Value::Null);
    Ok(Some(SessionUpdate {
        session,
        update: WireUpdate(update),
    }))
}

/// Token usage reported with a prompt result, in provider-native units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TokenUsage {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cached_input: Option<u64>,
}

/// The typed outcome of one prompt round: cancellation plus optional usage.
/// A `null` or missing `usage` means the agent reported none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PromptOutcome {
    pub(crate) cancelled: bool,
    pub(crate) usage: Option<TokenUsage>,
}

/// Parses a prompt response payload. Only the literal `"cancelled"`
/// stop reason counts as cancellation; other or missing reasons complete.
/// Usage counters default to zero when absent and must be `u64` when
/// present.
///
/// # Errors
///
/// Returns [`AcpError::ChildFailed`] for an error payload, or
/// [`AcpError::MalformedEnvelope`] for shape violations.
pub(crate) fn parse_prompt_result(payload: &AcpResponsePayload) -> Result<PromptOutcome, AcpError> {
    let result = match payload {
        AcpResponsePayload::Error { .. } => return Err(AcpError::ChildFailed),
        AcpResponsePayload::Result(result) => result,
    };
    let obj = result.as_object().ok_or(AcpError::MalformedEnvelope)?;
    let cancelled = match obj.get("stopReason") {
        None => false,
        Some(Value::String(reason)) => reason == "cancelled",
        Some(_) => return Err(AcpError::MalformedEnvelope),
    };
    let usage = match obj.get("usage") {
        None | Some(Value::Null) => None,
        Some(Value::Object(counters)) => {
            let counter = |name: &str| -> Result<u64, AcpError> {
                match counters.get(name) {
                    None | Some(Value::Null) => Ok(0),
                    Some(value) => value.as_u64().ok_or(AcpError::MalformedEnvelope),
                }
            };
            let cached = match counters.get("cachedReadTokens") {
                None | Some(Value::Null) => None,
                Some(value) => Some(value.as_u64().ok_or(AcpError::MalformedEnvelope)?),
            };
            Some(TokenUsage {
                input: counter("inputTokens")?,
                output: counter("outputTokens")?,
                cached_input: cached,
            })
        }
        Some(_) => return Err(AcpError::MalformedEnvelope),
    };
    Ok(PromptOutcome { cancelled, usage })
}

// ---------------------------------------------------------------------------
// Prompt content: typed blocks per row image mode, never file paths
// ---------------------------------------------------------------------------

/// How one engine row carries image payloads, mirroring the TypeScript
/// `image_input` evidence: embedded resource blocks or native image blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageMode {
    Embedded,
    Image,
}

/// One typed image payload passing through the transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageBlock {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) media_type: String,
    pub(crate) bytes: Vec<u8>,
}

/// One typed prompt part: plain text or an image block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptPart {
    Text(String),
    Image(ImageBlock),
}

fn percent_encode(segment: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[usize::from(byte >> 4)] as char);
            encoded.push(HEX[usize::from(byte & 0x0F)] as char);
        }
    }
    encoded
}

/// Builds the `session/prompt` content array, mirroring the TypeScript
/// `prompt_content` evidence: optional product-instructions wrapper first,
/// text parts inline, and image parts per the row's image mode (embedded
/// `artisan://attachment` resource vs native image block with base64 data).
///
/// # Errors
///
/// Returns [`AcpError::InvalidContent`] for empty image metadata.
pub(crate) fn build_prompt_content(
    mode: ImageMode,
    text: &str,
    parts: &[PromptPart],
    product_instructions: Option<&str>,
) -> Result<Vec<Value>, AcpError> {
    let mut content = Vec::new();
    if let Some(instructions) = product_instructions {
        content.push(serde_json::json!({
            "type": "text",
            "text": format!("<artisan-product-instructions>\n{instructions}\n</artisan-product-instructions>"),
        }));
    }
    if parts.is_empty() {
        content.push(serde_json::json!({ "type": "text", "text": text }));
        return Ok(content);
    }
    for part in parts {
        match part {
            PromptPart::Text(delta) => {
                content.push(serde_json::json!({ "type": "text", "text": delta }));
            }
            PromptPart::Image(image) => {
                if image.id.is_empty() || image.name.is_empty() || image.media_type.is_empty() {
                    return Err(AcpError::InvalidContent);
                }
                let data = base64::engine::general_purpose::STANDARD.encode(image.bytes.as_slice());
                match mode {
                    ImageMode::Image => {
                        content.push(serde_json::json!({
                            "type": "image",
                            "data": data,
                            "mimeType": image.media_type,
                        }));
                    }
                    ImageMode::Embedded => {
                        let uri = format!(
                            "artisan://attachment/{}/{}",
                            percent_encode(image.id.as_str()),
                            percent_encode(image.name.as_str())
                        );
                        content.push(serde_json::json!({
                            "type": "resource",
                            "resource": {
                                "blob": data,
                                "mimeType": image.media_type,
                                "uri": uri,
                            },
                        }));
                    }
                }
            }
        }
    }
    if content.is_empty() {
        content.push(serde_json::json!({ "type": "text", "text": text }));
    }
    Ok(content)
}

// ---------------------------------------------------------------------------
// Per-engine rows: plain data for grok and cursor only
// ---------------------------------------------------------------------------

/// Explicit launch params mirroring `GrokAcpArgs`/`CursorAcpArgs` inputs.
/// `GrokSettings`/`CursorSettings` do not exist yet, so rows share this
/// struct and each builder interprets its engine-scoped literals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchArgs {
    pub(crate) model: Option<String>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) speed_fast: bool,
    pub(crate) permission: Option<String>,
    pub(crate) write_access: bool,
}

/// Builds grok stdio args, mirroring `GrokAcpArgs`: `--no-auto-update`,
/// optional `--model`/`--reasoning-effort`, plan mode when writes are
/// denied, `auto`/`always-approve` permission mapping, then
/// `agent stdio`.
#[must_use = "arg rows must be passed to the spawn call"]
pub(crate) fn grok_build_args(args: &LaunchArgs) -> Vec<OsString> {
    let mut out = vec![OsString::from("--no-auto-update")];
    if let Some(model) = args.model.as_ref().filter(|model| !model.is_empty()) {
        out.push(OsString::from("--model"));
        out.push(OsString::from(model));
    }
    if let Some(effort) = args
        .reasoning_effort
        .as_ref()
        .filter(|effort| !effort.is_empty())
    {
        out.push(OsString::from("--reasoning-effort"));
        out.push(OsString::from(effort));
    }
    if !args.write_access {
        out.push(OsString::from("--permission-mode"));
        out.push(OsString::from("plan"));
    } else if args.permission.as_deref() == Some("auto") {
        out.push(OsString::from("--permission-mode"));
        out.push(OsString::from("auto"));
    } else if args.permission.as_deref() == Some("always-approve") {
        out.push(OsString::from("--always-approve"));
    }
    out.push(OsString::from("agent"));
    out.push(OsString::from("stdio"));
    out
}

fn has_cursor_effort_suffix(model: &str) -> bool {
    let base = model.strip_suffix("-fast").unwrap_or(model);
    base.rfind('-').is_some_and(|dash| {
        matches!(
            &base[dash + 1..],
            "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        )
    })
}

fn resolve_cursor_model(args: &LaunchArgs) -> Option<String> {
    let model = args.model.as_ref().filter(|model| !model.is_empty())?;
    if model.contains('[') {
        return Some(model.clone());
    }
    let mut resolved = model.clone();
    if let Some(effort) = args
        .reasoning_effort
        .as_ref()
        .filter(|effort| !effort.is_empty())
    {
        if !has_cursor_effort_suffix(&resolved) {
            resolved.push('-');
            resolved.push_str(effort);
        }
    }
    if args.speed_fast && !resolved.ends_with("-fast") {
        resolved.push_str("-fast");
    }
    Some(resolved)
}

/// Builds cursor stdio args, mirroring `CursorAcpArgs` with
/// `ResolveCursorModel`: optional resolved `--model`, ask mode when writes
/// are denied, `--force` mapping, then `acp`.
#[must_use = "arg rows must be passed to the spawn call"]
pub(crate) fn cursor_build_args(args: &LaunchArgs) -> Vec<OsString> {
    let mut out = Vec::new();
    if let Some(model) = resolve_cursor_model(args) {
        out.push(OsString::from("--model"));
        out.push(OsString::from(model));
    }
    if !args.write_access {
        out.push(OsString::from("--mode"));
        out.push(OsString::from("ask"));
    } else if args.permission.as_deref() == Some("force") {
        out.push(OsString::from("--force"));
    }
    out.push(OsString::from("acp"));
    out
}

fn contains_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle_bytes = needle.as_bytes();
    haystack
        .as_bytes()
        .windows(needle_bytes.len())
        .any(|window| {
            window
                .iter()
                .zip(needle_bytes.iter())
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        })
}

/// Shared authenticated-output classifier, mirroring the identical grok and
/// cursor `Authenticated` evidence: any `not authenticated`/`not logged in`
/// marker (case-insensitive) means unauthenticated.
#[must_use = "classifier results must gate admission"]
pub(crate) fn default_is_authenticated_output(output: &str) -> bool {
    !contains_insensitive(output, "not authenticated")
        && !contains_insensitive(output, "not logged in")
}

fn parse_semver(bytes: &[u8]) -> Option<String> {
    let mut cursor = 0_usize;
    for part in 0..3 {
        let digits = bytes[cursor..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        cursor += digits;
        if part < 2 {
            if bytes.get(cursor) != Some(&b'.') {
                return None;
            }
            cursor += 1;
        }
    }
    if bytes.get(cursor) == Some(&b'-') {
        let suffix = bytes[cursor + 1..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            .count();
        if suffix == 0 {
            return None;
        }
        cursor += 1 + suffix;
    }
    std::str::from_utf8(&bytes[..cursor])
        .ok()
        .map(str::to_owned)
}

/// Parses grok versions, mirroring `/\bgrok\s+(\d+\.\d+\.\d+(?:-…)?)/i`:
/// word-boundary `grok`, ASCII whitespace, then semver with an optional
/// prerelease tail.
#[must_use = "version results must gate admission"]
pub(crate) fn parse_grok_version(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut offset = 0_usize;
    while offset + 4 <= bytes.len() {
        let candidate = &bytes[offset..offset + 4];
        let is_grok = candidate[0].eq_ignore_ascii_case(&b'g')
            && candidate[1].eq_ignore_ascii_case(&b'r')
            && candidate[2].eq_ignore_ascii_case(&b'o')
            && candidate[3].eq_ignore_ascii_case(&b'k');
        if is_grok && (offset == 0 || !bytes[offset - 1].is_ascii_alphanumeric()) {
            let spaces = bytes[offset + 4..]
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
            if spaces > 0 {
                if let Some(version) = parse_semver(&bytes[offset + 4 + spaces..]) {
                    return Some(version);
                }
            }
        }
        offset += 1;
    }
    None
}

fn take_digits(bytes: &[u8], min: usize, max: usize) -> Option<usize> {
    let count = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
        .min(max);
    if count < min {
        return None;
    }
    Some(count)
}

fn cursor_dated_len(bytes: &[u8]) -> Option<usize> {
    let mut cursor = take_digits(bytes, 4, 4)?;
    if bytes.get(cursor) != Some(&b'.') {
        return None;
    }
    cursor += 1;
    cursor += take_digits(&bytes[cursor..], 1, 2)?;
    if bytes.get(cursor) != Some(&b'.') {
        return None;
    }
    cursor += 1;
    cursor += take_digits(&bytes[cursor..], 1, 2)?;
    if bytes.get(cursor) != Some(&b'-') {
        return None;
    }
    cursor += 1;
    let suffix = bytes[cursor..]
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        .count();
    if suffix == 0 {
        return None;
    }
    Some(cursor + suffix)
}

/// Parses cursor versions, mirroring
/// `/\b(\d{4}\.\d{1,2}\.\d{1,2}-[0-9A-Za-z._-]+)\b/`: a dated release with a
/// mandatory suffix tail.
#[must_use = "version results must gate admission"]
pub(crate) fn parse_cursor_version(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let boundary = offset == 0
            || !(bytes[offset - 1].is_ascii_alphanumeric() || bytes[offset - 1] == b'_');
        if boundary {
            if let Some(len) = cursor_dated_len(&bytes[offset..]) {
                return std::str::from_utf8(&bytes[offset..offset + len])
                    .ok()
                    .map(str::to_owned);
            }
        }
        offset += 1;
    }
    None
}

/// Selects the grok auth method, mirroring the TypeScript evidence: an API
/// key wins when offered, otherwise the cached token, otherwise nothing.
#[must_use = "auth selection must gate the handshake"]
pub(crate) fn grok_select_auth_method(
    available: &[&str],
    has_api_key: bool,
) -> Option<&'static str> {
    if has_api_key && available.contains(&"xai.api_key") {
        return Some("xai.api_key");
    }
    if available.contains(&"cached_token") {
        return Some("cached_token");
    }
    None
}

/// Selects the cursor auth method, mirroring the TypeScript evidence:
/// `cursor_login` when offered, otherwise nothing.
#[must_use = "auth selection must gate the handshake"]
pub(crate) fn cursor_select_auth_method(
    available: &[&str],
    _has_api_key: bool,
) -> Option<&'static str> {
    if available.contains(&"cursor_login") {
        return Some("cursor_login");
    }
    None
}

/// One per-engine ACP definition row: plain data only. No tasks, no I/O,
/// no bridges; the dispatch arms interpret these rows through this core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AcpDefinition {
    pub(crate) engine_id: &'static str,
    pub(crate) executable: &'static str,
    pub(crate) version_args: &'static [&'static str],
    pub(crate) parse_version: fn(&str) -> Option<String>,
    pub(crate) auth_probe_args: &'static [&'static str],
    pub(crate) is_authenticated_output: fn(&str) -> bool,
    pub(crate) select_auth_method: fn(&[&str], bool) -> Option<&'static str>,
    pub(crate) image_mode: ImageMode,
    pub(crate) build_args: fn(&LaunchArgs) -> Vec<OsString>,
}

/// Grok Build row: `grok` executable, embedded image blocks.
pub(crate) const GROK_ACP: AcpDefinition = AcpDefinition {
    engine_id: "grok",
    executable: "grok",
    version_args: &["--version"],
    parse_version: parse_grok_version,
    auth_probe_args: &["--no-auto-update", "models"],
    is_authenticated_output: default_is_authenticated_output,
    select_auth_method: grok_select_auth_method,
    image_mode: ImageMode::Embedded,
    build_args: grok_build_args,
};

/// Cursor executable name, mirroring the TypeScript platform evidence.
pub(crate) const CURSOR_EXECUTABLE: &str = if cfg!(windows) { "agent.cmd" } else { "agent" };

/// Cursor row: platform executable, native image blocks.
pub(crate) const CURSOR_ACP: AcpDefinition = AcpDefinition {
    engine_id: "cursor",
    executable: CURSOR_EXECUTABLE,
    version_args: &["--version"],
    parse_version: parse_cursor_version,
    auth_probe_args: &["status"],
    is_authenticated_output: default_is_authenticated_output,
    select_auth_method: cursor_select_auth_method,
    image_mode: ImageMode::Image,
    build_args: cursor_build_args,
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
            match (stdin, stdout, stderr) {
                (Some(stdin), Some(stdout), Some(stderr)) => AcpPipes {
                    stdin,
                    stdout,
                    stderr,
                },
                _ => {
                    let _ignored = grouped.start_kill();
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "acp stdio unavailable",
                    ));
                }
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
/// was observed, one more wait on the remaining budget, else quarantine.
#[derive(Debug)]
pub(crate) enum AcpShutdown {
    ReapedWithoutKill(ExitStatus),
    ReapedAfterKill(ExitStatus),
    Retained(AcpRetainedChild),
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
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .unwrap_or(Duration::ZERO);
    let settled = if remaining.is_zero() {
        match timeout(Duration::ZERO, inner.wait()).await {
            Ok(result) => result,
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "zero acp close budget expired",
            )),
        }
    } else {
        let second = Instant::now().checked_add(remaining).unwrap_or(deadline);
        wait_for_child(&mut inner, second).await
    };
    match settled {
        Ok(status) => AcpShutdown::ReapedAfterKill(status),
        Err(_) => AcpShutdown::Retained(AcpRetainedChild { inner }),
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

    async fn send_request(&mut self, method: &str, params: Value) -> Result<AcpId, AcpError> {
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
            Err(AcpError::InactivityStall) | Err(AcpError::PeerClosed) => {
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
            | Err(AcpError::InactivityStall)
            | Err(AcpError::PeerClosed) => Err(AcpError::AuthUnavailable),
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
            Err(AcpError::InactivityStall) | Err(AcpError::PeerClosed) => {
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
            | Err(AcpError::InactivityStall)
            | Err(AcpError::PeerClosed) => Err(AcpError::ChildFailed),
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

// ---------------------------------------------------------------------------
// Fixture-agent tests: a fake agent speaking JSON-RPC over stdio
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, DuplexStream, ReadHalf, WriteHalf, duplex, split};

    fn strict_bounds() -> AcpBounds {
        AcpBounds::new(
            4096,
            16_384,
            256,
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_secs(2),
        )
        .expect("test bounds hold")
    }

    fn launch_args() -> LaunchArgs {
        LaunchArgs {
            model: None,
            reasoning_effort: None,
            speed_fast: false,
            permission: None,
            write_access: true,
        }
    }

    async fn agent_read_value(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Option<Value> {
        let mut line = String::new();
        let count = reader.read_line(&mut line).await.expect("agent reads");
        if count == 0 {
            return None;
        }
        Some(serde_json::from_str(line.trim_end()).expect("driver frames stay valid json"))
    }

    async fn agent_write_line(writer: &mut WriteHalf<DuplexStream>, line: &str) {
        writer
            .write_all(line.as_bytes())
            .await
            .expect("agent writes");
        writer.write_all(b"\n").await.expect("agent writes");
        writer.flush().await.expect("agent flushes");
    }

    fn update_line(session: &str, index: u32) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": session, "update": { "kind": "delta", "index": index } },
        })
        .to_string()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_agent_lifecycle_initialize_session_updates_close() {
        let bounds = strict_bounds();
        let (driver_io, agent_io) = duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, agent_write_half) = split(agent_io);
        let mut agent_read = BufReader::new(agent_read_half);
        let mut agent_write = agent_write_half;
        let (cancel_seen_tx, cancel_seen_rx) = tokio::sync::oneshot::channel::<bool>();
        let (eof_seen_tx, eof_seen_rx) = tokio::sync::oneshot::channel::<bool>();

        let agent = tokio::spawn(async move {
            let init = agent_read_value(&mut agent_read)
                .await
                .expect("initialize request");
            assert_eq!(
                init.get("method").and_then(Value::as_str),
                Some(METHOD_INITIALIZE)
            );
            let init_id = init.get("id").cloned().expect("initialize id");
            agent_write_line(
                &mut agent_write,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": init_id,
                    "result": { "protocolVersion": 1, "authMethods": [{ "id": "cached_token" }] },
                })
                .to_string(),
            )
            .await;

            let auth = agent_read_value(&mut agent_read)
                .await
                .expect("authenticate request");
            assert_eq!(
                auth.get("method").and_then(Value::as_str),
                Some(METHOD_AUTHENTICATE)
            );
            let auth_id = auth.get("id").cloned().expect("authenticate id");
            agent_write_line(
                &mut agent_write,
                &serde_json::json!({ "jsonrpc": "2.0", "id": auth_id, "result": {} }).to_string(),
            )
            .await;

            let new = agent_read_value(&mut agent_read)
                .await
                .expect("session/new request");
            assert_eq!(
                new.get("method").and_then(Value::as_str),
                Some(METHOD_SESSION_NEW)
            );
            let new_id = new.get("id").cloned().expect("session/new id");
            agent_write_line(
                &mut agent_write,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": new_id,
                    "result": { "sessionId": "sess-1" },
                })
                .to_string(),
            )
            .await;

            let prompt = agent_read_value(&mut agent_read)
                .await
                .expect("session/prompt request");
            assert_eq!(
                prompt.get("method").and_then(Value::as_str),
                Some(METHOD_SESSION_PROMPT)
            );
            let prompt_id = prompt.get("id").cloned().expect("prompt id");
            for index in [1, 2] {
                agent_write_line(&mut agent_write, &update_line("sess-1", index)).await;
            }
            agent_write_line(
                &mut agent_write,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": prompt_id,
                    "result": {
                        "stopReason": "completed",
                        "usage": { "inputTokens": 3, "outputTokens": 5 },
                    },
                })
                .to_string(),
            )
            .await;

            let mut saw_cancel = false;
            while let Some(frame) = agent_read_value(&mut agent_read).await {
                if frame.get("method").and_then(Value::as_str) == Some(METHOD_SESSION_CANCEL) {
                    saw_cancel = true;
                }
            }
            let _ignored = cancel_seen_tx.send(saw_cancel);
            let _ignored = eof_seen_tx.send(true);
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let init = driver.initialize().await.expect("handshake");
        assert_eq!(init.protocol_version, ACP_PROTOCOL_VERSION);
        assert_eq!(init.auth_methods, vec!["cached_token".to_owned()]);
        let available: Vec<&str> = init.auth_methods.iter().map(String::as_str).collect();
        assert_eq!(
            (GROK_ACP.select_auth_method)(&available, false),
            Some("cached_token")
        );
        driver
            .authenticate("cached_token")
            .await
            .expect("authenticate");
        let session = driver.new_session("C:\\work").await.expect("session/new");
        assert_eq!(session.as_str(), "sess-1");
        let content =
            build_prompt_content(ImageMode::Embedded, "hello", &[], None).expect("prompt content");
        let prompt_id = driver.prompt(&session, content).await.expect("prompt");
        for _ in [1, 2] {
            let event = driver
                .next_update(&session, &prompt_id)
                .await
                .expect("session update");
            assert!(
                matches!(event, UpdateEvent::SessionUpdate(_)),
                "expected update, got {event:?}"
            );
        }
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("prompt result")
        {
            UpdateEvent::PromptResult(outcome) => {
                assert!(!outcome.cancelled);
                let usage = outcome.usage.expect("usage reported");
                assert_eq!(usage.input, 3);
                assert_eq!(usage.output, 5);
                assert_eq!(usage.cached_input, None);
            }
            other => panic!("expected prompt result, got {other:?}"),
        }
        driver.cancel(&session).await.expect("cancel notify");
        driver.shutdown_writer().await.expect("lifeline close");
        drop(driver);
        assert!(cancel_seen_rx.await.expect("cancel report"));
        assert!(eof_seen_rx.await.expect("eof report"));
        agent.await.expect("agent joins");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oversize_request_rejected_before_any_write() {
        let bounds = AcpBounds::new(
            4096,
            128,
            256,
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_secs(2),
        )
        .expect("test bounds hold");
        let (driver_io, agent_io) = duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, agent_write_half) = split(agent_io);
        let mut agent_read = BufReader::new(agent_read_half);

        let agent = tokio::spawn(async move {
            drop(agent_write_half);
            let mut line = String::new();
            agent_read
                .read_line(&mut line)
                .await
                .expect("agent reads one frame");
            assert!(
                line.contains(METHOD_SESSION_CANCEL),
                "first agent frame must be the cancel, got: {line}"
            );
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let big = Value::String("x".repeat(256));
        let error = driver
            .send_request(METHOD_SESSION_PROMPT, big)
            .await
            .expect_err("oversize envelope");
        assert_eq!(error, AcpError::EnvelopeTooLarge);
        let session = SessionId::parse("sess-1", 256).expect("session");
        driver.cancel(&session).await.expect("stream stays usable");
        driver.shutdown_writer().await.expect("close");
        drop(driver);
        agent.await.expect("agent joins");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn inactivity_stall_after_silence_and_rearm_on_activity() {
        let bounds = AcpBounds::new(
            4096,
            16_384,
            256,
            Duration::from_secs(5),
            Duration::from_millis(400),
            Duration::from_secs(2),
        )
        .expect("test bounds hold");
        let (driver_io, agent_io) = duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, mut agent_write) = split(agent_io);
        drop(agent_read_half);

        let agent = tokio::spawn(async move {
            agent_write_line(&mut agent_write, &update_line("sess-stall", 1)).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            agent_write_line(&mut agent_write, &update_line("sess-stall", 2)).await;
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        assert!(
            driver.activity_deadline() > Instant::now(),
            "rolling deadline sits one window ahead"
        );
        let session = SessionId::parse("sess-stall", 256).expect("session");
        let prompt_id = AcpId::Number(7);
        let first = driver
            .next_update(&session, &prompt_id)
            .await
            .expect("first update inside the window");
        assert!(matches!(first, UpdateEvent::SessionUpdate(_)));
        let second = driver
            .next_update(&session, &prompt_id)
            .await
            .expect("activity re-arms the deadline");
        assert!(matches!(second, UpdateEvent::SessionUpdate(_)));
        let stall = driver
            .next_update(&session, &prompt_id)
            .await
            .expect_err("silence must stall");
        assert_eq!(stall, AcpError::InactivityStall);
        agent.await.expect("agent joins");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn load_session_resume_round_trip() {
        let bounds = strict_bounds();
        let (driver_io, agent_io) = duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, mut agent_write) = split(agent_io);
        let mut agent_read = BufReader::new(agent_read_half);

        let agent = tokio::spawn(async move {
            let first = agent_read_value(&mut agent_read)
                .await
                .expect("session/load request");
            assert_eq!(
                first.get("method").and_then(Value::as_str),
                Some(METHOD_SESSION_LOAD)
            );
            assert_eq!(
                first
                    .get("params")
                    .and_then(|params| params.get("sessionId"))
                    .and_then(Value::as_str),
                Some("sess-9")
            );
            let first_id = first.get("id").cloned().expect("load id");
            agent_write_line(
                &mut agent_write,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": first_id,
                    "result": { "sessionId": "sess-9" },
                })
                .to_string(),
            )
            .await;

            let second = agent_read_value(&mut agent_read)
                .await
                .expect("second session/load request");
            let second_id = second.get("id").cloned().expect("load id");
            agent_write_line(
                &mut agent_write,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": second_id,
                    "error": { "code": -32_000 },
                })
                .to_string(),
            )
            .await;
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let session = SessionId::parse("sess-9", 256).expect("session");
        driver
            .load_session(&session, "C:\\work")
            .await
            .expect("resume");
        let error = driver
            .load_session(&session, "C:\\work")
            .await
            .expect_err("rejected resume");
        assert_eq!(error, AcpError::ChildFailed);
        agent.await.expect("agent joins");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn update_loop_skips_foreign_frames_and_surfaces_agent_requests() {
        let bounds = strict_bounds();
        let (driver_io, agent_io) = duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, mut agent_write) = split(agent_io);
        drop(agent_read_half);

        let agent = tokio::spawn(async move {
            agent_write_line(&mut agent_write, &update_line("other-sess", 9)).await;
            agent_write_line(
                &mut agent_write,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 99,
                    "method": "requestPermission",
                    "params": { "toolCall": { "toolCallId": "t1" } },
                })
                .to_string(),
            )
            .await;
            agent_write_line(&mut agent_write, &update_line("sess-1", 1)).await;
            agent_write_line(
                &mut agent_write,
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 5,
                    "result": { "stopReason": "cancelled" },
                })
                .to_string(),
            )
            .await;
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let session = SessionId::parse("sess-1", 256).expect("session");
        let prompt_id = AcpId::Number(5);
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("agent request")
        {
            UpdateEvent::AgentRequest { id, method, .. } => {
                assert_eq!(id, AcpId::Number(99));
                assert_eq!(method, "requestPermission");
            }
            other => panic!("expected agent request, got {other:?}"),
        }
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("own update")
        {
            UpdateEvent::SessionUpdate(update) => {
                assert_eq!(update.session.as_str(), "sess-1");
            }
            other => panic!("expected session update, got {other:?}"),
        }
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("prompt result")
        {
            UpdateEvent::PromptResult(outcome) => {
                assert!(outcome.cancelled);
                assert_eq!(outcome.usage, None);
            }
            other => panic!("expected prompt result, got {other:?}"),
        }
        agent.await.expect("agent joins");
    }

    #[cfg(windows)]
    fn silent_idle_child_command() -> Option<(OsString, Vec<OsString>)> {
        Some((
            OsString::from("powershell"),
            vec![
                OsString::from("-NoProfile"),
                OsString::from("-NonInteractive"),
                OsString::from("-Command"),
                OsString::from("Start-Sleep -Seconds 60"),
            ],
        ))
    }

    #[cfg(not(windows))]
    fn silent_idle_child_command() -> Option<(OsString, Vec<OsString>)> {
        for candidate in ["/bin/sleep", "/usr/bin/sleep"] {
            if Path::new(candidate).is_file() {
                return Some((OsString::from(candidate), vec![OsString::from("60")]));
            }
        }
        None
    }

    #[tokio::test(flavor = "current_thread")]
    async fn slow_agent_handshake_deadline_kill_with_custody() {
        let Some((program, args)) = silent_idle_child_command() else {
            eprintln!("SKIP: no silent idle child for the kill-custody proof");
            return;
        };
        let bounds = AcpBounds::new(
            4096,
            16_384,
            256,
            Duration::from_millis(300),
            Duration::from_millis(300),
            Duration::from_secs(5),
        )
        .expect("test bounds hold");
        let mut child = match spawn_acp_child(program.as_os_str(), &args, None) {
            Ok(child) => child,
            Err(_) => {
                eprintln!("SKIP: idle child spawn unavailable");
                return;
            }
        };
        assert!(child.id().is_some(), "spawned child reports a pid");
        let pipes = child.take_pipes().expect("piped stdio");
        drop(pipes.stderr);
        let mut driver = AcpTransport::new(BufReader::new(pipes.stdout), pipes.stdin, bounds);
        let error = driver
            .initialize()
            .await
            .expect_err("silent agent must miss the handshake deadline");
        assert_eq!(error, AcpError::HandshakeTimeout);
        driver.shutdown_writer().await.expect("lifeline close");
        drop(driver);
        match shutdown_acp_child(child, Duration::from_secs(5)).await {
            AcpShutdown::ReapedAfterKill(status) => {
                let observed = ObservedExit::from(status);
                assert!(
                    matches!(
                        classify_exit(observed, false),
                        ClassifiedExit::Interrupted | ClassifiedExit::Failed { .. }
                    ),
                    "killed child classifies as interrupted or failed"
                );
            }
            AcpShutdown::ReapedWithoutKill(_) => {}
            AcpShutdown::Retained(_) => panic!("idle child must be reaped after kill"),
        }
    }

    #[test]
    fn spawn_missing_executable_fails_without_shell() {
        let missing = if cfg!(windows) {
            "C:\\nonexistent\\artisan-acp-test-agent.exe"
        } else {
            "/nonexistent/artisan-acp-test-agent"
        };
        assert!(
            matches!(
                spawn_acp_child(OsStr::new(missing), &[], None),
                Err(error) if error.kind() == io::ErrorKind::NotFound
            ),
            "missing executable"
        );
    }

    #[test]
    fn framer_splits_fragmented_lines_and_trims_cr() {
        let mut framer = AcpFramer::new(64);
        let first = framer.push(b"{\"a\":1}\n{\"b\":").expect("frames");
        assert_eq!(first, vec!["{\"a\":1}".to_owned()]);
        let second = framer.push(b"2}\r\n").expect("frames");
        assert_eq!(second, vec!["{\"b\":2}".to_owned()]);
    }

    #[test]
    fn framer_rejects_oversize_line_and_poisons() {
        let mut framer = AcpFramer::new(8);
        assert_eq!(
            framer.push(b"123456789").expect_err("oversize"),
            AcpError::LineTooLong
        );
        assert_eq!(
            framer.push(b"\n").expect_err("poisoned"),
            AcpError::Poisoned
        );
    }

    #[test]
    fn framer_rejects_invalid_utf8_and_discards_partial_on_finish() {
        let mut framer = AcpFramer::new(64);
        assert_eq!(
            framer.push(&[0x7b, 0xff, 0x7d, b'\n']).expect_err("utf8"),
            AcpError::InvalidUtf8
        );
        assert_eq!(
            framer.push(b"{}\n").expect_err("poisoned"),
            AcpError::Poisoned
        );
        assert_eq!(
            framer.finish().expect_err("poisoned finish"),
            AcpError::Poisoned
        );

        let mut clean = AcpFramer::new(64);
        assert!(clean.push(b"partial").expect("buffered").is_empty());
        clean.finish().expect("finish discards partial");
        assert_eq!(
            clean.push(b"x\n").expect_err("finished"),
            AcpError::Poisoned
        );
    }

    #[test]
    fn malformed_envelopes_rejected() {
        let bound = 4096;
        let cases = [
            "",
            "not json",
            "[]",
            "null",
            "42",
            "\"x\"",
            "{\"jsonrpc\":\"2.0\"}",
            "{\"method\":\"m\"}",
            "{\"jsonrpc\":\"1.0\",\"id\":1,\"method\":\"m\"}",
            "{\"jsonrpc\":\"2.0\",\"id\":true,\"method\":\"m\"}",
            "{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"m\"}",
            "{\"jsonrpc\":\"2.0\",\"id\":1}",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":1,\"error\":{\"code\":1}}",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":42}",
            "{\"jsonrpc\":\"2.0\",\"id\":\"\",\"method\":\"m\"}",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"m\",\"result\":1}",
            "{\"jsonrpc\":\"2.0\",\"id\":1.5,\"method\":\"m\"}",
            "{\"jsonrpc\":\"2.0\",\"method\":\"m\",\"params\":1,\"id\":2,\"result\":null}",
        ];
        for line in cases {
            assert_eq!(
                parse_envelope(line, bound).expect_err("malformed"),
                AcpError::MalformedEnvelope,
                "line: {line}"
            );
        }
        assert_eq!(
            parse_envelope("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"m\"}", 8)
                .expect_err("oversize"),
            AcpError::EnvelopeTooLarge
        );
    }

    #[test]
    fn valid_envelopes_round_trip() {
        let bound = 4096;
        match parse_envelope(
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/prompt\",\"params\":{\"a\":1}}",
            bound,
        )
        .expect("request")
        {
            AcpEnvelope::Request { id, method, params } => {
                assert_eq!(id, AcpId::Number(3));
                assert_eq!(method, "session/prompt");
                assert_eq!(params, serde_json::json!({"a": 1}));
            }
            other => panic!("expected request, got {other:?}"),
        }
        match parse_envelope("{\"jsonrpc\":\"2.0\",\"method\":\"session/update\"}", bound)
            .expect("notification")
        {
            AcpEnvelope::Notification { method, params } => {
                assert_eq!(method, "session/update");
                assert_eq!(params, Value::Null);
            }
            other => panic!("expected notification, got {other:?}"),
        }
        match parse_envelope(
            "{\"jsonrpc\":\"2.0\",\"id\":\"a\",\"result\":{\"sessionId\":\"s\"}}",
            bound,
        )
        .expect("response")
        {
            AcpEnvelope::Response { id, payload } => {
                assert_eq!(id, AcpId::Text("a".to_owned()));
                assert_eq!(
                    payload,
                    AcpResponsePayload::Result(serde_json::json!({"sessionId": "s"}))
                );
            }
            other => panic!("expected response, got {other:?}"),
        }
        match parse_envelope(
            "{\"jsonrpc\":\"2.0\",\"id\":4,\"error\":{\"code\":-32600,\"message\":\"hidden\"}}",
            bound,
        )
        .expect("error response")
        {
            AcpEnvelope::Response { id, payload } => {
                assert_eq!(id, AcpId::Number(4));
                assert_eq!(payload, AcpResponsePayload::Error { code: -32_600 });
            }
            other => panic!("expected error response, got {other:?}"),
        }
    }

    #[test]
    fn session_id_validation() {
        assert_eq!(
            SessionId::parse("sess-1", 256).expect("valid").as_str(),
            "sess-1"
        );
        assert_eq!(SessionId::parse("x", 1).expect("boundary").as_str(), "x");
        for bad in ["", "a/b", "a?b", "a#b", "a%b", "a b", "a\nb", "xy"] {
            let max = if bad == "xy" { 1 } else { 256 };
            assert!(
                SessionId::parse(bad, max).is_err(),
                "session id must reject {bad:?}"
            );
        }
    }

    #[test]
    fn bounds_reject_zero_sizes() {
        let ok = || {
            AcpBounds::new(
                1024,
                1024,
                256,
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
        };
        assert!(ok().is_ok());
        assert_eq!(
            AcpBounds::new(
                0,
                1024,
                256,
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .expect_err("line"),
            AcpBoundsError::ZeroBound {
                field: "max_line_bytes"
            }
        );
        assert_eq!(
            AcpBounds::new(
                1024,
                0,
                256,
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .expect_err("envelope"),
            AcpBoundsError::ZeroBound {
                field: "max_envelope_bytes"
            }
        );
        assert_eq!(
            AcpBounds::new(
                1024,
                1024,
                0,
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .expect_err("session"),
            AcpBoundsError::ZeroBound {
                field: "max_session_id_bytes"
            }
        );
    }

    #[test]
    fn exit_classification_matrix() {
        assert_eq!(
            classify_exit(
                ObservedExit {
                    code: None,
                    signaled: false
                },
                false
            ),
            ClassifiedExit::Interrupted
        );
        assert_eq!(
            classify_exit(
                ObservedExit {
                    code: None,
                    signaled: true
                },
                false
            ),
            ClassifiedExit::Interrupted
        );
        assert_eq!(
            classify_exit(
                ObservedExit {
                    code: Some(0),
                    signaled: false
                },
                false
            ),
            ClassifiedExit::Failed { code: Some(0) }
        );
        assert_eq!(
            classify_exit(
                ObservedExit {
                    code: Some(1),
                    signaled: true
                },
                false
            ),
            ClassifiedExit::Interrupted
        );
        assert_eq!(
            classify_exit(
                ObservedExit {
                    code: Some(1),
                    signaled: false
                },
                false
            ),
            ClassifiedExit::Failed { code: Some(1) }
        );
        assert_eq!(
            classify_exit(
                ObservedExit {
                    code: Some(0),
                    signaled: false
                },
                true
            ),
            ClassifiedExit::Cancelled
        );
        assert_eq!(
            classify_exit(
                ObservedExit {
                    code: None,
                    signaled: true
                },
                true
            ),
            ClassifiedExit::Cancelled
        );
    }

    #[cfg(unix)]
    #[test]
    fn real_exit_status_converts() {
        let candidate = if Path::new("/bin/true").is_file() {
            "/bin/true"
        } else if Path::new("/usr/bin/true").is_file() {
            "/usr/bin/true"
        } else {
            eprintln!("SKIP: no true(1) for exit conversion");
            return;
        };
        let status = std::process::Command::new(candidate)
            .status()
            .expect("true runs");
        let observed = ObservedExit::from(status);
        assert!(!observed.signaled);
        assert_eq!(observed.code, Some(0));
        assert_eq!(
            classify_exit(observed, false),
            ClassifiedExit::Failed { code: Some(0) }
        );
    }

    #[test]
    fn grok_version_parser_accepts_and_rejects() {
        assert_eq!(parse_grok_version("grok 0.7.3"), Some("0.7.3".to_owned()));
        assert_eq!(
            parse_grok_version("GROK 1.2.3-beta.1"),
            Some("1.2.3-beta.1".to_owned())
        );
        assert_eq!(
            parse_grok_version("my grok 10.20.30 done"),
            Some("10.20.30".to_owned())
        );
        for bad in [
            "0.7.3",
            "grokk 1.2.3",
            "grok-cli 2.0.0",
            "grok",
            "grok x.y.z",
            "grok 1.2",
            "grok 1.2.3-",
        ] {
            assert_eq!(parse_grok_version(bad), None, "grok rejects {bad:?}");
        }
        assert_eq!(
            (GROK_ACP.parse_version)("grok 0.7.3"),
            Some("0.7.3".to_owned())
        );
    }

    #[test]
    fn cursor_version_parser_accepts_and_rejects() {
        assert_eq!(
            parse_cursor_version("agent 2026.9.6-stable.1 (abc)"),
            Some("2026.9.6-stable.1".to_owned())
        );
        assert_eq!(
            parse_cursor_version("2025.12.31-nightly"),
            Some("2025.12.31-nightly".to_owned())
        );
        for bad in [
            "1.2.3",
            "2026.9.6",
            "26.9.6-x",
            "2026.9-x",
            "no version here",
        ] {
            assert_eq!(parse_cursor_version(bad), None, "cursor rejects {bad:?}");
        }
        assert_eq!(
            (CURSOR_ACP.parse_version)("agent 2026.9.6-stable.1"),
            Some("2026.9.6-stable.1".to_owned())
        );
    }

    #[test]
    fn auth_classifiers_per_row() {
        assert_eq!(
            grok_select_auth_method(&["xai.api_key", "cached_token"], true),
            Some("xai.api_key")
        );
        assert_eq!(
            grok_select_auth_method(&["xai.api_key", "cached_token"], false),
            Some("cached_token")
        );
        assert_eq!(
            grok_select_auth_method(&["cached_token"], false),
            Some("cached_token")
        );
        assert_eq!(grok_select_auth_method(&[], true), None);
        assert_eq!(grok_select_auth_method(&["other"], false), None);

        assert_eq!(
            cursor_select_auth_method(&["cursor_login"], false),
            Some("cursor_login")
        );
        assert_eq!(
            cursor_select_auth_method(&["cursor_login"], true),
            Some("cursor_login")
        );
        assert_eq!(cursor_select_auth_method(&[], false), None);
        assert_eq!(cursor_select_auth_method(&["cached_token"], true), None);

        assert!((GROK_ACP.is_authenticated_output)("models:\n  foo"));
        assert!((CURSOR_ACP.is_authenticated_output)("signed in as s"));
        assert!(!default_is_authenticated_output(
            "Error: not authenticated, run login"
        ));
        assert!(!default_is_authenticated_output("Not logged in"));
    }

    #[test]
    fn grok_args_mirror_ts_matrix() {
        assert_eq!(
            grok_build_args(&launch_args()),
            vec![
                OsString::from("--no-auto-update"),
                OsString::from("agent"),
                OsString::from("stdio"),
            ]
        );
        let full = LaunchArgs {
            model: Some("grok-4".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            speed_fast: false,
            permission: None,
            write_access: true,
        };
        assert_eq!(
            grok_build_args(&full),
            vec![
                OsString::from("--no-auto-update"),
                OsString::from("--model"),
                OsString::from("grok-4"),
                OsString::from("--reasoning-effort"),
                OsString::from("high"),
                OsString::from("agent"),
                OsString::from("stdio"),
            ]
        );
        let plan = LaunchArgs {
            write_access: false,
            ..launch_args()
        };
        assert_eq!(
            grok_build_args(&plan),
            vec![
                OsString::from("--no-auto-update"),
                OsString::from("--permission-mode"),
                OsString::from("plan"),
                OsString::from("agent"),
                OsString::from("stdio"),
            ]
        );
        let auto = LaunchArgs {
            permission: Some("auto".to_owned()),
            ..launch_args()
        };
        assert!(grok_build_args(&auto).contains(&OsString::from("auto")));
        let approve = LaunchArgs {
            permission: Some("always-approve".to_owned()),
            ..launch_args()
        };
        assert!(grok_build_args(&approve).contains(&OsString::from("--always-approve")));
        assert_eq!((GROK_ACP.build_args)(&launch_args()).len(), 3);
    }

    #[test]
    fn cursor_args_mirror_ts_matrix() {
        assert_eq!(
            cursor_build_args(&launch_args()),
            vec![OsString::from("acp")]
        );
        let effort = LaunchArgs {
            model: Some("composer-1".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            ..launch_args()
        };
        let built = cursor_build_args(&effort);
        assert_eq!(built[0], OsString::from("--model"));
        assert_eq!(built[1], OsString::from("composer-1-high"));
        assert_eq!(built[2], OsString::from("acp"));

        let suffixed = LaunchArgs {
            model: Some("composer-1-high".to_owned()),
            reasoning_effort: Some("low".to_owned()),
            ..launch_args()
        };
        assert_eq!(
            cursor_build_args(&suffixed)[1],
            OsString::from("composer-1-high")
        );

        let fast = LaunchArgs {
            model: Some("composer-1".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            speed_fast: true,
            ..launch_args()
        };
        assert_eq!(
            cursor_build_args(&fast)[1],
            OsString::from("composer-1-high-fast")
        );

        let bracket = LaunchArgs {
            model: Some("cursor[fast]".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            ..launch_args()
        };
        assert_eq!(
            cursor_build_args(&bracket)[1],
            OsString::from("cursor[fast]")
        );

        let ask = LaunchArgs {
            write_access: false,
            ..launch_args()
        };
        assert_eq!(
            cursor_build_args(&ask),
            vec![
                OsString::from("--mode"),
                OsString::from("ask"),
                OsString::from("acp"),
            ]
        );
        let force = LaunchArgs {
            permission: Some("force".to_owned()),
            ..launch_args()
        };
        assert!(cursor_build_args(&force).contains(&OsString::from("--force")));
    }

    #[test]
    fn engine_rows_carry_plain_data() {
        assert_eq!(GROK_ACP.engine_id, "grok");
        assert_eq!(GROK_ACP.executable, "grok");
        assert_eq!(GROK_ACP.version_args, &["--version"][..]);
        assert_eq!(
            GROK_ACP.auth_probe_args,
            &["--no-auto-update", "models"][..]
        );
        assert_eq!(GROK_ACP.image_mode, ImageMode::Embedded);

        assert_eq!(CURSOR_ACP.engine_id, "cursor");
        assert!(CURSOR_ACP.executable.contains("agent"));
        assert_eq!(CURSOR_ACP.version_args, &["--version"][..]);
        assert_eq!(CURSOR_ACP.auth_probe_args, &["status"][..]);
        assert_eq!(CURSOR_ACP.image_mode, ImageMode::Image);
        if cfg!(windows) {
            assert_eq!(CURSOR_ACP.executable, "agent.cmd");
        } else {
            assert_eq!(CURSOR_ACP.executable, "agent");
        }
    }

    #[test]
    fn prompt_content_image_modes() {
        let image = PromptPart::Image(ImageBlock {
            id: "a/b".to_owned(),
            name: "x y.png".to_owned(),
            media_type: "image/png".to_owned(),
            bytes: vec![1, 2, 3],
        });
        let embedded =
            build_prompt_content(ImageMode::Embedded, "hi", &[image.clone()], Some("rules"))
                .expect("embedded content");
        assert_eq!(embedded.len(), 2);
        assert_eq!(
            embedded[0],
            serde_json::json!({
                "type": "text",
                "text": "<artisan-product-instructions>\nrules\n</artisan-product-instructions>",
            })
        );
        assert_eq!(
            embedded[1],
            serde_json::json!({
                "type": "resource",
                "resource": {
                    "blob": "AQID",
                    "mimeType": "image/png",
                    "uri": "artisan://attachment/a%2Fb/x%20y.png",
                },
            })
        );

        let native =
            build_prompt_content(ImageMode::Image, "hi", &[image], None).expect("image content");
        assert_eq!(native.len(), 1);
        assert_eq!(
            native[0],
            serde_json::json!({ "type": "image", "data": "AQID", "mimeType": "image/png" })
        );

        let plain = build_prompt_content(ImageMode::Image, "hi", &[], None).expect("text");
        assert_eq!(
            plain,
            vec![serde_json::json!({ "type": "text", "text": "hi" })]
        );

        let bad = PromptPart::Image(ImageBlock {
            id: String::new(),
            name: "n".to_owned(),
            media_type: "image/png".to_owned(),
            bytes: vec![1],
        });
        assert_eq!(
            build_prompt_content(ImageMode::Image, "hi", &[bad], None).expect_err("metadata"),
            AcpError::InvalidContent
        );
    }
}
