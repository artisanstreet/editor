use super::tests::{ctx, editor_grant, machine_grant, test_app, test_issuer};
use super::*;
use tokio_tungstenite::tungstenite::Message as Ws;

type StubSink =
    futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Ws>;
type StubSource =
    futures::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>>;

async fn recv(source: &mut StubSource, within: std::time::Duration) -> serde_json::Value {
    let text = tokio::time::timeout(within, source.next())
        .await
        .expect("the relay stub waited too long for Forge")
        .expect("Forge closed the relay connection")
        .expect("reading Forge's relay frame")
        .into_text()
        .expect("Forge sent a non-text relay frame");
    serde_json::from_str(&text).expect("Forge sent invalid JSON")
}

async fn recv_frame(source: &mut StubSource, kind: &str) -> serde_json::Value {
    loop {
        let frame = recv(source, std::time::Duration::from_secs(15)).await;
        if frame["type"] == kind {
            return frame;
        }
    }
}

async fn send(sink: &mut StubSink, frame: &ServerFrame) {
    sink.send(Ws::Text(serde_json::to_string(frame).unwrap().into()))
        .await
        .unwrap();
}

async fn send_data(sink: &mut StubSink, seq: u64, raw: &[u8]) {
    let frame = ClientFrame::Data(artisan_account::DataFrame {
        v: 1,
        channel: "ch1".into(),
        seq,
        body: b64u_encode(raw),
    });
    sink.send(Ws::Text(serde_json::to_string(&frame).unwrap().into()))
        .await
        .unwrap();
}

fn data_body(frame: &serde_json::Value) -> Vec<u8> {
    b64u_decode(frame["body"].as_str().expect("a data frame carries a body")).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_grant_check_does_not_stop_the_relay_connection_answering_pings() {
    let jwks = test_support::http::StubServer::serve(axum::Router::new().route(
        "/.well-known/jwks.json",
        axum::routing::get(std::future::pending::<String>),
    ))
    .await;
    let verifier = crate::account_auth::GrantVerifier::new(
        &format!("{}/.well-known/jwks.json", jwks.url()),
        "https://core.example.invalid",
    )
    .unwrap();
    let (issuer, _) = test_issuer();
    let (grant_jwt, grant_jti) = machine_grant(&issuer, &[0x45_u8; 32]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serve = tokio::spawn(serve_channel(
        format!("ws://{address}/v1/route/x/y"),
        grant_jwt,
        grant_jti,
        [0x45_u8; 32],
        [0x44_u8; 32],
        super::tests::ACCOUNT.into(),
        super::tests::MACHINE.into(),
        Arc::new(verifier),
        test_app(),
    ));
    let stream = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
        .await
        .unwrap();
    let (mut sink, mut source) = stream.split();
    recv_frame(&mut source, "hello").await;
    send(
        &mut sink,
        &ServerFrame::Challenge(artisan_account::ChallengeFrame {
            v: 1,
            nonce: b64u_encode(&[0x66_u8; 32]),
            timeout_ms: 60_000,
        }),
    )
    .await;
    recv_frame(&mut source, "authenticate").await;
    send(
        &mut sink,
        &ServerFrame::ChannelOpened(artisan_account::ChannelOpenedFrame {
            v: 1,
            channel: "ch1".into(),
            peer: "client".into(),
        }),
    )
    .await;
    let mut editor = NoiseChannel::initiator(&[0x11_u8; 32], None, ctx("ch1")).unwrap();
    let mut first = Vec::new();
    editor.next_handshake_message(&mut first).unwrap();
    send_data(&mut sink, 0, &first).await;
    let second = data_body(&recv_frame(&mut source, "data").await);
    editor.receive_handshake_message(&second).unwrap();
    let mut third = Vec::new();
    editor.next_handshake_message(&mut third).unwrap();
    send_data(&mut sink, 1, &third).await;
    let grant = editor_grant(&issuer, &[0x33_u8; 32]);
    let confirm = editor
        .emit_initiator_confirm(&[0x33_u8; 32], grant.as_bytes())
        .unwrap();
    send_data(&mut sink, 2, &confirm).await;
    send(
        &mut sink,
        &ServerFrame::Ping(artisan_account::PingFrame { v: 1, t: 7 }),
    )
    .await;
    let pong = loop {
        let frame = recv(&mut source, std::time::Duration::from_secs(2)).await;
        if frame["type"] == "pong" {
            break frame;
        }
    };
    assert_eq!(pong["t"], 7);
    serve.abort();
}

#[test]
fn a_response_larger_than_one_send_window_reaches_the_editor_as_acks_arrive() {
    let message: Vec<u8> = (0..artisan_api::TRANSFER_MAX_BYTES)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect();
    let mut window = SendWindow::new();
    assert!(window.enqueue(Pending::Transfer {
        id: "ch1:1".into(),
        bytes: message.clone(),
        next: 0,
    }));
    let mut reassembly = Reassembly::new();
    let mut reassembled = None;
    while reassembled.is_none() {
        let mut sent = None;
        while let Some((seq, body)) = window.next_frame(|plain| Ok(plain.to_vec())).unwrap() {
            sent = Some(seq);
            let chunk: artisan_api::TunnelChunk =
                serde_json::from_slice(&b64u_decode(&body).unwrap()).unwrap();
            reassembled = reassembly.feed("ch1", &chunk).unwrap();
        }
        window.on_ack(sent.expect("acknowledgements free room for the next frames"));
    }
    assert_eq!(reassembled.unwrap(), message);
}
