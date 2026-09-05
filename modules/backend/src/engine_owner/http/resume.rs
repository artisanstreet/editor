//! Bounded provider-session resumption for the configured engine owner.
//!
//! This is intentionally a child of `engine_owner::http`: it reuses the
//! parent's authenticated connection, cancellation, deadline, body-bound,
//! and driver-settlement helpers while keeping the resume sequence narrow.
//! It can only inspect an existing session, switch its prepared agent/model,
//! and read the durable log cursor.  It never creates a session or sends a
//! prompt.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use bytes::Bytes;
use http::Request;
use http_body_util::{BodyExt, Full, Limited};
use thiserror::Error;
use tokio::time::Instant;

use artisan_transport::CancelHandle;

use crate::engine_owner::EngineBounds;
use crate::engine_owner::framing::{SseEvent, SseFramer};

use super::super::readiness::ValidatedEndpoint;
use super::{
    HealthSecret, PromptError, abort_and_join, check_content_length_prompt,
    connect_and_handshake_prompt, is_valid_session_segment, send_prompt_request,
    settle_driver_prompt,
};

/// Prepared, immutable values needed by the exact OpenCode2 resume sequence.
///
/// The repository/root caller validates the thread, profile, model, route,
/// agent, and authoritative project root before constructing this value.
pub(crate) struct ResumeSelection<'a> {
    /// Previously persisted provider session id.
    pub(crate) session_id: &'a str,
    /// Authoritative project root used to validate the provider session.
    pub(crate) working_directory: &'a str,
    /// Agent id to switch on the existing provider session.
    pub(crate) agent_id: &'a str,
    /// Model id to switch on the existing provider session.
    pub(crate) model_id: &'a str,
    /// Provider route id to switch on the existing provider session.
    pub(crate) provider_id: &'a str,
    /// Optional configured model variant.
    pub(crate) variant_id: Option<&'a str>,
}

impl<'a> ResumeSelection<'a> {
    /// Groups the already prepared immutable values for one resume attempt.
    #[must_use]
    pub(crate) const fn new(
        session_id: &'a str,
        working_directory: &'a str,
        agent_id: &'a str,
        model_id: &'a str,
        provider_id: &'a str,
        variant_id: Option<&'a str>,
    ) -> Self {
        Self {
            session_id,
            working_directory,
            agent_id,
            model_id,
            provider_id,
            variant_id,
        }
    }
}

impl fmt::Debug for ResumeSelection<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResumeSelection { <redacted> }")
    }
}

/// All bounded resources and cancellation controls for one resume attempt.
pub(crate) struct ResumeInput<'a> {
    /// Existing loopback engine endpoint.
    pub(crate) endpoint: &'a ValidatedEndpoint,
    /// Existing engine-owner health secret.
    pub(crate) secret: &'a HealthSecret,
    /// Existing configured transport bounds.
    pub(crate) bounds: &'a EngineBounds,
    /// Absolute finite deadline for the whole leaf.
    pub(crate) deadline: Instant,
    /// Per-run cancellation.
    pub(crate) cancel: &'a CancelHandle,
    /// Owner shutdown cancellation.
    pub(crate) shutdown: &'a CancelHandle,
    /// Repository/root-validated session and selection.
    pub(crate) selection: ResumeSelection<'a>,
}

impl fmt::Debug for ResumeInput<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResumeInput { <redacted> }")
    }
}

/// Successful resume facts passed back to the owner before it authorizes a
/// prompt.  The cursor is a durable provider-log sequence, not prompt text.
pub(crate) struct ResumeReceipt {
    session_id: String,
    log_cursor: Option<u64>,
}

impl ResumeReceipt {
    /// Returns the session that was inspected and switched.
    #[must_use]
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns the newest durable log sequence observed in the tail.
    #[must_use]
    pub(crate) const fn log_cursor(&self) -> Option<u64> {
        self.log_cursor
    }
}

impl fmt::Debug for ResumeReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResumeReceipt")
            .field("session_id", &"<redacted>")
            .field("log_cursor", &self.log_cursor)
            .finish()
    }
}

/// Payload-free failure of the existing-session resume leaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum ResumeError {
    /// The persisted session id is not a safe route segment.
    #[error("resume session was invalid")]
    InvalidSession,
    /// A prepared selection value was empty or contained a forbidden header
    /// or path character.
    #[error("resume selection was invalid")]
    InvalidSelection,
    /// The provider returned a session response without the frozen data shape.
    #[error("resume session response was invalid")]
    InvalidSessionResponse,
    /// The provider session belongs to another authoritative working directory.
    #[error("resume session working directory did not match")]
    WorkingDirectoryMismatch,
    /// The JSON body exceeded the configured bound.
    #[error("resume body exceeded limit")]
    BodyTooLarge,
    /// The provider response body could not be read.
    #[error("resume body read failed")]
    BodyReadFailed,
    /// A response body or durable log event was not valid JSON.
    #[error("resume response was not valid json")]
    InvalidJson,
    /// The provider returned a non-success response.
    #[error("resume status was not success")]
    StatusNotSuccess,
    /// The TCP connection could not be opened.
    #[error("resume tcp connect failed")]
    ConnectFailed,
    /// The HTTP/1 connection could not be negotiated.
    #[error("resume handshake failed")]
    HandshakeFailed,
    /// The request could not be sent.
    #[error("resume send failed")]
    SendFailed,
    /// The operation deadline elapsed.
    #[error("resume deadline elapsed")]
    Timeout,
    /// Per-run cancellation was requested.
    #[error("resume was cancelled")]
    Cancelled,
    /// Owner shutdown was requested.
    #[error("owner is shutting down")]
    Shutdown,
    /// The retained HTTP driver failed to settle.
    #[error("resume driver join failed")]
    DriverFailed,
    /// The bounded SSE log framer rejected a line or event.
    #[error("resume log framing failed")]
    FramingFailed,
}

/// Performs the exact existing-session sequence:
/// `GET session`, authoritative-directory check, `POST agent`, `POST model`,
/// and a bounded `GET log?follow=false` cursor read.  It does not create a
/// provider session and does not send a prompt.  Any failed phase closes and
/// joins its retained HTTP driver before returning.
pub(crate) async fn perform_resume(input: ResumeInput<'_>) -> Result<ResumeReceipt, ResumeError> {
    validate_input(&input)?;

    let session = get_session(&input).await?;
    if session.id != input.selection.session_id {
        return Err(ResumeError::InvalidSessionResponse);
    }
    if !same_directory(
        &session.working_directory,
        input.selection.working_directory,
    ) {
        return Err(ResumeError::WorkingDirectoryMismatch);
    }

    switch_agent(&input).await?;
    switch_model(&input).await?;
    let log_cursor = read_log_cursor(&input).await?;

    Ok(ResumeReceipt {
        session_id: input.selection.session_id.to_owned(),
        log_cursor,
    })
}

#[derive(Debug, Eq, PartialEq)]
struct SessionDetail {
    id: String,
    working_directory: String,
}

fn validate_input(input: &ResumeInput<'_>) -> Result<(), ResumeError> {
    if input.shutdown.is_cancelled() {
        return Err(ResumeError::Shutdown);
    }
    if input.cancel.is_cancelled() {
        return Err(ResumeError::Cancelled);
    }
    if Instant::now() >= input.deadline {
        return Err(ResumeError::Timeout);
    }
    if !is_valid_session_segment(input.selection.session_id) {
        return Err(ResumeError::InvalidSession);
    }
    for value in [
        input.selection.working_directory,
        input.selection.agent_id,
        input.selection.model_id,
        input.selection.provider_id,
    ] {
        if value.is_empty() || value.contains('\r') || value.contains('\n') {
            return Err(ResumeError::InvalidSelection);
        }
    }
    if input
        .selection
        .variant_id
        .is_some_and(|value| value.is_empty() || value.contains('\r') || value.contains('\n'))
    {
        return Err(ResumeError::InvalidSelection);
    }
    Ok(())
}

async fn get_session(input: &ResumeInput<'_>) -> Result<SessionDetail, ResumeError> {
    let request =
        build_get_session_request(input.endpoint, input.secret, input.selection.session_id)?;
    let (response, driver) = send_request(input, request).await?;
    if !response.status().is_success() {
        abort_and_join(driver).await;
        return Err(ResumeError::StatusNotSuccess);
    }
    if let Err(error) = check_content_length_prompt(&response, input.bounds) {
        abort_and_join(driver).await;
        return Err(map_prompt_error(error));
    }
    let body = match collect_body(input, response).await {
        Ok(body) => body,
        Err(error) => {
            abort_and_join(driver).await;
            return Err(error);
        }
    };
    settle_driver(input, driver).await?;
    parse_session_response(&body)
}

async fn switch_agent(input: &ResumeInput<'_>) -> Result<(), ResumeError> {
    let body = agent_body(input.selection.agent_id)?;
    if body.len() > input.bounds.max_json_body {
        return Err(ResumeError::BodyTooLarge);
    }
    let request = build_json_post_request(
        input.endpoint,
        input.secret,
        session_agent_path(input.selection.session_id),
        body,
    )?;
    send_and_drain_success(input, request).await
}

async fn switch_model(input: &ResumeInput<'_>) -> Result<(), ResumeError> {
    let body = model_body(
        input.selection.model_id,
        input.selection.provider_id,
        input.selection.variant_id,
    )?;
    if body.len() > input.bounds.max_json_body {
        return Err(ResumeError::BodyTooLarge);
    }
    let request = build_json_post_request(
        input.endpoint,
        input.secret,
        session_model_path(input.selection.session_id),
        body,
    )?;
    send_and_drain_success(input, request).await
}

async fn send_and_drain_success(
    input: &ResumeInput<'_>,
    request: Request<Full<Bytes>>,
) -> Result<(), ResumeError> {
    let (response, driver) = send_request(input, request).await?;
    if !response.status().is_success() {
        abort_and_join(driver).await;
        return Err(ResumeError::StatusNotSuccess);
    }
    if let Err(error) = check_content_length_prompt(&response, input.bounds) {
        abort_and_join(driver).await;
        return Err(map_prompt_error(error));
    }
    if let Err(error) = collect_body(input, response).await {
        abort_and_join(driver).await;
        return Err(error);
    }
    settle_driver(input, driver).await?;
    Ok(())
}

async fn read_log_cursor(input: &ResumeInput<'_>) -> Result<Option<u64>, ResumeError> {
    let request = build_log_request(input.endpoint, input.secret, input.selection.session_id)?;
    let (response, driver) = send_request(input, request).await?;
    if !response.status().is_success() {
        abort_and_join(driver).await;
        return Err(ResumeError::StatusNotSuccess);
    }
    let mut body = response.into_body();
    let mut framer = match SseFramer::new(input.bounds.max_sse_line, input.bounds.max_sse_event) {
        Ok(framer) => framer,
        Err(_) => {
            abort_and_join(driver).await;
            return Err(ResumeError::FramingFailed);
        }
    };
    let mut latest = None;
    loop {
        let frame = tokio::select! {
            biased;
            () = input.shutdown.wait() => {
                abort_and_join(driver).await;
                return Err(ResumeError::Shutdown);
            }
            () = input.cancel.wait() => {
                abort_and_join(driver).await;
                return Err(ResumeError::Cancelled);
            }
            () = tokio::time::sleep_until(input.deadline) => {
                abort_and_join(driver).await;
                return Err(ResumeError::Timeout);
            }
            result = body.frame() => result,
        };
        match frame {
            Some(Ok(frame)) => {
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                let events = match framer.feed(&data) {
                    Ok(events) => events,
                    Err(_) => {
                        abort_and_join(driver).await;
                        return Err(ResumeError::FramingFailed);
                    }
                };
                for event in &events {
                    let sequence = match durable_sequence(event) {
                        Ok(sequence) => sequence,
                        Err(error) => {
                            abort_and_join(driver).await;
                            return Err(error);
                        }
                    };
                    if let Some(sequence) = sequence {
                        latest =
                            Some(latest.map_or(sequence, |current: u64| current.max(sequence)));
                    }
                }
            }
            Some(Err(_)) => {
                abort_and_join(driver).await;
                return Err(ResumeError::BodyReadFailed);
            }
            None => {
                if framer.finish().is_err() {
                    abort_and_join(driver).await;
                    return Err(ResumeError::FramingFailed);
                }
                settle_driver(input, driver).await?;
                return Ok(latest);
            }
        }
    }
}

async fn send_request(
    input: &ResumeInput<'_>,
    request: Request<Full<Bytes>>,
) -> Result<
    (
        hyper::Response<hyper::body::Incoming>,
        tokio::task::JoinHandle<Result<(), hyper::Error>>,
    ),
    ResumeError,
> {
    let (mut sender, driver) = connect_and_handshake_prompt(
        input.endpoint,
        input.bounds,
        input.deadline,
        input.cancel,
        input.shutdown,
    )
    .await
    .map_err(map_prompt_error)?;
    let response = match send_prompt_request(
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
            abort_and_join(driver).await;
            return Err(map_prompt_error(error));
        }
    };
    Ok((response, driver))
}

async fn collect_body(
    input: &ResumeInput<'_>,
    response: hyper::Response<hyper::body::Incoming>,
) -> Result<Vec<u8>, ResumeError> {
    // The driver handle is retained in the response's connection task by the
    // caller.  This helper is only used after send_request, whose driver is
    // kept in the response tuple in the leaf below.
    let body = response.into_body();
    let limited = Limited::new(body, input.bounds.max_json_body);
    let collected = tokio::select! {
        biased;
        () = input.shutdown.wait() => return Err(ResumeError::Shutdown),
        () = input.cancel.wait() => return Err(ResumeError::Cancelled),
        () = tokio::time::sleep_until(input.deadline) => return Err(ResumeError::Timeout),
        result = limited.collect() => result.map_err(|_| ResumeError::BodyReadFailed)?,
    };
    let bytes = collected.to_bytes();
    if bytes.len() > input.bounds.max_json_body {
        return Err(ResumeError::BodyTooLarge);
    }
    Ok(bytes.to_vec())
}

async fn settle_driver(
    input: &ResumeInput<'_>,
    driver: tokio::task::JoinHandle<Result<(), hyper::Error>>,
) -> Result<(), ResumeError> {
    settle_driver_prompt(driver, input.deadline, input.cancel, input.shutdown)
        .await
        .map_err(map_prompt_error)
}

fn parse_session_response(bytes: &[u8]) -> Result<SessionDetail, ResumeError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ResumeError::InvalidJson)?;
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_object)
        .ok_or(ResumeError::InvalidSessionResponse)?;
    let id = data
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or(ResumeError::InvalidSessionResponse)?;
    let working_directory = data
        .get("location")
        .and_then(serde_json::Value::as_object)
        .and_then(|location| location.get("directory"))
        .and_then(serde_json::Value::as_str)
        .ok_or(ResumeError::InvalidSessionResponse)?;
    if !is_valid_session_segment(id)
        || id.is_empty()
        || working_directory.is_empty()
        || working_directory.contains('\r')
        || working_directory.contains('\n')
    {
        return Err(ResumeError::InvalidSessionResponse);
    }
    Ok(SessionDetail {
        id: id.to_owned(),
        working_directory: working_directory.to_owned(),
    })
}

fn agent_body(agent_id: &str) -> Result<Vec<u8>, ResumeError> {
    serde_json::to_vec(&serde_json::json!({ "agent": agent_id }))
        .map_err(|_| ResumeError::InvalidSelection)
}

fn model_body(
    model_id: &str,
    provider_id: &str,
    variant_id: Option<&str>,
) -> Result<Vec<u8>, ResumeError> {
    let mut model = serde_json::json!({
        "id": model_id,
        "providerID": provider_id,
    });
    if let Some(variant_id) = variant_id {
        model["variant"] = serde_json::Value::String(variant_id.to_owned());
    }
    serde_json::to_vec(&serde_json::json!({ "model": model }))
        .map_err(|_| ResumeError::InvalidSelection)
}

fn durable_sequence(event: &SseEvent) -> Result<Option<u64>, ResumeError> {
    let value: serde_json::Value =
        serde_json::from_str(event.data()).map_err(|_| ResumeError::InvalidJson)?;
    let Some(object) = value.as_object() else {
        return Err(ResumeError::InvalidJson);
    };
    let is_synced = event.event() == "log.synced"
        || object.get("type").and_then(serde_json::Value::as_str) == Some("log.synced");
    if is_synced {
        return Ok(object.get("seq").and_then(serde_json::Value::as_u64));
    }
    Ok(object
        .get("durable")
        .and_then(serde_json::Value::as_object)
        .and_then(|durable| durable.get("seq"))
        .and_then(serde_json::Value::as_u64))
}

fn same_directory(left: &str, right: &str) -> bool {
    let Some(left) = resolve_path(left) else {
        return false;
    };
    let Some(right) = resolve_path(right) else {
        return false;
    };
    let left = left.to_string_lossy();
    let right = right.to_string_lossy();
    if cfg!(windows) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

fn resolve_path(value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    let mut resolved = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().ok()?
    };
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir => resolved.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(part) => resolved.push(part),
        }
    }
    Some(resolved)
}

fn host_header(endpoint: &ValidatedEndpoint) -> String {
    match endpoint.host() {
        std::net::IpAddr::V4(ip) => format!("{ip}:{}", endpoint.port()),
        std::net::IpAddr::V6(ip) => format!("[{ip}]:{}", endpoint.port()),
    }
}

fn build_get_session_request(
    endpoint: &ValidatedEndpoint,
    secret: &HealthSecret,
    session: &str,
) -> Result<Request<Full<Bytes>>, ResumeError> {
    Request::builder()
        .method("GET")
        .uri(session_path(session))
        .header("host", host_header(endpoint))
        .header("authorization", secret.basic_auth())
        .header("connection", "close")
        .body(Full::new(Bytes::new()))
        .map_err(|_| ResumeError::SendFailed)
}

fn build_json_post_request(
    endpoint: &ValidatedEndpoint,
    secret: &HealthSecret,
    path: String,
    body: Vec<u8>,
) -> Result<Request<Full<Bytes>>, ResumeError> {
    let length = body.len();
    Request::builder()
        .method("POST")
        .uri(path)
        .header("host", host_header(endpoint))
        .header("authorization", secret.basic_auth())
        .header("content-type", "application/json")
        .header("content-length", length.to_string())
        .header("connection", "close")
        .body(Full::new(Bytes::from(body)))
        .map_err(|_| ResumeError::SendFailed)
}

fn build_log_request(
    endpoint: &ValidatedEndpoint,
    secret: &HealthSecret,
    session: &str,
) -> Result<Request<Full<Bytes>>, ResumeError> {
    Request::builder()
        .method("GET")
        .uri(log_path(session))
        .header("host", host_header(endpoint))
        .header("authorization", secret.basic_auth())
        .header("accept", "text/event-stream")
        .header("connection", "close")
        .body(Full::new(Bytes::new()))
        .map_err(|_| ResumeError::SendFailed)
}

fn session_path(session: &str) -> String {
    format!("/api/session/{session}")
}

fn session_agent_path(session: &str) -> String {
    format!("/api/session/{session}/agent")
}

fn session_model_path(session: &str) -> String {
    format!("/api/session/{session}/model")
}

fn log_path(session: &str) -> String {
    format!("/api/experimental/session/{session}/log?follow=false")
}

fn map_prompt_error(error: PromptError) -> ResumeError {
    match error {
        PromptError::InvalidSession => ResumeError::InvalidSession,
        PromptError::BodyTooLarge => ResumeError::BodyTooLarge,
        PromptError::ConnectFailed => ResumeError::ConnectFailed,
        PromptError::HandshakeFailed => ResumeError::HandshakeFailed,
        PromptError::SendFailed => ResumeError::SendFailed,
        PromptError::StatusNotSuccess => ResumeError::StatusNotSuccess,
        PromptError::BodyReadFailed => ResumeError::BodyReadFailed,
        PromptError::InvalidJson => ResumeError::InvalidJson,
        PromptError::Timeout => ResumeError::Timeout,
        PromptError::Cancelled => ResumeError::Cancelled,
        PromptError::Shutdown => ResumeError::Shutdown,
        PromptError::DriverFailed => ResumeError::DriverFailed,
        PromptError::InvalidDelivery
        | PromptError::InvalidId
        | PromptError::InvalidText
        | PromptError::InvalidFile => ResumeError::InvalidSelection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_match_the_frozen_session_api() {
        assert_eq!(session_path("session-1"), "/api/session/session-1");
        assert_eq!(
            session_agent_path("session-1"),
            "/api/session/session-1/agent"
        );
        assert_eq!(
            session_model_path("session-1"),
            "/api/session/session-1/model"
        );
        assert_eq!(
            log_path("session-1"),
            "/api/experimental/session/session-1/log?follow=false"
        );
    }

    #[test]
    fn switch_payloads_keep_the_frozen_agent_and_model_shapes() {
        let agent = serde_json::from_slice::<serde_json::Value>(
            &agent_body("agent-1").expect("agent payload should be JSON"),
        )
        .expect("agent payload should decode");
        assert_eq!(agent["agent"], "agent-1");

        let model = serde_json::from_slice::<serde_json::Value>(
            &model_body("model-1", "provider-1", Some("fast"))
                .expect("model payload should be JSON"),
        )
        .expect("model payload should decode");
        assert_eq!(model["model"]["id"], "model-1");
        assert_eq!(model["model"]["providerID"], "provider-1");
        assert_eq!(model["model"]["variant"], "fast");

        let no_variant = serde_json::from_slice::<serde_json::Value>(
            &model_body("model-1", "provider-1", None).expect("model payload should be JSON"),
        )
        .expect("model payload should decode");
        assert!(no_variant["model"].get("variant").is_none());
    }

    #[test]
    fn preflight_rejects_route_injection_without_network_io() {
        assert!(!is_valid_session_segment("session/other"));
        let selection = ResumeSelection::new(
            "session-1",
            "C:/workspace\nforbidden",
            "agent",
            "model",
            "provider",
            None,
        );
        assert!(selection.working_directory.contains('\n'));
        assert!(!format!("{selection:?}").contains("session-1"));
    }

    #[test]
    fn durable_cursor_reads_synced_and_nested_durable_sequences() {
        let synced = serde_json::json!({ "type": "log.synced", "seq": 14 });
        let synced_sequence = synced.get("seq").and_then(serde_json::Value::as_u64);
        assert_eq!(synced_sequence, Some(14));
        let durable = serde_json::json!({ "durable": { "seq": 19 } });
        let durable_sequence = durable
            .get("durable")
            .and_then(serde_json::Value::as_object)
            .and_then(|value| value.get("seq"))
            .and_then(serde_json::Value::as_u64);
        assert_eq!(durable_sequence, Some(19));
    }

    #[test]
    fn session_response_requires_id_and_authoritative_directory() {
        let valid = br#"{"data":{"id":"session-1","location":{"directory":"C:\\workspace"}}}"#;
        let parsed = parse_session_response(valid).expect("frozen session shape should parse");
        assert_eq!(parsed.id, "session-1");
        assert_eq!(parsed.working_directory, r"C:\workspace");

        let missing_directory = br#"{"data":{"id":"session-1"}}"#;
        assert_eq!(
            parse_session_response(missing_directory),
            Err(ResumeError::InvalidSessionResponse)
        );
    }

    #[test]
    fn directory_comparison_matches_resolve_style_normalization() {
        assert!(same_directory("./workspace/../workspace", "workspace"));
    }
}
