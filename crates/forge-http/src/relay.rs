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
    #[error("the send window and park queue of channel {channel} are full")]
    WindowFull { channel: String },
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
    noise: NoiseChannel,
    claims: Option<artisan_account::GrantClaims>,
    account: String,
    machine: String,
}

enum StreamMsg {
    Frame(Vec<u8>),
    End { reason: &'static str },
    Expired,
    Response(artisan_api::TunnelResponse),
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
    match channel_authorization(channel.claims.as_ref(), now) {
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

struct SendWindow {
    sent_high: Option<u64>,
    acked_high: Option<u64>,
    sizes: std::collections::VecDeque<usize>,
    unacked_bytes: u64,
    parked: std::collections::VecDeque<String>,
}

const SEND_WINDOW_FRAMES: u64 = 64;
const SEND_WINDOW_BYTES: u64 = 1024 * 1024;
const SEND_PARKED_MAX: usize = 256;

impl SendWindow {
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
        self.unacked_frames() < SEND_WINDOW_FRAMES
            && self.unacked_bytes.saturating_add(body_len as u64) <= SEND_WINDOW_BYTES
    }

    fn should_park(&self, body_len: usize) -> bool {
        !self.parked.is_empty() || !self.has_space(body_len)
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
    _forwarder: tokio_util::task::AbortOnDropHandle<()>,
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
    let mut channels: HashMap<String, Channel> = HashMap::new();
    let mut streams: HashMap<String, Stream> = HashMap::new();
    let mut reassembly = Reassembly::new();
    let mut windows: HashMap<String, SendWindow> = HashMap::new();
    let mut relay_recv_seq: HashMap<String, u64> = HashMap::new();
    let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel::<(String, StreamMsg)>(256);

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
                handle_server_text(
                    &text,
                    &mut sink,
                    &grant_jti,
                    &cnf_secret,
                    &mut channels,
                    &mut streams,
                    &mut reassembly,
                    &forge_cnf_secret,
                    &verifier,
                    &app,
                    &account,
                    &machine,
                    &mut windows,
                    &mut relay_recv_seq,
                    &stream_tx,
                )
                .await?;
            }
            Some((name, msg)) = stream_rx.recv() => {
                if let Outgoing::Close { reason } = outgoing(&name, &msg, &channels, now_sec()) {
                    close_channel(&name, reason, &mut channels, &mut streams, &mut reassembly, &mut windows, &mut sink).await?;
                } else if let Err(error) = handle_stream_msg(&name, msg, &mut channels, &mut streams, &mut windows, &mut sink).await {
                    tracing::debug!(channel = %name, error = diagnostics::chain(&error), "Relay channel failed");
                    close_channel(&name, "channel-failed", &mut channels, &mut streams, &mut reassembly, &mut windows, &mut sink).await?;
                }
            }
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "one Relay turn needs all of these; see serve_channel"
)]
#[expect(
    clippy::too_many_lines,
    reason = "one Relay frame dispatch with all cases"
)]
async fn handle_server_text(
    text: &str,
    sink: &mut WsSink,
    grant_jti: &str,
    cnf_secret: &[u8; 32],
    channels: &mut HashMap<String, Channel>,
    streams: &mut HashMap<String, Stream>,
    reassembly: &mut Reassembly,
    forge_cnf_secret: &[u8; 32],
    verifier: &Arc<crate::account_auth::GrantVerifier>,
    app: &crate::routes::App,
    account: &str,
    machine: &str,
    windows: &mut HashMap<String, SendWindow>,
    relay_recv_seq: &mut HashMap<String, u64>,
    stream_tx: &tokio::sync::mpsc::Sender<(String, StreamMsg)>,
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
                account: account.to_owned(),
                machine: machine.to_owned(),
                channel: name.clone(),
            };
            let static_secret = fresh_static(&name)?;
            let noise = NoiseChannel::responder(&static_secret, None, ctx)?;
            channels.insert(
                name.clone(),
                Channel {
                    noise,
                    claims: None,
                    account: account.to_owned(),
                    machine: machine.to_owned(),
                },
            );
            windows.insert(name.clone(), SendWindow::new());
            relay_recv_seq.insert(name, 0);
        }
        ServerFrame::Data(data) => {
            let name = data.channel.clone();
            send_ack(&name, data.seq, sink).await?;
            let expected = relay_recv_seq.get(&name).copied().unwrap_or(0);
            if data.seq != expected {
                close_channel(
                    &name,
                    "out-of-order",
                    channels,
                    streams,
                    reassembly,
                    windows,
                    sink,
                )
                .await?;
                return Ok(());
            }
            relay_recv_seq.insert(name.clone(), expected.saturating_add(1));
            let raw = match b64u_decode(&data.body) {
                Ok(raw) => raw,
                Err(error) => {
                    tracing::debug!(channel = %name, error = %error, "Relay data is not base64url");
                    close_channel(
                        &name, "bad-body", channels, streams, reassembly, windows, sink,
                    )
                    .await?;
                    return Ok(());
                }
            };
            let outcome = handle_channel_bytes(
                &name,
                &raw,
                channels,
                streams,
                reassembly,
                forge_cnf_secret,
                verifier,
                app,
                windows,
                sink,
                stream_tx,
            )
            .await;
            if let Err(error) = outcome {
                tracing::debug!(channel = %name, error = diagnostics::chain(&error), "Relay channel failed");
                close_channel(
                    &name,
                    "channel-failed",
                    channels,
                    streams,
                    reassembly,
                    windows,
                    sink,
                )
                .await?;
            }
        }
        ServerFrame::ChannelClosed(closed) => {
            channels.remove(&closed.channel);
            streams.remove(&closed.channel);
            reassembly.drop_channel(&closed.channel);
            windows.remove(&closed.channel);
        }
        ServerFrame::Ack(ack) => {
            if let Some(window) = windows.get_mut(&ack.channel) {
                window.on_ack(ack.seq);
                drain_parked(&ack.channel, windows, sink).await?;
            }
        }
    }
    Ok(())
}

async fn close_channel(
    name: &str,
    reason: &str,
    channels: &mut HashMap<String, Channel>,
    streams: &mut HashMap<String, Stream>,
    reassembly: &mut Reassembly,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<(), FrameError> {
    channels.remove(name);
    streams.remove(name);
    reassembly.drop_channel(name);
    windows.remove(name);
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

#[expect(
    clippy::too_many_arguments,
    reason = "one channel turn needs all of these; see serve_channel"
)]
async fn handle_channel_bytes(
    name: &str,
    raw: &[u8],
    channels: &mut HashMap<String, Channel>,
    streams: &mut HashMap<String, Stream>,
    reassembly: &mut Reassembly,
    forge_cnf_secret: &[u8; 32],
    verifier: &Arc<crate::account_auth::GrantVerifier>,
    app: &crate::routes::App,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
    stream_tx: &tokio::sync::mpsc::Sender<(String, StreamMsg)>,
) -> Result<(), ChannelError> {
    let Some(channel) = channels.get_mut(name) else {
        return Err(ChannelError::Unknown {
            channel: name.to_owned(),
        });
    };
    if channel.noise.phase() == Phase::Handshake {
        channel.noise.receive_handshake_message(raw)?;
        let mut reply = Vec::new();
        if channel.noise.next_handshake_message(&mut reply).is_ok() && !reply.is_empty() {
            send_data(name, &reply, windows, sink).await?;
        }
        return Ok(());
    }
    if channel.noise.phase() == Phase::Confirm {
        let confirm = channel.noise.receive_initiator_confirm(raw)?;
        let jwt = std::str::from_utf8(&confirm.jwt).map_err(|source| ChannelError::GrantText {
            channel: name.to_owned(),
            source,
        })?;
        let claims = verifier
            .verify_channel_grant(jwt, &channel.account, &channel.machine, now_sec(), false)
            .await
            .map_err(|source| ChannelError::Grant {
                channel: name.to_owned(),
                source,
            })?;
        let cnf = artisan_account::cnf_public_key(&claims).map_err(|source| ChannelError::Cnf {
            channel: name.to_owned(),
            source,
        })?;
        channel.noise.accept_initiator_confirm(&cnf, &confirm)?;
        channel.claims = Some(claims);
        let reply = channel.noise.emit_responder_confirm(forge_cnf_secret)?;
        send_data(name, &reply, windows, sink).await?;
        return Ok(());
    }
    let plaintext = channel.noise.receive_data(raw)?;
    let channel_name = || name.to_owned();
    match channel_authorization(channel.claims.as_ref(), now_sec()) {
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
        if let Some(reassembled) = reassembly.feed(name, &chunk)? {
            dispatch_app_bytes(
                name,
                &reassembled,
                channels,
                streams,
                app,
                windows,
                sink,
                stream_tx,
            )
            .await?;
        }
        return Ok(());
    }
    dispatch_app_bytes(
        name, &plaintext, channels, streams, app, windows, sink, stream_tx,
    )
    .await?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "dispatch fans out to unary, subscribe, and unsubscribe paths"
)]
async fn dispatch_app_bytes(
    name: &str,
    plaintext: &[u8],
    channels: &mut HashMap<String, Channel>,
    streams: &mut HashMap<String, Stream>,
    app: &crate::routes::App,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
    stream_tx: &tokio::sync::mpsc::Sender<(String, StreamMsg)>,
) -> Result<(), ChannelError> {
    let request: Result<artisan_api::TunnelRequest, _> = serde_json::from_slice(plaintext);
    let Ok(request) = request else {
        let response =
            artisan_api::TunnelResponse::error("?", 400, "tunneled call is not a request");
        send_response(name, &response, channels, windows, sink).await?;
        return Ok(());
    };
    match request.method.as_str() {
        artisan_api::METHOD_SUBSCRIBE => {
            subscribe(
                name, &request, channels, streams, app, windows, sink, stream_tx,
            )
            .await?;
        }
        artisan_api::METHOD_UNSUBSCRIBE => {
            let existed = streams.remove(name).is_some();
            let response = artisan_api::TunnelResponse::ok(
                &request.id,
                serde_json::json!({"unsubscribed": existed}),
            );
            send_response(name, &response, channels, windows, sink).await?;
        }
        _ => {
            let Some(claims) = channels
                .get(name)
                .and_then(|channel| channel.claims.clone())
            else {
                let response = artisan_api::TunnelResponse::error(
                    &request.id,
                    401,
                    "no verified grant on this channel",
                );
                send_response(name, &response, channels, windows, sink).await?;
                return Ok(());
            };
            let router = crate::routes::router(app.clone());
            let tx = stream_tx.clone();
            let name = name.to_owned();
            tokio::spawn(async move {
                let response = dispatch_request(&request, &router, &claims).await;
                if tx
                    .send((name.clone(), StreamMsg::Response(response)))
                    .await
                    .is_err()
                {
                    tracing::debug!(channel = %name, request = %request.id, "the Relay connection closed before the response");
                }
            });
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "subscribe fans out to the pump; see serve_channel"
)]
async fn subscribe(
    name: &str,
    request: &artisan_api::TunnelRequest,
    channels: &mut HashMap<String, Channel>,
    streams: &mut HashMap<String, Stream>,
    app: &crate::routes::App,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
    stream_tx: &tokio::sync::mpsc::Sender<(String, StreamMsg)>,
) -> Result<(), ChannelError> {
    let Some(since) = crate::events::Since::from_events_path(&request.path) else {
        let response = artisan_api::TunnelResponse::error(&request.id, 400, "bad subscribe path");
        send_response(name, &response, channels, windows, sink).await?;
        return Ok(());
    };
    if streams.contains_key(name) {
        let response = artisan_api::TunnelResponse::error(
            &request.id,
            409,
            "one stream per channel; open another channel",
        );
        send_response(name, &response, channels, windows, sink).await?;
        return Ok(());
    }
    let Some(expires_at) = channels
        .get(name)
        .and_then(|channel| channel.claims.as_ref())
        .map(|claims| claims.exp)
    else {
        let response = artisan_api::TunnelResponse::error(
            &request.id,
            401,
            "no verified grant on this channel",
        );
        send_response(name, &response, channels, windows, sink).await?;
        return Ok(());
    };
    let expiry = tokio::time::Instant::now()
        + std::time::Duration::from_secs(
            expires_at.saturating_sub(now_sec()).max(0).cast_unsigned(),
        );
    let stream_id = name.to_owned();
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<artisan_api::Envelope>(artisan_api::STREAM_QUEUE_ENVELOPES);
    let pump_app = app.clone();
    let response = artisan_api::TunnelResponse::ok(
        &request.id,
        serde_json::json!({"$stream": {"stream": stream_id}}),
    );
    send_response(name, &response, channels, windows, sink).await?;
    let pumped = stream_id.clone();
    let pump = tokio::spawn(async move {
        if let Err(error) = crate::events::forward(&pump_app, &since, &sender).await {
            tracing::debug!(channel = %pumped, error = diagnostics::chain(&error), "tunneled event stream ended");
        }
    });
    let pump = tokio_util::task::AbortOnDropHandle::new(pump);
    let forwarder = tokio::spawn(forward_stream(
        name.to_owned(),
        receiver,
        stream_tx.clone(),
        expiry,
    ));
    streams.insert(
        name.to_owned(),
        Stream {
            _pump: pump,
            _forwarder: tokio_util::task::AbortOnDropHandle::new(forwarder),
        },
    );
    Ok(())
}

async fn forward_stream(
    name: String,
    mut receiver: tokio::sync::mpsc::Receiver<artisan_api::Envelope>,
    tx: tokio::sync::mpsc::Sender<(String, StreamMsg)>,
    expiry: tokio::time::Instant,
) {
    let end = loop {
        let envelope = tokio::select! {
            () = tokio::time::sleep_until(expiry) => break StreamMsg::Expired,
            envelope = receiver.recv() => envelope,
        };
        let Some(envelope) = envelope else {
            break StreamMsg::End { reason: "closed" };
        };
        let frame = serde_json::json!({
            "$event": {"stream": name, "envelope": envelope},
        });
        let bytes = serde_json::to_vec(&frame).expect("invariant: a JSON value always serializes");
        if tx
            .send((name.clone(), StreamMsg::Frame(bytes)))
            .await
            .is_err()
        {
            return;
        }
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
    streams: &mut HashMap<String, Stream>,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    if !channels.contains_key(name) {
        streams.remove(name);
        return Ok(());
    }
    let encoded = |what| {
        move |source| ChannelError::Encode {
            channel: name.to_owned(),
            what,
            source,
        }
    };
    let bytes = match msg {
        StreamMsg::Frame(bytes) => bytes,
        StreamMsg::Response(response) => {
            serde_json::to_vec(&response).map_err(encoded("tunneled response"))?
        }
        StreamMsg::End { reason } => {
            streams.remove(name);
            serde_json::to_vec(&serde_json::json!({"$event": {"stream": name, "end": reason}}))
                .map_err(encoded("stream end"))?
        }
        StreamMsg::Expired => {
            unreachable!("invariant: channel {name} closes on expiry before delivery")
        }
    };
    send_payload(name, &bytes, channels, windows, sink).await
}

async fn send_response(
    name: &str,
    response: &artisan_api::TunnelResponse,
    channels: &mut HashMap<String, Channel>,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    let bytes = serde_json::to_vec(response).map_err(|source| ChannelError::Encode {
        channel: name.to_owned(),
        what: "tunneled response",
        source,
    })?;
    send_payload(name, &bytes, channels, windows, sink).await
}

async fn send_payload(
    name: &str,
    bytes: &[u8],
    channels: &mut HashMap<String, Channel>,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    let Some(channel) = channels.get_mut(name) else {
        return Err(ChannelError::Unknown {
            channel: name.to_owned(),
        });
    };
    if channel.noise.phase() != Phase::Open {
        return Ok(());
    }
    if bytes.len() <= artisan_api::TUNNEL_PLAINTEXT_BUDGET {
        let sealed = channel.noise.seal(bytes)?;
        return send_data(name, &sealed, windows, sink).await;
    }
    let transfer = format!("{name}:{}", next_transfer_id());
    for chunk in artisan_api::chunk_bytes(&transfer, bytes) {
        let chunk = serde_json::to_vec(&chunk).map_err(|source| ChannelError::Encode {
            channel: name.to_owned(),
            what: "tunnel chunk",
            source,
        })?;
        let sealed = channel.noise.seal(&chunk)?;
        send_data(name, &sealed, windows, sink).await?;
    }
    Ok(())
}

async fn send_data(
    name: &str,
    raw: &[u8],
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<(), ChannelError> {
    let body = b64u_encode(raw);
    let window = windows
        .entry(name.to_owned())
        .or_insert_with(SendWindow::new);
    if window.should_park(body.len()) {
        if window.parked.len() >= SEND_PARKED_MAX {
            return Err(ChannelError::WindowFull {
                channel: name.to_owned(),
            });
        }
        window.parked.push_back(body);
        return Ok(());
    }
    let seq = window.sent_high.map_or(0, |sent| sent.saturating_add(1));
    send_frame(name, seq, &body, sink).await?;
    if let Some(window) = windows.get_mut(name) {
        window.noted_sent(seq, body.len());
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

async fn drain_parked(
    name: &str,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<(), FrameError> {
    loop {
        let ready = windows.get(name).is_some_and(|window| {
            window
                .parked
                .front()
                .is_some_and(|body| window.has_space(body.len()))
        });
        if !ready {
            break;
        }
        let body = windows
            .get_mut(name)
            .and_then(|window| window.parked.pop_front());
        let Some(body) = body else {
            break;
        };
        let seq = windows
            .get(name)
            .and_then(|window| window.sent_high)
            .map_or(0, |sent| sent.saturating_add(1));
        send_frame(name, seq, &body, sink).await?;
        if let Some(window) = windows.get_mut(name) {
            window.noted_sent(seq, body.len());
        }
    }
    Ok(())
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
    let bytes = match axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024).await {
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
#[allow(clippy::unwrap_used, reason = "test fixtures; failures fail the test")]
mod tests {
    use super::*;
    use artisan_account::NoiseChannel;
    use tokio_tungstenite::tungstenite::Message as Ws;

    const ACCOUNT: &str = "acct_00000000000000000000000000";
    const MACHINE: &str = "mch_11111111111111111111111111";

    fn ctx(channel: &str) -> ChannelCtx {
        ChannelCtx {
            account: ACCOUNT.into(),
            machine: MACHINE.into(),
            channel: channel.into(),
        }
    }

    fn test_issuer() -> (ed25519_dalek::SigningKey, String) {
        use base64::Engine as _;
        let key = ed25519_dalek::SigningKey::from_bytes(&[0x77_u8; 32]);
        let x =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes());
        (key, x)
    }

    fn machine_grant(
        issuer: &ed25519_dalek::SigningKey,
        cnf_secret: &[u8; 32],
    ) -> (String, String) {
        use base64::Engine as _;
        use ed25519_dalek::Signer as _;
        let cnf_public = ed25519_dalek::SigningKey::from_bytes(cnf_secret)
            .verifying_key()
            .to_bytes();
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({"alg": "EdDSA", "kid": "k1", "typ": "JWT"}).to_string());
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "v": 1,
                "iss": "https://core.example.invalid",
                "aud": "ast-relay",
                "sub": ACCOUNT,
                "mid": MACHINE,
                "role": "forge",
                "cnf": {"jwk": {"kty": "OKP", "crv": "Ed25519", "x": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cnf_public)}},
                "scope": ["relay:connect"],
                "iat": 1_700_000_000,
                "nbf": 1_700_000_000,
                "exp": 1_700_000_200,
                "jti": "tkn-relay-test-0001",
            })
            .to_string(),
        );
        let input = format!("{header}.{payload}");
        let signature = issuer.sign(input.as_bytes());
        (
            format!(
                "{input}.{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes())
            ),
            "tkn-relay-test-0001".into(),
        )
    }

    fn editor_grant(issuer: &ed25519_dalek::SigningKey, cnf_secret: &[u8; 32]) -> String {
        use base64::Engine as _;
        use ed25519_dalek::Signer as _;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1_700_000_100, |elapsed| elapsed.as_secs().cast_signed());
        let cnf_public = ed25519_dalek::SigningKey::from_bytes(cnf_secret)
            .verifying_key()
            .to_bytes();
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({"alg": "EdDSA", "kid": "k1", "typ": "JWT"}).to_string());
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "v": 1,
                "iss": "https://core.example.invalid",
                "aud": "ast-relay",
                "sub": ACCOUNT,
                "mid": MACHINE,
                "role": "client",
                "cnf": {"jwk": {"kty": "OKP", "crv": "Ed25519", "x": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cnf_public)}},
                "scope": ["relay:connect", "forge:execute"],
                "iat": now - 10,
                "nbf": now - 10,
                "exp": now + 200,
                "jti": "tkn-editor-test-0001",
            })
            .to_string(),
        );
        let input = format!("{header}.{payload}");
        let signature = issuer.sign(input.as_bytes());
        format!(
            "{input}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes())
        )
    }

    #[expect(clippy::too_many_lines, reason = "one stub-relay journey step by step")]
    #[tokio::test]
    async fn tunneled_state_unary_and_stream_against_stub_relay() {
        async fn recv_text(
            source: &mut futures::stream::SplitStream<
                tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
            >,
        ) -> String {
            tokio::time::timeout(std::time::Duration::from_secs(15), source.next())
                .await
                .expect("relay stub read timed out")
                .expect("relay closed the stream")
                .expect("relay read failed")
                .into_text()
                .expect("relay sent non-text")
                .to_string()
        }

        async fn send(
            sink: &mut futures::stream::SplitSink<
                tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
                Ws,
            >,
            text: String,
        ) {
            use futures::SinkExt as _;
            tokio::time::timeout(
                std::time::Duration::from_secs(15),
                sink.send(Ws::Text(text.into())),
            )
            .await
            .expect("relay stub send timed out")
            .unwrap();
        }

        let (issuer_key, issuer_x) = test_issuer();
        let (verifier, _jwks) = crate::account_auth::tests::verifier_with_stub(serde_json::json!({
            "keys": [{"kty": "OKP", "crv": "Ed25519", "x": issuer_x, "kid": "k1",
                      "use": "sig", "alg": "EdDSA"}],
        }))
        .await;
        let forge_cnf = [0x44_u8; 32];
        let editor_cnf = [0x33_u8; 32];
        let editor_noise = [0x11_u8; 32];
        let forge_enrolled_pub = ed25519_dalek::SigningKey::from_bytes(&forge_cnf)
            .verifying_key()
            .to_bytes();
        let (grant_jwt, grant_jti) = machine_grant(&issuer_key, &[0x45_u8; 32]);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let ws_url = format!("ws://{address}/v1/route/x/y");

        let stub = tokio::spawn(async move {
            let stream = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap();
            let (mut sink, mut source) = stream.split();
            let hello = recv_text(&mut source).await;
            let hello: artisan_account::ClientFrame = serde_json::from_str(&hello).unwrap();
            let grant = match hello {
                artisan_account::ClientFrame::Hello(h) => h.grant,
                artisan_account::ClientFrame::Authenticate(_)
                | artisan_account::ClientFrame::Open(_)
                | artisan_account::ClientFrame::Data(_)
                | artisan_account::ClientFrame::Ack(_)
                | artisan_account::ClientFrame::Close(_)
                | artisan_account::ClientFrame::Pong(_) => panic!("expected hello"),
            };
            assert!(!grant.is_empty());
            let nonce = [0x66_u8; 32];
            send(
                &mut sink,
                serde_json::to_string(&artisan_account::ServerFrame::Challenge(
                    artisan_account::ChallengeFrame {
                        v: 1,
                        nonce: b64u_encode(&nonce),
                        timeout_ms: 60_000,
                    },
                ))
                .unwrap(),
            )
            .await;
            let auth = recv_text(&mut source).await;
            let auth: artisan_account::ClientFrame = serde_json::from_str(&auth).unwrap();
            match auth {
                artisan_account::ClientFrame::Authenticate(a) => {
                    assert!(!a.sig.is_empty());
                }
                artisan_account::ClientFrame::Hello(_)
                | artisan_account::ClientFrame::Open(_)
                | artisan_account::ClientFrame::Data(_)
                | artisan_account::ClientFrame::Ack(_)
                | artisan_account::ClientFrame::Close(_)
                | artisan_account::ClientFrame::Pong(_) => panic!("expected authenticate"),
            }
            send(
                &mut sink,
                serde_json::to_string(&artisan_account::ServerFrame::Ping(
                    artisan_account::PingFrame { v: 1, t: 1234 },
                ))
                .unwrap(),
            )
            .await;
            let pong: serde_json::Value =
                serde_json::from_str(&recv_text(&mut source).await).unwrap();
            assert_eq!(pong, serde_json::json!({"v": 1, "type": "pong", "t": 1234}));
            send(
                &mut sink,
                serde_json::to_string(&artisan_account::ServerFrame::Ready(
                    artisan_account::ReadyFrame {
                        v: 1,
                        role: "forge".into(),
                        account: "a".into(),
                        machine: "m".into(),
                        forge_online: true,
                    },
                ))
                .unwrap(),
            )
            .await;
            send(
                &mut sink,
                serde_json::to_string(&artisan_account::ServerFrame::ChannelOpened(
                    artisan_account::ChannelOpenedFrame {
                        v: 1,
                        channel: "ch1".into(),
                        peer: "client".into(),
                    },
                ))
                .unwrap(),
            )
            .await;

            let mut init = NoiseChannel::initiator(&editor_noise, None, ctx("ch1")).unwrap();
            let mut relay_seq = 0_u64;
            let mut send_data = async |sink: &mut futures::stream::SplitSink<_, Ws>,
                                       raw: Vec<u8>| {
                let frame = serde_json::to_string(&artisan_account::ClientFrame::Data(
                    artisan_account::DataFrame {
                        v: 1,
                        channel: "ch1".into(),
                        seq: relay_seq,
                        body: b64u_encode(&raw),
                    },
                ))
                .unwrap();
                relay_seq += 1;
                sink.send(Ws::Text(frame.into())).await.unwrap();
            };
            let mut next_server_frame = async || -> artisan_account::ServerFrame {
                loop {
                    let text = recv_text(&mut source).await;
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value.get("type").and_then(|t| t.as_str()) == Some("ack") {
                        let ack: artisan_account::ClientFrame =
                            serde_json::from_value(value).unwrap();
                        match ack {
                            artisan_account::ClientFrame::Ack(a) => assert_eq!(a.channel, "ch1"),
                            artisan_account::ClientFrame::Hello(_)
                            | artisan_account::ClientFrame::Authenticate(_)
                            | artisan_account::ClientFrame::Open(_)
                            | artisan_account::ClientFrame::Data(_)
                            | artisan_account::ClientFrame::Close(_)
                            | artisan_account::ClientFrame::Pong(_) => panic!("expected ack shape"),
                        }
                        continue;
                    }
                    return artisan_account::read_server_frame(value).unwrap();
                }
            };

            let mut m1 = Vec::new();
            init.next_handshake_message(&mut m1).unwrap();
            send_data(&mut sink, m1).await;
            let m2 = match next_server_frame().await {
                ServerFrame::Data(data) => b64u_decode(&data.body).unwrap(),
                other @ (ServerFrame::Challenge(_)
                | ServerFrame::Ready(_)
                | ServerFrame::ChannelOpened(_)
                | ServerFrame::ChannelClosed(_)
                | ServerFrame::Ack(_)
                | ServerFrame::Presence(_)
                | ServerFrame::Error(_)
                | ServerFrame::Ping(_)) => panic!("expected data, got {other:?}"),
            };
            init.receive_handshake_message(&m2).unwrap();
            let mut m3 = Vec::new();
            init.next_handshake_message(&mut m3).unwrap();
            send_data(&mut sink, m3).await;
            let grant = editor_grant(&issuer_key, &editor_cnf);
            let c1 = init
                .emit_initiator_confirm(&editor_cnf, grant.as_bytes())
                .unwrap();
            send_data(&mut sink, c1).await;
            let c2 = match next_server_frame().await {
                ServerFrame::Data(data) => b64u_decode(&data.body).unwrap(),
                other @ (ServerFrame::Challenge(_)
                | ServerFrame::Ready(_)
                | ServerFrame::ChannelOpened(_)
                | ServerFrame::ChannelClosed(_)
                | ServerFrame::Ack(_)
                | ServerFrame::Presence(_)
                | ServerFrame::Error(_)
                | ServerFrame::Ping(_)) => panic!("expected data, got {other:?}"),
            };
            init.receive_responder_confirm(&c2, &forge_enrolled_pub)
                .unwrap();
            assert_eq!(init.phase(), artisan_account::Phase::Open);

            let request = artisan_api::TunnelRequest {
                id: "r1".into(),
                method: "GET".into(),
                path: "/engines".into(),
                body: None,
            };
            let ct = init.seal(&serde_json::to_vec(&request).unwrap()).unwrap();
            send_data(&mut sink, ct).await;
            let answer = match next_server_frame().await {
                ServerFrame::Data(data) => b64u_decode(&data.body).unwrap(),
                other @ (ServerFrame::Challenge(_)
                | ServerFrame::Ready(_)
                | ServerFrame::ChannelOpened(_)
                | ServerFrame::ChannelClosed(_)
                | ServerFrame::Ack(_)
                | ServerFrame::Presence(_)
                | ServerFrame::Error(_)
                | ServerFrame::Ping(_)) => panic!("expected data, got {other:?}"),
            };
            let plaintext = init.receive_data(&answer).unwrap();
            let response: artisan_api::TunnelResponse = serde_json::from_slice(&plaintext).unwrap();
            assert_eq!(response.id, "r1");
            assert_eq!(response.status, 200);
            assert!(response.body.is_array(), "engines list must round-trip");
        });

        let app = test_app();
        let serve = tokio::spawn(serve_channel(
            ws_url,
            grant_jwt,
            grant_jti,
            [0x45_u8; 32],
            forge_cnf,
            ACCOUNT.into(),
            MACHINE.into(),
            std::sync::Arc::new(verifier),
            app,
        ));
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(90), stub).await;
        serve.abort();
        outcome.expect("relay e2e journey timed out").unwrap();
    }

    fn test_app() -> crate::routes::App {
        let dir = tempfile::tempdir().unwrap();
        let dir = Box::leak(Box::new(dir)).path().to_owned();
        let store = forge_store::Store::open(&dir).unwrap();
        let engines = forge_engines::Engines::new(&dir);
        let hub = forge_runs::Hub::new(store, engines, dir);
        crate::routes::App {
            hub,
            forge_version: "test".into(),
            shutdown: tokio_util::sync::CancellationToken::new(),
            tasks: tokio_util::task::TaskTracker::new(),
            repositories: std::sync::Arc::new(crate::repository::Repositories::new().unwrap()),
            icons: std::sync::Arc::default(),
        }
    }

    #[test]
    fn parked_full_chunks_hold_back_a_fitting_final_chunk() {
        let mut window = SendWindow::new();
        let big = "x".repeat(300_000);
        window.noted_sent(0, big.len());
        window.noted_sent(1, big.len());
        window.noted_sent(2, big.len());
        let full = "y".repeat(150_000);
        assert!(!window.has_space(full.len()));
        window.parked.push_back(full.clone());
        let small = "z".repeat(100);
        assert!(window.has_space(small.len()));
        assert!(
            window.should_park(small.len()),
            "parked queue must force FIFO even when small fits"
        );
        window.parked.push_back(small.clone());
        assert_eq!(window.parked.len(), 2);
        assert_eq!(window.parked[0], full);
        assert_eq!(window.parked[1], small);
    }

    #[test]
    fn first_cumulative_ack_releases_exactly_what_was_sent() {
        let mut window = SendWindow::new();
        window.noted_sent(0, 100);
        window.noted_sent(1, 100);
        window.noted_sent(2, 100);
        window.on_ack(1);
        assert_eq!(window.acked_high, Some(1));
        assert_eq!(window.sizes.len(), 1);
        assert_eq!(window.unacked_bytes, 100);
        window.on_ack(2);
        assert_eq!(window.acked_high, Some(2));
        assert!(window.sizes.is_empty());
        assert_eq!(window.unacked_bytes, 0);
        window.on_ack(2);
        assert_eq!(window.acked_high, Some(2));
        window.on_ack(9);
        assert_eq!(window.acked_high, Some(2));
    }

    fn claims(exp: i64) -> artisan_account::GrantClaims {
        serde_json::from_value(serde_json::json!({
            "v": 1,
            "iss": "https://core.example.invalid",
            "aud": "ast-relay",
            "sub": ACCOUNT,
            "mid": MACHINE,
            "role": "client",
            "cnf": {"jwk": {"kty": "OKP", "crv": "Ed25519", "x": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}},
            "scope": ["relay:connect", "forge:execute"],
            "iat": exp - 300,
            "exp": exp,
            "jti": "tkn-editor-test-0002",
        }))
        .unwrap()
    }

    fn open_channel(exp: i64) -> HashMap<String, Channel> {
        let noise = NoiseChannel::responder(&[0x12_u8; 32], None, ctx("ch1")).unwrap();
        [(
            "ch1".to_owned(),
            Channel {
                noise,
                claims: Some(claims(exp)),
                account: ACCOUNT.into(),
                machine: MACHINE.into(),
            },
        )]
        .into()
    }

    #[test]
    fn events_queued_before_the_grant_expired_close_the_channel_instead_of_sending() {
        let channels = open_channel(1_700_000_100);
        let queued = StreamMsg::Frame(b"{}".to_vec());
        assert_eq!(
            outgoing("ch1", &queued, &channels, 1_700_000_099),
            Outgoing::Deliver
        );
        assert_eq!(
            outgoing("ch1", &queued, &channels, 1_700_000_100),
            Outgoing::Close {
                reason: "grant-expired"
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_tunneled_stream_ends_when_its_grant_expires() {
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let expiry = tokio::time::Instant::now() + std::time::Duration::from_secs(200);
        let forwarder = tokio::spawn(forward_stream("ch1".into(), receiver, tx, expiry));
        sender
            .send(artisan_api::Envelope {
                seq: Some(1),
                live_revision: 0,
                event: artisan_api::Event::Reset,
            })
            .await
            .unwrap();
        let first = rx.recv().await.unwrap();
        assert!(matches!(first, (name, StreamMsg::Frame(_)) if name == "ch1"));
        let last = tokio::time::timeout(std::time::Duration::from_secs(600), rx.recv())
            .await
            .expect("an expired grant kept its event stream open")
            .unwrap();
        assert!(matches!(last.1, StreamMsg::Expired));
        assert!(tokio::time::Instant::now() >= expiry);
        forwarder.await.unwrap();
        drop(sender);
    }
}
