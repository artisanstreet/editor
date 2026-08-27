//! Bounded authenticated HTTP/1 health handshake.
//!
//! Generates a fresh 32-byte OS secret, base64url-encodes it without padding,
//! and performs exactly one `GET /api/health` with Basic
//! `base64(opencode:<secret>)` over a Hyper HTTP/1 `TokioIo<TcpStream>`
//! connection. Hyper header count and read buffer limits are configured
//! through the pinned `Builder` APIs from caller-supplied bounds; the JSON
//! body is bounded by `Limited`. No secret is logged, cloned, or exposed in
//! `Debug`.
//!
//! Version compatibility is explicit: callers supply the expected version.
//! The fixture value `0.0.0-fixture` exists only under `cfg(test)` and is
//! never a shipping default.

use std::fmt;

use base64::Engine as _;
use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::client::conn::http1::Builder;
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::time::Instant;
use zeroize::Zeroize;

use artisan_transport::CancelHandle;

use super::readiness::ValidatedEndpoint;
use crate::engine_owner::EngineBounds;

/// Fixture expected health version, available only for tests.
///
/// Shipping code never uses this value as a default.
#[cfg(test)]
pub const FIXTURE_EXPECTED_VERSION: &str = "0.0.0-fixture";

/// Fixture incompatible health version, test-only.
#[cfg(test)]
pub const FIXTURE_INCOMPATIBLE_VERSION: &str = "0.0.0-fixture-incompatible";

/// Typed, payload-free failure of the health handshake.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum HealthError {
    /// Operating-system entropy unavailable for the 32-byte secret.
    #[error("health secret entropy failed")]
    EntropyFailed,

    /// TCP connect failed.
    #[error("health tcp connect failed")]
    ConnectFailed,

    /// Hyper HTTP/1 handshake failed.
    #[error("health handshake failed")]
    HandshakeFailed,

    /// Sending the health request failed.
    #[error("health send failed")]
    SendFailed,

    /// Health response status was not `2xx`.
    #[error("health status was not success")]
    StatusNotSuccess,

    /// Health response headers exceeded the configured limit.
    #[error("health headers exceeded limit")]
    HeadersTooLarge,

    /// Health `Content-Length` exceeded the configured JSON body limit.
    #[error("health body exceeded limit")]
    BodyTooLarge,

    /// Health body was not valid UTF-8 or JSON.
    #[error("health body was not valid json")]
    InvalidJson,

    /// Health JSON did not contain a nonempty `version` string.
    #[error("health version was missing or empty")]
    MissingVersion,

    /// Health version did not match the expected value.
    #[error("health version was incompatible")]
    IncompatibleVersion,

    /// Health response body read failed.
    #[error("health body read failed")]
    BodyReadFailed,

    /// Health deadline elapsed.
    #[error("health deadline elapsed")]
    Timeout,

    /// Operation was cancelled.
    #[error("health was cancelled")]
    Cancelled,

    /// Owner is shutting down.
    #[error("owner is shutting down")]
    Shutdown,

    /// The driver task failed to join.
    #[error("health driver join failed")]
    DriverFailed,
}

/// Fresh 32-byte secret, base64url without padding, zeroized on drop.
///
/// `Debug` is redacted and `Clone` is not implemented.
pub struct HealthSecret {
    inner: String,
}

impl HealthSecret {
    /// Generates a fresh 32-byte secret via `getrandom`.
    ///
    /// # Errors
    ///
    /// Returns [`HealthError::EntropyFailed`] when OS entropy is unavailable.
    pub fn generate() -> Result<Self, HealthError> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| HealthError::EntropyFailed)?;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        bytes.zeroize();
        Ok(Self { inner: encoded })
    }

    /// Creates a secret from a raw base64url string (test-only).
    #[cfg(test)]
    #[must_use]
    pub fn from_raw_for_tests(raw: String) -> Self {
        Self { inner: raw }
    }

    /// Returns the base64url secret string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// Returns the `Authorization` header value `Basic base64(opencode:<secret>)`.
    #[must_use]
    pub fn basic_auth(&self) -> String {
        let credentials = format!("opencode:{}", self.inner);
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
        let mut cred_bytes = credentials.into_bytes();
        cred_bytes.zeroize();
        format!("Basic {encoded}")
    }
}

impl Drop for HealthSecret {
    fn drop(&mut self) {
        self.inner.zeroize();
    }
}

impl fmt::Debug for HealthSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HealthSecret { <redacted> }")
    }
}

/// Performs the bounded authenticated `GET /api/health` handshake.
///
/// `expected_version` when `Some` requires the response `version` field to
/// equal that value; `None` checks only that version is nonempty. The
/// fixture expected version is supplied only under `cfg(test)`.
///
/// Health I/O respects `deadline`, `cancel`, and `shutdown`.
///
/// # Errors
///
/// Returns `HealthError::Shutdown`/`Cancelled`/`Timeout` for cancellation or
/// deadline, `ConnectFailed`/`HandshakeFailed`/`SendFailed`/`StatusNotSuccess`
/// for transport, `BodyTooLarge`/`BodyReadFailed`/`InvalidJson`/
/// `MissingVersion` for body validation, `IncompatibleVersion` for version
/// mismatch, or `DriverFailed` if the retained `Connection` task fails to join.
pub async fn perform_health(
    endpoint: &ValidatedEndpoint,
    secret: &HealthSecret,
    bounds: &EngineBounds,
    deadline: Instant,
    cancel: &CancelHandle,
    shutdown: &CancelHandle,
    expected_version: Option<&str>,
) -> Result<String, HealthError> {
    if shutdown.is_cancelled() {
        return Err(HealthError::Shutdown);
    }
    if cancel.is_cancelled() {
        return Err(HealthError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(HealthError::Timeout);
    }
    let request = build_health_request(endpoint, secret)?;
    let (mut sender, conn_handle) =
        connect_and_handshake(endpoint, bounds, deadline, cancel, shutdown).await?;
    let response = match send_request(&mut sender, request, deadline, cancel, shutdown).await {
        Ok(r) => r,
        Err(e) => {
            abort_and_join(conn_handle).await;
            return Err(e);
        }
    };
    if !response.status().is_success() {
        abort_and_join(conn_handle).await;
        return Err(HealthError::StatusNotSuccess);
    }
    if let Err(e) = check_content_length(&response, bounds) {
        abort_and_join(conn_handle).await;
        return Err(e);
    }
    let version = match collect_and_parse_version(
        response,
        bounds,
        deadline,
        cancel,
        shutdown,
        expected_version,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            abort_and_join(conn_handle).await;
            return Err(e);
        }
    };
    settle_driver(conn_handle, deadline, cancel, shutdown).await?;
    Ok(version)
}

async fn connect_and_handshake(
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
    HealthError,
> {
    let addr = endpoint.socket_addr();
    let stream = tokio::select! {
        biased;
        () = shutdown.wait() => return Err(HealthError::Shutdown),
        () = cancel.wait() => return Err(HealthError::Cancelled),
        () = tokio::time::sleep_until(deadline) => return Err(HealthError::Timeout),
        res = TcpStream::connect(addr) => res.map_err(|_| HealthError::ConnectFailed)?,
    };
    let io = TokioIo::new(stream);
    let mut builder = Builder::new();
    builder.max_headers(bounds.max_headers);
    builder.max_buf_size(bounds.max_buf_bytes);
    let (sender, connection) = tokio::select! {
        biased;
        () = shutdown.wait() => return Err(HealthError::Shutdown),
        () = cancel.wait() => return Err(HealthError::Cancelled),
        () = tokio::time::sleep_until(deadline) => return Err(HealthError::Timeout),
        res = builder.handshake::<_, Empty<Bytes>>(io) => res.map_err(|_| HealthError::HandshakeFailed)?,
    };
    let handle = tokio::spawn(connection);
    Ok((sender, handle))
}

fn build_health_request(
    endpoint: &ValidatedEndpoint,
    secret: &HealthSecret,
) -> Result<Request<Empty<Bytes>>, HealthError> {
    let host_header = match endpoint.host() {
        std::net::IpAddr::V4(ip) => format!("{ip}:{}", endpoint.port()),
        std::net::IpAddr::V6(ip) => format!("[{ip}]:{}", endpoint.port()),
    };
    let auth_header = secret.basic_auth();
    Request::builder()
        .method("GET")
        .uri("/api/health")
        .header("host", host_header)
        .header("authorization", auth_header)
        .header("connection", "close")
        .body(Empty::<Bytes>::new())
        .map_err(|_| HealthError::SendFailed)
}

async fn send_request(
    sender: &mut hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    request: Request<Empty<Bytes>>,
    deadline: Instant,
    cancel: &CancelHandle,
    shutdown: &CancelHandle,
) -> Result<hyper::Response<hyper::body::Incoming>, HealthError> {
    tokio::select! {
        biased;
        () = shutdown.wait() => Err(HealthError::Shutdown),
        () = cancel.wait() => Err(HealthError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(HealthError::Timeout),
        res = send_once(sender, request) => res,
    }
}

fn check_content_length(
    response: &hyper::Response<hyper::body::Incoming>,
    bounds: &EngineBounds,
) -> Result<(), HealthError> {
    let Some(len_header) = response.headers().get("content-length") else {
        return Ok(());
    };
    let Ok(len_str) = len_header.to_str() else {
        return Err(HealthError::InvalidJson);
    };
    let Ok(len) = len_str.parse::<usize>() else {
        return Err(HealthError::InvalidJson);
    };
    if len > bounds.max_json_body {
        return Err(HealthError::BodyTooLarge);
    }
    Ok(())
}

async fn collect_and_parse_version(
    response: hyper::Response<hyper::body::Incoming>,
    bounds: &EngineBounds,
    deadline: Instant,
    cancel: &CancelHandle,
    shutdown: &CancelHandle,
    expected_version: Option<&str>,
) -> Result<String, HealthError> {
    let body = response.into_body();
    let limited = Limited::new(body, bounds.max_json_body);
    let collected = tokio::select! {
        biased;
        () = shutdown.wait() => return Err(HealthError::Shutdown),
        () = cancel.wait() => return Err(HealthError::Cancelled),
        () = tokio::time::sleep_until(deadline) => return Err(HealthError::Timeout),
        res = limited.collect() => res.map_err(|_| HealthError::BodyReadFailed)?,
    };
    let bytes = collected.to_bytes();
    if bytes.len() > bounds.max_json_body {
        return Err(HealthError::BodyTooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| HealthError::InvalidJson)?;
    let Some(object) = value.as_object() else {
        return Err(HealthError::InvalidJson);
    };
    let Some(version) = object.get("version").and_then(|v| v.as_str()) else {
        return Err(HealthError::MissingVersion);
    };
    if version.is_empty() {
        return Err(HealthError::MissingVersion);
    }
    if let Some(expected) = expected_version
        && version != expected
    {
        return Err(HealthError::IncompatibleVersion);
    }
    Ok(version.to_owned())
}

async fn settle_driver(
    mut handle: tokio::task::JoinHandle<Result<(), hyper::Error>>,
    deadline: Instant,
    cancel: &CancelHandle,
    shutdown: &CancelHandle,
) -> Result<(), HealthError> {
    let driver_result = tokio::select! {
        biased;
        () = shutdown.wait() => Err(HealthError::Shutdown),
        () = cancel.wait() => Err(HealthError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(HealthError::Timeout),
        res = &mut handle => {
            match res {
                Ok(Ok(())) => {
                    #[cfg(test)]
                    super::process::witness_control_driver_joined();
                    Ok(())
                }
                Ok(Err(_)) | Err(_) => Err(HealthError::DriverFailed),
            }
        }
    };
    if let Err(HealthError::Shutdown | HealthError::Cancelled | HealthError::Timeout) =
        driver_result
    {
        handle.abort();
        let _ = handle.await;
        return Err(driver_result.unwrap_err());
    }
    driver_result
}

async fn send_once(
    sender: &mut hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    request: Request<Empty<Bytes>>,
) -> Result<hyper::Response<hyper::body::Incoming>, HealthError> {
    sender.ready().await.map_err(|_| HealthError::SendFailed)?;
    sender
        .send_request(request)
        .await
        .map_err(|_| HealthError::SendFailed)
}

async fn abort_and_join(handle: tokio::task::JoinHandle<Result<(), hyper::Error>>) {
    handle.abort();
    let _ = handle.await;
}
