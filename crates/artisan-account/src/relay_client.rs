use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use artisan_api::{
    ApiError, Envelope, ErrorCode, Reassembly, ReassemblyError, StreamEvent, StreamStart,
    TunnelChunk, TunnelRequest, TunnelResponse,
};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::{SinkExt as _, StreamExt as _};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{self, Message as Ws};
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use crate::grant::{UnverifiedGrantError, is_valid_id, unverified_jti, unverified_part};
use crate::{
    AckFrame, AuthenticateFrame, ChannelCtx, ClientFrame, CoreSessionClient, DataFrame, HelloFrame,
    MachineKeyError, MachineKeypair, NoiseChannel, NoiseError, OpenFrame, PongFrame, ServerFrame,
    SessionToken, network::NetworkError, pop_sign,
};

const OPEN_DEADLINE: Duration = Duration::from_secs(30);
const EXCHANGE_DEADLINE: Duration = Duration::from_secs(30);
const REFRESH_MARGIN_S: i64 = 60;

#[derive(Debug, thiserror::Error)]
pub enum ConnectRelayError {
    #[error("decode the Forge's enrolled key")]
    ForgeKey(#[source] base64::DecodeError),
    #[error("the Forge's enrolled key has {bytes} bytes, not 32")]
    ForgeKeyLength { bytes: usize },
    #[error("open the unary Relay channel")]
    Open(#[source] OpenChannelError),
}

#[derive(Debug, thiserror::Error)]
#[error("open a {prefix} Relay channel")]
pub struct OpenChannelError {
    pub prefix: &'static str,
    #[source]
    pub failure: OpenFailure,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenFailure {
    #[error("make a connection key")]
    Key(#[source] MachineKeyError),
    #[error("refresh the Relay grant")]
    Grant(#[source] NetworkError),
    #[error("the Relay did not open the channel within 30 seconds")]
    TimedOut,
    #[error("read random bytes")]
    Random(#[source] getrandom::Error),
    #[error("the Relay route {account}/{machine} is not valid")]
    Route { account: String, machine: String },
    #[error("the Relay URL {url} is not ws(s):// or http(s)://")]
    RelayUrl { url: String },
    #[error("connect to the Relay at {url}")]
    Connect {
        url: String,
        #[source]
        source: tungstenite::Error,
    },
    #[error("read the Relay grant")]
    UnverifiedGrant(#[source] UnverifiedGrantError),
    #[error("decode the Relay challenge")]
    Challenge(#[source] base64::DecodeError),
    #[error("the Relay challenge has {bytes} bytes, not 32")]
    ChallengeLength { bytes: usize },
    #[error("talk to the Relay")]
    Socket(#[source] SocketError),
    #[error("start the Noise session")]
    Noise(#[source] NoiseError),
    #[error("shake hands with the Forge")]
    Handshake(#[source] ExchangeError),
}

#[derive(Debug, thiserror::Error)]
#[error("{method} {path} over the Relay")]
pub struct RelayCallError {
    pub method: String,
    pub path: String,
    #[source]
    pub failure: Box<CallFailure>,
}

#[derive(Debug, thiserror::Error)]
pub enum CallFailure {
    #[error("the Relay connection closed")]
    Closed,
    #[error("open a channel for the call")]
    Open(#[source] OpenChannelError),
    #[error("exchange the call with the Forge")]
    Exchange(#[source] ExchangeError),
    #[error("the Forge did not answer within 30 seconds")]
    TimedOut,
    #[error("the Forge refused the call")]
    Refused(#[source] ApiError),
    #[error("read the Forge's answer")]
    Decode(#[source] serde_json::Error),
    #[error("read the Forge's bytes")]
    Bytes(#[source] TunnelBytesError),
    #[error("the Forge's answer of {bytes} bytes exceeds {limit} bytes")]
    TooLarge { bytes: usize, limit: usize },
}

#[derive(Debug, thiserror::Error)]
pub enum TunnelBytesError {
    #[error("read the bytes envelope")]
    Envelope(#[source] serde_json::Error),
    #[error("decode the bytes envelope")]
    Base64(#[source] base64::DecodeError),
    #[error("encode the JSON answer")]
    Json(#[source] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum EventStreamError {
    #[error("the Relay connection closed")]
    Closed,
    #[error("read event stream {stream}")]
    Exchange {
        stream: String,
        #[source]
        source: ExchangeError,
    },
    #[error("the Forge ended event stream {stream}: {error}")]
    Ended { stream: String, error: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("talk to the Relay")]
    Socket(#[source] SocketError),
    #[error("secure the channel")]
    Noise(#[source] NoiseError),
    #[error("the Relay delivered data {got} where {expected} was next")]
    OutOfOrder { expected: u64, got: u64 },
    #[error("decode a Relay data frame")]
    FrameBody(#[source] base64::DecodeError),
    #[error("reassemble a chunked answer")]
    Reassembly(#[source] ReassemblyError),
    #[error("encode the tunneled call")]
    Encode(#[source] serde_json::Error),
    #[error("read the Forge's answer")]
    Answer(#[source] serde_json::Error),
    #[error("read the Forge event")]
    Event(#[source] serde_json::Error),
}

impl From<SocketError> for ExchangeError {
    fn from(error: SocketError) -> Self {
        Self::Socket(error)
    }
}

impl From<NoiseError> for ExchangeError {
    fn from(error: NoiseError) -> Self {
        Self::Noise(error)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    #[error("encode a Relay frame")]
    Encode(#[source] serde_json::Error),
    #[error("the Relay connection ended")]
    Ended,
    #[error("the Relay closed the channel: {reason}")]
    ChannelClosed { reason: String },
    #[error("the Relay refused: {code} {message}")]
    Refused { code: String, message: String },
    #[error("send to the Relay")]
    Send(#[source] tungstenite::Error),
    #[error("read from the Relay")]
    Read(#[source] tungstenite::Error),
    #[error("the Relay closed the connection")]
    PeerClosed,
}

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
    exchange_deadline: Duration,
}

impl RelayClient {
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
    ) -> Result<Self, ConnectRelayError> {
        let forge_key = URL_SAFE_NO_PAD
            .decode(forge_ed25519_b64u)
            .map_err(ConnectRelayError::ForgeKey)?;
        let forge_pub: [u8; 32] = forge_key
            .try_into()
            .map_err(|key: Vec<u8>| ConnectRelayError::ForgeKeyLength { bytes: key.len() })?;
        let route = Arc::new(Route {
            relay_base: relay_base_url.to_owned(),
            account: account_id.to_owned(),
            machine: machine_id.to_owned(),
            forge_pub,
            core,
            session,
        });
        let cancel = CancellationToken::new();
        let conn = route
            .open_with("unary", grant_jwt, cnf_secret, &cancel)
            .await
            .map_err(ConnectRelayError::Open)?;
        Ok(Self {
            route,
            unary: Arc::new(tokio::sync::Mutex::new(Some(conn))),
            cancel,
            exchange_deadline: EXCHANGE_DEADLINE,
        })
    }

    pub fn close_all(&self) {
        self.cancel.cancel();
    }

    pub async fn request_json<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, RelayCallError> {
        let response = Box::pin(self.request(method, path, body)).await?;
        serde_json::from_value(response.body)
            .map_err(|source| failed(method, path, CallFailure::Decode(source)))
    }

    pub async fn raw_bytes(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        limit: usize,
    ) -> Result<Vec<u8>, RelayCallError> {
        let response = Box::pin(self.request(method, path, body)).await?;
        let bytes = decode_tunnel_bytes(&response)
            .map_err(|source| failed(method, path, CallFailure::Bytes(source)))?;
        if bytes.len() > limit {
            return Err(failed(
                method,
                path,
                CallFailure::TooLarge {
                    bytes: bytes.len(),
                    limit,
                },
            ));
        }
        Ok(bytes)
    }

    pub async fn subscribe(&self, path: &str) -> Result<RelayEventStream, RelayCallError> {
        let method = artisan_api::METHOD_SUBSCRIBE;
        let cancel = self.cancel.child_token();
        let open = async {
            let mut conn = self
                .route
                .open("events", &cancel)
                .await
                .map_err(CallFailure::Open)?;
            let response = conn
                .call(&request(method, path, None))
                .await
                .map_err(CallFailure::Exchange)?;
            check_tunnel(&response).map_err(CallFailure::Refused)?;
            let Subscribed { start } =
                serde_json::from_value(response.body).map_err(CallFailure::Decode)?;
            Ok::<_, CallFailure>((conn, start.stream))
        };
        let (conn, stream_id) = cancel
            .run_until_cancelled(Box::pin(open))
            .await
            .ok_or(CallFailure::Closed)
            .flatten()
            .map_err(|failure| failed(method, path, failure))?;
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
    ) -> Result<TunnelResponse, RelayCallError> {
        let request = request(method, path, body);
        let response = self
            .cancel
            .run_until_cancelled(Box::pin(self.call(&request)))
            .await
            .ok_or(CallFailure::Closed)
            .flatten()
            .and_then(|response| {
                check_tunnel(&response).map_err(CallFailure::Refused)?;
                Ok(response)
            });
        response.map_err(|failure| failed(method, path, failure))
    }

    async fn call(&self, request: &TunnelRequest) -> Result<TunnelResponse, CallFailure> {
        let mut unary = self.unary.lock().await;
        let mut result = self.call_once(&mut unary, request).await;
        if result.is_err() && matches!(request.method.as_str(), "GET" | "HEAD") {
            result = self.call_once(&mut unary, request).await;
        }
        drop(unary);
        result
    }

    async fn call_once(
        &self,
        unary: &mut Option<Conn>,
        request: &TunnelRequest,
    ) -> Result<TunnelResponse, CallFailure> {
        let mut conn = match unary.take().filter(Conn::usable) {
            Some(conn) => conn,
            None => self
                .route
                .open("unary", &self.cancel)
                .await
                .map_err(CallFailure::Open)?,
        };
        let response = tokio::time::timeout(self.exchange_deadline, conn.call(request))
            .await
            .map_err(|_elapsed| CallFailure::TimedOut)?
            .map_err(CallFailure::Exchange)?;
        *unary = Some(conn);
        Ok(response)
    }
}

fn failed(method: &str, path: &str, failure: CallFailure) -> RelayCallError {
    RelayCallError {
        method: method.to_owned(),
        path: path.to_owned(),
        failure: Box::new(failure),
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
    pub async fn next_event(&mut self) -> Result<Option<Envelope>, EventStreamError> {
        while !self.ended {
            let plaintext = self
                .cancel
                .run_until_cancelled(self.conn.next_plaintext())
                .await
                .ok_or(EventStreamError::Closed)?
                .map_err(|source| self.exchange(source))?;
            let Message::Event(event) =
                read_message(&plaintext).map_err(|source| self.exchange(source))?
            else {
                continue;
            };
            let StreamEvent {
                envelope,
                end,
                error,
                ..
            } = *event;
            if let Some(error) = error {
                return Err(EventStreamError::Ended {
                    stream: self.stream_id.clone(),
                    error,
                });
            }
            if let Some(envelope) = envelope {
                return Ok(Some(envelope));
            }
            self.ended = end.is_some();
        }
        Ok(None)
    }

    fn exchange(&self, source: ExchangeError) -> EventStreamError {
        EventStreamError::Exchange {
            stream: self.stream_id.clone(),
            source,
        }
    }

    pub async fn unsubscribe(&mut self) -> Result<(), RelayCallError> {
        let method = artisan_api::METHOD_UNSUBSCRIBE;
        let path = format!("/events/{}", self.stream_id);
        let request = request(method, &path, None);
        let response = self
            .cancel
            .run_until_cancelled(self.conn.call(&request))
            .await
            .ok_or(CallFailure::Closed)
            .and_then(|response| response.map_err(CallFailure::Exchange));
        self.ended = true;
        response
            .and_then(|response| check_tunnel(&response).map_err(CallFailure::Refused))
            .map_err(|failure| failed(method, &path, failure))
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
    async fn open(
        &self,
        prefix: &'static str,
        parent: &CancellationToken,
    ) -> Result<Conn, OpenChannelError> {
        let opening = |failure| OpenChannelError { prefix, failure };
        let keypair =
            MachineKeypair::generate().map_err(|source| opening(OpenFailure::Key(source)))?;
        let grant = self
            .core
            .issue_editor_grant(&self.session, &self.machine, &keypair.public_b64u())
            .await
            .map_err(|source| opening(OpenFailure::Grant(source)))?;
        self.open_with(prefix, &grant.token, keypair.secret(), parent)
            .await
    }

    async fn open_with(
        &self,
        prefix: &'static str,
        grant_jwt: &str,
        cnf_secret: &[u8; 32],
        parent: &CancellationToken,
    ) -> Result<Conn, OpenChannelError> {
        tokio::time::timeout(
            OPEN_DEADLINE,
            self.open_channel(prefix, grant_jwt, cnf_secret, parent),
        )
        .await
        .map_err(|_elapsed| OpenFailure::TimedOut)
        .flatten()
        .map_err(|failure| OpenChannelError { prefix, failure })
    }

    async fn open_channel(
        &self,
        prefix: &str,
        grant_jwt: &str,
        cnf_secret: &[u8; 32],
        parent: &CancellationToken,
    ) -> Result<Conn, OpenFailure> {
        let channel = format!("{prefix}-{}", hex::encode(random::<16>()?));
        let url = ws_url(&self.relay_base, &self.account, &self.machine)?;
        let mut socket = connect_socket(&url, parent.child_token()).await?;
        admit(&mut socket, grant_jwt, cnf_secret).await?;
        let expires_at = grant_expires_at(grant_jwt)?;
        socket
            .send(&ClientFrame::Open(OpenFrame {
                v: 1,
                channel: channel.clone(),
            }))
            .map_err(OpenFailure::Socket)?;
        loop {
            if let ServerFrame::ChannelOpened(opened) =
                socket.recv().await.map_err(OpenFailure::Socket)?
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
            noise: NoiseChannel::initiator(&random()?, None, ctx).map_err(OpenFailure::Noise)?,
            channel,
            send_seq: 0,
            recv_seq: 0,
            pending: VecDeque::new(),
            reassembly: Reassembly::new(),
            expires_at,
        };
        conn.handshake(grant_jwt, cnf_secret, &self.forge_pub)
            .await
            .map_err(OpenFailure::Handshake)?;
        Ok(conn)
    }
}

async fn admit(
    socket: &mut Socket,
    grant_jwt: &str,
    cnf_secret: &[u8; 32],
) -> Result<(), OpenFailure> {
    let jti = unverified_jti(grant_jwt).map_err(OpenFailure::UnverifiedGrant)?;
    socket
        .send(&ClientFrame::Hello(HelloFrame {
            v: 1,
            grant: grant_jwt.to_owned(),
        }))
        .map_err(OpenFailure::Socket)?;
    loop {
        match socket.recv().await.map_err(OpenFailure::Socket)? {
            ServerFrame::Challenge(challenge) => {
                let nonce = URL_SAFE_NO_PAD
                    .decode(challenge.nonce)
                    .map_err(OpenFailure::Challenge)?;
                let nonce: [u8; 32] =
                    nonce
                        .try_into()
                        .map_err(|nonce: Vec<u8>| OpenFailure::ChallengeLength {
                            bytes: nonce.len(),
                        })?;
                socket
                    .send(&ClientFrame::Authenticate(AuthenticateFrame {
                        v: 1,
                        jti: jti.clone(),
                        sig: URL_SAFE_NO_PAD.encode(pop_sign(cnf_secret, &nonce, &jti)),
                    }))
                    .map_err(OpenFailure::Socket)?;
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

struct Conn {
    socket: Socket,
    channel: String,
    noise: NoiseChannel,
    send_seq: u64,
    recv_seq: u64,
    pending: VecDeque<Vec<u8>>,
    reassembly: Reassembly,
    expires_at: i64,
}

enum Inbound {
    Data(Vec<u8>),
    Ack(u64),
}

impl Conn {
    fn usable(&self) -> bool {
        !self.socket.token.is_cancelled() && self.expires_at - now_sec() > REFRESH_MARGIN_S
    }

    async fn handshake(
        &mut self,
        grant_jwt: &str,
        cnf_secret: &[u8; 32],
        forge_pub: &[u8; 32],
    ) -> Result<(), ExchangeError> {
        let mut first = Vec::new();
        self.noise.next_handshake_message(&mut first)?;
        self.send_raw(&first)?;
        let second = self.next_data().await?;
        self.noise.receive_handshake_message(&second)?;
        let mut third = Vec::new();
        self.noise.next_handshake_message(&mut third)?;
        self.send_raw(&third)?;
        let confirm = self
            .noise
            .emit_initiator_confirm(cnf_secret, grant_jwt.as_bytes())?;
        self.send_raw(&confirm)?;
        let reply = self.next_data().await?;
        self.noise.receive_responder_confirm(&reply, forge_pub)?;
        Ok(())
    }

    async fn call(&mut self, request: &TunnelRequest) -> Result<TunnelResponse, ExchangeError> {
        let bytes = serde_json::to_vec(request).map_err(ExchangeError::Encode)?;
        if bytes.len() > artisan_api::TUNNEL_PLAINTEXT_BUDGET {
            for chunk in artisan_api::chunk_bytes(&request.id, &bytes) {
                self.send(&serde_json::to_vec(&chunk).map_err(ExchangeError::Encode)?)
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

    async fn send(&mut self, plaintext: &[u8]) -> Result<(), ExchangeError> {
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

    fn send_raw(&mut self, bytes: &[u8]) -> Result<u64, SocketError> {
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

    async fn next_plaintext(&mut self) -> Result<Vec<u8>, ExchangeError> {
        loop {
            let plaintext = if let Some(plaintext) = self.pending.pop_front() {
                plaintext
            } else {
                let raw = self.next_data().await?;
                self.noise.receive_data(&raw)?
            };
            let chunk = match serde_json::from_slice::<TunnelChunk>(&plaintext) {
                Ok(chunk) => chunk,
                Err(_whole_message) => return Ok(plaintext),
            };
            if let Some(whole) = self
                .reassembly
                .feed(&self.channel, &chunk)
                .map_err(ExchangeError::Reassembly)?
            {
                return Ok(whole);
            }
        }
    }

    async fn next_data(&mut self) -> Result<Vec<u8>, ExchangeError> {
        loop {
            if let Inbound::Data(raw) = self.inbound().await? {
                return Ok(raw);
            }
        }
    }

    async fn inbound(&mut self) -> Result<Inbound, ExchangeError> {
        loop {
            match self.socket.recv().await? {
                ServerFrame::Data(DataFrame {
                    channel, seq, body, ..
                }) => {
                    self.socket
                        .send(&ClientFrame::Ack(AckFrame { v: 1, channel, seq }))?;
                    if seq != self.recv_seq {
                        return Err(ExchangeError::OutOfOrder {
                            expected: self.recv_seq,
                            got: seq,
                        });
                    }
                    self.recv_seq += 1;
                    return Ok(Inbound::Data(
                        URL_SAFE_NO_PAD
                            .decode(body)
                            .map_err(ExchangeError::FrameBody)?,
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

struct Socket {
    out: mpsc::UnboundedSender<String>,
    frames: mpsc::UnboundedReceiver<Result<ServerFrame, SocketError>>,
    token: CancellationToken,
    _pump: AbortOnDropHandle<()>,
}

impl Socket {
    fn send(&self, frame: &ClientFrame) -> Result<(), SocketError> {
        let text = serde_json::to_string(frame).map_err(SocketError::Encode)?;
        self.out.send(text).map_err(|_closed| SocketError::Ended)
    }

    async fn recv(&mut self) -> Result<ServerFrame, SocketError> {
        match self.frames.recv().await.ok_or(SocketError::Ended)?? {
            ServerFrame::ChannelClosed(closed) => Err(SocketError::ChannelClosed {
                reason: closed.reason,
            }),
            ServerFrame::Error(error) => Err(SocketError::Refused {
                code: error.code,
                message: error.message,
            }),
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

async fn connect_socket(url: &str, token: CancellationToken) -> Result<Socket, OpenFailure> {
    let (ws, _response) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|source| OpenFailure::Connect {
            url: url.to_owned(),
            source,
        })?;
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

async fn run_pump(
    ws: WsConnection,
    mut outgoing: mpsc::UnboundedReceiver<String>,
    frames: &mpsc::UnboundedSender<Result<ServerFrame, SocketError>>,
) -> Result<(), SocketError> {
    let (mut sink, mut source) = ws.split();
    loop {
        tokio::select! {
            text = outgoing.recv() => {
                let Some(text) = text else { return Ok(()) };
                sink.send(Ws::Text(text.into())).await.map_err(SocketError::Send)?;
            }
            message = source.next() => {
                match message.ok_or(SocketError::PeerClosed)?.map_err(SocketError::Read)? {
                    Ws::Text(text) => match serde_json::from_str::<ServerFrame>(&text) {
                        Ok(ServerFrame::Ping(ping)) => {
                            let answer = serde_json::to_string(&ClientFrame::Pong(PongFrame { v: 1, t: ping.t }))
                                .map_err(SocketError::Encode)?;
                            sink.send(Ws::Text(answer.into())).await.map_err(SocketError::Send)?;
                        }
                        Ok(ServerFrame::Presence(_)) | Err(_) => {}
                        Ok(frame) => {
                            let terminal = matches!(frame, ServerFrame::ChannelClosed(_) | ServerFrame::Error(_));
                            if frames.send(Ok(frame)).is_err() || terminal {
                                return Ok(());
                            }
                        }
                    },
                    Ws::Ping(data) => sink.send(Ws::Pong(data)).await.map_err(SocketError::Send)?,
                    Ws::Close(_) => return Err(SocketError::PeerClosed),
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

fn read_message(bytes: &[u8]) -> Result<Message, ExchangeError> {
    let mut value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(ExchangeError::Answer)?;
    match value
        .get_mut(artisan_api::STREAM_EVENT_KEY)
        .map(serde_json::Value::take)
    {
        Some(event) => Ok(Message::Event(Box::new(
            serde_json::from_value(event).map_err(ExchangeError::Event)?,
        ))),
        None => Ok(Message::Response(
            serde_json::from_value(value).map_err(ExchangeError::Answer)?,
        )),
    }
}

fn check_tunnel(response: &TunnelResponse) -> Result<(), ApiError> {
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
            .and_then(|code| ErrorCode::deserialize(serde_json::Value::from(code)).ok())
            .unwrap_or(ErrorCode::Internal)
    };
    let message = detail("message").map_or_else(
        || format!("Forge answered {}", response.status),
        str::to_owned,
    );
    Err(ApiError { code, message })
}

fn decode_tunnel_bytes(response: &TunnelResponse) -> Result<Vec<u8>, TunnelBytesError> {
    if let Some(envelope) = response.body.get(artisan_api::BYTES_ENVELOPE_KEY) {
        let bytes =
            artisan_api::TunnelBytes::deserialize(envelope).map_err(TunnelBytesError::Envelope)?;
        return URL_SAFE_NO_PAD
            .decode(bytes.b64u)
            .map_err(TunnelBytesError::Base64);
    }
    if let Some(text) = response.body.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    serde_json::to_vec(&response.body).map_err(TunnelBytesError::Json)
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

fn ws_url(relay_base: &str, account: &str, machine: &str) -> Result<String, OpenFailure> {
    if !is_valid_id(account) || !is_valid_id(machine) {
        return Err(OpenFailure::Route {
            account: account.to_owned(),
            machine: machine.to_owned(),
        });
    }
    let base = relay_base.trim_end_matches('/');
    let base = [("https://", "wss://"), ("http://", "ws://")]
        .into_iter()
        .find_map(|(from, to)| base.strip_prefix(from).map(|rest| format!("{to}{rest}")))
        .unwrap_or_else(|| base.to_owned());
    if !base.starts_with("ws://") && !base.starts_with("wss://") {
        return Err(OpenFailure::RelayUrl {
            url: relay_base.to_owned(),
        });
    }
    Ok(format!("{base}/v1/route/{account}/{machine}"))
}

fn grant_expires_at(grant_jwt: &str) -> Result<i64, OpenFailure> {
    #[derive(Deserialize)]
    struct Expiry {
        exp: i64,
    }
    let Expiry { exp } = unverified_part(grant_jwt, 1).map_err(OpenFailure::UnverifiedGrant)?;
    Ok(exp)
}

fn now_sec() -> i64 {
    artisan_api::now() / 1000
}

fn random<const N: usize>() -> Result<[u8; N], OpenFailure> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(OpenFailure::Random)?;
    Ok(bytes)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test setup; failures fail the test"
)]
#[path = "../tests/account/relay_client.rs"]
mod tests;
