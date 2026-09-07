//! Endpoint construction, loopback binding, and lifecycle helpers.
//!
//! The trust model behind these builders is deliberately narrow: callers
//! hand over DER material plus the pinned end-entity fingerprint, and this
//! module wires ring-backed rustls with a fixed ALPN and explicit resource
//! budgets onto Quinn endpoints bound to `127.0.0.1:0`. Every client
//! configuration runs ordinary chain, validity, name, and signature
//! verification first, then requires the SHA-256 fingerprint of the full
//! end-entity DER to equal [`PinnedIdentity`] exactly. Certificate
//! provisioning, pairing, and reconnection remain the production trust
//! decision (an open plan decision) and are not invented here.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{
    ClientConfig, Connection, Endpoint, IdleTimeout, ServerConfig, TransportConfig, VarInt,
};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{DigitallySignedStruct, SignatureScheme};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};

use crate::error::TransportError;
use crate::identity::PinnedIdentity;

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

/// Maximum peer-initiated bidirectional streams admitted concurrently.
const MAX_INCOMING_BIDI_STREAMS: u32 = 16;

/// Server advertises no incoming unidirectional streams because clients
/// never initiate them toward Forge.
const SERVER_MAX_INCOMING_UNI_STREAMS: u32 = 0;

/// Client advertises exactly one incoming unidirectional stream reserved
/// for the server-owned conversation-delivery stream.
const CLIENT_MAX_INCOMING_UNI_STREAMS: u32 = 1;

/// Per-stream receive credit. Quinn replenishes this window as consumers
/// read, so it deliberately need not equal the maximum framed payload.
const STREAM_RECEIVE_WINDOW_BYTES: u32 = 1_250_000;

/// Aggregate receive credit shared by all streams on one connection.
const CONNECTION_RECEIVE_WINDOW_BYTES: u32 = 20_000_000;

/// Maximum unacknowledged outbound data retained for one connection.
const SEND_WINDOW_BYTES: u64 = 10_000_000;

/// The editor keeps an otherwise idle local session alive. The server does
/// not duplicate these probes because one side is sufficient.
const CLIENT_KEEP_ALIVE_SECONDS: u64 = 15;

fn ring_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn transport_config(
    keep_alive_interval: Option<Duration>,
    max_incoming_uni_streams: u32,
) -> TransportConfig {
    let mut transport = TransportConfig::default();
    let idle_timeout = IdleTimeout::from(VarInt::from_u32(IDLE_TIMEOUT_MS));
    transport
        .max_concurrent_bidi_streams(VarInt::from_u32(MAX_INCOMING_BIDI_STREAMS))
        .max_concurrent_uni_streams(VarInt::from_u32(max_incoming_uni_streams))
        .max_idle_timeout(Some(idle_timeout))
        .stream_receive_window(VarInt::from_u32(STREAM_RECEIVE_WINDOW_BYTES))
        .receive_window(VarInt::from_u32(CONNECTION_RECEIVE_WINDOW_BYTES))
        .send_window(SEND_WINDOW_BYTES)
        .send_fairness(true)
        .keep_alive_interval(keep_alive_interval)
        .datagram_receive_buffer_size(None);
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
    config.transport_config(Arc::new(transport_config(
        None,
        SERVER_MAX_INCOMING_UNI_STREAMS,
    )));
    Ok(config)
}

/// Builds a QUIC client configuration trusting exactly `trusted_root` and
/// accepting only a server whose end-entity certificate hashes, via
/// SHA-256 over the full DER, to exactly `pinned_identity`.
///
/// Ordinary chain construction, validity windows, name matching, and
/// signature verification stay delegated to the standard web-PKI verifier
/// built on the same ring provider as the rest of the configuration; the
/// pin is enforced only after that succeeds. There is deliberately no
/// unpinned client configuration: everything produced here requires both
/// checks.
///
/// Loopback proofs pass their ephemeral server certificate and its
/// fingerprint here; remote trust models reuse this call with the
/// handed-off pin without touching the rest of the transport.
///
/// # Errors
///
/// Returns [`TransportError::Tls`] when the root cannot be parsed into a
/// trust anchor or the versions are rejected,
/// [`TransportError::VerifierBuilder`] when the underlying web-PKI
/// verifier cannot be constructed, and [`TransportError::Crypto`] when
/// the resulting configuration cannot serve QUIC.
pub fn client_config(
    trusted_root: CertificateDer<'static>,
    pinned_identity: PinnedIdentity,
) -> Result<ClientConfig, TransportError> {
    let provider = ring_provider();

    let mut roots = rustls::RootCertStore::empty();
    roots.add(trusted_root).map_err(TransportError::Tls)?;

    let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(TransportError::VerifierBuilder)?;

    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(TransportError::Tls)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServerVerifier {
            inner,
            pinned_identity,
        }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN_PROTOCOL.to_vec()];

    let crypto = QuicClientConfig::try_from(tls)?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config(
        Some(Duration::from_secs(CLIENT_KEEP_ALIVE_SECONDS)),
        CLIENT_MAX_INCOMING_UNI_STREAMS,
    )));
    Ok(config)
}

/// Exact-leaf pin wrapped around standard server-certificate verification.
///
/// The inner verifier performs the complete ordinary check — chain to the
/// handed-off trust anchor, validity window, DNS/SAN match, and signature
/// verification — using the same ring provider as the rest of the
/// configuration. Only once that succeeds is the SHA-256 of the full
/// end-entity DER compared against the pin. A mismatch yields a fixed
/// zero-payload certificate error so failures reveal nothing about how
/// close a presented certificate came to the pin.
struct PinnedServerVerifier {
    /// Ordinary verification already bound to the shared trust roots and
    /// ring signature algorithms.
    inner: Arc<WebPkiServerVerifier>,
    /// Required SHA-256 fingerprint of the end-entity DER.
    pinned_identity: PinnedIdentity,
}

impl fmt::Debug for PinnedServerVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedServerVerifier")
            .field("algorithm", &PinnedIdentity::ALGORITHM)
            .field("fingerprint", &self.pinned_identity.to_hex())
            .finish_non_exhaustive()
    }
}

impl ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verified = self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        )?;
        if PinnedIdentity::from_certificate(end_entity) != self.pinned_identity {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner
            .verify_tls12_signature(message, certificate, signature)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner
            .verify_tls13_signature(message, certificate, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
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
