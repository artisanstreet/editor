use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelRequest {
    pub id: String,
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelBytes {
    pub content_type: String,
    pub b64u: String,
}

pub const BYTES_ENVELOPE_KEY: &str = "$bytes";

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelResponse {
    pub id: String,
    pub status: u16,
    pub body: serde_json::Value,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelChunk {
    pub(crate) transfer: String,
    pub(crate) index: u32,
    pub(crate) total: u32,
    pub(crate) total_bytes: u64,
    pub(crate) bytes_b64u: String,
}

pub(crate) const CHUNK_RAW_BYTES: usize = 40 * 1024;
pub const TUNNEL_PLAINTEXT_BUDGET: usize = 60_000;
pub const TUNNEL_BODY_MAX_BYTES: usize = 32 * 1024 * 1024;
const MESSAGE_ENVELOPE_BYTES: u64 = 64 * 1024;
pub const TRANSFER_MAX_BYTES: u64 =
    (TUNNEL_BODY_MAX_BYTES as u64).div_ceil(3) * 4 + MESSAGE_ENVELOPE_BYTES;
pub const REASSEMBLY_MAX_BYTES_PER_CONN: u64 = 64 * 1024 * 1024;
pub(crate) const TRANSFER_MAX_PARTS: u32 = 1094;
const _: () = assert!(
    TRANSFER_MAX_PARTS as u64 == TRANSFER_MAX_BYTES.div_ceil(CHUNK_RAW_BYTES as u64),
    "TRANSFER_MAX_PARTS must be the div_ceil part count"
);
const _: () = assert!(
    TRANSFER_MAX_BYTES <= REASSEMBLY_MAX_BYTES_PER_CONN,
    "a connection must hold one transfer of the largest body"
);
pub(crate) const REASSEMBLY_MAX_IN_FLIGHT: usize = 64;

#[must_use]
pub const fn chunk_count(bytes: usize) -> usize {
    if bytes == 0 {
        1
    } else {
        bytes.div_ceil(CHUNK_RAW_BYTES)
    }
}

#[must_use]
pub fn chunk_at(transfer: &str, bytes: &[u8], index: usize) -> TunnelChunk {
    use base64::Engine as _;
    let start = index.saturating_mul(CHUNK_RAW_BYTES).min(bytes.len());
    let end = start.saturating_add(CHUNK_RAW_BYTES).min(bytes.len());
    TunnelChunk {
        transfer: transfer.to_owned(),
        index: u32::try_from(index).unwrap_or(u32::MAX),
        total: u32::try_from(chunk_count(bytes.len())).unwrap_or(u32::MAX),
        total_bytes: bytes.len() as u64,
        bytes_b64u: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes[start..end]),
    }
}

#[must_use]
pub fn chunk_bytes(transfer: &str, bytes: &[u8]) -> Vec<TunnelChunk> {
    (0..chunk_count(bytes.len()))
        .map(|index| chunk_at(transfer, bytes, index))
        .collect()
}

pub(crate) fn decode_chunk_bytes(chunk: &TunnelChunk) -> Result<Vec<u8>, BodyDecodeError> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(chunk.bytes_b64u.as_bytes())
        .map_err(|_decode| BodyDecodeError::InvalidBase64)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test fixtures; failures fail the test")]
#[path = "../tests/tunnel/unit.rs"]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamStart {
    pub stream: String,
}

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

pub const STREAM_EVENT_KEY: &str = "$event";

pub const METHOD_SUBSCRIBE: &str = "SUBSCRIBE";
pub const METHOD_UNSUBSCRIBE: &str = "UNSUBSCRIBE";
