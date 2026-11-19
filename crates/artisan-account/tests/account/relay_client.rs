use super::*;
use tokio::net::TcpListener;

const WAIT: Duration = Duration::from_secs(2);

async fn listen() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    (listener, url)
}

#[tokio::test]
async fn the_socket_answers_pings_skips_noise_and_ends_on_channel_close() {
    let (listener, url) = listen().await;
    let relay = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.send(Ws::Text(r#"{"v":1,"type":"ping","t":1234}"#.into()))
            .await
            .unwrap();
        let pong = tokio::time::timeout(WAIT, ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let pong: serde_json::Value = serde_json::from_str(pong.to_text().unwrap()).unwrap();
        assert_eq!(
            pong,
            serde_json::json!({"v": 1, "type": "pong", "t": 1234}),
            "an idle socket must answer the heartbeat"
        );
        for frame in [
            r#"{"v":1,"type":"presence","forge_online":true,"clients":2}"#,
            r#"{"v":1,"type":"from-a-newer-relay"}"#,
            "not json",
            r#"{"v":1,"type":"ack","channel":"c","seq":3}"#,
            r#"{"v":1,"type":"channel-closed","channel":"c","reason":"forge-offline"}"#,
        ] {
            ws.send(Ws::Text(frame.into())).await.unwrap();
        }
        let ended = tokio::time::timeout(WAIT, ws.next()).await.unwrap();
        assert!(
            !matches!(ended, Some(Ok(Ws::Text(_)))),
            "the socket kept talking after its channel closed"
        );
    });
    let mut socket = connect_socket(&url, CancellationToken::new())
        .await
        .unwrap();
    let frame = tokio::time::timeout(WAIT, socket.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(frame, ServerFrame::Ack(AckFrame { seq: 3, .. })),
        "presence and unknown frames must not reach the reader: {frame:?}"
    );
    let error = socket.recv().await.unwrap_err();
    assert!(format!("{error:#}").contains("forge-offline"), "{error:#}");
    tokio::time::timeout(WAIT, socket.token.cancelled())
        .await
        .expect("a closed channel marks the socket dead");
    relay.await.unwrap();
}

#[tokio::test]
async fn signing_out_ends_open_sockets() {
    let (listener, url) = listen().await;
    let relay = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let ended = tokio::time::timeout(WAIT, ws.next()).await.unwrap();
        assert!(
            !matches!(ended, Some(Ok(Ws::Text(_)))),
            "socket outlived sign-out"
        );
    });
    let client = CancellationToken::new();
    let mut socket = connect_socket(&url, client.child_token()).await.unwrap();
    client.cancel();
    let read = tokio::time::timeout(WAIT, socket.recv()).await.unwrap();
    assert!(
        read.is_err(),
        "a reader must wake when the client signs out"
    );
    relay.await.unwrap();
}

#[tokio::test]
async fn signing_out_during_subscribe_ends_it() {
    let core = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let core_url = format!("http://{}", core.local_addr().unwrap());
    let hung_core = tokio::spawn(async move {
        let (_tcp, _) = core.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let client = RelayClient {
        route: Arc::new(Route {
            relay_base: "ws://127.0.0.1:1".to_owned(),
            account: "acct_00000000000000000000000000".to_owned(),
            machine: "mch_11111111111111111111111111".to_owned(),
            forge_pub: [0; 32],
            core: CoreSessionClient::new(&core_url).unwrap(),
            session: SessionToken::new("session".to_owned()),
        }),
        unary: Arc::new(tokio::sync::Mutex::new(None)),
        cancel: CancellationToken::new(),
        exchange_deadline: EXCHANGE_DEADLINE,
    };
    let subscribing = tokio::spawn({
        let client = client.clone();
        async move {
            Box::pin(client.subscribe("/events?since=0"))
                .await
                .map(drop)
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    client.close_all();
    let outcome = tokio::time::timeout(WAIT, subscribing)
        .await
        .unwrap()
        .unwrap();
    assert!(outcome.is_err(), "a subscription survived sign-out");
    hung_core.abort();
}

#[tokio::test]
async fn every_socket_opens_a_fresh_channel_name() {
    let (listener, url) = listen().await;
    let relay = tokio::spawn(async move {
        let mut names = std::collections::HashSet::new();
        for _ in 0..4 {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let hello = ws.next().await.unwrap().unwrap();
            assert!(
                hello.to_text().unwrap().contains("\"hello\""),
                "hello first"
            );
            ws.send(Ws::Text(
                r#"{"v":1,"type":"ready","role":"client","account":"a","machine":"m","forge_online":true}"#
                    .into(),
            ))
            .await
            .unwrap();
            let open = ws.next().await.unwrap().unwrap();
            let open: serde_json::Value = serde_json::from_str(open.to_text().unwrap()).unwrap();
            assert_eq!(open["type"], "open", "open after admission");
            let name = open["channel"].as_str().unwrap().to_owned();
            assert!(names.insert(name.clone()), "channel reused: {name}");
            ws.send(Ws::Text(
                r#"{"v":1,"type":"error","code":"probe-complete","message":"open recorded"}"#
                    .into(),
            ))
            .await
            .unwrap();
        }
    });
    let route = Route {
        relay_base: url,
        account: "a".to_owned(),
        machine: "m".to_owned(),
        forge_pub: [2; 32],
        core: CoreSessionClient::new("http://127.0.0.1:1").unwrap(),
        session: SessionToken::new("session".to_owned()),
    };
    let payload = URL_SAFE_NO_PAD.encode(br#"{"jti":"channel-test","exp":4000000000}"#);
    let grant = format!("header.{payload}.signature");
    for prefix in ["unary", "unary", "events", "events"] {
        let error = route
            .open_with(prefix, &grant, &[1; 32], &CancellationToken::new())
            .await
            .err()
            .unwrap();
        assert!(
            matches!(
                &error.failure,
                OpenFailure::Socket(SocketError::Refused { code, .. }) if code == "probe-complete"
            ),
            "{error:?}"
        );
    }
    relay.await.unwrap();
}

#[test]
fn responses_read_as_responses() {
    let message =
        read_message(br#"{"id":"rs2","status":200,"body":{"unsubscribed":true}}"#).unwrap();
    assert!(
        matches!(&message, Message::Response(response) if response.id == "rs2"),
        "response misread as an event"
    );
}

#[test]
fn events_interleaved_with_a_response_are_events() {
    let message = read_message(br#"{"$event":{"stream":"events-1","end":"closed"}}"#).unwrap();
    assert!(
        matches!(&message, Message::Event(event) if event.end.as_deref() == Some("closed")),
        "event misread as a response"
    );
}

#[test]
fn malformed_answers_keep_the_decode_error() {
    let error = read_message(br#"{"id":1,"status":"bogus"}"#).err().unwrap();
    assert!(
        matches!(&error, ExchangeError::Answer(source) if source.to_string().contains("invalid type")),
        "{error:?}"
    );
}

fn confirmed_pair(channel: &str) -> NoiseChannel {
    confirmed_ends(channel).0
}

fn confirmed_ends(channel: &str) -> (NoiseChannel, NoiseChannel) {
    let ctx = ChannelCtx {
        account: "acct_00000000000000000000000000".to_owned(),
        machine: "mch_11111111111111111111111111".to_owned(),
        channel: channel.to_owned(),
    };
    let mut editor = NoiseChannel::initiator(&[3; 32], None, ctx.clone()).unwrap();
    let mut forge = NoiseChannel::responder(&[4; 32], None, ctx).unwrap();
    let mut first = Vec::new();
    editor.next_handshake_message(&mut first).unwrap();
    forge.receive_handshake_message(&first).unwrap();
    let mut second = Vec::new();
    forge.next_handshake_message(&mut second).unwrap();
    editor.receive_handshake_message(&second).unwrap();
    let mut third = Vec::new();
    editor.next_handshake_message(&mut third).unwrap();
    forge.receive_handshake_message(&third).unwrap();
    let cnf_secret = [5; 32];
    let forge_secret = [6; 32];
    let public = |secret: &[u8; 32]| {
        ed25519_dalek::SigningKey::from_bytes(secret)
            .verifying_key()
            .to_bytes()
    };
    let confirm = editor
        .emit_initiator_confirm(&cnf_secret, b"grant.jwt.body")
        .unwrap();
    let parsed = forge.receive_initiator_confirm(&confirm).unwrap();
    forge
        .accept_initiator_confirm(&public(&cnf_secret), &parsed)
        .unwrap();
    let reply = forge.emit_responder_confirm(&forge_secret).unwrap();
    editor
        .receive_responder_confirm(&reply, &public(&forge_secret))
        .unwrap();
    (editor, forge)
}

#[tokio::test]
async fn a_forge_that_never_answers_frees_the_unary_channel_for_the_next_call() {
    let (listener, url) = listen().await;
    let relay = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        while let Some(Ok(Ws::Text(text))) = ws.next().await {
            let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
            if frame["type"] == "data" {
                let ack = serde_json::json!({
                    "v": 1,
                    "type": "ack",
                    "channel": frame["channel"],
                    "seq": frame["seq"],
                });
                ws.send(Ws::Text(ack.to_string().into())).await.unwrap();
            }
        }
    });
    let channel = "unary-silent";
    let conn = Conn {
        socket: connect_socket(&url, CancellationToken::new())
            .await
            .unwrap(),
        channel: channel.to_owned(),
        noise: confirmed_pair(channel),
        send_seq: 0,
        recv_seq: 0,
        pending: VecDeque::new(),
        reassembly: Reassembly::new(),
        expires_at: now_sec() + 3600,
    };
    let client = RelayClient {
        route: Arc::new(Route {
            relay_base: "ws://127.0.0.1:1".to_owned(),
            account: "acct_00000000000000000000000000".to_owned(),
            machine: "mch_11111111111111111111111111".to_owned(),
            forge_pub: [0; 32],
            core: CoreSessionClient::new("http://127.0.0.1:1").unwrap(),
            session: SessionToken::new("session".to_owned()),
        }),
        unary: Arc::new(tokio::sync::Mutex::new(Some(conn))),
        cancel: CancellationToken::new(),
        exchange_deadline: Duration::from_millis(100),
    };
    let first = tokio::time::timeout(
        WAIT,
        client.request_json::<serde_json::Value>("PUT", "/drafts/thread", None),
    )
    .await
    .expect("a silent Forge holds the unary channel forever")
    .unwrap_err();
    assert!(matches!(*first.failure, CallFailure::TimedOut), "{first:?}");
    let unary = tokio::time::timeout(WAIT, client.unary.lock())
        .await
        .expect("the next call waits behind the silent one");
    assert!(unary.is_none(), "the half-used channel is kept for reuse");
    relay.abort();
}

#[tokio::test(start_paused = true)]
async fn a_heartbeating_event_stream_that_falls_silent_ends_instead_of_hanging() {
    let (listener, url) = listen().await;
    let channel = "events-silent";
    let (editor, mut forge) = confirmed_ends(channel);
    let relay = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let envelope = Envelope {
            seq: Some(7),
            live_revision: 0,
            event: artisan_api::Event::Reset,
        };
        for (seq, event) in [
            serde_json::json!({"$event": {"stream": channel, "envelope": envelope}}),
            serde_json::json!({"$event": {"stream": channel}}),
        ]
        .into_iter()
        .enumerate()
        {
            let sealed = forge.seal(&serde_json::to_vec(&event).unwrap()).unwrap();
            let frame = ServerFrame::Data(crate::frames::DataFrame {
                v: 1,
                channel: channel.to_owned(),
                seq: u64::try_from(seq).unwrap(),
                body: URL_SAFE_NO_PAD.encode(sealed),
            });
            ws.send(Ws::Text(serde_json::to_string(&frame).unwrap().into()))
                .await
                .unwrap();
        }
        while let Some(Ok(_)) = ws.next().await {}
    });
    let mut stream = RelayEventStream {
        conn: Conn {
            socket: connect_socket(&url, CancellationToken::new())
                .await
                .unwrap(),
            channel: channel.to_owned(),
            noise: editor,
            send_seq: 0,
            recv_seq: 0,
            pending: VecDeque::new(),
            reassembly: Reassembly::new(),
            expires_at: now_sec() + 3600,
        },
        stream_id: channel.to_owned(),
        liveness: Liveness::Unproven,
        cancel: CancellationToken::new(),
    };
    let silence = Duration::from_secs(45);
    let first = stream.next_event(silence).await.unwrap();
    assert_eq!(first.and_then(|envelope| envelope.seq), Some(7));
    let started = tokio::time::Instant::now();
    let error = stream.next_event(silence).await.unwrap_err();
    assert!(
        matches!(&error, EventStreamError::Silent { stream, silent_for } if stream == channel && *silent_for == silence),
        "{error:?}"
    );
    assert!(
        tokio::time::Instant::now() - started >= silence,
        "the stream is given the whole silence deadline after its heartbeat"
    );
    relay.abort();
}
