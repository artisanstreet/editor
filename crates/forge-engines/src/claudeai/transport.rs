use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use futures::StreamExt as _;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use sse_core::{SseDecoder, SseEvent, SseStream, SseStreamError};

use super::session::{LoadSessionError, Session, cookie_change};
use super::session_writer::{SessionChange, SessionWriter};
use crate::process::MAX_LINE_BYTES;

pub const HOST: &str = "https://claude.ai";

const MAX_EVENT_BYTES: NonZeroUsize = NonZeroUsize::new(MAX_LINE_BYTES).unwrap();

const MAX_QUOTED: usize = 300;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Request {
    pub prompt: String,
    pub model: String,
    pub thinking_mode: String,
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_states: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(
        rename = "parent_message_uuid",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent: Option<String>,
}

impl Request {
    #[must_use]
    pub fn body(&self) -> Value {
        let mut body = json!({
            "prompt": self.prompt,
            "model": self.model,
            "thinking_mode": self.thinking_mode,
            "effort": self.effort,
            "rendering_mode": "messages",
            "attachments": [],
            "files": self.files,
            "sync_sources": [],
            "tools": self.tools,
            "tool_states": self.tool_states,
        });
        if let Some(parent) = &self.parent {
            body["parent_message_uuid"] = parent.as_str().into();
        }
        body
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Challenge,
    SignedOut(String),
    ConversationGone,
    UsageLimit {
        resets_at: Option<artisan_api::Timestamp>,
        message: String,
    },
    RateLimited(String),
    Overloaded(String),
    ContextLimit(String),
    Other(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Challenge => f.write_str(
                "claude.ai asked for a browser check, so the session needs renewing in a browser",
            ),
            Self::SignedOut(message) => {
                write!(
                    f,
                    "claude.ai signed the session out ({message}); sign in again"
                )
            }
            Self::ConversationGone => f.write_str("the claude.ai conversation is gone"),
            Self::UsageLimit { message, .. } => {
                write!(f, "Claude.ai reported that your {message} was reached.")
            }
            Self::Overloaded(message) => write!(f, "claude.ai is overloaded ({message})"),
            Self::RateLimited(message) | Self::ContextLimit(message) | Self::Other(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for Refusal {}

#[must_use]
pub fn refusal(status: reqwest::StatusCode, challenge_header: bool, body: &str) -> Refusal {
    let html = body.trim_start().starts_with('<');
    if challenge_header
        || (html && (body.contains("challenge-platform") || body.contains("Just a moment")))
    {
        return Refusal::Challenge;
    }
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = &value["error"];
    let code = error["details"]["error_code"]
        .as_str()
        .or_else(|| error["type"].as_str())
        .unwrap_or_default();
    let message = error["message"]
        .as_str()
        .map_or_else(|| quote(body, status), str::to_owned);
    match status.as_u16() {
        404 if code.contains("conversation") => Refusal::ConversationGone,
        401 | 403 => Refusal::SignedOut(message),
        429 => rate_limit(&message),
        500..=599 => Refusal::Overloaded(message),
        _ if code == "overloaded_error" => Refusal::Overloaded(message),
        413 => Refusal::ContextLimit(format!(
            "claude.ai refused the request ({status}): {message}"
        )),
        _ if crate::provider::exceeds_context(error) => Refusal::ContextLimit(format!(
            "claude.ai refused the request ({status}): {message}"
        )),
        _ => Refusal::Other(format!(
            "claude.ai refused the request ({status}): {message}"
        )),
    }
}

pub(super) fn rate_limit(message: &str) -> Refusal {
    let inner: Value = serde_json::from_str(message).unwrap_or(Value::Null);
    if inner.is_object() {
        return limit_refusal(&inner);
    }
    Refusal::RateLimited(if message.trim().is_empty() {
        "Claude.ai temporarily limited this request (HTTP 429). Try again shortly.".to_owned()
    } else {
        format!(
            "Claude.ai temporarily limited this request (HTTP 429). Provider message: {message}"
        )
    })
}

#[derive(Debug)]
enum Limit {
    Concurrent,
    Exceeded {
        quota: &'static str,
        resets_at: Option<i64>,
    },
    Unidentified,
}

impl Limit {
    fn read(limit: &Value) -> Self {
        let claim = limit["representativeClaim"].as_str();
        if claim == Some("concurrents") {
            return Self::Concurrent;
        }
        let quota = match claim {
            Some("five_hour") => Some("session usage limit"),
            Some("seven_day") if limit["perModelLimit"].as_bool() == Some(true) => {
                Some("weekly usage limit for this model")
            }
            Some("seven_day") => Some("weekly usage limit"),
            Some("seven_day_opus") => Some("weekly Opus usage limit"),
            Some("seven_day_sonnet") => Some("weekly Sonnet usage limit"),
            Some("overage") => Some("extra usage limit"),
            _ => None,
        }
        .or_else(|| {
            [("5h", "session usage limit"), ("7d", "weekly usage limit")]
                .into_iter()
                .find_map(|(key, label)| {
                    (limit["windows"][key]["status"].as_str() == Some("exceeded_limit"))
                        .then_some(label)
                })
        });
        match quota.filter(|_| limit["type"].as_str() == Some("exceeded_limit")) {
            Some(quota) => Self::Exceeded {
                quota,
                resets_at: limit["resetsAt"]
                    .as_i64()
                    .map(|seconds| seconds.saturating_mul(1000)),
            },
            None => Self::Unidentified,
        }
    }

    fn refusal(self) -> Refusal {
        match self {
            Self::Concurrent => Refusal::RateLimited(
                "Claude.ai has too many active responses. Try again shortly.".to_owned(),
            ),
            Self::Exceeded { quota, resets_at } => Refusal::UsageLimit {
                resets_at,
                message: quota.to_owned(),
            },
            Self::Unidentified => Refusal::RateLimited(
                "Claude.ai rejected this request because of a limit, but did not identify which limit. Try again shortly.".to_owned(),
            ),
        }
    }
}

pub(super) fn limit_refusal(limit: &Value) -> Refusal {
    let limit = Limit::read(limit);
    tracing::warn!(?limit, "claude.ai reported a completion limit");
    limit.refusal()
}

fn quote(body: &str, status: reqwest::StatusCode) -> String {
    let body = body.trim();
    if body.is_empty() || body.starts_with('<') {
        return status.to_string();
    }
    let mut end = body.len().min(MAX_QUOTED);
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    if end < body.len() {
        format!("{}…", &body[..end])
    } else {
        body.to_owned()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("reaching claude.ai for {method} {path}")]
    Unreachable {
        method: reqwest::Method,
        path: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("claude.ai refused {method} {path}")]
    Refused {
        method: reqwest::Method,
        path: String,
        #[source]
        source: Refusal,
    },
    #[error("reading claude.ai's answer to {method} {path}")]
    Body {
        method: reqwest::Method,
        path: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("claude.ai's answer to {method} {path} is not JSON")]
    NotJson {
        method: reqwest::Method,
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

pub trait Refused: std::error::Error {
    fn refusal(&self) -> Option<&Refusal>;
}

impl Refused for Refusal {
    fn refusal(&self) -> Option<&Refusal> {
        Some(self)
    }
}

impl Refused for RequestError {
    fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Refused { source, .. } => Some(source),
            Self::Unreachable { .. } | Self::Body { .. } | Self::NotJson { .. } => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenTransportError {
    #[error("reading the claude.ai session in {home}")]
    Load {
        home: PathBuf,
        #[source]
        source: LoadSessionError,
    },
    #[error("there is no claude.ai session in {home}")]
    NoSession { home: PathBuf },
    #[error("building the claude.ai client")]
    Client(#[source] BuildClientError),
}

#[derive(Debug, thiserror::Error)]
#[error("building the claude.ai client for {home}")]
pub struct BuildClientError {
    pub home: PathBuf,
    #[source]
    pub source: reqwest::Error,
}

#[derive(Debug, thiserror::Error)]
#[error("renaming claude.ai conversation {conversation}")]
pub struct RenameConversationError {
    pub conversation: String,
    #[source]
    pub source: RequestError,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateConversationError {
    #[error("starting a claude.ai conversation")]
    Request(#[source] RequestError),
    #[error("claude.ai started a conversation without a uuid")]
    NoUuid,
}

impl Refused for CreateConversationError {
    fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Request(source) => source.refusal(),
            Self::NoUuid => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("asking claude.ai to continue conversation {conversation}")]
pub struct CompleteError {
    pub conversation: String,
    #[source]
    pub source: RequestError,
}

impl Refused for CompleteError {
    fn refusal(&self) -> Option<&Refusal> {
        self.source.refusal()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    #[error("the claude.ai event stream of conversation {conversation} failed")]
    Transport {
        conversation: String,
        #[source]
        source: SseStreamError<reqwest::Error>,
    },
    #[error("claude.ai sent an unreadable {event} event in conversation {conversation}")]
    Unreadable {
        conversation: String,
        event: String,
        #[source]
        source: serde_json::Error,
    },
}

fn stream_event(
    conversation: &str,
    event: Result<SseEvent, SseStreamError<reqwest::Error>>,
) -> Option<Result<Value, StreamError>> {
    match event {
        Ok(SseEvent::Message(message)) if message.data.trim().is_empty() => None,
        Ok(SseEvent::Message(message)) => Some(
            serde_json::from_str::<Value>(&message.data).map_err(|source| {
                StreamError::Unreadable {
                    conversation: conversation.to_owned(),
                    event: message.event.into_owned(),
                    source,
                }
            }),
        ),
        Ok(SseEvent::Retry(_)) => None,
        Err(source) => Some(Err(StreamError::Transport {
            conversation: conversation.to_owned(),
            source,
        })),
    }
}

impl Refused for StreamError {
    fn refusal(&self) -> Option<&Refusal> {
        None
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("reading {path} to upload it to claude.ai")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("uploading {path} to claude.ai")]
    Request {
        path: PathBuf,
        #[source]
        source: RequestError,
    },
    #[error("claude.ai took {path} without naming a file id")]
    NoFileId { path: PathBuf },
}

impl Refused for UploadError {
    fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Request { source, .. } => source.refusal(),
            Self::Read { .. } | Self::NoFileId { .. } => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("stopping claude.ai's completion in conversation {conversation}")]
pub struct StopError {
    pub conversation: String,
    #[source]
    pub source: RequestError,
}

impl Refused for StopError {
    fn refusal(&self) -> Option<&Refusal> {
        self.source.refusal()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteConversationError {
    #[error("reaching claude.ai to delete conversation {conversation}")]
    Unreachable {
        conversation: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("reading claude.ai's {status} answer to deleting conversation {conversation}")]
    Body {
        conversation: String,
        status: reqwest::StatusCode,
        #[source]
        source: reqwest::Error,
    },
    #[error("claude.ai answered {status} to deleting conversation {conversation}: {body}")]
    Refused {
        conversation: String,
        status: reqwest::StatusCode,
        body: String,
    },
}

pub type Events = BoxStream<'static, Result<Value, StreamError>>;

pub trait Backend: Send + Sync {
    fn create_conversation(&self) -> BoxFuture<'_, Result<String, CreateConversationError>>;
    fn complete<'a>(
        &'a self,
        conversation: &'a str,
        request: &'a Request,
    ) -> BoxFuture<'a, Result<Events, CompleteError>>;
    fn upload<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<String, UploadError>>;
    fn stop<'a>(&'a self, conversation: &'a str) -> BoxFuture<'a, Result<(), StopError>>;
    fn delete_conversation<'a>(
        &'a self,
        conversation: &'a str,
    ) -> BoxFuture<'a, Result<(), DeleteConversationError>>;
}

async fn multipart(path: &Path, boundary: &str) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt as _;
    let name = path.file_name().map_or_else(
        || "image".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let mime = match path.extension().and_then(|ext| ext.to_str()) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        _ => "image/webp",
    };
    let head = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {mime}\r\n\r\n",
        name.replace('"', "")
    );
    let tail = format!("\r\n--{boundary}--\r\n");
    let mut file = tokio::fs::File::open(path).await?;
    let size = usize::try_from(file.metadata().await?.len()).map_err(std::io::Error::other)?;
    let mut body = Vec::with_capacity(head.len() + size + tail.len() + READ_SLACK);
    body.extend_from_slice(head.as_bytes());
    file.read_to_end(&mut body).await?;
    body.extend_from_slice(tail.as_bytes());
    Ok(body)
}

const READ_SLACK: usize = 32;

pub struct Transport {
    client: reqwest::Client,
    session: Mutex<Session>,
    writer: SessionWriter,
}

impl Transport {
    pub fn open(home: &Path) -> Result<Self, OpenTransportError> {
        let session = Session::load(home)
            .map_err(|source| OpenTransportError::Load {
                home: home.to_owned(),
                source,
            })?
            .ok_or_else(|| OpenTransportError::NoSession {
                home: home.to_owned(),
            })?;
        Self::new(home, session).map_err(OpenTransportError::Client)
    }

    pub fn new(home: &Path, session: Session) -> Result<Self, BuildClientError> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|source| BuildClientError {
                home: home.to_owned(),
                source,
            })?;
        Ok(Self {
            client,
            session: Mutex::new(session),
            writer: SessionWriter::new(home),
        })
    }

    #[must_use]
    pub fn session(&self) -> Session {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn builder(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let session = self.session();
        self.client
            .request(method, format!("{HOST}{path}"))
            .header("user-agent", &session.user_agent)
            .header("cookie", session.cookie_header())
            .header("accept", "application/json")
    }

    fn org(&self) -> String {
        self.session().org_uuid
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        build: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, RequestError> {
        let response = build(self.builder(method.clone(), path))
            .send()
            .await
            .map_err(|source| RequestError::Unreachable {
                method: method.clone(),
                path: path.to_owned(),
                source,
            })?;
        self.keep_cookies(response.headers());
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let challenge = response
            .headers()
            .get("cf-mitigated")
            .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"challenge"));
        let body = response.text().await.map_err(|source| RequestError::Body {
            method: method.clone(),
            path: path.to_owned(),
            source,
        })?;
        let refusal = refusal(status, challenge, &body);
        self.record(&refusal);
        Err(RequestError::Refused {
            method,
            path: path.to_owned(),
            source: refusal,
        })
    }

    async fn json(
        method: reqwest::Method,
        path: &str,
        response: reqwest::Response,
    ) -> Result<Value, RequestError> {
        let text = response.text().await.map_err(|source| RequestError::Body {
            method: method.clone(),
            path: path.to_owned(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| RequestError::NotJson {
            method,
            path: path.to_owned(),
            source,
        })
    }

    fn keep_cookies(&self, headers: &reqwest::header::HeaderMap) {
        let Some(now) = chrono::DateTime::from_timestamp_millis(artisan_api::now()) else {
            return;
        };
        let changes: Vec<_> = headers
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|header| cookie_change(header, now))
            .collect();
        if changes.is_empty() {
            return;
        }
        let new_cookies = {
            let mut session = self
                .session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let before = session.cookies.clone();
            session.apply(&changes);
            session.cookies != before
        };
        if new_cookies {
            self.writer.submit(SessionChange::Cookies(changes));
        }
    }

    fn record(&self, refusal: &Refusal) {
        let change = match refusal {
            Refusal::Challenge => SessionChange::NeedsRenewal,
            Refusal::SignedOut(message) => {
                SessionChange::Expired(format!("claude.ai signed it out: {message}"))
            }
            Refusal::ConversationGone
            | Refusal::UsageLimit { .. }
            | Refusal::RateLimited(_)
            | Refusal::Overloaded(_)
            | Refusal::ContextLimit(_)
            | Refusal::Other(_) => return,
        };
        self.writer.submit(change);
    }

    pub async fn get(&self, path: &str) -> Result<Value, RequestError> {
        let response = self
            .send(reqwest::Method::GET, path, |request| request)
            .await?;
        Self::json(reqwest::Method::GET, path, response).await
    }

    pub async fn rename_conversation(
        &self,
        conversation: &str,
        title: &str,
    ) -> Result<(), RenameConversationError> {
        let path = format!(
            "/api/organizations/{}/chat_conversations/{conversation}",
            self.org()
        );
        let body = json!({ "name": title }).to_string();
        self.send(reqwest::Method::PUT, &path, |request| {
            request
                .header("content-type", "application/json")
                .body(body)
        })
        .await
        .map_err(|source| RenameConversationError {
            conversation: conversation.to_owned(),
            source,
        })?;
        Ok(())
    }

    async fn post_json(
        &self,
        path: &str,
        body: &Value,
        accept: &str,
    ) -> Result<reqwest::Response, RequestError> {
        self.send(reqwest::Method::POST, path, |request| {
            request
                .header("accept", accept)
                .header("content-type", "application/json")
                .body(body.to_string())
        })
        .await
    }
}

impl Backend for Transport {
    fn create_conversation(&self) -> BoxFuture<'_, Result<String, CreateConversationError>> {
        Box::pin(async move {
            let path = format!("/api/organizations/{}/chat_conversations", self.org());
            let body = json!({ "name": "Artisan", "is_temporary": true });
            let response = self
                .post_json(&path, &body, "application/json")
                .await
                .map_err(CreateConversationError::Request)?;
            let value = Self::json(reqwest::Method::POST, &path, response)
                .await
                .map_err(CreateConversationError::Request)?;
            value["uuid"]
                .as_str()
                .map(str::to_owned)
                .ok_or(CreateConversationError::NoUuid)
        })
    }

    fn complete<'a>(
        &'a self,
        conversation: &'a str,
        request: &'a Request,
    ) -> BoxFuture<'a, Result<Events, CompleteError>> {
        Box::pin(async move {
            let path = format!(
                "/api/organizations/{}/chat_conversations/{conversation}/completion",
                self.org()
            );
            let response = self
                .post_json(&path, &request.body(), "text/event-stream")
                .await
                .map_err(|source| CompleteError {
                    conversation: conversation.to_owned(),
                    source,
                })?;
            let mut stream = SseStream::with_decoder(SseDecoder::with_limit(MAX_EVENT_BYTES));
            stream.attach(response.bytes_stream());
            let conversation = conversation.to_owned();
            let events = stream
                .filter_map(move |event| std::future::ready(stream_event(&conversation, event)));
            Ok(events.boxed())
        })
    }

    fn upload<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<String, UploadError>> {
        Box::pin(async move {
            let boundary = format!("artisan{:016x}", rand_u64());
            let body = multipart(path, &boundary)
                .await
                .map_err(|source| UploadError::Read {
                    path: path.to_owned(),
                    source,
                })?;
            let upload = format!("/api/{}/upload", self.org());
            let failed = |source| UploadError::Request {
                path: path.to_owned(),
                source,
            };
            let response = self
                .send(reqwest::Method::POST, &upload, |request| {
                    request
                        .header(
                            "content-type",
                            format!("multipart/form-data; boundary={boundary}"),
                        )
                        .body(body)
                })
                .await
                .map_err(failed)?;
            let value = Self::json(reqwest::Method::POST, &upload, response)
                .await
                .map_err(failed)?;
            value["file_uuid"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| UploadError::NoFileId {
                    path: path.to_owned(),
                })
        })
    }

    fn stop<'a>(&'a self, conversation: &'a str) -> BoxFuture<'a, Result<(), StopError>> {
        Box::pin(async move {
            let path = format!(
                "/api/organizations/{}/chat_conversations/{conversation}/stop_response",
                self.org()
            );
            self.post_json(&path, &json!({}), "application/json")
                .await
                .map_err(|source| StopError {
                    conversation: conversation.to_owned(),
                    source,
                })?;
            Ok(())
        })
    }

    fn delete_conversation<'a>(
        &'a self,
        conversation: &'a str,
    ) -> BoxFuture<'a, Result<(), DeleteConversationError>> {
        Box::pin(async move {
            let path = format!(
                "/api/organizations/{}/chat_conversations/{conversation}",
                self.org()
            );
            let response = self
                .builder(reqwest::Method::DELETE, &path)
                .send()
                .await
                .map_err(|source| DeleteConversationError::Unreachable {
                    conversation: conversation.to_owned(),
                    source,
                })?;
            self.keep_cookies(response.headers());
            let status = response.status();
            if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
                return Ok(());
            }
            let body = response
                .text()
                .await
                .map_err(|source| DeleteConversationError::Body {
                    conversation: conversation.to_owned(),
                    status,
                    source,
                })?;
            Err(DeleteConversationError::Refused {
                conversation: conversation.to_owned(),
                status,
                body: quote(&body, status),
            })
        })
    }
}

fn rand_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).ok();
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
#[path = "../../tests/claudeai/transport/unit.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/claudeai/transport.rs"]
mod stream_tests;
