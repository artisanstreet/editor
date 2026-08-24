//! Endpoint construction, loopback binding, and lifecycle helpers.
//!
//! The trust model behind these builders is deliberately minimal: callers
//! hand over DER material, and this module wires ring-backed rustls with a
//! fixed ALPN onto Quinn endpoints bound to `127.0.0.1:0`. Certificate
//! provisioning, peer authentication, pairing, and reconnection are the
//! production trust decision (an open plan decision) and are not invented
//! here.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{
    ClientConfig, Connection, Endpoint, IdleTimeout, ServerConfig, TransportConfig, VarInt,
};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

use crate::error::TransportError;

/// ALPN identifier every Artisan transport connection negotiates.
pub const ALPN_PROTOCOL: &[u8] = b"artisan/1";

/// Server name clients present for certificate validation on loopback.
///
/// Test PKI must issue certificates whose subject alternative names cover
/// this name so loopback handshakes validate against it.
pub const LOOPBACK_SERVER_NAME: &str = "localhost";

/// Finite idle-timeout guard applied to endpoints built here.
///
/// Deterministic shutdown always comes from explicit closes; the finite
/// timeout only bounds how long a silently lost peer can keep an endpoint
/// from reaching idle state on its own.
const IDLE_TIMEOUT_MS: u32 = 30_000;

fn ring_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn transport_config() -> TransportConfig {
    let mut transport = TransportConfig::default();
    let idle_timeout = IdleTimeout::from(VarInt::from_u32(IDLE_TIMEOUT_MS));
    transport.max_idle_timeout(Some(idle_timeout));
    transport
}

/// Builds a QUIC server configuration presenting `certificate_chain` with
/// `private_key`, requiring TLS 1.3 through the ring provider, negotiating
/// [`ALPN_PROTOCOL`], and accepting no client certificate.
///
/// # Errors
///
/// Returns [`TransportError::Tls`] when the versions or key material are
/// rejected by rustls and [`TransportError::Crypto`] when the resulting
/// configuration cannot serve QUIC.
pub fn server_config(
    certificate_chain: Vec<CertificateDer<'static>>,
    private_key: PrivatePkcs8KeyDer<'static>,
) -> Result<ServerConfig, TransportError> {
    let mut tls = rustls::ServerConfig::builder_with_provider(ring_provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(TransportError::Tls)?
        .with_no_client_auth()
        .with_single_cert(certificate_chain, private_key.into())
        .map_err(TransportError::Tls)?;
    tls.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];

    let crypto = QuicServerConfig::try_from(tls)?;
    let mut config = ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config()));
    Ok(config)
}

/// Builds a QUIC client configuration trusting exactly `trusted_root`,
/// requiring TLS 1.3 through the ring provider, and offering
/// [`ALPN_PROTOCOL`].
///
/// Loopback proofs pass their ephemeral server certificate here; remote
/// trust models replace this call without touching the rest of the
/// transport.
///
/// # Errors
///
/// Returns [`TransportError::Tls`] when the root cannot be parsed into a
/// trust anchor, the versions are rejected, or the resulting configuration
/// cannot serve QUIC.
pub fn client_config(
    trusted_root: CertificateDer<'static>,
) -> Result<ClientConfig, TransportError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(trusted_root).map_err(TransportError::Tls)?;

    let mut tls = rustls::ClientConfig::builder_with_provider(ring_provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(TransportError::Tls)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];

    let crypto = QuicClientConfig::try_from(tls)?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config()));
    Ok(config)
}

/// Binds a server endpoint on `127.0.0.1:0`.
///
/// The chosen port is available through
/// [`Endpoint::local_addr`](quinn::Endpoint::local_addr).
///
/// # Errors
///
/// Returns [`TransportError::Bind`] when the loopback socket cannot be
/// bound.
pub fn bind_loopback_server(config: ServerConfig) -> Result<Endpoint, TransportError> {
    let local_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
    Endpoint::server(config, local_addr).map_err(TransportError::Bind)
}

/// Binds a client endpoint on `127.0.0.1:0` with `config` as its default.
///
/// # Errors
///
/// Returns [`TransportError::Bind`] when the loopback socket cannot be
/// bound.
pub fn bind_loopback_client(config: ClientConfig) -> Result<Endpoint, TransportError> {
    let local_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
    let mut endpoint = Endpoint::client(local_addr).map_err(TransportError::Bind)?;
    endpoint.set_default_client_config(config);
    Ok(endpoint)
}

/// Connects to the server at `server_addr`, validating its certificate
/// against `server_name`.
///
/// Callers own timeouts around the returned future; nothing here blocks
/// indefinitely beyond what QUIC's own handshake timers allow.
///
/// # Errors
///
/// Returns [`TransportError::Connect`] when the connection request is
/// rejected locally or during handshaking and
/// [`TransportError::Connection`] once the handshake itself fails.
pub async fn connect(
    endpoint: &Endpoint,
    server_addr: SocketAddr,
    server_name: &str,
) -> Result<Connection, TransportError> {
    Ok(endpoint.connect(server_addr, server_name)?.await?)
}

/// Closes all of `endpoint`'s connections with `error_code` and `reason`
/// and waits until it reaches idle state, bounded by `limit`.
///
/// # Errors
///
/// Returns [`TransportError::ShutdownTimeout`] when connections are still
/// open after `limit` elapses.
pub async fn shutdown(
    endpoint: &Endpoint,
    error_code: VarInt,
    reason: &[u8],
    limit: Duration,
) -> Result<(), TransportError> {
    endpoint.close(error_code, reason);
    if tokio::time::timeout(limit, endpoint.wait_idle())
        .await
        .is_err()
    {
        return Err(TransportError::ShutdownTimeout { limit });
    }
    Ok(())
}
