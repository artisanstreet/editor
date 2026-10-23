//! Signing in to a host's HTTP MCP servers. Forge runs the OAuth flow and
//! keeps the tokens; this Editor opens the browser and catches its redirect
//! on a loopback address (RFC 8252), then hands the code to Forge. The
//! browser runs where the Editor does, so this works for remote hosts too.

use crate::host::HostData;
use crate::runtime::runtime;
use crate::{Core, CoreEvent, HostId};
use anyhow::{Context as _, Result, anyhow, bail};
use artisan_api::{McpSignInFinish, McpSignInStart};
use gpui::{Context, Entity, Task};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

/// The port the redirect comes back to when it is free. A stable address
/// lets Forge reuse the client it registered with the server.
const PREFERRED_PORT: u16 = 33418;
/// How long a sign-in waits for the browser.
const WAITS: Duration = Duration::from_mins(10);
/// The most of a request the callback reads.
const MAX_REQUEST: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpSignInPhase {
    /// Asking Forge where to sign in.
    Starting,
    /// The sign-in page is open; waiting for the browser to come back.
    Waiting,
    /// Forge is trading the code for tokens.
    Finishing,
    Failed(String),
}

/// One server's sign-in in progress, or the reason the last one failed.
#[derive(Clone, Debug)]
pub struct McpSignIn {
    pub phase: McpSignInPhase,
    /// The page the user approves on, once Forge has answered.
    pub authorize_url: Option<String>,
    id: u64,
    state: Option<String>,
    cancel: CancellationToken,
}

#[derive(Default)]
pub struct McpSignIns {
    next_id: u64,
    by_server: HashMap<(HostId, String), McpSignIn>,
}

impl Core {
    /// The sign-in of `server` on `host`, while one runs or after it failed.
    #[must_use]
    pub fn mcp_sign_in(&self, host: &HostId, server: &str) -> Option<&McpSignIn> {
        self.mcp_sign_ins
            .by_server
            .get(&(host.clone(), server.to_owned()))
    }

    /// Starts signing in `server` on `host`, or reuses the sign-in already
    /// waiting, so the card's button and an agent never race. With `open`
    /// the page opens in the browser. The task answers the page's address.
    #[expect(
        clippy::too_many_lines,
        reason = "one sign-in from start to finish, in the order it happens"
    )]
    pub fn start_mcp_sign_in(
        &mut self,
        host: &Entity<HostData>,
        server: &str,
        open: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<String>> {
        let key = (host.read(cx).id.clone(), server.to_owned());
        if let Some(existing) = self.mcp_sign_ins.by_server.get(&key) {
            match (&existing.phase, existing.authorize_url.clone()) {
                (McpSignInPhase::Waiting, Some(url)) => {
                    if open {
                        cx.open_url(&url);
                    }
                    return Task::ready(Ok(url));
                }
                (McpSignInPhase::Starting | McpSignInPhase::Waiting, _) => {
                    return Task::ready(Err(anyhow!(
                        "The sign-in for {server} is still starting; try again in a moment."
                    )));
                }
                (McpSignInPhase::Finishing, _) => {
                    return Task::ready(Err(anyhow!("{server} is finishing its sign-in already.")));
                }
                (McpSignInPhase::Failed(_), _) => {}
            }
        }
        let id = self.mcp_sign_ins.next_id;
        self.mcp_sign_ins.next_id += 1;
        let cancel = CancellationToken::new();
        self.mcp_sign_ins.by_server.insert(
            key.clone(),
            McpSignIn {
                phase: McpSignInPhase::Starting,
                authorize_url: None,
                id,
                state: None,
                cancel: cancel.clone(),
            },
        );
        Self::sign_ins_changed(cx);
        let client = host.read(cx).client.clone();
        let (answer, answered) = futures::channel::oneshot::channel::<Result<String>>();
        cx.spawn(async move |this, cx| {
            let begun = async {
                let (listener, redirect_uri) =
                    AbortOnDropHandle::new(runtime().spawn(bind())).await??;
                let started = client
                    .mcp_sign_in_start(&McpSignInStart {
                        server: key.1.clone(),
                        redirect_uri,
                    })
                    .await?;
                anyhow::Ok((listener, started))
            }
            .await;
            let (listener, started) = match begun {
                Ok(begun) => begun,
                Err(error) => {
                    let message = format!("{error:#}");
                    this.update(cx, |core, cx| {
                        core.settle(&key, id, McpSignInPhase::Failed(message.clone()), cx);
                    })
                    .ok();
                    answer.send(Err(anyhow!(message))).ok();
                    return;
                }
            };
            let still_wanted = this
                .update(cx, |core, cx| {
                    let Some(entry) = core.mcp_sign_ins.by_server.get_mut(&key) else {
                        return false;
                    };
                    if entry.id != id {
                        return false;
                    }
                    entry.phase = McpSignInPhase::Waiting;
                    entry.authorize_url = Some(started.authorize_url.clone());
                    entry.state = Some(started.state.clone());
                    if open {
                        cx.open_url(&started.authorize_url);
                    }
                    Self::sign_ins_changed(cx);
                    true
                })
                .unwrap_or(false);
            if !still_wanted {
                client.mcp_sign_in_cancel(&started.state).await.ok();
                answer.send(Err(anyhow!("The sign-in was cancelled."))).ok();
                return;
            }
            answer.send(Ok(started.authorize_url.clone())).ok();
            let state = started.state.clone();
            let waited = AbortOnDropHandle::new(runtime().spawn(wait_for_code(
                listener,
                state.clone(),
                cancel,
            )))
            .await;
            let (code, stream) = match waited {
                Ok(Ok(Some(found))) => found,
                // Cancelled: whoever cancelled tidied up.
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    client.mcp_sign_in_cancel(&state).await.ok();
                    let message = format!("{error:#}");
                    this.update(cx, |core, cx| {
                        core.settle(&key, id, McpSignInPhase::Failed(message), cx);
                    })
                    .ok();
                    return;
                }
                Err(error) => {
                    let message = format!("Waiting for the browser failed: {error:#}");
                    this.update(cx, |core, cx| {
                        core.settle(&key, id, McpSignInPhase::Failed(message), cx);
                    })
                    .ok();
                    return;
                }
            };
            this.update(cx, |core, cx| {
                core.settle(&key, id, McpSignInPhase::Finishing, cx);
            })
            .ok();
            let finished = client
                .mcp_sign_in_finish(&McpSignInFinish { state, code })
                .await;
            let page = match &finished {
                Ok(_) => Page::signed_in(&key.1),
                Err(error) => Page::failed(&key.1, &format!("{error:#}")),
            };
            runtime().spawn(respond(stream, page));
            this.update(cx, |core, cx| match finished {
                Ok(_) => core.forget(&key, id, cx),
                Err(error) => {
                    core.settle(&key, id, McpSignInPhase::Failed(format!("{error:#}")), cx);
                }
            })
            .ok();
        })
        .detach();
        cx.spawn(async move |_, _| {
            answered
                .await
                .map_err(|cancelled| anyhow!("The sign-in stopped: {cancelled}"))?
        })
    }

    /// Stops a waiting sign-in, or clears the reason one failed.
    pub fn cancel_mcp_sign_in(
        &mut self,
        host: &Entity<HostData>,
        server: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (host.read(cx).id.clone(), server.to_owned());
        let Some(entry) = self.mcp_sign_ins.by_server.remove(&key) else {
            return;
        };
        entry.cancel.cancel();
        if let Some(state) = entry.state {
            let client = host.read(cx).client.clone();
            cx.spawn(async move |_, _| {
                if let Err(error) = client.mcp_sign_in_cancel(&state).await {
                    tracing::debug!("cancelling an MCP sign-in: {error:#}");
                }
            })
            .detach();
        }
        Self::sign_ins_changed(cx);
    }

    /// Records where sign-in `id` stands, unless a newer one replaced it.
    fn settle(
        &mut self,
        key: &(HostId, String),
        id: u64,
        phase: McpSignInPhase,
        cx: &mut Context<Self>,
    ) {
        if let Some(entry) = self.mcp_sign_ins.by_server.get_mut(key)
            && entry.id == id
        {
            entry.phase = phase;
            Self::sign_ins_changed(cx);
        }
    }

    fn forget(&mut self, key: &(HostId, String), id: u64, cx: &mut Context<Self>) {
        if self
            .mcp_sign_ins
            .by_server
            .get(key)
            .is_some_and(|entry| entry.id == id)
        {
            self.mcp_sign_ins.by_server.remove(key);
            Self::sign_ins_changed(cx);
        }
    }

    fn sign_ins_changed(cx: &mut Context<Self>) {
        cx.emit(CoreEvent::McpSignInChanged);
        cx.notify();
    }
}

/// Listens on the preferred port, or any free one when it is taken.
async fn bind() -> Result<(TcpListener, String)> {
    let listener = match TcpListener::bind(("127.0.0.1", PREFERRED_PORT)).await {
        Ok(listener) => listener,
        Err(taken) => {
            tracing::debug!("port {PREFERRED_PORT} is taken ({taken}); using another");
            TcpListener::bind(("127.0.0.1", 0))
                .await
                .context("listen for the browser's redirect")?
        }
    };
    let port = listener.local_addr()?.port();
    Ok((listener, format!("http://127.0.0.1:{port}/callback")))
}

/// The code the browser brings back for `state`, with the connection to
/// answer it on. `None` when cancelled.
async fn wait_for_code(
    listener: TcpListener,
    state: String,
    cancel: CancellationToken,
) -> Result<Option<(String, TcpStream)>> {
    let deadline = tokio::time::sleep(WAITS);
    tokio::pin!(deadline);
    loop {
        let mut stream = tokio::select! {
            () = cancel.cancelled() => return Ok(None),
            () = &mut deadline => bail!("The browser didn't come back within 10 minutes; start the sign-in again."),
            accepted = listener.accept() => accepted.context("accept the browser's redirect")?.0,
        };
        let Ok(Ok(target)) =
            tokio::time::timeout(Duration::from_secs(10), read_target(&mut stream)).await
        else {
            continue;
        };
        match callback(&target, &state) {
            Callback::Code(code) => return Ok(Some((code, stream))),
            Callback::Denied(reason) => {
                respond(stream, Page::failed("the server", &reason)).await;
                bail!("The sign-in was refused: {reason}");
            }
            // A favicon, a stale tab: not this sign-in.
            Callback::Other => respond(stream, Page::not_found()).await,
        }
    }
}

/// The request target of the first line (`GET /callback?… HTTP/1.1`).
async fn read_target(stream: &mut TcpStream) -> Result<String> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 2048];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.len() > MAX_REQUEST {
            bail!("the request is too large");
        }
    }
    let text = String::from_utf8_lossy(&request);
    let line = text.lines().next().unwrap_or_default();
    let mut parts = line.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("GET"), Some(target)) => Ok(target.to_owned()),
        _ => bail!("not a GET request"),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Code(String),
    /// The user or the server refused.
    Denied(String),
    Other,
}

/// What a request to the loopback address means for the sign-in `state`.
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

/// The page the browser shows after the redirect.
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

#[cfg(test)]
mod tests {
    use super::{Callback, callback, escape};

    #[test]
    fn the_callback_takes_only_this_sign_ins_code() {
        assert_eq!(
            callback("/callback?code=abc&state=s1", "s1"),
            Callback::Code("abc".to_owned())
        );
        assert_eq!(
            callback("/callback?code=abc&state=s2", "s1"),
            Callback::Other
        );
        assert_eq!(callback("/favicon.ico", "s1"), Callback::Other);
        assert_eq!(callback("/callback?state=s1", "s1"), Callback::Other);
        assert_eq!(
            callback(
                "/callback?error=access_denied&error_description=The%20user%20declined&state=s1",
                "s1"
            ),
            Callback::Denied("The user declined".to_owned())
        );
        assert_eq!(
            callback("/callback?error=access_denied&state=s1", "s1"),
            Callback::Denied("access_denied".to_owned())
        );
    }

    #[test]
    fn page_text_is_escaped() {
        assert_eq!(
            escape("<a href=\"x\">&</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;"
        );
    }
}
