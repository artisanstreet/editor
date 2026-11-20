use crate::core::client::CallError;
use crate::core::{
    ForgeClient,
    delivery::{AdmissionOrder, Delivery},
    host::HostInstance,
    hosts::HostId,
    runtime::runtime,
};
use artisan_account::browser;
use artisan_api::{McpSignInFinish, McpSignInStart};
use std::{collections::HashMap, num::NonZeroUsize, time::Duration};
use tokio::io::AsyncWriteExt as _;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};

const PREFERRED_PORT: u16 = 33418;
const WAITS: Duration = Duration::from_mins(10);

#[derive(Debug, thiserror::Error)]
pub enum StartMcpSignInError {
    #[error("{host} is no longer connected, so {server} cannot sign in")]
    Disconnected { host: HostId, server: String },
    #[error("The sign-in for {server} is still starting; try again in a moment.")]
    StillStarting { host: HostId, server: String },
    #[error("{server} is finishing its sign-in already.")]
    Finishing { host: HostId, server: String },
    #[error("listen for the browser's redirect for {server}")]
    Listen {
        server: String,
        #[source]
        source: std::io::Error,
    },
    #[error("start the sign-in for {server} on {host}")]
    Start {
        host: HostId,
        server: String,
        #[source]
        source: CallError,
    },
    #[error("The sign-in for {server} was cancelled.")]
    Cancelled { host: HostId, server: String },
}

async fn bind() -> Result<(TcpListener, String), std::io::Error> {
    let listener = match TcpListener::bind(("127.0.0.1", PREFERRED_PORT)).await {
        Ok(listener) => listener,
        Err(taken) => {
            tracing::debug!("port {PREFERRED_PORT} is taken ({taken}); using another");
            TcpListener::bind(("127.0.0.1", 0)).await?
        }
    };
    let port = listener.local_addr()?.port();
    Ok((listener, format!("http://127.0.0.1:{port}/callback")))
}

#[derive(Debug, thiserror::Error)]
pub enum CallbackError {
    #[error("The browser didn't come back within 10 minutes; start the sign-in again.")]
    TimedOut,
    #[error("The sign-in was refused ({}).", .code.code())]
    Denied { code: Refusal },
    #[error("accepting the browser's redirect")]
    Accept(#[source] std::io::Error),
}

pub async fn wait_for_code(
    listener: TcpListener,
    state: String,
    cancel: CancellationToken,
) -> Result<Option<(String, TcpStream)>, CallbackError> {
    let deadline = tokio::time::sleep(WAITS);
    tokio::pin!(deadline);
    loop {
        let mut stream = tokio::select! {
            () = cancel.cancelled() => return Ok(None),
            () = &mut deadline => return Err(CallbackError::TimedOut),
            accepted = listener.accept() => accepted.map_err(CallbackError::Accept)?.0,
        };
        let target = match browser::read_get_target(&mut stream).await {
            Ok(target) => target,
            Err(browser::RequestError::NotGet) => {
                respond(stream, Page::not_found()).await;
                continue;
            }
            Err(
                unreadable @ (browser::RequestError::Read(_)
                | browser::RequestError::TooLarge { .. }
                | browser::RequestError::TimedOut),
            ) => {
                tracing::debug!("ignoring an unreadable browser request: {unreadable}");
                continue;
            }
        };
        match callback(&target, &state) {
            Callback::Code(code) => return Ok(Some((code, stream))),
            Callback::Denied(code) => {
                let denied = CallbackError::Denied { code };
                respond(stream, Page::failed("the server", &denied.to_string())).await;
                return Err(denied);
            }
            Callback::Other => respond(stream, Page::not_found()).await,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    InvalidRequest,
    UnauthorizedClient,
    AccessDenied,
    UnsupportedResponseType,
    InvalidScope,
    ServerError,
    TemporarilyUnavailable,
    Unrecognized,
}

impl Refusal {
    fn of(code: &str) -> Self {
        match code {
            "invalid_request" => Self::InvalidRequest,
            "unauthorized_client" => Self::UnauthorizedClient,
            "access_denied" => Self::AccessDenied,
            "unsupported_response_type" => Self::UnsupportedResponseType,
            "invalid_scope" => Self::InvalidScope,
            "server_error" => Self::ServerError,
            "temporarily_unavailable" => Self::TemporarilyUnavailable,
            _ => Self::Unrecognized,
        }
    }

    const fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnauthorizedClient => "unauthorized_client",
            Self::AccessDenied => "access_denied",
            Self::UnsupportedResponseType => "unsupported_response_type",
            Self::InvalidScope => "invalid_scope",
            Self::ServerError => "server_error",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::Unrecognized => "an OAuth error code Artisan doesn't know",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Code(String),
    Denied(Refusal),
    Other,
}

fn callback(target: &str, state: &str) -> Callback {
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Other;
    };
    if url.path() != "/callback" {
        return Callback::Other;
    }
    let pairs: HashMap<String, String> = url.query_pairs().into_owned().collect();
    if pairs.get("state").map(String::as_str) != Some(state) {
        return Callback::Other;
    }
    if let Some(error) = pairs.get("error") {
        return Callback::Denied(Refusal::of(error));
    }
    pairs
        .get("code")
        .filter(|code| !code.is_empty())
        .map_or(Callback::Other, |code| Callback::Code(code.clone()))
}

struct Page {
    status: &'static str,
    title: String,
    body: String,
}

impl Page {
    fn signed_in(server: &str) -> Self {
        Self {
            status: "200 OK",
            title: format!("Signed in to {server}"),
            body:
                "You can close this tab and go back to Artisan. New threads get the server's tools."
                    .to_owned(),
        }
    }

    fn failed(server: &str, reason: &str) -> Self {
        Self {
            status: "200 OK",
            title: format!("Couldn't sign in to {server}"),
            body: format!(
                "{reason} You can start the sign-in again from Settings → Extensions in Artisan."
            ),
        }
    }

    fn not_found() -> Self {
        Self {
            status: "404 Not Found",
            title: "Nothing here".to_owned(),
            body: "This address only finishes Artisan's sign-ins.".to_owned(),
        }
    }

    fn html(&self) -> String {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
             <style>body{{font:16px system-ui,sans-serif;max-width:32rem;margin:15vh auto;padding:0 1.5rem;\
             color:#1b1b1b;background:#fafafa}}@media(prefers-color-scheme:dark){{body{{color:#ececec;background:#141414}}}}\
             h1{{font-size:1.4rem;font-weight:600}}p{{opacity:.75;line-height:1.5}}</style></head>\
             <body><h1>{title}</h1><p>{body}</p></body></html>",
            title = escape(&self.title),
            body = escape(&self.body),
        )
    }
}

async fn respond(mut stream: TcpStream, page: Page) {
    let html = page.html();
    let response = format!(
        "HTTP/1.1 {}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n{html}",
        page.status,
        html.len(),
    );
    if let Err(error) = stream.write_all(response.as_bytes()).await {
        tracing::debug!("answering the browser: {error}");
    }
    if let Err(error) = stream.shutdown().await {
        tracing::debug!("closing the browser's connection: {error}");
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpSignInPhase {
    Starting,
    Waiting,
    Finishing,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct McpSignIn {
    phase: McpSignInPhase,
    authorize_url: Option<String>,
    id: u64,
    instance: HostInstance,
    state: Option<String>,
    cancel: CancellationToken,
}

impl McpSignIn {
    pub const fn phase(&self) -> &McpSignInPhase {
        &self.phase
    }
    pub fn authorize_url(&self) -> Option<&str> {
        self.authorize_url.as_deref()
    }
}

type Answer = tokio::sync::oneshot::Sender<Result<String, StartMcpSignInError>>;

fn reply(answer: Answer, key: &(HostId, String), outcome: Result<String, StartMcpSignInError>) {
    if answer.send(outcome).is_err() {
        tracing::debug!(host = %key.0, server = %key.1, "the MCP sign-in's requester stopped waiting for its answer");
    }
}

async fn cancel_started(client: &ForgeClient, key: &(HostId, String), state: &str) {
    if let Err(error) = client.mcp_sign_in_cancel(state).await {
        tracing::debug!(host = %key.0, server = %key.1, error = %::diagnostics::chain(&error), "cancelling an MCP sign-in");
    }
}

enum Message {
    Started {
        key: (HostId, String),
        id: u64,
        url: String,
        state: String,
        open: bool,
        answer: tokio::sync::oneshot::Sender<bool>,
    },
    Phase {
        key: (HostId, String),
        id: u64,
        phase: McpSignInPhase,
    },
    Finished {
        key: (HostId, String),
        id: u64,
    },
    Cleanup {
        key: (HostId, String),
        id: u64,
    },
}

pub(crate) enum McpEvent {
    Changed,
    OpenUrl(String),
}

pub(crate) struct McpSignIns {
    next_id: u64,
    by_server: HashMap<(HostId, String), McpSignIn>,
    jobs: HashMap<((HostId, String), u64), AbortOnDropHandle<()>>,
    delivery: Delivery<Message>,
    events: crate::core::store::Effects,
}

async fn begin(
    client: &ForgeClient,
    key: &(HostId, String),
) -> Result<(TcpListener, artisan_api::McpSignInStarted), StartMcpSignInError> {
    let (listener, redirect_uri) = bind().await.map_err(|source| StartMcpSignInError::Listen {
        server: key.1.clone(),
        source,
    })?;
    let started = client
        .mcp_sign_in_start(&McpSignInStart {
            server: key.1.clone(),
            redirect_uri,
        })
        .await
        .map_err(|source| StartMcpSignInError::Start {
            host: key.0.clone(),
            server: key.1.clone(),
            source,
        })?;
    Ok((listener, started))
}

async fn sign_in(
    task_key: (HostId, String),
    id: u64,
    client: ForgeClient,
    open: bool,
    cancel: CancellationToken,
    answer: Answer,
    sender: crate::core::delivery::DeliverySender<Message>,
) {
    let (listener, started) = match begin(&client, &task_key).await {
        Ok(value) => value,
        Err(error) => {
            sender
                .offer(Message::Phase {
                    key: task_key.clone(),
                    id,
                    phase: McpSignInPhase::Failed(::diagnostics::chain(&error)),
                })
                .await;
            reply(answer, &task_key, Err(error));
            sender.offer(Message::Cleanup { key: task_key, id }).await;
            return;
        }
    };
    let (accept, accepted) = tokio::sync::oneshot::channel();
    if sender
        .send(Message::Started {
            key: task_key.clone(),
            id,
            url: started.authorize_url.clone(),
            state: started.state.clone(),
            open,
            answer: accept,
        })
        .await
        .is_err()
        || !accepted.await.unwrap_or(false)
    {
        cancel_started(&client, &task_key, &started.state).await;
        let outcome = Err(StartMcpSignInError::Cancelled {
            host: task_key.0.clone(),
            server: task_key.1.clone(),
        });
        reply(answer, &task_key, outcome);
        sender.offer(Message::Finished { key: task_key, id }).await;
        return;
    }
    reply(answer, &task_key, Ok(started.authorize_url));
    let state = started.state;
    let (code, stream) = match wait_for_code(listener, state.clone(), cancel).await {
        Ok(Some(found)) => found,
        Ok(None) => {
            sender.offer(Message::Finished { key: task_key, id }).await;
            return;
        }
        Err(error) => {
            cancel_started(&client, &task_key, &state).await;
            sender
                .offer(Message::Phase {
                    key: task_key.clone(),
                    id,
                    phase: McpSignInPhase::Failed(error.to_string()),
                })
                .await;
            sender.offer(Message::Cleanup { key: task_key, id }).await;
            return;
        }
    };
    if sender
        .send(Message::Phase {
            key: task_key.clone(),
            id,
            phase: McpSignInPhase::Finishing,
        })
        .await
        .is_err()
    {
        return;
    }
    let finished = client
        .mcp_sign_in_finish(&McpSignInFinish { state, code })
        .await;
    let page = match &finished {
        Ok(_) => Page::signed_in(&task_key.1),
        Err(error) => Page::failed(&task_key.1, &::diagnostics::chain(&error)),
    };
    runtime().spawn(respond(stream, page));
    let message = match finished {
        Ok(_) => Message::Finished {
            key: task_key.clone(),
            id,
        },
        Err(error) => Message::Phase {
            key: task_key.clone(),
            id,
            phase: McpSignInPhase::Failed(::diagnostics::chain(&error)),
        },
    };
    sender.offer(message).await;
    sender.offer(Message::Cleanup { key: task_key, id }).await;
}

impl McpSignIns {
    pub(crate) fn new(
        wake: aqua::mailbox::Sender<()>,
        order: AdmissionOrder,
        effects: crate::core::store::Effects,
    ) -> Self {
        Self {
            next_id: 0,
            by_server: HashMap::new(),
            jobs: HashMap::new(),
            delivery: Delivery::with_order(
                NonZeroUsize::new(64)
                    .expect("invariant: the sign-in delivery capacity is non-zero"),
                wake,
                order,
            ),
            events: effects,
        }
    }
    pub(crate) fn get(
        &self,
        host: &HostId,
        instance: HostInstance,
        server: &str,
    ) -> Option<&McpSignIn> {
        self.by_server
            .get(&(host.clone(), server.to_owned()))
            .filter(|entry| entry.instance == instance)
    }
    pub(crate) fn end_host(&mut self, host: &HostId) {
        let ended = self.by_server.len();
        self.by_server.retain(|(owner, _), entry| {
            let kept = owner != host;
            if !kept {
                entry.cancel.cancel();
            }
            kept
        });
        self.jobs.retain(|((owner, _), _), _| owner != host);
        if self.by_server.len() != ended {
            self.events.mcp(McpEvent::Changed);
            self.delivery.wake();
        }
    }
    pub(crate) fn end_all(&mut self) {
        let hosts: Vec<HostId> = self
            .by_server
            .keys()
            .chain(self.jobs.keys().map(|(key, _)| key))
            .map(|(host, _)| host.clone())
            .collect();
        for host in hosts {
            self.end_host(&host);
        }
    }
    pub(crate) fn start(
        &mut self,
        host: HostId,
        instance: HostInstance,
        server: String,
        client: ForgeClient,
        open: bool,
    ) -> tokio::sync::oneshot::Receiver<Result<String, StartMcpSignInError>> {
        let key = (host, server);
        let (answer, answered) = tokio::sync::oneshot::channel();
        if self
            .by_server
            .get(&key)
            .is_some_and(|existing| existing.instance != instance)
        {
            self.end_host(&key.0);
        }
        if let Some(existing) = self.by_server.get(&key) {
            match (&existing.phase, existing.authorize_url.clone()) {
                (McpSignInPhase::Waiting, Some(url)) => {
                    if open {
                        self.events.mcp(McpEvent::OpenUrl(url.clone()));
                        self.delivery.wake();
                    }
                    reply(answer, &key, Ok(url));
                    return answered;
                }
                (McpSignInPhase::Starting | McpSignInPhase::Waiting, _) => {
                    let outcome = Err(StartMcpSignInError::StillStarting {
                        host: key.0.clone(),
                        server: key.1.clone(),
                    });
                    reply(answer, &key, outcome);
                    return answered;
                }
                (McpSignInPhase::Finishing, _) => {
                    let outcome = Err(StartMcpSignInError::Finishing {
                        host: key.0.clone(),
                        server: key.1.clone(),
                    });
                    reply(answer, &key, outcome);
                    return answered;
                }
                (McpSignInPhase::Failed(_), _) => {}
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        let cancel = CancellationToken::new();
        self.by_server.insert(
            key.clone(),
            McpSignIn {
                phase: McpSignInPhase::Starting,
                authorize_url: None,
                id,
                instance,
                state: None,
                cancel: cancel.clone(),
            },
        );
        self.events.mcp(McpEvent::Changed);
        self.delivery.wake();
        let sender = self.delivery.sender();
        let task_key = key.clone();
        let task = runtime().spawn(sign_in(task_key, id, client, open, cancel, answer, sender));
        self.jobs.insert((key, id), AbortOnDropHandle::new(task));
        answered
    }
    pub(crate) fn cancel(&mut self, host: &HostId, server: &str, client: ForgeClient) {
        let key = (host.clone(), server.to_owned());
        let Some(entry) = self.by_server.remove(&key) else {
            return;
        };
        entry.cancel.cancel();
        if let Some(state) = entry.state {
            runtime().spawn(async move {
                if let Err(error) = client.mcp_sign_in_cancel(&state).await {
                    tracing::debug!(error = %::diagnostics::chain(&error), "cancelling an MCP sign-in");
                }
            });
        }
        self.events.mcp(McpEvent::Changed);
        self.delivery.wake();
    }
    pub(crate) fn peek_order(&mut self) -> Option<u64> {
        self.delivery.peek_order()
    }
    pub(crate) fn admitted(&self) -> usize {
        self.delivery.admitted()
    }
    pub(crate) fn drain(&mut self) {
        if let Some(message) = self.delivery.drain_one() {
            match message {
                Message::Started {
                    key,
                    id,
                    url,
                    state,
                    open,
                    answer,
                } => {
                    let wanted = self.by_server.get_mut(&key).is_some_and(|entry| {
                        if entry.id != id {
                            return false;
                        }
                        entry.phase = McpSignInPhase::Waiting;
                        entry.authorize_url = Some(url.clone());
                        entry.state = Some(state);
                        true
                    });
                    if wanted {
                        if open {
                            self.events.mcp(McpEvent::OpenUrl(url));
                        }
                        self.events.mcp(McpEvent::Changed);
                    }
                    if answer.send(wanted).is_err() {
                        tracing::debug!(host = %key.0, server = %key.1, "the MCP sign-in task ended before its start was accepted");
                    }
                }
                Message::Phase { key, id, phase } => {
                    if let Some(entry) = self.by_server.get_mut(&key)
                        && entry.id == id
                    {
                        entry.phase = phase;
                        self.events.mcp(McpEvent::Changed);
                    }
                }
                Message::Cleanup { key, id } => {
                    self.jobs.remove(&(key, id));
                }
                Message::Finished { key, id } => {
                    if self.by_server.get(&key).is_some_and(|entry| entry.id == id) {
                        self.by_server.remove(&key);
                        self.events.mcp(McpEvent::Changed);
                    }
                    self.jobs.remove(&(key, id));
                }
            }
        }
    }
}
