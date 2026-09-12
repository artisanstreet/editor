#![allow(clippy::module_name_repetitions)]
#![allow(dead_code)]

use std::fmt;
use std::time::Duration;

use super::super::consts::HEX_UPPER;
use base64::Engine as _;
use serde_json::Value;
use thiserror::Error;
use tokio::time::Instant;

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
                if let Ok(line) = String::from_utf8(line_bytes) {
                    out.push(line);
                } else {
                    self.poisoned = true;
                    return Err(AcpError::InvalidUtf8);
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
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX_UPPER[usize::from(byte >> 4)] as char);
            encoded.push(HEX_UPPER[usize::from(byte & 0x0F)] as char);
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
