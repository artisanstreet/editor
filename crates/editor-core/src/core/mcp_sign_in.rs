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
    #[error("The sign-in was refused: {reason}")]
    Denied { reason: String },
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
            Callback::Denied(reason) => {
                respond(stream, Page::failed("the server", &reason)).await;
                return Err(CallbackError::Denied { reason });
            }
            Callback::Other => respond(stream, Page::not_found()).await,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Code(String),
    Denied(String),
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
        return Callback::Denied(
            pairs
                .get("error_description")
                .filter(|description| !description.trim().is_empty())
                .unwrap_or(error)
                .clone(),
        );
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
    stream.shutdown().await.ok();
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
    pub phase: McpSignInPhase,
    pub authorize_url: Option<String>,
    id: u64,
    instance: HostInstance,
    state: Option<String>,
    cancel: CancellationToken,
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

impl McpSignIns {
    pub fn new(
        wake: aqua::mailbox::Sender<()>,
        order: AdmissionOrder,
        effects: crate::core::store::Effects,
    ) -> Self {
        Self {
            next_id: 0,
            by_server: HashMap::new(),
            jobs: HashMap::new(),
            delivery: Delivery::with_order(NonZeroUsize::new(64).unwrap(), wake, order),
            events: effects,
        }
    }
    pub fn get(&self, host: &HostId, instance: HostInstance, server: &str) -> Option<&McpSignIn> {
        self.by_server
            .get(&(host.clone(), server.to_owned()))
            .filter(|entry| entry.instance == instance)
    }
    pub fn end_host(&mut self, host: &HostId) {
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
    pub fn end_all(&mut self) {
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
    pub fn start(
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
                    let _ = answer.send(Ok(url));
                    return answered;
                }
                (McpSignInPhase::Starting | McpSignInPhase::Waiting, _) => {
                    let _ = answer.send(Err(StartMcpSignInError::StillStarting {
                        host: key.0.clone(),
                        server: key.1.clone(),
                    }));
                    return answered;
                }
                (McpSignInPhase::Finishing, _) => {
                    let _ = answer.send(Err(StartMcpSignInError::Finishing {
                        host: key.0.clone(),
                        server: key.1.clone(),
                    }));
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
        let task = runtime().spawn(async move {
            let begun = async {
                let (listener, redirect_uri) =
                    bind().await.map_err(|source| StartMcpSignInError::Listen {
                        server: task_key.1.clone(),
                        source,
                    })?;
                let started = client
                    .mcp_sign_in_start(&McpSignInStart {
                        server: task_key.1.clone(),
                        redirect_uri,
                    })
                    .await
                    .map_err(|source| StartMcpSignInError::Start {
                        host: task_key.0.clone(),
                        server: task_key.1.clone(),
                        source,
                    })?;
                Ok::<_, StartMcpSignInError>((listener, started))
            }
            .await;
            let (listener, started) = match begun {
                Ok(value) => value,
                Err(error) => {
                    let _ = sender
                        .send(Message::Phase {
                            key: task_key.clone(),
                            id,
                            phase: McpSignInPhase::Failed(::diagnostics::chain(&error)),
                        })
                        .await;
                    let _ = answer.send(Err(error));
                    let _ = sender.send(Message::Cleanup { key: task_key, id }).await;
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
                client.mcp_sign_in_cancel(&started.state).await.ok();
                let _ = answer.send(Err(StartMcpSignInError::Cancelled {
                    host: task_key.0.clone(),
                    server: task_key.1.clone(),
                }));
                let _ = sender.send(Message::Finished { key: task_key, id }).await;
                return;
            }
            let _ = answer.send(Ok(started.authorize_url));
            let state = started.state;
            let (code, stream) = match wait_for_code(listener, state.clone(), cancel).await {
                Ok(Some(found)) => found,
                Ok(None) => {
                    let _ = sender.send(Message::Finished { key: task_key, id }).await;
                    return;
                }
                Err(error) => {
                    client.mcp_sign_in_cancel(&state).await.ok();
                    let _ = sender
                        .send(Message::Phase {
                            key: task_key.clone(),
                            id,
                            phase: McpSignInPhase::Failed(error.to_string()),
                        })
                        .await;
                    let _ = sender.send(Message::Cleanup { key: task_key, id }).await;
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
            let _ = sender.send(message).await;
            let _ = sender.send(Message::Cleanup { key: task_key, id }).await;
        });
        self.jobs.insert((key, id), AbortOnDropHandle::new(task));
        answered
    }
    pub fn cancel(&mut self, host: &HostId, server: &str, client: ForgeClient) {
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
    pub fn peek_order(&mut self) -> Option<u64> {
        self.delivery.peek_order()
    }
    pub fn drain(&mut self) {
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
                    let _ = answer.send(wanted);
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
