use artisan_account::{ChannelCtx, ClientFrame, NoiseChannel, NoiseError, Phase, ServerFrame};
use futures::{SinkExt as _, StreamExt as _};
use std::collections::HashMap;
use std::sync::Arc;

type WsError = tokio_tungstenite::tungstenite::Error;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("encode the Relay {frame} frame for channel {channel}")]
    Encode {
        channel: String,
        frame: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("send the Relay {frame} frame for channel {channel}")]
    Send {
        channel: String,
        frame: &'static str,
        #[source]
        source: Box<WsError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ServeChannelError {
    #[error("connect to Relay at {url}")]
    Connect {
        url: String,
        #[source]
        source: Box<WsError>,
    },
    #[error("encode the Relay {frame} frame")]
    Encode {
        frame: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("send the Relay {frame} frame")]
    Send {
        frame: &'static str,
        #[source]
        source: Box<WsError>,
    },
    #[error("read a Relay frame")]
    Read(#[source] Box<WsError>),
    #[error("read the Relay challenge nonce")]
    ChallengeNonce(#[source] base64::DecodeError),
    #[error("the Relay challenge nonce is {bytes} bytes, not 32")]
    ChallengeLength { bytes: usize },
    #[error("Relay refused the tunnel: {code} {message}")]
    Refused { code: String, message: String },
    #[error("draw the static key of channel {channel}")]
    StaticKey {
        channel: String,
        #[source]
        source: getrandom::Error,
    },
    #[error(transparent)]
    Noise(#[from] NoiseError),
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("channel {channel} is not open")]
    Unknown { channel: String },
    #[error(transparent)]
    Noise(#[from] NoiseError),
    #[error("the grant on channel {channel} is not text")]
    GrantText {
        channel: String,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("verify the grant on channel {channel}")]
    Grant {
        channel: String,
        #[source]
        source: crate::account_auth::VerifyGrantError,
    },
    #[error("read the cnf key of the grant on channel {channel}")]
    Cnf {
        channel: String,
        #[source]
        source: artisan_account::GrantError,
    },
    #[error("channel {channel} has no verified grant")]
    Unverified { channel: String },
    #[error("the grant on channel {channel} lacks the forge:execute scope")]
    LacksExecuteScope { channel: String },
    #[error("the grant on channel {channel} expired mid-channel")]
    Expired { channel: String },
    #[error(transparent)]
    Reassembly(#[from] artisan_api::ReassemblyError),
    #[error("encode a {what} for channel {channel}")]
    Encode {
        channel: String,
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("the send queue of channel {channel} is full")]
    WindowFull { channel: String },
    #[error("produce the next frame of channel {channel}")]
    NextFrame {
        channel: String,
        #[source]
        source: NextFrameError,
    },
    #[error(transparent)]
    Frame(#[from] FrameError),
}

fn encode_frame(
    channel: &str,
    frame: &'static str,
    value: &ClientFrame,
) -> Result<String, FrameError> {
    serde_json::to_string(value).map_err(|source| FrameError::Encode {
        channel: channel.to_owned(),
        frame,
        source,
    })
}

async fn send_text(
    sink: &mut WsSink,
    channel: &str,
    frame: &'static str,
    text: String,
) -> Result<(), FrameError> {
    sink.send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
        .await
        .map_err(|source| FrameError::Send {
            channel: channel.to_owned(),
            frame,
            source: Box::new(source),
        })
}

async fn send_connection_frame(
    sink: &mut WsSink,
    frame: &'static str,
    value: &ClientFrame,
) -> Result<(), ServeChannelError> {
    let text = serde_json::to_string(value)
        .map_err(|source| ServeChannelError::Encode { frame, source })?;
    sink.send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
        .await
        .map_err(|source| ServeChannelError::Send {
            frame,
            source: Box::new(source),
        })
}

struct Channel {
    instance: u64,
    noise: NoiseChannel,
    grant: Grant,
    account: String,
    machine: String,
    next_receive: u64,
    window: SendWindow,
    stream: Option<Stream>,
    requests: tokio::task::JoinSet<()>,
}

enum Grant {
    Awaiting,
    Verifying(artisan_account::InitiatorConfirm),
    Verified(artisan_account::GrantClaims),
}

impl Grant {
    const fn claims(&self) -> Option<&artisan_account::GrantClaims> {
        match self {
            Self::Verified(claims) => Some(claims),
            Self::Awaiting | Self::Verifying(_) => None,
        }
    }
}

struct Admission {
    channel: String,
    instance: u64,
    verified: Result<artisan_account::GrantClaims, ChannelError>,
}

struct Charged {
    bytes: Vec<u8>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

enum StreamMsg {
    Frame(Charged),
    End { reason: &'static str },
    Expired,
}

const QUEUED_BYTES: usize = 64 * 1024 * 1024;
const STREAM_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);
const CHANNEL_REQUESTS: usize = 16;

#[derive(Clone)]
struct Outbound {
    tx: tokio::sync::mpsc::Sender<(String, StreamMsg)>,
    budget: Arc<tokio::sync::Semaphore>,
}

impl Outbound {
    async fn charge(&self, bytes: Vec<u8>) -> Charged {
        let charged = u32::try_from(bytes.len().min(QUEUED_BYTES))
            .expect("invariant: the tunnel's byte budget fits a semaphore");
        let permit = Arc::clone(&self.budget)
            .acquire_many_owned(charged)
            .await
            .expect("invariant: the tunnel's byte budget is never closed");
        Charged {
            bytes,
            _permit: permit,
        }
    }

    async fn send(&self, name: &str, bytes: Vec<u8>) -> bool {
        let charged = self.charge(bytes).await;
        self.tx
            .send((name.to_owned(), StreamMsg::Frame(charged)))
            .await
            .is_ok()
    }
}

struct TunnelSink {
    name: String,
    outbound: Outbound,
}

impl TunnelSink {
    async fn beat(&self) {
        let heartbeat = serde_json::to_vec(&serde_json::json!({"$event": {"stream": self.name}}))
            .expect("invariant: a stream heartbeat encodes");
        let mut ticks = tokio::time::interval(STREAM_HEARTBEAT);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            if !self.outbound.send(&self.name, heartbeat.clone()).await {
                return;
            }
        }
    }
}

impl crate::events::EnvelopeSink for TunnelSink {
    async fn send(&self, envelope: artisan_api::Envelope) -> Result<(), crate::events::SinkError> {
        let seq = envelope.seq;
        let frame = |envelope: &artisan_api::Envelope| {
            serde_json::to_vec(&serde_json::json!({
                "$event": {"stream": self.name, "envelope": envelope},
            }))
            .map_err(|source| crate::events::SinkError::Encode { seq, source })
        };
        let mut bytes = frame(&envelope)?;
        if bytes.len() > QUEUED_BYTES {
            bytes = frame(&artisan_api::Envelope {
                seq: None,
                live_revision: 0,
                event: artisan_api::Event::Reset,
            })?;
        }
        if self.outbound.send(&self.name, bytes).await {
            Ok(())
        } else {
            Err(crate::events::SinkError::Gone)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ChannelAuthorization {
    Unverified,
    LacksExecuteScope,
    Expired,
    Granted,
}

fn channel_authorization(
    claims: Option<&artisan_account::GrantClaims>,
    now: i64,
) -> ChannelAuthorization {
    match claims {
        None => ChannelAuthorization::Unverified,
        Some(claims) if !claims.scope.contains(&artisan_account::Scope::ForgeExecute) => {
            ChannelAuthorization::LacksExecuteScope
        }
        Some(claims) if claims.exp <= now => ChannelAuthorization::Expired,
        Some(_) => ChannelAuthorization::Granted,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Outgoing {
    Deliver,
    Close { reason: &'static str },
}

fn outgoing(
    name: &str,
    msg: &StreamMsg,
    channels: &HashMap<String, Channel>,
    now: i64,
) -> Outgoing {
    let Some(channel) = channels.get(name) else {
        return Outgoing::Deliver;
    };
    if matches!(msg, StreamMsg::Expired) {
        return Outgoing::Close {
            reason: "grant-expired",
        };
    }
    match channel_authorization(channel.grant.claims(), now) {
        ChannelAuthorization::Granted => Outgoing::Deliver,
        ChannelAuthorization::Expired => Outgoing::Close {
            reason: "grant-expired",
        },
        ChannelAuthorization::Unverified | ChannelAuthorization::LacksExecuteScope => {
            Outgoing::Close {
                reason: "unauthorized",
            }
        }
    }
}

enum Pending {
    Body(String),
    Sealed(Vec<u8>),
    Transfer {
        id: String,
        bytes: Vec<u8>,
        next: usize,
    },
}

impl Pending {
    const fn queued_bytes(&self) -> usize {
        match self {
            Self::Body(body) => body.len(),
            Self::Sealed(bytes) | Self::Transfer { bytes, .. } => bytes.len(),
        }
    }
}

struct SendWindow {
    sent_high: Option<u64>,
    acked_high: Option<u64>,
    sizes: std::collections::VecDeque<usize>,
    unacked_bytes: u64,
    queue: std::collections::VecDeque<Pending>,
}

const SEND_WINDOW_FRAMES: u64 = 64;
const SEND_WINDOW_BYTES: u64 = 1024 * 1024;
const SEND_QUEUE_BYTES: u64 = artisan_api::REASSEMBLY_MAX_BYTES_PER_CONN;

#[derive(Debug, thiserror::Error)]
pub enum NextFrameError {
    #[error(transparent)]
    Noise(#[from] NoiseError),
    #[error("encode tunnel chunk {index} of transfer {transfer}")]
    Chunk {
        transfer: String,
        index: usize,
        #[source]
        source: serde_json::Error,
    },
}

impl SendWindow {
    const fn new() -> Self {
        Self {
            sent_high: None,
            acked_high: None,
            sizes: std::collections::VecDeque::new(),
            unacked_bytes: 0,
            queue: std::collections::VecDeque::new(),
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
        self.unacked_frames() < SEND_WINDOW_FRAMES
            && self.unacked_bytes.saturating_add(body_len as u64) <= SEND_WINDOW_BYTES
    }

    fn queued_bytes(&self) -> u64 {
        self.queue
            .iter()
            .map(|pending| pending.queued_bytes() as u64)
            .sum()
    }

    fn enqueue(&mut self, pending: Pending) -> bool {
        if self
            .queued_bytes()
            .saturating_add(pending.queued_bytes() as u64)
            > SEND_QUEUE_BYTES
        {
            return false;
        }
        self.queue.push_back(pending);
        true
    }

    fn next_frame(
        &mut self,
        mut seal: impl FnMut(&[u8]) -> Result<Vec<u8>, NoiseError>,
    ) -> Result<Option<(u64, String)>, NextFrameError> {
        let Some(pending) = self.queue.pop_front() else {
            return Ok(None);
        };
        let body = match pending {
            Pending::Body(body) => body,
            Pending::Sealed(plaintext) => b64u_encode(&seal(&plaintext)?),
            Pending::Transfer { id, bytes, next } => {
                let chunk = artisan_api::chunk_at(&id, &bytes, next);
                let plaintext =
                    serde_json::to_vec(&chunk).map_err(|source| NextFrameError::Chunk {
                        transfer: id.clone(),
                        index: next,
                        source,
                    })?;
                if next + 1 < artisan_api::chunk_count(bytes.len()) {
                    self.queue.push_front(Pending::Transfer {
                        id,
                        bytes,
                        next: next + 1,
                    });
                }
                b64u_encode(&seal(&plaintext)?)
            }
        };
        if !self.has_space(body.len()) {
            self.queue.push_front(Pending::Body(body));
            return Ok(None);
        }
        let seq = self.sent_high.map_or(0, |sent| sent.saturating_add(1));
        self.noted_sent(seq, body.len());
        Ok(Some((seq, body)))
    }

    fn noted_sent(&mut self, seq: u64, body_len: usize) {
        self.sent_high = Some(seq);
        self.sizes.push_back(body_len);
        self.unacked_bytes = self.unacked_bytes.saturating_add(body_len as u64);
    }

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
}

use artisan_api::Reassembly;

struct Stream {
    _pump: tokio_util::task::AbortOnDropHandle<()>,
}

type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Message,
>;

use crate::core::now_sec;

fn b64u_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn b64u_decode(text: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text.as_bytes())
}

struct Connection {
    channels: HashMap<String, Channel>,
    reassembly: Reassembly,
    admissions: tokio::task::JoinSet<Admission>,
    outbound: Outbound,
    verifier: Arc<crate::account_auth::GrantVerifier>,
    forge_cnf_secret: [u8; 32],
    account: String,
    machine: String,
    app: crate::routes::App,
}

impl Connection {
    fn forget(&mut self, name: &str) {
        self.channels.remove(name);
        self.reassembly.drop_channel(name);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one connection turn needs all of these; callers pass a config struct next"
)]
pub async fn serve_channel(
    ws_url: String,
    grant_jwt: String,
    grant_jti: String,
    cnf_secret: [u8; 32],
    forge_cnf_secret: [u8; 32],
    account: String,
    machine: String,
    verifier: Arc<crate::account_auth::GrantVerifier>,
    app: crate::routes::App,
) -> Result<(), ServeChannelError> {
    use tokio_tungstenite::tungstenite::Message as Ws;
    let (stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|source| ServeChannelError::Connect {
            url: ws_url.clone(),
            source: Box::new(source),
        })?;
    let (mut sink, mut source) = stream.split();
    send_connection_frame(
        &mut sink,
        "hello",
        &ClientFrame::Hello(artisan_account::HelloFrame {
            v: 1,
            grant: grant_jwt,
        }),
    )
    .await?;
    let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel::<(String, StreamMsg)>(256);
    let mut connection = Connection {
        channels: HashMap::new(),
        reassembly: Reassembly::new(),
        admissions: tokio::task::JoinSet::new(),
        outbound: Outbound {
            tx: stream_tx,
            budget: Arc::new(tokio::sync::Semaphore::new(QUEUED_BYTES)),
        },
        verifier,
        forge_cnf_secret,
        account,
        machine,
        app,
    };

    loop {
        tokio::select! {
            message = source.next() => {
                let Some(message) = message else { break };
                let message = message.map_err(|source| ServeChannelError::Read(Box::new(source)))?;
                let text = match message {
                    Ws::Text(text) => text.to_string(),
                    Ws::Close(_) => break,
                    Ws::Ping(data) => {
                        sink.send(Ws::Pong(data)).await.map_err(|source| ServeChannelError::Send {
                            frame: "pong",
                            source: Box::new(source),
                        })?;
                        continue;
                    }
                    Ws::Binary(_) | Ws::Pong(_) | Ws::Frame(_) => continue,
                };
                handle_server_text(&text, &mut sink, &grant_jti, &cnf_secret, &mut connection).await?;
            }
            Some((name, msg)) = stream_rx.recv() => {
                if let Outgoing::Close { reason } = outgoing(&name, &msg, &connection.channels, now_sec()) {
                    close_channel(&name, reason, &mut connection, &mut sink).await?;
                } else if let Err(error) = handle_stream_msg(&name, msg, &mut connection.channels, &mut sink).await {
                    tracing::debug!(channel = %name, error = diagnostics::chain(&error), "Relay channel failed");
                    close_channel(&name, "channel-failed", &mut connection, &mut sink).await?;
                }
            }
            Some(admitted) = connection.admissions.join_next(), if !connection.admissions.is_empty() => {
                let admission = admitted.expect("invariant: grant verification never panics and is only aborted with its connection");
                let name = admission.channel.clone();
                if let Err(error) = admit(admission, &mut connection, &mut sink).await {
                    tracing::debug!(channel = %name, error = diagnostics::chain(&error), "Relay channel failed");
                    close_channel(&name, "channel-failed", &mut connection, &mut sink).await?;
                }
            }
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "one Relay frame dispatch with all cases"
)]
async fn handle_server_text(
    text: &str,
    sink: &mut WsSink,
    grant_jti: &str,
    cnf_secret: &[u8; 32],
    connection: &mut Connection,
) -> Result<(), ServeChannelError> {
    let frame = match serde_json::from_str(text).and_then(artisan_account::read_server_frame) {
        Ok(frame) => frame,
        Err(error) => {
            tracing::debug!(error = %error, "skipped a Relay frame");
            return Ok(());
        }
    };
    match frame {
        ServerFrame::Challenge(challenge) => {
            use base64::Engine as _;
            let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(challenge.nonce.as_bytes())
                .map_err(ServeChannelError::ChallengeNonce)?;
            let nonce = <[u8; 32]>::try_from(nonce.as_slice())
                .map_err(|_length| ServeChannelError::ChallengeLength { bytes: nonce.len() })?;
            let signature = artisan_account::pop_sign(cnf_secret, &nonce, grant_jti);
            send_connection_frame(
                sink,
                "authenticate",
                &ClientFrame::Authenticate(artisan_account::AuthenticateFrame {
                    v: 1,
                    jti: grant_jti.to_owned(),
                    sig: b64u_encode(&signature),
                }),
            )
            .await?;
        }
        ServerFrame::Ready(_) | ServerFrame::Presence(_) => {}
        ServerFrame::Ping(ping) => {
            send_connection_frame(
                sink,
                "pong",
                &ClientFrame::Pong(artisan_account::PongFrame { v: 1, t: ping.t }),
            )
            .await?;
        }
        ServerFrame::Error(error) => {
            return Err(ServeChannelError::Refused {
                code: error.code,
                message: error.message,
            });
        }
        ServerFrame::ChannelOpened(opened) => {
            let name = opened.channel;
            let ctx = ChannelCtx {
                account: connection.account.clone(),
                machine: connection.machine.clone(),
                channel: name.clone(),
            };
            let static_secret = fresh_static(&name)?;
            let noise = NoiseChannel::responder(&static_secret, None, ctx)?;
            connection.reassembly.drop_channel(&name);
            connection.channels.insert(
                name,
                Channel {
                    instance: next_channel_instance(),
                    noise,
                    grant: Grant::Awaiting,
                    account: connection.account.clone(),
                    machine: connection.machine.clone(),
                    next_receive: 0,
                    window: SendWindow::new(),
                    stream: None,
                    requests: tokio::task::JoinSet::new(),
                },
            );
        }
        ServerFrame::Data(data) => {
            let name = data.channel.clone();
            send_ack(&name, data.seq, sink).await?;
            let Some(channel) = connection.channels.get_mut(&name) else {
                close_channel(&name, "unknown-channel", connection, sink).await?;
                return Ok(());
            };
            if data.seq != channel.next_receive {
                close_channel(&name, "out-of-order", connection, sink).await?;
                return Ok(());
            }
            channel.next_receive = channel.next_receive.saturating_add(1);
            let raw = match b64u_decode(&data.body) {
                Ok(raw) => raw,
                Err(error) => {
                    tracing::debug!(channel = %name, error = %error, "Relay data is not base64url");
                    close_channel(&name, "bad-body", connection, sink).await?;
                    return Ok(());
                }
            };
            if let Err(error) = handle_channel_bytes(&name, &raw, connection, sink).await {
                tracing::debug!(channel = %name, error = diagnostics::chain(&error), "Relay channel failed");
                close_channel(&name, "channel-failed", connection, sink).await?;
            }
        }
        ServerFrame::ChannelClosed(closed) => connection.forget(&closed.channel),
        ServerFrame::Ack(ack) => {
            if let Some(channel) = connection.channels.get_mut(&ack.channel) {
                channel.window.on_ack(ack.seq);
                if let Err(error) = pump(&ack.channel, channel, sink).await {
                    tracing::debug!(channel = %ack.channel, error = diagnostics::chain(&error), "Relay channel failed");
                    close_channel(&ack.channel, "channel-failed", connection, sink).await?;
                }
            }
        }
    }
    Ok(())
}

fn next_channel_instance() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static INSTANCES: AtomicU64 = AtomicU64::new(1);
    INSTANCES.fetch_add(1, Ordering::Relaxed)
}

async fn close_channel(
    name: &str,
    reason: &str,
    connection: &mut Connection,
    sink: &mut WsSink,
) -> Result<(), FrameError> {
    connection.forget(name);
    let close = encode_frame(
        name,
        "close",
        &ClientFrame::Close(artisan_account::CloseFrame {
            v: 1,
            channel: name.to_owned(),
            reason: Some(reason.to_owned()),
        }),
    )?;
    send_text(sink, name, "close", close).await
}

fn fresh_static(channel: &str) -> Result<[u8; 32], ServeChannelError> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|source| ServeChannelError::StaticKey {
        channel: channel.to_owned(),
        source,
    })?;
    Ok(secret)
}

async fn handle_channel_bytes(
    name: &str,
    raw: &[u8],
    connection: &mut Connection,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    let Some(channel) = connection.channels.get_mut(name) else {
        return Err(ChannelError::Unknown {
            channel: name.to_owned(),
        });
    };
    let channel_name = || name.to_owned();
    if channel.noise.phase() == Phase::Handshake {
        channel.noise.receive_handshake_message(raw)?;
        let mut reply = Vec::new();
        if channel.noise.next_handshake_message(&mut reply).is_ok() && !reply.is_empty() {
            enqueue(name, channel, Pending::Body(b64u_encode(&reply)), sink).await?;
        }
        return Ok(());
    }
    match &channel.grant {
        Grant::Awaiting => {
            let confirm = channel.noise.receive_initiator_confirm(raw)?;
            let jwt = String::from_utf8(confirm.jwt.clone()).map_err(|source| {
                ChannelError::GrantText {
                    channel: channel_name(),
                    source: source.utf8_error(),
                }
            })?;
            channel.grant = Grant::Verifying(confirm);
            let grants = Arc::clone(&connection.verifier);
            let (account, machine) = (channel.account.clone(), channel.machine.clone());
            let instance = channel.instance;
            let channel = channel_name();
            connection.admissions.spawn(async move {
                let verified = grants
                    .verify_channel_grant(&jwt, &account, &machine, now_sec(), false)
                    .await
                    .map_err(|source| ChannelError::Grant {
                        channel: channel.clone(),
                        source,
                    });
                Admission {
                    channel,
                    instance,
                    verified,
                }
            });
            return Ok(());
        }
        Grant::Verifying(_) => {
            return Err(ChannelError::Unverified {
                channel: channel_name(),
            });
        }
        Grant::Verified(_) => {}
    }
    let plaintext = channel.noise.receive_data(raw)?;
    match channel_authorization(channel.grant.claims(), now_sec()) {
        ChannelAuthorization::Unverified => {
            return Err(ChannelError::Unverified {
                channel: channel_name(),
            });
        }
        ChannelAuthorization::LacksExecuteScope => {
            return Err(ChannelError::LacksExecuteScope {
                channel: channel_name(),
            });
        }
        ChannelAuthorization::Expired => {
            return Err(ChannelError::Expired {
                channel: channel_name(),
            });
        }
        ChannelAuthorization::Granted => {}
    }
    if let Ok(chunk) = serde_json::from_slice::<artisan_api::TunnelChunk>(&plaintext) {
        if let Some(reassembled) = connection.reassembly.feed(name, &chunk)? {
            dispatch_app_bytes(name, &reassembled, connection, sink).await?;
        }
        return Ok(());
    }
    dispatch_app_bytes(name, &plaintext, connection, sink).await
}

async fn admit(
    admission: Admission,
    connection: &mut Connection,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    let name = admission.channel;
    let Some(channel) = connection
        .channels
        .get_mut(&name)
        .filter(|channel| channel.instance == admission.instance)
    else {
        return Ok(());
    };
    let claims = admission.verified?;
    let Grant::Verifying(confirm) = std::mem::replace(&mut channel.grant, Grant::Awaiting) else {
        unreachable!("invariant: channel {name} verifies its grant once, in Verifying");
    };
    let cnf = artisan_account::cnf_public_key(&claims).map_err(|source| ChannelError::Cnf {
        channel: name.clone(),
        source,
    })?;
    channel.noise.accept_initiator_confirm(&cnf, &confirm)?;
    channel.grant = Grant::Verified(claims);
    let reply = channel
        .noise
        .emit_responder_confirm(&connection.forge_cnf_secret)?;
    enqueue(&name, channel, Pending::Body(b64u_encode(&reply)), sink).await
}

async fn dispatch_app_bytes(
    name: &str,
    plaintext: &[u8],
    connection: &mut Connection,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    let request: Result<artisan_api::TunnelRequest, _> = serde_json::from_slice(plaintext);
    let Ok(request) = request else {
        let response =
            artisan_api::TunnelResponse::error("?", 400, "tunneled call is not a request");
        send_response(name, &response, &mut connection.channels, sink).await?;
        return Ok(());
    };
    match request.method.as_str() {
        artisan_api::METHOD_SUBSCRIBE => {
            subscribe(name, &request, connection, sink).await?;
        }
        artisan_api::METHOD_UNSUBSCRIBE => {
            let existed = connection
                .channels
                .get_mut(name)
                .and_then(|channel| channel.stream.take())
                .is_some();
            let response = artisan_api::TunnelResponse::ok(
                &request.id,
                serde_json::json!({"unsubscribed": existed}),
            );
            send_response(name, &response, &mut connection.channels, sink).await?;
        }
        _ => {
            let Some(claims) = connection
                .channels
                .get(name)
                .and_then(|channel| channel.grant.claims().cloned())
            else {
                let response = artisan_api::TunnelResponse::error(
                    &request.id,
                    401,
                    "no verified grant on this channel",
                );
                send_response(name, &response, &mut connection.channels, sink).await?;
                return Ok(());
            };
            let channel = connection
                .channels
                .get_mut(name)
                .expect("invariant: a channel with a verified grant is open");
            let router = crate::routes::router(connection.app.clone());
            let outbound = connection.outbound.clone();
            let id = request.id.clone();
            let owner = name.to_owned();
            let started = start_request(channel, async move {
                let name = owner;
                let response = dispatch_request(&request, &router, &claims).await;
                let bytes = serde_json::to_vec(&response)
                    .expect("invariant: a tunneled response is plain JSON");
                if !outbound.send(&name, bytes).await {
                    tracing::debug!(channel = %name, request = %request.id, "the Relay connection closed before the response");
                }
            });
            if !started {
                let response = artisan_api::TunnelResponse::error(
                    &id,
                    429,
                    "too many requests in flight on this channel; wait for one to finish",
                );
                send_response(name, &response, &mut connection.channels, sink).await?;
            }
        }
    }
    Ok(())
}

fn start_request(
    channel: &mut Channel,
    request: impl Future<Output = ()> + Send + 'static,
) -> bool {
    while channel.requests.try_join_next().is_some() {}
    if channel.requests.len() >= CHANNEL_REQUESTS {
        return false;
    }
    channel.requests.spawn(request);
    true
}

async fn subscribe(
    name: &str,
    request: &artisan_api::TunnelRequest,
    connection: &mut Connection,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    let channels = &mut connection.channels;
    let Some(since) = crate::events::Since::from_events_path(&request.path) else {
        let response = artisan_api::TunnelResponse::error(&request.id, 400, "bad subscribe path");
        send_response(name, &response, channels, sink).await?;
        return Ok(());
    };
    if channels
        .get(name)
        .is_some_and(|channel| channel.stream.is_some())
    {
        let response = artisan_api::TunnelResponse::error(
            &request.id,
            409,
            "one stream per channel; open another channel",
        );
        send_response(name, &response, channels, sink).await?;
        return Ok(());
    }
    let Some(expires_at) = channels
        .get(name)
        .and_then(|channel| channel.grant.claims())
        .map(|claims| claims.exp)
    else {
        let response = artisan_api::TunnelResponse::error(
            &request.id,
            401,
            "no verified grant on this channel",
        );
        send_response(name, &response, channels, sink).await?;
        return Ok(());
    };
    let expiry = tokio::time::Instant::now()
        + std::time::Duration::from_secs(
            expires_at.saturating_sub(now_sec()).max(0).cast_unsigned(),
        );
    let stream_id = name.to_owned();
    let pump_app = connection.app.clone();
    let response = artisan_api::TunnelResponse::ok(
        &request.id,
        serde_json::json!({"$stream": {"stream": stream_id}}),
    );
    send_response(name, &response, channels, sink).await?;
    let sink = TunnelSink {
        name: stream_id.clone(),
        outbound: connection.outbound.clone(),
    };
    let tx = connection.outbound.tx.clone();
    let pump = tokio::spawn(until_expiry(stream_id, tx, expiry, async move {
        tokio::select! {
            forwarded = crate::events::forward(&pump_app, &since, &sink) => {
                if let Err(error) = forwarded {
                    tracing::debug!(channel = %sink.name, error = diagnostics::chain(&error), "tunneled event stream ended");
                }
            }
            () = sink.beat() => {}
        }
    }));
    let channel = channels
        .get_mut(name)
        .expect("invariant: a channel answering its subscribe request is still open");
    channel.stream = Some(Stream {
        _pump: tokio_util::task::AbortOnDropHandle::new(pump),
    });
    Ok(())
}

async fn until_expiry(
    name: String,
    tx: tokio::sync::mpsc::Sender<(String, StreamMsg)>,
    expiry: tokio::time::Instant,
    stream: impl Future<Output = ()>,
) {
    let end = tokio::select! {
        () = tokio::time::sleep_until(expiry) => StreamMsg::Expired,
        () = stream => StreamMsg::End { reason: "closed" },
    };
    if tx.send((name.clone(), end)).await.is_err() {
        tracing::debug!(channel = %name, "the Relay connection closed before the stream's end");
    }
}

fn next_transfer_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static IDS: AtomicU64 = AtomicU64::new(1);
    IDS.fetch_add(1, Ordering::Relaxed)
}

async fn handle_stream_msg(
    name: &str,
    msg: StreamMsg,
    channels: &mut HashMap<String, Channel>,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    let Some(channel) = channels.get_mut(name) else {
        return Ok(());
    };
    let encoded = |what| {
        move |source| ChannelError::Encode {
            channel: name.to_owned(),
            what,
            source,
        }
    };
    let bytes = match msg {
        StreamMsg::Frame(Charged { bytes, .. }) => bytes,
        StreamMsg::End { reason } => {
            channel.stream = None;
            serde_json::to_vec(&serde_json::json!({"$event": {"stream": name, "end": reason}}))
                .map_err(encoded("stream end"))?
        }
        StreamMsg::Expired => {
            unreachable!("invariant: channel {name} closes on expiry before delivery")
        }
    };
    send_payload(name, bytes, channel, sink).await
}

async fn send_response(
    name: &str,
    response: &artisan_api::TunnelResponse,
    channels: &mut HashMap<String, Channel>,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    let bytes = serde_json::to_vec(response).map_err(|source| ChannelError::Encode {
        channel: name.to_owned(),
        what: "tunneled response",
        source,
    })?;
    let Some(channel) = channels.get_mut(name) else {
        return Err(ChannelError::Unknown {
            channel: name.to_owned(),
        });
    };
    send_payload(name, bytes, channel, sink).await
}

async fn send_payload(
    name: &str,
    bytes: Vec<u8>,
    channel: &mut Channel,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    if channel.noise.phase() != Phase::Open {
        return Ok(());
    }
    let pending = if bytes.len() <= artisan_api::TUNNEL_PLAINTEXT_BUDGET {
        Pending::Sealed(bytes)
    } else {
        Pending::Transfer {
            id: format!("{name}:{}", next_transfer_id()),
            bytes,
            next: 0,
        }
    };
    enqueue(name, channel, pending, sink).await
}

async fn enqueue(
    name: &str,
    channel: &mut Channel,
    pending: Pending,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    if !channel.window.enqueue(pending) {
        return Err(ChannelError::WindowFull {
            channel: name.to_owned(),
        });
    }
    pump(name, channel, sink).await
}

async fn pump(name: &str, channel: &mut Channel, sink: &mut WsSink) -> Result<(), ChannelError> {
    let noise = &mut channel.noise;
    while let Some((seq, body)) = channel
        .window
        .next_frame(|plaintext| noise.seal(plaintext))
        .map_err(|source| ChannelError::NextFrame {
            channel: name.to_owned(),
            source,
        })?
    {
        send_frame(name, seq, &body, sink).await?;
    }
    Ok(())
}

async fn send_frame(name: &str, seq: u64, body: &str, sink: &mut WsSink) -> Result<(), FrameError> {
    let frame = artisan_account::DataFrame {
        v: 1,
        channel: name.to_owned(),
        seq,
        body: body.to_owned(),
    };
    let text = encode_frame(name, "data", &ClientFrame::Data(frame))?;
    send_text(sink, name, "data", text).await
}

async fn send_ack(name: &str, seq: u64, sink: &mut WsSink) -> Result<(), FrameError> {
    let frame = artisan_account::AckFrame {
        v: 1,
        channel: name.to_owned(),
        seq,
    };
    let text = encode_frame(name, "ack", &ClientFrame::Ack(frame))?;
    send_text(sink, name, "ack", text).await
}

async fn dispatch_request(
    request: &artisan_api::TunnelRequest,
    app: &axum::Router,
    claims: &artisan_account::GrantClaims,
) -> artisan_api::TunnelResponse {
    use tower::ServiceExt as _;
    let method = match request.method.as_str() {
        "GET" => axum::http::Method::GET,
        "POST" => axum::http::Method::POST,
        "PUT" => axum::http::Method::PUT,
        "PATCH" => axum::http::Method::PATCH,
        "DELETE" => axum::http::Method::DELETE,
        _ => return artisan_api::TunnelResponse::error(&request.id, 400, "bad tunneled method"),
    };
    if request.path == "/events" || request.path.starts_with("/events?") {
        return artisan_api::TunnelResponse::error(
            &request.id,
            400,
            "subscribe to /events with SUBSCRIBE on its own channel",
        );
    }
    let Ok((content_type, bytes)) = artisan_api::request_body_bytes(request.body.as_ref()) else {
        return artisan_api::TunnelResponse::error(&request.id, 400, "bad tunneled body");
    };
    let http_request = axum::http::Request::builder()
        .method(method)
        .uri(&request.path)
        .header("content-type", content_type)
        .body(axum::body::Body::from(bytes));
    let Ok(mut http_request) = http_request else {
        return artisan_api::TunnelResponse::error(&request.id, 400, "bad tunneled call");
    };
    http_request
        .extensions_mut()
        .insert(crate::routes::TunnelAuth {
            claims: claims.clone(),
        });
    let Ok(response) = app.clone().oneshot(http_request).await;
    let status = response.status().as_u16();
    let mime = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes = match axum::body::to_bytes(response.into_body(), artisan_api::TUNNEL_BODY_MAX_BYTES)
        .await
    {
        Ok(bytes) => bytes,
        Err(error) => {
            return artisan_api::TunnelResponse::error(
                &request.id,
                502,
                &format!("read the Forge response: {error}"),
            );
        }
    };
    if let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && (mime.starts_with("application/json") || body.is_object() || body.is_array())
    {
        return artisan_api::TunnelResponse {
            id: request.id.clone(),
            status,
            body,
            content_type: None,
        };
    }
    artisan_api::TunnelResponse {
        id: request.id.clone(),
        status,
        body: serde_json::json!({"$bytes": {
            "content_type": mime,
            "b64u": b64u_encode(&bytes),
        }}),
        content_type: Some(mime),
    }
}

#[cfg(test)]
#[path = "../tests/relay.rs"]
mod relay_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test fixtures; failures fail the test")]
#[path = "../tests/relay/unit.rs"]
mod tests;
