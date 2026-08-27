//! Bounded authenticated SSE follower for experimental session log.

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
use crate::engine_owner::event::decode_sse_event;
use crate::engine_owner::framing::SseFramer;
use crate::engine_owner::observation::{EngineObservation, TerminalState, deliver_observation};

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
    pub(crate) fn state(&self) -> TerminalState {
        self.state
    }
}

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
}

impl<'a> StreamInput<'a> {
    #[must_use]
    pub(crate) fn new(
        endpoint: &'a ValidatedEndpoint,
        secret: &'a HealthSecret,
        bounds: &'a EngineBounds,
        deadline: Instant,
        cancel: &'a CancelHandle,
        shutdown: &'a CancelHandle,
        session: &'a str,
        after: u64,
        sender: mpsc::Sender<EngineObservation>,
    ) -> Self {
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
pub(crate) async fn follow_stream(input: StreamInput<'_>) -> Result<StreamReceipt, StreamError> {
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
    let request = build_stream_request(input.endpoint, input.secret, input.session, input.after)
        .map_err(|_| StreamError::SendFailed)?;
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
        Ok(r) => r,
        Err(e) => {
            abort_and_join(conn_handle).await;
            return Err(e);
        }
    };
    if !response.status().is_success() {
        abort_and_join(conn_handle).await;
        return Err(StreamError::StatusNotSuccess);
    }
    if !is_sse_content_type(response.headers()) {
        abort_and_join(conn_handle).await;
        return Err(StreamError::ContentTypeInvalid);
    }
    let mut body = response.into_body();
    let mut framer = SseFramer::new(input.bounds.max_sse_line, input.bounds.max_sse_event)
        .map_err(|_| StreamError::FramingFailed)?;
    let mut terminal_state: Option<TerminalState> = None;
    loop {
        let frame_opt = tokio::select! {
            biased;
            () = input.shutdown.wait() => {
                abort_and_join(conn_handle).await;
                return Err(StreamError::Shutdown);
            }
            () = input.cancel.wait() => {
                abort_and_join(conn_handle).await;
                return Err(StreamError::Cancelled);
            }
            () = tokio::time::sleep_until(input.deadline) => {
                abort_and_join(conn_handle).await;
                return Err(StreamError::Timeout);
            }
            res = body.frame() => res,
        };
        match frame_opt {
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    if data.is_empty() {
                        continue;
                    }
                    let events = match framer.feed(&data) {
                        Ok(ev) => ev,
                        Err(_) => {
                            abort_and_join(conn_handle).await;
                            return Err(StreamError::FramingFailed);
                        }
                    };
                    if events.is_empty() {
                        continue;
                    }
                    // Decode all events first to detect order violations within batch
                    // before delivering, but we must deliver sequentially.
                    // To keep bounded behavior, decode lazily while tracking violation.
                    let mut decoded_batches: Vec<Vec<EngineObservation>> =
                        Vec::with_capacity(events.len());
                    for event in &events {
                        match decode_sse_event(event) {
                            Ok(obs) => decoded_batches.push(obs),
                            Err(_) => {
                                abort_and_join(conn_handle).await;
                                return Err(StreamError::DecodeFailed);
                            }
                        }
                    }
                    // Check order violation across the whole chunk batch if terminal already seen
                    // or second terminal / observation after terminal inside batch.
                    let mut seen_terminal_in_batch = false;
                    let mut batch_has_violation = false;
                    for batch in &decoded_batches {
                        for obs in batch {
                            let is_terminal = matches!(obs, EngineObservation::Terminal(_));
                            if terminal_state.is_some() {
                                batch_has_violation = true;
                                break;
                            }
                            if is_terminal {
                                if seen_terminal_in_batch {
                                    batch_has_violation = true;
                                    break;
                                }
                                seen_terminal_in_batch = true;
                            } else if seen_terminal_in_batch {
                                // text after terminal in same batch
                                batch_has_violation = true;
                                break;
                            }
                        }
                        if batch_has_violation {
                            break;
                        }
                        if seen_terminal_in_batch {
                            // Any further batch with any observation is also violation,
                            // but will be caught by above loop when continued.
                            // To detect, we continue loop but terminal already seen in batch
                            // means subsequent batches should be considered violation if non-empty.
                            // The outer check with seen_terminal_in_batch captures it on next iteration
                            // via the inner `if seen_terminal_in_batch && !batch.is_empty()` logic above.
                            // However we need to keep seen flag across batches.
                        }
                    }
                    if batch_has_violation {
                        abort_and_join(conn_handle).await;
                        return Err(StreamError::OrderViolation);
                    }
                    if terminal_state.is_some() && decoded_batches.iter().any(|b| !b.is_empty()) {
                        abort_and_join(conn_handle).await;
                        return Err(StreamError::OrderViolation);
                    }
                    // Now deliver in order; on first terminal, capture state and finish after delivering prior.
                    for batch in decoded_batches {
                        for obs in batch {
                            let is_terminal = matches!(&obs, EngineObservation::Terminal(_));
                            let state_for_receipt = if is_terminal {
                                match &obs {
                                    EngineObservation::Terminal(t) => Some(t.state()),
                                    _ => None,
                                }
                            } else {
                                None
                            };
                            match deliver_observation(
                                obs,
                                input.sender.clone(),
                                input.shutdown,
                                input.cancel,
                                input.deadline,
                            )
                            .await
                            {
                                Ok(()) => {}
                                Err(e) => {
                                    abort_and_join(conn_handle).await;
                                    return Err(map_delivery_error(e));
                                }
                            }
                            if let Some(state) = state_for_receipt {
                                // Ensure no remaining observations after this terminal in the same original batch
                                // Already checked above, so success.
                                terminal_state = Some(state);
                                abort_and_join(conn_handle).await;
                                return Ok(StreamReceipt { state });
                            }
                        }
                    }
                }
            }
            Some(Err(_)) => {
                abort_and_join(conn_handle).await;
                return Err(StreamError::BodyFailed);
            }
            None => {
                // EOF
                let _ = framer.finish();
                abort_and_join(conn_handle).await;
                if terminal_state.is_some() {
                    // Should have returned earlier; but if terminal was not delivered via loop
                    // (e.g., terminal never appeared), treat as missing.
                    return Err(StreamError::MissingTerminal);
                }
                return Err(StreamError::MissingTerminal);
            }
        }
    }
}

fn build_stream_request(
    endpoint: &ValidatedEndpoint,
    secret: &HealthSecret,
    session: &str,
    after: u64,
) -> Result<Request<Empty<Bytes>>, StreamError> {
    let host_header = match endpoint.host() {
        std::net::IpAddr::V4(ip) => format!("{ip}:{}", endpoint.port()),
        std::net::IpAddr::V6(ip) => format!("[{ip}]:{}", endpoint.port()),
    };
    let auth_header = secret.basic_auth();
    let uri = format!("/api/experimental/session/{session}/log?after={after}&follow=true");
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

async fn abort_and_join(handle: tokio::task::JoinHandle<Result<(), hyper::Error>>) {
    handle.abort();
    let _ = handle.await;
}
