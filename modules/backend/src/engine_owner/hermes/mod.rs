//! Finite Hermes owner runtime over the private-service WebSocket gateway.
//!
//! Spawns the resolved Hermes CLI as a private loopback service
//! (`serve --host 127.0.0.1 --port 0`), parses the bounded
//! `HERMES_BACKEND_READY port=N` readiness record, connects a minimal
//! RFC 6455 JSON-RPC client to `/api/ws`, validates the live `model.options`
//! inventory before opening anything, creates or resumes exactly one session
//! with the guidance seed, normalizes streaming frames onto the shared S1a
//! observation vocabulary (`TextDelta` / `Terminal` plus cumulative usage and
//! subagent lifecycle rows), and supports steer, interrupt/cancel, and close
//! with child-custody teardown that quarantines on unobserved reaps.
//!
//! The wire boundary is typed: inbound text frames decode into
//! [`DecodedEnvelope`] through bounded `serde_json::Value` extraction. Only
//! validated identities and text cross the boundary: approval and question
//! frames become [`HermesApprovalRequest`] / [`HermesQuestionRequest`] and map
//! onto domain `ApprovalRequest` / `QuestionInput` constructors for the
//! durable A-approve resolve path. Deny lands with no side effect while the
//! turn continues; allow answers through the same durable path.
//!
//! Hermes carries no new dependencies: the gateway client is a small
//! loopback-only WebSocket implementation over Tokio TCP (masked client
//! frames, 16 MiB cap mirroring the TypeScript transport, ping/pong, close).
//! The `Sec-WebSocket-Accept` check needs SHA-1, which no workspace crate
//! provides, so a local SHA-1 over the handshake key is implemented here and
//! pinned against the RFC 6455 test vector. Authentication stays
//! profile-owned per the engine descriptor: the service spawn inherits the
//! ambient environment plus the dashboard session token, and no credential is
//! plumbed, synthesized, or logged.
//!
//! Images are rejected with [`HermesTurnError::ImagesUnsupported`]: the
//! Hermes catalog reports `image_input: false`, so an image attachment fails
//! the turn closed instead of sending a degraded text-only prompt.
//!
//! Tool frames are tracked by identity only and never adopt the root turn;
//! the shared S1a vocabulary has no tool row, so tool projections stay a
//! later packet. Compaction markers clear internal state without emitting
//! rows for the same reason.

#![forbid(unsafe_code)]

mod adapter;
mod gateway;
mod inventory;

#[cfg(test)]
pub(crate) use self::adapter::{
    HERMES_CONTINUATION_MINIMUM_SERVICE_VERSION, compare_hermes_service_versions,
    hermes_requires_group_termination, hermes_service_meets_minimum, parse_ready_port_line,
    read_ready_port,
};
pub(crate) use self::adapter::{
    HermesContinuationDecision, HermesContinuationGateInput, HermesSettings, HermesTurnError,
    VerifiedHermesLaunch, check_hermes_native_continuation, drive_service_readiness,
    new_session_token, reject_image_attachments, resolve_service_executable,
};
pub(crate) use self::gateway::{
    ApplyContext, GatewayClient, GatewayError, HermesEvent, HermesNormalizer, HermesPendingTracker,
    OpenSessionInput, RequestScope, SessionError, apply_observations, has_stalled,
    interrupt_live_turn, open_session, steer_live_turn,
};
#[cfg(test)]
pub(crate) use self::gateway::{
    DecodedEnvelope, HERMES_MAX_ANSWERS, HERMES_MAX_FRAME_BYTES, WsFrame, WsFrameError,
    answer_approval, answer_questions, decode_approval, decode_envelope, decode_questions,
    guidance_seed_messages, read_ws_frame, resume_selection_matches, usage_report, usage_sample,
    websocket_accept_key, write_client_text, write_server_text,
};
pub(crate) use self::inventory::{
    InventoryError, inventory_supports, validate_model_options_inventory,
};
