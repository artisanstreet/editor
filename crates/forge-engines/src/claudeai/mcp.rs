//! An MCP client for the claude.ai transport.
//!
//! Claude Code starts the thread's MCP servers itself; the claude.ai
//! transport has no process, so it speaks to them here. stdio servers are
//! started with their command, env and args; remote servers speak JSON-RPC
//! over Streamable HTTP, or over the legacy HTTP+SSE transport. Either way
//! the transport lists the server's tools, offers them to the model under
//! unique `mcp__server__tool` names, and runs the calls Forge approves.
//!
//! A thread's servers stay connected between its turns ([`for_thread`]):
//! the next turn with the same servers reuses them, a changed config or
//! [`IDLE_TIMEOUT`] without a turn shuts them down, and one that died is
//! started again.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
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

use crate::process::MAX_LINE_BYTES;

/// The protocol version the client asks for.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// How long listing a server's tools may take, page by page.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a tool call may take, from the moment it is sent. Progress
/// notifications do not extend it.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// How long starting a server and listing its tools may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a thread's servers stay connected without a turn.
pub const IDLE_TIMEOUT: Duration = Duration::from_mins(10);
/// The most tools all servers together offer.
const MAX_TOOLS: usize = 256;
/// The longest tool name claude.ai takes.
const MAX_NAME: usize = 64;
/// The most of a project's `.mcp.json` that is read, and the most servers
/// it may hold.
const MAX_MCP_JSON_BYTES: u64 = 1024 * 1024;
const MAX_PROJECT_SERVERS: usize = 64;
/// The most `tools/list` pages read.
const MAX_PAGES: usize = 20;
/// The largest message a server may send: a stdio line, an SSE event or an
/// HTTP body.
const MAX_MESSAGE_BYTES: usize = MAX_LINE_BYTES;
const MAX_EVENT_BYTES: NonZeroUsize = NonZeroUsize::new(MAX_MESSAGE_BYTES).unwrap();
/// The most of a server's stderr kept for a failed start.
const MAX_STDERR: usize = 8 * 1024;
/// The largest image a result attaches, and the most one result attaches.
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_IMAGES: usize = 8;
/// Values shorter than this are not redacted from logs: they are flags
/// (`1`, `true`), not secrets, and redacting them garbles the text.
const MIN_SECRET_CHARS: usize = 4;

/// Every MCP server one run connected to.
#[derive(Default)]
pub struct Servers {
    clients: Vec<Arc<Client>>,
    /// Every tool offered, under its model-facing name.
    tools: Vec<Tool>,
    /// A model-facing name → its index in `tools`.
    names: HashMap<String, usize>,
    failures: Vec<(String, String)>,
    /// Images calls returned that have not gone to claude.ai yet.
    images: Mutex<Vec<Image>>,
    /// The thread whose warm servers these are, and which turn's use.
    warm: Option<(String, u64)>,
}

/// One connected server.
pub struct Client {
    pub server: String,
    listed: Vec<Listed>,
    transport: Transport,
}

/// One tool as its server lists it.
struct Listed {
    raw: String,
    description: String,
    schema: Value,
}

/// One tool offered to the model.
#[derive(Clone, Debug)]
pub struct Tool {
    /// The name the model sees: `mcp__server__tool`, shortened with a hash
    /// when it is too long or two tools would share it.
    pub name: String,
    /// The server's name, as configured.
    pub server: String,
    /// The tool's name on its server.
    pub raw: String,
    pub description: String,
    pub schema: Value,
    /// Its client, in [`Servers::clients`].
    client: usize,
}

/// An image a tool returned, for the message that carries the results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub mime: String,
    pub data: Vec<u8>,
}

impl Image {
    /// The file extension claude.ai's upload reads the type from.
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

/// A thread's connected servers, kept between its turns.
struct Warm {
    /// The servers' config and working directory they were started with.
    config: String,
    clients: Vec<Arc<Client>>,
    /// Which turn took the set last; an idle timer only ends the set its
    /// own turn left.
    generation: u64,
}

static WARM: LazyLock<Mutex<HashMap<String, Warm>>> = LazyLock::new(Mutex::default);
static GENERATION: AtomicU64 = AtomicU64::new(1);

tokio::task_local! {
    /// The thread whose servers [`Servers::connect`] keeps warm.
    static THREAD: String;
}

/// Runs `future`, which builds a turn's runtime, with the MCP servers it
/// connects kept warm for `thread`'s next turn.
pub async fn for_thread<F: Future>(thread: &str, future: F) -> F::Output {
    THREAD.scope(thread.to_owned(), future).await
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Servers {
    /// Connects to every server at once. A server that fails to start is
    /// recorded and left out; the run carries on without it. Inside
    /// [`for_thread`], the thread's servers from its last turn are reused
    /// when their config is unchanged.
    pub async fn connect(servers: &[McpServer], cwd: &Path) -> Self {
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
        let config = fingerprint(&servers, cwd);
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
        let started =
            futures::future::join_all(
                servers
                    .iter()
                    .filter(|server| !warm.contains_key(&server.name))
                    .map(|server| async move {
                        (server.name.clone(), connect_client(server, cwd).await)
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
            let replaced = lock(&WARM).insert(
                thread.clone(),
                Warm {
                    config,
                    clients: connected.clients.clone(),
                    generation,
                },
            );
            drop(replaced);
            connected.warm = Some((thread, generation));
        }
        connected
    }

    /// Names every tool the model is offered, up to [`MAX_TOOLS`] in the
    /// servers' order.
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

    /// Whether any server is connected.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// Each connected server, with how many tools it offers.
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

    /// The servers that could not start, with a short reason fit for the
    /// model and the user: never their output or a response body.
    #[must_use]
    pub fn failures(&self) -> &[(String, String)] {
        &self.failures
    }

    /// Every tool, in the shape claude.ai takes.
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

    /// The client and tool a model-facing name names.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<(&Client, &Tool)> {
        let tool = &self.tools[*self.names.get(name)?];
        Some((self.clients[tool.client].as_ref(), tool))
    }

    /// The server (as configured) and the tool's own name behind a
    /// model-facing name, for showing the call.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<(&str, &str)> {
        self.find(name)
            .map(|(_, tool)| (tool.server.as_str(), tool.raw.as_str()))
    }

    /// The images calls returned since the last take, for the message that
    /// carries their results.
    pub fn take_images(&self) -> Vec<Image> {
        std::mem::take(&mut *lock(&self.images))
    }

    /// Calls a tool and returns what the model is told. `Err` is the
    /// server's or the transport's error text. Images the tool returned
    /// wait in [`Self::take_images`]. Dropping the future cancels the call
    /// on the server.
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
                tracing::warn!(server = %client.server, tool = %tool.raw, "an MCP call failed: {error:#}");
                format!("The {} MCP server failed: {error:#}", client.server)
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
        // The servers stay up for the thread's next turn, until they idle
        // out; a turn that takes them meanwhile moves the generation on.
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

/// Takes a thread's warm servers if no turn took them since `generation`.
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

/// What a set of servers was started from: two turns with the same
/// fingerprint can share them.
fn fingerprint(servers: &[&McpServer], cwd: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(cwd.to_string_lossy().as_bytes());
    for server in servers {
        hash.update([0]);
        hash.update(serde_json::to_vec(server).unwrap_or_default());
    }
    hex::encode(hash.finalize())
}

/// Starts one server and lists its tools. `Err` is the short reason the
/// model and the user are told; the log has the detail.
async fn connect_client(server: &McpServer, cwd: &Path) -> Result<Client, String> {
    let resolved = resolve(server, &|name| std::env::var(name).ok()).inspect_err(|reason| {
        tracing::warn!(server = %server.name, step = "reading its config", "an MCP server could not start: {reason}");
    })?;
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let transport = Transport::open(&resolved, cwd, deadline)
        .await
        .map_err(|error| {
            tracing::warn!(
                server = %server.name,
                step = "starting it",
                "an MCP server could not start: {}",
                redact(&format!("{error:#}"), &resolved.secrets)
            );
            format!("failed to start: {}", error.root_cause())
        })?;
    let listed = async {
        transport.initialize(deadline).await.context("initialize")?;
        list_tools(&transport, &server.name, deadline)
            .await
            .context("listing its tools")
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
                redact(&format!("{error:#}"), &resolved.secrets)
            );
            Err(exited.map_or_else(
                || format!("{error} failed: {}", short_reason(&error)),
                |code| format!("failed to start: {code}"),
            ))
        }
    }
}

/// The cause of a failed step, short and free of what the server said.
fn short_reason(error: &anyhow::Error) -> String {
    let cause = error.chain().nth(1).unwrap_or_else(|| error.as_ref());
    cause.downcast_ref::<RpcError>().map_or_else(
        || cause.to_string(),
        |refused| format!("refused (JSON-RPC error {})", refused.code),
    )
}

/// The tools a server lists.
async fn list_tools(
    transport: &Transport,
    server: &str,
    deadline: Instant,
) -> anyhow::Result<Vec<Listed>> {
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

/// The plain name the model would see for a server's tool, before it is
/// made short and unique.
#[must_use]
pub fn prefixed_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", component(server), component(tool))
}

/// A name part inside the tool-name grammar.
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

/// The names the model sees, one per `(server, tool)`, each matching
/// `^[a-zA-Z0-9_-]{1,64}$` and unique across every server. A plain name
/// that is too long, or that two tools would share (`a.b` and `a_b` both
/// read `a_b`), is cut short and ends in a hash of its server and tool, so
/// the same servers always get the same names.
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

/// `name` cut to fit, then `_` and eight hex digits of its server and tool.
fn hashed_name(name: &str, server: &str, tool: &str, salt: u32) -> String {
    let mut hash = Sha256::new();
    hash.update(server.as_bytes());
    hash.update([0]);
    hash.update(tool.as_bytes());
    hash.update(salt.to_le_bytes());
    let digest = hex::encode(&hash.finalize()[..4]);
    // The name is ASCII: every other character became `_`.
    let keep = name.len().min(MAX_NAME - digest.len() - 1);
    format!("{}_{digest}", &name[..keep])
}

/// A tool result for the model: its text, and the images that go with it.
struct Rendered {
    text: String,
    images: Vec<Image>,
}

/// The text of a tool result's content blocks; images are taken out to be
/// attached, text resources are embedded and links named.
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

/// Takes an image out of a result for attaching, and says where it went.
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

/// An embedded resource: its text, or what it is when it is binary.
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

/// A link to a resource the model may ask for by name.
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

/// A size for people: `64 KB`, `32 MB`.
fn size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{} MB", bytes / (1024 * 1024))
    } else {
        format!("{} KB", bytes / 1024)
    }
}

/// A server's config with `${VAR}` and `${VAR:-default}` expanded, as
/// Claude Code expands `.mcp.json`.
struct Resolved {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    url: Option<String>,
    headers: BTreeMap<String, String>,
    sse: bool,
    /// What logs never show: env and header values, and whatever the
    /// expansion put in.
    secrets: Vec<String>,
}

/// Expands a server's config. `Err` names the variables that are not set.
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
    // Longest first, so a secret that contains another is hidden whole.
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
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find('}').filter(|&end| end > 0) else {
                out.push_str(&rest[start..start + 2]);
                rest = after;
                continue;
            };
            let inner = &after[..end];
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
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        out
    }
}

/// `text` with every secret replaced.
fn redact(text: &str, secrets: &[String]) -> String {
    secrets.iter().fold(text.to_owned(), |text, secret| {
        text.replace(secret, "[redacted]")
    })
}

/// A JSON-RPC error a server answered with.
#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// The `result` of a JSON-RPC reply, or its error.
fn response_result(reply: &Value) -> anyhow::Result<Value> {
    if let Some(error) = reply.get("error") {
        return Err(RpcError {
            code: error["code"].as_i64().unwrap_or_default(),
            message: error["message"]
                .as_str()
                .unwrap_or("the server failed")
                .to_owned(),
        }
        .into());
    }
    Ok(reply["result"].clone())
}

/// What the client answers a request from the server: an empty result to
/// `ping`, "not supported" to everything else (sampling, roots,
/// elicitation). `None` for notifications and replies.
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

/// The messages in one JSON-RPC payload: itself, or a batch's members.
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

enum Transport {
    /// A stdio server, or a legacy HTTP+SSE one: messages both ways.
    Stream(Stream),
    /// A Streamable HTTP server: one POST per message.
    Http(Arc<Http>),
}

impl Transport {
    async fn open(server: &Resolved, cwd: &Path, deadline: Instant) -> anyhow::Result<Self> {
        match &server.url {
            Some(url) if server.sse => Ok(Self::Stream(
                Stream::sse(url, header_map(&server.headers)?, deadline).await?,
            )),
            Some(url) => Ok(Self::Http(Arc::new(Http::new(
                url,
                header_map(&server.headers)?,
            )?))),
            None => Ok(Self::Stream(Stream::stdio(server, cwd, MAX_MESSAGE_BYTES)?)),
        }
    }

    /// The MCP handshake, then the server is ready.
    async fn initialize(&self, deadline: Instant) -> anyhow::Result<()> {
        self.request("initialize", initialize_params(), deadline)
            .await?;
        self.notify("notifications/initialized", json!({})).await
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> anyhow::Result<Value> {
        match self {
            Self::Stream(stream) => stream.request(method, params, deadline).await,
            Self::Http(http) => http.request(method, params, deadline).await,
        }
    }

    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        match self {
            Self::Stream(stream) => stream.notify(method, &params),
            Self::Http(http) => http.notify(method, params).await,
        }
    }

    /// Whether the server may still answer; a dead one starts again next
    /// turn.
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

    /// How a stdio server that stopped answering ended, once it has.
    async fn exit_status(&self) -> Option<String> {
        let Self::Stream(Stream {
            child: Some(child), ..
        }) = self
        else {
            return None;
        };
        // Its output closes a moment before it is reaped.
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

/// The requests waiting on a message stream's server.
struct Rpc {
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
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

    fn register(&self) -> (u64, oneshot::Receiver<Result<Value, String>>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        lock(&self.pending).insert(id, sender);
        (id, receiver)
    }

    fn forget(&self, id: u64) {
        lock(&self.pending).remove(&id);
    }

    /// Fails one request: its reply cannot come.
    fn fail(&self, id: u64, reason: String) {
        let waiting = lock(&self.pending).remove(&id);
        if let Some(waiting) = waiting {
            waiting.send(Err(reason)).ok();
        }
    }

    /// The server is gone: every waiting request fails with `reason`.
    fn close(&self, reason: &str) {
        self.alive.store(false, Ordering::SeqCst);
        let pending = std::mem::take(&mut *lock(&self.pending));
        for waiting in pending.into_values() {
            waiting.send(Err(reason.to_owned())).ok();
        }
    }

    /// Takes in one message from the server and returns what to answer.
    /// A message with a `method` is the server's own request or
    /// notification, never a reply to ours, whatever its id.
    fn receive(&self, message: Value) -> Option<Value> {
        if message.get("method").is_some() {
            return answer_to(&message);
        }
        let id = message.get("id").and_then(Value::as_u64)?;
        let waiting = lock(&self.pending).remove(&id);
        if let Some(waiting) = waiting {
            waiting.send(Ok(message)).ok();
        }
        None
    }
}

/// A server that speaks JSON-RPC as a stream of messages both ways: a stdio
/// server's pipes, or a legacy HTTP+SSE server's event stream and endpoint.
struct Stream {
    rpc: Arc<Rpc>,
    /// Messages to the server, written in order by the writer task.
    outgoing: mpsc::UnboundedSender<Value>,
    /// Held so a stdio server is killed when the client goes.
    child: Option<Mutex<crate::process::OwnedChild>>,
    stderr: Arc<Mutex<String>>,
    /// The reader and writer (and stderr) tasks, ended with the client.
    _tasks: Vec<AbortOnDropHandle<()>>,
}

impl Stream {
    fn stdio(server: &Resolved, cwd: &Path, max_line: usize) -> anyhow::Result<Self> {
        let mut command = tokio::process::Command::new(&server.command);
        command
            .args(&server.args)
            .envs(server.env.iter())
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = crate::process::owned_command(command)?;
        let stdin = child.stdin().take().context("the server has no stdin")?;
        let stdout = child.stdout().take().context("the server has no stdout")?;
        let stderr = Arc::new(Mutex::new(String::new()));
        let rpc = Arc::new(Rpc::new());
        let (outgoing, messages) = mpsc::unbounded_channel();
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

    /// Opens a legacy HTTP+SSE server: its event stream names the endpoint
    /// messages are POSTed to, and carries the replies.
    async fn sse(url: &str, headers: HeaderMap, deadline: Instant) -> anyhow::Result<Self> {
        let client = http_client()?;
        let base = reqwest::Url::parse(url).context("the url is not valid")?;
        let response = tokio::time::timeout_at(
            deadline,
            client
                .get(base.clone())
                .headers(headers.clone())
                .header("accept", "text/event-stream")
                .send(),
        )
        .await
        .map_err(|_elapsed| anyhow!("the server did not open its event stream in time"))?
        .map_err(|error| anyhow!(reach_error(&error)))?;
        if !response.status().is_success() {
            bail!("HTTP {}", response.status().as_u16());
        }
        let mut events = sse_events(response);
        let endpoint = loop {
            let event = tokio::time::timeout_at(deadline, events.next())
                .await
                .map_err(|_elapsed| anyhow!("the server did not name its endpoint in time"))?;
            match event {
                Some(Ok(SseEvent::Message(message))) if message.event == "endpoint" => {
                    break base
                        .join(message.data.trim())
                        .context("the server's endpoint is not a valid url")?;
                }
                Some(Ok(SseEvent::Message(_) | SseEvent::Retry(_))) => {}
                Some(Err(_)) | None => {
                    bail!("the event stream ended before the server named its endpoint")
                }
            }
        };
        if endpoint.origin() != base.origin() {
            bail!("the server's endpoint is on another origin");
        }
        let rpc = Arc::new(Rpc::new());
        let (outgoing, messages) = mpsc::unbounded_channel();
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

    /// Sends a request and waits for its reply until `deadline`; messages
    /// the server sends meanwhile (progress, pings) do not move it.
    async fn request(
        &self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> anyhow::Result<Value> {
        let limit = deadline.saturating_duration_since(Instant::now());
        let (id, reply) = self.rpc.register();
        let mut pending = Pending {
            stream: self,
            id,
            cancel: method != "initialize",
        };
        if !self.rpc.alive.load(Ordering::SeqCst)
            || self
                .outgoing
                .send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
                .is_err()
        {
            pending.cancel = false;
            bail!("the server is no longer running");
        }
        let Ok(answered) = tokio::time::timeout_at(deadline, reply).await else {
            bail!(
                "the server did not answer {method} within {}s",
                limit.as_secs_f32().round()
            );
        };
        pending.cancel = false;
        let reply = answered
            .unwrap_or_else(|_closed| Err("the server closed its output".to_owned()))
            .map_err(|reason| anyhow!(reason))?;
        response_result(&reply)
    }

    fn notify(&self, method: &str, params: &Value) -> anyhow::Result<()> {
        self.outgoing
            .send(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .map_err(|_closed| anyhow!("the server is no longer running"))
    }
}

/// A request in flight. Dropped before its reply came (timed out, or the
/// turn was stopped), it tells the server to stop working on it.
struct Pending<'a> {
    stream: &'a Stream,
    id: u64,
    cancel: bool,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.stream.rpc.forget(self.id);
        if self.cancel && self.stream.rpc.alive.load(Ordering::SeqCst) {
            self.stream.outgoing.send(cancelled(self.id)).ok();
        }
    }
}

/// Writes messages to a stdio server, one line each.
async fn write_lines(
    mut stdin: tokio::process::ChildStdin,
    mut messages: mpsc::UnboundedReceiver<Value>,
    rpc: Arc<Rpc>,
) {
    while let Some(message) = messages.recv().await {
        let mut line = message.to_string();
        line.push('\n');
        let written = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        }
        .await;
        if let Err(error) = written {
            tracing::debug!("writing to an MCP server: {error}");
            rpc.close("the server closed its input");
            break;
        }
    }
}

/// Reads a stdio server's output, one message per line. A line over
/// `max_line` fails the request it answers and is skipped; the next line
/// reads as usual.
async fn read_lines(
    stdout: impl AsyncRead + Unpin,
    rpc: Arc<Rpc>,
    outgoing: mpsc::UnboundedSender<Value>,
    max_line: usize,
) {
    let mut lines = Lines::new(stdout, max_line);
    loop {
        match lines.next().await {
            Ok(Some(Line::Message(line))) => {
                // Anything else on stdout (a stray log line) is not ours.
                let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                    continue;
                };
                for message in batch(value) {
                    if let Some(answer) = rpc.receive(message) {
                        outgoing.send(answer).ok();
                    }
                }
            }
            Ok(Some(Line::Oversized(id))) => {
                tracing::warn!(
                    id,
                    limit = max_line,
                    "an MCP server wrote a message over the limit; it was skipped"
                );
                if let Some(id) = id {
                    rpc.fail(
                        id,
                        format!("the server's answer was larger than {}", size(max_line)),
                    );
                }
            }
            Ok(None) => {
                rpc.close("the server closed its output");
                break;
            }
            Err(error) => {
                tracing::debug!("reading an MCP server: {error}");
                rpc.close("the server's output failed");
                break;
            }
        }
    }
}

/// Keeps the last [`MAX_STDERR`] bytes a stdio server wrote to stderr.
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

/// One line of a server's output.
enum Line {
    Message(Vec<u8>),
    /// A line over the limit, skipped; the request id it answers, when it
    /// names one at its top level.
    Oversized(Option<u64>),
}

/// Reads lines without ever holding more than the limit of one.
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

    /// The next line, or `None` at the end of the output.
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

/// Finds the top-level numeric `"id"` of a JSON object read in pieces,
/// without keeping it.
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a byte-at-a-time scanner's state, read together in one match"
)]
struct IdScan {
    depth: u32,
    in_string: bool,
    escaped: bool,
    /// At the top level, the next string is a key.
    expect_key: bool,
    reading_key: bool,
    key: Vec<u8>,
    /// The last key read was `id`, and its value comes next.
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

/// The events of an SSE body, too large ones reported and skipped.
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

/// Reads a legacy SSE server's event stream: its replies and requests.
async fn read_events(mut events: Events, rpc: Arc<Rpc>, outgoing: mpsc::UnboundedSender<Value>) {
    while let Some(event) = events.next().await {
        match event {
            Ok(SseEvent::Message(message)) if message.event == "message" => {
                let Ok(value) = serde_json::from_str::<Value>(&message.data) else {
                    continue;
                };
                for message in batch(value) {
                    if let Some(answer) = rpc.receive(message) {
                        outgoing.send(answer).ok();
                    }
                }
            }
            Ok(SseEvent::Message(_) | SseEvent::Retry(_)) => {}
            Err(EventError::TooLarge) => {
                tracing::warn!("an MCP server sent an event over the limit; it was skipped");
            }
            Err(EventError::Broken) => break,
        }
    }
    rpc.close("the server closed its event stream");
}

/// POSTs messages to a legacy SSE server's endpoint, in order.
async fn post_messages(
    client: reqwest::Client,
    endpoint: reqwest::Url,
    headers: HeaderMap,
    mut messages: mpsc::UnboundedReceiver<Value>,
    rpc: Arc<Rpc>,
) {
    while let Some(message) = messages.recv().await {
        // Our own requests carry a method and an id; a refused one fails.
        let id = message
            .get("method")
            .and_then(|_| message.get("id"))
            .and_then(Value::as_u64);
        let sent = client
            .post(endpoint.clone())
            .headers(headers.clone())
            .header("content-type", "application/json")
            .body(message.to_string())
            .send()
            .await;
        let failure = match sent {
            Ok(response) if response.status().is_success() => None,
            Ok(response) => Some(format!("HTTP {}", response.status().as_u16())),
            Err(error) => {
                let reason = reach_error(&error).to_owned();
                tracing::debug!("posting to an MCP server: {}", error.without_url());
                Some(reason)
            }
        };
        if let (Some(reason), Some(id)) = (failure, id) {
            rpc.fail(id, reason);
        }
    }
}

/// The headers a config names, marked sensitive.
fn header_map(headers: &BTreeMap<String, String>) -> anyhow::Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let header = HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|_invalid| anyhow!("the header name {name} is not valid"))?;
        let mut value = HeaderValue::from_str(value.trim())
            .map_err(|_invalid| anyhow!("the {name} header's value is not valid"))?;
        value.set_sensitive(true);
        map.insert(header, value);
    }
    Ok(map)
}

fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(format!("artisan/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("building the MCP client")
}

/// Why a request did not reach the server, without its url.
fn reach_error(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "the request timed out"
    } else if error.is_connect() {
        "could not connect to the server"
    } else {
        "the request to the server failed"
    }
}

/// The server no longer knows the session the request named.
#[derive(Debug)]
struct SessionGone;

impl std::fmt::Display for SessionGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the server ended the session")
    }
}

impl std::error::Error for SessionGone {}

/// A remote MCP server: JSON-RPC over Streamable HTTP.
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
    fn new(url: &str, headers: HeaderMap) -> anyhow::Result<Self> {
        reqwest::Url::parse(url).context("the url is not valid")?;
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

    /// Sends a request and waits for its reply until `deadline`.
    async fn request(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> anyhow::Result<Value> {
        let limit = deadline.saturating_duration_since(Instant::now());
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut pending = HttpPending {
            http: Arc::clone(self),
            id,
            cancel: method != "initialize",
        };
        let Ok(reply) = tokio::time::timeout_at(deadline, self.exchange(id, method, &params)).await
        else {
            bail!(
                "the server did not answer {method} within {}s",
                limit.as_secs_f32().round()
            );
        };
        pending.cancel = false;
        response_result(&reply?)
    }

    /// One request, with a new session and a second try when the server
    /// forgot the one it gave.
    async fn exchange(&self, id: u64, method: &str, params: &Value) -> anyhow::Result<Value> {
        match self.round_trip(id, method, params).await {
            Err(error) if error.is::<SessionGone>() && method != "initialize" => {
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
                    .context("starting a new session")?;
                response_result(&started).context("starting a new session")?;
                self.notify("notifications/initialized", json!({}))
                    .await
                    .context("starting a new session")?;
                self.round_trip(id, method, params).await
            }
            other => other,
        }
    }

    /// POSTs a request and reads its reply: a JSON body, or the first
    /// message with its id on an event stream.
    async fn round_trip(&self, id: u64, method: &str, params: &Value) -> anyhow::Result<Value> {
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let (response, named_session) = self.post(&message).await?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND && named_session {
            return Err(SessionGone.into());
        }
        if !status.is_success() {
            bail!("HTTP {}", status.as_u16());
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
            let body = read_body(response, MAX_MESSAGE_BYTES).await?;
            let value: Value = serde_json::from_slice(&body)
                .map_err(|_invalid| anyhow!("the server's answer to {method} is not JSON"))?;
            batch(value)
                .into_iter()
                .find(|reply| reply.get("method").is_none() && reply["id"].as_u64() == Some(id))
                .with_context(|| format!("the server answered nothing for {method}"))?
        };
        if method == "initialize"
            && let Some(version) = reply["result"]["protocolVersion"].as_str()
        {
            *lock(&self.protocol) = Some(version.to_owned());
        }
        Ok(reply)
    }

    /// The reply with `id` on an event stream, as soon as it comes: the
    /// server may keep the stream open after it.
    async fn reply_from_events(
        &self,
        mut events: Events,
        id: u64,
        method: &str,
    ) -> anyhow::Result<Value> {
        while let Some(event) = events.next().await {
            match event {
                Ok(SseEvent::Message(message)) => {
                    let Ok(value) = serde_json::from_str::<Value>(&message.data) else {
                        continue;
                    };
                    for message in batch(value) {
                        if let Some(answer) = answer_to(&message) {
                            if let Err(error) = self.post(&answer).await {
                                tracing::debug!("answering an MCP server's request: {error:#}");
                            }
                            continue;
                        }
                        if message.get("method").is_none() && message["id"].as_u64() == Some(id) {
                            return Ok(message);
                        }
                    }
                }
                Ok(SseEvent::Retry(_)) => {}
                Err(EventError::TooLarge) => bail!(
                    "the server's answer to {method} was larger than {}",
                    size(MAX_MESSAGE_BYTES)
                ),
                Err(EventError::Broken) => bail!("the server's answer to {method} broke off"),
            }
        }
        bail!("the server closed the stream without answering {method}")
    }

    /// POSTs one message; also says whether it named a session.
    async fn post(&self, message: &Value) -> anyhow::Result<(reqwest::Response, bool)> {
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
            let reason = reach_error(&error).to_owned();
            tracing::debug!("reaching an MCP server: {}", error.without_url());
            anyhow!(reason)
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

    async fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let (response, _) = self.post(&message).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            bail!("HTTP {}", response.status().as_u16())
        }
    }
}

/// An HTTP request in flight; dropped before its reply, it tells the
/// server to stop working on it.
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
                tracing::debug!("cancelling an MCP call: {error:#}");
            }
        });
    }
}

/// Reads a whole HTTP body of at most `limit` bytes. A body that breaks off
/// or runs over is an error, never a shortened answer.
async fn read_body(response: reqwest::Response, limit: usize) -> anyhow::Result<Vec<u8>> {
    let too_large = || anyhow!("the server's answer was larger than {}", size(limit));
    if response
        .content_length()
        .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
    {
        return Err(too_large());
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|error| {
            tracing::debug!("reading an MCP server's answer: {}", error.without_url());
            anyhow!("the server's answer broke off")
        })?;
        if body.len() + chunk.len() > limit {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The `.mcp.json` servers of the project, as Claude Code reads them: from
/// the repository root down to the working directory, with a nearer file's
/// server of the same name winning. `${VAR}` references stay as written
/// until the server starts.
#[must_use]
pub fn project_servers(cwd: &Path) -> Vec<McpServer> {
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

/// The string values of a JSON object.
fn strings(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
        .collect()
}

/// The servers one `.mcp.json` file holds.
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

/// Reads one HTTP request off a test socket: its head and body.
#[cfg(test)]
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => read,
        };
        data.extend_from_slice(&buffer[..read]);
        let text = String::from_utf8_lossy(&data);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::trim)
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                return Some((
                    text[..end].to_owned(),
                    text[end + 4..end + 4 + length].to_owned(),
                ));
            }
        }
    }
}

/// What the test servers answer: one `search` tool that finds it.
#[cfg(test)]
fn fake_reply(value: &Value) -> Value {
    let result = match value["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "serverInfo": { "name": "fake", "version": "1" }
        }),
        Some("tools/list") => json!({
            "tools": [{
                "name": "search",
                "description": "Find things",
                "inputSchema": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } }
                }
            }]
        }),
        Some("tools/call") => json!({
            "content": [{ "type": "text", "text": "found it" }],
            "isError": false
        }),
        _ => json!({}),
    };
    json!({ "jsonrpc": "2.0", "id": value["id"], "result": result })
}

/// A tiny MCP-over-HTTP server for tests: `initialize`, `tools/list` and
/// `tools/call`, one JSON-RPC request per connection. Runs until its task
/// is aborted.
#[cfg(test)]
pub async fn fake_server(listener: tokio::net::TcpListener) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            break;
        };
        let Some((_, body)) = read_request(&mut socket).await else {
            break;
        };
        let value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        if value.get("id").is_none() {
            socket
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await
                .ok();
            continue;
        }
        let body = fake_reply(&value).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.ok();
    }
}

/// A stdio MCP server for tests: this test binary, running
/// `tests::fake_stdio_server` with `env` (see there for what it does).
#[cfg(test)]
pub fn fake_stdio(name: &str, env: &[(&str, &str)]) -> McpServer {
    let mut vars: BTreeMap<String, String> = env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    vars.insert("ARTISAN_FAKE_MCP".to_owned(), "1".to_owned());
    McpServer {
        name: name.to_owned(),
        command: std::env::current_exe()
            .expect("the test binary")
            .to_string_lossy()
            .into_owned(),
        args: [
            "--exact",
            "claudeai::mcp::tests::fake_stdio_server",
            "--nocapture",
            "--test-threads=1",
            "--quiet",
        ]
        .map(str::to_owned)
        .to_vec(),
        env: vars,
        ..McpServer::default()
    }
}

#[cfg(test)]
mod tests;
