//! Outbound Relay connector: Forge → Relay → Editor.
//!
//! The Forge holds one WebSocket to its Relay room and serves Noise
//! channels as the responder (the opener is always the client/Editor).
//! Admission is JWT + Ed25519 proof-of-possession; every channel runs the
//! standard `Noise_XX` session (`contracts/e2e.md`), then mutual key
//! confirmation binds the session to the Core identities: the initiator's
//! grant JWT travels INSIDE its encrypted confirm, the Forge verifies that
//! grant itself (Core JWKS) and takes ONLY the verified `cnf` key for the
//! binding check. No static-key registry, no per-session publishing step,
//! no Core cnf endpoint. Relay admission alone never authorizes execution:
//! stored verified claims must also carry `forge:execute` before anything
//! runs. Relay sees only opaque bodies.
//!
//! Flow control (`contracts/wire.md`): the connector acks every received
//! Relay data frame (cumulative); senders that stall the window are
//! dropped by Relay.
//!
//! Application protocol (see `docs/plans/ast-account-wire.md`): unary
//! `TunnelRequest`/`TunnelResponse` calls plus event streaming. A
//! `SUBSCRIBE` call opens a bounded server→client stream on its channel
//! (backlog + live envelopes, `Reset` on lag, explicit `end`); closing
//! the Relay channel cancels the stream. Streams run on their own
//! channels with per-channel Noise sessions, so unary calls proceed
//! while a stream is open.

use anyhow::{Context, Result};
use artisan_account::{ChannelCtx, ClientFrame, NoiseChannel, Phase, ServerFrame};
use futures::{SinkExt as _, StreamExt as _};
use std::collections::HashMap;
use std::sync::Arc;

/// One live channel on the Relay connection.
struct Channel {
    noise: NoiseChannel,
    /// Verified in-confirm grant claims; `None` until confirmation.
    /// Dispatch and streaming both require `forge:execute` + unexpired.
    claims: Option<artisan_account::GrantClaims>,
    account: String,
    machine: String,
}

/// Work finished off the socket loop, headed back to one channel.
enum StreamMsg {
    Frame(Vec<u8>),
    End { reason: &'static str },
    Response(artisan_api::TunnelResponse),
}

/// Per-channel send window mirroring the Relay room's accounting
/// (`room.ts`: at most 64 unacked frames AND 1 MiB unacked bytes per
/// channel per direction). Sizes are the BASE64 STRING
/// `frame.body.length` exactly as Relay counts them (4/3 expansion
/// included) — never a guessed raw count. Overflow parks in a bounded
/// per-channel queue and drains as forwarded cumulative acks slide the
/// window. Queue overflow fails the channel (fail-closed) rather than
/// overflowing Relay.
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

    /// Strict FIFO: once anything parks, every later ciphertext parks
    /// behind it even if it would fit the remaining window. Bypassing
    /// would reorder Noise nonces (peer decrypt failure).
    fn should_park(&self, body_len: usize) -> bool {
        !self.parked.is_empty() || !self.has_space(body_len)
    }

    fn noted_sent(&mut self, seq: u64, body_len: usize) {
        self.sent_high = Some(seq);
        self.sizes.push_back(body_len);
        self.unacked_bytes = self.unacked_bytes.saturating_add(body_len as u64);
    }

    /// Release everything up to `ack_seq` (cumulative). Mirrors the room:
    /// duplicates and ahead-of-sent acks are ignored. Releases exactly the
    /// frames implied by the previous ack cursor (ack_seq+1 entries on the
    /// first ack), bounded by the actual unacked queue — never fabricates
    /// credit beyond what was sent.
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

/// One live tunneled event stream. Dropping it aborts the pump and the
/// forwarder; the pump also ends on its own when the receiver closes.
struct Stream {
    _pump: tokio_util::task::AbortOnDropHandle<()>,
    _forwarder: tokio_util::task::AbortOnDropHandle<()>,
}

type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Message,
>;

fn now_sec() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    )
    .unwrap_or(0)
}

fn b64u_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn b64u_decode(text: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.as_bytes())
        .context("bad base64url body")
}

/// Serve Relay channels into the local Forge app until the socket closes.
///
/// `grant_jwt`/`grant_jti`/`cnf_secret`: fresh machine grant for admission
/// (`aud=ast-relay`, `relay:connect`) and its proof key. `forge_cnf_secret`:
/// the enrolled machine Ed25519 key that signs key confirmations. The
/// caller refreshes grants by reconnecting (short TTLs by design).
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
) -> Result<()> {
    use tokio_tungstenite::tungstenite::Message as Ws;
    let (stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .with_context(|| format!("connect to Relay at {ws_url}"))?;
    let (mut sink, mut source) = stream.split();
    let hello = serde_json::to_string(&ClientFrame::Hello(artisan_account::HelloFrame {
        v: 1,
        grant: grant_jwt,
    }))?;
    sink.send(Ws::Text(hello.into())).await?;
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
                let message = message.context("read Relay frame")?;
                let text = match message {
                    Ws::Text(text) => text.to_string(),
                    Ws::Close(_) => break,
                    Ws::Ping(data) => {
                        sink.send(Ws::Pong(data)).await?;
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
                if let Err(error) = handle_stream_msg(&name, msg, &mut channels, &mut streams, &mut windows, &mut sink).await {
                    tracing::debug!(channel = %name, "Relay channel failed: {error:#}");
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
) -> Result<()> {
    use tokio_tungstenite::tungstenite::Message as Ws;
    // A frame this build does not know (a newer Relay) is skipped, never fatal.
    let frame = match serde_json::from_str(text)
        .map_err(anyhow::Error::from)
        .and_then(|value| artisan_account::read_server_frame(value).map_err(anyhow::Error::from))
    {
        Ok(frame) => frame,
        Err(error) => {
            tracing::debug!("skipped a Relay frame: {error:#}");
            return Ok(());
        }
    };
    match frame {
        ServerFrame::Challenge(challenge) => {
            use base64::Engine as _;
            let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(challenge.nonce.as_bytes())
                .context("read the Relay challenge")?;
            let nonce: [u8; 32] = nonce
                .try_into()
                .map_err(|_nonce| anyhow::anyhow!("bad challenge nonce"))?;
            let signature = artisan_account::pop_sign(cnf_secret, &nonce, grant_jti);
            let authenticate = serde_json::to_string(&ClientFrame::Authenticate(
                artisan_account::AuthenticateFrame {
                    v: 1,
                    jti: grant_jti.to_owned(),
                    sig: b64u_encode(&signature),
                },
            ))?;
            sink.send(Ws::Text(authenticate.into())).await?;
        }
        ServerFrame::Ready(_) | ServerFrame::Presence(_) => {}
        ServerFrame::Ping(ping) => {
            let pong = ClientFrame::Pong(artisan_account::PongFrame { v: 1, t: ping.t });
            sink.send(Ws::Text(serde_json::to_string(&pong)?.into()))
                .await?;
        }
        ServerFrame::Error(error) => {
            anyhow::bail!("Relay refused the tunnel: {} {}", error.code, error.message);
        }
        ServerFrame::ChannelOpened(opened) => {
            let name = opened.channel;
            let ctx = ChannelCtx {
                account: account.to_owned(),
                machine: machine.to_owned(),
                channel: name.clone(),
            };
            // Fresh Noise static per channel (pure DH randomness).
            let static_secret = fresh_static()?;
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
            // Ack every received data frame first (cumulative), so the
            // sender never stalls the window — even for unknown channels
            // (Relay ignores acks for closed/foreign channels).
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
            let Ok(raw) = b64u_decode(&data.body) else {
                close_channel(
                    &name, "bad-body", channels, streams, reassembly, windows, sink,
                )
                .await?;
                return Ok(());
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
                tracing::debug!(channel = %name, "Relay channel failed: {error:#}");
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
            // Relay forwards the recipient's cumulative ack to the
            // original sender: slide our send window and drain parked
            // frames. Stale acks for closed/foreign channels are ignored.
            if let Some(window) = windows.get_mut(&ack.channel) {
                window.on_ack(ack.seq);
                drain_parked(&ack.channel, windows, sink).await?;
            }
        }
    }
    Ok(())
}

/// Ends one channel: drops its state and tells the peer. Only a failed
/// socket write is returned, since that ends the whole connection anyway.
async fn close_channel(
    name: &str,
    reason: &str,
    channels: &mut HashMap<String, Channel>,
    streams: &mut HashMap<String, Stream>,
    reassembly: &mut Reassembly,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<()> {
    use tokio_tungstenite::tungstenite::Message as Ws;
    channels.remove(name);
    streams.remove(name);
    reassembly.drop_channel(name);
    windows.remove(name);
    let close = serde_json::to_string(&ClientFrame::Close(artisan_account::CloseFrame {
        v: 1,
        channel: name.to_owned(),
        reason: Some(reason.to_owned()),
    }))?;
    sink.send(Ws::Text(close.into()))
        .await
        .context("send Relay close")
}

fn fresh_static() -> Result<[u8; 32]> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|_rand| anyhow::anyhow!("random static failed"))?;
    Ok(secret)
}

/// One inbound channel payload: handshake, confirm, or transport data.
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
) -> Result<()> {
    let Some(channel) = channels.get_mut(name) else {
        anyhow::bail!("unknown channel");
    };
    // Handshake bytes travel as data bodies until Split.
    if channel.noise.phase() == Phase::Handshake {
        channel.noise.receive_handshake_message(raw)?;
        let mut reply = Vec::new();
        if channel.noise.next_handshake_message(&mut reply).is_ok() && !reply.is_empty() {
            send_data(name, &reply, windows, sink).await?;
        }
        return Ok(());
    }
    // Key confirmation carries the initiator grant JWT.
    if channel.noise.phase() == Phase::Confirm {
        let confirm = channel.noise.receive_initiator_confirm(raw)?;
        let jwt = std::str::from_utf8(&confirm.jwt).context("grant is not text")?;
        let claims = verifier
            .verify_channel_grant(jwt, &channel.account, &channel.machine, now_sec(), false)
            .await?;
        let cnf = artisan_account::cnf_public_key(&claims)
            .map_err(|_cnf| anyhow::anyhow!("bad grant cnf"))?;
        channel.noise.accept_initiator_confirm(&cnf, &confirm)?;
        channel.claims = Some(claims);
        let reply = channel.noise.emit_responder_confirm(forge_cnf_secret)?;
        send_data(name, &reply, windows, sink).await?;
        return Ok(());
    }
    // Transport data: requires a stored execution-capable grant.
    let plaintext = channel.noise.receive_data(raw)?;
    let Some(claims) = channel.claims.as_ref() else {
        anyhow::bail!("no verified grant on this channel");
    };
    if !claims.scope.contains(&artisan_account::Scope::ForgeExecute) {
        anyhow::bail!("grant lacks the forge:execute scope");
    }
    if claims.exp <= now_sec() {
        anyhow::bail!("grant expired mid-channel");
    }
    // Chunked transfers reassemble here (bounded per transfer + per conn).
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
) -> Result<()> {
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
            // Off the socket loop: a slow route must not stall acks, pings
            // or any other channel.
            let router = crate::routes::router(app.clone());
            let tx = stream_tx.clone();
            let name = name.to_owned();
            tokio::spawn(async move {
                let response = dispatch_request(&request, &router, &claims).await;
                tx.send((name, StreamMsg::Response(response))).await.ok();
            });
        }
    }
    Ok(())
}

/// Start a bounded event stream on this channel.
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
) -> Result<()> {
    let Some(since) = crate::events::Since::parse_events_path(&request.path) else {
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
    let stream_id = name.to_owned();
    let (sender, mut receiver) =
        tokio::sync::mpsc::channel::<artisan_api::Envelope>(artisan_api::STREAM_QUEUE_ENVELOPES);
    let pump_app = app.clone();
    // The stream id IS the channel (one stream per channel, v1).
    let response = artisan_api::TunnelResponse::ok(
        &request.id,
        serde_json::json!({"$stream": {"stream": stream_id}}),
    );
    send_response(name, &response, channels, windows, sink).await?;
    let pump = tokio::spawn(async move {
        crate::events::forward_tunnel(&pump_app, &since, &sender)
            .await
            .ok();
    });
    let pump = tokio_util::task::AbortOnDropHandle::new(pump);
    // Forward envelopes from the pump queue to the channel as $event frames.
    // Ends with an explicit end marker; dropping the Stream aborts both
    // tasks (cancel on channel close), which ends the pump via its closed
    // receiver.
    let tx = stream_tx.clone();
    let name_owned = name.to_owned();
    let forwarder = tokio::spawn(async move {
        while let Some(envelope) = receiver.recv().await {
            let frame = serde_json::json!({
                "$event": {"stream": name_owned, "envelope": envelope},
            });
            let bytes = serde_json::to_vec(&frame).unwrap_or_default();
            if tx
                .send((name_owned.clone(), StreamMsg::Frame(bytes)))
                .await
                .is_err()
            {
                break;
            }
        }
        tx.send((name_owned, StreamMsg::End { reason: "closed" }))
            .await
            .ok();
    });
    streams.insert(
        name.to_owned(),
        Stream {
            _pump: pump,
            _forwarder: tokio_util::task::AbortOnDropHandle::new(forwarder),
        },
    );
    Ok(())
}

/// Unique transfer names per chunked payload: responses and stream
/// envelopes share one Reassembly namespace per channel, so a constant
/// transfer name would collide a second large payload with the drained
/// state of the first. A process-wide counter keeps every batch distinct.
fn next_transfer_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static IDS: AtomicU64 = AtomicU64::new(1);
    IDS.fetch_add(1, Ordering::Relaxed)
}

/// Work that finished off the socket loop, sent back on its channel. A
/// channel closed meanwhile just drops it.
async fn handle_stream_msg(
    name: &str,
    msg: StreamMsg,
    channels: &mut HashMap<String, Channel>,
    streams: &mut HashMap<String, Stream>,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<()> {
    if !channels.contains_key(name) {
        streams.remove(name);
        return Ok(());
    }
    let bytes = match msg {
        StreamMsg::Frame(bytes) => bytes,
        StreamMsg::Response(response) => serde_json::to_vec(&response)?,
        StreamMsg::End { reason } => {
            streams.remove(name);
            serde_json::to_vec(&serde_json::json!({"$event": {"stream": name, "end": reason}}))?
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
) -> Result<()> {
    send_payload(
        name,
        &serde_json::to_vec(response)?,
        channels,
        windows,
        sink,
    )
    .await
}

/// Seals one application payload, chunked when it exceeds the plaintext budget.
async fn send_payload(
    name: &str,
    bytes: &[u8],
    channels: &mut HashMap<String, Channel>,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<()> {
    let channel = channels.get_mut(name).context("unknown channel")?;
    if channel.noise.phase() != Phase::Open {
        return Ok(());
    }
    if bytes.len() <= artisan_api::TUNNEL_PLAINTEXT_BUDGET {
        let sealed = channel.noise.seal(bytes)?;
        return send_data(name, &sealed, windows, sink).await;
    }
    let transfer = format!("{name}:{}", next_transfer_id());
    for chunk in artisan_api::chunk_bytes(&transfer, bytes) {
        let sealed = channel.noise.seal(&serde_json::to_vec(&chunk)?)?;
        send_data(name, &sealed, windows, sink).await?;
    }
    Ok(())
}

/// Send one Relay data frame, windowed. Bodies that arrive while the
/// window is full park in a bounded per-channel queue; queue overflow
/// fails the channel (fail-closed) rather than overflowing the Relay.
/// Once anything parks, EVERY later ciphertext parks behind it (strict
/// FIFO) even if it would fit the remaining window — otherwise a small
/// final chunk could bypass parked full-size ciphertexts and reorder the
/// Noise nonces (decrypt failure on the peer).
async fn send_data(
    name: &str,
    raw: &[u8],
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<()> {
    let body = b64u_encode(raw);
    let window = windows
        .entry(name.to_owned())
        .or_insert_with(SendWindow::new);
    if window.should_park(body.len()) {
        if window.parked.len() >= SEND_PARKED_MAX {
            anyhow::bail!("relay window full and park queue full");
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

async fn send_frame(name: &str, seq: u64, body: &str, sink: &mut WsSink) -> Result<()> {
    use tokio_tungstenite::tungstenite::Message as Ws;
    let frame = artisan_account::DataFrame {
        v: 1,
        channel: name.to_owned(),
        seq,
        body: body.to_owned(),
    };
    let text = serde_json::to_string(&ClientFrame::Data(frame))?;
    sink.send(Ws::Text(text.into()))
        .await
        .context("send Relay data")?;
    Ok(())
}

/// Drain parked frames while the window has space (called after an ack).
async fn drain_parked(
    name: &str,
    windows: &mut HashMap<String, SendWindow>,
    sink: &mut WsSink,
) -> Result<()> {
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

async fn send_ack(name: &str, seq: u64, sink: &mut WsSink) -> Result<()> {
    use tokio_tungstenite::tungstenite::Message as Ws;
    let frame = artisan_account::AckFrame {
        v: 1,
        channel: name.to_owned(),
        seq,
    };
    let text = serde_json::to_string(&ClientFrame::Ack(frame))?;
    sink.send(Ws::Text(text.into()))
        .await
        .context("send Relay ack")?;
    Ok(())
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
    // The event stream never ends, so a unary call to it would never answer.
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
    // Grant-based authorization: verified at channel confirmation, carried
    // per request (the direct path keeps using device certificates).
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
        // Valid *now*: the verifier checks iat/exp against the real clock
        // with TTL ≤ 300 s.
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

    /// Full journey against a stub Relay: admission, Noise handshake,
    /// JWT-in-confirm verification, unary dispatch, acks, and an event
    /// stream — with concurrency between stream and unary calls.
    #[expect(clippy::too_many_lines, reason = "one stub-relay journey step by step")]
    #[tokio::test]
    async fn tunneled_state_unary_and_stream_against_stub_relay() {
        // Bounded reads everywhere: a stalled peer fails fast instead of
        // hanging the suite (regression guard for the ~3min idle hang).
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
        let (verifier, _jwks_base) =
            crate::account_auth::tests::verifier_with_stub(serde_json::json!({
                "keys": [{"kty": "OKP", "crv": "Ed25519", "x": issuer_x, "kid": "k1",
                          "use": "sig", "alg": "EdDSA"}],
            }))
            .await;
        // Forge side keys: machine grant cnf + enrolled confirm key.
        let forge_cnf = [0x44_u8; 32];
        let editor_cnf = [0x33_u8; 32];
        let editor_noise = [0x11_u8; 32];
        let forge_enrolled_pub = ed25519_dalek::SigningKey::from_bytes(&forge_cnf)
            .verifying_key()
            .to_bytes();
        let (grant_jwt, grant_jti) = machine_grant(&issuer_key, &[0x45_u8; 32]);

        // Local Forge app under test: state route only (dispatch path).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let ws_url = format!("ws://{address}/v1/route/x/y");

        // Stub Relay: verify admission, then drive one channel as the room.
        let stub = tokio::spawn(async move {
            let stream = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
                .await
                .unwrap();
            let (mut sink, mut source) = stream.split();
            // Admission.
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

            // Noise XX as initiator (client side), no static pinning.
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
                    // Acks are assertions, not protocol steps here.
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

            // m1 → m2 → m3.
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
            // Responder sends exactly one handshake message (m2); m3
            // completes its handshake with no further reply.
            // Initiator confirm carrying the editor grant JWT.
            let grant = editor_grant(&issuer_key, &editor_cnf);
            let c1 = init
                .emit_initiator_confirm(&editor_cnf, grant.as_bytes())
                .unwrap();
            send_data(&mut sink, c1).await;
            // Responder confirm; verify against the Forge enrolled key.
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

            // Unary call through the tunnel (real route on the test app).
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
        // Bounded whole-journey timeout: any stall fails fast with a log
        // instead of hanging the suite.
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
        // Leak the tempdir: the hub owns its data for the test lifetime.
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
        // Regression: once full-size ciphertexts park, a later small final
        // chunk must park behind them (strict FIFO) — never bypass them
        // and reorder Noise nonces.
        let mut window = SendWindow::new();
        // Fill the byte window with max-size bodies without hitting the
        // frame cap: 64 frames would trip first, so use few large bodies.
        let big = "x".repeat(300_000);
        window.noted_sent(0, big.len());
        window.noted_sent(1, big.len());
        window.noted_sent(2, big.len());
        // ~900 KiB unacked: a further ~150 KiB body no longer fits the
        // 1 MiB cap and must park.
        let full = "y".repeat(150_000);
        assert!(!window.has_space(full.len()));
        window.parked.push_back(full.clone());
        // A small final chunk WOULD fit the remaining bytes, but must still
        // park behind the older ciphertext.
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
        // Regression: the first cumulative ack (acked_high None) must
        // release exactly ack_seq+1 entries, bounded by the queue — never
        // jump acked_high to ack_seq after popping a single size.
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
        // Duplicate acks are ignored; ahead-of-sent acks are ignored.
        window.on_ack(2);
        assert_eq!(window.acked_high, Some(2));
        window.on_ack(9);
        assert_eq!(window.acked_high, Some(2));
    }
}
