//! Tunneled Relay client for account Forges. Each channel is its own
//! WebSocket carrying one Noise XX session to the Forge; unary calls share
//! one channel, every event subscription gets its own.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use artisan_api::{
    ApiError, Envelope, ErrorCode, Reassembly, StreamEvent, StreamStart, TunnelChunk,
    TunnelRequest, TunnelResponse,
};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::{SinkExt as _, StreamExt as _};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use crate::grant::{is_valid_id, unverified_jti, unverified_part};
use crate::{
    AckFrame, AuthenticateFrame, ChannelCtx, ClientFrame, CoreSessionClient, DataFrame,
    HelloFrame, MachineKeypair, NoiseChannel, OpenFrame, PongFrame, ServerFrame, SessionToken,
    pop_sign,
};

const OPEN_DEADLINE: Duration = Duration::from_secs(30);
const REFRESH_MARGIN_S: i64 = 60;

#[derive(Clone)]
pub struct RelayDialContext {
    pub relay_base_url: String,
    pub core: CoreSessionClient,
    pub session: SessionToken,
}

#[derive(Clone)]
pub struct RelayClient {
    route: Arc<Route>,
    unary: Arc<tokio::sync::Mutex<Option<Conn>>>,
    cancel: CancellationToken,
}

impl RelayClient {
    /// `forge_ed25519_b64u` is the Forge's enrolled key from the
    /// authenticated Core machine record; the responder confirm must be
    /// signed by it.
    #[expect(
        clippy::too_many_arguments,
        reason = "the dial request's fields, kept as the consumers pass them"
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
        let forge_pub: [u8; 32] = URL_SAFE_NO_PAD
            .decode(forge_ed25519_b64u)
            .context("read the Forge key")?
            .try_into()
            .ok()
            .context("the Forge key is not 32 bytes")?;
        let route = Arc::new(Route {
            relay_base: relay_base_url.to_owned(),
            account: account_id.to_owned(),
            machine: machine_id.to_owned(),
            forge_pub,
            core,
            session,
        });
        let cancel = CancellationToken::new();
        let conn = route.open_with("unary", grant_jwt, cnf_secret, &cancel).await?;
        Ok(Self {
            route,
            unary: Arc::new(tokio::sync::Mutex::new(Some(conn))),
            cancel,
        })
    }

    /// Ends the unary channel and every event stream at once; later calls
    /// and subscriptions fail.
    pub fn close_all(&self) {
        self.cancel.cancel();
    }

    pub async fn request_json<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let response = Box::pin(self.request(method, path, body)).await?;
        serde_json::from_value(response.body).context("read Forge's answer")
    }

    /// Decodes a `$bytes` envelope, else the UTF-8/JSON body; refuses
    /// answers over `limit`.
    pub async fn raw_bytes(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        limit: usize,
    ) -> Result<Vec<u8>> {
        let bytes = decode_tunnel_bytes(&Box::pin(self.request(method, path, body)).await?)?;
        ensure!(bytes.len() <= limit, "Forge response exceeds {limit} bytes");
        Ok(bytes)
    }

    pub async fn subscribe(&self, path: &str) -> Result<RelayEventStream> {
        let cancel = self.cancel.child_token();
        let open = async {
            let mut conn = self.route.open("events", &cancel).await?;
            let response = conn
                .call(&request(artisan_api::METHOD_SUBSCRIBE, path, None))
                .await?;
            check_tunnel(&response)?;
            let Subscribed { start } =
                serde_json::from_value(response.body).context("read the event stream id")?;
            Ok::<_, anyhow::Error>((conn, start.stream))
        };
        let (conn, stream_id) = cancel
            .run_until_cancelled(Box::pin(open))
            .await
            .context("relay connection closed")??;
        Ok(RelayEventStream {
            conn,
            stream_id,
            ended: false,
            cancel,
        })
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<TunnelResponse> {
        let request = request(method, path, body);
        let response = self
            .cancel
            .run_until_cancelled(Box::pin(self.call(&request)))
            .await
            .context("relay connection closed")??;
        check_tunnel(&response)?;
        Ok(response)
    }

    /// Reads are repeated once on a fresh channel; a mutation may already
    /// have run, so its retry is the caller's choice.
    async fn call(&self, request: &TunnelRequest) -> Result<TunnelResponse> {
        let mut unary = self.unary.lock().await;
        let mut result = self.call_once(&mut unary, request).await;
        if result.is_err() && matches!(request.method.as_str(), "GET" | "HEAD") {
            result = self.call_once(&mut unary, request).await;
        }
        drop(unary);
        result
    }

    /// The connection is taken out for the call and only put back after a
    /// complete answer, so a failed or abandoned call never leaves a
    /// half-used Noise session behind.
    async fn call_once(
        &self,
        unary: &mut Option<Conn>,
        request: &TunnelRequest,
    ) -> Result<TunnelResponse> {
        let mut conn = match unary.take().filter(Conn::usable) {
            Some(conn) => conn,
            None => self.route.open("unary", &self.cancel).await?,
        };
        let response = conn.call(request).await?;
        *unary = Some(conn);
        Ok(response)
    }
}

#[derive(Deserialize)]
struct Subscribed {
    #[serde(rename = "$stream")]
    start: StreamStart,
}

pub struct RelayEventStream {
    conn: Conn,
    stream_id: String,
    ended: bool,
    cancel: CancellationToken,
}

impl RelayEventStream {
    /// `None` once the Forge ends the stream; a lagging subscriber gets a
    /// `Reset` envelope and re-subscribes from its cursor.
    pub async fn next_event(&mut self) -> Result<Option<Envelope>> {
        while !self.ended {
            let plaintext = self
                .cancel
                .run_until_cancelled(self.conn.next_plaintext())
                .await
                .context("relay connection closed")??;
            let Message::Event(event) = read_message(&plaintext)? else {
                continue;
            };
            let StreamEvent { envelope, end, error, .. } = *event;
            if let Some(error) = error {
                bail!("the Forge ended the event stream: {error}");
            }
            if let Some(envelope) = envelope {
                return Ok(Some(envelope));
            }
            self.ended = end.is_some();
        }
        Ok(None)
    }

    pub async fn unsubscribe(&mut self) -> Result<()> {
        let path = format!("/events/{}", self.stream_id);
        let request = request(artisan_api::METHOD_UNSUBSCRIBE, &path, None);
        let response = self
            .cancel
            .run_until_cancelled(self.conn.call(&request))
            .await
            .context("relay connection closed")??;
        self.ended = true;
        check_tunnel(&response)
    }
}

struct Route {
    relay_base: String,
    account: String,
    machine: String,
    forge_pub: [u8; 32],
    core: CoreSessionClient,
    session: SessionToken,
}

impl Route {
    async fn open(&self, prefix: &str, parent: &CancellationToken) -> Result<Conn> {
        let keypair = MachineKeypair::generate().context("make a connection key")?;
        let grant = self
            .core
            .issue_editor_grant(&self.session, &self.machine, &keypair.public_b64u())
            .await
            .context("refresh the Relay grant")?;
        self.open_with(prefix, &grant.token, keypair.secret(), parent)
            .await
    }

    async fn open_with(
        &self,
        prefix: &str,
        grant_jwt: &str,
        cnf_secret: &[u8; 32],
        parent: &CancellationToken,
    ) -> Result<Conn> {
        tokio::time::timeout(
            OPEN_DEADLINE,
            self.open_channel(prefix, grant_jwt, cnf_secret, parent),
        )
        .await
        .context("the Relay did not open the channel in time")?
    }

    async fn open_channel(
        &self,
        prefix: &str,
        grant_jwt: &str,
        cnf_secret: &[u8; 32],
        parent: &CancellationToken,
    ) -> Result<Conn> {
        let channel = format!("{prefix}-{}", hex::encode(random::<16>()?));
        let url = ws_url(&self.relay_base, &self.account, &self.machine)?;
        let mut socket = connect_socket(&url, parent.child_token()).await?;
        admit(&mut socket, grant_jwt, cnf_secret).await?;
        let expires_at = grant_expires_at(grant_jwt)?;
        socket.send(&ClientFrame::Open(OpenFrame {
            v: 1,
            channel: channel.clone(),
        }))?;
        loop {
            if let ServerFrame::ChannelOpened(opened) = socket.recv().await?
                && opened.channel == channel
            {
                break;
            }
        }
        let ctx = ChannelCtx {
            account: self.account.clone(),
            machine: self.machine.clone(),
            channel: channel.clone(),
        };
        let mut conn = Conn {
            socket,
            noise: NoiseChannel::initiator(&random()?, None, ctx)?,
            channel,
            send_seq: 0,
            recv_seq: 0,
            pending: VecDeque::new(),
            reassembly: Reassembly::new(),
            expires_at,
        };
        conn.handshake(grant_jwt, cnf_secret, &self.forge_pub).await?;
        Ok(conn)
    }
}

async fn admit(socket: &mut Socket, grant_jwt: &str, cnf_secret: &[u8; 32]) -> Result<()> {
    let jti = unverified_jti(grant_jwt)?;
    socket.send(&ClientFrame::Hello(HelloFrame {
        v: 1,
        grant: grant_jwt.to_owned(),
    }))?;
    loop {
        match socket.recv().await? {
            ServerFrame::Challenge(challenge) => {
                let nonce: [u8; 32] = URL_SAFE_NO_PAD
                    .decode(challenge.nonce)
                    .context("read the Relay challenge")?
                    .try_into()
                    .ok()
                    .context("the Relay challenge is not 32 bytes")?;
                socket.send(&ClientFrame::Authenticate(AuthenticateFrame {
                    v: 1,
                    jti: jti.clone(),
                    sig: URL_SAFE_NO_PAD.encode(pop_sign(cnf_secret, &nonce, &jti)),
                }))?;
            }
            ServerFrame::Ready(_) => return Ok(()),
            ServerFrame::ChannelOpened(_)
            | ServerFrame::Data(_)
            | ServerFrame::ChannelClosed(_)
            | ServerFrame::Ack(_)
            | ServerFrame::Presence(_)
            | ServerFrame::Error(_)
            | ServerFrame::Ping(_) => {}
        }
    }
}

/// One open, confirmed Noise channel over its own Relay socket.
struct Conn {
    socket: Socket,
    channel: String,
    noise: NoiseChannel,
    send_seq: u64,
    recv_seq: u64,
    /// Data that arrived while a send waited for its ack, in Noise order.
    pending: VecDeque<Vec<u8>>,
    reassembly: Reassembly,
    expires_at: i64,
}

enum Inbound {
    Data(Vec<u8>),
    Ack(u64),
}

impl Conn {
    /// Forge checks the grant's `exp` on every request, so a channel is
    /// replaced before its grant runs out.
    fn usable(&self) -> bool {
        !self.socket.token.is_cancelled() && self.expires_at - now_sec() > REFRESH_MARGIN_S
    }

    async fn handshake(
        &mut self,
        grant_jwt: &str,
        cnf_secret: &[u8; 32],
        forge_pub: &[u8; 32],
    ) -> Result<()> {
        let mut first = Vec::new();
        self.noise.next_handshake_message(&mut first)?;
        self.send_raw(&first)?;
        let second = self.next_data().await?;
        self.noise
            .receive_handshake_message(&second)
            .context("shake hands with the Forge")?;
        let mut third = Vec::new();
        self.noise.next_handshake_message(&mut third)?;
        self.send_raw(&third)?;
        let confirm = self
            .noise
            .emit_initiator_confirm(cnf_secret, grant_jwt.as_bytes())?;
        self.send_raw(&confirm)?;
        let reply = self.next_data().await?;
        self.noise
            .receive_responder_confirm(&reply, forge_pub)
            .context("confirm the Forge's identity")
    }

    async fn call(&mut self, request: &TunnelRequest) -> Result<TunnelResponse> {
        let bytes = serde_json::to_vec(request).context("encode the tunneled call")?;
        if bytes.len() > artisan_api::TUNNEL_PLAINTEXT_BUDGET {
            for chunk in artisan_api::chunk_bytes(&request.id, &bytes) {
                self.send(&serde_json::to_vec(&chunk).context("encode the tunneled call")?)
                    .await?;
            }
        } else {
            self.send(&bytes).await?;
        }
        loop {
            if let Message::Response(response) = read_message(&self.next_plaintext().await?)?
                && response.id == request.id
            {
                return Ok(response);
            }
        }
    }

    /// Stop-and-wait: the Relay window can never fill.
    async fn send(&mut self, plaintext: &[u8]) -> Result<()> {
        let sealed = self.noise.seal(plaintext)?;
        let seq = self.send_raw(&sealed)?;
        loop {
            match self.inbound().await? {
                Inbound::Ack(acked) if acked >= seq => return Ok(()),
                Inbound::Ack(_) => {}
                Inbound::Data(raw) => {
                    let plaintext = self.noise.receive_data(&raw)?;
                    self.pending.push_back(plaintext);
                }
            }
        }
    }

    fn send_raw(&mut self, bytes: &[u8]) -> Result<u64> {
        let seq = self.send_seq;
        self.socket.send(&ClientFrame::Data(DataFrame {
            v: 1,
            channel: self.channel.clone(),
            seq,
            body: URL_SAFE_NO_PAD.encode(bytes),
        }))?;
        self.send_seq += 1;
        Ok(seq)
    }

    /// One complete application message, reassembled from chunks.
    async fn next_plaintext(&mut self) -> Result<Vec<u8>> {
        loop {
            let plaintext = if let Some(plaintext) = self.pending.pop_front() {
                plaintext
            } else {
                let raw = self.next_data().await?;
                self.noise.receive_data(&raw)?
            };
            let Ok(chunk) = serde_json::from_slice::<TunnelChunk>(&plaintext) else {
                return Ok(plaintext);
            };
            if let Some(whole) = self.reassembly.feed(&self.channel, &chunk)? {
                return Ok(whole);
            }
        }
    }

    async fn next_data(&mut self) -> Result<Vec<u8>> {
        loop {
            if let Inbound::Data(raw) = self.inbound().await? {
                return Ok(raw);
            }
        }
    }

    async fn inbound(&mut self) -> Result<Inbound> {
        loop {
            match self.socket.recv().await? {
                ServerFrame::Data(DataFrame { channel, seq, body, .. }) => {
                    self.socket
                        .send(&ClientFrame::Ack(AckFrame { v: 1, channel, seq }))?;
                    ensure!(seq == self.recv_seq, "the Relay delivered data out of order");
                    self.recv_seq += 1;
                    return Ok(Inbound::Data(
                        URL_SAFE_NO_PAD.decode(body).context("read the Relay frame")?,
                    ));
                }
                ServerFrame::Ack(ack) => return Ok(Inbound::Ack(ack.seq)),
                ServerFrame::Challenge(_)
                | ServerFrame::Ready(_)
                | ServerFrame::ChannelOpened(_)
                | ServerFrame::ChannelClosed(_)
                | ServerFrame::Presence(_)
                | ServerFrame::Error(_)
                | ServerFrame::Ping(_) => {}
            }
        }
    }
}

type WsConnection =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A Relay WebSocket owned by its pump task, which answers heartbeats on
/// its own and never waits on the reader.
struct Socket {
    out: mpsc::UnboundedSender<String>,
    frames: mpsc::UnboundedReceiver<Result<ServerFrame>>,
    /// Cancelled when the client signs out or the socket ends.
    token: CancellationToken,
    _pump: AbortOnDropHandle<()>,
}

impl Socket {
    fn send(&self, frame: &ClientFrame) -> Result<()> {
        let text = serde_json::to_string(frame).context("encode a Relay frame")?;
        self.out.send(text).context("the Relay connection ended")
    }

    async fn recv(&mut self) -> Result<ServerFrame> {
        match self.frames.recv().await.context("the Relay connection ended")?? {
            ServerFrame::ChannelClosed(closed) => {
                bail!("the Relay closed the channel: {}", closed.reason)
            }
            ServerFrame::Error(error) => {
                bail!("the Relay refused: {} {}", error.code, error.message)
            }
            frame @ (ServerFrame::Challenge(_)
            | ServerFrame::Ready(_)
            | ServerFrame::ChannelOpened(_)
            | ServerFrame::Data(_)
            | ServerFrame::Ack(_)
            | ServerFrame::Presence(_)
            | ServerFrame::Ping(_)) => Ok(frame),
        }
    }
}

async fn connect_socket(url: &str, token: CancellationToken) -> Result<Socket> {
    let (ws, _response) = tokio_tungstenite::connect_async(url)
        .await
        .with_context(|| format!("connect to the Relay at {url}"))?;
    let (out, outgoing) = mpsc::unbounded_channel();
    let (incoming, frames) = mpsc::unbounded_channel();
    let pump_token = token.clone();
    let pump = tokio::spawn(async move {
        let outcome = pump_token
            .run_until_cancelled(run_pump(ws, outgoing, &incoming))
            .await;
        pump_token.cancel();
        if let Some(Err(error)) = outcome {
            let _reader_gone = incoming.send(Err(error));
        }
    });
    Ok(Socket {
        out,
        frames,
        token,
        _pump: AbortOnDropHandle::new(pump),
    })
}

/// Incoming frames go to an unbounded queue: Relay data is already bounded
/// by its ack window, which only moves when the reader consumes it.
async fn run_pump(
    ws: WsConnection,
    mut outgoing: mpsc::UnboundedReceiver<String>,
    frames: &mpsc::UnboundedSender<Result<ServerFrame>>,
) -> Result<()> {
    let (mut sink, mut source) = ws.split();
    loop {
        tokio::select! {
            text = outgoing.recv() => {
                let Some(text) = text else { return Ok(()) };
                sink.send(Ws::Text(text.into())).await.context("send to the Relay")?;
            }
            message = source.next() => {
                match message.context("the Relay closed the connection")?.context("read from the Relay")? {
                    Ws::Text(text) => match serde_json::from_str::<ServerFrame>(&text) {
                        Ok(ServerFrame::Ping(ping)) => {
                            let answer = serde_json::to_string(&ClientFrame::Pong(PongFrame { v: 1, t: ping.t }))?;
                            sink.send(Ws::Text(answer.into())).await.context("answer the Relay")?;
                        }
                        Ok(ServerFrame::Presence(_)) | Err(_) => {}
                        Ok(frame) => {
                            let terminal = matches!(frame, ServerFrame::ChannelClosed(_) | ServerFrame::Error(_));
                            if frames.send(Ok(frame)).is_err() || terminal {
                                return Ok(());
                            }
                        }
                    },
                    Ws::Ping(data) => sink.send(Ws::Pong(data)).await.context("answer the Relay")?,
                    Ws::Close(_) => bail!("the Relay closed the connection"),
                    Ws::Binary(_) | Ws::Pong(_) | Ws::Frame(_) => {}
                }
            }
        }
    }
}

enum Message {
    Response(TunnelResponse),
    Event(Box<StreamEvent>),
}

/// A live `$event` can interleave with a response on an event channel.
fn read_message(bytes: &[u8]) -> Result<Message> {
    let mut value: serde_json::Value =
        serde_json::from_slice(bytes).context("read Forge's answer")?;
    match value.get_mut(artisan_api::STREAM_EVENT_KEY).map(serde_json::Value::take) {
        Some(event) => Ok(Message::Event(Box::new(
            serde_json::from_value(event).context("read the Forge event")?,
        ))),
        None => Ok(Message::Response(
            serde_json::from_value(value).context("read Forge's answer")?,
        )),
    }
}

fn check_tunnel(response: &TunnelResponse) -> Result<()> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let detail = |key: &str| {
        response
            .body
            .pointer(&format!("/error/{key}"))
            .and_then(serde_json::Value::as_str)
    };
    let code = if matches!(response.status, 401 | 403) {
        ErrorCode::Unauthorized
    } else {
        detail("code")
            .and_then(|code| serde_json::from_value(code.into()).ok())
            .unwrap_or(ErrorCode::Internal)
    };
    let message = detail("message")
        .map_or_else(|| format!("Forge answered {}", response.status), str::to_owned);
    Err(anyhow!(ApiError { code, message }))
}

fn decode_tunnel_bytes(response: &TunnelResponse) -> Result<Vec<u8>> {
    if let Some(envelope) = response.body.get(artisan_api::BYTES_ENVELOPE_KEY) {
        let bytes: artisan_api::TunnelBytes =
            serde_json::from_value(envelope.clone()).context("read Forge bytes")?;
        return URL_SAFE_NO_PAD.decode(bytes.b64u).context("read Forge bytes");
    }
    if let Some(text) = response.body.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    serde_json::to_vec(&response.body).context("read Forge bytes")
}

fn request(method: &str, path: &str, body: Option<serde_json::Value>) -> TunnelRequest {
    use std::sync::atomic::{AtomicU64, Ordering};
    static IDS: AtomicU64 = AtomicU64::new(1);
    TunnelRequest {
        id: format!("e{}", IDS.fetch_add(1, Ordering::Relaxed)),
        method: method.to_owned(),
        path: path.to_owned(),
        body,
    }
}

fn ws_url(relay_base: &str, account: &str, machine: &str) -> Result<String> {
    ensure!(
        is_valid_id(account) && is_valid_id(machine),
        "the Relay route is not valid"
    );
    let base = relay_base.trim_end_matches('/');
    let base = [("https://", "wss://"), ("http://", "ws://")]
        .into_iter()
        .find_map(|(from, to)| base.strip_prefix(from).map(|rest| format!("{to}{rest}")))
        .unwrap_or_else(|| base.to_owned());
    ensure!(
        base.starts_with("ws://") || base.starts_with("wss://"),
        "the Relay URL must be ws(s):// or http(s)://"
    );
    Ok(format!("{base}/v1/route/{account}/{machine}"))
}

fn grant_expires_at(grant_jwt: &str) -> Result<i64> {
    #[derive(Deserialize)]
    struct Expiry {
        exp: i64,
    }
    let Expiry { exp } = unverified_part(grant_jwt, 1)?;
    Ok(exp)
}

fn now_sec() -> i64 {
    artisan_api::now() / 1000
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).context("read random bytes")?;
    Ok(bytes)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test setup; failures fail the test"
)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    const WAIT: Duration = Duration::from_secs(2);

    async fn listen() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        (listener, url)
    }

    #[tokio::test]
    async fn the_socket_answers_pings_skips_noise_and_ends_on_channel_close() {
        let (listener, url) = listen().await;
        let relay = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            ws.send(Ws::Text(r#"{"v":1,"type":"ping","t":1234}"#.into()))
                .await
                .unwrap();
            let pong = tokio::time::timeout(WAIT, ws.next()).await.unwrap().unwrap().unwrap();
            let pong: serde_json::Value = serde_json::from_str(pong.to_text().unwrap()).unwrap();
            assert_eq!(
                pong,
                serde_json::json!({"v": 1, "type": "pong", "t": 1234}),
                "an idle socket must answer the heartbeat"
            );
            for frame in [
                r#"{"v":1,"type":"presence","forge_online":true,"clients":2}"#,
                r#"{"v":1,"type":"from-a-newer-relay"}"#,
                "not json",
                r#"{"v":1,"type":"ack","channel":"c","seq":3}"#,
                r#"{"v":1,"type":"channel-closed","channel":"c","reason":"forge-offline"}"#,
            ] {
                ws.send(Ws::Text(frame.into())).await.unwrap();
            }
            let ended = tokio::time::timeout(WAIT, ws.next()).await.unwrap();
            assert!(
                !matches!(ended, Some(Ok(Ws::Text(_)))),
                "the socket kept talking after its channel closed"
            );
        });
        let mut socket = connect_socket(&url, CancellationToken::new()).await.unwrap();
        let frame = tokio::time::timeout(WAIT, socket.recv()).await.unwrap().unwrap();
        assert!(
            matches!(frame, ServerFrame::Ack(AckFrame { seq: 3, .. })),
            "presence and unknown frames must not reach the reader: {frame:?}"
        );
        let error = socket.recv().await.unwrap_err();
        assert!(format!("{error:#}").contains("forge-offline"), "{error:#}");
        tokio::time::timeout(WAIT, socket.token.cancelled())
            .await
            .expect("a closed channel marks the socket dead");
        relay.await.unwrap();
    }

    #[tokio::test]
    async fn signing_out_ends_open_sockets() {
        let (listener, url) = listen().await;
        let relay = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let ended = tokio::time::timeout(WAIT, ws.next()).await.unwrap();
            assert!(!matches!(ended, Some(Ok(Ws::Text(_)))), "socket outlived sign-out");
        });
        let client = CancellationToken::new();
        let mut socket = connect_socket(&url, client.child_token()).await.unwrap();
        client.cancel();
        let read = tokio::time::timeout(WAIT, socket.recv()).await.unwrap();
        assert!(read.is_err(), "a reader must wake when the client signs out");
        relay.await.unwrap();
    }

    #[tokio::test]
    async fn signing_out_during_subscribe_ends_it() {
        let core = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let core_url = format!("http://{}", core.local_addr().unwrap());
        let hung_core = tokio::spawn(async move {
            let (_tcp, _) = core.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let client = RelayClient {
            route: Arc::new(Route {
                relay_base: "ws://127.0.0.1:1".to_owned(),
                account: "acct_00000000000000000000000000".to_owned(),
                machine: "mch_11111111111111111111111111".to_owned(),
                forge_pub: [0; 32],
                core: CoreSessionClient::new(&core_url).unwrap(),
                session: SessionToken::new("session".to_owned()),
            }),
            unary: Arc::new(tokio::sync::Mutex::new(None)),
            cancel: CancellationToken::new(),
        };
        let subscribing = tokio::spawn({
            let client = client.clone();
            async move { Box::pin(client.subscribe("/events?since=0")).await.map(drop) }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        client.close_all();
        let outcome = tokio::time::timeout(WAIT, subscribing).await.unwrap().unwrap();
        assert!(outcome.is_err(), "a subscription survived sign-out");
        hung_core.abort();
    }

    #[tokio::test]
    async fn every_socket_opens_a_fresh_channel_name() {
        let (listener, url) = listen().await;
        let relay = tokio::spawn(async move {
            let mut names = std::collections::HashSet::new();
            for _ in 0..4 {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let hello = ws.next().await.unwrap().unwrap();
                assert!(hello.to_text().unwrap().contains("\"hello\""), "hello first");
                ws.send(Ws::Text(
                    r#"{"v":1,"type":"ready","role":"client","account":"a","machine":"m","forge_online":true}"#
                        .into(),
                ))
                .await
                .unwrap();
                let open = ws.next().await.unwrap().unwrap();
                let open: serde_json::Value = serde_json::from_str(open.to_text().unwrap()).unwrap();
                assert_eq!(open["type"], "open", "open after admission");
                let name = open["channel"].as_str().unwrap().to_owned();
                assert!(names.insert(name.clone()), "channel reused: {name}");
                ws.send(Ws::Text(
                    r#"{"v":1,"type":"error","code":"probe-complete","message":"open recorded"}"#.into(),
                ))
                .await
                .unwrap();
            }
        });
        let route = Route {
            relay_base: url,
            account: "a".to_owned(),
            machine: "m".to_owned(),
            forge_pub: [2; 32],
            core: CoreSessionClient::new("http://127.0.0.1:1").unwrap(),
            session: SessionToken::new("session".to_owned()),
        };
        let payload = URL_SAFE_NO_PAD.encode(br#"{"jti":"channel-test","exp":4000000000}"#);
        let grant = format!("header.{payload}.signature");
        for prefix in ["unary", "unary", "events", "events"] {
            let error = route
                .open_with(prefix, &grant, &[1; 32], &CancellationToken::new())
                .await
                .err()
                .unwrap();
            assert!(format!("{error:#}").contains("probe-complete"), "{error:#}");
        }
        relay.await.unwrap();
    }

    #[test]
    fn responses_read_as_responses() {
        let message = read_message(br#"{"id":"rs2","status":200,"body":{"unsubscribed":true}}"#).unwrap();
        assert!(
            matches!(&message, Message::Response(response) if response.id == "rs2"),
            "response misread as an event"
        );
    }

    #[test]
    fn events_interleaved_with_a_response_are_events() {
        let message = read_message(br#"{"$event":{"stream":"events-1","end":"closed"}}"#).unwrap();
        assert!(
            matches!(&message, Message::Event(event) if event.end.as_deref() == Some("closed")),
            "event misread as a response"
        );
    }

    #[test]
    fn malformed_answers_keep_the_decode_error() {
        let error = read_message(br#"{"id":1,"status":"bogus"}"#).err().unwrap();
        let text = format!("{error:#}");
        assert!(text.contains("read Forge's answer") && text.contains("invalid type"), "{text}");
    }
}
