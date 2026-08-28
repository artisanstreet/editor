//! Resource-budget policy for Artisan's local-only QUIC endpoints.

use std::time::Duration;

use crate::harness::{
    TEST_DEADLINE, connect_client, endpoint_configs, server_connection, spawn_loopback,
};
use quinn::{SendDatagramError, VarInt};

/// A short observation window proves an operation remains admission-blocked
/// without making the overall test suite slow.
const BLOCKED_OPERATION_WINDOW: Duration = Duration::from_millis(100);

const COMMON_BUDGET_FIELDS: &[&str] = &[
    "max_concurrent_bidi_streams: 16",
    "max_concurrent_uni_streams: 0",
    "max_idle_timeout: Some(30000)",
    "stream_receive_window: 1250000",
    "receive_window: 20000000",
    "send_window: 10000000",
    "send_fairness: true",
    "datagram_receive_buffer_size: None",
];

fn assert_common_budget(debug: &str) {
    for expected in COMMON_BUDGET_FIELDS {
        assert!(
            debug.contains(expected),
            "transport configuration omitted `{expected}`: {debug}"
        );
    }
}

#[test]
fn endpoint_configs_encode_the_exact_resource_policy() {
    let (server, client) = endpoint_configs();
    let server_transport = format!("{:?}", server.transport);
    let client_config = format!("{client:?}");

    assert_common_budget(&server_transport);
    assert_common_budget(&client_config);
    assert!(server_transport.contains("keep_alive_interval: None"));
    assert!(client_config.contains("keep_alive_interval: Some(15s)"));
}

#[tokio::test]
async fn seventeenth_bidirectional_stream_waits_until_credit_returns() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let mut client_streams = Vec::new();
    let mut server_streams = Vec::new();

    for marker in 0..16u8 {
        let (mut client_send, client_recv) = client_connection
            .open_bi()
            .await
            .expect("advertised bidirectional stream credit");
        client_send
            .write_all(&[marker])
            .await
            .expect("stream marker written");
        let (server_send, mut server_recv) =
            tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi())
                .await
                .expect("peer observes stream within deadline")
                .expect("peer accepts stream");
        let mut observed = [0u8; 1];
        server_recv
            .read_exact(&mut observed)
            .await
            .expect("stream marker readable");
        assert_eq!(observed, [marker]);
        client_streams.push((client_send, client_recv));
        server_streams.push((server_send, server_recv));
    }

    assert!(
        tokio::time::timeout(BLOCKED_OPERATION_WINDOW, client_connection.open_bi())
            .await
            .is_err(),
        "a seventeenth live bidirectional stream exceeded the advertised budget"
    );

    // Quinn batches MAX_STREAMS updates until more than one-eighth of the
    // advertised window has been returned. Closing three of sixteen streams
    // crosses that threshold and proves credit is replenished, not exhausted
    // permanently after the first sixteen stream IDs.
    for _ in 0..3 {
        let (mut client_send, mut client_recv) =
            client_streams.pop().expect("sixteen client streams");
        let (mut server_send, mut server_recv) =
            server_streams.pop().expect("sixteen server streams");
        client_send.finish().expect("client direction finished");
        server_send.finish().expect("server direction finished");
        assert!(
            server_recv
                .read_to_end(0)
                .await
                .expect("server observes client finish")
                .is_empty()
        );
        assert!(
            client_recv
                .read_to_end(0)
                .await
                .expect("client observes server finish")
                .is_empty()
        );
        let stopped = tokio::time::timeout(TEST_DEADLINE, server_send.stopped())
            .await
            .expect("server FIN acknowledged within deadline")
            .expect("server send stream remains connected");
        assert!(stopped.is_none());
        drop(client_send);
        drop(client_recv);
        drop(server_send);
        drop(server_recv);
    }

    let returned_credit = tokio::time::timeout(TEST_DEADLINE, client_connection.open_bi())
        .await
        .expect("closed stream returns credit within deadline")
        .expect("returned stream credit is usable");

    drop(returned_credit);
    drop(client_streams);
    drop(server_streams);
    drop(client_connection);
    drop(server_connection);
    loopback
        .drain(VarInt::from_u32(0), b"resource budget verified")
        .await;
}

#[tokio::test]
async fn unidirectional_streams_and_application_datagrams_are_disabled() {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;

    assert!(
        tokio::time::timeout(BLOCKED_OPERATION_WINDOW, client_connection.open_uni())
            .await
            .is_err(),
        "the server unexpectedly advertised unidirectional stream credit"
    );
    assert!(
        tokio::time::timeout(BLOCKED_OPERATION_WINDOW, server_connection.open_uni())
            .await
            .is_err(),
        "the client unexpectedly advertised unidirectional stream credit"
    );

    assert!(matches!(
        client_connection.send_datagram(b"client datagram".as_slice().into()),
        Err(SendDatagramError::Disabled)
    ));
    assert!(matches!(
        server_connection.send_datagram(b"server datagram".as_slice().into()),
        Err(SendDatagramError::Disabled)
    ));

    drop(client_connection);
    drop(server_connection);
    loopback
        .drain(VarInt::from_u32(0), b"resource budget verified")
        .await;
}
