//! Bounded authenticated SSE follower for experimental session log.

use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Empty};
use hyper::client::conn::http1::Builder;
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;

use artisan_transport::CancelHandle;

use super::http::HealthSecret;
use super::readiness::ValidatedEndpoint;
use crate::engine_owner::EngineBounds;
use crate::engine_owner::framing::{SseEvent, SseFramer};
use crate::engine_owner::observation::{
    EngineObservation, TerminalState, TextSnapshot, UsageObservation, deliver_observation,
};
use crate::engine_owner::opencode_event::{OpenCodeEventAdapter, OpenCodeTextReconciliation};
use crate::engine_owner::usage::{OpenCode2UsageContext, parse_opencode2_usage};

use artisan_domain::{EngineModelId, EngineRouteId, EngineVariantId, RunId, ThreadId, UnixMillis};

/// Effective `after` cursor selection for one follow.
#[derive(Clone, Copy, Debug, Default)]
enum AfterCursor {
    /// Use the session's durable sequence.
    #[default]
    Session,
    /// Use the caller-supplied cursor; `None` omits the cursor entirely.
    Explicit(Option<u64>),
}

/// Immutable launch attribution used only while normalizing provider usage.
/// The provider session remains the authenticated stream input and is never
/// accepted from an event envelope as authority.
#[expect(
    clippy::struct_field_names,
    reason = "fields mirror the launch identity vocabulary; renaming would obscure the mapping"
)]
pub(crate) struct StreamUsageContext {
    run_id: RunId,
    thread_id: ThreadId,
    model_id: EngineModelId,
    provider_route_id: EngineRouteId,
    variant_id: Option<EngineVariantId>,
}

impl StreamUsageContext {
    pub(crate) fn new(
        run_id: RunId,
        thread_id: ThreadId,
        model_id: EngineModelId,
        provider_route_id: EngineRouteId,
        variant_id: Option<EngineVariantId>,
    ) -> Self {
        Self {
            run_id,
            thread_id,
            model_id,
            provider_route_id,
            variant_id,
        }
    }
}

/// Per-turn stream custody shared by the normal follow and abort
/// reconciliation follow. Durable cursors are never replaced by an invented
/// local value; local sequence numbers are only used inside normalized
/// observations that have no provider cursor.
pub(crate) struct StreamState {
    durable_cursor: Option<u64>,
    local_sequence: u64,
    adapter: Option<OpenCodeEventAdapter>,
}

#[allow(dead_code)]
impl StreamState {
    pub(crate) fn new(after: Option<u64>) -> Self {
        Self {
            durable_cursor: after,
            local_sequence: 0,
            adapter: None,
        }
    }

    pub(crate) fn for_run(
        run_id: RunId,
        session_id: String,
        after: Option<u64>,
    ) -> Result<Self, StreamError> {
        let adapter = OpenCodeEventAdapter::new(run_id, session_id)
            .map_err(|_| StreamError::InvalidSession)?;
        Ok(Self {
            durable_cursor: after,
            local_sequence: 0,
            adapter: Some(adapter),
        })
    }

    fn next_local_sequence(&mut self) -> Result<u64, StreamError> {
        self.local_sequence = self
            .local_sequence
            .checked_add(1)
            .ok_or(StreamError::DecodeFailed)?;
        Ok(self.local_sequence)
    }

    fn request_after(&self, requested: Option<u64>) -> Option<u64> {
        self.durable_cursor.or(requested)
    }

    fn remember_provider_cursor(&mut self, cursor: Option<u64>) {
        if let Some(cursor) = cursor {
            self.durable_cursor = Some(
                self.durable_cursor
                    .map_or(cursor, |current| current.max(cursor)),
            );
        }
    }
}

/// Payload-free typed failure of the bounded SSE stream follower.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum StreamError {
    #[error("stream session was invalid")]
    InvalidSession,
    #[error("stream tcp connect failed")]
    ConnectFailed,
    #[error("stream handshake failed")]
    HandshakeFailed,
    #[error("stream send failed")]
    SendFailed,
    #[error("stream status was not success")]
    StatusNotSuccess,
    #[error("stream content type was not event stream")]
    ContentTypeInvalid,
    #[error("stream body read failed")]
    BodyFailed,
    #[error("stream framing failed")]
    FramingFailed,
    #[error("stream decode failed")]
    DecodeFailed,
    #[error("stream delivery failed")]
    DeliveryFailed,
    #[error("stream ended without terminal")]
    MissingTerminal,
    #[error("stream order violated")]
    OrderViolation,
    #[error("stream deadline elapsed")]
    Timeout,
    #[error("stream was cancelled")]
    Cancelled,
    #[error("owner is shutting down")]
    Shutdown,
    #[error("stream driver join failed")]
    DriverFailed,
}

/// Payload-free receipt preserving the terminal state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StreamReceipt {
    state: TerminalState,
}

impl StreamReceipt {
    #[must_use]
    pub(crate) fn state(self) -> TerminalState {
        self.state
    }
}

/// Grouped borrowed values used to construct one stream operation.
pub(crate) type StreamInputParts<'a> = (
    &'a ValidatedEndpoint,
    &'a HealthSecret,
    &'a EngineBounds,
    Instant,
    &'a CancelHandle,
    &'a CancelHandle,
    &'a str,
    u64,
    mpsc::Sender<EngineObservation>,
);

/// Small input struct to avoid a long argument list.
pub(crate) struct StreamInput<'a> {
    pub(crate) endpoint: &'a ValidatedEndpoint,
    pub(crate) secret: &'a HealthSecret,
    pub(crate) bounds: &'a EngineBounds,
    pub(crate) deadline: Instant,
    pub(crate) cancel: &'a CancelHandle,
    pub(crate) shutdown: &'a CancelHandle,
    pub(crate) session: &'a str,
    pub(crate) after: u64,
    pub(crate) sender: mpsc::Sender<EngineObservation>,
    after_cursor: AfterCursor,
    usage_context: Option<StreamUsageContext>,
}

impl<'a> StreamInput<'a> {
    #[must_use]
    pub(crate) fn new(parts: StreamInputParts<'a>) -> Self {
        let (endpoint, secret, bounds, deadline, cancel, shutdown, session, after, sender) = parts;
        Self {
            endpoint,
            secret,
            bounds,
            deadline,
            cancel,
            shutdown,
            session,
            after,
            sender,
            after_cursor: AfterCursor::default(),
            usage_context: None,
        }
    }

    /// Replaces the provider `after` cursor for this follow. `None` omits the
    /// cursor rather than inventing one when a resume log had no sequence.
    pub(crate) fn with_after(mut self, after: Option<u64>) -> Self {
        self.after_cursor = AfterCursor::Explicit(after);
        self
    }

    /// Supplies immutable launch attribution for provider usage events.
    pub(crate) fn with_usage_context_option(mut self, context: Option<StreamUsageContext>) -> Self {
        self.usage_context = context;
        self
    }

    fn effective_after(&self) -> Option<u64> {
        match self.after_cursor {
            AfterCursor::Session => Some(self.after),
            AfterCursor::Explicit(after) => after,
        }
    }
}

impl std::fmt::Debug for StreamInput<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamInput { <redacted> }")
    }
}

/// Follows the authenticated `GET /api/experimental/session/:session/log?after=:after&follow=true` SSE stream.
///
/// Validates the session segment with the same injection rules as prompt,
/// requires `2xx` and `text/event-stream`, drives `hyper::body::Incoming`
/// incrementally under `shutdown > cancel > deadline > body frame`, feeds
/// chunks into `SseFramer`, decodes with `decode_sse_event`, and delivers
/// via `deliver_observation` sequentially. Exactly one terminal ends success;
/// clean EOF without terminal is a typed error.
#[allow(dead_code)]
pub(crate) async fn follow_stream(input: StreamInput<'_>) -> Result<StreamReceipt, StreamError> {
    let mut state = StreamState::new(input.effective_after());
    follow_stream_inner(input, None, &mut state).await
}

/// Follows the stream while requiring every authenticated envelope to carry
/// the immutable run identity owned by the current operation.
#[allow(dead_code)]
pub(crate) async fn follow_stream_for_run(
    input: StreamInput<'_>,
    expected_run: &RunId,
) -> Result<StreamReceipt, StreamError> {
    let mut state = StreamState::new(input.effective_after());
    follow_stream_inner(input, Some(expected_run), &mut state).await
}

/// Follows a run while retaining the caller-owned cursor/dedup state across a
/// normal stream and the later abort/terminal reconciliation stream.
pub(crate) async fn follow_stream_for_run_with_state(
    input: StreamInput<'_>,
    expected_run: &RunId,
    state: &mut StreamState,
) -> Result<StreamReceipt, StreamError> {
    follow_stream_inner(input, Some(expected_run), state).await
}

async fn follow_stream_inner(
    input: StreamInput<'_>,
    expected_run: Option<&RunId>,
    state: &mut StreamState,
) -> Result<StreamReceipt, StreamError> {
    let (request, mut framer) =
        prepare_stream(&input, state.request_after(input.effective_after()))?;
    let (mut sender, conn_handle) = connect_and_handshake_stream(
        input.endpoint,
        input.bounds,
        input.deadline,
        input.cancel,
        input.shutdown,
    )
    .await?;
    let response = match send_stream_request(
        &mut sender,
        request,
        input.deadline,
        input.cancel,
        input.shutdown,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            return Err(abort_and_join_with_fallback(conn_handle, error).await);
        }
    };
    if !response.status().is_success() {
        return Err(abort_and_join_with_fallback(conn_handle, StreamError::StatusNotSuccess).await);
    }
    if !is_sse_content_type(response.headers()) {
        return Err(
            abort_and_join_with_fallback(conn_handle, StreamError::ContentTypeInvalid).await,
        );
    }
    let mut body = response.into_body();
    loop {
        let frame = tokio::select! {
            biased;
            () = input.shutdown.wait() => {
                return Err(abort_and_join_with_fallback(conn_handle, StreamError::Shutdown).await);
            }
            () = input.cancel.wait() => {
                return Err(abort_and_join_with_fallback(conn_handle, StreamError::Cancelled).await);
            }
            () = tokio::time::sleep_until(input.deadline) => {
                return Err(abort_and_join_with_fallback(conn_handle, StreamError::Timeout).await);
            }
            result = body.frame() => result,
        };
        match frame {
            Some(Ok(frame)) => {
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                if data.is_empty() {
                    continue;
                }
                let Ok(events) = framer.feed(&data) else {
                    return Err(abort_and_join_with_fallback(
                        conn_handle,
                        StreamError::FramingFailed,
                    )
                    .await);
                };
                match deliver_events(&events, &input, expected_run, state).await {
                    Ok(None) => {}
                    Ok(Some(state)) => {
                        return match abort_and_join(conn_handle).await {
                            Ok(()) => Ok(StreamReceipt { state }),
                            Err(error) => Err(error),
                        };
                    }
                    Err(error) => {
                        return Err(abort_and_join_with_fallback(conn_handle, error).await);
                    }
                }
            }
            Some(Err(_)) => {
                return Err(
                    abort_and_join_with_fallback(conn_handle, StreamError::BodyFailed).await,
                );
            }
            None => {
                let eof_error = match framer.finish() {
                    Ok(_) => StreamError::MissingTerminal,
                    Err(_) => StreamError::FramingFailed,
                };
                return Err(abort_and_join_with_fallback(conn_handle, eof_error).await);
            }
        }
    }
}

fn prepare_stream(
    input: &StreamInput<'_>,
    after: Option<u64>,
) -> Result<(Request<Empty<Bytes>>, SseFramer), StreamError> {
    if !super::http::is_valid_session_segment(input.session) {
        return Err(StreamError::InvalidSession);
    }
    if input.shutdown.is_cancelled() {
        return Err(StreamError::Shutdown);
    }
    if input.cancel.is_cancelled() {
        return Err(StreamError::Cancelled);
    }
    if Instant::now() >= input.deadline {
        return Err(StreamError::Timeout);
    }
    let request = build_stream_request(input.endpoint, input.secret, input.session, after)
        .map_err(|_| StreamError::SendFailed)?;
    let framer = SseFramer::new(input.bounds.max_sse_line, input.bounds.max_sse_event)
        .map_err(|_| StreamError::FramingFailed)?;
    Ok((request, framer))
}

async fn deliver_events(
    events: &[SseEvent],
    input: &StreamInput<'_>,
    expected_run: Option<&RunId>,
    state: &mut StreamState,
) -> Result<Option<TerminalState>, StreamError> {
    let mut seen_terminal = false;
    let mut normalized = Vec::new();
    for event in events {
        if let Some((is_sync, Some(provider_cursor))) = provider_sequence(event)?
            && !is_sync
            && state
                .durable_cursor
                .is_some_and(|current| provider_cursor <= current)
        {
            continue;
        }
        let result = if let Some(adapter) = state.adapter.as_mut() {
            adapter
                .normalize(event)
                .map_err(|_| StreamError::DecodeFailed)?
        } else {
            #[cfg(test)]
            {
                super::event::decode_sse_event_for_run(
                    event,
                    expected_run,
                    expected_run.map(|_| input.session),
                )
                .map(|observations| super::opencode_event::OpenCodeEventResult {
                    observations,
                    provider_cursor: None,
                    recognized_event_kind: super::opencode_event::OpenCodeEventKindFlags::default(),
                    text_reconciliation: None,
                })
                .map_err(|_| StreamError::DecodeFailed)?
            }
            #[cfg(not(test))]
            {
                let _ = expected_run;
                return Err(StreamError::DecodeFailed);
            }
        };
        let super::opencode_event::OpenCodeEventResult {
            observations,
            provider_cursor,
            recognized_event_kind,
            text_reconciliation,
        } = result;
        state.remember_provider_cursor(provider_cursor);
        let mut observations = observations;
        if recognized_event_kind.usage
            && let Some(context) = input.usage_context.as_ref()
            && let Some(observation) = parse_usage_observation(event, input.session, context)
        {
            observations.push(observation);
        }
        if let Some(reconciliation) = text_reconciliation {
            let sequence = provider_cursor.map_or_else(|| state.next_local_sequence(), Ok)?;
            if let Some(observation) = reconciliation_observation(reconciliation, sequence) {
                observations.push(observation);
            }
        }
        for observation in observations {
            if seen_terminal {
                return Err(StreamError::OrderViolation);
            }
            if matches!(observation, EngineObservation::Terminal(_)) {
                seen_terminal = true;
            }
            normalized.push(observation);
        }
    }

    for observation in normalized {
        let receipt_state = match &observation {
            EngineObservation::Terminal(terminal) => Some(terminal.state()),
            EngineObservation::SummaryTitle { .. }
            | EngineObservation::TextDelta(_)
            | EngineObservation::TextSnapshot(_)
            | EngineObservation::Usage(_)
            // Activity rows are progress observations committed by the
            // dispatcher in a separate packet; they never settle the stream
            // receipt here, exactly like subagent rows below.
            | EngineObservation::Activity(_)
            // Subagent rows are progress observations, never terminal
            // receipt state; they travel the Claude owner channel only.
            | EngineObservation::Subagent(_)
            | EngineObservation::SubagentTranscript(_) => None,
        };
        deliver_observation(
            observation,
            input.sender.clone(),
            input.shutdown,
            input.cancel,
            input.deadline,
        )
        .await
        .map_err(map_delivery_error)?;
        if receipt_state.is_some() {
            return Ok(receipt_state);
        }
    }
    Ok(None)
}

fn provider_sequence(event: &SseEvent) -> Result<Option<(bool, Option<u64>)>, StreamError> {
    let value: serde_json::Value =
        serde_json::from_str(event.data()).map_err(|_| StreamError::DecodeFailed)?;
    let object = value.as_object().ok_or(StreamError::DecodeFailed)?;
    let event_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .or_else(|| (!event.event().is_empty()).then_some(event.event()));
    let is_sync = event.event() == "log.synced" || event_type == Some("log.synced");
    if is_sync {
        let sequence = object
            .get("seq")
            .map(|value| value.as_u64().ok_or(StreamError::DecodeFailed))
            .transpose()?;
        return Ok(Some((true, sequence)));
    }
    let sequence = object
        .get("durable")
        .map(|value| {
            let durable = value.as_object().ok_or(StreamError::DecodeFailed)?;
            durable
                .get("seq")
                .and_then(serde_json::Value::as_u64)
                .ok_or(StreamError::DecodeFailed)
        })
        .transpose()?;
    Ok(Some((false, sequence)))
}

fn current_unix_millis() -> Option<UnixMillis> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Some(UnixMillis::from_millis(millis))
}

fn parse_usage_observation(
    event: &SseEvent,
    session: &str,
    context: &StreamUsageContext,
) -> Option<EngineObservation> {
    let observed_at = current_unix_millis()?;
    let usage_context = OpenCode2UsageContext::new(
        &context.run_id,
        &context.thread_id,
        session,
        &context.model_id,
        &context.provider_route_id,
        context.variant_id.as_ref(),
        observed_at,
    );
    parse_opencode2_usage(event, &usage_context)
        .ok()
        .flatten()
        .map(UsageObservation::new)
        .map(EngineObservation::Usage)
}

fn reconciliation_observation(
    reconciliation: OpenCodeTextReconciliation,
    sequence: u64,
) -> Option<EngineObservation> {
    match reconciliation {
        OpenCodeTextReconciliation::Confirmed { .. } => None,
        OpenCodeTextReconciliation::Replace {
            run_id,
            part_id,
            text,
            ..
        } => Some(EngineObservation::TextSnapshot(TextSnapshot::new(
            run_id, sequence, part_id, text,
        ))),
    }
}

fn build_stream_request(
    endpoint: &ValidatedEndpoint,
    secret: &HealthSecret,
    session: &str,
    after: Option<u64>,
) -> Result<Request<Empty<Bytes>>, StreamError> {
    let host_header = match endpoint.host() {
        std::net::IpAddr::V4(ip) => format!("{ip}:{}", endpoint.port()),
        std::net::IpAddr::V6(ip) => format!("[{ip}]:{}", endpoint.port()),
    };
    let auth_header = secret.basic_auth();
    let uri = match after {
        Some(after) => format!("/api/experimental/session/{session}/log?after={after}&follow=true"),
        None => format!("/api/experimental/session/{session}/log?follow=true"),
    };
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", host_header)
        .header("authorization", auth_header)
        .header("accept", "text/event-stream")
        .header("connection", "close")
        .body(Empty::<Bytes>::new())
        .map_err(|_| StreamError::SendFailed)
}

async fn connect_and_handshake_stream(
    endpoint: &ValidatedEndpoint,
    bounds: &EngineBounds,
    deadline: Instant,
    cancel: &CancelHandle,
    shutdown: &CancelHandle,
) -> Result<
    (
        hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
        tokio::task::JoinHandle<Result<(), hyper::Error>>,
    ),
    StreamError,
> {
    let addr = endpoint.socket_addr();
    let stream = tokio::select! {
        biased;
        () = shutdown.wait() => return Err(StreamError::Shutdown),
        () = cancel.wait() => return Err(StreamError::Cancelled),
        () = tokio::time::sleep_until(deadline) => return Err(StreamError::Timeout),
        res = TcpStream::connect(addr) => res.map_err(|_| StreamError::ConnectFailed)?,
    };
    let io = TokioIo::new(stream);
    let mut builder = Builder::new();
    builder.max_headers(bounds.max_headers);
    builder.max_buf_size(bounds.max_buf_bytes);
    let (sender, connection) = tokio::select! {
        biased;
        () = shutdown.wait() => return Err(StreamError::Shutdown),
        () = cancel.wait() => return Err(StreamError::Cancelled),
        () = tokio::time::sleep_until(deadline) => return Err(StreamError::Timeout),
        res = builder.handshake::<_, Empty<Bytes>>(io) => res.map_err(|_| StreamError::HandshakeFailed)?,
    };
    let handle = tokio::spawn(connection);
    Ok((sender, handle))
}

async fn send_stream_request(
    sender: &mut hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    request: Request<Empty<Bytes>>,
    deadline: Instant,
    cancel: &CancelHandle,
    shutdown: &CancelHandle,
) -> Result<hyper::Response<hyper::body::Incoming>, StreamError> {
    tokio::select! {
        biased;
        () = shutdown.wait() => Err(StreamError::Shutdown),
        () = cancel.wait() => Err(StreamError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(StreamError::Timeout),
        res = send_once_stream(sender, request) => res,
    }
}

async fn send_once_stream(
    sender: &mut hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    request: Request<Empty<Bytes>>,
) -> Result<hyper::Response<hyper::body::Incoming>, StreamError> {
    sender.ready().await.map_err(|_| StreamError::SendFailed)?;
    sender
        .send_request(request)
        .await
        .map_err(|_| StreamError::SendFailed)
}

fn is_sse_content_type(headers: &http::HeaderMap) -> bool {
    let Some(value) = headers.get("content-type") else {
        return false;
    };
    let Ok(text) = value.to_str() else {
        return false;
    };
    let base = text.split(';').next().unwrap_or("").trim();
    base.eq_ignore_ascii_case("text/event-stream")
}

fn map_delivery_error(err: crate::engine_owner::observation::DeliveryError) -> StreamError {
    use crate::engine_owner::observation::DeliveryError as D;
    match err {
        D::Shutdown => StreamError::Shutdown,
        D::Cancelled => StreamError::Cancelled,
        D::Deadline => StreamError::Timeout,
        D::SinkClosed => StreamError::DeliveryFailed,
    }
}

async fn abort_and_join(
    handle: tokio::task::JoinHandle<Result<(), hyper::Error>>,
) -> Result<(), StreamError> {
    handle.abort();
    match handle.await {
        Ok(Ok(())) => Ok(()),
        Err(error) if error.is_cancelled() => Ok(()),
        Ok(Err(_)) | Err(_) => Err(StreamError::DriverFailed),
    }
}

async fn abort_and_join_with_fallback(
    handle: tokio::task::JoinHandle<Result<(), hyper::Error>>,
    fallback: StreamError,
) -> StreamError {
    match abort_and_join(handle).await {
        Ok(()) => fallback,
        Err(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_state_retains_only_actual_provider_cursor() {
        let mut state = StreamState::new(None);
        assert_eq!(state.request_after(None), None);
        state.remember_provider_cursor(Some(17));
        state.remember_provider_cursor(Some(12));
        assert_eq!(state.request_after(None), Some(17));
        assert_eq!(state.request_after(Some(9)), Some(17));
    }

    #[test]
    fn missing_provider_cursor_does_not_advance_abort_cursor() {
        let mut state = StreamState::new(Some(4));
        state.remember_provider_cursor(None);
        assert_eq!(state.request_after(None), Some(4));
    }

    #[test]
    fn ended_replacement_becomes_one_snapshot_observation() {
        let run_id = RunId::parse("runtime-reconciliation-run").expect("bounded run id");
        let observation = reconciliation_observation(
            OpenCodeTextReconciliation::Replace {
                run_id: run_id.clone(),
                session_id: "provider-session".to_owned(),
                part_id: "assistant-part-1".to_owned(),
                source_event_id: "event-1".to_owned(),
                text: "corrected text".to_owned(),
            },
            23,
        )
        .expect("replacement should produce an observation");
        let EngineObservation::TextSnapshot(snapshot) = observation else {
            panic!("replacement must not become a text delta");
        };
        assert_eq!(snapshot.run_id(), &run_id);
        assert_eq!(snapshot.sequence(), 23);
        assert_eq!(snapshot.part_id(), "assistant-part-1");
        assert_eq!(snapshot.text(), "corrected text");
    }
}
