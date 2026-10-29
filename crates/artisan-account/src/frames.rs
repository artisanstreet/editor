use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientFrame {
    Hello(HelloFrame),
    Authenticate(AuthenticateFrame),
    Open(OpenFrame),
    Data(DataFrame),
    Ack(AckFrame),
    Close(CloseFrame),
    Pong(PongFrame),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloFrame {
    pub v: u32,
    pub grant: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticateFrame {
    pub v: u32,
    pub jti: String,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerFrame {
    Challenge(ChallengeFrame),
    Ready(ReadyFrame),
    ChannelOpened(ChannelOpenedFrame),
    Data(DataFrame),
    ChannelClosed(ChannelClosedFrame),
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

pub fn read_server_frame(value: serde_json::Value) -> Result<ServerFrame, serde_json::Error> {
    serde_json::from_value(value)
}
