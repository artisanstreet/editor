//! Application Hello/Welcome ordering over real loopback QUIC.

#[allow(
    dead_code,
    reason = "this target reuses loopback setup but not the proof-only echo helpers"
)]
mod harness;

use std::error::Error;

use artisan_domain::UnixMillis;
use artisan_protocol::{
    ConnectionId, ErrorCode, ErrorDetail, FrameId, Hello, HelloCredential, LocalCapability,
    ProtocolFailure, ProtocolVersion, ReconnectCapability, VersionOffer, Welcome, WireEnvelope,
    WireEnvelopeBody,
};
use artisan_transport as transport;
use harness::{Loopback, TEST_DEADLINE, connect_client, server_connection, spawn_loopback};
use quinn::VarInt;

const INITIAL_CAPABILITY: [u8; 32] = [0x49; 32];
const ROTATED_CAPABILITY: [u8; 32] = [0xc2; 32];

fn hello() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("client-hello")?,
        sent_at: UnixMillis::from_millis(10),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])?,
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
        }),
    })
}

fn welcome() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("server-welcome")?,
        sent_at: UnixMillis::from_millis(11),
        body: WireEnvelopeBody::Welcome(Welcome {
            negotiated_version: ProtocolVersion::V1,
            connection_id: ConnectionId::parse("connection-1")?,
            reconnect_capability: ReconnectCapability::from_bytes(ROTATED_CAPABILITY),
        }),
    })
}

fn rejection() -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("server-rejection")?,
        sent_at: UnixMillis::from_millis(11),
        body: WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::InvalidInput,
            detail: ErrorDetail::parse("credential rejected")?,
            retryable: false,
            request_id: None,
        }),
    })
}

async fn drain(loopback: Loopback) {
    loopback
        .drain(VarInt::from_u32(0), b"handshake test complete")
        .await;
}

#[tokio::test]
async fn hello_and_welcome_establish_the_application_boundary() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_receive) = client_connection.open_bi().await?;

    let client = tokio::time::timeout(
        TEST_DEADLINE,
        transport::client_handshake(&mut client_send, &mut client_receive, hello()?),
    );
    let server = async {
        let (mut server_send, mut server_receive) =
            tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
        let received = tokio::time::timeout(
            TEST_DEADLINE,
            transport::receive_client_hello(&mut server_receive),
        )
        .await??;
        assert_eq!(received.protocol_version, ProtocolVersion::V1);
        assert_eq!(received.frame_id.as_str(), "client-hello");
        assert_eq!(received.sent_at, UnixMillis::from_millis(10));
        assert!(
            received.hello.credential
                == HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY))
        );
        let response = welcome()?;
        tokio::time::timeout(
            TEST_DEADLINE,
            transport::send_server_welcome(
                &mut server_send,
                &response,
                &received.hello.supported_versions,
            ),
        )
        .await??;
        server_send.finish()?;
        Ok::<_, Box<dyn Error>>((server_send, server_receive))
    };

    let (client_result, server_result) = tokio::join!(client, server);
    let received = client_result??;
    let (server_send, server_receive) = server_result?;
    assert_eq!(received.protocol_version, ProtocolVersion::V1);
    assert_eq!(received.frame_id.as_str(), "server-welcome");
    assert_eq!(received.sent_at, UnixMillis::from_millis(11));
    assert!(
        received.welcome
            == match welcome()?.body {
                WireEnvelopeBody::Welcome(value) => value,
                _ => unreachable!("fixture is Welcome"),
            }
    );

    client_send.finish()?;
    drop(client_send);
    drop(client_receive);
    drop(server_send);
    drop(server_receive);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn server_rejects_any_first_message_other_than_hello() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, client_receive) = client_connection.open_bi().await?;
    tokio::time::timeout(
        TEST_DEADLINE,
        transport::send_envelope(&mut client_send, &welcome()?),
    )
    .await??;

    let (server_send, mut server_receive) =
        tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
    let result = tokio::time::timeout(
        TEST_DEADLINE,
        transport::receive_client_hello(&mut server_receive),
    )
    .await?;
    assert!(matches!(
        result,
        Err(transport::HandshakeError::UnexpectedMessage {
            expected: transport::HandshakeMessageKind::Hello,
            received: transport::HandshakeMessageKind::Welcome,
        })
    ));

    client_send.finish()?;
    drop(client_send);
    drop(client_receive);
    drop(server_send);
    drop(server_receive);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}

#[tokio::test]
async fn client_preserves_typed_server_rejection() -> Result<(), Box<dyn Error>> {
    let mut loopback = spawn_loopback();
    let client_connection = connect_client(&loopback).await;
    let server_connection = server_connection(&mut loopback).await;
    let (mut client_send, mut client_receive) = client_connection.open_bi().await?;

    let client = tokio::time::timeout(
        TEST_DEADLINE,
        transport::client_handshake(&mut client_send, &mut client_receive, hello()?),
    );
    let server = async {
        let (mut server_send, mut server_receive) =
            tokio::time::timeout(TEST_DEADLINE, server_connection.accept_bi()).await??;
        let received = tokio::time::timeout(
            TEST_DEADLINE,
            transport::receive_client_hello(&mut server_receive),
        )
        .await??;
        tokio::time::timeout(
            TEST_DEADLINE,
            transport::send_envelope(&mut server_send, &rejection()?),
        )
        .await??;
        server_send.finish()?;
        Ok::<_, Box<dyn Error>>((received, server_send, server_receive))
    };

    let (client_result, server_result) = tokio::join!(client, server);
    let Err(error) = client_result? else {
        panic!("server rejection must fail the handshake");
    };
    let (received, server_send, server_receive) = server_result?;
    assert!(matches!(
        received.hello.credential,
        HelloCredential::Initial(_)
    ));
    assert!(matches!(
        error,
        transport::HandshakeError::Rejected {
            failure: ProtocolFailure {
                code: ErrorCode::InvalidInput,
                retryable: false,
                request_id: None,
                ..
            }
        }
    ));

    client_send.finish()?;
    drop(client_send);
    drop(client_receive);
    drop(server_send);
    drop(server_receive);
    drop(client_connection);
    drop(server_connection);
    drain(loopback).await;
    Ok(())
}
