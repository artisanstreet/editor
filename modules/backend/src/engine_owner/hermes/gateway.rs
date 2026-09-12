//! Hermes gateway/session boundary: the minimal RFC 6455 JSON-RPC client,
//! typed envelope decoding, session lifecycle verbs, pending interactions,
//! usage sampling, and the streaming observation normalizer.

mod projection;
mod session;
mod wire;

use std::time::Duration;

/// Maximum accepted gateway frame bytes (mirrors the TS 16 MiB transport cap).
pub(crate) const HERMES_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Maximum UTF-8 bytes retained for one approval/question text field.
const HERMES_MAX_TEXT_FIELD_BYTES: usize = 8 * 1024;

/// Maximum questions retained per `clarify.request` frame.
const HERMES_MAX_QUESTIONS_PER_FRAME: usize = 32;

/// Maximum options retained per question (the domain observation ceiling).
const HERMES_MAX_OPTIONS_PER_QUESTION: usize = 16;

/// Maximum answers retained per question response (the domain ceiling).
#[cfg(test)]
pub(crate) const HERMES_MAX_ANSWERS: usize = 16;

/// Maximum bytes accepted for one provider identity field.
const HERMES_MAX_ID_BYTES: usize = 256;

/// Maximum bytes for the HTTP upgrade head.
const HERMES_MAX_HANDSHAKE_BYTES: usize = 8 * 1024;

/// Gateway JSON-RPC request timeout (mirrors the TS 60s request timeout).
const HERMES_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Minimum accepted desktop contract (mirrors the TS contract-4 floor).
const HERMES_MINIMUM_DESKTOP_CONTRACT: u64 = 4;

/// WebSocket globally unique identifier from RFC 6455 section 1.3.
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub(crate) use self::projection::{
    ApplyContext, HermesNormalizer, HermesPendingTracker, apply_observations,
};
pub(crate) use self::session::{
    GatewayClient, OpenSessionInput, SessionError, has_stalled, interrupt_live_turn, open_session,
    steer_live_turn,
};
pub(crate) use self::wire::{GatewayError, HermesEvent, RequestScope};

#[cfg(test)]
pub(crate) use self::projection::{
    answer_approval, answer_questions, decode_approval, decode_questions, usage_report,
    usage_sample,
};
#[cfg(test)]
pub(crate) use self::session::{guidance_seed_messages, resume_selection_matches};
#[cfg(test)]
pub(crate) use self::wire::{
    DecodedEnvelope, WsFrame, WsFrameError, decode_envelope, read_ws_frame, websocket_accept_key,
    write_client_text, write_server_text,
};
