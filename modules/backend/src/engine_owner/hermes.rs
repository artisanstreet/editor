//! Finite Hermes owner runtime over the private-service WebSocket gateway.
//!
//! Spawns the resolved Hermes CLI as a private loopback service
//! (`serve --host 127.0.0.1 --port 0`), parses the bounded
//! `HERMES_BACKEND_READY port=N` readiness record, connects a minimal
//! RFC 6455 JSON-RPC client to `/api/ws`, validates the live `model.options`
//! inventory before opening anything, creates or resumes exactly one session
//! with the guidance seed, normalizes streaming frames onto the shared S1a
//! observation vocabulary (`TextDelta` / `Terminal` plus cumulative usage and
//! subagent lifecycle rows), and supports steer, interrupt/cancel, and close
//! with child-custody teardown that quarantines on unobserved reaps.
//!
//! The wire boundary is typed: inbound text frames decode into
//! [`DecodedEnvelope`] through bounded `serde_json::Value` extraction. Only
//! validated identities and text cross the boundary: approval and question
//! frames become [`HermesApprovalRequest`] / [`HermesQuestionRequest`] and map
//! onto domain `ApprovalRequest` / `QuestionInput` constructors for the
//! durable A-approve resolve path. Deny lands with no side effect while the
//! turn continues; allow answers through the same durable path.
//!
//! Hermes carries no new dependencies: the gateway client is a small
//! loopback-only WebSocket implementation over Tokio TCP (masked client
//! frames, 16 MiB cap mirroring the TypeScript transport, ping/pong, close).
//! The `Sec-WebSocket-Accept` check needs SHA-1, which no workspace crate
//! provides, so a local SHA-1 over the handshake key is implemented here and
//! pinned against the RFC 6455 test vector. Authentication stays
//! profile-owned per the engine descriptor: the service spawn inherits the
//! ambient environment plus the dashboard session token, and no credential is
//! plumbed, synthesized, or logged.
//!
//! Images are rejected with [`HermesTurnError::ImagesUnsupported`]: the
//! Hermes catalog reports `image_input: false`, so an image attachment fails
//! the turn closed instead of sending a degraded text-only prompt.
//!
//! Tool frames are tracked by identity only and never adopt the root turn;
//! the shared S1a vocabulary has no tool row, so tool projections stay a
//! later packet. Compaction markers clear internal state without emitting
//! rows for the same reason.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use artisan_domain::{
    ApprovalRequest, EngineModelId, EngineRouteId, HermesSelection, ObservationId,
    ObservationSequence, QuestionInput, QuestionOption, RunId, RunUsageBasis, RunUsageReport,
    RunUsageReportInput, SubagentInput, SubagentObservation, SubagentState, ThreadId, UnixMillis,
};
use base64::Engine as _;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::observation::{EngineObservation, TerminalState, UsageObservation, chunk_text};
use super::process::ChildParts;
use super::readiness::ReadinessError;
use artisan_transport::CancelHandle;

/// Maximum accepted gateway frame bytes (mirrors the TS 16 MiB transport cap).
pub(crate) const HERMES_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Maximum bytes consumed while waiting for the readiness record (mirrors the
/// TS 256 KiB readiness bound).
pub(crate) const HERMES_MAX_READY_BYTES: usize = 256 * 1024;

/// Maximum UTF-8 bytes retained for one approval/question text field.
const HERMES_MAX_TEXT_FIELD_BYTES: usize = 8 * 1024;

/// Maximum questions retained per `clarify.request` frame.
const HERMES_MAX_QUESTIONS_PER_FRAME: usize = 32;

/// Maximum options retained per question (the domain observation ceiling).
const HERMES_MAX_OPTIONS_PER_QUESTION: usize = 16;

/// Maximum answers retained per question response (the domain ceiling).
#[cfg(test)]
pub(crate) const HERMES_MAX_ANSWERS: usize = 16;

/// Maximum bytes accepted for one provider identity field.
const HERMES_MAX_ID_BYTES: usize = 256;

/// Maximum bytes for the HTTP upgrade head.
const HERMES_MAX_HANDSHAKE_BYTES: usize = 8 * 1024;

/// Gateway JSON-RPC request timeout (mirrors the TS 60s request timeout).
const HERMES_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Minimum accepted desktop contract (mirrors the TS contract-4 floor).
const HERMES_MINIMUM_DESKTOP_CONTRACT: u64 = 4;

/// WebSocket globally unique identifier from RFC 6455 section 1.3.
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Payload-free failure of the Hermes wire boundary.
///
/// Transport mistakes (spawn, readiness, handshake, request, stream, stall,
/// cancel, shutdown, deadline, interruption, exit) surface as the owner
/// [`EngineOperationError`](super::operation::EngineOperationError) at the
/// dispatch arm; only typed-boundary mistakes originate here.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum HermesTurnError {
    #[error("hermes turn misconfigured")]
    Configuration,
    #[error("hermes does not accept image attachments")]
    ImagesUnsupported,
    #[error("hermes gateway stream failed")]
    StreamFailed,
}

/// Typed Hermes settings derived from the durable selection.
///
/// Mirrors the TypeScript `Open` selection checks (`profile_id`,
/// `provider_route_id`, `model_id`) plus the `hermes.*` provider options. The
/// domain selection already validates every field, so construction is total.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesSettings {
    profile_id: String,
    model_id: String,
    route_id: String,
    permission_yolo: bool,
    reasoning_effort: Option<String>,
    fast: bool,
}

impl HermesSettings {
    /// Derives typed gateway settings from the durable selection.
    #[must_use]
    pub(crate) fn from_selection(selection: &HermesSelection) -> Self {
        Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model_id: selection.model_id().as_str().to_owned(),
            route_id: selection.route_id().as_str().to_owned(),
            permission_yolo: selection.permission_mode().as_str() == "yolo",
            reasoning_effort: selection
                .reasoning_effort()
                .map(|effort| effort.as_str().to_owned()),
            fast: selection.fast(),
        }
    }

    /// Returns the managed profile identity.
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the selected model identity.
    pub(crate) fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Returns the selected provider route identity.
    pub(crate) fn route_id(&self) -> &str {
        &self.route_id
    }

    /// Builds the `session.create` params object for one turn.
    ///
    /// The installed profile is omitted for the `default` profile exactly
    /// like the TypeScript engine; reasoning effort defaults to `medium` at
    /// argument-building time per the domain contract.
    pub(crate) fn create_params(&self, project_root: &str, guidance: &[Value]) -> Value {
        let mut params = serde_json::Map::new();
        params.insert("close_on_disconnect".to_owned(), Value::Bool(false));
        params.insert("cwd".to_owned(), Value::String(project_root.to_owned()));
        params.insert("fast".to_owned(), Value::Bool(self.fast));
        params.insert("messages".to_owned(), Value::Array(guidance.to_owned()));
        params.insert("model".to_owned(), Value::String(self.model_id.clone()));
        if self.profile_id != "default" {
            params.insert("profile".to_owned(), Value::String(self.profile_id.clone()));
        }
        params.insert("provider".to_owned(), Value::String(self.route_id.clone()));
        params.insert(
            "reasoning_effort".to_owned(),
            Value::String(
                self.reasoning_effort
                    .clone()
                    .unwrap_or_else(|| "medium".to_owned()),
            ),
        );
        params.insert("source".to_owned(), Value::String("artisan".to_owned()));
        Value::Object(params)
    }
}

/// Verified Hermes service launch for one turn.
///
/// Carries the resolved executable, the selecting profile identity, and the
/// probed version. Never `Clone`: the dispatch arm moves it into the single
/// internal input. Revalidation rechecks that the executable is still a file;
/// install-fence depth beyond that stays with the native-engine discovery
/// packet.
pub(crate) struct VerifiedHermesLaunch {
    executable: PathBuf,
    profile_id: String,
    version: String,
}

impl VerifiedHermesLaunch {
    /// Creates a launch after validating its identities.
    ///
    /// Returns `None` when any field is empty.
    pub(crate) fn new(executable: PathBuf, profile_id: String, version: String) -> Option<Self> {
        if profile_id.is_empty() || version.is_empty() {
            return None;
        }
        Some(Self {
            executable,
            profile_id,
            version,
        })
    }

    /// Returns the managed profile identity.
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the resolved executable path.
    pub(crate) fn executable_path(&self) -> &Path {
        &self.executable
    }

    /// Returns the probed version string.
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    /// Revalidates that the executable is still present.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] when the executable is no longer a file.
    pub(crate) fn revalidate(&self) -> std::io::Result<()> {
        if self.executable.is_file() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "hermes launch rejected",
            ))
        }
    }
}

impl std::fmt::Debug for VerifiedHermesLaunch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VerifiedHermesLaunch { <redacted> }")
    }
}

/// Resolves the Hermes executable through discovery precedence
/// (`HERMES_EXECUTABLE`, installed local-app-data, `PATH`).
#[must_use]
pub(crate) fn resolve_service_executable() -> Option<PathBuf> {
    artisan_native_engine::hermes::resolve_hermes_executable()
        .map(|resolved| resolved.path().to_owned())
}

/// Mints a fresh 32-byte dashboard session token (base64url, no padding).
///
/// Returns `None` when operating-system entropy is unavailable; the caller
/// maps that to its entropy failure without touching the child.
#[must_use]
pub(crate) fn new_session_token() -> Option<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Parses one `HERMES_BACKEND_READY port=N` readiness line.
///
/// Returns the loopback port, or `None` for any other line, blank line, or
/// out-of-range port (0 and values above 65535 never bind a service).
#[must_use]
pub(crate) fn parse_ready_port_line(line: &str) -> Option<u16> {
    let trimmed = line.trim_end_matches(['\r', '\n']).trim();
    let rest = trimmed.strip_prefix("HERMES_BACKEND_READY port=")?;
    if rest.is_empty() || rest.len() > 5 || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let port: u16 = rest.parse().ok()?;
    if port == 0 { None } else { Some(port) }
}

/// Reads readiness lines from a generic stream until the ready record.
///
/// Test seam for the readiness line protocol; the owner driver
/// ([`drive_service_readiness`]) adds stderr pumping around the same grammar.
#[cfg(test)]
pub(crate) async fn read_ready_port<R: AsyncRead + Unpin>(
    reader: &mut R,
    maximum_line: usize,
    maximum_bytes: usize,
    deadline: Instant,
    cancel: &CancelHandle,
    shutdown: &CancelHandle,
) -> Result<u16, ReadinessError> {
    let mut buffered = BufReader::new(reader);
    let mut line = String::new();
    let mut consumed: usize = 0;
    loop {
        if shutdown.is_cancelled() {
            return Err(ReadinessError::Shutdown);
        }
        if cancel.is_cancelled() {
            return Err(ReadinessError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(ReadinessError::Deadline);
        }
        line.clear();
        let outcome = tokio::select! {
            biased;
            () = shutdown.wait() => Err(ReadinessError::Shutdown),
            () = cancel.wait() => Err(ReadinessError::Cancelled),
            () = tokio::time::sleep_until(deadline) => Err(ReadinessError::Deadline),
            read = buffered.read_line(&mut line) => match read {
                Ok(0) => Err(ReadinessError::EofBeforeNewline),
                Ok(count) => Ok(count),
                Err(_) => Err(ReadinessError::Io),
            },
        };
        let count = outcome?;
        consumed = consumed.saturating_add(count);
        if consumed > maximum_bytes || line.len() > maximum_line {
            return Err(ReadinessError::Io);
        }
        if let Some(port) = parse_ready_port_line(&line) {
            return Ok(port);
        }
    }
}

/// Drives bounded service readiness on the spawned child.
///
/// Mirrors the owner `drive_readiness` discipline: shutdown, cancellation,
/// and the phase deadline win over output, and stderr counting is pumped
/// while waiting so a chatty child cannot wedge the pipe.
pub(crate) async fn drive_service_readiness(
    stdout: &mut tokio::process::ChildStdout,
    parts: &mut ChildParts,
    deadline: Instant,
    shutdown: &CancelHandle,
    control: &CancelHandle,
    maximum_line: usize,
) -> Result<u16, ReadinessError> {
    let mut line = String::new();
    let mut consumed: usize = 0;
    let mut reader = BufReader::new(stdout);
    loop {
        if shutdown.is_cancelled() {
            return Err(ReadinessError::Shutdown);
        }
        if control.is_cancelled() {
            return Err(ReadinessError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(ReadinessError::Deadline);
        }
        line.clear();
        tokio::select! {
            biased;
            () = shutdown.wait() => return Err(ReadinessError::Shutdown),
            () = control.wait() => return Err(ReadinessError::Cancelled),
            () = tokio::time::sleep_until(deadline) => return Err(ReadinessError::Deadline),
            event = parts.stderr_counter.pump(), if parts.stderr_counter.state() == super::process::StderrState::Open => {
                let _ = event;
            }
            waited = parts.child.wait() => {
                match waited {
                    Ok(_) => return Err(ReadinessError::EofBeforeNewline),
                    Err(_) => return Err(ReadinessError::Io),
                }
            }
            read = reader.read_line(&mut line) => {
                match read {
                    Ok(0) => return Err(ReadinessError::EofBeforeNewline),
                    Ok(count) => {
                        consumed = consumed.saturating_add(count);
                        if consumed > HERMES_MAX_READY_BYTES || line.len() > maximum_line {
                            return Err(ReadinessError::Io);
                        }
                        if let Some(port) = parse_ready_port_line(&line) {
                            return Ok(port);
                        }
                    }
                    Err(_) => return Err(ReadinessError::Io),
                }
            }
        }
    }
}

/// Computes SHA-1 over one message (FIPS 180-4, big-endian length suffix).
///
/// Local because no workspace crate provides SHA-1; used only for the
/// `Sec-WebSocket-Accept` handshake check and pinned against the RFC 6455
/// test vector below.
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut state = [
        0x6745_2301_u32,
        0xEFCD_AB89_u32,
        0x98BA_DCFE_u32,
        0x1032_5476_u32,
        0xC3D2_E1F0_u32,
    ];
    let mut padded = message.to_vec();
    let bit_length = u64::try_from(message.len())
        .unwrap_or(u64::MAX)
        .wrapping_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut schedule = [0_u32; 80];
        for (index, bytes) in chunk.chunks_exact(4).enumerate().take(16) {
            schedule[index] = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for index in 16..80 {
            schedule[index] = (schedule[index - 3]
                ^ schedule[index - 8]
                ^ schedule[index - 14]
                ^ schedule[index - 16])
                .rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) =
            (state[0], state[1], state[2], state[3], state[4]);
        for (index, word) in schedule.iter().enumerate() {
            let (f, k) = match index {
                0..20 => ((b & c) | ((!b) & d), 0x5A82_7999_u32),
                20..40 => (b ^ c ^ d, 0x6ED9_EBA1_u32),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC_u32),
                _ => (b ^ c ^ d, 0xCA62_C1D6_u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
    }
    let mut digest = [0_u8; 20];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// Computes the expected `Sec-WebSocket-Accept` value for a client key.
#[must_use]
pub(crate) fn websocket_accept_key(client_key: &str) -> String {
    let material = format!("{client_key}{WEBSOCKET_GUID}");
    base64::engine::general_purpose::STANDARD.encode(sha1(material.as_bytes()))
}

/// Failure decoding or driving one WebSocket frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WsFrameError {
    TooLarge,
    InvalidFrame,
    Closed,
}

/// One decoded WebSocket frame payload.
///
/// Continuation frames stay explicit so the gateway client (and only it)
/// owns reassembly with the same bound; [`read_ws_frame`] never buffers
/// across frames itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WsFrame {
    Text(Vec<u8>, bool),
    Binary(Vec<u8>, bool),
    Continuation(Vec<u8>, bool),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

/// Reads one complete WebSocket frame.
///
/// Rejects reserved bits, fragmented control frames, oversized control
/// payloads, and payloads above `maximum` with typed errors before
/// allocating. A clean end of stream at a frame boundary reports
/// [`WsFrameError::Closed`]; mid-frame truncation reports
/// [`WsFrameError::InvalidFrame`]. Masking applies whenever the mask bit is
/// set, so the same reader serves the client and the fixture server.
///
/// # Errors
///
/// Returns [`WsFrameError`] when the peer closed, the frame is malformed, or
/// the payload exceeds `maximum`.
pub(crate) async fn read_ws_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    maximum: usize,
) -> Result<WsFrame, WsFrameError> {
    let mut header = [0_u8; 2];
    let mut first = [0_u8; 1];
    match reader.read(&mut first).await {
        Ok(0) => return Err(WsFrameError::Closed),
        Ok(_) => {}
        Err(_) => return Err(WsFrameError::InvalidFrame),
    }
    header[0] = first[0];
    reader
        .read_exact(&mut header[1..])
        .await
        .map_err(|_| WsFrameError::InvalidFrame)?;
    let finished = header[0] & 0x80 != 0;
    if header[0] & 0x70 != 0 {
        return Err(WsFrameError::InvalidFrame);
    }
    let opcode = header[0] & 0x0f;
    let masked = header[1] & 0x80 != 0;
    let mut length = u64::from(header[1] & 0x7f);
    if length == 126 {
        let mut extended = [0_u8; 2];
        reader
            .read_exact(&mut extended)
            .await
            .map_err(|_| WsFrameError::InvalidFrame)?;
        length = u64::from(u16::from_be_bytes(extended));
    } else if length == 127 {
        let mut extended = [0_u8; 8];
        reader
            .read_exact(&mut extended)
            .await
            .map_err(|_| WsFrameError::InvalidFrame)?;
        length = u64::from_be_bytes(extended);
    }
    let maximum_u64 = u64::try_from(maximum).unwrap_or(u64::MAX);
    if length > maximum_u64 {
        return Err(WsFrameError::TooLarge);
    }
    let is_control = opcode >= 0x08;
    if is_control && (!finished || length > 125) {
        return Err(WsFrameError::InvalidFrame);
    }
    let mask = if masked {
        let mut key = [0_u8; 4];
        reader
            .read_exact(&mut key)
            .await
            .map_err(|_| WsFrameError::InvalidFrame)?;
        Some(key)
    } else {
        None
    };
    let size = usize::try_from(length).map_err(|_| WsFrameError::TooLarge)?;
    let mut payload = vec![0_u8; size];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|_| WsFrameError::InvalidFrame)?;
    if let Some(key) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= key[index % 4];
        }
    }
    match opcode {
        0x00 => Ok(WsFrame::Continuation(payload, finished)),
        0x01 => Ok(WsFrame::Text(payload, finished)),
        0x02 => Ok(WsFrame::Binary(payload, finished)),
        0x08 => Ok(WsFrame::Close),
        0x09 => Ok(WsFrame::Ping(payload)),
        0x0a => Ok(WsFrame::Pong(payload)),
        _ => Err(WsFrameError::InvalidFrame),
    }
}

/// Writes one masked client text frame.
///
/// Clients must mask; the mask comes from operating-system entropy. Payloads
/// above `maximum` reject before any byte is written.
///
/// # Errors
///
/// Returns [`WsFrameError`] when entropy is unavailable, the payload exceeds
/// `maximum`, or the write fails.
pub(crate) async fn write_client_text<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
    maximum: usize,
) -> Result<(), WsFrameError> {
    if payload.len() > maximum {
        return Err(WsFrameError::TooLarge);
    }
    let mut mask = [0_u8; 4];
    getrandom::fill(&mut mask).map_err(|_| WsFrameError::InvalidFrame)?;
    write_frame(writer, 0x81, Some(mask), payload)
        .await
        .map_err(|_| WsFrameError::InvalidFrame)
}

/// Writes one unmasked server text frame.
///
/// Fixture servers only: production traffic is always client-masked.
///
/// # Errors
///
/// Returns [`WsFrameError`] when the payload exceeds `maximum` or the write
/// fails.
#[cfg(test)]
pub(crate) async fn write_server_text<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
    maximum: usize,
) -> Result<(), WsFrameError> {
    if payload.len() > maximum {
        return Err(WsFrameError::TooLarge);
    }
    write_frame(writer, 0x81, None, payload)
        .await
        .map_err(|_| WsFrameError::InvalidFrame)
}

/// Writes one masked client control frame (pong or close).
async fn write_client_control<W: AsyncWrite + Unpin>(
    writer: &mut W,
    opcode: u8,
    payload: &[u8],
) -> Result<(), WsFrameError> {
    if payload.len() > 125 {
        return Err(WsFrameError::InvalidFrame);
    }
    let mut mask = [0_u8; 4];
    getrandom::fill(&mut mask).map_err(|_| WsFrameError::InvalidFrame)?;
    write_frame(writer, 0x80 | opcode, Some(mask), payload)
        .await
        .map_err(|_| WsFrameError::InvalidFrame)
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    first: u8,
    mask: Option<[u8; 4]>,
    payload: &[u8],
) -> std::io::Result<()> {
    let length = payload.len();
    let mut header = vec![first];
    if length < 126 {
        let size = u8::try_from(length).unwrap_or(u8::MAX);
        header.push(size | if mask.is_some() { 0x80 } else { 0 });
    } else if length < 65536 {
        let size = u16::try_from(length).unwrap_or(u16::MAX);
        header.push(126 | if mask.is_some() { 0x80 } else { 0 });
        header.extend_from_slice(&size.to_be_bytes());
    } else {
        let size = u64::try_from(length).unwrap_or(u64::MAX);
        header.push(127 | if mask.is_some() { 0x80 } else { 0 });
        header.extend_from_slice(&size.to_be_bytes());
    }
    writer.write_all(&header).await?;
    if let Some(key) = mask {
        writer.write_all(&key).await?;
        let masked: Vec<u8> = payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ key[index % 4])
            .collect();
        writer.write_all(&masked).await?;
    } else {
        writer.write_all(payload).await?;
    }
    writer.flush().await
}

/// Payload-free failure of the gateway JSON-RPC boundary.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum GatewayError {
    #[error("hermes gateway handshake failed")]
    Handshake,
    #[error("hermes gateway request timed out")]
    Timeout,
    #[error("hermes gateway connection closed")]
    Closed,
    #[error("hermes gateway frame was malformed")]
    Decode,
    #[error("hermes gateway protocol violated")]
    Protocol,
    #[error("hermes gateway rejected the request")]
    Remote { code: Option<i64> },
    #[error("hermes gateway operation cancelled")]
    Cancelled,
    #[error("owner is shutting down")]
    Shutdown,
    #[error("hermes gateway stream failed")]
    StreamFailed,
}

/// Deadline and cancellation scope for one gateway request.
pub(crate) struct RequestScope<'a> {
    pub(crate) deadline: Instant,
    pub(crate) cancel: &'a CancelHandle,
    pub(crate) shutdown: &'a CancelHandle,
}

/// One typed gateway event after bounded boundary decoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesEvent {
    event_type: String,
    session_id: Option<String>,
    payload: Value,
}

impl HermesEvent {
    /// Returns the gateway event type.
    pub(crate) fn event_type(&self) -> &str {
        &self.event_type
    }

    /// Returns the gateway session identity, when the event carried one.
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Returns the raw event payload for typed extraction.
    pub(crate) fn payload(&self) -> &Value {
        &self.payload
    }
}

/// One decoded JSON-RPC text payload.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DecodedEnvelope {
    Response {
        id: u64,
        result: Value,
        error_code: Option<i64>,
    },
    Event(HermesEvent),
    Ignored,
}

/// Decodes one gateway text payload into a typed envelope.
///
/// Oversized input rejects before parsing; invalid JSON, non-object
/// envelopes, and over-long event types reject with typed errors and never
/// panic. Unknown methods and foreign response ids project to
/// [`DecodedEnvelope::Ignored`] so future gateway additions stay observable
/// without disturbing the turn.
///
/// # Errors
///
/// Returns [`WsFrameError`] when the payload exceeds the frame bound, is not
/// valid JSON, or is not a JSON-RPC envelope.
pub(crate) fn decode_envelope(text: &str) -> Result<DecodedEnvelope, WsFrameError> {
    if text.len() > HERMES_MAX_FRAME_BYTES {
        return Err(WsFrameError::TooLarge);
    }
    let value: Value = serde_json::from_str(text).map_err(|_| WsFrameError::InvalidFrame)?;
    let object = value.as_object().ok_or(WsFrameError::InvalidFrame)?;
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        if method != "event" {
            return Ok(DecodedEnvelope::Ignored);
        }
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let params_object = params.as_object().ok_or(WsFrameError::InvalidFrame)?;
        let event_type = params_object
            .get("type")
            .and_then(Value::as_str)
            .ok_or(WsFrameError::InvalidFrame)?;
        if event_type.is_empty() || event_type.len() > HERMES_MAX_ID_BYTES {
            return Err(WsFrameError::InvalidFrame);
        }
        let session_id = params_object
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)
            .map(str::to_owned);
        let payload = params_object.get("payload").cloned().unwrap_or(Value::Null);
        return Ok(DecodedEnvelope::Event(HermesEvent {
            event_type: event_type.to_owned(),
            session_id,
            payload,
        }));
    }
    let Some(id) = object.get("id").and_then(Value::as_u64) else {
        return Ok(DecodedEnvelope::Ignored);
    };
    if let Some(error) = object.get("error") {
        let code = error.get("code").and_then(Value::as_i64);
        return Ok(DecodedEnvelope::Response {
            id,
            result: Value::Null,
            error_code: Some(code.unwrap_or(0)),
        });
    }
    Ok(DecodedEnvelope::Response {
        id,
        result: object.get("result").cloned().unwrap_or(Value::Null),
        error_code: None,
    })
}

fn frame_to_gateway(error: WsFrameError) -> GatewayError {
    match error {
        WsFrameError::Closed => GatewayError::Closed,
        WsFrameError::TooLarge => GatewayError::Protocol,
        WsFrameError::InvalidFrame => GatewayError::Decode,
    }
}

/// One routed inbound frame: either a gateway event or a request response.
enum RoutedFrame {
    Event(HermesEvent),
    Response {
        id: u64,
        result: Value,
        error_code: Option<i64>,
    },
}

/// Stashed response for an id awaited outside the current read.
struct PendingResponse {
    result: Value,
    error_code: Option<i64>,
}

/// Finite JSON-RPC client over one private loopback WebSocket.
///
/// All traffic is sequential on the owner task: requests await their response
/// while buffering interleaved events, and the streaming pump reads events
/// while stashing stray responses for the next request. Exactly one
/// `gateway.ready` event gates [`GatewayClient::connect`].
pub(crate) struct GatewayClient {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    next_id: u64,
    pending: HashMap<u64, PendingResponse>,
    buffered_events: VecDeque<HermesEvent>,
    fragment: Option<(bool, Vec<u8>)>,
    closed: bool,
}

impl GatewayClient {
    /// Connects to the private gateway and waits for its ready event.
    ///
    /// The handshake carries the dashboard session token as a query
    /// parameter, exactly like the TypeScript service. Only loopback
    /// addresses are dialed; any other address fails closed.
    ///
    /// # Errors
    ///
    /// Returns [`GatewayError`] when the address is not loopback, the
    /// handshake is rejected or malformed, the ready event does not arrive
    /// before `scope` expires, or the operation is cancelled or shut down.
    pub(crate) async fn connect(
        address: SocketAddr,
        token: &str,
        scope: &RequestScope<'_>,
    ) -> Result<Self, GatewayError> {
        if !address.ip().is_loopback() {
            return Err(GatewayError::Handshake);
        }
        if token.is_empty() || token.len() > 256 {
            return Err(GatewayError::Handshake);
        }
        let stream = tokio::select! {
            biased;
            () = scope.shutdown.wait() => return Err(GatewayError::Shutdown),
            () = scope.cancel.wait() => return Err(GatewayError::Cancelled),
            () = tokio::time::sleep_until(scope.deadline) => return Err(GatewayError::Timeout),
            connected = TcpStream::connect(address) => connected.map_err(|_| GatewayError::Handshake)?,
        };
        let mut key_bytes = [0_u8; 16];
        getrandom::fill(&mut key_bytes).map_err(|_| GatewayError::Handshake)?;
        let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
        let expected_accept = websocket_accept_key(&key);
        let request = format!(
            "GET /api/ws?token={token} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            address.ip(),
        );
        let (reader, writer) = stream.into_split();
        let mut client = Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 0,
            pending: HashMap::new(),
            buffered_events: VecDeque::new(),
            fragment: None,
            closed: false,
        };
        client
            .writer
            .write_all(request.as_bytes())
            .await
            .map_err(|_| GatewayError::Handshake)?;
        client
            .writer
            .flush()
            .await
            .map_err(|_| GatewayError::Handshake)?;
        client.read_handshake_head(&expected_accept, scope).await?;
        client.wait_gateway_ready(scope).await?;
        Ok(client)
    }

    async fn read_handshake_head(
        &mut self,
        expected_accept: &str,
        scope: &RequestScope<'_>,
    ) -> Result<(), GatewayError> {
        let mut head = Vec::new();
        loop {
            if head.len() > HERMES_MAX_HANDSHAKE_BYTES {
                return Err(GatewayError::Handshake);
            }
            let chunk = tokio::select! {
                biased;
                () = scope.shutdown.wait() => return Err(GatewayError::Shutdown),
                () = scope.cancel.wait() => return Err(GatewayError::Cancelled),
                () = tokio::time::sleep_until(scope.deadline) => return Err(GatewayError::Timeout),
                read = self.reader.fill_buf() => read.map_err(|_| GatewayError::Handshake)?.to_owned(),
            };
            if chunk.is_empty() {
                return Err(GatewayError::Handshake);
            }
            let take = chunk.len().min(HERMES_MAX_HANDSHAKE_BYTES + 1 - head.len());
            head.extend_from_slice(&chunk[..take]);
            self.reader.consume(take);
            if head.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let text = std::str::from_utf8(&head).map_err(|_| GatewayError::Handshake)?;
        let mut lines = text.split("\r\n");
        let status = lines.next().ok_or(GatewayError::Handshake)?;
        if status.split_whitespace().nth(1) != Some("101") {
            return Err(GatewayError::Handshake);
        }
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("sec-websocket-accept")
                    && value.trim() == expected_accept
                {
                    return Ok(());
                }
            }
        }
        Err(GatewayError::Handshake)
    }

    async fn wait_gateway_ready(&mut self, scope: &RequestScope<'_>) -> Result<(), GatewayError> {
        loop {
            match self.read_routed(scope).await? {
                RoutedFrame::Event(event) if event.event_type() == "gateway.ready" => return Ok(()),
                RoutedFrame::Event(event) => self.buffered_events.push_back(event),
                RoutedFrame::Response {
                    id,
                    result,
                    error_code,
                } => {
                    self.pending
                        .insert(id, PendingResponse { result, error_code });
                }
            }
        }
    }

    /// Sends one JSON-RPC request and awaits its typed result.
    ///
    /// Interleaved gateway events are collected and returned alongside the
    /// result so the caller can project them in order.
    ///
    /// # Errors
    ///
    /// Returns [`GatewayError`] when the write fails, the gateway answers
    /// with a remote error, a frame is malformed, or `scope` expires or is
    /// cancelled or shut down.
    pub(crate) async fn request(
        &mut self,
        method: &str,
        params: Value,
        scope: &RequestScope<'_>,
    ) -> Result<(Value, Vec<HermesEvent>), GatewayError> {
        if self.closed {
            return Err(GatewayError::Closed);
        }
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let line = serde_json::json!({
            "id": id,
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        })
        .to_string();
        write_client_text(&mut self.writer, line.as_bytes(), HERMES_MAX_FRAME_BYTES)
            .await
            .map_err(frame_to_gateway)?;
        if let Some(stashed) = self.pending.remove(&id) {
            return stashed_result(stashed);
        }
        let bounded = RequestScope {
            deadline: scope.deadline.min(Instant::now() + HERMES_REQUEST_TIMEOUT),
            cancel: scope.cancel,
            shutdown: scope.shutdown,
        };
        let mut events = Vec::new();
        loop {
            match self.read_routed(&bounded).await? {
                RoutedFrame::Event(event) => events.push(event),
                RoutedFrame::Response {
                    id: answered,
                    result,
                    error_code,
                } if answered == id => {
                    if let Some(code) = error_code {
                        return Err(GatewayError::Remote { code: Some(code) });
                    }
                    return Ok((result, events));
                }
                RoutedFrame::Response {
                    id,
                    result,
                    error_code,
                } => {
                    self.pending
                        .insert(id, PendingResponse { result, error_code });
                }
            }
        }
    }

    /// Reads the next gateway event, stashing stray responses.
    ///
    /// Carries no deadline of its own: the owner pump selects it against the
    /// attempt deadline, stall budget, cancellation, and shutdown.
    ///
    /// # Errors
    ///
    /// Returns [`GatewayError`] when a frame is malformed or the connection
    /// closes, fails, or is cancelled or shut down.
    pub(crate) async fn next_event(
        &mut self,
        cancel: &CancelHandle,
        shutdown: &CancelHandle,
    ) -> Result<HermesEvent, GatewayError> {
        if let Some(event) = self.buffered_events.pop_front() {
            return Ok(event);
        }
        let scope = RequestScope {
            deadline: Instant::now() + Duration::from_secs(24 * 60 * 60),
            cancel,
            shutdown,
        };
        loop {
            match self.read_routed(&scope).await? {
                RoutedFrame::Event(event) => return Ok(event),
                RoutedFrame::Response {
                    id,
                    result,
                    error_code,
                } => {
                    self.pending
                        .insert(id, PendingResponse { result, error_code });
                }
            }
        }
    }

    /// Closes the gateway connection best-effort (close frame, then TCP).
    pub(crate) async fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let _ = write_client_control(&mut self.writer, 0x08, &[]).await;
        let _ = self.writer.shutdown().await;
    }

    async fn read_routed(&mut self, scope: &RequestScope<'_>) -> Result<RoutedFrame, GatewayError> {
        loop {
            let frame = tokio::select! {
                biased;
                () = scope.shutdown.wait() => return Err(GatewayError::Shutdown),
                () = scope.cancel.wait() => return Err(GatewayError::Cancelled),
                () = tokio::time::sleep_until(scope.deadline) => return Err(GatewayError::Timeout),
                frame = read_ws_frame(&mut self.reader, HERMES_MAX_FRAME_BYTES) => frame.map_err(frame_to_gateway)?,
            };
            match frame {
                WsFrame::Ping(payload) => {
                    let _ = write_client_control(&mut self.writer, 0x0a, &payload).await;
                }
                WsFrame::Pong(_) => {}
                WsFrame::Close => {
                    self.closed = true;
                    return Err(GatewayError::Closed);
                }
                WsFrame::Binary(_, _) | WsFrame::Text(_, _) | WsFrame::Continuation(_, _) => {
                    if let Some(bytes) = self.assemble(frame)? {
                        let text = std::str::from_utf8(&bytes).map_err(|_| GatewayError::Decode)?;
                        match decode_envelope(text).map_err(|_| GatewayError::Decode)? {
                            DecodedEnvelope::Event(event) => {
                                return Ok(RoutedFrame::Event(event));
                            }
                            DecodedEnvelope::Response {
                                id,
                                result,
                                error_code,
                            } => {
                                return Ok(RoutedFrame::Response {
                                    id,
                                    result,
                                    error_code,
                                });
                            }
                            DecodedEnvelope::Ignored => {}
                        }
                    }
                }
            }
        }
    }

    /// Assembles fragmented text frames; binary payloads reject.
    ///
    /// Returns the completed text bytes once the final fragment arrives.
    fn assemble(&mut self, frame: WsFrame) -> Result<Option<Vec<u8>>, GatewayError> {
        match frame {
            WsFrame::Text(bytes, finished) => {
                if self.fragment.is_some() {
                    return Err(GatewayError::Protocol);
                }
                if finished {
                    return Ok(Some(bytes));
                }
                self.fragment = Some((true, bytes));
                Ok(None)
            }
            WsFrame::Binary(_, _) => Err(GatewayError::Protocol),
            WsFrame::Continuation(bytes, finished) => {
                let Some((is_text, mut buffer)) = self.fragment.take() else {
                    return Err(GatewayError::Protocol);
                };
                if !is_text {
                    return Err(GatewayError::Protocol);
                }
                if buffer.len().saturating_add(bytes.len()) > HERMES_MAX_FRAME_BYTES {
                    return Err(GatewayError::Protocol);
                }
                buffer.extend_from_slice(&bytes);
                if finished {
                    return Ok(Some(buffer));
                }
                self.fragment = Some((true, buffer));
                Ok(None)
            }
            WsFrame::Ping(_) | WsFrame::Pong(_) | WsFrame::Close => Err(GatewayError::Protocol),
        }
    }
}

fn stashed_result(stashed: PendingResponse) -> Result<(Value, Vec<HermesEvent>), GatewayError> {
    if let Some(code) = stashed.error_code {
        return Err(GatewayError::Remote { code: Some(code) });
    }
    Ok((stashed.result, Vec::new()))
}

/// Builds the guidance seed messages for `session.create`.
///
/// Mirrors the TypeScript seed: product instructions and workspace guidance
/// join with a blank line into one system history entry. An empty section
/// list sends no messages.
///
/// # Panics
///
/// Never panics; sections are joined verbatim.
#[must_use]
pub(crate) fn guidance_seed_messages(sections: &[String]) -> Vec<Value> {
    if sections.is_empty() {
        return Vec::new();
    }
    vec![serde_json::json!({
        "role": "system",
        "content": sections.join("\n\n"),
    })]
}

/// Rejects image attachments with a typed error.
///
/// The Hermes catalog reports `image_input: false`, so any attachment fails
/// the turn closed here instead of sending a degraded text-only prompt.
///
/// # Errors
///
/// Returns [`HermesTurnError::ImagesUnsupported`] when the prompt carries any
/// image attachment.
pub(crate) fn reject_image_attachments(
    prompt: &artisan_domain::QueueMessagePayload,
) -> Result<(), HermesTurnError> {
    if prompt.attachments().is_empty() {
        Ok(())
    } else {
        Err(HermesTurnError::ImagesUnsupported)
    }
}

/// Returns whether a silent active turn has stalled past its inactivity
/// deadline. Idle sessions between turns are silent by design and never
/// stall; only a turn already in flight owes output.
#[must_use]
pub(crate) fn has_stalled(
    turn_active: bool,
    last_activity: Instant,
    inactivity: Duration,
    now: Instant,
) -> bool {
    turn_active && now.saturating_duration_since(last_activity) >= inactivity
}

fn current_unix_millis() -> Option<UnixMillis> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Some(UnixMillis::from_millis(millis))
}

/// One validated provider/model pair from `model.options`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InventoryEntry {
    provider: String,
    model: String,
    enabled: bool,
}

/// Validated live `model.options` inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidatedInventory {
    entries: Vec<InventoryEntry>,
}

/// Failure validating the live `model.options` inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum InventoryError {
    #[error("hermes model inventory was malformed")]
    InvalidShape,
    #[error("hermes model inventory contained a duplicate model")]
    DuplicateModel,
}

/// Validates the live `model.options` inventory without inferring fields.
///
/// Requires the top-level `providers` array and well-typed required fields;
/// entries missing their slug, name, or model list are skipped because they
/// cannot be attributed, while present-but-mistyped fields reject the whole
/// inventory fail-closed. Duplicate `(provider, model)` pairs reject:
/// either the gateway or the transport tampered with the rows. Unknown extra
/// fields are ignored, never inferred into identities, prices, or flags.
///
/// # Errors
///
/// Returns [`InventoryError`] when the shape is not a provider inventory or
/// a provider/model pair repeats.
pub(crate) fn validate_model_options_inventory(
    value: &Value,
) -> Result<ValidatedInventory, InventoryError> {
    let providers = value
        .get("providers")
        .and_then(Value::as_array)
        .ok_or(InventoryError::InvalidShape)?;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for provider in providers {
        let object = match provider.as_object() {
            Some(object) => object,
            None => continue,
        };
        let (Some(slug), Some(name), Some(models)) = (
            object.get("slug").and_then(Value::as_str),
            object.get("name").and_then(Value::as_str),
            object.get("models").and_then(Value::as_array),
        ) else {
            continue;
        };
        if slug.is_empty() || name.is_empty() {
            continue;
        }
        let authenticated = object
            .get("authenticated")
            .map(|value| value.as_bool().ok_or(InventoryError::InvalidShape))
            .transpose()?
            .unwrap_or(true);
        let unavailable = match object.get("unavailable_models") {
            None => HashSet::new(),
            Some(list) => list
                .as_array()
                .ok_or(InventoryError::InvalidShape)?
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .ok_or(InventoryError::InvalidShape)
                        .map(str::to_owned)
                })
                .collect::<Result<HashSet<_>, _>>()?,
        };
        for model in models {
            let model_id = model.as_str().ok_or(InventoryError::InvalidShape)?;
            if model_id.is_empty() {
                return Err(InventoryError::InvalidShape);
            }
            if !seen.insert((slug.to_owned(), model_id.to_owned())) {
                return Err(InventoryError::DuplicateModel);
            }
            entries.push(InventoryEntry {
                provider: slug.to_owned(),
                model: model_id.to_owned(),
                enabled: authenticated && !unavailable.contains(model_id),
            });
        }
    }
    Ok(ValidatedInventory { entries })
}

/// Returns whether the validated inventory enables the selected route/model.
#[must_use]
pub(crate) fn inventory_supports(inventory: &ValidatedInventory, route: &str, model: &str) -> bool {
    inventory
        .entries
        .iter()
        .any(|entry| entry.enabled && entry.provider == route && entry.model == model)
}

/// Failure opening one Hermes session.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum SessionError {
    #[error("hermes session misconfigured")]
    Configuration,
    #[error("hermes provider request failed")]
    ProviderRequestFailed,
    #[error("hermes desktop contract incompatible")]
    IncompatibleVersion,
    #[error("hermes gateway operation cancelled")]
    Cancelled,
    #[error("owner is shutting down")]
    Shutdown,
    #[error("hermes gateway deadline elapsed")]
    Deadline,
    #[error("hermes gateway stream failed")]
    StreamFailed,
}

fn map_gateway_to_session(error: GatewayError) -> SessionError {
    match error {
        GatewayError::Shutdown => SessionError::Shutdown,
        GatewayError::Cancelled => SessionError::Cancelled,
        GatewayError::Timeout => SessionError::Deadline,
        GatewayError::Closed | GatewayError::Decode | GatewayError::Protocol => {
            SessionError::ProviderRequestFailed
        }
        GatewayError::Remote { .. } => SessionError::ProviderRequestFailed,
        GatewayError::Handshake | GatewayError::StreamFailed => SessionError::StreamFailed,
    }
}

/// Input for opening one Hermes session.
pub(crate) struct OpenSessionInput<'a> {
    pub(crate) settings: &'a HermesSettings,
    pub(crate) project_root: &'a str,
    pub(crate) guidance_sections: &'a [String],
    pub(crate) resume_stored_session_id: Option<&'a str>,
}

/// Opened Hermes session identities.
///
/// The runtime id scopes gateway traffic; the durable id is the stored
/// session the dispatcher binds with tag `hermes` format 1. Setup-phase
/// events interleaved with the open requests ride along for ordered
/// projection after authorization.
pub(crate) struct OpenedSession {
    pub(crate) runtime_session_id: String,
    pub(crate) durable_session_id: String,
    pub(crate) setup_events: Vec<HermesEvent>,
}

/// Returns whether a resumed session keeps the identical model selection.
///
/// Mirrors `CheckNativeContinuation`: a resume is compatible only when the
/// gateway-reported model and provider both match the current selection (or
/// the gateway disclosed neither). Anything else fails closed with a
/// configuration error at the caller.
#[must_use]
pub(crate) fn resume_selection_matches(
    model_id: &str,
    route_id: &str,
    info_model: Option<&str>,
    info_provider: Option<&str>,
) -> bool {
    if let Some(reported) = info_model {
        if reported != model_id {
            return false;
        }
    }
    if let Some(reported) = info_provider {
        if reported != route_id {
            return false;
        }
    }
    true
}

fn bounded_session_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)
        .map(str::to_owned)
}

/// Opens one Hermes session: create with the guidance seed, or resume with
/// original-model enforcement.
///
/// Both paths set the `yolo` session flag from the durable permission mode
/// exactly like the TypeScript engine, then validate the desktop contract
/// floor. Resume compares the gateway-reported model and provider against the
/// current selection and fails closed on any mismatch.
///
/// # Errors
///
/// Returns [`SessionError`] when the gateway misbehaves, the contract is too
/// old, a resume changes the model selection, or `scope` expires or is
/// cancelled or shut down.
pub(crate) async fn open_session(
    client: &mut GatewayClient,
    input: &OpenSessionInput<'_>,
    scope: &RequestScope<'_>,
) -> Result<OpenedSession, SessionError> {
    let profile = if input.settings.profile_id() == "default" {
        None
    } else {
        Some(input.settings.profile_id().to_owned())
    };
    let response = if let Some(stored) = input.resume_stored_session_id {
        let mut params = serde_json::Map::new();
        params.insert("close_on_disconnect".to_owned(), Value::Bool(false));
        params.insert("defer_history".to_owned(), Value::Bool(true));
        params.insert("omit_messages".to_owned(), Value::Bool(true));
        if let Some(profile) = profile {
            params.insert("profile".to_owned(), Value::String(profile));
        }
        params.insert("session_id".to_owned(), Value::String(stored.to_owned()));
        params.insert("source".to_owned(), Value::String("artisan".to_owned()));
        client
            .request("session.resume", Value::Object(params), scope)
            .await
            .map_err(map_gateway_to_session)?
    } else {
        let guidance = guidance_seed_messages(input.guidance_sections);
        let params = input.settings.create_params(input.project_root, &guidance);
        client
            .request("session.create", params, scope)
            .await
            .map_err(map_gateway_to_session)?
    };
    let (result, mut setup_events) = response;
    let object = result
        .as_object()
        .ok_or(SessionError::ProviderRequestFailed)?;
    let runtime_session_id = object
        .get("session_id")
        .and_then(bounded_session_id)
        .ok_or(SessionError::ProviderRequestFailed)?;
    if let Some(contract) = object
        .get("info")
        .and_then(Value::as_object)
        .and_then(|info| info.get("desktop_contract"))
        .and_then(Value::as_u64)
    {
        if contract < HERMES_MINIMUM_DESKTOP_CONTRACT {
            return Err(SessionError::IncompatibleVersion);
        }
    }
    if input.resume_stored_session_id.is_some() {
        let info = object.get("info");
        let reported_model = info
            .and_then(|info| info.get("model"))
            .and_then(Value::as_str);
        let reported_provider = info
            .and_then(|info| info.get("provider"))
            .and_then(Value::as_str);
        if !resume_selection_matches(
            input.settings.model_id(),
            input.settings.route_id(),
            reported_model,
            reported_provider,
        ) {
            return Err(SessionError::Configuration);
        }
        let durable = input
            .resume_stored_session_id
            .map(str::to_owned)
            .ok_or(SessionError::ProviderRequestFailed)?;
        let permission_events =
            set_session_permission(client, &runtime_session_id, input.settings, scope).await?;
        setup_events.extend(permission_events);
        return Ok(OpenedSession {
            runtime_session_id,
            durable_session_id: durable,
            setup_events,
        });
    }
    let durable_session_id = object
        .get("stored_session_id")
        .and_then(bounded_session_id)
        .or_else(|| object.get("session_key").and_then(bounded_session_id))
        .ok_or(SessionError::ProviderRequestFailed)?;
    let permission_events =
        set_session_permission(client, &runtime_session_id, input.settings, scope).await?;
    setup_events.extend(permission_events);
    Ok(OpenedSession {
        runtime_session_id,
        durable_session_id,
        setup_events,
    })
}

async fn set_session_permission(
    client: &mut GatewayClient,
    runtime_session_id: &str,
    settings: &HermesSettings,
    scope: &RequestScope<'_>,
) -> Result<Vec<HermesEvent>, SessionError> {
    let params = serde_json::json!({
        "key": "yolo",
        "scope": "session",
        "session_id": runtime_session_id,
        "value": if settings.permission_yolo { "on" } else { "off" },
    });
    let (_, events) = client
        .request("config.set", params, scope)
        .await
        .map_err(map_gateway_to_session)?;
    Ok(events)
}

/// Sends follow-up text to a live turn (`session.steer`).
///
/// Test-only until dispatcher steer wiring lands: proves the steer verb
/// against the fixture gateway without disturbing the authorize-once
/// production flow.
///
/// # Errors
///
/// Returns [`HermesTurnError`] when the gateway fails or `scope` expires.
#[cfg(test)]
pub(crate) async fn steer_live_turn(
    client: &mut GatewayClient,
    runtime_session_id: &str,
    text: &str,
    scope: &RequestScope<'_>,
) -> Result<(), HermesTurnError> {
    let params = serde_json::json!({
        "session_id": runtime_session_id,
        "text": text,
    });
    client
        .request("session.steer", params, scope)
        .await
        .map_err(|_| HermesTurnError::StreamFailed)?;
    Ok(())
}

/// Interrupts a live turn (`session.interrupt`) before cancelling the driver.
pub(crate) async fn interrupt_live_turn(
    client: &mut GatewayClient,
    runtime_session_id: &str,
    scope: &RequestScope<'_>,
) -> Result<(), HermesTurnError> {
    let params = serde_json::json!({
        "session_id": runtime_session_id,
    });
    client
        .request("session.interrupt", params, scope)
        .await
        .map_err(|_| HermesTurnError::StreamFailed)?;
    Ok(())
}

/// One typed Hermes approval request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesApprovalRequest {
    approval_id: String,
    description: String,
    command: Option<String>,
}

impl HermesApprovalRequest {
    /// Maps this request onto the domain approval vocabulary.
    ///
    /// The owner validates every request through this constructor before
    /// tracking it, so an out-of-bound provider frame never reaches the
    /// durable A-approve rows.
    ///
    /// # Errors
    ///
    /// Returns [`HermesTurnError::Configuration`] when the bounded fields
    /// violate domain ceilings.
    pub(crate) fn to_domain_request(&self) -> Result<ApprovalRequest, HermesTurnError> {
        if let Some(command) = self.command.clone() {
            ApprovalRequest::command(command, None, Some(self.description.clone()))
                .map_err(|_| HermesTurnError::Configuration)
        } else {
            ApprovalRequest::action(Some(self.description.clone()))
                .map_err(|_| HermesTurnError::Configuration)
        }
    }
}

/// One typed Hermes question.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesQuestion {
    question_id: String,
    question_key: Option<String>,
    text: String,
    multi_select: bool,
    options: Vec<String>,
}

impl HermesQuestion {
    /// Returns the provider question identity.
    pub(crate) fn question_id(&self) -> &str {
        &self.question_id
    }

    /// Maps this question onto the domain question vocabulary.
    ///
    /// # Errors
    ///
    /// Returns [`HermesTurnError::Configuration`] when identities, text, or
    /// options violate domain ceilings.
    pub(crate) fn to_domain_input(&self) -> Result<QuestionInput, HermesTurnError> {
        let question_id = ObservationId::parse(self.question_id.clone())
            .map_err(|_| HermesTurnError::Configuration)?;
        if self.text.trim().is_empty() {
            return Err(HermesTurnError::Configuration);
        }
        let mut options = Vec::new();
        for label in &self.options {
            options.push(
                QuestionOption::new(label.clone(), None)
                    .map_err(|_| HermesTurnError::Configuration)?,
            );
        }
        Ok(QuestionInput {
            question_id,
            text: self.text.clone(),
            header: Some("Hermes question".to_owned()),
            multi_select: self.multi_select,
            options: if options.is_empty() {
                None
            } else {
                Some(options)
            },
        })
    }
}

/// One typed Hermes question request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesQuestionRequest {
    request_id: String,
    questions: Vec<HermesQuestion>,
}

impl HermesQuestionRequest {
    /// Returns the questions in this request group.
    pub(crate) fn questions(&self) -> &[HermesQuestion] {
        &self.questions
    }
}

fn bounded_text(value: &Value, field: &str) -> Option<String> {
    let text = value.get(field)?.as_str()?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > HERMES_MAX_TEXT_FIELD_BYTES {
        return None;
    }
    Some(trimmed.to_owned())
}

/// Decodes one `approval.request` event into a typed request.
pub(crate) fn decode_approval(event: &HermesEvent) -> Option<HermesApprovalRequest> {
    if event.event_type() != "approval.request" {
        return None;
    }
    let payload = event.payload().as_object()?;
    let id = payload
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)?;
    let command = payload
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("Hermes requests approval to continue.")
        .to_owned();
    Some(HermesApprovalRequest {
        approval_id: id.to_owned(),
        description,
        command,
    })
}

/// Decodes one `clarify.request` event into typed question requests.
pub(crate) fn decode_questions(event: &HermesEvent) -> Vec<HermesQuestionRequest> {
    if event.event_type() != "clarify.request" {
        return Vec::new();
    }
    let Some(payload) = event.payload().as_object() else {
        return Vec::new();
    };
    let Some(request_id) = payload
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)
    else {
        return Vec::new();
    };
    if let Some(items) = payload.get("questions").and_then(Value::as_array) {
        let mut questions = Vec::new();
        for item in items.iter().take(HERMES_MAX_QUESTIONS_PER_FRAME) {
            if let Some(question) = decode_listed_question(request_id, item) {
                questions.push(question);
            }
        }
        if questions.is_empty() {
            return Vec::new();
        }
        return vec![HermesQuestionRequest {
            request_id: request_id.to_owned(),
            questions,
        }];
    }
    let Some(question) = decode_single_question(request_id, payload) else {
        return Vec::new();
    };
    vec![HermesQuestionRequest {
        request_id: request_id.to_owned(),
        questions: vec![question],
    }]
}

fn decode_listed_question(request_id: &str, item: &Value) -> Option<HermesQuestion> {
    let object = item.as_object()?;
    let key = object
        .get("qid")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty() && key.len() <= HERMES_MAX_ID_BYTES)?;
    let text = bounded_text(item, "question")?;
    Some(HermesQuestion {
        question_id: format!("{request_id}:{key}"),
        question_key: Some(key.to_owned()),
        text,
        multi_select: object
            .get("multi_select")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        options: decode_options(object.get("choices")),
    })
}

fn decode_single_question(
    request_id: &str,
    payload: &serde_json::Map<String, Value>,
) -> Option<HermesQuestion> {
    let value = Value::Object(payload.clone());
    let text = bounded_text(&value, "question")?;
    Some(HermesQuestion {
        question_id: request_id.to_owned(),
        question_key: None,
        text,
        multi_select: payload
            .get("multi_select")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        options: decode_options(payload.get("choices")),
    })
}

fn decode_options(choices: Option<&Value>) -> Vec<String> {
    choices
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(HERMES_MAX_OPTIONS_PER_QUESTION)
                .filter_map(|choice| {
                    choice
                        .as_str()
                        .filter(|label| {
                            !label.trim().is_empty() && label.len() <= HERMES_MAX_TEXT_FIELD_BYTES
                        })
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// In-memory pending interaction tracker for one live Hermes turn.
///
/// Approval requests land as pending approvals and `clarify.request` frames
/// land as pending questions. Resolutions apply through the durable resolve
/// path; a deny records the decision with no turn side effect while the run
/// continues. Subagent discoveries are retained for transcript projection but
/// never adopt the root turn.
#[derive(Debug, Default)]
pub(crate) struct HermesPendingTracker {
    approvals: HashMap<String, HermesApprovalRequest>,
    questions: HashMap<String, HermesQuestion>,
    subagents: Vec<(String, String)>,
}

impl HermesPendingTracker {
    /// Creates an empty tracker for one turn.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Notes one approval request; re-noting the same id is a no-op.
    ///
    /// The request is validated through the domain constructor first, so an
    /// out-of-bound provider frame never reaches the durable A-approve rows.
    pub(crate) fn note_approval(&mut self, request: HermesApprovalRequest) -> bool {
        if request.to_domain_request().is_err() {
            return false;
        }
        if self.approvals.contains_key(&request.approval_id) {
            return false;
        }
        self.approvals.insert(request.approval_id.clone(), request);
        true
    }

    /// Notes one question request group; re-noting the same id is a no-op.
    ///
    /// Each question is validated through the domain constructor first, so
    /// an out-of-bound provider frame never reaches the durable rows.
    pub(crate) fn note_questions(&mut self, request: &HermesQuestionRequest) -> usize {
        let mut added = 0;
        for question in request.questions() {
            if question.to_domain_input().is_err() {
                continue;
            }
            if !self.questions.contains_key(question.question_id()) {
                self.questions
                    .insert(question.question_id().to_owned(), question.clone());
                added += 1;
            }
        }
        added
    }

    /// Notes one subagent discovery without adopting the root turn.
    pub(crate) fn note_subagent(&mut self, agent_thread_id: &str, parent_thread_id: &str) {
        self.subagents
            .push((agent_thread_id.to_owned(), parent_thread_id.to_owned()));
    }

    /// Resolves one approval; returns whether it was pending.
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn resolve_approval(&mut self, approval_id: &str) -> bool {
        self.approvals.remove(approval_id).is_some()
    }

    /// Resolves one question; returns whether it was pending.
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn resolve_question(&mut self, question_id: &str) -> bool {
        self.questions.remove(question_id).is_some()
    }

    /// Returns the number of pending approvals.
    #[cfg(test)]
    pub(crate) fn pending_approvals(&self) -> usize {
        self.approvals.len()
    }

    /// Returns the number of pending questions.
    #[cfg(test)]
    pub(crate) fn pending_questions(&self) -> usize {
        self.questions.len()
    }

    /// Returns the number of discovered subagents.
    #[cfg(test)]
    pub(crate) fn subagent_count(&self) -> usize {
        self.subagents.len()
    }
}

/// Answers one pending approval through the durable decision.
///
/// Test-only until dispatcher delivery wiring lands: deny carries no turn
/// side effect and the run continues either way.
///
/// # Errors
///
/// Returns [`HermesTurnError::Configuration`] for an unknown or resolved
/// target and [`HermesTurnError::StreamFailed`] when the gateway fails.
#[cfg(test)]
pub(crate) async fn answer_approval(
    client: &mut GatewayClient,
    tracker: &mut HermesPendingTracker,
    runtime_session_id: &str,
    approval_id: &str,
    approved: bool,
    scope: &RequestScope<'_>,
) -> Result<(), HermesTurnError> {
    if !tracker.resolve_approval(approval_id) {
        return Err(HermesTurnError::Configuration);
    }
    let params = serde_json::json!({
        "choice": if approved { "once" } else { "deny" },
        "request_id": approval_id,
        "session_id": runtime_session_id,
    });
    client
        .request("approval.respond", params, scope)
        .await
        .map_err(|_| HermesTurnError::StreamFailed)?;
    Ok(())
}

/// Answers one question request group through the durable answers.
///
/// Test-only until dispatcher delivery wiring lands.
///
/// # Errors
///
/// Returns [`HermesTurnError::Configuration`] for an unknown or resolved
/// target and [`HermesTurnError::StreamFailed`] when the gateway fails.
#[cfg(test)]
pub(crate) async fn answer_questions(
    client: &mut GatewayClient,
    tracker: &mut HermesPendingTracker,
    runtime_session_id: &str,
    request: &HermesQuestionRequest,
    answers: &[(String, Vec<String>)],
    scope: &RequestScope<'_>,
) -> Result<(), HermesTurnError> {
    for (question_id, _) in answers {
        if !tracker.resolve_question(question_id) {
            return Err(HermesTurnError::Configuration);
        }
    }
    for (question_id, options) in answers.iter().take(HERMES_MAX_ANSWERS) {
        let Some(question) = request
            .questions()
            .iter()
            .find(|known| known.question_id() == question_id)
        else {
            return Err(HermesTurnError::Configuration);
        };
        let mut params = serde_json::Map::new();
        params.insert("answer".to_owned(), Value::String(options.join(", ")));
        if let Some(key) = question.question_key.clone() {
            params.insert("question_id".to_owned(), Value::String(key));
        }
        params.insert(
            "request_id".to_owned(),
            Value::String(request.request_id.clone()),
        );
        params.insert(
            "session_id".to_owned(),
            Value::String(runtime_session_id.to_owned()),
        );
        client
            .request("clarify.respond", Value::Object(params), scope)
            .await
            .map_err(|_| HermesTurnError::StreamFailed)?;
    }
    Ok(())
}

/// Cumulative usage sample from one gateway usage payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageSample {
    input: Option<u64>,
    output: Option<u64>,
    context: Option<u64>,
    context_max: Option<u64>,
}

/// Extracts the cumulative usage sample from a gateway payload.
///
/// Reads `usage` when present, else the payload itself; accepts both the
/// short (`input`/`output`) and long (`input_tokens`/`output_tokens`)
/// spellings. Returns `None` when no token field carries a value: Hermes
/// reports cumulative totals, and an empty measurement is not a report.
///
/// Hermes `cost_usd` has no S1a usage row and is dropped at the boundary.
pub(crate) fn usage_sample(payload: &Value) -> Option<UsageSample> {
    let source = payload
        .get("usage")
        .filter(|usage| usage.is_object())
        .unwrap_or(payload);
    let number = |fields: &[&str]| {
        fields
            .iter()
            .find_map(|field| source.get(*field).and_then(Value::as_u64))
    };
    let sample = UsageSample {
        input: number(&["input", "input_tokens"]),
        output: number(&["output", "output_tokens"]),
        context: number(&["context_used"]),
        context_max: number(&["context_max"]),
    };
    if sample.input.is_none()
        && sample.output.is_none()
        && sample.context.is_none()
        && sample.context_max.is_none()
    {
        return None;
    }
    Some(sample)
}

/// Builds the cumulative usage report for one sample.
///
/// Fails closed (`None`) when the thread scope is missing, the selection
/// identities do not parse, or the report violates domain bounds: usage is
/// never synthesized from partial identities.
pub(crate) fn usage_report(
    run_id: &RunId,
    thread_id: Option<&ThreadId>,
    provider_session: &str,
    provider_turn: Option<String>,
    source_sequence: u64,
    settings: &HermesSettings,
    sample: &UsageSample,
    observed_at: UnixMillis,
) -> Option<RunUsageReport> {
    let thread_id = thread_id.cloned()?;
    let model_id = EngineModelId::parse(settings.model_id().to_owned()).ok()?;
    let provider_route_id = EngineRouteId::parse(settings.route_id().to_owned()).ok()?;
    RunUsageReport::new(RunUsageReportInput {
        run_id: run_id.clone(),
        thread_id,
        provider_session_id: provider_session.to_owned(),
        source_sequence,
        model_id,
        provider_route_id,
        variant_id: None,
        basis: RunUsageBasis::Cumulative,
        provider_turn_id: provider_turn,
        input_tokens: sample.input,
        cached_input_tokens: None,
        output_tokens: sample.output,
        context_tokens: sample.context,
        context_window_tokens: sample.context_max,
        observed_at,
    })
    .ok()
}

/// One normalized Hermes observation for the owner pump.
pub(crate) enum HermesObservation {
    Text(String),
    Usage(UsageSample),
    Terminal(TerminalState),
    Subagent(SubagentObservation),
    Approval(HermesApprovalRequest),
    Question(HermesQuestionRequest),
}

/// Stateful projection from Hermes gateway events into owner observations.
///
/// Mirrors the TypeScript normalizer: message deltas and interim/final text
/// project to text, usage payloads project to cumulative samples, terminal
/// failures project distinctly, subagent lifecycles project without adopting
/// the root turn, and approvals/questions decode for the pending tracker.
/// Reasoning deltas, tool frames, and compaction markers carry no S1a row.
pub(crate) struct HermesNormalizer {
    turn_index: u64,
    frame_sequence: u64,
    active_compaction: Option<String>,
    compaction_index: u64,
}

impl HermesNormalizer {
    /// Creates a normalizer for one turn.
    pub(crate) fn new() -> Self {
        Self {
            turn_index: 0,
            frame_sequence: 0,
            active_compaction: None,
            compaction_index: 0,
        }
    }

    /// Returns the current provider turn identity for usage attribution.
    pub(crate) fn turn_label(&self, run_id: &RunId) -> String {
        format!("{}:turn:{}", run_id.as_str(), self.turn_index)
    }

    /// Normalizes one gateway event into owner observations.
    pub(crate) fn normalize(
        &mut self,
        run_id: &RunId,
        event: &HermesEvent,
    ) -> Vec<HermesObservation> {
        self.frame_sequence = self.frame_sequence.wrapping_add(1);
        let sequence = self.frame_sequence;
        let payload = event.payload().as_object();
        if self.active_compaction.is_some() && is_compaction_resume(event) {
            self.active_compaction = None;
        }
        match event.event_type() {
            "message.start" => {
                self.turn_index = self.turn_index.wrapping_add(1);
                Vec::new()
            }
            "message.delta" => match payload
                .and_then(|object| {
                    object
                        .get("text")
                        .or_else(|| object.get("rendered"))
                        .and_then(Value::as_str)
                })
                .filter(|text| !text.is_empty())
            {
                Some(delta) => vec![HermesObservation::Text(delta.to_owned())],
                None => Vec::new(),
            },
            "message.interim" => match payload
                .and_then(|object| object.get("text"))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                Some(text) => vec![HermesObservation::Text(text.to_owned())],
                None => Vec::new(),
            },
            "message.complete" => {
                let mut out = Vec::new();
                if let Some(text) = payload
                    .and_then(|object| object.get("text"))
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    out.push(HermesObservation::Text(text.to_owned()));
                }
                if let Some(sample) = usage_sample(event.payload()) {
                    out.push(HermesObservation::Usage(sample));
                }
                let failed = payload
                    .and_then(|object| object.get("failure_reason"))
                    .and_then(Value::as_str)
                    .is_some();
                self.active_compaction = None;
                out.push(HermesObservation::Terminal(if failed {
                    TerminalState::Failed
                } else {
                    TerminalState::Completed
                }));
                out
            }
            "thinking.delta" | "reasoning.delta" | "reasoning.available" => Vec::new(),
            "tool.start" | "tool.progress" | "tool.generating" | "tool.complete" => Vec::new(),
            "approval.request" => match decode_approval(event) {
                Some(request) => vec![HermesObservation::Approval(request)],
                None => Vec::new(),
            },
            "clarify.request" => decode_questions(event)
                .into_iter()
                .map(HermesObservation::Question)
                .collect(),
            "status.update" => {
                let compacting = payload
                    .and_then(|object| object.get("kind"))
                    .and_then(Value::as_str)
                    == Some("compacting");
                if compacting && self.active_compaction.is_none() {
                    self.compaction_index = self.compaction_index.wrapping_add(1);
                    self.active_compaction = Some(format!(
                        "{}:turn:{}:compaction:{}",
                        run_id.as_str(),
                        self.turn_index,
                        self.compaction_index
                    ));
                }
                Vec::new()
            }
            "session.usage" => match usage_sample(event.payload()) {
                Some(sample) => vec![HermesObservation::Usage(sample)],
                None => Vec::new(),
            },
            "subagent.spawn_requested"
            | "subagent.start"
            | "subagent.progress"
            | "subagent.complete" => match subagent_row(run_id, event, sequence) {
                Some(row) => vec![HermesObservation::Subagent(row)],
                None => Vec::new(),
            },
            "error" => vec![HermesObservation::Terminal(TerminalState::Failed)],
            _ => Vec::new(),
        }
    }
}

/// Events the gateway itself treats as proof that a compacted turn resumed.
fn is_compaction_resume(event: &HermesEvent) -> bool {
    if event.event_type() == "status.update" {
        return event.payload().get("kind").and_then(Value::as_str) == Some("compacted");
    }
    matches!(
        event.event_type(),
        "message.start"
            | "message.delta"
            | "message.interim"
            | "thinking.delta"
            | "reasoning.delta"
            | "reasoning.available"
            | "moa.reference"
            | "moa.aggregating"
            | "moa.progress"
            | "moa.phase"
            | "tool.start"
            | "tool.progress"
            | "tool.generating"
            | "tool.complete"
    )
}

fn subagent_row(run_id: &RunId, event: &HermesEvent, sequence: u64) -> Option<SubagentObservation> {
    let payload = event.payload().as_object()?;
    let agent_id = payload
        .get("subagent_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)?;
    let parent = payload
        .get("parent_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)
        .or_else(|| event.session_id())
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)?;
    let status = payload.get("status").and_then(Value::as_str);
    let state = match event.event_type() {
        "subagent.complete" if status == Some("failed") => SubagentState::Failed,
        "subagent.complete" => SubagentState::Completed,
        "subagent.spawn_requested" => SubagentState::Discovered,
        _ => SubagentState::Running,
    };
    let activity = payload
        .get("goal")
        .or_else(|| payload.get("summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let id = ObservationId::parse(format!(
        "{}:hermes:subagent:{agent_id}:{sequence}",
        run_id.as_str()
    ))
    .ok()?;
    let observation_sequence = ObservationSequence::new(sequence).ok()?;
    SubagentObservation::new(
        id,
        observation_sequence,
        SubagentInput {
            agent_native_thread_id: ObservationId::parse(agent_id).ok()?,
            parent_native_thread_id: ObservationId::parse(parent).ok()?,
            state,
            activity,
            agent_path: None,
            turn_id: None,
        },
    )
    .ok()
}

/// Borrowed context for applying one normalized observation batch.
pub(crate) struct ApplyContext<'a> {
    pub(crate) run_id: &'a RunId,
    pub(crate) thread_id: Option<&'a ThreadId>,
    pub(crate) settings: &'a HermesSettings,
    pub(crate) runtime_session_id: &'a str,
    pub(crate) tracker: &'a mut HermesPendingTracker,
    pub(crate) active_turn: &'a mut Option<String>,
    pub(crate) observations: &'a mpsc::Sender<EngineObservation>,
    pub(crate) frame_sequence: u64,
}

/// Applies one normalized batch; returns the terminal state when the turn ends.
///
/// Text projects onto the shared chunked vocabulary; usage reports fail
/// closed to nothing when their identities do not validate; approvals and
/// questions populate the pending tracker with no control-flow side effect;
/// subagent rows travel the owner channel without ever adopting the root
/// turn.
pub(crate) async fn apply_observations(
    normalizer: &mut HermesNormalizer,
    event: &HermesEvent,
    context: ApplyContext<'_>,
) -> Option<TerminalState> {
    let ApplyContext {
        run_id,
        thread_id,
        settings,
        runtime_session_id,
        tracker,
        active_turn,
        observations,
        frame_sequence,
    } = context;
    if event.session_id() != Some(runtime_session_id) {
        return None;
    }
    for observation in normalizer.normalize(run_id, event) {
        match observation {
            HermesObservation::Text(delta) => {
                if active_turn.is_none() {
                    *active_turn = Some(runtime_session_id.to_owned());
                }
                let native_id = format!("hermes:{frame_sequence}");
                for chunk in chunk_text(run_id, frame_sequence, &native_id, &delta) {
                    if observations
                        .send(EngineObservation::TextDelta(chunk))
                        .await
                        .is_err()
                    {
                        return Some(TerminalState::Interrupted);
                    }
                }
            }
            HermesObservation::Usage(sample) => {
                let Some(observed_at) = current_unix_millis() else {
                    continue;
                };
                let report = usage_report(
                    run_id,
                    thread_id,
                    runtime_session_id,
                    Some(normalizer.turn_label(run_id)),
                    frame_sequence,
                    settings,
                    &sample,
                    observed_at,
                );
                if let Some(report) = report {
                    if observations
                        .send(EngineObservation::Usage(UsageObservation::new(report)))
                        .await
                        .is_err()
                    {
                        return Some(TerminalState::Interrupted);
                    }
                }
            }
            HermesObservation::Terminal(state) => return Some(state),
            HermesObservation::Subagent(row) => {
                tracker.note_subagent(
                    row.agent_native_thread_id().as_str(),
                    row.parent_native_thread_id().as_str(),
                );
                if observations
                    .send(EngineObservation::Subagent(
                        super::observation::SubagentLifecycleRow::new(row),
                    ))
                    .await
                    .is_err()
                {
                    return Some(TerminalState::Interrupted);
                }
            }
            HermesObservation::Approval(request) => {
                tracker.note_approval(request);
            }
            HermesObservation::Question(request) => {
                tracker.note_questions(&request);
            }
        }
    }
    None
}

// H3: native continuation gate, recorded service version, and group teardown
// ---------------------------------------------------------------------------

/// Minimum Hermes service for native continuation.
///
/// The transport floor and the continuation floor are the same verified
/// release (`0.20.0`): the launch authority already refuses older services
/// at probe time, and this gate re-checks the recorded version (mirroring
/// `minimum_hermes_version` in `modules/engines/src/hermes/service.ts`) so a
/// stale capability can never authorize a resume the installed service no
/// longer honors.
pub(crate) const HERMES_CONTINUATION_MINIMUM_SERVICE_VERSION: &str = "0.20.0";

/// Returns whether Hermes teardown must terminate the whole process group.
///
/// Always true: the owner spawns Hermes with whole-group custody (Job Object
/// on Windows), so teardown kills hermes grandchildren that still hold pipes
/// instead of orphaning them. Unobserved reaps quarantine through the shared
/// `cleanup_after_abort` / `finish_turn_result` path.
pub(crate) const fn hermes_requires_group_termination() -> bool {
    true
}

/// Compares two service version spellings by their numeric core.
///
/// A leading name (`Hermes Agent v`) and any trailing pre-release suffix are
/// ignored, so `Hermes Agent v0.20.0` compares equal to `0.20.0`. The
/// recorded launch version is the bare `Display` form (`0.20.0`), which the
/// unanchored TypeScript probe pattern also accepts. Returns `None` when
/// either side has no parseable triple; callers fail closed on `None`.
pub(crate) fn compare_hermes_service_versions(
    left: &str,
    right: &str,
) -> Option<std::cmp::Ordering> {
    Some(parse_service_triple(left)?.cmp(&parse_service_triple(right)?))
}

/// Returns whether a recorded service version meets a minimum floor.
pub(crate) fn hermes_service_meets_minimum(version: &str, minimum: &str) -> bool {
    matches!(
        compare_hermes_service_versions(version, minimum),
        Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater)
    )
}

fn parse_service_triple(text: &str) -> Option<[u64; 3]> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let run: String = text[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let mut parts = run.split('.');
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}

/// Native-continuation decision for one Hermes turn.
///
/// `Compatible` authorizes `session.resume` against the stored durable
/// session; `Incompatible` carries the stable reason the dispatcher surfaces
/// instead of silently starting fresh or resuming across engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HermesContinuationDecision {
    Compatible,
    Incompatible { reason: &'static str },
}

/// Bounded input for [`check_hermes_native_continuation`].
pub(crate) struct HermesContinuationGateInput<'a> {
    /// Recorded service version (`VerifiedHermesLaunch::version`).
    pub service_version: &'a str,
    /// Explicit target model from the current selection; `None` fails closed.
    pub target_model: Option<&'a str>,
    /// Advertised models when a `model.options` inventory was read; `None`
    /// skips advertisement validation (the live inventory check pre-validates
    /// the exact target model before resume) but never skips the
    /// explicit-model or service gates.
    pub advertised_models: Option<&'a [&'a str]>,
    /// Whether the stored binding names the same `hermes` engine. The
    /// dispatcher scopes continuation reads to `EngineId::Hermes`, so a false
    /// value fails closed instead of resuming across engines.
    pub same_engine: bool,
}

/// Gates one Hermes native continuation without touching provider state.
///
/// Order is contractual: same-engine first, then the explicit target model
/// (pre-validated before resume), then the `0.20.0` service floor, then model
/// advertisement when an inventory is supplied. Any failure is a typed
/// incompatible — never a silent fresh start and never a cross-engine resume.
pub(crate) fn check_hermes_native_continuation(
    input: &HermesContinuationGateInput<'_>,
) -> HermesContinuationDecision {
    if !input.same_engine {
        return HermesContinuationDecision::Incompatible {
            reason: "Hermes native continuation cannot resume across engines",
        };
    }
    let Some(target) = input.target_model.filter(|model| !model.is_empty()) else {
        return HermesContinuationDecision::Incompatible {
            reason: "Hermes native continuation requires an explicit target model",
        };
    };
    if !hermes_service_meets_minimum(
        input.service_version,
        HERMES_CONTINUATION_MINIMUM_SERVICE_VERSION,
    ) {
        return HermesContinuationDecision::Incompatible {
            reason: "Hermes native continuation requires service 0.20.0 or newer",
        };
    }
    if let Some(advertised) = input.advertised_models
        && !advertised.contains(&target)
    {
        return HermesContinuationDecision::Incompatible {
            reason: "Hermes does not currently advertise the target model",
        };
    }
    HermesContinuationDecision::Compatible
}
