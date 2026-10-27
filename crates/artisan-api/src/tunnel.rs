//! Tunnel application envelope: request/response JSON inside the Noise
//! data channel. Relay sees only opaque bodies; this framing is Forge's
//! own application protocol (v1: unary calls, chunked bodies, event
//! streams; see `docs/plans/ast-account-wire.md`).
//!
//! Strict requests (unknown fields rejected), forward-compatible reads.
//! `id` is caller-made and echoed back for pairing. `path` is origin-form
//! only: never a full URL, never credentials, never `..`.

use serde::{Deserialize, Serialize};

/// One application call inside an open Noise data channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelRequest {
    /// Caller-made id, echoed back for pairing.
    pub id: String,
    /// `GET`, `POST`, `PUT`, `PATCH` or `DELETE`.
    pub method: String,
    /// Origin-form path, e.g. `/state` or `/threads/abc/items?limit=50`.
    /// Never a full URL; never credentials, never `..`.
    pub path: String,
    /// JSON body for `POST`/`PUT`/`PATCH`; absent otherwise. Binary uploads
    /// use the `$bytes` envelope (see [`TunnelBytes`]) instead of raw JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

/// Binary body envelope.
///
/// A binary body inside [`TunnelRequest::body`] or [`TunnelResponse::body`]:
/// `{"$bytes": {"content_type": "<mime>", "b64u": "<base64url>"}}`.
/// Used for corpus that is not JSON (`PUT /blobs` WebP, `GET /blobs/*`,
/// exported sources). Keeps every tunnel frame JSON while carrying bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelBytes {
    pub content_type: String,
    pub b64u: String,
}

/// The envelope key naming a [`TunnelBytes`] value.
pub const BYTES_ENVELOPE_KEY: &str = "$bytes";

/// Split a request body into raw bytes.
///
/// The `$bytes` envelope form carries binary with its content type,
/// anything else is re-encoded as JSON.
/// Corrupt base64 is an explicit error, never silently empty bytes.
pub fn request_body_bytes(
    body: Option<&serde_json::Value>,
) -> Result<(String, Vec<u8>), BodyDecodeError> {
    match body {
        Some(serde_json::Value::Object(map))
            if map.len() == 1 && map.contains_key(BYTES_ENVELOPE_KEY) =>
        {
            match serde_json::from_value::<TunnelBytes>(map[BYTES_ENVELOPE_KEY].clone()) {
                Ok(envelope) => {
                    use base64::Engine as _;
                    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(envelope.b64u.as_bytes())
                        .map_err(|_decode| BodyDecodeError::InvalidBase64)?;
                    Ok((envelope.content_type, raw))
                }
                Err(_) => Ok(("application/json".into(), body_json_bytes(body))),
            }
        }
        _ => Ok(("application/json".into(), body_json_bytes(body))),
    }
}

/// Decoding a `$bytes` envelope or chunk failed: the bytes are corrupt.
/// Callers must fail the transfer, never substitute empty bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BodyDecodeError {
    #[error("invalid base64url in a $bytes envelope or chunk")]
    InvalidBase64,
}

fn body_json_bytes(body: Option<&serde_json::Value>) -> Vec<u8> {
    body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(value).unwrap_or_default()
    })
}

/// The answer to a [`TunnelRequest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelResponse {
    /// Echo of the request id.
    pub id: String,
    /// HTTP-style status the local route produced.
    pub status: u16,
    /// JSON body (Forge error shape on failure), or a `$bytes` envelope
    /// when the local route answered non-JSON bytes (see `content_type`).
    pub body: serde_json::Value,
    /// Local response MIME when `body` carries a `$bytes` envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

impl TunnelResponse {
    #[must_use]
    pub fn ok(id: &str, body: serde_json::Value) -> Self {
        Self {
            id: id.to_owned(),
            status: 200,
            body,
            content_type: None,
        }
    }

    #[must_use]
    pub fn error(id: &str, status: u16, message: &str) -> Self {
        Self {
            id: id.to_owned(),
            status,
            body: serde_json::json!({"error": {"code": "tunnel_error", "message": message}}),
            content_type: None,
        }
    }
}

/// One chunk of a large transfer.
///
/// Every chunk frame stays JSON and under the 60 KiB Noise plaintext cap
/// (see the wire doc for the protocol).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelChunk {
    /// Transfer this chunk belongs to (caller-made, e.g. the request id).
    pub transfer: String,
    /// Zero-based part index; frames arrive in Noise order.
    pub index: u32,
    /// Total part count; the transfer completes at `index == total - 1`.
    pub total: u32,
    /// Total reassembled byte count (receiver verifies before parsing).
    pub total_bytes: u64,
    /// This part's bytes, base64url.
    pub bytes_b64u: String,
}

/// Chunking limits.
///
/// 40 KiB of raw bytes per chunk. Base64url expands 40 KiB to ~54.6 KiB
/// of text, so even the largest chunk frame stays under the 60 KiB Noise
/// plaintext cap after JSON framing (asserted in tests against
/// [`TUNNEL_PLAINTEXT_BUDGET`]).
pub const CHUNK_RAW_BYTES: usize = 40 * 1024;
/// Noise data-plaintext ceiling every chunk frame must fit.
pub const TUNNEL_PLAINTEXT_BUDGET: usize = 60_000;
/// Transfer size cap.
///
/// 32 MiB of raw bytes per transfer max (matches the Forge blob/source
/// ceilings so no transcript becomes unreadable: oversize is an explicit
/// error, never truncation). Encoded inflight runs ~4/3 of this; the
/// per-connection aggregate cap below bounds the expansion.
pub const TRANSFER_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// Per-connection aggregate cap across all in-progress chunk transfers
/// (bounds base64 expansion + JSON overhead, not only each transfer).
pub const REASSEMBLY_MAX_BYTES_PER_CONN: u64 = 64 * 1024 * 1024;
/// Maximum chunk parts per transfer.
///
/// `TRANSFER_MAX_BYTES` split into `CHUNK_RAW_BYTES` pieces, rounded up.
/// Validated BEFORE allocating the parts vector, so a corrupt `total`
/// can never trigger a huge allocation.
pub const TRANSFER_MAX_PARTS: u32 = 820;
const _: () = assert!(
    TRANSFER_MAX_PARTS as u64 == TRANSFER_MAX_BYTES.div_ceil(CHUNK_RAW_BYTES as u64),
    "TRANSFER_MAX_PARTS must be the div_ceil part count"
);
/// Maximum concurrent in-progress chunk transfers per reassembly scope.
/// Bounds memory when many partial transfers arrive without completing.
pub const REASSEMBLY_MAX_IN_FLIGHT: usize = 64;

/// Split bytes into [`TunnelChunk`] frames for one transfer.
#[must_use]
pub fn chunk_bytes(transfer: &str, bytes: &[u8]) -> Vec<TunnelChunk> {    use base64::Engine as _;
    if bytes.is_empty() {
        return vec![TunnelChunk {
            transfer: transfer.to_owned(),
            index: 0,
            total: 1,
            total_bytes: 0,
            bytes_b64u: String::new(),
        }];
    }
    let total = bytes.len().div_ceil(CHUNK_RAW_BYTES);
    bytes
        .chunks(CHUNK_RAW_BYTES)
        .enumerate()
        .map(|(index, part)| TunnelChunk {
            transfer: transfer.to_owned(),
            index: u32::try_from(index).unwrap_or(u32::MAX),
            total: u32::try_from(total).unwrap_or(u32::MAX),
            total_bytes: bytes.len() as u64,
            bytes_b64u: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(part),
        })
        .collect()
}

/// Decode one chunk's payload: corrupt base64 fails the transfer,
/// never substitutes empty bytes.
pub fn decode_chunk_bytes(chunk: &TunnelChunk) -> Result<Vec<u8>, BodyDecodeError> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(chunk.bytes_b64u.as_bytes())
        .map_err(|_decode| BodyDecodeError::InvalidBase64)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;

    #[test]
    fn chunks_round_trip_and_fit_the_noise_plaintext_cap() {
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let chunks = chunk_bytes("t1", &bytes);
        assert!(chunks.len() > 1);
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index as usize, index);
            assert_eq!(chunk.total as usize, chunks.len());
            assert_eq!(chunk.total_bytes, bytes.len() as u64);
            let framed = serde_json::to_vec(chunk).unwrap();
            assert!(
                framed.len() <= TUNNEL_PLAINTEXT_BUDGET,
                "chunk {} serializes to {} bytes, over the Noise cap",
                index,
                framed.len()
            );
            assert!(decode_chunk_bytes(chunk).unwrap().len() <= CHUNK_RAW_BYTES);
        }
        let mut reassembled = Vec::new();
        for chunk in &chunks {
            reassembled.extend_from_slice(&decode_chunk_bytes(chunk).unwrap());
        }
        assert_eq!(reassembled, bytes);
    }

    #[test]
    fn corrupt_envelopes_fail_loudly() {
        let bad = TunnelChunk {
            transfer: "t".into(),
            index: 0,
            total: 1,
            total_bytes: 3,
            bytes_b64u: "!!!not-base64!!!".into(),
        };
        assert_eq!(
            decode_chunk_bytes(&bad).unwrap_err(),
            BodyDecodeError::InvalidBase64
        );
        let body = serde_json::json!({"$bytes": {"content_type": "image/webp", "b64u": "%%%"}});
        assert_eq!(
            request_body_bytes(Some(&body)).unwrap_err(),
            BodyDecodeError::InvalidBase64
        );
        // A well-formed envelope decodes exactly.
        let body = serde_json::json!({"$bytes": {"content_type": "image/webp", "b64u": "aGk"}});
        assert_eq!(
            request_body_bytes(Some(&body)).unwrap(),
            ("image/webp".to_owned(), b"hi".to_vec())
        );
        // Plain JSON bodies pass through as JSON.
        let body = serde_json::json!({"text": "hi"});
        let (content_type, raw) = request_body_bytes(Some(&body)).unwrap();
        assert_eq!(content_type, "application/json");
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&raw).unwrap(), body);
    }
}

/// Stream start answer.
///
/// A `SUBSCRIBE` request answers `{"$stream": {"stream": "<id>"}}`
/// as the response `body`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamStart {
    pub stream: String,
}

/// Stream envelope key in a subscribe response body.
pub const STREAM_START_KEY: &str = "$stream";

/// Tunneled stream frame.
///
/// One tunneled stream frame (server → client DATA plaintext):
/// `{"$event": {"stream": "<id>", ...}}` with exactly one of
/// `envelope` (one stored/live [`super::Envelope`]), `end`
/// (terminal reason), or `error` (terminal failure).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEvent {
    pub stream: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<super::Envelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Stream envelope key in DATA frames.
pub const STREAM_EVENT_KEY: &str = "$event";

/// Request methods beyond plain unary calls.
pub const METHOD_SUBSCRIBE: &str = "SUBSCRIBE";
pub const METHOD_UNSUBSCRIBE: &str = "UNSUBSCRIBE";
/// Subscribe path: `/events?since=N[&summary=true][&thread=<id>]`.
pub const EVENTS_PATH: &str = "/events";

/// Per-stream queue bound (envelopes). Overflow ends the stream with a
/// `lagged` reset so the client re-subscribes from its last seq —
/// the standard `since` cursor, no new bookkeeping.
pub const STREAM_QUEUE_ENVELOPES: usize = 256;
