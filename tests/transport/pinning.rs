//! External evidence for exact-leaf certificate pinning over real QUIC.
//!
//! Every scenario runs a genuine rustls 1.3 handshake between a Quinn
//! client on the test runtime and a Quinn server on its own thread (the
//! isolated-loopback convention: two endpoints sharing one runtime
//! blackhole each other's datagrams on this host). The PKI is generated
//! per test with rcgen: one authority issuing fresh localhost leaves, and
//! pins derived from the exact leaf DER handed to the server.
//!
//! Boundedness contract: every await sits inside [`TEST_DEADLINE`], the
//! fixture's channel is capacity-one, and the server thread treats a
//! failed or rejected handshake as evidence — never as a panic — so the
//! negative proofs tear down deterministically. Failure assertions match
//! typed transport variants only; no certificate bytes, addresses, peer
//! text, or pin payloads appear in them.

#[allow(
    dead_code,
    reason = "this target reuses loopback setup but not the proof-only echo helpers"
)]
mod harness;

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::thread::JoinHandle;

use artisan_transport as transport;
use harness::{TEST_DEADLINE, connect_client, server_connection, spawn_loopback};
use quinn::crypto::rustls::HandshakeData;
use quinn::{Connection, Endpoint, VarInt};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

/// Subject alternative name every valid test leaf must carry.
const LOCALHOST: &str = "localhost";

/// SAN for the leaf that must fail name validation despite a matching pin.
const OTHER_NAME: &str = "elsewhere.example";

const SAMPLE_DIGEST: [u8; 32] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
];

/// The lowercase-hex oracle for [`SAMPLE_DIGEST`].
const SAMPLE_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

/// A self-signed authority that issues fresh leaves for one scenario.
struct TestAuthority {
    /// DER of the self-signed CA certificate handed to client roots.
    certificate: CertificateDer<'static>,
    /// CA parameters borrowed while building leaf issuers.
    params: CertificateParams,
    /// CA signing key referenced through an issuer on demand.
    key: KeyPair,
}

impl TestAuthority {
    /// Generates a fresh unconstrained self-signed CA.
    fn new() -> Self {
        let key = KeyPair::generate().expect("CA key pair");
        let mut params = CertificateParams::new(Vec::new()).expect("CA parameters");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let cert = params.self_signed(&key).expect("CA certificate");
        Self {
            certificate: cert.der().clone(),
            params,
            key,
        }
    }

    /// Issues a brand-new leaf covering exactly `names`.
    fn issue(
        &self,
        names: &[&'static str],
    ) -> (CertificateDer<'static>, PrivatePkcs8KeyDer<'static>) {
        let subjects: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        let key = KeyPair::generate().expect("leaf key pair");
        let params = CertificateParams::new(subjects).expect("leaf parameters");
        let issuer = rcgen::Issuer::from_params(&self.params, &self.key);
        let cert = params.signed_by(&key, &issuer).expect("leaf issuance");
        (
            cert.der().clone(),
            PrivatePkcs8KeyDer::from(key.serialize_der()),
        )
    }
}

/// Reads the negotiated ALPN protocol from a completed handshake.
fn negotiated_alpn(connection: &Connection) -> Option<Vec<u8>> {
    let data = connection
        .handshake_data()
        .expect("post-handshake data present")
        .downcast::<HandshakeData>()
        .expect("rustls handshake data");
    data.protocol
}

/// Asserts the transport boundary rejected the handshake with a typed
/// connection error. Deliberately variant-loose inside `Connection`:
/// whether the local verifier, the local TLS stack, or the peer's alert
/// surfaces first is not stable across failure orders.
fn assert_handshake_rejected(outcome: &Result<Connection, transport::TransportError>) {
    assert!(
        matches!(outcome, Err(transport::TransportError::Connection(_))),
        "handshake must be rejected at the transport boundary"
    );
}

/// Builds a QUIC server configuration negotiating an ALPN no production
/// client offers. Test-only rustls wiring, used solely because the fixed
/// production server configuration always offers the Artisan ALPN.
fn foreign_alpn_server_config(
    chain: Vec<CertificateDer<'static>>,
    private_key: PrivatePkcs8KeyDer<'static>,
) -> quinn::ServerConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("protocol versions accepted")
        .with_no_client_auth()
        .with_single_cert(chain, private_key.into())
        .expect("certificate material accepted");
    // Leaving alpn_protocols empty makes any client proposal fail.
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .expect("QUIC-capable TLS configuration");
    quinn::ServerConfig::with_crypto(Arc::new(crypto))
}

/// A pinned loopback pair whose negative handshakes are evidence, not
/// crashes: the dedicated-thread server swallows failed handshakes and
/// keeps serving until asked to stop.
struct PinFixture {
    /// Address the server bound on `127.0.0.1`.
    server_addr: SocketAddr,
    /// Dialing side of the pair, hosted by the test runtime.
    client: Endpoint,
    /// Established server-side connections, in acceptance order.
    connections: tokio::sync::mpsc::Receiver<Connection>,
    /// Completing this signal asks the server thread to shut down.
    stop_server: tokio::sync::oneshot::Sender<()>,
    /// The thread hosting the server runtime and endpoint.
    server_thread: JoinHandle<()>,
}

impl PinFixture {
    /// Spawns the pair with the given endpoint configurations.
    ///
    /// # Panics
    ///
    /// Panics when the server thread cannot start or bind within
    /// [`TEST_DEADLINE`]; failed handshakes never reach this path.
    fn start(server_config: quinn::ServerConfig, client_config: quinn::ClientConfig) -> Self {
        let (addr_tx, addr_rx) = std::sync::mpsc::channel();
        let (connections_tx, connections_rx) = tokio::sync::mpsc::channel(1);
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();

        let server_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("server runtime");
            runtime.block_on(async move {
                let server = transport::bind_loopback_server(server_config).expect("server bind");
                addr_tx
                    .send(server.local_addr().expect("bound server address"))
                    .expect("the test waits for the server address");

                loop {
                    tokio::select! {
                        _ = &mut stop_rx => break,
                        incoming = server.accept() => {
                            let Some(incoming) = incoming else {
                                break;
                            };
                            // A pin, SAN, issuer, or ALPN rejection lands
                            // here as Err. It is the expected outcome of the
                            // negative proofs; swallow it and keep serving.
                            if let Ok(Ok(connection)) =
                                tokio::time::timeout(TEST_DEADLINE, incoming).await
                            {
                                // The test may already have moved on. Keep
                                // draining accepts so teardown stays deterministic.
                                let _delivery_result = connections_tx.send(connection).await;
                            }
                        }
                    }
                }

                transport::shutdown(
                    &server,
                    VarInt::from_u32(0),
                    b"pinning proof complete",
                    TEST_DEADLINE,
                )
                .await
                .expect("server endpoint drains");
            });
        });

        let server_addr = match addr_rx.recv_timeout(TEST_DEADLINE) {
            Ok(server_addr) => server_addr,
            Err(RecvTimeoutError::Timeout) => panic!("server did not bind within deadline"),
            Err(RecvTimeoutError::Disconnected) => panic!("server thread died before binding"),
        };
        let client = transport::bind_loopback_client(client_config).expect("client bind");

        Self {
            server_addr,
            client,
            connections: connections_rx,
            stop_server: stop_tx,
            server_thread,
        }
    }

    /// Drains both endpoints within [`TEST_DEADLINE`].
    ///
    /// # Panics
    ///
    /// Panics when either side fails to shut down deterministically.
    async fn drain(self) {
        drop(self.connections);
        let _stopped = self.stop_server.send(());
        self.server_thread.join().expect("server thread finishes");
        transport::shutdown(
            &self.client,
            VarInt::from_u32(0),
            b"pinning proof complete",
            TEST_DEADLINE,
        )
        .await
        .expect("client endpoint drains");
    }
}

/// Dials the fixture under [`TEST_DEADLINE`], returning the typed result.
async fn bounded_connect(fixture: &PinFixture) -> Result<Connection, transport::TransportError> {
    tokio::time::timeout(
        TEST_DEADLINE,
        transport::connect(
            &fixture.client,
            fixture.server_addr,
            transport::LOOPBACK_SERVER_NAME,
        ),
    )
    .await
    .expect("handshake outcome arrives within deadline")
}

/// Pin material parses and renders exactly, and parse failures stay
/// payload-safe.
#[test]
fn pinned_identity_hex_round_trip_and_payload_safe_parse_failures() {
    let identity = transport::PinnedIdentity::from_digest(SAMPLE_DIGEST);
    assert_eq!(identity.to_hex(), SAMPLE_HEX);
    assert_eq!(identity.to_string(), SAMPLE_HEX);
    assert_eq!(
        format!("{identity:?}"),
        format!("PinnedIdentity {{ algorithm: \"sha256\", fingerprint: \"{SAMPLE_HEX}\" }}")
    );

    let reparsed = transport::PinnedIdentity::parse(SAMPLE_HEX).expect("oracle hex parses");
    assert_eq!(reparsed, identity);
    assert_eq!(reparsed.as_bytes(), &SAMPLE_DIGEST);

    let uppercase =
        transport::PinnedIdentity::parse(&SAMPLE_HEX.to_uppercase()).expect("uppercase hex parses");
    assert_eq!(uppercase.to_hex(), SAMPLE_HEX);

    assert_eq!(
        transport::PinnedIdentity::parse(&SAMPLE_HEX[..63]),
        Err(transport::PinnedIdentityError::WrongLength)
    );
    let long = format!("{SAMPLE_HEX}0");
    assert_eq!(
        transport::PinnedIdentity::parse(&long),
        Err(transport::PinnedIdentityError::WrongLength)
    );

    let non_hex_tail = format!("{}g", &SAMPLE_HEX[..63]);
    assert_eq!(
        transport::PinnedIdentity::parse(&non_hex_tail),
        Err(transport::PinnedIdentityError::NonHex)
    );
    let non_hex_head = format!("g{}", &SAMPLE_HEX[1..]);
    assert_eq!(
        transport::PinnedIdentity::parse(&non_hex_head),
        Err(transport::PinnedIdentityError::NonHex)
    );

    assert_eq!(
        transport::PinnedIdentityError::WrongLength.to_string(),
        "pinned identity must be exactly 64 hexadecimal characters"
    );
    assert_eq!(
        transport::PinnedIdentityError::NonHex.to_string(),
        "pinned identity contains a non-hexadecimal character"
    );
}

/// The shared positive harness connects end to end once its loopback pins
/// the ephemeral leaf it serves.
#[tokio::test]
async fn shared_loopback_harness_connects_under_leaf_pin() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    drop(client_connection);
    drop(server_connection);
    loopback
        .drain(VarInt::from_u32(0), b"pinning harness proof")
        .await;
}

/// A WebPKI-valid CA-issued leaf whose SHA-256 equals the pin completes a
/// real handshake and negotiates the fixed ALPN on both sides.
#[tokio::test]
async fn webpki_chain_with_matching_leaf_pin_connects_and_negotiates_alpn()
-> Result<(), Box<dyn Error>> {
    let authority = TestAuthority::new();
    let (leaf, leaf_key) = authority.issue(&[LOCALHOST]);
    let pin = transport::PinnedIdentity::from_certificate(&leaf);

    let mut fixture = PinFixture::start(
        transport::server_config(vec![leaf.clone()], leaf_key)?,
        transport::client_config(authority.certificate.clone(), pin)?,
    );

    let client_side = tokio::time::timeout(
        TEST_DEADLINE,
        transport::connect(
            &fixture.client,
            fixture.server_addr,
            transport::LOOPBACK_SERVER_NAME,
        ),
    )
    .await??;
    let server_side = tokio::time::timeout(TEST_DEADLINE, fixture.connections.recv())
        .await
        .expect("server observes the connection within deadline")
        .expect("server keeps accepting");

    for connection in [&client_side, &server_side] {
        assert_eq!(
            negotiated_alpn(connection).as_deref(),
            Some(transport::ALPN_PROTOCOL)
        );
    }

    drop(client_side);
    drop(server_side);
    fixture.drain().await;
    Ok(())
}

/// A fully WebPKI-valid chain presented with a well-formed but different
/// pin is refused after ordinary verification succeeds.
#[tokio::test]
async fn valid_webpki_chain_with_wrong_pin_is_rejected() -> Result<(), Box<dyn Error>> {
    let authority = TestAuthority::new();
    let (leaf, leaf_key) = authority.issue(&[LOCALHOST]);
    let decoy_pin = transport::PinnedIdentity::from_digest([0x37; 32]);
    assert_ne!(
        decoy_pin,
        transport::PinnedIdentity::from_certificate(&leaf)
    );

    let fixture = PinFixture::start(
        transport::server_config(vec![leaf], leaf_key)?,
        transport::client_config(authority.certificate.clone(), decoy_pin)?,
    );

    assert_handshake_rejected(&bounded_connect(&fixture).await);
    fixture.drain().await;
    Ok(())
}

/// The same authority issued both leaves; serving the sibling while
/// pinning the first fails even though the sibling passes `WebPKI` against
/// the trusted CA. This is the exact-leaf proof beyond root trust.
#[tokio::test]
async fn sibling_leaf_from_same_authority_fails_the_exact_leaf_pin() -> Result<(), Box<dyn Error>> {
    let authority = TestAuthority::new();
    let (leaf_a, _) = authority.issue(&[LOCALHOST]);
    let (leaf_b, leaf_b_key) = authority.issue(&[LOCALHOST]);
    assert_ne!(
        transport::PinnedIdentity::from_certificate(&leaf_a),
        transport::PinnedIdentity::from_certificate(&leaf_b)
    );

    let fixture = PinFixture::start(
        transport::server_config(vec![leaf_b], leaf_b_key)?,
        transport::client_config(
            authority.certificate.clone(),
            transport::PinnedIdentity::from_certificate(&leaf_a),
        )?,
    );

    assert_handshake_rejected(&bounded_connect(&fixture).await);
    fixture.drain().await;
    Ok(())
}

/// A pin computed from the presented leaf cannot rescue a certificate
/// that does not cover the dialed name: SAN validation survives pinning.
#[tokio::test]
async fn matching_pin_cannot_rescue_a_wrong_san_leaf() -> Result<(), Box<dyn Error>> {
    let authority = TestAuthority::new();
    let (elsewhere_leaf, elsewhere_key) = authority.issue(&[OTHER_NAME]);

    let fixture = PinFixture::start(
        transport::server_config(vec![elsewhere_leaf.clone()], elsewhere_key)?,
        transport::client_config(
            authority.certificate.clone(),
            transport::PinnedIdentity::from_certificate(&elsewhere_leaf),
        )?,
    );

    assert_handshake_rejected(&bounded_connect(&fixture).await);
    fixture.drain().await;
    Ok(())
}

/// A pin matching the presented leaf cannot substitute for chain-of-
/// trust: an unrelated authority's leaf stays untrusted.
#[tokio::test]
async fn matching_pin_cannot_rescue_an_unrelated_issuer_leaf() -> Result<(), Box<dyn Error>> {
    let trusted_authority = TestAuthority::new();
    let foreign_authority = TestAuthority::new();
    let (foreign_leaf, foreign_leaf_key) = foreign_authority.issue(&[LOCALHOST]);

    let fixture = PinFixture::start(
        transport::server_config(vec![foreign_leaf.clone()], foreign_leaf_key)?,
        transport::client_config(
            trusted_authority.certificate.clone(),
            transport::PinnedIdentity::from_certificate(&foreign_leaf),
        )?,
    );

    assert_handshake_rejected(&bounded_connect(&fixture).await);
    fixture.drain().await;
    Ok(())
}

/// With trust and pin both satisfied, an ALPN disagreement still fails
/// the handshake: the wrapper changes nothing about protocol selection.
#[tokio::test]
async fn matching_trust_and_pin_still_fail_on_alpn_mismatch() -> Result<(), Box<dyn Error>> {
    let authority = TestAuthority::new();
    let (leaf, leaf_key) = authority.issue(&[LOCALHOST]);
    let pin = transport::PinnedIdentity::from_certificate(&leaf);

    let fixture = PinFixture::start(
        foreign_alpn_server_config(vec![leaf], leaf_key),
        transport::client_config(authority.certificate.clone(), pin)?,
    );

    assert_handshake_rejected(&bounded_connect(&fixture).await);
    fixture.drain().await;
    Ok(())
}
