//! Typed failures crossing the transport lifecycle boundary.

use std::time::Duration;

use quinn::{ConnectError, ConnectionError};
use thiserror::Error;

/// Failure modes raised by endpoint construction, connection establishment,
/// and deterministic shutdown. Stream-level framing failures stay in
/// [`FrameError`](crate::FrameError); this enum owns the connection around
/// them.
#[derive(Debug, Error)]
pub enum TransportError {
    /// TLS material or protocol-version selection was rejected.
    #[error("TLS configuration failed: {0}")]
    Tls(#[source] rustls::Error),

    /// The rustls configuration cannot serve QUIC because the provider
    /// lacks the mandatory TLS 1.3 AES-128-GCM-SHA256 suite.
    #[error(transparent)]
    Crypto(#[from] quinn::crypto::rustls::NoInitialCipherSuite),

    /// Binding a loopback socket failed.
    #[error("binding the loopback endpoint failed: {0}")]
    Bind(#[source] std::io::Error),

    /// Opening a connection to the peer failed before any stream existed.
    #[error("connecting to the server failed: {0}")]
    Connect(#[from] ConnectError),

    /// An established connection was lost or closed by either side.
    #[error(transparent)]
    Connection(#[from] ConnectionError),

    /// The endpoint did not drain its connections within the shutdown limit.
    #[error("endpoint shutdown exceeded its {}ms limit", limit.as_millis())]
    ShutdownTimeout {
        /// The deadline granted to graceful drain.
        limit: Duration,
    },
}
