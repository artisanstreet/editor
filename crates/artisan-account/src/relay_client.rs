//! Tunneled Relay client for account Forges (native Editor, CLI, harnesses).
//!
//! The Forge holds one WebSocket to its Relay room and serves Noise
//! channels as responder; this client opens channels as initiator, presents
//! its Core grant JWT inside the encrypted confirm, and verifies the Forge
//! responder confirm against the enrolled key from the authenticated Core
//! machine record. Outer WSS is transport-only; E2E is Noise XX_25519.
//! Grant JWTs are short TTL (<=300 s): the transport refreshes via the
//! session-authed [`CoreSessionClient`] and re-handshakes (new channel)
//! before expiry, so a long-lived connection never dies permanently.
//!
//! No GPUI here: every entry is runtime-agnostic async; callers run it on
//! the network runtime (never an ambient reactor).

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{CoreSessionClient, SessionToken};

/// Terminal close signal shared by every handle of one channel.
///
/// `tokio::sync::watch::Sender::send` drops the value when no receiver
/// exists yet, so a bare `send(true)` can miss a reader that subscribes
/// just after [`close`]. [`close`] therefore uses `send_replace`, which
/// retains the terminal state regardless of receivers, and [`aborted`]
/// checks the atomic flag too — a close-before-subscribe or
/// close-while-waiting race always resolves to closed, never to a stream
/// timeout.
#[derive(Clone, Debug)]
struct CloseSignal {
    closed: Arc<AtomicBool>,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl CloseSignal {
    fn new() -> Self {
        Self {
            closed: Arc::new(AtomicBool::new(false)),
            shutdown: tokio::sync::watch::Sender::new(false),
        }
    }

    /// Record the terminal state. Idempotent; safe from any executor.
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.shutdown.send_replace(true);
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Completes once [`close`] ran — including a close that happened
    /// before this waiter subscribed.
    #[expect(
        clippy::let_underscore_must_use,
        reason = "close signal result is irrelevant; waking is what matters"
    )]
    async fn aborted(&self) {
        if self.is_closed() {
            return;
        }
        let mut rx = self.shutdown.subscribe();
        // Re-check after subscribing: `send_replace` retains the terminal
        // value, so a close racing the subscribe is still observed here.
        if *rx.borrow() || self.is_closed() {
            return;
        }
        let _ = rx.changed().await;
    }
}

/// Everything the Relay dialer needs beyond one prepared request.
///
/// Where the Relay lives plus the session-authed Core client for grant refresh.
/// Built once after sign-in; Emily wires it into `set_relay_dialer`.
#[derive(Clone)]
pub struct RelayDialContext {
    /// `wss://relay.example` (no trailing slash; `https://` also accepted).
    pub relay_base_url: String,
    /// Session-authed Core HTTP for grant refresh.
    pub core: CoreSessionClient,
    /// Persistent session token (grants are short-lived; this is not).
    pub session: SessionToken,
}

/// Relay base URL from deployment config (`AST_RELAY_URL`). `None` when
/// unset: the Relay path stays off while direct pairing keeps working.
#[must_use]
pub fn relay_base_url_from_env() -> Option<String> {
    std::env::var("AST_RELAY_URL")
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
}

impl RelayDialContext {
    /// Build from deployment config + the signed-in session. `None` when
    /// `AST_RELAY_URL` is unset: the caller leaves the dialer uninstalled
    /// and account Forges explain the Relay path is still arriving.
    #[must_use]
    pub fn from_env(
        core: CoreSessionClient,
        session: SessionToken,
    ) -> Option<Self> {
        relay_base_url_from_env().map(|relay_base_url| Self {
            relay_base_url,
            core,
            session,
        })
    }
}

/// Build the account dialer Emily registers via `set_relay_dialer`.
///
/// The closure captures Core + session + Relay URL; each dial uses the
/// prepared `RelayDialRequest` for the first handshake and refreshes
/// grants from Core from then on.
///
/// Network work runs on the network runtime (never the GPUI executor —
/// WS connect, timers, and tungstenite need its reactor), wrapped so
/// dropping the GPUI `Task` aborts the dial.
/// One tunneled Forge: unary calls share one WS/Noise channel (serialized),
/// each event subscription gets its own WS/Noise channel.
pub struct RelayClient {
    unary: RelayChannel,
    ctx_account: String,
    ctx_machine: String,
    /// Weak handles to every event channel opened by
    /// [`RelayClient::subscribe`], so [`RelayClient::close_all`]
    /// terminates streams as well as unary. Lock is never held across
    /// an await; dead entries are pruned on close.
    streams: Arc<std::sync::Mutex<Vec<std::sync::Weak<RelayChannelInner>>>>,
}

impl Clone for RelayClient {
    fn clone(&self) -> Self {
        Self {
            unary: self.unary.clone(),
            ctx_account: self.ctx_account.clone(),
            ctx_machine: self.ctx_machine.clone(),
            streams: Arc::clone(&self.streams),
        }
    }
}

impl RelayClient {
    /// Open the unary channel: admission, Noise XX, JWT-in-confirm verified
    /// against the Forge key. `forge_ed25519_b64u` is the enrolled key from
    /// the authenticated Core machine record (transcript pinning, never a
    /// TLS fingerprint).
    #[expect(
        clippy::too_many_arguments,
        reason = "relay dial needs endpoint, identity, grant, keys, and session together"
    )]
    pub async fn connect(
        relay_base_url: &str,
        account_id: &str,
        machine_id: &str,
        grant_jwt: &str,
        cnf_secret: &[u8; 32],
        forge_ed25519_b64u: &str,
        core: CoreSessionClient,
        session: SessionToken,
    ) -> Result<Self> {
        let forge_pub = decode_32(forge_ed25519_b64u).context("bad Forge key")?;
        let unary = RelayChannel::connect(
            relay_base_url,
            account_id,
            machine_id,
            "unary",
            grant_jwt,
            cnf_secret,
            &forge_pub,
            core,
            session,
        )
        .await?;
        Ok(Self {
            unary,
            ctx_account: account_id.to_owned(),
            ctx_machine: machine_id.to_owned(),
            streams: Arc::new(std::sync::Mutex::new(Vec::new())),
        })
    }

    /// Promptly terminate the unary channel; in-flight and future calls
    /// fail fast, blocked reads wake at once. See [`RelayChannel::close`]:
    /// synchronous, idempotent, callable from any executor.
    pub fn close(&self) {
        self.unary.close();
    }

    /// Promptly terminate the unary channel and every stream opened by
    /// [`RelayClient::subscribe`]. Sign-out calls this when dropping
    /// account hosts so views briefly holding clones stop at once, and no
    /// grant refresh, reconnect, or new subscription can start afterwards
    /// (those paths check the closed signal first). Synchronous,
    /// idempotent, callable from any executor.
    pub fn close_all(&self) {
        self.close();
        let channels = self
            .streams
            .lock()
            .map(|mut open| {
                open.retain(|weak| std::sync::Weak::upgrade(weak).is_some());
                open.iter()
                    .filter_map(std::sync::Weak::upgrade)
                    .map(|inner| RelayChannel { inner })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for channel in channels {
            channel.close();
        }
    }

    /// Whether [`RelayClient::close_all`] (or [`RelayClient::close`]) ran.
    /// New subscriptions are refused once closed.
    pub fn is_closed(&self) -> bool {
        self.unary.is_closed()
    }

    /// Connect snapshot over the tunnel (same shape as direct `/state`).
    pub async fn state(&self) -> Result<artisan_api::State> {
        self.request_json("GET", "/state", None).await
    }

    /// Unary TunnelRequest with a JSON answer; parses `body` into `T`.
    /// Responses are forward-compatible (unknown fields ignored); requests
    /// never carry unknown fields. Non-2xx answers become `ApiError`.
    pub async fn request_json<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let response = self.raw_request(method, path, body).await?;
        serde_json::from_value(response.body).context("read Forge's answer")
    }

    /// Unary call answered with `()`; non-2xx becomes `ApiError`.
    pub async fn request_empty(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<()> {
        self.raw_request(method, path, body).await.map(drop)
    }

    /// Unary call returning the checked `TunnelResponse` itself.
    pub async fn raw_request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<artisan_api::TunnelResponse> {
        let response = self.unary.call(method, path, body).await?;
        check_tunnel(&response)?;
        Ok(response)
    }

    /// Binary download with a byte ceiling: `$bytes` envelope decoded, else
    /// UTF-8/JSON fallback. Enforces `limit`, never truncates.
    pub async fn raw_bytes(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        limit: usize,
    ) -> Result<Vec<u8>> {
        let response = self.raw_request(method, path, body).await?;
        let bytes = decode_tunnel_bytes(&response)?;
        anyhow::ensure!(
            bytes.len() <= limit,
            "Forge response exceeds {limit} bytes"
        );
        Ok(bytes)
    }

    /// Binary download: `$bytes` envelope decoded, else UTF-8/JSON fallback.
    pub async fn request_bytes(&self, path: &str) -> Result<Vec<u8>> {
        self.raw_bytes("GET", path, None, 32 * 1024 * 1024).await
    }

    /// Open a dedicated event stream channel (`SUBSCRIBE`), returning the
    /// stream id and a live envelope receiver. Oversized envelopes arrive
    /// chunked and reassembled; a lagged broadcast or full queue ends the
    /// stream with `Reset` (caller re-subscribes from its `since` cursor).
    /// Refused once [`RelayClient::close_all`] ran.
    pub async fn subscribe(&self, path: &str) -> Result<RelayEventStream> {
        // Signal first: no new subscriptions after close.
        if self.is_closed() {
            anyhow::bail!("relay connection closed");
        }
        let stream = RelayEventStream::open(
            &self.unary,
            &self.ctx_account,
            &self.ctx_machine,
            path,
        )
        .await?;
        if let Ok(mut open) = self.streams.lock() {
            open.retain(|weak| weak.upgrade().is_some());
            open.push(std::sync::Arc::downgrade(&stream.channel.inner));
        }
        Ok(stream)
    }
}

fn check_tunnel(response: &artisan_api::TunnelResponse) -> Result<()> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let (code, message) = tunnel_error(&response.body);
    let mut error = serde_json::from_value::<artisan_api::ApiError>(serde_json::json!({
        "code": code,
        "message": message,
    }))
    .unwrap_or_else(|_| artisan_api::ApiError {
        code: artisan_api::ErrorCode::Internal,
        message: format!("Forge answered {}", response.status),
    });
    if matches!(response.status, 401 | 403) {
        error.code = artisan_api::ErrorCode::Unauthorized;
    }
    Err(anyhow!(error))
}

fn tunnel_error(body: &serde_json::Value) -> (String, String) {
    let code = body
        .pointer("/error/code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("tunnel_error")
        .to_owned();
    let message = body
        .pointer("/error/message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("the Forge did not complete the tunneled call")
        .to_owned();
    (code, message)
}

fn decode_tunnel_bytes(response: &artisan_api::TunnelResponse) -> Result<Vec<u8>> {
    use base64::Engine as _;
    if let Some(envelope) = response.body.get(artisan_api::BYTES_ENVELOPE_KEY) {
        let bytes: artisan_api::TunnelBytes =
            serde_json::from_value(envelope.clone()).context("read Forge bytes")?;
        return base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(bytes.b64u.as_bytes())
            .context("read Forge bytes");
    }
    if let Some(text) = response.body.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    serde_json::to_vec(&response.body).context("read Forge bytes")
}

#[expect(
    clippy::map_err_ignore,
    reason = "relay errors use user-facing messages; transport details stay out of logs"
)]
fn decode_32(b64u: &str) -> Result<[u8; 32]> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64u.as_bytes())
        .map_err(|_| anyhow!("bad key encoding"))?;
    raw.try_into()
        .map_err(|_| anyhow!("bad key length"))
}

/// Short grant TTLs by design: refresh a minute before expiry.
fn grant_expires_at(jwt: &str) -> Option<i64> {
    use base64::Engine as _;
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("exp")?.as_i64()
}

fn grant_fresh_enough(jwt: &str) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs().cast_signed());
    grant_expires_at(jwt).is_some_and(|exp| exp - now > 60)
}


#[expect(
    clippy::map_err_ignore,
    reason = "relay errors use user-facing messages; transport details stay out of logs"
)]
fn ws_url_for(relay_base: &str, account: &str, machine: &str) -> Result<String> {
    let route = crate::route_path(account, machine).map_err(|_| anyhow!("bad relay route"))?;
    let mut base = relay_base.trim_end_matches('/').to_owned();
    if let Some(rest) = base.strip_prefix("https://") {
        base = format!("wss://{rest}");
    } else if let Some(rest) = base.strip_prefix("http://") {
        base = format!("ws://{rest}");
    }
    if !(base.starts_with("ws://") || base.starts_with("wss://")) {
        anyhow::bail!("relay URL must be ws(s):// or http(s)://");
    }
    Ok(format!("{base}{}", route.path))
}

/// One WS + Noise XX channel with send-window pacing and chunk reassembly.
struct RelayChannel {
    inner: Arc<RelayChannelInner>,
}

struct RelayChannelInner {
    relay_base: String,
    account: String,
    machine: String,
    channel: String,
    forge_pub: [u8; 32],
    core: CoreSessionClient,
    session: SessionToken,
    state: tokio::sync::Mutex<ChannelState>,
    /// Terminal close signal shared by every handle of this channel.
    close: CloseSignal,
}

struct ChannelState {
    sink: WsSink,
    stream: WsStream,
    noise: crate::NoiseChannel,
    send_seq: u64,
    recv_seq: u64,
    window: RelaySendWindow,
    reassembly: artisan_api::Reassembly,
    /// Complete responses that arrived while pacing a send (stop-and-wait
    /// acks); the following `read_response` picks them up by id.
    stashed: Vec<artisan_api::TunnelResponse>,
    grant_jwt: String,
    cnf_secret: [u8; 32],
}

type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Message,
>;
type WsStream = futures::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Client-side send window mirroring the Relay room: at most 64 unacked
/// frames AND 1 MiB unacked bytes per direction, sizes counted as the
/// BASE64 STRING length exactly as the room counts. Overflow parks in a
/// bounded FIFO; once anything parks, everything after parks behind it
/// (strict FIFO preserves Noise nonce order).
struct RelaySendWindow {
    sent_high: Option<u64>,
    acked_high: Option<u64>,
    sizes: std::collections::VecDeque<usize>,
    unacked_bytes: u64,
    parked: std::collections::VecDeque<String>,
}

impl RelaySendWindow {
    const FRAMES: u64 = 64;
    const BYTES: u64 = 1024 * 1024;
    const PARKED_MAX: usize = 256;

    const fn new() -> Self {
        Self {
            sent_high: None,
            acked_high: None,
            sizes: std::collections::VecDeque::new(),
            unacked_bytes: 0,
            parked: std::collections::VecDeque::new(),
        }
    }

    fn unacked_frames(&self) -> u64 {
        match (self.sent_high, self.acked_high) {
            (Some(sent), Some(acked)) => sent.saturating_sub(acked),
            (Some(_), None) => self.sizes.len() as u64,
            _ => 0,
        }
    }

    fn has_space(&self, body_len: usize) -> bool {
        self.unacked_frames() < Self::FRAMES
            && self.unacked_bytes.saturating_add(body_len as u64) <= Self::BYTES
    }

    fn noted_sent(&mut self, seq: u64, body_len: usize) {
        self.sent_high = Some(seq);
        self.sizes.push_back(body_len);
        self.unacked_bytes = self.unacked_bytes.saturating_add(body_len as u64);
    }

    /// Baseline after a handshake/confirm exchange whose frames traveled
    /// outside this window: seqs `0..=acked` count as sent AND acked, so
    /// the first tracked app frame (seq `acked + 1`, e.g. 3 after m1/m3/
    /// confirm) has exact cumulative accounting. Late handshake acks
    /// (`seq <= acked`) are then ignored rather than misread as credit —
    /// without this, `on_ack(3)` would release one entry yet leave
    /// `acked_high` at 0 and the ack wait would never complete.
    const fn acknowledged_through(acked: u64) -> Self {
        let mut window = Self::new();
        window.sent_high = Some(acked);
        window.acked_high = Some(acked);
        window
    }

    /// Release everything up to `ack_seq` (cumulative). Mirrors the room:
    /// duplicates and ahead-of-sent acks are ignored. Releases exactly the
    /// frames implied by the previous cursor, bounded by the actual queue —
    /// never fabricates credit beyond what was sent.
    fn on_ack(&mut self, ack_seq: u64) {
        if self.acked_high.is_some_and(|acked| ack_seq <= acked) {
            return;
        }
        if self.sent_high.is_none_or(|sent| ack_seq > sent) {
            return;
        }
        let base = self.acked_high.map_or(0, |acked| acked.saturating_add(1));
        if ack_seq < base {
            return;
        }
        let mut to_release = ack_seq.saturating_sub(base).saturating_add(1);
        to_release = to_release.min(self.sizes.len() as u64);
        for _ in 0..to_release {
            let Some(size) = self.sizes.pop_front() else {
                break;
            };
            self.unacked_bytes = self.unacked_bytes.saturating_sub(size as u64);
        }
        if to_release > 0 {
            self.acked_high = Some(base.saturating_add(to_release).saturating_sub(1));
        }
    }

    fn should_park(&self, body_len: usize) -> bool {
        !self.parked.is_empty() || !self.has_space(body_len)
    }
}

impl Drop for ChannelState {
    fn drop(&mut self) {
        self.cnf_secret.fill(0);
    }
}

impl Clone for RelayChannel {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// Open one WS + Noise XX channel: admission, channel open, handshake,
/// then the JWT-in-confirm exchange verified against the Forge key.
async fn open_channel_state(
    relay_base: &str,
    account: &str,
    machine: &str,
    channel: &str,
    grant_jwt: &str,
    cnf_secret: &[u8; 32],
    forge_pub: &[u8; 32],
) -> Result<ChannelState> {
    let url = ws_url_for(relay_base, account, machine)?;
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .with_context(|| format!("connect to the Relay at {url}"))?;
    let (mut sink, mut stream) = futures::StreamExt::split(ws);
    admit(&mut sink, &mut stream, grant_jwt, cnf_secret).await?;
    open_channel(&mut sink, &mut stream, channel).await?;
    let (mut noise, send_seq, recv_seq) =
        handshake_channel(&mut sink, &mut stream, account, machine, channel).await?;
    let (send_seq, recv_seq) = confirm_channel(
        &mut sink,
        &mut stream,
        &mut noise,
        channel,
        send_seq,
        recv_seq,
        grant_jwt,
        cnf_secret,
        forge_pub,
    )
    .await?;
    Ok(ChannelState {
        sink,
        stream,
        noise,
        send_seq,
        recv_seq,
        // m1/m3/confirm used seqs 0..send_seq outside the window; baseline
        // the cursor so the first tracked app frame acks exactly.
        window: RelaySendWindow::acknowledged_through(send_seq.saturating_sub(1)),
        reassembly: artisan_api::Reassembly::new(),
        stashed: Vec::new(),
        grant_jwt: grant_jwt.to_owned(),
        cnf_secret: *cnf_secret,
    })
}

fn next_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static IDS: AtomicU64 = AtomicU64::new(1);
    format!("e{}", IDS.fetch_add(1, Ordering::Relaxed))
}

impl RelayChannel {
    #[expect(
        clippy::too_many_arguments,
        reason = "relay dial needs endpoint, identity, grant, keys, and session together"
    )]
    async fn connect(
        relay_base: &str,
        account: &str,
        machine: &str,
        channel: &str,
        grant_jwt: &str,
        cnf_secret: &[u8; 32],
        forge_pub: &[u8; 32],
        core: CoreSessionClient,
        session: SessionToken,
    ) -> Result<Self> {
        let state = open_channel_state(
            relay_base,
            account,
            machine,
            channel,
            grant_jwt,
            cnf_secret,
            forge_pub,
        )
        .await?;
        Ok(Self {
            inner: Arc::new(RelayChannelInner {
                relay_base: relay_base.to_owned(),
                account: account.to_owned(),
                machine: machine.to_owned(),
                channel: channel.to_owned(),
                forge_pub: *forge_pub,
                core,
                session,
                state: tokio::sync::Mutex::new(state),
                close: CloseSignal::new(),
            }),
        })
    }

    /// Promptly terminate the channel: in-flight and future calls fail
    /// fast, blocked reads wake immediately, no new frames are sealed.
    /// Synchronous and idempotent — safe to call from sign-out on any
    /// executor, even while views still hold clones. Dropping the last
    /// handle then closes the socket; no grant semantics change (remote
    /// session revoke keeps its ≤300 s tail, but the local client stops
    /// at once).
    pub fn close(&self) {
        self.inner.close.close();
    }

    fn is_closed(&self) -> bool {
        self.inner.close.is_closed()
    }

    /// Completes the moment [`RelayChannel::close`] runs (or immediately
    /// when already closed).
    async fn aborted(&self) {
        self.inner.close.aborted().await;
    }

    /// Unary call: seal (chunking when large), paced send, reassembled answer.
    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<artisan_api::TunnelResponse> {
        let id = next_request_id();
        let request = artisan_api::TunnelRequest {
            id: id.clone(),
            method: method.to_owned(),
            path: path.to_owned(),
            body,
        };
        let bytes = serde_json::to_vec(&request).context("encode the tunneled call")?;
        // Signal first: never wait on the mutex of a closed channel.
        if self.is_closed() {
            anyhow::bail!("relay connection closed");
        }
        let mut guard = self.inner.state.lock().await;
        self.ensure_fresh_locked(&mut guard).await?;
        if bytes.len() > artisan_api::TUNNEL_PLAINTEXT_BUDGET {
            for chunk in artisan_api::chunk_bytes(&id, &bytes) {
                let framed = serde_json::to_vec(&chunk).context("encode the tunneled call")?;
                self.send_plaintext_locked(&mut guard, &framed).await?;
            }
        } else {
            self.send_plaintext_locked(&mut guard, &bytes).await?;
        }
        self.read_response_locked(&mut guard, &id).await
    }

    #[expect(
        clippy::map_err_ignore,
        reason = "relay errors use user-facing messages; transport details stay out of logs"
    )]
    async fn ensure_fresh_locked(&self, conn: &mut ChannelState) -> Result<()> {
        // Never refresh or reconnect a closed channel: close_all wins over
        // grant rotation, so logout cannot resurrect a channel mid-teardown.
        if self.is_closed() {
            anyhow::bail!("relay connection closed");
        }
        if grant_fresh_enough(&conn.grant_jwt) {
            return Ok(());
        }
        let keypair =
            crate::MachineKeypair::generate().map_err(|_| anyhow!("make a key"))?;
        let grant = self
            .inner
            .core
            .issue_editor_grant(
                &self.inner.session,
                &self.inner.machine,
                &keypair.public_b64u(),
            )
            .await
            .map_err(|_| anyhow!("refresh the Relay grant"))?;
        *conn = open_channel_state(
            &self.inner.relay_base,
            &self.inner.account,
            &self.inner.machine,
            &self.inner.channel,
            &grant.token,
            keypair.secret(),
            &self.inner.forge_pub,
        )
        .await?;
        Ok(())
    }

    #[expect(
        clippy::map_err_ignore,
        reason = "relay errors use user-facing messages; transport details stay out of logs"
    )]
    async fn send_plaintext_locked(
        &self,
        conn: &mut ChannelState,
        plaintext: &[u8],
    ) -> Result<()> {
        let sealed = conn
            .noise
            .seal(plaintext)
            .map_err(|_| anyhow!("seal the tunneled call"))?;
        self.send_raw_locked(conn, &sealed).await
    }

    async fn send_raw_locked(&self, conn: &mut ChannelState, raw: &[u8]) -> Result<()> {
        use base64::Engine as _;
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        if conn.window.should_park(body.len()) {
            if conn.window.parked.len() >= RelaySendWindow::PARKED_MAX {
                anyhow::bail!("relay window full");
            }
            conn.window.parked.push_back(body);
            self.drain_parked_locked(conn).await?;
            return Ok(());
        }
        let seq = conn.send_seq;
        send_data_frame(&mut conn.sink, &self.inner.channel, seq, &body).await?;
        conn.send_seq = conn.send_seq.saturating_add(1);
        conn.window.noted_sent(seq, body.len());
        self.wait_ack_locked(conn, seq).await?;
        self.drain_parked_locked(conn).await?;
        Ok(())
    }

    async fn drain_parked_locked(&self, conn: &mut ChannelState) -> Result<()> {
        while let Some(body) = conn.window.parked.front().cloned() {
            if !conn.window.has_space(body.len()) {
                break;
            }
            conn.window.parked.pop_front();
            let seq = conn.send_seq;
            send_data_frame(&mut conn.sink, &self.inner.channel, seq, &body).await?;
            conn.send_seq = conn.send_seq.saturating_add(1);
            conn.window.noted_sent(seq, body.len());
            self.wait_ack_locked(conn, seq).await?;
        }
        Ok(())
    }

    #[expect(
        clippy::map_err_ignore,
        reason = "relay errors use user-facing messages; transport details stay out of logs"
    )]
    async fn wait_ack_locked(&self, conn: &mut ChannelState, seq: u64) -> Result<()> {
        let deadline = tokio::time::Duration::from_secs(30);
        loop {
            if conn.window.acked_high.is_some_and(|acked| acked >= seq) {
                return Ok(());
            }
            // Abort wins over the 30 s pacing wait: logout never stalls here.
            let abort = self.aborted();
            tokio::select! {
                biased;
                () = abort => {
                    anyhow::bail!("relay connection closed");
                }
                result = tokio::time::timeout(deadline, read_server_frame(&mut conn.stream)) => {
                    let frame = result
                        .map_err(|_| anyhow!("the Relay did not answer"))??;
                    if let Some(response) = self.handle_frame_locked(conn, frame).await? {
                        conn.stashed.push(response);
                    }
                }
            }
        }
    }

    #[expect(
        clippy::map_err_ignore,
        reason = "relay errors use user-facing messages; transport details stay out of logs"
    )]
    async fn read_response_locked(
        &self,
        conn: &mut ChannelState,
        id: &str,
    ) -> Result<artisan_api::TunnelResponse> {
        if let Some(position) = conn.stashed.iter().position(|response| response.id == id) {
            return Ok(conn.stashed.remove(position));
        }
        let deadline = tokio::time::Duration::from_secs(120);
        loop {
            // Abort wins over the 120 s idle wait: logout never stalls here.
            let abort = self.aborted();
            tokio::select! {
                biased;
                () = abort => {
                    anyhow::bail!("relay connection closed");
                }
                result = tokio::time::timeout(deadline, read_server_frame(&mut conn.stream)) => {
                    let frame = result
                        .map_err(|_| anyhow!("the Forge did not answer"))??;
                    if let Some(response) = self.handle_frame_locked(conn, frame).await? {
                        if response.id == id {
                            return Ok(response);
                        }
                        conn.stashed.push(response);
                    }
                }
            }
        }
    }

    /// Handle one Relay frame. Returns a complete unary `TunnelResponse`
    /// when one arrives (reassembling chunks); `None` for acks and
    /// housekeeping. Responses for other in-flight ids are returned too —
    /// the caller stashes or matches them.
    #[expect(
        clippy::map_err_ignore,
        reason = "relay errors use user-facing messages; transport details stay out of logs"
    )]
    async fn handle_frame_locked(
        &self,
        conn: &mut ChannelState,
        frame: crate::ServerFrame,
    ) -> Result<Option<artisan_api::TunnelResponse>> {
        use crate::ServerFrame;
        match frame {
            ServerFrame::Data(data) => {
                send_ack_frame(&mut conn.sink, &data.channel, data.seq).await?;
                if data.seq != conn.recv_seq {
                    anyhow::bail!("relay data out of order");
                }
                conn.recv_seq = conn.recv_seq.saturating_add(1);
                let raw = decode_32_or_bytes(&data.body)?;
                let plaintext = conn
                    .noise
                    .receive_data(&raw)
                    .map_err(|_| anyhow!("open the Forge reply"))?;
                if let Ok(chunk) = serde_json::from_slice::<artisan_api::TunnelChunk>(&plaintext) {
                    if let Some(done) = conn
                        .reassembly
                        .feed(&self.inner.channel, &chunk)
                        .map_err(|_| anyhow!("reassemble the Forge reply"))?
                    {
                        match classify_frame_body(&done)? {
                            FrameBody::Response(response) => return Ok(Some(response)),
                            // Live $event interleaved while a call is in
                            // flight (e.g. UNSUBSCRIBE racing backlog/live
                            // mail on an event channel): not the awaited
                            // answer — skip it and keep waiting.
                            FrameBody::LiveEvent => return Ok(None),
                        }
                    }
                    return Ok(None);
                }
                match classify_frame_body(&plaintext)? {
                    FrameBody::Response(response) => Ok(Some(response)),
                    // Same interleave as above on the unchunked path.
                    FrameBody::LiveEvent => Ok(None),
                }
            }
            ServerFrame::Ack(ack) => {
                conn.window.on_ack(ack.seq);
                Ok(None)
            }
            ServerFrame::ChannelClosed(closed) => {
                anyhow::bail!("the Relay closed {}", closed.channel)
            }
            ServerFrame::Error(error) => {
                anyhow::bail!("the Relay refused the tunnel: {} {}", error.code, error.message)
            }
            ServerFrame::Challenge(_)
            | ServerFrame::Ready(_)
            | ServerFrame::Presence(_)
            | ServerFrame::Ping(_)
            | ServerFrame::ChannelOpened(_) => Ok(None),
        }
    }
}

fn decode_32_or_bytes(b64u: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64u.as_bytes())
        .context("read the Relay frame")
}

/// What one assembled plaintext holds: a tunneled response, or a live
/// `$event` envelope that interleaved on an event channel while a call
/// was in flight.
enum FrameBody {
    Response(artisan_api::TunnelResponse),
    LiveEvent,
}

/// Split an assembled plaintext without losing error detail. A live
/// `$event` (object with `$event`, no top-level `id`) is normal traffic
/// on event channels — e.g. backlog/live mail racing UNSUBSCRIBE — and
/// must skip the response wait, not fail it. Anything else that is not a
/// `TunnelResponse` keeps the exact serde error via `{e:#}` upstream.
fn classify_frame_body(body: &[u8]) -> Result<FrameBody> {
    match serde_json::from_slice::<artisan_api::TunnelResponse>(body) {
        Ok(response) => Ok(FrameBody::Response(response)),
        Err(decode_error) => {
            let is_event = serde_json::from_slice::<serde_json::Value>(body)
                .is_ok_and(|value| value.get("$event").is_some());
            if is_event {
                Ok(FrameBody::LiveEvent)
            } else {
                Err(anyhow!(decode_error).context("read Forge's answer"))
            }
        }
    }
}

#[expect(
    clippy::map_err_ignore,
    reason = "relay errors use user-facing messages; transport details stay out of logs"
)]
async fn admit(sink: &mut WsSink, stream: &mut WsStream, grant_jwt: &str, cnf_secret: &[u8; 32]) -> Result<()> {
    use futures::SinkExt as _;
    use tokio_tungstenite::tungstenite::Message as Ws;
    let hello = serde_json::to_string(&crate::ClientFrame::Hello(
        crate::HelloFrame {
            v: 1,
            grant: grant_jwt.to_owned(),
        },
    ))?;
    sink.send(Ws::Text(hello.into())).await?;
    let jti = crate::unverified_jti(grant_jwt).map_err(|_| anyhow!("bad grant"))?;
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(15), read_server_frame(stream))
            .await
            .map_err(|_| anyhow!("the Relay did not answer"))??;
        match frame {
            crate::ServerFrame::Challenge(challenge) => {
                use base64::Engine as _;
                let nonce: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(challenge.nonce.as_bytes())
                    .map_err(|_| anyhow!("bad Relay challenge"))?
                    .try_into()
                    .map_err(|_| anyhow!("bad Relay challenge"))?;
                let sig = crate::pop_sign(cnf_secret, &nonce, &jti);
                let auth = serde_json::to_string(&crate::ClientFrame::Authenticate(
                    crate::AuthenticateFrame {
                        v: 1,
                        jti: jti.clone(),
                        sig: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig),
                    },
                ))?;
                sink.send(Ws::Text(auth.into())).await?;
            }
            crate::ServerFrame::Ready(_) => return Ok(()),
            crate::ServerFrame::Error(error) => {
                anyhow::bail!("the Relay refused the tunnel: {} {}", error.code, error.message)
            }
            crate::ServerFrame::ChannelOpened(_)
            | crate::ServerFrame::Data(_)
            | crate::ServerFrame::ChannelClosed(_)
            | crate::ServerFrame::Ack(_)
            | crate::ServerFrame::Presence(_)
            | crate::ServerFrame::Ping(_) => {}
        }
    }
}

#[expect(
    clippy::map_err_ignore,
    reason = "relay errors use user-facing messages; transport details stay out of logs"
)]
async fn open_channel(sink: &mut WsSink, stream: &mut WsStream, channel: &str) -> Result<()> {
    use futures::SinkExt as _;
    use tokio_tungstenite::tungstenite::Message as Ws;
    let open = serde_json::to_string(&crate::ClientFrame::Open(
        crate::OpenFrame {
            v: 1,
            channel: channel.to_owned(),
        },
    ))?;
    sink.send(Ws::Text(open.into())).await?;
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(15), read_server_frame(stream))
            .await
            .map_err(|_| anyhow!("the Relay did not open the channel"))??;
        match frame {
            crate::ServerFrame::ChannelOpened(opened) if opened.channel == channel => {
                return Ok(());
            }
            crate::ServerFrame::Error(error) => {
                anyhow::bail!("the Relay refused the channel: {} {}", error.code, error.message)
            }
            crate::ServerFrame::Challenge(_)
            | crate::ServerFrame::Ready(_)
            | crate::ServerFrame::Data(_)
            | crate::ServerFrame::ChannelClosed(_)
            | crate::ServerFrame::Ack(_)
            | crate::ServerFrame::Presence(_)
            | crate::ServerFrame::Ping(_)
            | crate::ServerFrame::ChannelOpened(_) => {}
        }
    }
}

#[expect(
    clippy::map_err_ignore,
    reason = "relay errors use user-facing messages; transport details stay out of logs"
)]
async fn handshake_channel(
    sink: &mut WsSink,
    stream: &mut WsStream,
    account: &str,
    machine: &str,
    channel: &str,
) -> Result<(crate::NoiseChannel, u64, u64)> {
    let static_secret = fresh_noise_secret()?;
    let ctx = crate::ChannelCtx {
        account: account.to_owned(),
        machine: machine.to_owned(),
        channel: channel.to_owned(),
    };
    let mut noise = crate::NoiseChannel::initiator(&static_secret, None, ctx)
        .map_err(|_| anyhow!("start the Noise handshake"))?;
    // NOTE: no X25519 peer pinning here — there is intentionally no static
    // registry. The Forge generates a fresh Noise static per channel, so the
    // enrolled Ed25519 identity key MUST NOT be passed as the Noise peer
    // static (different key system AND different value; pinning it fails
    // every handshake at m2). Forge identity binds one step later: the
    // initiator confirm carries the Core grant JWT, and the responder
    // confirm signature is verified against the enrolled key.
    let mut out = Vec::new();
    noise
        .next_handshake_message(&mut out)
        .map_err(|_| anyhow!("start the Noise handshake"))?;
    let mut send_seq = 0_u64;
    let mut recv_seq = 0_u64;
    send_data_frame(sink, channel, send_seq, &b64u_encode(&out)).await?;
    send_seq = send_seq.saturating_add(1);
    // XX: initiator sends m1, receives m2, sends m3.
    for _ in 0..8 {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(15), read_server_frame(stream))
            .await
            .map_err(|_| anyhow!("the Forge did not shake hands"))??;
        match frame {
            crate::ServerFrame::Data(data) => {
                send_ack_frame(sink, &data.channel, data.seq).await?;
                if data.seq != recv_seq {
                    anyhow::bail!("relay data out of order");
                }
                recv_seq = recv_seq.saturating_add(1);
                let raw = decode_32_or_bytes(&data.body)?;
                noise
                    .receive_handshake_message(&raw)
                    .map_err(|error| anyhow!("shake hands with the Forge: {error}"))?;
                if noise.phase() != crate::Phase::Handshake {
                    break;
                }
                let mut reply = Vec::new();
                if noise.next_handshake_message(&mut reply).is_ok() && !reply.is_empty() {
                    send_data_frame(sink, channel, send_seq, &b64u_encode(&reply)).await?;
                    send_seq = send_seq.saturating_add(1);
                }
                if noise.phase() != crate::Phase::Handshake {
                    break;
                }
            }
            crate::ServerFrame::Ack(_)
            | crate::ServerFrame::Challenge(_)
            | crate::ServerFrame::Ready(_)
            | crate::ServerFrame::ChannelOpened(_)
            | crate::ServerFrame::Presence(_)
            | crate::ServerFrame::Ping(_) => {}
            crate::ServerFrame::Error(error) => {
                anyhow::bail!("the Relay refused the handshake: {} {}", error.code, error.message)
            }
            crate::ServerFrame::ChannelClosed(closed) => {
                anyhow::bail!("the Relay closed {}", closed.channel)
            }
        }
    }
    if noise.phase() == crate::Phase::Handshake {
        anyhow::bail!("the Forge did not shake hands");
    }
    Ok((noise, send_seq, recv_seq))
}

#[expect(
    clippy::too_many_arguments,
    reason = "confirm needs sink, stream, noise, seqs, grant, keys together"
)]
#[expect(
    clippy::map_err_ignore,
    reason = "relay errors use user-facing messages; transport details stay out of logs"
)]
async fn confirm_channel(
    sink: &mut WsSink,
    stream: &mut WsStream,
    noise: &mut crate::NoiseChannel,
    channel: &str,
    mut send_seq: u64,
    mut recv_seq: u64,
    grant_jwt: &str,
    cnf_secret: &[u8; 32],
    forge_pub: &[u8; 32],
) -> Result<(u64, u64)> {
    let confirm = noise
        .emit_initiator_confirm(cnf_secret, grant_jwt.as_bytes())
        .map_err(|error| anyhow!("confirm the Relay session: {error}"))?;
    send_data_frame(sink, channel, send_seq, &b64u_encode(&confirm)).await?;
    send_seq = send_seq.saturating_add(1);
    for _ in 0..8 {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(15), read_server_frame(stream))
            .await
            .map_err(|_| anyhow!("the Forge did not confirm the session"))??;
        match frame {
            crate::ServerFrame::Data(data) => {
                send_ack_frame(sink, &data.channel, data.seq).await?;
                if data.seq != recv_seq {
                    anyhow::bail!("relay data out of order");
                }
                recv_seq = recv_seq.saturating_add(1);
                let raw = decode_32_or_bytes(&data.body)?;
                noise
                    .receive_responder_confirm(&raw, forge_pub)
                    .map_err(|error| anyhow!("confirm the Relay session: {error}"))?;
                return Ok((send_seq, recv_seq));
            }
            crate::ServerFrame::Ack(_)
            | crate::ServerFrame::Challenge(_)
            | crate::ServerFrame::Ready(_)
            | crate::ServerFrame::ChannelOpened(_)
            | crate::ServerFrame::Presence(_)
            | crate::ServerFrame::Ping(_) => {}
            crate::ServerFrame::Error(error) => {
                anyhow::bail!("the Relay refused the session: {} {}", error.code, error.message)
            }
            crate::ServerFrame::ChannelClosed(closed) => {
                anyhow::bail!("the Relay closed {}", closed.channel)
            }
        }
    }
    anyhow::bail!("the Forge did not confirm the session")
}

#[expect(
    clippy::map_err_ignore,
    reason = "relay errors use user-facing messages; transport details stay out of logs"
)]
fn fresh_noise_secret() -> Result<[u8; 32]> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|_| anyhow!("make a key"))?;
    if secret == [0_u8; 32] {
        anyhow::bail!("make a key");
    }
    Ok(secret)
}

fn b64u_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

async fn send_data_frame(sink: &mut WsSink, channel: &str, seq: u64, body: &str) -> Result<()> {
    use futures::SinkExt as _;
    use tokio_tungstenite::tungstenite::Message as Ws;
    // Real channel name is filled by the caller; the confirm helper passes
    // through here only for its frame encoding.
    let frame = crate::DataFrame {
        v: 1,
        channel: channel.to_owned(),
        seq,
        body: body.to_owned(),
    };
    let text = serde_json::to_string(&crate::ClientFrame::Data(frame))?;
    sink.send(Ws::Text(text.into()))
        .await
        .context("send to the Relay")?;
    Ok(())
}

async fn send_ack_frame(sink: &mut WsSink, channel: &str, seq: u64) -> Result<()> {
    use futures::SinkExt as _;
    use tokio_tungstenite::tungstenite::Message as Ws;
    let frame = crate::AckFrame {
        v: 1,
        channel: channel.to_owned(),
        seq,
    };
    let text = serde_json::to_string(&crate::ClientFrame::Ack(frame))?;
    sink.send(Ws::Text(text.into())).await.ok();
    Ok(())
}

#[expect(
    clippy::map_err_ignore,
    reason = "relay errors use user-facing messages; transport details stay out of logs"
)]
async fn read_server_frame(stream: &mut WsStream) -> Result<crate::ServerFrame> {
    use futures::StreamExt as _;
    use tokio_tungstenite::tungstenite::Message as Ws;
    loop {
        let message = stream
            .next()
            .await
            .context("the Relay closed the connection")??;
        match message {
            Ws::Text(text) => {
                let value: serde_json::Value =
                    serde_json::from_str(&text).map_err(|_| anyhow!("bad Relay frame"))?;
                return crate::read_server_frame(value)
                    .map_err(|_| anyhow!("bad Relay frame"));
            }
            Ws::Close(_) => anyhow::bail!("the Relay closed the connection"),
            Ws::Ping(data) => {
                let _ = data;
            }
            Ws::Binary(_) | Ws::Pong(_) | Ws::Frame(_) => {}
        }
    }
}

/// One live tunneled event subscription on its own WS/Noise channel.
pub struct RelayEventStream {
    channel: RelayChannel,
    stream_id: String,
    ended: Option<String>,
}

impl RelayEventStream {
    #[expect(
        clippy::map_err_ignore,
        reason = "relay errors use user-facing messages; transport details stay out of logs"
    )]
    async fn open(
        unary_like: &RelayChannel,
        account: &str,
        machine: &str,
        path: &str,
    ) -> Result<Self> {
        // A dedicated channel per subscription: unary calls proceed while
        // the stream is open (concurrency preserved, one stream per
        // channel as the Forge requires). Fresh grant + ephemeral `cnf`
        // per channel; nothing secret persists beyond the session.
        let inner = std::sync::Arc::clone(&unary_like.inner);
        let keypair =
            crate::MachineKeypair::generate().map_err(|_| anyhow!("make a key"))?;
        let grant = inner
            .core
            .issue_editor_grant(&inner.session, machine, &keypair.public_b64u())
            .await
            .map_err(|_| anyhow!("prepare the event stream"))?;
        let channel = RelayChannel::connect(
            &inner.relay_base,
            account,
            machine,
            "events",
            &grant.token,
            keypair.secret(),
            &inner.forge_pub,
            inner.core.clone(),
            inner.session.clone(),
        )
        .await?;
        let id = next_request_id();
        let request = artisan_api::TunnelRequest {
            id: id.clone(),
            method: artisan_api::METHOD_SUBSCRIBE.to_owned(),
            path: path.to_owned(),
            body: None,
        };
        let bytes = serde_json::to_vec(&request).context("open the event stream")?;
        let mut guard = channel.inner.state.lock().await;
        channel.send_plaintext_locked(&mut guard, &bytes).await?;
        let response = channel.read_response_locked(&mut guard, &id).await?;
        drop(guard);
        check_tunnel(&response)?;
        let stream_id = response
            .body
            .pointer("/$stream/stream")
            .and_then(serde_json::Value::as_str)
            .context("open the event stream")?
            .to_owned();
        Ok(Self {
            channel,
            stream_id,
            ended: None,
        })
    }

    /// The stream the Forge assigned at subscribe time.
    #[must_use]
    pub const fn stream_id(&self) -> &String {
        &self.stream_id
    }

    /// The terminal reason once the Forge ends the stream, if it has.
    #[must_use]
    pub const fn ended(&self) -> Option<&String> {
        self.ended.as_ref()
    }

    /// Next live envelope. Returns `None` once the Forge ends the stream
    /// (lagged broadcast or full queue arrive as `Reset`: re-subscribe
    /// from the last `seq` via a fresh `subscribe`).
    #[expect(
        clippy::map_err_ignore,
        reason = "relay errors use user-facing messages; transport details stay out of logs"
    )]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "channel state stays locked across the frame read to keep seq and reassembly consistent"
    )]
    pub async fn next_event(&mut self) -> Result<Option<artisan_api::Envelope>> {
        if self.ended.is_some() {
            return Ok(None);
        }
        // Signal first: never wait on the mutex of a closed stream.
        if self.channel.is_closed() {
            anyhow::bail!("relay connection closed");
        }
        let mut guard = self.channel.inner.state.lock().await;
        loop {
            // Abort wins over the 120 s idle wait: logout never stalls here.
            let abort = self.channel.aborted();
            let frame = tokio::select! {
                biased;
                () = abort => {
                    anyhow::bail!("relay connection closed");
                }
                result = tokio::time::timeout(
                    std::time::Duration::from_secs(120),
                    read_server_frame(&mut guard.stream),
                ) => {
                    result
                        .map_err(|_| anyhow!("the Forge event stream stalled"))??
                }
            };
            let channel_name = self.channel.inner.channel.clone();
            match frame {
                crate::ServerFrame::Data(data) => {
                    send_ack_frame(&mut guard.sink, &data.channel, data.seq).await?;
                    if data.seq != guard.recv_seq {
                        anyhow::bail!("relay data out of order");
                    }
                    guard.recv_seq = guard.recv_seq.saturating_add(1);
                    let raw = decode_32_or_bytes(&data.body)?;
                    let plaintext = guard
                        .noise
                        .receive_data(&raw)
                        .map_err(|_| anyhow!("open the Forge event"))?;
                    if let Ok(chunk) =
                        serde_json::from_slice::<artisan_api::TunnelChunk>(&plaintext)
                    {
                        if let Some(done) = guard
                            .reassembly
                            .feed(&channel_name, &chunk)
                            .map_err(|_| anyhow!("reassemble the Forge event"))?
                        {
                            match decode_event(&done)? {
                                StreamFrame::Envelope(envelope) => return Ok(Some(*envelope)),
                                StreamFrame::End(reason) => {
                                    self.ended = Some(reason);
                                    return Ok(None);
                                }
                            }
                        }
                    } else {
                        match decode_event(&plaintext)? {
                            StreamFrame::Envelope(envelope) => return Ok(Some(*envelope)),
                            StreamFrame::End(reason) => {
                                self.ended = Some(reason);
                                return Ok(None);
                            }
                        }
                    }
                }
                crate::ServerFrame::Ack(ack) => {
                    guard.window.on_ack(ack.seq);
                }
                crate::ServerFrame::ChannelClosed(closed) => {
                    anyhow::bail!("the Relay closed {}", closed.channel)
                }
                crate::ServerFrame::Error(error) => {
                    anyhow::bail!(
                        "the Relay refused the event stream: {} {}",
                        error.code,
                        error.message
                    )
                }
                crate::ServerFrame::Challenge(_)
                | crate::ServerFrame::Ready(_)
                | crate::ServerFrame::Presence(_)
                | crate::ServerFrame::Ping(_)
                | crate::ServerFrame::ChannelOpened(_) => {}
            }
        }
    }

    /// End this stream without closing the channel.
    pub async fn unsubscribe(&mut self) -> Result<()> {
        let path = format!("/events/{}", self.stream_id);
        self.channel
            .call(artisan_api::METHOD_UNSUBSCRIBE, &path, None)
            .await?;
        Ok(())
    }

    /// Promptly terminate the stream's channel; in-flight `next_event`
    /// fails fast. Synchronous, idempotent; closing the client via
    /// [`RelayClient::close_all`] covers streams the caller dropped.
    pub fn close(&self) {
        self.channel.close();
    }
}

/// One decoded `$event` frame: a live envelope, or the terminal `end`
/// marker carrying the Forge's reason.
enum StreamFrame {
    Envelope(Box<artisan_api::Envelope>),
    End(String),
}

/// Decode one `$event` frame — never a silent drop: non-event frames fail.
fn decode_event(plaintext: &[u8]) -> Result<StreamFrame> {
    let value: serde_json::Value =
        serde_json::from_slice(plaintext).context("read the Forge event")?;
    let event = value
        .get("$event")
        .context("the Forge event stream sent a non-event frame")?;
    if let Some(end) = event.get("end") {
        let reason = end.as_str().unwrap_or("ended").to_owned();
        return Ok(StreamFrame::End(reason));
    }
    let envelope: artisan_api::Envelope = serde_json::from_value(
        event
            .get("envelope")
            .cloned()
            .context("read the Forge event")?,
    )
    .context("read the Forge event")?;
    Ok(StreamFrame::Envelope(Box::new(envelope)))
}



#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test setup; failures fail the test"
)]
mod tests {
    use super::*;

    async fn completes(signal: &CloseSignal) {
        tokio::time::timeout(std::time::Duration::from_secs(5), signal.aborted())
            .await
            .expect("close must resolve, never hang on a stream timeout");
    }

    #[tokio::test]
    async fn close_before_subscribe_still_wakes() {
        // The race: close() runs while no reader exists yet (watch send
        // would drop the value), then a reader subscribes. send_replace
        // retains the terminal state, so the late subscriber still wakes.
        let signal = CloseSignal::new();
        signal.close();
        completes(&signal).await;
        assert!(signal.is_closed());
    }

    #[tokio::test]
    async fn repeated_close_stays_closed() {
        let signal = CloseSignal::new();
        signal.close();
        signal.close();
        completes(&signal).await;
        assert!(signal.is_closed());
    }

    #[tokio::test]
    async fn close_wakes_a_blocked_reader() {
        let signal = CloseSignal::new();
        let waiter = tokio::spawn({
            let signal = signal.clone();
            async move {
                signal.aborted().await;
            }
        });
        tokio::task::yield_now().await;
        signal.close();
        tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
            .await
            .expect("close must wake a blocked reader")
            .unwrap();
        assert!(signal.is_closed());
    }

    #[test]
    fn first_tracked_app_frame_after_handshake_baseline_acks() {
        // Handshake (m1, m3) + confirm used seqs 0..=2 outside the window.
        // The first tracked app frame has seq 3, not 0: a cumulative
        // ack(3) must release exactly it and advance the cursor to 3.
        // (Without the baseline, ack(3) released one entry yet left
        // acked_high at 0 and the ack wait never completed.)
        let mut window = RelaySendWindow::acknowledged_through(2);
        assert_eq!(window.acked_high, Some(2));
        assert_eq!(window.unacked_frames(), 0);
        // A late handshake ack is ignored, never credit.
        window.on_ack(1);
        assert_eq!(window.acked_high, Some(2));
        assert_eq!(window.unacked_bytes, 0);
        // First tracked app frame: seq 3.
        assert!(window.has_space(100));
        window.noted_sent(3, 100);
        assert_eq!(window.unacked_frames(), 1);
        window.on_ack(3);
        assert_eq!(window.acked_high, Some(3));
        assert_eq!(window.unacked_bytes, 0);
        assert_eq!(window.unacked_frames(), 0);
    }

    #[test]
    fn response_frames_classify_as_responses() {
        let body = br#"{"id":"rs2","status":200,"body":{"unsubscribed":true}}"#;
        let classified = classify_frame_body(body).unwrap();
        assert!(
            matches!(&classified, FrameBody::Response(response) if response.id == "rs2" && response.status == 200),
            "response misclassified as event"
        );
    }

    #[test]
    fn queued_event_frames_skip_the_response_wait() {
        // The exact interleaving that failed native UNSUBSCRIBE: a live
        // $event queued ahead of the response has no top-level `id` and
        // must not fail the wait with "missing field `id`".
        let body = br#"{"$event":{"stream":"events-1","envelope":{"seq":9,"live_revision":0,"type":"setting","key":"label","value":"x"}}}"#;
        let classified = classify_frame_body(body).unwrap();
        assert!(
            matches!(classified, FrameBody::LiveEvent),
            "event misclassified as response"
        );
    }

    #[test]
    fn malformed_frames_keep_the_exact_decode_error() {
        // Not a response and not an event: the caller must see the real
        // serde error via `{e:#}`, not a generic failure.
        let body = br#"{"id":1,"status":"bogus"}"#;
        let result = classify_frame_body(body);
        assert!(result.is_err(), "garbage classified as a frame");
        let text = format!("{:#}", result.err().unwrap());
        assert!(text.contains("read Forge's answer"), "lost context: {text}");
    }
}
