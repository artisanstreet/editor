//! Shared loopback orchestration for the Phase 1 QUIC proof tests.
//!
//! The harness owns everything the production transport deliberately does
//! not: ephemeral rcgen PKI, endpoint spawning, connection establishment
//! under deadlines, and deterministic teardown.
//!
//! One verified Windows/Quinn constraint shapes the layout: two Quinn
//! endpoints that share a single Tokio runtime in one process blackhole
//! each other's datagrams on this host (observed with current-thread and
//! multi-thread runtimes alike, while one endpoint per thread works). The
//! server therefore lives on its own thread with its own runtime and hands
//! established [`Connection`] handles to the test, whose runtime hosts the
//! client endpoint alone. Every protocol decision stays on the test
//! thread.

use std::net::SocketAddr;
use std::sync::mpsc::RecvTimeoutError;
use std::thread::JoinHandle;
use std::time::Duration;

use artisan_transport as transport;
use quinn::{Connection, Endpoint, VarInt};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

/// Generous-but-bounded deadline so a regression fails fast instead of
/// hanging the test runner.
pub(crate) const TEST_DEADLINE: Duration = Duration::from_secs(5);

/// Frame bound used by test readers; deliberately tighter than
/// [`transport::MAX_FRAME_LEN`] so oversized rejection needs no large
/// allocations.
pub(crate) const FRAME_BOUND: u32 = 2 * 1024 * 1024;

/// A Quinn server on a dedicated thread plus a Quinn client on the test
/// runtime, both bound to loopback with fresh test PKI.
pub(crate) struct Loopback {
    /// Address the server bound on `127.0.0.1`.
    pub(crate) server_addr: SocketAddr,
    /// Dialing side of the pair, hosted by the test's own runtime.
    pub(crate) client: Endpoint,
    /// Established server-side connections, in acceptance order.
    pub(crate) server_connections: tokio::sync::mpsc::Receiver<Connection>,
    /// Completing this signal asks the server thread to shut down.
    pub(crate) stop_server: tokio::sync::oneshot::Sender<()>,
    /// The thread hosting the server runtime and endpoint.
    pub(crate) server_thread: JoinHandle<()>,
}

impl Loopback {
    /// Drains both endpoints within [`TEST_DEADLINE`].
    ///
    /// The caller must have dropped every connection and stream handle it
    /// took from this pair first, so both endpoints can provably reach
    /// idle state instead of waiting on live references.
    ///
    /// # Panics
    ///
    /// Panics when either side fails to shut down deterministically.
    pub(crate) async fn drain(self, error_code: VarInt, reason: &[u8]) {
        let Self {
            client,
            server_connections,
            stop_server,
            server_thread,
            ..
        } = self;

        // Stop the server first: its own bounded shutdown flushes close
        // notification towards the client endpoint before it exits.
        drop(server_connections);
        let _stopped = stop_server.send(());
        server_thread.join().expect("server thread finishes");

        transport::shutdown(&client, error_code, reason, TEST_DEADLINE)
            .await
            .expect("client endpoint drains");
    }
}

/// Generates an ephemeral self-signed certificate valid for `localhost`.
fn ephemeral_certificate() -> (CertificateDer<'static>, PrivatePkcs8KeyDer<'static>) {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("valid SANs");
    (
        certified_key.cert.der().clone(),
        PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der()),
    )
}

/// Builds a matching server/client configuration pair with fresh test PKI.
pub(crate) fn endpoint_configs() -> (quinn::ServerConfig, quinn::ClientConfig) {
    let (certificate, private_key) = ephemeral_certificate();
    let pinned_identity = transport::PinnedIdentity::from_certificate(&certificate);
    let server_config = transport::server_config(vec![certificate.clone()], private_key)
        .expect("server configuration");
    let client_config =
        transport::client_config(certificate, pinned_identity).expect("client configuration");
    (server_config, client_config)
}

/// Spawns a fresh loopback endpoint pair with its own certificate.
///
/// # Panics
///
/// Panics when the server thread cannot start or bind within
/// [`TEST_DEADLINE`].
pub(crate) fn spawn_loopback() -> Loopback {
    let (server_config, client_config) = endpoint_configs();

    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    let (connections_tx, connections_rx) = tokio::sync::mpsc::channel(1);
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel();

    let server_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("server runtime");
        runtime.block_on(async move {
            let server = match transport::bind_loopback_server(server_config) {
                Ok(server) => server,
                Err(error) => panic!("server bind failed: {error}"),
            };
            let server_addr = server.local_addr().expect("bound server address");
            addr_tx
                .send(server_addr)
                .expect("the test waits for the server address");

            loop {
                let incoming = tokio::select! {
                    _ = &mut stop_rx => break,
                    incoming = server.accept() => incoming,
                };
                let Some(incoming) = incoming else {
                    break;
                };
                let established = tokio::time::timeout(TEST_DEADLINE, incoming)
                    .await
                    .expect("server handshake within deadline")
                    .expect("server connection established");
                if connections_tx.send(established).await.is_err() {
                    break;
                }
            }

            transport::shutdown(
                &server,
                VarInt::from_u32(0),
                b"test complete",
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

    Loopback {
        server_addr,
        client,
        server_connections: connections_rx,
        stop_server: stop_tx,
        server_thread,
    }
}

/// Establishes the client side of the loopback connection under a deadline.
///
/// # Panics
///
/// Panics when the handshake exceeds [`TEST_DEADLINE`] or fails.
pub(crate) async fn connect_client(loopback: &Loopback) -> Connection {
    let connecting = loopback
        .client
        .connect(loopback.server_addr, transport::LOOPBACK_SERVER_NAME)
        .expect("connect request accepted");

    tokio::time::timeout(TEST_DEADLINE, connecting)
        .await
        .expect("handshake completes within deadline")
        .expect("connection established")
}

/// Takes the next server-side connection established for the test.
///
/// # Panics
///
/// Panics when no connection arrives within [`TEST_DEADLINE`].
pub(crate) async fn server_connection(loopback: &mut Loopback) -> Connection {
    tokio::time::timeout(TEST_DEADLINE, loopback.server_connections.recv())
        .await
        .expect("server connection arrives within deadline")
        .expect("server keeps accepting")
}

/// Spawns an echo task answering every bidirectional stream with exactly
/// the frame it received.
pub(crate) fn spawn_echo(connection: Connection) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok((mut send, mut recv)) = connection.accept_bi().await {
            match transport::read_frame(&mut recv, FRAME_BOUND).await {
                Ok(frame) => {
                    if transport::write_frame(&mut send, &frame).await.is_err() {
                        break;
                    }
                    let _finished = send.finish();
                }
                Err(_) => break,
            }
        }
    })
}

/// Builds a deterministic payload of `length` bytes.
pub(crate) fn deterministic_payload(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).expect("remainder fits u8"))
        .collect()
}
