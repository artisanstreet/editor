//! Loopback JSON-RPC client and Hermes session lifecycle verbs.

#![allow(clippy::module_name_repetitions)]

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::Instant;

use super::super::adapter::{HermesSettings, HermesTurnError};
use super::wire::{
    DecodedEnvelope, GatewayError, HermesEvent, RequestScope, WsFrame, decode_envelope,
    frame_to_gateway, read_ws_frame, websocket_accept_key, write_client_control, write_client_text,
};
use super::{
    HERMES_MAX_FRAME_BYTES, HERMES_MAX_HANDSHAKE_BYTES, HERMES_MAX_ID_BYTES,
    HERMES_MINIMUM_DESKTOP_CONTRACT, HERMES_REQUEST_TIMEOUT,
};
use artisan_transport::CancelHandle;

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
#[derive(Debug)]
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
#[derive(Debug)]
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
            let start = head.len().saturating_sub(3);
            let mut header_end: Option<usize> = None;
            for index in start..head.len().saturating_add(take).saturating_sub(3) {
                let byte = |at: usize| {
                    if at < head.len() {
                        head[at]
                    } else {
                        chunk[at - head.len()]
                    }
                };
                if byte(index) == b'\r'
                    && byte(index + 1) == b'\n'
                    && byte(index + 2) == b'\r'
                    && byte(index + 3) == b'\n'
                {
                    header_end = Some(index + 4);
                    break;
                }
            }
            if let Some(end) = header_end {
                if end <= head.len() {
                    break;
                }
                let need = end - head.len();
                head.extend_from_slice(&chunk[..need]);
                self.reader.consume(need);
                break;
            }
            head.extend_from_slice(&chunk[..take]);
            self.reader.consume(take);
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
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("sec-websocket-accept")
                && value.trim() == expected_accept
            {
                return Ok(());
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
            deadline: Instant::now() + Duration::from_hours(24),
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
            WsFrame::Binary(_, _) | WsFrame::Ping(_) | WsFrame::Pong(_) | WsFrame::Close => {
                Err(GatewayError::Protocol)
            }
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

fn map_gateway_to_session(error: &GatewayError) -> SessionError {
    match error {
        GatewayError::Shutdown => SessionError::Shutdown,
        GatewayError::Cancelled => SessionError::Cancelled,
        GatewayError::Timeout => SessionError::Deadline,
        GatewayError::Closed | GatewayError::Decode | GatewayError::Protocol => {
            SessionError::ProviderRequestFailed
        }
        GatewayError::Remote { .. } => SessionError::ProviderRequestFailed,
        GatewayError::Handshake => SessionError::StreamFailed,
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
#[derive(Debug)]
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
    if let Some(reported) = info_model
        && reported != model_id
    {
        return false;
    }
    if let Some(reported) = info_provider
        && reported != route_id
    {
        return false;
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
            .map_err(|error| map_gateway_to_session(&error))?
    } else {
        let guidance = guidance_seed_messages(input.guidance_sections);
        let params = input.settings.create_params(input.project_root, &guidance);
        client
            .request("session.create", params, scope)
            .await
            .map_err(|error| map_gateway_to_session(&error))?
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
        && contract < HERMES_MINIMUM_DESKTOP_CONTRACT
    {
        return Err(SessionError::IncompatibleVersion);
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
        .map_err(|error| map_gateway_to_session(&error))?;
    Ok(events)
}

/// Sends follow-up text to a live turn (`session.steer`).
///
/// Production verb behind [`AcceptedTurn::steer_text`](super::operation::AcceptedTurn::steer_text):
/// the pump issues the request over its owned gateway client and the
/// correlated gateway result resolves the delivery. Interleaved gateway
/// events ride back alongside the result so the pump can project them in
/// order. Proves the steer verb against the fixture gateway without
/// disturbing the authorize-once production flow.
///
/// # Errors
///
/// Returns [`HermesTurnError`] when the gateway fails or `scope` expires.
pub(crate) async fn steer_live_turn(
    client: &mut GatewayClient,
    runtime_session_id: &str,
    text: &str,
    scope: &RequestScope<'_>,
) -> Result<Vec<HermesEvent>, HermesTurnError> {
    let params = serde_json::json!({
        "session_id": runtime_session_id,
        "text": text,
    });
    let (_, events) = client
        .request("session.steer", params, scope)
        .await
        .map_err(|_| HermesTurnError::StreamFailed)?;
    Ok(events)
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
