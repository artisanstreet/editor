//! Phase 1 direct-QUIC feasibility proof.
//!
//! These tests drive a real Quinn client and server over `127.0.0.1` with
//! real TLS: ephemeral rcgen PKI, the fixed transport ALPN, bounded framing
//! from the production crate, and typed errors for every negative path.
//! Every potentially hanging await is wrapped in a short timeout so a
//! regression fails fast instead of hanging the runner.
//!
//! Teardown contract: each test drops its connection and stream handles
//! before calling [`Loopback::drain`], so both endpoints can provably
//! reach idle state without waiting on references the test still holds.

mod harness;
mod resource_budget;

use artisan_transport as transport;
use artisan_transport::frame::{FrameOutcome, read_next_frame};
use harness::{
    FRAME_BOUND, Loopback, TEST_DEADLINE, connect_client, deterministic_payload, server_connection,
    spawn_echo, spawn_loopback,
};
use quinn::crypto::rustls::HandshakeData;
use quinn::{ConnectionError, ReadError, VarInt};

/// Tight reader bound used to prove oversized rejection without allocating.
const SMALL_BOUND: u32 = 1024;

/// Reads the negotiated ALPN protocol from a completed handshake.
fn negotiated_alpn(connection: &quinn::Connection) -> Option<Vec<u8>> {
    let handshake = connection
        .handshake_data()?
        .downcast::<HandshakeData>()
        .expect("rustls handshake data");
    handshake.protocol
}

/// A real QUIC handshake over loopback negotiates the fixed ALPN and both
/// sides observe each other's addresses.
#[tokio::test]
async fn loopback_handshake_negotiates_the_fixed_alpn() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    assert_eq!(
        negotiated_alpn(&client_connection).as_deref(),
        Some(transport::ALPN_PROTOCOL)
    );
    assert_eq!(
        negotiated_alpn(&server_connection).as_deref(),
        Some(transport::ALPN_PROTOCOL)
    );

    assert_eq!(client_connection.remote_address(), loopback.server_addr);
    assert_eq!(
        server_connection.remote_address(),
        loopback.client.local_addr().expect("client bound address")
    );

    drop(client_connection);
    drop(server_connection);
    loopback.drain(VarInt::from_u32(0), b"proof complete").await;
}

/// One bidirectional stream carries a length-prefixed request and a reply
/// with exact boundaries, including payloads larger than the QUIC flow
/// control window, and framing stays aligned across repeated streams.
#[tokio::test]
async fn bidi_frames_round_trip_over_streams() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let echo = spawn_echo(server_connection);

    // Larger than the configured 1,250,000-byte stream receive window on
    // purpose: Quinn must replenish credit while the framed body is read.
    let large_payload = deterministic_payload(1_500_000);
    let (mut send, mut recv) = client_connection.open_bi().await.expect("bidi stream");
    transport::write_frame(&mut send, &large_payload)
        .await
        .expect("request written");
    send.finish().expect("request finished");

    let reply = tokio::time::timeout(TEST_DEADLINE, transport::read_frame(&mut recv, FRAME_BOUND))
        .await
        .expect("reply arrives within deadline")
        .expect("reply readable");
    assert_eq!(reply, large_payload);

    let small_payload = deterministic_payload(7);
    let (mut send, mut recv) = client_connection.open_bi().await.expect("bidi stream");
    transport::write_frame(&mut send, &small_payload)
        .await
        .expect("second request written");
    send.finish().expect("second request finished");

    let reply = tokio::time::timeout(TEST_DEADLINE, transport::read_frame(&mut recv, FRAME_BOUND))
        .await
        .expect("second reply arrives within deadline")
        .expect("second reply readable");
    assert_eq!(reply, small_payload);

    echo.abort();
    drop(send);
    drop(recv);
    drop(client_connection);
    loopback.drain(VarInt::from_u32(0), b"proof complete").await;
}

/// An oversized length prefix is rejected with [`FrameError::TooLarge`]
/// before the body is awaited or allocated, even while the peer keeps the
/// stream open without finishing it.
#[tokio::test]
async fn oversized_frame_prefix_is_rejected_before_body_allocation() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    let announced_length = SMALL_BOUND + 1;
    let (mut send, _recv) = client_connection.open_bi().await.expect("bidi stream");
    send.write_all(&announced_length.to_le_bytes())
        .await
        .expect("prefix written");
    send.write_all(&deterministic_payload(16))
        .await
        .expect("partial body written");
    // Deliberately no finish(): the reader must reject without waiting.

    let (_send, mut recv) = server_connection
        .accept_bi()
        .await
        .expect("stream accepted");
    let outcome =
        tokio::time::timeout(TEST_DEADLINE, transport::read_frame(&mut recv, SMALL_BOUND))
            .await
            .expect("rejection within deadline");

    assert_eq!(
        outcome,
        Err(transport::FrameError::TooLarge {
            length: announced_length,
            bound: SMALL_BOUND,
        })
    );

    drop(send);
    drop(recv);
    drop(client_connection);
    drop(server_connection);
    loopback.drain(VarInt::from_u32(0), b"proof complete").await;
}

/// A frame exactly at the bound is accepted, proving the limit is
/// inclusive and not off by one.
#[tokio::test]
async fn frame_exactly_at_the_bound_is_accepted() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    let at_bound = deterministic_payload(SMALL_BOUND as usize);
    let (mut send, _recv) = client_connection.open_bi().await.expect("bidi stream");
    transport::write_frame(&mut send, &at_bound)
        .await
        .expect("bound-sized frame written");
    send.finish().expect("frame finished");

    let (_send, mut recv) = server_connection
        .accept_bi()
        .await
        .expect("stream accepted");
    let received =
        tokio::time::timeout(TEST_DEADLINE, transport::read_frame(&mut recv, SMALL_BOUND))
            .await
            .expect("frame arrives within deadline")
            .expect("frame readable");
    assert_eq!(received, at_bound);

    drop(send);
    drop(recv);
    drop(client_connection);
    drop(server_connection);
    loopback.drain(VarInt::from_u32(0), b"proof complete").await;
}

/// A zero-length prefix is malformed input and returns a typed error
/// without panicking.
#[tokio::test]
async fn empty_frame_prefix_is_rejected_as_malformed() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    let (mut send, _recv) = client_connection.open_bi().await.expect("bidi stream");
    send.write_all(&0u32.to_le_bytes())
        .await
        .expect("empty prefix written");

    let (_send, mut recv) = server_connection
        .accept_bi()
        .await
        .expect("stream accepted");
    let outcome =
        tokio::time::timeout(TEST_DEADLINE, transport::read_frame(&mut recv, SMALL_BOUND))
            .await
            .expect("rejection within deadline");

    assert_eq!(outcome, Err(transport::FrameError::Empty));

    drop(send);
    drop(recv);
    drop(client_connection);
    drop(server_connection);
    loopback.drain(VarInt::from_u32(0), b"proof complete").await;
}

/// A truncated frame surfaces as typed truncation once the peer finishes
/// the stream early.
#[tokio::test]
async fn truncated_frame_surfaces_a_typed_error() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    let announced_length = 64u32;
    let delivered = 10usize;
    let (mut send, _recv) = client_connection.open_bi().await.expect("bidi stream");
    send.write_all(&announced_length.to_le_bytes())
        .await
        .expect("prefix written");
    send.write_all(&deterministic_payload(delivered))
        .await
        .expect("partial body written");
    send.finish().expect("stream finished early");

    let (_send, mut recv) = server_connection
        .accept_bi()
        .await
        .expect("stream accepted");
    let outcome =
        tokio::time::timeout(TEST_DEADLINE, transport::read_frame(&mut recv, SMALL_BOUND))
            .await
            .expect("truncation observed within deadline");

    assert_eq!(
        outcome,
        Err(transport::FrameError::Truncated {
            expected: usize::try_from(announced_length).expect("small announcement"),
            received: delivered,
        })
    );

    drop(send);
    drop(recv);
    drop(client_connection);
    drop(server_connection);
    loopback.drain(VarInt::from_u32(0), b"proof complete").await;
}

/// Sequential stream ends on one connection classify exactly as typed
/// outcomes: a finish before any prefix byte reads as
/// [`FrameOutcome::Finished`], a complete frame yields its owned bytes, a
/// finish inside the prefix stays exact [`transport::FrameError::Truncated`],
/// and the legacy [`transport::read_frame`] keeps reporting a clean pre-frame
/// finish as truncation of the whole prefix.
///
/// The four scenarios deliberately share one loopback pair and run strictly
/// in sequence: classification is per-stream state, and consolidating the
/// endpoint pairs keeps the suite deterministic under the parallel test
/// runner's bind pressure.
#[tokio::test]
async fn sequential_stream_ends_classify_as_finished_frames_or_truncation() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    // A finish carrying no prefix bytes at all is a graceful end of stream,
    // not a truncation.
    {
        let (mut send, _recv) = client_connection.open_bi().await.expect("bidi stream");
        send.finish().expect("stream finished without any byte");
        drop(send);

        let (_send, mut recv) = server_connection
            .accept_bi()
            .await
            .expect("first stream accepted");
        let outcome = tokio::time::timeout(TEST_DEADLINE, read_next_frame(&mut recv, SMALL_BOUND))
            .await
            .expect("finish observed within deadline");
        assert_eq!(outcome, Ok(FrameOutcome::Finished));
    }

    // A complete frame still yields its owned body bytes afterwards.
    {
        let payload = deterministic_payload(96);
        let (mut send, _recv) = client_connection.open_bi().await.expect("bidi stream");
        transport::write_frame(&mut send, &payload)
            .await
            .expect("frame written");
        send.finish().expect("frame finished");

        let (_send, mut recv) = server_connection
            .accept_bi()
            .await
            .expect("second stream accepted");
        let outcome = tokio::time::timeout(TEST_DEADLINE, read_next_frame(&mut recv, SMALL_BOUND))
            .await
            .expect("frame arrives within deadline");
        assert_eq!(outcome, Ok(FrameOutcome::Frame(payload)));
    }

    // A finish partway through the four-byte prefix stays typed truncation
    // with the full expected length and the exact received count.
    {
        let announced_length = 64u32;
        let delivered_prefix_bytes = 2usize;
        let (mut send, _recv) = client_connection.open_bi().await.expect("bidi stream");
        send.write_all(&announced_length.to_le_bytes()[..delivered_prefix_bytes])
            .await
            .expect("partial prefix written");
        send.finish().expect("stream finished mid-prefix");

        let (_send, mut recv) = server_connection
            .accept_bi()
            .await
            .expect("third stream accepted");
        let outcome = tokio::time::timeout(TEST_DEADLINE, read_next_frame(&mut recv, SMALL_BOUND))
            .await
            .expect("truncation observed within deadline");
        assert_eq!(
            outcome,
            Err(transport::FrameError::Truncated {
                expected: 4,
                received: delivered_prefix_bytes,
            })
        );
    }

    // The legacy reader keeps mapping a clean pre-frame finish to truncation
    // of the whole prefix, preserving existing dispatch-loop policy.
    {
        let (mut send, _recv) = client_connection.open_bi().await.expect("bidi stream");
        send.finish()
            .expect("fourth stream finished without any byte");
        drop(send);

        let (_send, mut recv) = server_connection
            .accept_bi()
            .await
            .expect("fourth stream accepted");
        let outcome =
            tokio::time::timeout(TEST_DEADLINE, transport::read_frame(&mut recv, SMALL_BOUND))
                .await
                .expect("legacy truncation observed within deadline");
        assert_eq!(
            outcome,
            Err(transport::FrameError::Truncated {
                expected: 4,
                received: 0,
            })
        );
    }

    drop(client_connection);
    drop(server_connection);
    loopback.drain(VarInt::from_u32(0), b"proof complete").await;
}

/// An explicit application close is observed by the peer with the exact
/// code and reason, and both endpoints then reach idle state.
#[tokio::test]
async fn application_close_is_observed_and_endpoints_reach_idle() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    client_connection.close(VarInt::from_u32(0x01), b"done");

    let observed = tokio::time::timeout(TEST_DEADLINE, server_connection.closed())
        .await
        .expect("close observed within deadline");
    match observed {
        ConnectionError::ApplicationClosed(close) => {
            assert_eq!(close.error_code, VarInt::from_u32(0x01));
            assert_eq!(close.reason.as_ref(), b"done".as_slice());
        }
        unexpected => panic!("expected ApplicationClosed, got {unexpected:?}"),
    }

    drop(client_connection);
    drop(server_connection);
    loopback.drain(VarInt::from_u32(0x01), b"done").await;
}

/// Dropping the client's connection and endpoint mid-read surfaces a typed
/// connection-lost error on the blocked server read within the deadline.
#[tokio::test]
async fn abruptly_dropped_client_surfaces_a_typed_error_on_the_server() {
    let Loopback {
        server_addr,
        client,
        mut server_connections,
        stop_server,
        server_thread,
    } = spawn_loopback();

    let connecting = client
        .connect(server_addr, transport::LOOPBACK_SERVER_NAME)
        .expect("connect request accepted");
    let client_connection = tokio::time::timeout(TEST_DEADLINE, connecting)
        .await
        .expect("handshake completes within deadline")
        .expect("connection established");
    let server_connection = tokio::time::timeout(TEST_DEADLINE, server_connections.recv())
        .await
        .expect("server connection arrives within deadline")
        .expect("server keeps accepting");

    // Open one bidirectional stream whose body will never arrive, so the
    // server-side read stays blocked mid-frame.
    let (mut client_send, client_recv) = client_connection.open_bi().await.expect("bidi stream");
    let announced_length = 4096u32;
    client_send
        .write_all(&announced_length.to_le_bytes())
        .await
        .expect("prefix written");
    client_send
        .write_all(&deterministic_payload(4))
        .await
        .expect("partial body written");
    // Deliberately no finish(): the server read must stay pending.

    let (server_send, mut server_recv) = server_connection
        .accept_bi()
        .await
        .expect("stream accepted");

    // Abrupt teardown: dropping the final client handles implicitly closes
    // the connection with application error code zero.
    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drop(client);

    let outcome = tokio::time::timeout(
        TEST_DEADLINE,
        transport::read_frame(&mut server_recv, FRAME_BOUND),
    )
    .await
    .expect("typed failure within deadline");

    match outcome {
        Err(transport::FrameError::Read(ReadError::ConnectionLost(
            ConnectionError::ApplicationClosed(close),
        ))) => {
            assert_eq!(close.error_code, VarInt::from_u32(0));
        }
        unexpected => panic!("expected ConnectionLost(ApplicationClosed), got {unexpected:?}"),
    }

    drop(server_recv);
    drop(server_send);
    drop(server_connection);
    drop(server_connections);
    let _stopped = stop_server.send(());
    server_thread.join().expect("server thread finishes");
}
