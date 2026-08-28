//! Owned application-envelope conversion over real bounded Quinn streams.

#[allow(
    dead_code,
    reason = "this target reuses loopback setup but not the proof-only echo helpers"
)]
mod harness;

use std::error::Error;

use artisan_domain::{AttachProject, Command, DirectoryId, RequestId, UnixMillis};
use artisan_protocol::{
    ClientRequest, ConnectionId, FrameId, Hello, HelloCredential, LocalCapability,
    ProtocolDecodeError, ProtocolEncodeError, ProtocolValueError, ProtocolVersion,
    ReconnectCapability, VersionOffer, Welcome, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport as transport;
use harness::{Loopback, TEST_DEADLINE, connect_client, server_connection, spawn_loopback};
use quinn::VarInt;

const INITIAL_CAPABILITY: [u8; 32] = [0x35; 32];
const RECONNECT_CAPABILITY: [u8; 32] = [0xa7; 32];

fn hello() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("client-hello-1")?,
        sent_at: UnixMillis::from_millis(1_000),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])?,
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
        }),
    })
}

fn welcome() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("server-welcome-1")?,
        sent_at: UnixMillis::from_millis(1_001),
        body: WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: ProtocolVersion::V1,
            connection_id: ConnectionId::parse("connection-1")?,
            reconnect_capability: ReconnectCapability::from_bytes(RECONNECT_CAPABILITY),
        }),
    })
}

async fn drain(loopback: Loopback) {
    loopback
        .drain(VarInt::from_u32(0), b"protocol stream test complete")
        .await;
}

#[tokio::test]
async fn owned_envelopes_cross_one_bounded_bidirectional_stream() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_recv) = client_connection.open_bi().await?;

    let expected_hello = hello()?;
    tokio::time::timeout(
        TEST_DEADLINE,
        transport::send_envelope(&mut client_send, &expected_hello),
    )
    .await??;

    let (mut server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    let received_hello =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut server_recv))
            .await??;
    assert!(received_hello == expected_hello);

    let expected_welcome = welcome()?;
    tokio::time::timeout(
        TEST_DEADLINE,
        transport::send_envelope(&mut server_send, &expected_welcome),
    )
    .await??;
    let received_welcome =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut client_recv))
            .await??;
    assert!(received_welcome == expected_welcome);

    client_send.finish()?;
    server_send.finish()?;
    drop(client_send);
    drop(client_recv);
    drop(server_send);
    drop(server_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn protocol_decode_failures_remain_distinct_from_frame_failures() -> Result<(), Box<dyn Error>>
{
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, client_recv) = client_connection.open_bi().await?;

    tokio::time::timeout(
        TEST_DEADLINE,
        transport::write_frame(&mut client_send, &[1, 2, 3, 4]),
    )
    .await??;
    client_send.finish()?;

    let (server_send, mut server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    let result =
        tokio::time::timeout(TEST_DEADLINE, transport::receive_envelope(&mut server_recv)).await?;
    assert!(matches!(
        result,
        Err(transport::EnvelopeReceiveError::Decode(
            ProtocolDecodeError::Capnp { .. }
        ))
    ));

    let (mut empty_send, empty_recv) = client_connection.open_bi().await?;
    tokio::time::timeout(TEST_DEADLINE, empty_send.write_all(&0u32.to_le_bytes())).await??;
    empty_send.finish()?;
    let (empty_server_send, mut empty_server_recv) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::receive_envelope(&mut empty_server_recv),
    )
    .await?;
    assert!(matches!(
        result,
        Err(transport::EnvelopeReceiveError::Frame(
            transport::FrameError::Empty
        ))
    ));

    drop(client_send);
    drop(client_recv);
    drop(empty_send);
    drop(empty_recv);
    drop(server_send);
    drop(server_recv);
    drop(empty_server_send);
    drop(empty_server_recv);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn invalid_owned_correlation_fails_before_stream_output() -> Result<(), Box<dyn Error>> {
    let loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let (mut client_send, client_recv) = client_connection.open_bi().await?;
    let invalid = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("outer-request")?,
        sent_at: UnixMillis::EPOCH,
        body: WireEnvelopeBody::Request(ClientRequest::Command(Command::AttachProject(
            AttachProject {
                request_id: RequestId::parse("inner-request")?,
                directory_id: DirectoryId::parse("directory-1")?,
            },
        ))),
    };

    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::send_envelope(&mut client_send, &invalid),
    )
    .await?;
    assert!(matches!(
        result,
        Err(transport::EnvelopeSendError::Encode(
            ProtocolEncodeError::Value(ProtocolValueError::RequestCorrelationMismatch)
        ))
    ));

    drop(client_send);
    drop(client_recv);
    drop(client_connection);
    drain(loopback).await;
    Ok(())
}
