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
    pub transfer: String,
    pub index: u32,
    pub total: u32,
    pub total_bytes: u64,
    pub bytes_b64u: String,
}

pub const CHUNK_RAW_BYTES: usize = 40 * 1024;
pub const TUNNEL_PLAINTEXT_BUDGET: usize = 60_000;
pub const TRANSFER_MAX_BYTES: u64 = 32 * 1024 * 1024;
pub const REASSEMBLY_MAX_BYTES_PER_CONN: u64 = 64 * 1024 * 1024;
pub const TRANSFER_MAX_PARTS: u32 = 820;
const _: () = assert!(
    TRANSFER_MAX_PARTS as u64 == TRANSFER_MAX_BYTES.div_ceil(CHUNK_RAW_BYTES as u64),
    "TRANSFER_MAX_PARTS must be the div_ceil part count"
);
pub const REASSEMBLY_MAX_IN_FLIGHT: usize = 64;

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
        let body = serde_json::json!({"$bytes": {"content_type": "image/webp", "b64u": "aGk"}});
        assert_eq!(
            request_body_bytes(Some(&body)).unwrap(),
            ("image/webp".to_owned(), b"hi".to_vec())
        );
        let body = serde_json::json!({"text": "hi"});
        let (content_type, raw) = request_body_bytes(Some(&body)).unwrap();
        assert_eq!(content_type, "application/json");
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&raw).unwrap(), body);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamStart {
    pub stream: String,
}

pub const STREAM_START_KEY: &str = "$stream";

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
pub const EVENTS_PATH: &str = "/events";

pub const STREAM_QUEUE_ENVELOPES: usize = 256;
