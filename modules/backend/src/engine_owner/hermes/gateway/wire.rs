//! RFC 6455 frame codec, JSON-RPC envelope decoding, and typed gateway
//! boundary errors for the Hermes gateway.

use base64::Engine as _;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::Instant;

use super::{HERMES_MAX_FRAME_BYTES, HERMES_MAX_ID_BYTES, WEBSOCKET_GUID};
use artisan_transport::CancelHandle;

/// Computes SHA-1 over one message (FIPS 180-4, big-endian length suffix).
///
/// Local because no workspace crate provides SHA-1; used only for the
/// `Sec-WebSocket-Accept` handshake check and pinned against the RFC 6455
/// test vector below.
#[expect(
    clippy::many_single_char_names,
    reason = "SHA-1 round variables keep their canonical FIPS 180-4 single-letter names"
)]
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut state = [
        0x6745_2301_u32,
        0xEFCD_AB89_u32,
        0x98BA_DCFE_u32,
        0x1032_5476_u32,
        0xC3D2_E1F0_u32,
    ];
    let mut padded = message.to_vec();
    let bit_length = u64::try_from(message.len())
        .unwrap_or(u64::MAX)
        .wrapping_mul(8);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());
    for chunk in padded.as_chunks::<64>().0 {
        let mut schedule = [0_u32; 80];
        for (index, bytes) in chunk.as_chunks::<4>().0.iter().enumerate().take(16) {
            schedule[index] = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for index in 16..80 {
            schedule[index] = (schedule[index - 3]
                ^ schedule[index - 8]
                ^ schedule[index - 14]
                ^ schedule[index - 16])
                .rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) =
            (state[0], state[1], state[2], state[3], state[4]);
        for (index, word) in schedule.iter().enumerate() {
            let (f, k) = match index {
                0..20 => ((b & c) | ((!b) & d), 0x5A82_7999_u32),
                20..40 => (b ^ c ^ d, 0x6ED9_EBA1_u32),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC_u32),
                _ => (b ^ c ^ d, 0xCA62_C1D6_u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
    }
    let mut digest = [0_u8; 20];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// Computes the expected `Sec-WebSocket-Accept` value for a client key.
#[must_use]
pub(crate) fn websocket_accept_key(client_key: &str) -> String {
    let material = format!("{client_key}{WEBSOCKET_GUID}");
    base64::engine::general_purpose::STANDARD.encode(sha1(material.as_bytes()))
}

/// Failure decoding or driving one WebSocket frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WsFrameError {
    TooLarge,
    InvalidFrame,
    Closed,
}

/// One decoded WebSocket frame payload.
///
/// Continuation frames stay explicit so the gateway client (and only it)
/// owns reassembly with the same bound; [`read_ws_frame`] never buffers
/// across frames itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WsFrame {
    Text(Vec<u8>, bool),
    Binary(Vec<u8>, bool),
    Continuation(Vec<u8>, bool),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

/// Reads one complete WebSocket frame.
///
/// Rejects reserved bits, fragmented control frames, oversized control
/// payloads, and payloads above `maximum` with typed errors before
/// allocating. A clean end of stream at a frame boundary reports
/// [`WsFrameError::Closed`]; mid-frame truncation reports
/// [`WsFrameError::InvalidFrame`]. Masking applies whenever the mask bit is
/// set, so the same reader serves the client and the fixture server.
///
/// # Errors
///
/// Returns [`WsFrameError`] when the peer closed, the frame is malformed, or
/// the payload exceeds `maximum`.
pub(crate) async fn read_ws_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    maximum: usize,
) -> Result<WsFrame, WsFrameError> {
    let mut header = [0_u8; 2];
    let mut first = [0_u8; 1];
    match reader.read(&mut first).await {
        Ok(0) => return Err(WsFrameError::Closed),
        Ok(_) => {}
        Err(_) => return Err(WsFrameError::InvalidFrame),
    }
    header[0] = first[0];
    reader
        .read_exact(&mut header[1..])
        .await
        .map_err(|_| WsFrameError::InvalidFrame)?;
    let finished = header[0] & 0x80 != 0;
    if header[0] & 0x70 != 0 {
        return Err(WsFrameError::InvalidFrame);
    }
    let opcode = header[0] & 0x0f;
    let masked = header[1] & 0x80 != 0;
    let mut length = u64::from(header[1] & 0x7f);
    if length == 126 {
        let mut extended = [0_u8; 2];
        reader
            .read_exact(&mut extended)
            .await
            .map_err(|_| WsFrameError::InvalidFrame)?;
        length = u64::from(u16::from_be_bytes(extended));
    } else if length == 127 {
        let mut extended = [0_u8; 8];
        reader
            .read_exact(&mut extended)
            .await
            .map_err(|_| WsFrameError::InvalidFrame)?;
        length = u64::from_be_bytes(extended);
    }
    let maximum_u64 = u64::try_from(maximum).unwrap_or(u64::MAX);
    if length > maximum_u64 {
        return Err(WsFrameError::TooLarge);
    }
    let is_control = opcode >= 0x08;
    if is_control && (!finished || length > 125) {
        return Err(WsFrameError::InvalidFrame);
    }
    let mask = if masked {
        let mut key = [0_u8; 4];
        reader
            .read_exact(&mut key)
            .await
            .map_err(|_| WsFrameError::InvalidFrame)?;
        Some(key)
    } else {
        None
    };
    let size = usize::try_from(length).map_err(|_| WsFrameError::TooLarge)?;
    let mut payload = vec![0_u8; size];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|_| WsFrameError::InvalidFrame)?;
    if let Some(key) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= key[index % 4];
        }
    }
    match opcode {
        0x00 => Ok(WsFrame::Continuation(payload, finished)),
        0x01 => Ok(WsFrame::Text(payload, finished)),
        0x02 => Ok(WsFrame::Binary(payload, finished)),
        0x08 => Ok(WsFrame::Close),
        0x09 => Ok(WsFrame::Ping(payload)),
        0x0a => Ok(WsFrame::Pong(payload)),
        _ => Err(WsFrameError::InvalidFrame),
    }
}

/// Writes one masked client text frame.
///
/// Clients must mask; the mask comes from operating-system entropy. Payloads
/// above `maximum` reject before any byte is written.
///
/// # Errors
///
/// Returns [`WsFrameError`] when entropy is unavailable, the payload exceeds
/// `maximum`, or the write fails.
pub(crate) async fn write_client_text<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
    maximum: usize,
) -> Result<(), WsFrameError> {
    if payload.len() > maximum {
        return Err(WsFrameError::TooLarge);
    }
    let mut mask = [0_u8; 4];
    getrandom::fill(&mut mask).map_err(|_| WsFrameError::InvalidFrame)?;
    write_frame(writer, 0x81, Some(mask), payload)
        .await
        .map_err(|_| WsFrameError::InvalidFrame)
}

/// Writes one unmasked server text frame.
///
/// Fixture servers only: production traffic is always client-masked.
///
/// # Errors
///
/// Returns [`WsFrameError`] when the payload exceeds `maximum` or the write
/// fails.
#[cfg(test)]
pub(crate) async fn write_server_text<W: AsyncWrite + Unpin>(
    writer: &mut W,
    payload: &[u8],
    maximum: usize,
) -> Result<(), WsFrameError> {
    if payload.len() > maximum {
        return Err(WsFrameError::TooLarge);
    }
    write_frame(writer, 0x81, None, payload)
        .await
        .map_err(|_| WsFrameError::InvalidFrame)
}

/// Writes one masked client control frame (pong or close).
pub(super) async fn write_client_control<W: AsyncWrite + Unpin>(
    writer: &mut W,
    opcode: u8,
    payload: &[u8],
) -> Result<(), WsFrameError> {
    if payload.len() > 125 {
        return Err(WsFrameError::InvalidFrame);
    }
    let mut mask = [0_u8; 4];
    getrandom::fill(&mut mask).map_err(|_| WsFrameError::InvalidFrame)?;
    write_frame(writer, 0x80 | opcode, Some(mask), payload)
        .await
        .map_err(|_| WsFrameError::InvalidFrame)
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    first: u8,
    mask: Option<[u8; 4]>,
    payload: &[u8],
) -> std::io::Result<()> {
    let length = payload.len();
    let mut header = vec![first];
    if length < 126 {
        let size = u8::try_from(length).unwrap_or(u8::MAX);
        header.push(size | if mask.is_some() { 0x80 } else { 0 });
    } else if length < 65536 {
        let size = u16::try_from(length).unwrap_or(u16::MAX);
        header.push(0x7e | if mask.is_some() { 0x80 } else { 0 });
        header.extend_from_slice(&size.to_be_bytes());
    } else {
        let size = u64::try_from(length).unwrap_or(u64::MAX);
        header.push(127 | if mask.is_some() { 0x80 } else { 0 });
        header.extend_from_slice(&size.to_be_bytes());
    }
    writer.write_all(&header).await?;
    if let Some(key) = mask {
        writer.write_all(&key).await?;
        let masked: Vec<u8> = payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ key[index % 4])
            .collect();
        writer.write_all(&masked).await?;
    } else {
        writer.write_all(payload).await?;
    }
    writer.flush().await
}

/// Payload-free failure of the gateway JSON-RPC boundary.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum GatewayError {
    #[error("hermes gateway handshake failed")]
    Handshake,
    #[error("hermes gateway request timed out")]
    Timeout,
    #[error("hermes gateway connection closed")]
    Closed,
    #[error("hermes gateway frame was malformed")]
    Decode,
    #[error("hermes gateway protocol violated")]
    Protocol,
    #[error("hermes gateway rejected the request")]
    Remote { code: Option<i64> },
    #[error("hermes gateway operation cancelled")]
    Cancelled,
    #[error("owner is shutting down")]
    Shutdown,
}

/// Deadline and cancellation scope for one gateway request.
pub(crate) struct RequestScope<'a> {
    pub(crate) deadline: Instant,
    pub(crate) cancel: &'a CancelHandle,
    pub(crate) shutdown: &'a CancelHandle,
}

/// One typed gateway event after bounded boundary decoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesEvent {
    event_type: String,
    session_id: Option<String>,
    payload: Value,
}

impl HermesEvent {
    /// Returns the gateway event type.
    pub(crate) fn event_type(&self) -> &str {
        &self.event_type
    }

    /// Returns the gateway session identity, when the event carried one.
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Returns the raw event payload for typed extraction.
    pub(crate) fn payload(&self) -> &Value {
        &self.payload
    }
}

/// One decoded JSON-RPC text payload.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DecodedEnvelope {
    Response {
        id: u64,
        result: Value,
        error_code: Option<i64>,
    },
    Event(HermesEvent),
    Ignored,
}

/// Decodes one gateway text payload into a typed envelope.
///
/// Oversized input rejects before parsing; invalid JSON, non-object
/// envelopes, and over-long event types reject with typed errors and never
/// panic. Unknown methods and foreign response ids project to
/// [`DecodedEnvelope::Ignored`] so future gateway additions stay observable
/// without disturbing the turn.
///
/// # Errors
///
/// Returns [`WsFrameError`] when the payload exceeds the frame bound, is not
/// valid JSON, or is not a JSON-RPC envelope.
pub(crate) fn decode_envelope(text: &str) -> Result<DecodedEnvelope, WsFrameError> {
    if text.len() > HERMES_MAX_FRAME_BYTES {
        return Err(WsFrameError::TooLarge);
    }
    let value: Value = serde_json::from_str(text).map_err(|_| WsFrameError::InvalidFrame)?;
    let object = value.as_object().ok_or(WsFrameError::InvalidFrame)?;
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        if method != "event" {
            return Ok(DecodedEnvelope::Ignored);
        }
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let params_object = params.as_object().ok_or(WsFrameError::InvalidFrame)?;
        let event_type = params_object
            .get("type")
            .and_then(Value::as_str)
            .ok_or(WsFrameError::InvalidFrame)?;
        if event_type.is_empty() || event_type.len() > HERMES_MAX_ID_BYTES {
            return Err(WsFrameError::InvalidFrame);
        }
        let session_id = params_object
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)
            .map(str::to_owned);
        let payload = params_object.get("payload").cloned().unwrap_or(Value::Null);
        return Ok(DecodedEnvelope::Event(HermesEvent {
            event_type: event_type.to_owned(),
            session_id,
            payload,
        }));
    }
    let Some(id) = object.get("id").and_then(Value::as_u64) else {
        return Ok(DecodedEnvelope::Ignored);
    };
    if let Some(error) = object.get("error") {
        let code = error.get("code").and_then(Value::as_i64);
        return Ok(DecodedEnvelope::Response {
            id,
            result: Value::Null,
            error_code: Some(code.unwrap_or(0)),
        });
    }
    Ok(DecodedEnvelope::Response {
        id,
        result: object.get("result").cloned().unwrap_or(Value::Null),
        error_code: None,
    })
}

pub(super) fn frame_to_gateway(error: WsFrameError) -> GatewayError {
    match error {
        WsFrameError::Closed => GatewayError::Closed,
        WsFrameError::TooLarge => GatewayError::Protocol,
        WsFrameError::InvalidFrame => GatewayError::Decode,
    }
}
