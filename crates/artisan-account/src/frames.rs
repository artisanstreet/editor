//! Relay admission + channel framing (no E2E data crypto).
//!
//! Client -> server reads are strict (unknown fields rejected); server ->
//! client reads are forward-compatible. `data.body` is opaque base64url to
//! this crate: the inner channel cipher arrives with the HPKE-or-Noise
//! decision and is NOT implemented here.

use serde::{Deserialize, Serialize};

/// `^[A-Za-z0-9][A-Za-z0-9._~:-]{0,63}$`.
#[must_use]
pub fn is_valid_channel(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    rest.iter().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'.' | b'_' | b'~' | b':' | b'-')
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelId(pub String);

/// A base64url body up to ~256 KiB of ciphertext.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Body(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientFrame {
    Hello(HelloFrame),
    Authenticate(AuthenticateFrame),
    Open(OpenFrame),
    Data(DataFrame),
    /// Cumulative delivery ack: frames with seq ≤ ack toward the acker.
    /// Required after every received data frame; Relay drops senders that
    /// stall the window (64 frames / 1 MiB per direction).
    Ack(AckFrame),
    Close(CloseFrame),
    Pong(PongFrame),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloFrame {
    pub v: u32,
    /// Core-signed grant JWT (in-band; never in URL or headers).
    pub grant: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticateFrame {
    pub v: u32,
    pub jti: String,
    /// PoP signature, base64url.
    pub sig: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenFrame {
    pub v: u32,
    pub channel: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataFrame {
    pub v: u32,
    pub channel: String,
    pub seq: u64,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseFrame {
    pub v: u32,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Cumulative ack for received data on a channel (`wire.md` flow control).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckFrame {
    pub v: u32,
    pub channel: String,
    pub seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PongFrame {
    pub v: u32,
    pub t: i64,
}

/// Server -> client: tolerant reads (unknown fields ignored).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerFrame {
    Challenge(ChallengeFrame),
    Ready(ReadyFrame),
    #[serde(rename = "channel-opened")]
    ChannelOpened(ChannelOpenedFrame),
    Data(DataFrame),
    #[serde(rename = "channel-closed")]
    ChannelClosed(ChannelClosedFrame),
    /// Cumulative delivery ack forwarded by the room to the original
    /// sender (slides our send window; never ack an ack).
    Ack(AckFrame),
    Presence(PresenceFrame),
    Error(ErrorFrame),
    Ping(PingFrame),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChallengeFrame {
    pub v: u32,
    pub nonce: String,
    pub timeout_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyFrame {
    pub v: u32,
    pub role: String,
    pub account: String,
    pub machine: String,
    pub forge_online: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelOpenedFrame {
    pub v: u32,
    pub channel: String,
    pub peer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelClosedFrame {
    pub v: u32,
    pub channel: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceFrame {
    pub v: u32,
    pub forge_online: bool,
    pub clients: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorFrame {
    pub v: u32,
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingFrame {
    pub v: u32,
    pub t: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("unknown fields or bad shape")]
    Invalid,
}

/// Strict decode of a client -> server frame.
#[expect(
    clippy::map_err_ignore,
    reason = "frame errors are coarse Invalid to avoid leaking peer bytes"
)]
pub fn decode_client_frame(value: &serde_json::Value) -> Result<ClientFrame, FrameError> {
    serde_json::from_value(value.clone()).map_err(|_| FrameError::Invalid)
}

/// Forward-compatible read of a server -> client frame.
#[expect(
    clippy::map_err_ignore,
    reason = "frame errors are coarse Invalid to avoid leaking peer bytes"
)]
pub fn read_server_frame(value: serde_json::Value) -> Result<ServerFrame, FrameError> {
    serde_json::from_value(value).map_err(|_| FrameError::Invalid)
}

/// Whether `next` may follow `last` (strictly increasing per channel/dir).
#[must_use]
pub const fn seq_advances(last: Option<u64>, next: u64) -> bool {
    match last {
        None => true,
        Some(last) => next > last,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_client_reads_reject_unknown_fields() {
        let hello = json!({"type": "hello", "v": 1, "grant": "g", "future": 1});
        assert!(decode_client_frame(&hello).is_err());
        let hello = json!({"type": "hello", "v": 1, "grant": "g"});
        assert!(decode_client_frame(&hello).is_ok());
    }

    #[test]
    fn server_reads_tolerate_future_fields() {
        let ping = serde_json::json!({"type": "ping", "v": 1, "t": 5, "future": [1, 2]});
        assert!(read_server_frame(ping).is_ok());
    }

    #[test]
    fn channel_charset_matches_relay_rules() {
        assert!(is_valid_channel("a"));
        assert!(is_valid_channel("ch:1._-~x"));
        assert!(!is_valid_channel(""));
        assert!(!is_valid_channel("-bad"));
        assert!(!is_valid_channel("has space"));
    }

    #[test]
    fn seq_must_strictly_increase() {
        assert!(seq_advances(None, 0));
        assert!(seq_advances(Some(0), 1));
        assert!(!seq_advances(Some(1), 1));
        assert!(!seq_advances(Some(2), 1));
    }
}
