//! End-to-end host tests: a real listener, real WebSocket clients, and the
//! full lifecycle from state card to lease release.

use std::path::PathBuf;

use artisan_forge::config::ForgeConfig;
use artisan_forge::host::run_forge;
use artisan_protocol::common_capnp::Origin;
use artisan_protocol::handshake_capnp::control_frame;
use artisan_transport::frame::{FrameCodec, decode_frame, encode_frame};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

struct TempHome(PathBuf);

impl TempHome {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("artisan-forge-e2e-{name}-{}", std::process::id()));
        let _unused = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp home creates");
        Self(dir)
    }

    fn config_text(&self, instance: &str) -> String {
        format!(
            "database_path = '{}'\nmigrations_path = 'unused-by-host-tests'\ninstance_id = '{instance}'\nlisten_port = 0\n",
            self.0
                .join("artisan.db")
                .to_string_lossy()
                .replace('\\', "/")
        )
    }

    fn state_path(&self) -> PathBuf {
        self.0.join("forge-state.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.0.join("artisan.db.artisan-forge.lock")
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _unused = std::fs::remove_dir_all(&self.0);
    }
}

fn build_hello(message_id: &str, supported: &[u16], schema_version: u16) -> Vec<u8> {
    encode_frame(FrameCodec::new().max_frame_bytes(), |message| {
        let mut control = message.init_root::<control_frame::Builder>();
        let mut hello = control.reborrow().init_hello();
        hello.set_message_id(message_id);
        hello.set_origin(Origin::Frontend);
        hello.set_schema_version(schema_version);
        hello.set_sent_at("2026-08-24T10:00:00.000Z");
        let mut payload = hello.reborrow().init_payload();
        payload.set_last_journal_sequence(0);
        let count = u32::try_from(supported.len()).expect("version list fits u32");
        let mut versions = payload.reborrow().init_supported_protocol_versions(count);
        for (index, version) in supported.iter().enumerate() {
            let index = u32::try_from(index).expect("index fits u32");
            versions.set(index, *version);
        }
        payload.reborrow().init_event_cursors(0);
    })
    .expect("hello encodes")
}

async fn connect(
    endpoint: std::net::SocketAddr,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://{endpoint}/api/ws");
    let (stream, _response) = tokio_tungstenite::connect_async(url)
        .await
        .expect("websocket connects");
    stream
}

async fn next_binary<S>(stream: &mut S) -> Vec<u8>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let message = stream
            .next()
            .await
            .expect("stream open")
            .expect("message ok");
        if let Message::Binary(bytes) = message {
            return bytes.to_vec();
        }
    }
}

/// Decodes and hands over the typed control root within `f`'s lifetime.
fn with_control<T>(bytes: &[u8], f: impl FnOnce(control_frame::Reader<'_>) -> T) -> T {
    let frame = decode_frame(&FrameCodec::new(), bytes).expect("frame decodes");
    f(frame
        .get_root::<control_frame::Reader>()
        .expect("typed root"))
}

#[tokio::test]
async fn happy_handshake_negotiates_and_shutdown_releases_everything() {
    let home = TempHome::new("happy");
    let state_path = home.state_path();
    let handle = run_forge(
        ForgeConfig::decode_toml(&home.config_text("forge-e2e")).expect("config decodes"),
        Some(state_path.clone()),
    )
    .await
    .expect("forge starts");
    assert!(state_path.exists(), "state card published at startup");

    let mut client = connect(handle.endpoint()).await;
    client
        .send(Message::Binary(build_hello("hello-1", &[2], 1).into()))
        .await
        .expect("hello sends");

    let reply = next_binary(&mut client).await;
    with_control(&reply, |root| {
        match root.which().expect("variant in schema") {
            control_frame::Which::Welcome(Ok(welcome)) => {
                assert_eq!(
                    welcome.get_correlation_id().expect("correlation id"),
                    "hello-1"
                );
                assert_eq!(welcome.get_origin().expect("origin"), Origin::Backend);
                assert_eq!(welcome.get_protocol_version(), 2);
                let payload = welcome.get_payload();
                assert!(
                    !payload
                        .get_connection_id()
                        .expect("connection id")
                        .is_empty(),
                    "connection id is assigned"
                );
                assert!(
                    !payload
                        .get_stream_ticket()
                        .expect("stream ticket")
                        .is_empty(),
                    "stream ticket is assigned"
                );
                assert_eq!(payload.get_heartbeat_interval_ms(), 15_000);
                assert_eq!(
                    payload.get_current_event_cursors().expect("cursors").len(),
                    0
                );
            }
            _ => panic!("expected the welcome variant"),
        }
    });

    handle.shutdown().await;
    assert!(!state_path.exists(), "state card removed on drain");
    assert!(!home.lock_path().exists(), "lease released on drain");
}

#[tokio::test]
async fn hello_without_version_support_gets_no_common_protocol_version() {
    let home = TempHome::new("noversion");
    let handle = run_forge(
        ForgeConfig::decode_toml(&home.config_text("forge-v")).expect("config decodes"),
        None,
    )
    .await
    .expect("forge starts");

    let mut client = connect(handle.endpoint()).await;
    client
        .send(Message::Binary(build_hello("hello-old", &[1, 3], 1).into()))
        .await
        .expect("hello sends");

    let reply = next_binary(&mut client).await;
    with_control(&reply, |root| match root.which().expect("variant") {
        control_frame::Which::PreNegotiationProtocolError(Ok(error)) => {
            let detail = error.get_payload().expect("error detail");
            assert_eq!(
                detail.get_code().expect("code"),
                "no_common_protocol_version"
            );
            assert!(detail.get_retryable());
        }
        _ => panic!("expected the protocol.error variant"),
    });

    handle.shutdown().await;
}

#[tokio::test]
async fn wrong_schema_version_is_rejected() {
    let home = TempHome::new("schemaver");
    let handle = run_forge(
        ForgeConfig::decode_toml(&home.config_text("forge-sv")).expect("config decodes"),
        None,
    )
    .await
    .expect("forge starts");

    let mut client = connect(handle.endpoint()).await;
    client
        .send(Message::Binary(build_hello("hello-9", &[2], 7).into()))
        .await
        .expect("hello sends");

    let reply = next_binary(&mut client).await;
    with_control(&reply, |root| match root.which().expect("variant") {
        control_frame::Which::PreNegotiationProtocolError(Ok(error)) => {
            let detail = error.get_payload().expect("error detail");
            assert_eq!(
                detail.get_code().expect("code"),
                "unsupported_schema_version"
            );
        }
        _ => panic!("expected the protocol.error variant"),
    });

    handle.shutdown().await;
}
