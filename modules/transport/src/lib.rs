//! Direct QUIC transport for the Artisan application protocol.
//!
//! The low-level layers own bounded length-prefixed framing over Quinn streams
//! ([`frame`]), typed transport failures ([`TransportError`]), endpoint
//! construction with deterministic lifecycle helpers ([`endpoint`]), and the
//! sole conversion seam between bounded streams and owned application
//! envelopes ([`application`]), plus contiguous per-session ordering for
//! Forge-originated server events ([`event_sequence`]). Session
//! authentication, request coordination, reconnect policy, and
//! frontend-facing channels build above these layers.

pub mod application;
pub mod endpoint;
pub mod error;
pub mod event_sequence;
pub mod frame;
pub mod handshake;
pub mod request_correlation;

pub use application::{EnvelopeReceiveError, EnvelopeSendError, receive_envelope, send_envelope};
pub use endpoint::{
    ALPN_PROTOCOL, LOOPBACK_SERVER_NAME, bind_loopback_client, bind_loopback_server, client_config,
    connect, server_config, shutdown,
};
pub use error::TransportError;
pub use event_sequence::{EventSequenceError, EventSequenceTracker, validate_event_successor};
pub use frame::{FrameError, MAX_FRAME_LEN, read_frame, write_frame};
pub use handshake::{
    ClientHello, HandshakeError, HandshakeMessageKind, ServerWelcome, client_handshake,
    receive_client_hello, send_server_welcome,
};
pub use request_correlation::{RequestCorrelationError, RequestCorrelationRegistry};
