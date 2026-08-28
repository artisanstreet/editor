//! Direct QUIC transport for the Artisan application protocol.
//!
//! Phase 1 owns three concerns only: bounded length-prefixed framing over
//! Quinn streams ([`frame`]), typed transport failures ([`TransportError`]),
//! and endpoint construction with deterministic lifecycle helpers
//! ([`endpoint`]). Certificates, trust material, and connection
//! orchestration stay with callers until the production trust and
//! authentication model is decided; nothing here authenticates, pairs,
//! reconnects, or interprets product messages.

pub mod endpoint;
pub mod error;
pub mod frame;

pub use endpoint::{
    ALPN_PROTOCOL, LOOPBACK_SERVER_NAME, bind_loopback_client, bind_loopback_server, client_config,
    connect, server_config, shutdown,
};
pub use error::TransportError;
pub use frame::{FrameError, MAX_FRAME_LEN, read_frame, write_frame};
