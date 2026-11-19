use super::*;
use artisan_account::NoiseChannel;
use tokio_tungstenite::tungstenite::Message as Ws;

pub(super) const ACCOUNT: &str = "acct_00000000000000000000000000";
pub(super) const MACHINE: &str = "mch_11111111111111111111111111";

pub(super) fn ctx(channel: &str) -> ChannelCtx {
    ChannelCtx {
        account: ACCOUNT.into(),
        machine: MACHINE.into(),
        channel: channel.into(),
    }
}

pub(super) fn test_issuer() -> (ed25519_dalek::SigningKey, String) {
    use base64::Engine as _;
    let key = ed25519_dalek::SigningKey::from_bytes(&[0x77_u8; 32]);
    let x = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.verifying_key().to_bytes());
    (key, x)
}

pub(super) fn machine_grant(
    issuer: &ed25519_dalek::SigningKey,
    cnf_secret: &[u8; 32],
) -> (String, String) {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let cnf_public = ed25519_dalek::SigningKey::from_bytes(cnf_secret)
        .verifying_key()
        .to_bytes();
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::json!({"alg": "EdDSA", "kid": "k1", "typ": "JWT"}).to_string());
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "v": 1,
            "iss": "https://core.example.invalid",
            "aud": "ast-relay",
            "sub": ACCOUNT,
            "mid": MACHINE,
            "role": "forge",
            "cnf": {"jwk": {"kty": "OKP", "crv": "Ed25519", "x": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cnf_public)}},
            "scope": ["relay:connect"],
            "iat": 1_700_000_000,
            "nbf": 1_700_000_000,
            "exp": 1_700_000_200,
            "jti": "tkn-relay-test-0001",
        })
        .to_string(),
    );
    let input = format!("{header}.{payload}");
    let signature = issuer.sign(input.as_bytes());
    (
        format!(
            "{input}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ),
        "tkn-relay-test-0001".into(),
    )
}

pub(super) fn editor_grant(issuer: &ed25519_dalek::SigningKey, cnf_secret: &[u8; 32]) -> String {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1_700_000_100, |elapsed| elapsed.as_secs().cast_signed());
    let cnf_public = ed25519_dalek::SigningKey::from_bytes(cnf_secret)
        .verifying_key()
        .to_bytes();
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::json!({"alg": "EdDSA", "kid": "k1", "typ": "JWT"}).to_string());
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::json!({
            "v": 1,
            "iss": "https://core.example.invalid",
            "aud": "ast-relay",
            "sub": ACCOUNT,
            "mid": MACHINE,
            "role": "client",
            "cnf": {"jwk": {"kty": "OKP", "crv": "Ed25519", "x": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(cnf_public)}},
            "scope": ["relay:connect", "forge:execute"],
            "iat": now - 10,
            "nbf": now - 10,
            "exp": now + 200,
            "jti": "tkn-editor-test-0001",
        })
        .to_string(),
    );
    let input = format!("{header}.{payload}");
    let signature = issuer.sign(input.as_bytes());
    format!(
        "{input}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

#[expect(clippy::too_many_lines, reason = "one stub-relay journey step by step")]
#[tokio::test]
async fn tunneled_state_unary_and_stream_against_stub_relay() {
    async fn recv_text(
        source: &mut futures::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        >,
    ) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(15), source.next())
            .await
            .expect("relay stub read timed out")
            .expect("relay closed the stream")
            .expect("relay read failed")
            .into_text()
            .expect("relay sent non-text")
            .to_string()
    }

    async fn send(
        sink: &mut futures::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
            Ws,
        >,
        text: String,
    ) {
        use futures::SinkExt as _;
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            sink.send(Ws::Text(text.into())),
        )
        .await
        .expect("relay stub send timed out")
        .unwrap();
    }

    let (issuer_key, issuer_x) = test_issuer();
    let (verifier, _jwks) = crate::account_auth::tests::verifier_with_stub(serde_json::json!({
        "keys": [{"kty": "OKP", "crv": "Ed25519", "x": issuer_x, "kid": "k1",
                  "use": "sig", "alg": "EdDSA"}],
    }))
    .await;
    let forge_cnf = [0x44_u8; 32];
    let editor_cnf = [0x33_u8; 32];
    let editor_noise = [0x11_u8; 32];
    let forge_enrolled_pub = ed25519_dalek::SigningKey::from_bytes(&forge_cnf)
        .verifying_key()
        .to_bytes();
    let (grant_jwt, grant_jti) = machine_grant(&issuer_key, &[0x45_u8; 32]);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let ws_url = format!("ws://{address}/v1/route/x/y");

    let stub = tokio::spawn(async move {
        let stream = tokio_tungstenite::accept_async(listener.accept().await.unwrap().0)
            .await
            .unwrap();
        let (mut sink, mut source) = stream.split();
        let hello = recv_text(&mut source).await;
        let hello: artisan_account::ClientFrame = serde_json::from_str(&hello).unwrap();
        let grant = match hello {
            artisan_account::ClientFrame::Hello(h) => h.grant,
            artisan_account::ClientFrame::Authenticate(_)
            | artisan_account::ClientFrame::Open(_)
            | artisan_account::ClientFrame::Data(_)
            | artisan_account::ClientFrame::Ack(_)
            | artisan_account::ClientFrame::Close(_)
            | artisan_account::ClientFrame::Pong(_) => panic!("expected hello"),
        };
        assert!(!grant.is_empty());
        let nonce = [0x66_u8; 32];
        send(
            &mut sink,
            serde_json::to_string(&artisan_account::ServerFrame::Challenge(
                artisan_account::ChallengeFrame {
                    v: 1,
                    nonce: b64u_encode(&nonce),
                    timeout_ms: 60_000,
                },
            ))
            .unwrap(),
        )
        .await;
        let auth = recv_text(&mut source).await;
        let auth: artisan_account::ClientFrame = serde_json::from_str(&auth).unwrap();
        match auth {
            artisan_account::ClientFrame::Authenticate(a) => {
                assert!(!a.sig.is_empty());
            }
            artisan_account::ClientFrame::Hello(_)
            | artisan_account::ClientFrame::Open(_)
            | artisan_account::ClientFrame::Data(_)
            | artisan_account::ClientFrame::Ack(_)
            | artisan_account::ClientFrame::Close(_)
            | artisan_account::ClientFrame::Pong(_) => panic!("expected authenticate"),
        }
        send(
            &mut sink,
            serde_json::to_string(&artisan_account::ServerFrame::Ping(
                artisan_account::PingFrame { v: 1, t: 1234 },
            ))
            .unwrap(),
        )
        .await;
        let pong: serde_json::Value = serde_json::from_str(&recv_text(&mut source).await).unwrap();
        assert_eq!(pong, serde_json::json!({"v": 1, "type": "pong", "t": 1234}));
        send(
            &mut sink,
            serde_json::to_string(&artisan_account::ServerFrame::Ready(
                artisan_account::ReadyFrame {
                    v: 1,
                    role: "forge".into(),
                    account: "a".into(),
                    machine: "m".into(),
                    forge_online: true,
                },
            ))
            .unwrap(),
        )
        .await;
        send(
            &mut sink,
            serde_json::to_string(&artisan_account::ServerFrame::ChannelOpened(
                artisan_account::ChannelOpenedFrame {
                    v: 1,
                    channel: "ch1".into(),
                    peer: "client".into(),
                },
            ))
            .unwrap(),
        )
        .await;

        let mut init = NoiseChannel::initiator(&editor_noise, None, ctx("ch1")).unwrap();
        let mut relay_seq = 0_u64;
        let mut send_data = async |sink: &mut futures::stream::SplitSink<_, Ws>, raw: Vec<u8>| {
            let frame = serde_json::to_string(&artisan_account::ClientFrame::Data(
                artisan_account::DataFrame {
                    v: 1,
                    channel: "ch1".into(),
                    seq: relay_seq,
                    body: b64u_encode(&raw),
                },
            ))
            .unwrap();
            relay_seq += 1;
            sink.send(Ws::Text(frame.into())).await.unwrap();
        };
        let mut next_server_frame = async || -> artisan_account::ServerFrame {
            loop {
                let text = recv_text(&mut source).await;
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value.get("type").and_then(|t| t.as_str()) == Some("ack") {
                    let ack: artisan_account::ClientFrame = serde_json::from_value(value).unwrap();
                    match ack {
                        artisan_account::ClientFrame::Ack(a) => assert_eq!(a.channel, "ch1"),
                        artisan_account::ClientFrame::Hello(_)
                        | artisan_account::ClientFrame::Authenticate(_)
                        | artisan_account::ClientFrame::Open(_)
                        | artisan_account::ClientFrame::Data(_)
                        | artisan_account::ClientFrame::Close(_)
                        | artisan_account::ClientFrame::Pong(_) => panic!("expected ack shape"),
                    }
                    continue;
                }
                return artisan_account::read_server_frame(value).unwrap();
            }
        };

        let mut m1 = Vec::new();
        init.next_handshake_message(&mut m1).unwrap();
        send_data(&mut sink, m1).await;
        let m2 = match next_server_frame().await {
            ServerFrame::Data(data) => b64u_decode(&data.body).unwrap(),
            other @ (ServerFrame::Challenge(_)
            | ServerFrame::Ready(_)
            | ServerFrame::ChannelOpened(_)
            | ServerFrame::ChannelClosed(_)
            | ServerFrame::Ack(_)
            | ServerFrame::Presence(_)
            | ServerFrame::Error(_)
            | ServerFrame::Ping(_)) => panic!("expected data, got {other:?}"),
        };
        init.receive_handshake_message(&m2).unwrap();
        let mut m3 = Vec::new();
        init.next_handshake_message(&mut m3).unwrap();
        send_data(&mut sink, m3).await;
        let grant = editor_grant(&issuer_key, &editor_cnf);
        let c1 = init
            .emit_initiator_confirm(&editor_cnf, grant.as_bytes())
            .unwrap();
        send_data(&mut sink, c1).await;
        let c2 = match next_server_frame().await {
            ServerFrame::Data(data) => b64u_decode(&data.body).unwrap(),
            other @ (ServerFrame::Challenge(_)
            | ServerFrame::Ready(_)
            | ServerFrame::ChannelOpened(_)
            | ServerFrame::ChannelClosed(_)
            | ServerFrame::Ack(_)
            | ServerFrame::Presence(_)
            | ServerFrame::Error(_)
            | ServerFrame::Ping(_)) => panic!("expected data, got {other:?}"),
        };
        init.receive_responder_confirm(&c2, &forge_enrolled_pub)
            .unwrap();
        assert_eq!(init.phase(), artisan_account::Phase::Open);

        let request = artisan_api::TunnelRequest {
            id: "r1".into(),
            method: "GET".into(),
            path: "/engines".into(),
            body: None,
        };
        let ct = init.seal(&serde_json::to_vec(&request).unwrap()).unwrap();
        send_data(&mut sink, ct).await;
        let answer = match next_server_frame().await {
            ServerFrame::Data(data) => b64u_decode(&data.body).unwrap(),
            other @ (ServerFrame::Challenge(_)
            | ServerFrame::Ready(_)
            | ServerFrame::ChannelOpened(_)
            | ServerFrame::ChannelClosed(_)
            | ServerFrame::Ack(_)
            | ServerFrame::Presence(_)
            | ServerFrame::Error(_)
            | ServerFrame::Ping(_)) => panic!("expected data, got {other:?}"),
        };
        let plaintext = init.receive_data(&answer).unwrap();
        let response: artisan_api::TunnelResponse = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(response.id, "r1");
        assert_eq!(response.status, 200);
        assert!(response.body.is_array(), "engines list must round-trip");
    });

    let app = test_app();
    let serve = tokio::spawn(serve_channel(
        ws_url,
        grant_jwt,
        grant_jti,
        [0x45_u8; 32],
        forge_cnf,
        ACCOUNT.into(),
        MACHINE.into(),
        std::sync::Arc::new(verifier),
        app,
    ));
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(90), stub).await;
    serve.abort();
    outcome.expect("relay e2e journey timed out").unwrap();
}

pub(super) fn test_app() -> crate::routes::App {
    let dir = tempfile::tempdir().unwrap();
    let dir = Box::leak(Box::new(dir)).path().to_owned();
    let store = forge_store::Store::open(&dir).unwrap();
    let engines = forge_engines::Engines::new(&dir);
    let hub = forge_runs::Hub::new(store, engines, dir);
    crate::routes::App {
        hub,
        forge_version: "test".into(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        tasks: tokio_util::task::TaskTracker::new(),
        repositories: std::sync::Arc::new(crate::repository::Repositories::new().unwrap()),
        icons: std::sync::Arc::default(),
    }
}

#[test]
fn first_cumulative_ack_releases_exactly_what_was_sent() {
    let mut window = SendWindow::new();
    window.noted_sent(0, 100);
    window.noted_sent(1, 100);
    window.noted_sent(2, 100);
    window.on_ack(1);
    assert_eq!(window.acked_high, Some(1));
    assert_eq!(window.sizes.len(), 1);
    assert_eq!(window.unacked_bytes, 100);
    window.on_ack(2);
    assert_eq!(window.acked_high, Some(2));
    assert!(window.sizes.is_empty());
    assert_eq!(window.unacked_bytes, 0);
    window.on_ack(2);
    assert_eq!(window.acked_high, Some(2));
    window.on_ack(9);
    assert_eq!(window.acked_high, Some(2));
}

fn claims(exp: i64) -> artisan_account::GrantClaims {
    serde_json::from_value(serde_json::json!({
        "v": 1,
        "iss": "https://core.example.invalid",
        "aud": "ast-relay",
        "sub": ACCOUNT,
        "mid": MACHINE,
        "role": "client",
        "cnf": {"jwk": {"kty": "OKP", "crv": "Ed25519", "x": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}},
        "scope": ["relay:connect", "forge:execute"],
        "iat": exp - 300,
        "exp": exp,
        "jti": "tkn-editor-test-0002",
    }))
    .unwrap()
}

fn open_channel(exp: i64) -> HashMap<String, Channel> {
    let noise = NoiseChannel::responder(&[0x12_u8; 32], None, ctx("ch1")).unwrap();
    [(
        "ch1".to_owned(),
        Channel {
            instance: 1,
            noise,
            grant: Grant::Verified(claims(exp)),
            account: ACCOUNT.into(),
            machine: MACHINE.into(),
            next_receive: 0,
            window: SendWindow::new(),
            stream: None,
            requests: tokio::task::JoinSet::new(),
        },
    )]
    .into()
}

#[test]
fn events_queued_before_the_grant_expired_close_the_channel_instead_of_sending() {
    let channels = open_channel(1_700_000_100);
    let budget = Arc::new(tokio::sync::Semaphore::new(2));
    let queued = StreamMsg::Frame(Charged {
        bytes: b"{}".to_vec(),
        _permit: budget.try_acquire_many_owned(2).unwrap(),
    });
    assert_eq!(
        outgoing("ch1", &queued, &channels, 1_700_000_099),
        Outgoing::Deliver
    );
    assert_eq!(
        outgoing("ch1", &queued, &channels, 1_700_000_100),
        Outgoing::Close {
            reason: "grant-expired"
        }
    );
}

#[tokio::test]
async fn a_channel_runs_a_bounded_number_of_requests_and_closing_it_cancels_them() {
    let mut channels = open_channel(1_700_000_100);
    let channel = channels.get_mut("ch1").unwrap();
    let mut waiting = Vec::new();
    for _ in 0..CHANNEL_REQUESTS {
        let (held, gone) = tokio::sync::oneshot::channel::<()>();
        assert!(start_request(channel, async move {
            let _held = held;
            std::future::pending::<()>().await;
        }));
        waiting.push(gone);
    }
    assert!(!start_request(channel, async {}));
    channels.remove("ch1");
    for gone in waiting {
        tokio::time::timeout(std::time::Duration::from_secs(5), gone)
            .await
            .expect("closing the channel cancels its requests")
            .unwrap_err();
    }
}

#[tokio::test(start_paused = true)]
async fn an_idle_tunneled_stream_ends_when_its_grant_expires() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let outbound = Outbound {
        tx: tx.clone(),
        budget: Arc::new(tokio::sync::Semaphore::new(QUEUED_BYTES)),
    };
    let sink = TunnelSink {
        name: "ch1".into(),
        outbound,
    };
    let expiry = tokio::time::Instant::now() + std::time::Duration::from_secs(200);
    let pump = tokio::spawn(until_expiry("ch1".into(), tx, expiry, async move {
        crate::events::EnvelopeSink::send(
            &sink,
            artisan_api::Envelope {
                seq: Some(1),
                live_revision: 0,
                event: artisan_api::Event::Reset,
            },
        )
        .await
        .unwrap();
        std::future::pending::<()>().await;
    }));
    let first = rx.recv().await.unwrap();
    assert!(matches!(first, (name, StreamMsg::Frame(_)) if name == "ch1"));
    let last = tokio::time::timeout(std::time::Duration::from_secs(600), rx.recv())
        .await
        .expect("an expired grant kept its event stream open")
        .unwrap();
    assert!(matches!(last.1, StreamMsg::Expired));
    assert!(tokio::time::Instant::now() >= expiry);
    pump.await.unwrap();
}

#[tokio::test]
async fn a_tunneled_stream_nobody_reads_holds_at_most_the_connection_budget() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let outbound = Outbound {
        tx,
        budget: Arc::new(tokio::sync::Semaphore::new(QUEUED_BYTES)),
    };
    let sink = TunnelSink {
        name: "ch1".into(),
        outbound: outbound.clone(),
    };
    let item = "x".repeat(512 * 1024);
    let pump = tokio::spawn(async move {
        loop {
            let envelope = artisan_api::Envelope {
                seq: None,
                live_revision: 0,
                event: artisan_api::Event::Suggestion {
                    thread_id: artisan_api::ThreadId("thread".into()),
                    text: Some(item.clone()),
                },
            };
            if crate::events::EnvelopeSink::send(&sink, envelope)
                .await
                .is_err()
            {
                break;
            }
        }
    });
    for _ in 0..10_000 {
        tokio::task::yield_now().await;
    }
    let queued = QUEUED_BYTES - outbound.budget.available_permits();
    assert!(queued <= QUEUED_BYTES, "{queued} bytes queued");
    assert!(
        rx.len() * 512 * 1024 <= QUEUED_BYTES,
        "{} frames of 512 KiB queued for a reader that never reads",
        rx.len()
    );
    pump.abort();
    rx.close();
}

#[tokio::test(start_paused = true)]
async fn a_quiet_tunneled_stream_still_sends_a_heartbeat_the_editor_can_time() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let sink = TunnelSink {
        name: "ch1".into(),
        outbound: Outbound {
            tx,
            budget: Arc::new(tokio::sync::Semaphore::new(QUEUED_BYTES)),
        },
    };
    let beating = tokio::spawn(async move { sink.beat().await });
    let started = tokio::time::Instant::now();
    for beat in 0..3_u32 {
        let (name, frame) = rx.recv().await.expect("the stream keeps beating");
        assert_eq!(name, "ch1");
        let StreamMsg::Frame(Charged { bytes, .. }) = frame else {
            panic!("a heartbeat is an ordinary frame on the stream");
        };
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"$event": {"stream": "ch1"}})
        );
        assert_eq!(
            tokio::time::Instant::now() - started,
            STREAM_HEARTBEAT * beat,
            "heartbeat {beat} arrives on schedule"
        );
    }
    beating.abort();
}

#[tokio::test]
async fn a_tunneled_call_with_a_broken_bytes_body_is_refused_with_the_reason() {
    let request = artisan_api::TunnelRequest {
        id: "r1".into(),
        method: "POST".into(),
        path: "/blobs".into(),
        body: Some(serde_json::json!({ "$bytes": { "content_type": "image/png" } })),
    };

    let response = dispatch_request(&request, &axum::Router::new(), &claims(1_700_000_100)).await;

    assert_eq!(response.status, 400);
    assert!(
        response
            .body
            .to_string()
            .contains("without a content type and base64url bytes"),
        "{response:?}"
    );
}
