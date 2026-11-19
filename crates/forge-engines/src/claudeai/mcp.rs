use crate::sandbox::Sandbox;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use artisan_api::McpServer;
use base64::Engine as _;
use futures::StreamExt as _;
use futures::stream::BoxStream;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sse_core::{SseDecoder, SseEvent, SseStream, SseStreamError};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;

use crate::process::{MAX_LINE_BYTES, SpawnError};

type UrlError = <reqwest::Url as std::str::FromStr>::Err;

#[derive(Debug, thiserror::Error)]
enum OpenError {
    #[error("the header name {name} is not valid")]
    HeaderName {
        name: String,
        #[source]
        source: reqwest::header::InvalidHeaderName,
    },
    #[error("the {name} header's value is not valid")]
    HeaderValue {
        name: String,
        #[source]
        source: reqwest::header::InvalidHeaderValue,
    },
    #[error("building the MCP client")]
    Client(#[source] reqwest::Error),
    #[error("the url is not valid")]
    InvalidUrl(#[source] UrlError),
    #[error("starting {command}")]
    Spawn {
        command: String,
        #[source]
        source: SpawnError,
    },
    #[error("the server did not open its event stream in time")]
    StreamTimedOut,
    #[error("{}", reach_error(.0))]
    Unreachable(#[source] reqwest::Error),
    #[error("HTTP {status}")]
    Status { status: u16 },
    #[error("the server did not name its endpoint in time")]
    EndpointTimedOut,
    #[error("the server's endpoint is not a valid url")]
    InvalidEndpoint(#[source] UrlError),
    #[error("the event stream ended before the server named its endpoint")]
    NoEndpoint,
    #[error("the server's endpoint is on another origin")]
    ForeignEndpoint,
}

impl OpenError {
    fn reason(&self) -> String {
        match self {
            Self::Unreachable(_) | Self::HeaderName { .. } | Self::HeaderValue { .. } => {
                self.to_string()
            }
            Self::Client(_)
            | Self::InvalidUrl(_)
            | Self::Spawn { .. }
            | Self::StreamTimedOut
            | Self::Status { .. }
            | Self::EndpointTimedOut
            | Self::InvalidEndpoint(_)
            | Self::NoEndpoint
            | Self::ForeignEndpoint => root_cause(self),
        }
    }
}

fn root_cause(error: &(dyn std::error::Error + 'static)) -> String {
    std::iter::successors(Some(error), |error| error.source())
        .last()
        .map_or_else(String::new, ToString::to_string)
}

#[derive(Clone, Debug, thiserror::Error)]
enum Closed {
    #[error("the server closed its input")]
    Input,
    #[error("the server closed its output")]
    Output,
    #[error("the server's output failed")]
    OutputFailed(#[source] Arc<std::io::Error>),
    #[error("the server closed its event stream")]
    EventStream,
    #[error(
        "the server stopped reading while {} of messages waited for it",
        size(MAX_QUEUED_BYTES)
    )]
    Backlogged,
}

#[derive(Debug, thiserror::Error)]
enum Undelivered {
    #[error("the server's answer was larger than {}", size(*.limit))]
    TooLarge { limit: usize },
    #[error(transparent)]
    Closed(Closed),
    #[error("HTTP {status}")]
    Status { status: u16 },
    #[error("{}", reach_error(.0))]
    Unreachable(#[source] reqwest::Error),
}

#[derive(Debug, thiserror::Error)]
enum RequestError {
    #[error("the server is no longer running")]
    Stopped,
    #[error("the server did not answer {method} within {seconds}s")]
    TimedOut { method: String, seconds: f32 },
    #[error(transparent)]
    Undelivered(Undelivered),
    #[error(transparent)]
    Rpc(RpcError),
    #[error("{}", reach_error(.0))]
    Unreachable(#[source] reqwest::Error),
    #[error("HTTP {status}")]
    Status { status: u16 },
    #[error("the server's answer to {method} is not JSON")]
    NotJson {
        method: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("the server answered nothing for {method}")]
    NoAnswer { method: String },
    #[error(transparent)]
    Body(BodyError),
    #[error("the server's answer to {method} was larger than {}", size(*.limit))]
    EventTooLarge { method: String, limit: usize },
    #[error("the server's answer to {method} broke off")]
    EventBrokeOff { method: String },
    #[error("the server closed the stream without answering {method}")]
    Unanswered { method: String },
    #[error("the server ended the session")]
    SessionGone,
    #[error("starting a new session")]
    Restart(#[source] Box<Self>),
    #[error("starting a new session")]
    RestartNotify(#[source] NotifyError),
}

impl RequestError {
    fn reason(&self) -> String {
        match self {
            Self::Rpc(refused) => format!("refused (JSON-RPC error {})", refused.code),
            Self::Stopped
            | Self::TimedOut { .. }
            | Self::Undelivered(_)
            | Self::Unreachable(_)
            | Self::Status { .. }
            | Self::NotJson { .. }
            | Self::NoAnswer { .. }
            | Self::Body(_)
            | Self::EventTooLarge { .. }
            | Self::EventBrokeOff { .. }
            | Self::Unanswered { .. }
            | Self::SessionGone
            | Self::Restart(_)
            | Self::RestartNotify(_) => self.to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum NotifyError {
    #[error("the server is no longer running")]
    Stopped,
    #[error(
        "the server stopped reading while {} of messages waited for it",
        size(MAX_QUEUED_BYTES)
    )]
    Backlogged,
    #[error("{}", reach_error(.0))]
    Unreachable(#[source] reqwest::Error),
    #[error("HTTP {status}")]
    Status { status: u16 },
}

#[derive(Debug, thiserror::Error)]
enum BodyError {
    #[error("the server's answer was larger than {}", size(*.limit))]
    TooLarge { limit: usize },
    #[error("the server's answer broke off")]
    BrokeOff(#[source] reqwest::Error),
}

#[derive(Debug, thiserror::Error)]
enum StartError {
    #[error("initialize")]
    Initialize(#[source] RequestError),
    #[error("initialize")]
    Initialized(#[source] NotifyError),
    #[error("listing its tools")]
    ListTools(#[source] RequestError),
}

impl StartError {
    fn reason(&self) -> String {
        match self {
            Self::Initialize(source) | Self::ListTools(source) => source.reason(),
            Self::Initialized(source) => source.to_string(),
        }
    }
}

const PROTOCOL_VERSION: &str = "2025-06-18";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const IDLE_TIMEOUT: Duration = Duration::from_mins(10);
const MAX_WARM_THREADS: usize = 8;
const MAX_TOOLS: usize = 256;
const MAX_NAME: usize = 64;
const MAX_MCP_JSON_BYTES: u64 = 1024 * 1024;
const MAX_PROJECT_SERVERS: usize = 64;
const MAX_PAGES: usize = 20;
const MAX_MESSAGE_BYTES: usize = MAX_LINE_BYTES;
const MAX_EVENT_BYTES: NonZeroUsize = NonZeroUsize::new(MAX_MESSAGE_BYTES).unwrap();
const MAX_STDERR: usize = 8 * 1024;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_IMAGES: usize = 8;
const MAX_QUEUED_BYTES: usize = 16 * 1024 * 1024;
const MIN_SECRET_CHARS: usize = 4;

#[derive(Debug, Default)]
pub struct Servers {
    clients: Vec<Arc<Client>>,
    tools: Vec<Tool>,
    names: HashMap<String, usize>,
    failures: Vec<(String, String)>,
    images: Mutex<Vec<Image>>,
    warm: Option<(String, u64)>,
}

#[derive(Debug)]
pub struct Client {
    server: String,
    listed: Vec<Listed>,
    transport: Transport,
}

#[derive(Debug)]
struct Listed {
    raw: String,
    description: String,
    schema: Value,
}

#[derive(Clone, Debug)]
pub struct Tool {
    pub(crate) name: String,
    pub(crate) server: String,
    pub(crate) raw: String,
    pub(crate) description: String,
    pub(crate) schema: Value,
    client: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub mime: String,
    pub data: Vec<u8>,
}

impl Image {
    #[must_use]
    pub fn extension(&self) -> &'static str {
        match self.mime.as_str() {
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        }
    }
}

struct Warm {
    config: String,
    clients: Vec<Arc<Client>>,
    generation: u64,
}

static WARM: LazyLock<Mutex<HashMap<String, Warm>>> = LazyLock::new(Mutex::default);
static GENERATION: AtomicU64 = AtomicU64::new(1);

tokio::task_local! {
    static THREAD: String;
}

pub(super) async fn for_thread<F: Future>(thread: &str, future: F) -> F::Output {
    THREAD.scope(thread.to_owned(), future).await
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Servers {
    pub async fn connect(servers: &[McpServer], cwd: &Path, sandbox: &Sandbox) -> Self {
        let thread = THREAD.try_with(Clone::clone).ok();
        let mut seen = HashSet::new();
        let servers: Vec<&McpServer> = servers
            .iter()
            .filter(|server| {
                if server.name.trim().is_empty() {
                    return false;
                }
                if !seen.insert(server.name.as_str()) {
                    tracing::warn!(server = %server.name, "an MCP server name is used twice; the second is skipped");
                    return false;
                }
                true
            })
            .collect();
        let config = fingerprint(&servers, cwd, sandbox);
        let mut warm: HashMap<String, Arc<Client>> = HashMap::new();
        if let Some(thread) = &thread {
            let previous = lock(&WARM).remove(thread);
            if let Some(previous) = previous {
                if previous.config == config {
                    for client in previous.clients {
                        if client.transport.alive() {
                            warm.insert(client.server.clone(), client);
                        } else {
                            tracing::info!(thread, server = %client.server, "an MCP server stopped since the last turn; starting it again");
                        }
                    }
                } else {
                    tracing::info!(
                        thread,
                        "the thread's MCP servers changed; the old ones shut down"
                    );
                }
            }
        }
        let started = futures::future::join_all(
            servers
                .iter()
                .filter(|server| !warm.contains_key(&server.name))
                .map(|server| async move {
                    (
                        server.name.clone(),
                        connect_client(server, cwd, sandbox).await,
                    )
                }),
        )
        .await;
        let mut started: HashMap<String, Result<Client, String>> = started.into_iter().collect();
        let mut connected = Self::default();
        for server in &servers {
            let client = match warm.remove(&server.name) {
                Some(client) => client,
                None => match started.remove(&server.name) {
                    Some(Ok(client)) => Arc::new(client),
                    Some(Err(reason)) => {
                        connected.failures.push((server.name.clone(), reason));
                        continue;
                    }
                    None => continue,
                },
            };
            if client.listed.is_empty() {
                tracing::info!(server = %server.name, "an MCP server offers no tools");
            }
            connected.clients.push(client);
        }
        connected.name_tools();
        if let Some(thread) = thread {
            let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
            let retired = keep_warm(
                thread.clone(),
                Warm {
                    config,
                    clients: connected.clients.clone(),
                    generation,
                },
            );
            drop(retired);
            connected.warm = Some((thread, generation));
        }
        connected
    }

    fn name_tools(&mut self) {
        let mut offered = Vec::new();
        'clients: for (index, client) in self.clients.iter().enumerate() {
            for listed in &client.listed {
                if offered.len() >= MAX_TOOLS {
                    tracing::warn!(
                        server = %client.server,
                        "the MCP servers list more tools than the transport offers; the rest are left out"
                    );
                    break 'clients;
                }
                offered.push((index, listed));
            }
        }
        let pairs: Vec<(&str, &str)> = offered
            .iter()
            .map(|(index, listed)| (self.clients[*index].server.as_str(), listed.raw.as_str()))
            .collect();
        let names = tool_names(&pairs);
        let tools: Vec<Tool> = offered
            .iter()
            .zip(names)
            .map(|((index, listed), name)| Tool {
                name,
                server: self.clients[*index].server.clone(),
                raw: listed.raw.clone(),
                description: listed.description.clone(),
                schema: listed.schema.clone(),
                client: *index,
            })
            .collect();
        self.names = tools
            .iter()
            .enumerate()
            .map(|(index, tool)| (tool.name.clone(), index))
            .collect();
        self.tools = tools;
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    #[must_use]
    pub fn offered(&self) -> Vec<(&str, usize)> {
        self.clients
            .iter()
            .enumerate()
            .map(|(index, client)| {
                let count = self
                    .tools
                    .iter()
                    .filter(|tool| tool.client == index)
                    .count();
                (client.server.as_str(), count)
            })
            .collect()
    }

    #[must_use]
    pub fn failures(&self) -> &[(String, String)] {
        &self.failures
    }

    #[must_use]
    pub fn definitions(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.schema,
                })
            })
            .collect()
    }

    #[must_use]
    pub fn find(&self, name: &str) -> Option<(&Client, &Tool)> {
        let tool = &self.tools[*self.names.get(name)?];
        Some((self.clients[tool.client].as_ref(), tool))
    }

    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<(&str, &str)> {
        self.find(name)
            .map(|(_, tool)| (tool.server.as_str(), tool.raw.as_str()))
    }

    pub fn take_images(&self) -> Vec<Image> {
        std::mem::take(&mut *lock(&self.images))
    }

    pub async fn call(
        &self,
        client: &Client,
        tool: &Tool,
        input: &Value,
    ) -> Result<String, String> {
        let arguments = if input.is_null() {
            json!({})
        } else {
            input.clone()
        };
        let result = client
            .transport
            .request(
                "tools/call",
                json!({ "name": tool.raw, "arguments": arguments }),
                Instant::now() + CALL_TIMEOUT,
            )
            .await
            .map_err(|error| {
                let reason = diagnostics::chain(&error);
                tracing::warn!(server = %client.server, tool = %tool.raw, "an MCP call failed: {reason}");
                format!("The {} MCP server failed: {reason}", client.server)
            })?;
        let rendered = render(&result);
        if !rendered.images.is_empty() {
            lock(&self.images).extend(rendered.images);
        }
        if result["isError"].as_bool() == Some(true) {
            return Err(if rendered.text.is_empty() {
                format!("The {} MCP server reported an error.", client.server)
            } else {
                rendered.text
            });
        }
        Ok(if rendered.text.is_empty() {
            "(no output)".to_owned()
        } else {
            rendered.text
        })
    }
}

impl Drop for Servers {
    fn drop(&mut self) {
        let Some((thread, generation)) = self.warm.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            drop(take_warm(&thread, generation));
            return;
        };
        runtime.spawn(async move {
            tokio::time::sleep(IDLE_TIMEOUT).await;
            if take_warm(&thread, generation).is_some() {
                tracing::info!(thread, "a thread's MCP servers were idle; they shut down");
            }
        });
    }
}

fn keep_warm(thread: String, set: Warm) -> Vec<Warm> {
    let mut warm = lock(&WARM);
    let mut retired: Vec<Warm> = warm.insert(thread, set).into_iter().collect();
    while warm.len() > MAX_WARM_THREADS {
        let Some(oldest) = warm
            .iter()
            .min_by_key(|(_, set)| set.generation)
            .map(|(thread, _)| thread.clone())
        else {
            break;
        };
        tracing::info!(
            thread = oldest,
            limit = MAX_WARM_THREADS,
            "more threads keep MCP servers warm than allowed; the longest unused set shuts down"
        );
        retired.extend(warm.remove(&oldest));
    }
    retired
}

fn take_warm(thread: &str, generation: u64) -> Option<Warm> {
    let mut warm = lock(&WARM);
    if warm
        .get(thread)
        .is_some_and(|set| set.generation == generation)
    {
        warm.remove(thread)
    } else {
        None
    }
}

fn fingerprint(servers: &[&McpServer], cwd: &Path, sandbox: &Sandbox) -> String {
    let mut hash = Sha256::new();
    hash.update(cwd.to_string_lossy().as_bytes());
    hash.update([u8::from(sandbox.confined())]);
    for server in servers {
        hash.update([0]);
        hash.update(serde_json::to_vec(server).unwrap_or_default());
    }
    hex::encode(hash.finalize())
}

async fn connect_client(
    server: &McpServer,
    cwd: &Path,
    sandbox: &Sandbox,
) -> Result<Client, String> {
    let resolved = resolve(server, &|name| std::env::var(name).ok()).inspect_err(|reason| {
        tracing::warn!(server = %server.name, step = "reading its config", "an MCP server could not start: {reason}");
    })?;
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let transport = Transport::open(&resolved, cwd, deadline, sandbox)
        .await
        .map_err(|error| {
            tracing::warn!(
                server = %server.name,
                step = "starting it",
                "an MCP server could not start: {}",
                redact(&diagnostics::chain(&error), &resolved.secrets)
            );
            format!("failed to start: {}", error.reason())
        })?;
    let listed = async {
        transport.initialize(deadline).await?;
        list_tools(&transport, &server.name, deadline)
            .await
            .map_err(StartError::ListTools)
    }
    .await;
    match listed {
        Ok(listed) => Ok(Client {
            server: server.name.clone(),
            listed,
            transport,
        }),
        Err(error) => {
            let exited = transport.exit_status().await;
            tracing::warn!(
                server = %server.name,
                step = %error,
                stderr = %redact(&transport.stderr_tail(), &resolved.secrets),
                "an MCP server could not start: {}",
                redact(&diagnostics::chain(&error), &resolved.secrets)
            );
            Err(exited.map_or_else(
                || format!("{error} failed: {}", error.reason()),
                |code| format!("failed to start: {code}"),
            ))
        }
    }
}

async fn list_tools(
    transport: &Transport,
    server: &str,
    deadline: Instant,
) -> Result<Vec<Listed>, RequestError> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let params = cursor
            .as_deref()
            .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
        let page_deadline = deadline.min(Instant::now() + REQUEST_TIMEOUT);
        let result = transport
            .request("tools/list", params, page_deadline)
            .await?;
        for tool in result["tools"].as_array().into_iter().flatten() {
            if tools.len() >= MAX_TOOLS {
                tracing::warn!(
                    server,
                    "an MCP server lists more tools than the transport offers; the rest are left out"
                );
                return Ok(tools);
            }
            let Some(raw) = tool["name"].as_str().filter(|name| !name.trim().is_empty()) else {
                continue;
            };
            let description =
                tool["description"]
                    .as_str()
                    .map_or_else(String::new, |description| {
                        let mut description = description.to_owned();
                        if description.len() > 2048 {
                            let end = description.floor_char_boundary(2048);
                            description.truncate(end);
                        }
                        description
                    });
            let schema = if tool["inputSchema"].is_object() {
                tool["inputSchema"].clone()
            } else {
                json!({ "type": "object" })
            };
            tools.push(Listed {
                raw: raw.to_owned(),
                description,
                schema,
            });
        }
        cursor = result["nextCursor"]
            .as_str()
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(tools)
}

#[must_use]
pub(super) fn prefixed_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", component(server), component(tool))
}

fn component(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "server".to_owned()
    } else {
        sanitized
    }
}

fn tool_names(tools: &[(&str, &str)]) -> Vec<String> {
    let plain: Vec<String> = tools
        .iter()
        .map(|(server, tool)| prefixed_name(server, tool))
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for name in &plain {
        *counts.entry(name.as_str()).or_default() += 1;
    }
    let keeps = |name: &str| name.len() <= MAX_NAME && counts.get(name) == Some(&1);
    let mut taken: HashSet<String> = plain
        .iter()
        .filter(|name| keeps(name.as_str()))
        .cloned()
        .collect();
    plain
        .iter()
        .zip(tools)
        .map(|(name, (server, tool))| {
            if keeps(name.as_str()) {
                return name.clone();
            }
            let mut salt = 0_u32;
            loop {
                let candidate = hashed_name(name, server, tool, salt);
                if taken.insert(candidate.clone()) {
                    return candidate;
                }
                salt += 1;
            }
        })
        .collect()
}

fn hashed_name(name: &str, server: &str, tool: &str, salt: u32) -> String {
    let mut hash = Sha256::new();
    hash.update(server.as_bytes());
    hash.update([0]);
    hash.update(tool.as_bytes());
    hash.update(salt.to_le_bytes());
    let digest = hex::encode(&hash.finalize()[..4]);
    let (kept, _) = name.split_at(name.floor_char_boundary(MAX_NAME - digest.len() - 1));
    format!("{kept}_{digest}")
}

struct Rendered {
    text: String,
    images: Vec<Image>,
}

fn render(result: &Value) -> Rendered {
    let mut parts = Vec::new();
    let mut images = Vec::new();
    for part in result["content"].as_array().into_iter().flatten() {
        let text = match part["type"].as_str() {
            Some("image") => Some(image(
                part["mimeType"].as_str(),
                part["data"].as_str(),
                &mut images,
            )),
            Some("audio") => Some(format!(
                "[audio ({}), which cannot be played here]",
                part["mimeType"].as_str().unwrap_or("audio")
            )),
            Some("resource") => Some(resource(&part["resource"], &mut images)),
            Some("resource_link") => Some(link(part)),
            _ => part["text"].as_str().map(str::to_owned),
        };
        parts.extend(text);
    }
    if parts.is_empty()
        && let Some(structured) = result
            .get("structuredContent")
            .filter(|structured| !structured.is_null())
    {
        parts.push(structured.to_string());
    }
    Rendered {
        text: parts.join("\n"),
        images,
    }
}

fn image(mime: Option<&str>, data: Option<&str>, images: &mut Vec<Image>) -> String {
    let mime = mime.unwrap_or("image/png").to_ascii_lowercase();
    if !matches!(
        mime.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) {
        return format!("[an image in {mime}, which cannot be shown]");
    }
    let encoded: String = data
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    let Ok(data) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return "[an image that could not be decoded]".to_owned();
    };
    if data.is_empty() {
        return "[an empty image]".to_owned();
    }
    if data.len() > MAX_IMAGE_BYTES {
        return format!(
            "[an image larger than {}, not attached]",
            size(MAX_IMAGE_BYTES)
        );
    }
    if images.len() >= MAX_IMAGES {
        return format!("[an image left out: one result attaches at most {MAX_IMAGES}]");
    }
    let note = format!("[image ({mime}): attached to this message]");
    images.push(Image { mime, data });
    note
}

fn resource(resource: &Value, images: &mut Vec<Image>) -> String {
    let uri = resource["uri"].as_str().unwrap_or("resource");
    if let Some(text) = resource["text"].as_str() {
        return format!("<resource uri=\"{uri}\">\n{text}\n</resource>");
    }
    let mime = resource["mimeType"].as_str();
    if let Some(blob) = resource["blob"].as_str()
        && mime.is_some_and(|mime| mime.starts_with("image/"))
    {
        return image(mime, Some(blob), images);
    }
    mime.map_or_else(
        || format!("[resource {uri}]"),
        |mime| format!("[resource {uri} ({mime}): binary, not shown]"),
    )
}

fn link(part: &Value) -> String {
    let uri = part["uri"].as_str().unwrap_or_default();
    let mut text = part["name"]
        .as_str()
        .or_else(|| part["title"].as_str())
        .map_or_else(
            || format!("[resource link: <{uri}>"),
            |name| format!("[resource link: {name} <{uri}>"),
        );
    if let Some(description) = part["description"].as_str() {
        text.push_str(" — ");
        text.push_str(description);
    }
    text.push(']');
    text
}

fn size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{} MB", bytes / (1024 * 1024))
    } else {
        format!("{} KB", bytes / 1024)
    }
}

struct Resolved {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    url: Option<String>,
    headers: BTreeMap<String, String>,
    sse: bool,
    secrets: Vec<String>,
}

fn resolve(
    server: &McpServer,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Resolved, String> {
    let mut expander = Expander {
        lookup,
        missing: Vec::new(),
        inserted: Vec::new(),
    };
    let command = expander.expand(&server.command);
    let args = server.args.iter().map(|arg| expander.expand(arg)).collect();
    let env: BTreeMap<String, String> = server
        .env
        .iter()
        .map(|(key, value)| (key.clone(), expander.expand(value)))
        .collect();
    let url = server
        .url
        .as_deref()
        .filter(|url| !url.is_empty())
        .map(|url| expander.expand(url));
    let headers: BTreeMap<String, String> = server
        .headers
        .iter()
        .map(|(key, value)| (key.clone(), expander.expand(value)))
        .collect();
    match expander.missing.as_slice() {
        [] => {}
        [name] => return Err(format!("the environment variable {name} is not set")),
        names => {
            return Err(format!(
                "the environment variables {} are not set",
                names.join(", ")
            ));
        }
    }
    if url.is_none() && command.trim().is_empty() {
        return Err("it has neither a command nor a url".to_owned());
    }
    let mut secrets: Vec<String> = env
        .values()
        .chain(headers.values())
        .cloned()
        .chain(expander.inserted)
        .filter(|value| value.chars().count() >= MIN_SECRET_CHARS)
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    Ok(Resolved {
        command,
        args,
        env,
        url,
        headers,
        sse: server.sse,
        secrets,
    })
}

struct Expander<'a> {
    lookup: &'a dyn Fn(&str) -> Option<String>,
    missing: Vec<String>,
    inserted: Vec<String>,
}

impl Expander<'_> {
    fn expand(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("${") {
            let (before, opened) = rest.split_at(start);
            out.push_str(before);
            let (_, after) = opened.split_at("${".len());
            let Some((inner, closed)) =
                after.split_once('}').filter(|(inner, _)| !inner.is_empty())
            else {
                out.push_str("${");
                rest = after;
                continue;
            };
            let (name, default) = match inner.split_once(":-") {
                Some((name, default)) => (name, Some(default)),
                None => (inner, None),
            };
            match ((self.lookup)(name), default) {
                (Some(value), _) => {
                    out.push_str(&value);
                    self.inserted.push(value);
                }
                (None, Some(default)) => out.push_str(default),
                (None, None) => {
                    if !self.missing.iter().any(|known| known == name) {
                        self.missing.push(name.to_owned());
                    }
                }
            }
            rest = closed;
        }
        out.push_str(rest);
        out
    }
}

fn redact(text: &str, secrets: &[String]) -> String {
    secrets.iter().fold(text.to_owned(), |text, secret| {
        text.replace(secret, "[redacted]")
    })
}

#[derive(Debug, thiserror::Error)]
#[error("{message} (code {code})")]
struct RpcError {
    code: i64,
    message: String,
}

fn response_result(reply: &Value) -> Result<Value, RequestError> {
    if let Some(error) = reply.get("error") {
        return Err(RequestError::Rpc(RpcError {
            code: error["code"].as_i64().unwrap_or_default(),
            message: error["message"]
                .as_str()
                .unwrap_or("the server failed")
                .to_owned(),
        }));
    }
    Ok(reply["result"].clone())
}

fn answer_to(message: &Value) -> Option<Value> {
    let method = message.get("method")?.as_str()?;
    let id = message.get("id").filter(|id| !id.is_null())?;
    Some(if method == "ping" {
        json!({ "jsonrpc": "2.0", "id": id, "result": {} })
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": format!("{method} is not supported") },
        })
    })
}

fn batch(value: Value) -> Vec<Value> {
    match value {
        Value::Array(values) => values,
        value @ (Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Object(_)) => vec![value],
    }
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": "artisan",
            "title": "Artisan Editor",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn cancelled(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": id, "reason": "The user stopped the call or it timed out." },
    })
}

#[derive(Debug)]
enum Transport {
    Stream(Stream),
    Http(Arc<Http>),
}

impl Transport {
    async fn open(
        server: &Resolved,
        cwd: &Path,
        deadline: Instant,
        sandbox: &Sandbox,
    ) -> Result<Self, OpenError> {
        match &server.url {
            Some(url) if server.sse => Ok(Self::Stream(
                Stream::sse(url, header_map(&server.headers)?, deadline).await?,
            )),
            Some(url) => Ok(Self::Http(Arc::new(Http::new(
                url,
                header_map(&server.headers)?,
            )?))),
            None => Ok(Self::Stream(Stream::stdio(
                server,
                cwd,
                MAX_MESSAGE_BYTES,
                sandbox,
            )?)),
        }
    }

    async fn initialize(&self, deadline: Instant) -> Result<(), StartError> {
        self.request("initialize", initialize_params(), deadline)
            .await
            .map_err(StartError::Initialize)?;
        self.notify("notifications/initialized", json!({}))
            .await
            .map_err(StartError::Initialized)
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, RequestError> {
        match self {
            Self::Stream(stream) => stream.request(method, params, deadline).await,
            Self::Http(http) => http.request(method, params, deadline).await,
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), NotifyError> {
        match self {
            Self::Stream(stream) => stream.notify(method, &params),
            Self::Http(http) => http.notify(method, params).await,
        }
    }

    fn alive(&self) -> bool {
        match self {
            Self::Stream(stream) => stream.rpc.alive.load(Ordering::SeqCst),
            Self::Http(http) => http.alive.load(Ordering::SeqCst),
        }
    }

    fn stderr_tail(&self) -> String {
        match self {
            Self::Stream(stream) => lock(&stream.stderr).trim().to_owned(),
            Self::Http(_) => String::new(),
        }
    }

    async fn exit_status(&self) -> Option<String> {
        let Self::Stream(Stream {
            child: Some(child), ..
        }) = self
        else {
            return None;
        };
        for _ in 0..20 {
            let status = lock(child).try_wait().ok().flatten();
            if let Some(status) = status {
                return Some(status.code().map_or_else(
                    || "it was stopped by a signal".to_owned(),
                    |code| format!("exited with status {code}"),
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        None
    }
}

#[derive(Debug)]
struct Rpc {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, Undelivered>>>>,
    alive: AtomicBool,
}

impl Rpc {
    fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            pending: Mutex::default(),
            alive: AtomicBool::new(true),
        }
    }

    fn register(&self) -> (u64, oneshot::Receiver<Result<Value, Undelivered>>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        lock(&self.pending).insert(id, sender);
        (id, receiver)
    }

    fn forget(&self, id: u64) {
        lock(&self.pending).remove(&id);
    }

    fn fail(&self, id: u64, reason: Undelivered) {
        let waiting = lock(&self.pending).remove(&id);
        if let Some(waiting) = waiting {
            let _requester_gone = waiting.send(Err(reason));
        }
    }

    fn close(&self, reason: &Closed) {
        self.alive.store(false, Ordering::SeqCst);
        let pending = std::mem::take(&mut *lock(&self.pending));
        for waiting in pending.into_values() {
            let _requester_gone = waiting.send(Err(Undelivered::Closed(reason.clone())));
        }
    }

    fn receive(&self, message: Value) -> Option<Value> {
        if message.get("method").is_some() {
            return answer_to(&message);
        }
        let id = message.get("id").and_then(Value::as_u64)?;
        let waiting = lock(&self.pending).remove(&id);
        if let Some(waiting) = waiting {
            let _requester_gone = waiting.send(Ok(message));
        }
        None
    }
}

struct Queued {
    text: String,
    request: Option<u64>,
}

enum Refused {
    Backlogged,
    Stopped,
}

#[derive(Debug, Clone)]
struct Outbox {
    sender: mpsc::UnboundedSender<Queued>,
    bytes: Arc<AtomicUsize>,
}

impl Outbox {
    fn new() -> (Self, Inbox) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let bytes = Arc::new(AtomicUsize::new(0));
        (
            Self {
                sender,
                bytes: Arc::clone(&bytes),
            },
            Inbox { receiver, bytes },
        )
    }

    fn send(&self, message: &Value) -> Result<(), Refused> {
        let text = message.to_string();
        let request = message
            .get("method")
            .and_then(|_| message.get("id"))
            .and_then(Value::as_u64);
        let size = text.len();
        if self.bytes.fetch_add(size, Ordering::AcqRel) + size > MAX_QUEUED_BYTES {
            self.bytes.fetch_sub(size, Ordering::AcqRel);
            return Err(Refused::Backlogged);
        }
        self.sender
            .send(Queued { text, request })
            .map_err(|_closed| {
                self.bytes.fetch_sub(size, Ordering::AcqRel);
                Refused::Stopped
            })
    }
}

struct Inbox {
    receiver: mpsc::UnboundedReceiver<Queued>,
    bytes: Arc<AtomicUsize>,
}

impl Inbox {
    async fn recv(&mut self) -> Option<Queued> {
        self.receiver.recv().await
    }

    fn sent(&self, queued: &Queued) {
        self.bytes.fetch_sub(queued.text.len(), Ordering::AcqRel);
    }
}

fn answer(rpc: &Rpc, outgoing: &Outbox, message: &Value) -> bool {
    match outgoing.send(message) {
        Ok(()) | Err(Refused::Stopped) => true,
        Err(Refused::Backlogged) => {
            tracing::warn!(
                limit = MAX_QUEUED_BYTES,
                "an MCP server stopped reading while it kept asking; it is disconnected"
            );
            rpc.close(&Closed::Backlogged);
            false
        }
    }
}

#[derive(Debug)]
struct Stream {
    rpc: Arc<Rpc>,
    outgoing: Outbox,
    child: Option<Mutex<crate::process::OwnedChild>>,
    stderr: Arc<Mutex<String>>,
    _tasks: Vec<AbortOnDropHandle<()>>,
}

impl Stream {
    fn stdio(
        server: &Resolved,
        cwd: &Path,
        max_line: usize,
        sandbox: &Sandbox,
    ) -> Result<Self, OpenError> {
        let mut command = tokio::process::Command::new(&server.command);
        command
            .args(&server.args)
            .envs(server.env.iter())
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child =
            crate::process::owned_command(command, sandbox.clone()).map_err(|source| {
                OpenError::Spawn {
                    command: server.command.clone(),
                    source,
                }
            })?;
        let stdin = child
            .stdin()
            .take()
            .expect("invariant: a server started with piped stdin has its stdin");
        let stdout = child
            .stdout()
            .take()
            .expect("invariant: a server started with piped stdout has its stdout");
        let stderr = Arc::new(Mutex::new(String::new()));
        let rpc = Arc::new(Rpc::new());
        let (outgoing, messages) = Outbox::new();
        let mut tasks = vec![
            AbortOnDropHandle::new(tokio::spawn(write_lines(stdin, messages, Arc::clone(&rpc)))),
            AbortOnDropHandle::new(tokio::spawn(read_lines(
                stdout,
                Arc::clone(&rpc),
                outgoing.clone(),
                max_line,
            ))),
        ];
        if let Some(pipe) = child.stderr().take() {
            tasks.push(AbortOnDropHandle::new(tokio::spawn(keep_tail(
                pipe,
                Arc::clone(&stderr),
            ))));
        }
        Ok(Self {
            rpc,
            outgoing,
            child: Some(Mutex::new(child)),
            stderr,
            _tasks: tasks,
        })
    }

    async fn sse(url: &str, headers: HeaderMap, deadline: Instant) -> Result<Self, OpenError> {
        let client = http_client()?;
        let base = reqwest::Url::parse(url).map_err(OpenError::InvalidUrl)?;
        let response = tokio::time::timeout_at(
            deadline,
            client
                .get(base.clone())
                .headers(headers.clone())
                .header("accept", "text/event-stream")
                .send(),
        )
        .await
        .map_err(|_elapsed| OpenError::StreamTimedOut)?
        .map_err(|error| OpenError::Unreachable(error.without_url()))?;
        if !response.status().is_success() {
            return Err(OpenError::Status {
                status: response.status().as_u16(),
            });
        }
        let mut events = sse_events(response);
        let endpoint = loop {
            let event = tokio::time::timeout_at(deadline, events.next())
                .await
                .map_err(|_elapsed| OpenError::EndpointTimedOut)?;
            match event {
                Some(Ok(SseEvent::Message(message))) if message.event == "endpoint" => {
                    break base
                        .join(message.data.trim())
                        .map_err(OpenError::InvalidEndpoint)?;
                }
                Some(Ok(SseEvent::Message(_) | SseEvent::Retry(_))) => {}
                Some(Err(_)) | None => return Err(OpenError::NoEndpoint),
            }
        };
        if endpoint.origin() != base.origin() {
            return Err(OpenError::ForeignEndpoint);
        }
        let rpc = Arc::new(Rpc::new());
        let (outgoing, messages) = Outbox::new();
        let tasks = vec![
            AbortOnDropHandle::new(tokio::spawn(read_events(
                events,
                Arc::clone(&rpc),
                outgoing.clone(),
            ))),
            AbortOnDropHandle::new(tokio::spawn(post_messages(
                client,
                endpoint,
                headers,
                messages,
                Arc::clone(&rpc),
            ))),
        ];
        Ok(Self {
            rpc,
            outgoing,
            child: None,
            stderr: Arc::default(),
            _tasks: tasks,
        })
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, RequestError> {
        let limit = deadline.saturating_duration_since(Instant::now());
        let (id, reply) = self.rpc.register();
        let mut pending = Pending {
            stream: self,
            id,
            cancel: method != "initialize",
        };
        if !self.rpc.alive.load(Ordering::SeqCst) {
            pending.cancel = false;
            return Err(RequestError::Stopped);
        }
        match self
            .outgoing
            .send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        {
            Ok(()) => {}
            Err(Refused::Stopped) => {
                pending.cancel = false;
                return Err(RequestError::Stopped);
            }
            Err(Refused::Backlogged) => {
                pending.cancel = false;
                return Err(RequestError::Undelivered(Undelivered::Closed(
                    Closed::Backlogged,
                )));
            }
        }
        let Ok(answered) = tokio::time::timeout_at(deadline, reply).await else {
            return Err(RequestError::TimedOut {
                method: method.to_owned(),
                seconds: limit.as_secs_f32().round(),
            });
        };
        pending.cancel = false;
        let reply = answered
            .unwrap_or(Err(Undelivered::Closed(Closed::Output)))
            .map_err(RequestError::Undelivered)?;
        response_result(&reply)
    }

    fn notify(&self, method: &str, params: &Value) -> Result<(), NotifyError> {
        match self
            .outgoing
            .send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
        {
            Ok(()) => Ok(()),
            Err(Refused::Stopped) => Err(NotifyError::Stopped),
            Err(Refused::Backlogged) => Err(NotifyError::Backlogged),
        }
    }
}

#[derive(Debug)]
struct Pending<'a> {
    stream: &'a Stream,
    id: u64,
    cancel: bool,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.stream.rpc.forget(self.id);
        if self.cancel && self.stream.rpc.alive.load(Ordering::SeqCst) {
            answer(&self.stream.rpc, &self.stream.outgoing, &cancelled(self.id));
        }
    }
}

async fn write_lines(mut stdin: tokio::process::ChildStdin, mut messages: Inbox, rpc: Arc<Rpc>) {
    while let Some(message) = messages.recv().await {
        let written = async {
            stdin.write_all(message.text.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        }
        .await;
        messages.sent(&message);
        if let Err(error) = written {
            tracing::debug!("writing to an MCP server: {error}");
            rpc.close(&Closed::Input);
            break;
        }
    }
}

async fn read_lines(
    stdout: impl AsyncRead + Unpin,
    rpc: Arc<Rpc>,
    outgoing: Outbox,
    max_line: usize,
) {
    let mut lines = Lines::new(stdout, max_line);
    loop {
        match lines.next().await {
            Ok(Some(Line::Message(line))) => {
                let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                    continue;
                };
                if !answer_all(&rpc, &outgoing, value) {
                    break;
                }
            }
            Ok(Some(Line::Oversized(id))) => {
                tracing::warn!(
                    id,
                    limit = max_line,
                    "an MCP server wrote a message over the limit; it was skipped"
                );
                if let Some(id) = id {
                    rpc.fail(id, Undelivered::TooLarge { limit: max_line });
                }
            }
            Ok(None) => {
                rpc.close(&Closed::Output);
                break;
            }
            Err(error) => {
                tracing::debug!("reading an MCP server: {error}");
                rpc.close(&Closed::OutputFailed(Arc::new(error)));
                break;
            }
        }
    }
}

async fn keep_tail(mut pipe: tokio::process::ChildStderr, tail: Arc<Mutex<String>>) {
    let mut buffer = vec![0_u8; 4096];
    while let Ok(read) = pipe.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&buffer[..read]);
        let mut tail = lock(&tail);
        tail.push_str(&text);
        if tail.len() > MAX_STDERR {
            let cut = tail.ceil_char_boundary(tail.len() - MAX_STDERR);
            tail.drain(..cut);
        }
    }
}

enum Line {
    Message(Vec<u8>),
    Oversized(Option<u64>),
}

struct Lines<R> {
    inner: R,
    buffer: Vec<u8>,
    start: usize,
    end: usize,
    max: usize,
}

impl<R: AsyncRead + Unpin> Lines<R> {
    fn new(inner: R, max: usize) -> Self {
        Self {
            inner,
            buffer: vec![0; 64 * 1024],
            start: 0,
            end: 0,
            max,
        }
    }

    async fn next(&mut self) -> std::io::Result<Option<Line>> {
        let mut line = Vec::new();
        let mut skipped: Option<IdScan> = None;
        loop {
            if self.start == self.end {
                let read = self.inner.read(&mut self.buffer).await?;
                if read == 0 {
                    return Ok(None);
                }
                self.start = 0;
                self.end = read;
            }
            let available = &self.buffer[self.start..self.end];
            let newline = available.iter().position(|&byte| byte == b'\n');
            let part = &available[..newline.unwrap_or(available.len())];
            if let Some(scan) = &mut skipped {
                scan.feed(part);
            } else if line.len() + part.len() > self.max {
                let mut scan = IdScan::default();
                scan.feed(&line);
                scan.feed(part);
                line = Vec::new();
                skipped = Some(scan);
            } else {
                line.extend_from_slice(part);
            }
            self.start += part.len() + usize::from(newline.is_some());
            if newline.is_some() {
                return Ok(Some(match skipped {
                    Some(scan) => Line::Oversized(scan.id),
                    None => Line::Message(line),
                }));
            }
        }
    }
}

#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a byte-at-a-time scanner's state, read together in one match"
)]
struct IdScan {
    depth: u32,
    in_string: bool,
    escaped: bool,
    expect_key: bool,
    reading_key: bool,
    key: Vec<u8>,
    id_value_next: bool,
    number: Option<u64>,
    id: Option<u64>,
}

impl IdScan {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.step(byte);
        }
    }

    fn step(&mut self, byte: u8) {
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if byte == b'\\' {
                self.escaped = true;
            } else if byte == b'"' {
                self.in_string = false;
                if self.reading_key {
                    self.reading_key = false;
                    self.id_value_next = self.key == b"id";
                }
                return;
            }
            if self.reading_key && self.key.len() <= 2 {
                self.key.push(byte);
            }
            return;
        }
        if let Some(number) = self.number {
            if byte.is_ascii_digit() {
                self.number = number
                    .checked_mul(10)
                    .and_then(|number| number.checked_add(u64::from(byte - b'0')));
                return;
            }
            self.number = None;
            self.id = self.id.or(Some(number));
        }
        match byte {
            b'"' => {
                self.in_string = true;
                if self.depth == 1 && self.expect_key {
                    self.expect_key = false;
                    self.reading_key = true;
                    self.key.clear();
                }
                self.id_value_next = false;
            }
            b'{' | b'[' => {
                self.depth += 1;
                self.expect_key = self.depth == 1 && byte == b'{';
                self.id_value_next = false;
            }
            b'}' | b']' => self.depth = self.depth.saturating_sub(1),
            b',' => self.expect_key = self.depth == 1,
            b':' if self.depth == 1 => {}
            b'0'..=b'9' if self.depth == 1 && self.id_value_next => {
                self.id_value_next = false;
                self.number = Some(u64::from(byte - b'0'));
            }
            b' ' | b'\t' | b'\r' | b'\n' => {}
            _ => self.id_value_next = false,
        }
    }
}

type Events = BoxStream<'static, Result<SseEvent, EventError>>;

enum EventError {
    TooLarge,
    Broken,
}

fn sse_events(response: reqwest::Response) -> Events {
    let mut stream = SseStream::with_decoder(SseDecoder::with_limit(MAX_EVENT_BYTES));
    stream.attach(response.bytes_stream());
    stream
        .map(|event| {
            event.map_err(|error| match error {
                SseStreamError::PayloadTooLarge(_) => EventError::TooLarge,
                SseStreamError::Inner(_) => EventError::Broken,
            })
        })
        .boxed()
}

fn answer_all(rpc: &Rpc, outgoing: &Outbox, value: Value) -> bool {
    batch(value)
        .into_iter()
        .filter_map(|message| rpc.receive(message))
        .all(|reply| answer(rpc, outgoing, &reply))
}

async fn read_events(mut events: Events, rpc: Arc<Rpc>, outgoing: Outbox) {
    while let Some(event) = events.next().await {
        match event {
            Ok(SseEvent::Message(message)) if message.event == "message" => {
                let Ok(value) = serde_json::from_str::<Value>(&message.data) else {
                    continue;
                };
                if !answer_all(&rpc, &outgoing, value) {
                    return;
                }
            }
            Ok(SseEvent::Message(_) | SseEvent::Retry(_)) => {}
            Err(EventError::TooLarge) => {
                tracing::warn!("an MCP server sent an event over the limit; it was skipped");
            }
            Err(EventError::Broken) => break,
        }
    }
    rpc.close(&Closed::EventStream);
}

async fn post_messages(
    client: reqwest::Client,
    endpoint: reqwest::Url,
    headers: HeaderMap,
    mut messages: Inbox,
    rpc: Arc<Rpc>,
) {
    while let Some(message) = messages.recv().await {
        messages.sent(&message);
        let id = message.request;
        let sent = client
            .post(endpoint.clone())
            .headers(headers.clone())
            .header("content-type", "application/json")
            .body(message.text)
            .send()
            .await;
        let failure = match sent {
            Ok(response) if response.status().is_success() => None,
            Ok(response) => Some(Undelivered::Status {
                status: response.status().as_u16(),
            }),
            Err(error) => {
                let error = error.without_url();
                tracing::debug!("posting to an MCP server: {error}");
                Some(Undelivered::Unreachable(error))
            }
        };
        if let (Some(reason), Some(id)) = (failure, id) {
            rpc.fail(id, reason);
        }
    }
}

fn header_map(headers: &BTreeMap<String, String>) -> Result<HeaderMap, OpenError> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let header = HeaderName::from_bytes(name.trim().as_bytes()).map_err(|source| {
            OpenError::HeaderName {
                name: name.clone(),
                source,
            }
        })?;
        let mut value =
            HeaderValue::from_str(value.trim()).map_err(|source| OpenError::HeaderValue {
                name: name.clone(),
                source,
            })?;
        value.set_sensitive(true);
        map.insert(header, value);
    }
    Ok(map)
}

fn http_client() -> Result<reqwest::Client, OpenError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(format!("artisan/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(OpenError::Client)
}

fn reach_error(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "the request timed out"
    } else if error.is_connect() {
        "could not connect to the server"
    } else {
        "the request to the server failed"
    }
}

#[derive(Debug)]
struct Http {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    session: Mutex<Option<String>>,
    protocol: Mutex<Option<String>>,
    next_id: AtomicU64,
    alive: AtomicBool,
}

impl Http {
    fn new(url: &str, headers: HeaderMap) -> Result<Self, OpenError> {
        reqwest::Url::parse(url).map_err(OpenError::InvalidUrl)?;
        Ok(Self {
            client: http_client()?,
            url: url.to_owned(),
            headers,
            session: Mutex::new(None),
            protocol: Mutex::new(None),
            next_id: AtomicU64::new(1),
            alive: AtomicBool::new(true),
        })
    }

    async fn request(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, RequestError> {
        let limit = deadline.saturating_duration_since(Instant::now());
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut pending = HttpPending {
            http: Arc::clone(self),
            id,
            cancel: method != "initialize",
        };
        let Ok(reply) = tokio::time::timeout_at(deadline, self.exchange(id, method, &params)).await
        else {
            return Err(RequestError::TimedOut {
                method: method.to_owned(),
                seconds: limit.as_secs_f32().round(),
            });
        };
        pending.cancel = false;
        response_result(&reply?)
    }

    async fn exchange(&self, id: u64, method: &str, params: &Value) -> Result<Value, RequestError> {
        match self.round_trip(id, method, params).await {
            Err(RequestError::SessionGone) if method != "initialize" => {
                tracing::info!(
                    method,
                    "an MCP server ended its session; starting a new one"
                );
                lock(&self.session).take();
                lock(&self.protocol).take();
                let start = self.next_id.fetch_add(1, Ordering::Relaxed);
                let started = self
                    .round_trip(start, "initialize", &initialize_params())
                    .await
                    .map_err(|error| RequestError::Restart(Box::new(error)))?;
                response_result(&started)
                    .map_err(|error| RequestError::Restart(Box::new(error)))?;
                self.notify("notifications/initialized", json!({}))
                    .await
                    .map_err(RequestError::RestartNotify)?;
                self.round_trip(id, method, params).await
            }
            other => other,
        }
    }

    async fn round_trip(
        &self,
        id: u64,
        method: &str,
        params: &Value,
    ) -> Result<Value, RequestError> {
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let (response, named_session) = self
            .post(&message)
            .await
            .map_err(RequestError::Unreachable)?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND && named_session {
            return Err(RequestError::SessionGone);
        }
        if !status.is_success() {
            return Err(RequestError::Status {
                status: status.as_u16(),
            });
        }
        let streamed = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/event-stream"));
        let reply = if streamed {
            self.reply_from_events(sse_events(response), id, method)
                .await?
        } else {
            let body = read_body(response, MAX_MESSAGE_BYTES)
                .await
                .map_err(RequestError::Body)?;
            let value: Value =
                serde_json::from_slice(&body).map_err(|source| RequestError::NotJson {
                    method: method.to_owned(),
                    source,
                })?;
            batch(value)
                .into_iter()
                .find(|reply| reply.get("method").is_none() && reply["id"].as_u64() == Some(id))
                .ok_or_else(|| RequestError::NoAnswer {
                    method: method.to_owned(),
                })?
        };
        if method == "initialize"
            && let Some(version) = reply["result"]["protocolVersion"].as_str()
        {
            *lock(&self.protocol) = Some(version.to_owned());
        }
        Ok(reply)
    }

    async fn reply_from_events(
        &self,
        mut events: Events,
        id: u64,
        method: &str,
    ) -> Result<Value, RequestError> {
        while let Some(event) = events.next().await {
            match event {
                Ok(SseEvent::Message(message)) => {
                    let Ok(value) = serde_json::from_str::<Value>(&message.data) else {
                        continue;
                    };
                    for message in batch(value) {
                        if let Some(answer) = answer_to(&message) {
                            if let Err(error) = self.post(&answer).await {
                                tracing::debug!("answering an MCP server's request: {error}");
                            }
                            continue;
                        }
                        if message.get("method").is_none() && message["id"].as_u64() == Some(id) {
                            return Ok(message);
                        }
                    }
                }
                Ok(SseEvent::Retry(_)) => {}
                Err(EventError::TooLarge) => {
                    return Err(RequestError::EventTooLarge {
                        method: method.to_owned(),
                        limit: MAX_MESSAGE_BYTES,
                    });
                }
                Err(EventError::Broken) => {
                    return Err(RequestError::EventBrokeOff {
                        method: method.to_owned(),
                    });
                }
            }
        }
        Err(RequestError::Unanswered {
            method: method.to_owned(),
        })
    }

    async fn post(&self, message: &Value) -> Result<(reqwest::Response, bool), reqwest::Error> {
        let mut request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(message.to_string());
        let session = lock(&self.session).clone();
        if let Some(session) = session.as_deref() {
            request = request.header("mcp-session-id", session);
        }
        let protocol = lock(&self.protocol).clone();
        if let Some(protocol) = protocol.as_deref() {
            request = request.header("mcp-protocol-version", protocol);
        }
        let response = request.send().await.map_err(|error| {
            if error.is_connect() {
                self.alive.store(false, Ordering::SeqCst);
            }
            let error = error.without_url();
            tracing::debug!("reaching an MCP server: {error}");
            error
        })?;
        if let Some(given) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            *lock(&self.session) = Some(given.to_owned());
        }
        Ok((response, session.is_some()))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), NotifyError> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let (response, _) = self
            .post(&message)
            .await
            .map_err(NotifyError::Unreachable)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(NotifyError::Status {
                status: response.status().as_u16(),
            })
        }
    }
}

struct HttpPending {
    http: Arc<Http>,
    id: u64,
    cancel: bool,
}

impl Drop for HttpPending {
    fn drop(&mut self) {
        if !self.cancel {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let http = Arc::clone(&self.http);
        let id = self.id;
        runtime.spawn(async move {
            let params = cancelled(id)["params"].clone();
            let sent = tokio::time::timeout(
                Duration::from_secs(10),
                http.notify("notifications/cancelled", params),
            )
            .await;
            if let Ok(Err(error)) = sent {
                tracing::debug!("cancelling an MCP call: {}", diagnostics::chain(&error));
            }
        });
    }
}

async fn read_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, BodyError> {
    let too_large = || BodyError::TooLarge { limit };
    if response
        .content_length()
        .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
    {
        return Err(too_large());
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|error| BodyError::BrokeOff(error.without_url()))?;
        if body.len() + chunk.len() > limit {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[must_use]
pub(super) fn project_servers(cwd: &Path) -> Vec<McpServer> {
    let mut dirs: Vec<&Path> = cwd.ancestors().collect();
    if let Some(root) = dirs.iter().position(|dir| dir.join(".git").exists()) {
        dirs.truncate(root + 1);
    } else {
        dirs.truncate(1);
    }
    let mut servers: Vec<McpServer> = Vec::new();
    for dir in dirs.into_iter().rev() {
        for server in from_file(&dir.join(".mcp.json")) {
            if servers.len() >= MAX_PROJECT_SERVERS {
                return servers;
            }
            if let Some(slot) = servers.iter_mut().find(|known| known.name == server.name) {
                *slot = server;
            } else {
                servers.push(server);
            }
        }
    }
    servers
}

fn strings(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect()
}

fn from_file(path: &Path) -> Vec<McpServer> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if metadata.len() > MAX_MCP_JSON_BYTES {
        tracing::warn!(file = %path.display(), "a project .mcp.json is too large to read");
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        tracing::warn!(file = %path.display(), "a project .mcp.json is not valid JSON");
        return Vec::new();
    };
    value["mcpServers"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, entry)| {
            if let Some(url) = entry["url"].as_str().filter(|url| !url.is_empty()) {
                return Some(McpServer {
                    name: name.clone(),
                    url: Some(url.to_owned()),
                    headers: strings(&entry["headers"]),
                    sse: entry["type"].as_str() == Some("sse"),
                    ..McpServer::default()
                });
            }
            let command = entry["command"].as_str()?;
            if command.trim().is_empty() {
                return None;
            }
            Some(McpServer {
                name: name.clone(),
                command: command.to_owned(),
                args: entry["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                env: strings(&entry["env"]),
                ..McpServer::default()
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/claudeai/mcp/fakes.rs"]
pub(super) mod fakes;

#[cfg(test)]
#[path = "../../tests/claudeai/mcp/unit.rs"]
mod tests;
