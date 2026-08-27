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
use crate::engine_owner::framing::{SseEvent, SseFramer};
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
    let (request, mut framer) = prepare_stream(&input)?;
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
                match deliver_events(&events, &input).await {
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
    let request = build_stream_request(input.endpoint, input.secret, input.session, input.after)
        .map_err(|_| StreamError::SendFailed)?;
    let framer = SseFramer::new(input.bounds.max_sse_line, input.bounds.max_sse_event)
        .map_err(|_| StreamError::FramingFailed)?;
    Ok((request, framer))
}

async fn deliver_events(
    events: &[SseEvent],
    input: &StreamInput<'_>,
) -> Result<Option<TerminalState>, StreamError> {
    let mut seen_terminal = false;
    for event in events {
        let Ok(observations) = decode_sse_event(event) else {
            return Err(StreamError::DecodeFailed);
        };
        for observation in observations {
            if seen_terminal {
                return Err(StreamError::OrderViolation);
            }
            if matches!(observation, EngineObservation::Terminal(_)) {
                seen_terminal = true;
            }
        }
    }

    for event in events {
        let Ok(observations) = decode_sse_event(event) else {
            return Err(StreamError::DecodeFailed);
        };
        for observation in observations {
            let receipt_state = match &observation {
                EngineObservation::Terminal(terminal) => Some(terminal.state()),
                EngineObservation::TextDelta(_) => None,
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
    }
    Ok(None)
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
