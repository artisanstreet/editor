//! The control-plane host: loopback listener, WebSocket upgrades, and the
//! version-2 Cap'n Proto handshake.
//!
//! Parity anchors from `modules/forge`: the WebSocket path is `/api/ws`, the
//! first frame a peer sends must be a valid `hello` control frame, and the
//! reply is either a `welcome` (backend-origin, negotiated) or a typed
//! pre-negotiation protocol error followed by close.
//!
//! Deliberate boundary for this packet: origin allowlists and token pairing
//! are not yet enforced here. Binding loopback-only is the current security
//! boundary; the pairing and origin policy arrive with the control-authority
//! packet, and this note shrinks accordingly.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use artisan_protocol::common_capnp::Origin;
use artisan_protocol::handshake_capnp::{control_frame, hello_envelope};
use artisan_transport::frame::{FrameCodec, decode_frame, encode_frame};
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use tokio::sync::watch;

use crate::config::ForgeConfig;
use crate::lease::{ForgeDatabaseLease, LeaseOwner, acquire_lease};
use crate::state::{ForgeStateCard, remove_state_card};

/// How long the host waits for the first frame before failing the handshake.
const HANDSHAKE_TIMEOUT_SECONDS: u64 = 10;

/// Everything that can fail while bringing the host up.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Configuration was rejected before any resource was opened.
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),

    /// The database lease could not be acquired.
    #[error(transparent)]
    Lease(#[from] crate::lease::LeaseError),

    /// The listener could not bind.
    #[error("failed to bind {host}:{port}: {source}")]
    Bind {
        /// Configured listen host.
        host: &'static str,
        /// Configured listen port.
        port: u16,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// The state card could not be written.
    #[error("failed to write forge state card: {source}")]
    State {
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
}

/// A running Forge: its bound endpoint plus an awaited drain.
pub struct ForgeHandle {
    endpoint: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    drained: tokio::sync::oneshot::Receiver<()>,
}

impl ForgeHandle {
    /// The loopback endpoint actually bound (resolves configured port 0).
    #[must_use]
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }

    /// Triggers the drain and resolves when the listener is closed, sessions
    /// have ended, and the lease and state card are released.
    pub async fn shutdown(self) {
        let _unused = self.shutdown_tx.send(true);
        let _unused = self.drained.await;
    }
}

/// Resources held for the lifetime of the listener, released on drain.
struct HostResources {
    lease: ForgeDatabaseLease,
    state_path: Option<PathBuf>,
}

impl HostResources {
    fn release(&mut self) {
        let _unused = self.lease.release();
        if let Some(path) = self.state_path.take() {
            remove_state_card(&path);
        }
    }
}

/// Brings the Forge host up: lease, listener, state card, session loop.
///
/// # Errors
/// Returns [`HostError`] for config rejection, lease conflicts, bind
/// failures, or an unwritable state card. On any failure every partially
/// acquired resource is released before returning.
pub async fn run_forge(
    config: ForgeConfig,
    state_path: Option<PathBuf>,
) -> Result<ForgeHandle, HostError> {
    let lease = acquire_lease(
        config.database_path(),
        &LeaseOwner::new(config.instance_id(), std::process::id()),
    )?;
    let mut resources = HostResources {
        lease,
        state_path: state_path.clone(),
    };

    let addr = SocketAddr::new(config.listen_host().address(), config.listen_port());
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(source) => {
            resources.release();
            return Err(HostError::Bind {
                host: config.listen_host().as_str(),
                port: config.listen_port(),
                source,
            });
        }
    };
    let endpoint = match listener.local_addr() {
        Ok(endpoint) => endpoint,
        Err(source) => {
            resources.release();
            return Err(HostError::Bind {
                host: config.listen_host().as_str(),
                port: config.listen_port(),
                source,
            });
        }
    };

    if let Some(path) = &state_path {
        let card = ForgeStateCard::new(
            endpoint.to_string(),
            config.instance_id(),
            std::process::id(),
        );
        if let Err(source) = card.write_atomic(path) {
            resources.release();
            return Err(HostError::State { source });
        }
    }

    let config = Arc::new(config);
    let websocket_path = config.websocket_path().to_string();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let app_shutdown = shutdown_rx.clone();
    let app = Router::new().route(
        websocket_path.as_str(),
        get(move |upgrade: WebSocketUpgrade| {
            let config = Arc::clone(&config);
            let session_shutdown = app_shutdown.clone();
            async move {
                upgrade.on_upgrade(move |socket| handle_session(socket, config, session_shutdown))
            }
        }),
    );

    let (drained_tx, drained_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut listener_shutdown = shutdown_rx.clone();
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _unused = listener_shutdown.changed().await;
        });
        let _unused = server.await;
        resources.release();
        let _unused = drained_tx.send(());
    });

    Ok(ForgeHandle {
        endpoint,
        shutdown_tx,
        drained: drained_rx,
    })
}

async fn handle_session(
    socket: WebSocket,
    config: Arc<ForgeConfig>,
    mut shutdown: watch::Receiver<bool>,
) {
    use futures_util::{SinkExt, StreamExt};

    let codec = FrameCodec::new();
    let (mut sender, mut receiver) = socket.split();

    let first = tokio::time::timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECONDS),
        receiver.next(),
    )
    .await;
    let (reply_bytes, negotiated) = first_frame_reply(&codec, &config, first);
    if let Some(bytes) = reply_bytes {
        if sender.send(Message::Binary(bytes.into())).await.is_err() {
            return;
        }
    }
    if !negotiated {
        return;
    }

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                let _unused = sender.send(Message::Close(None)).await;
                return;
            }
            incoming = receiver.next() => match incoming {
                None | Some(Ok(Message::Close(_)) | Err(_)) => return,
                Some(Ok(Message::Binary(bytes))) => {
                    if decode_frame(&codec, &bytes).is_err() {
                        let _unused = sender.send(Message::Close(None)).await;
                        return;
                    }
                }
                Some(Ok(_)) => {}
            },
        }
    }
}

type FirstFrame = Result<Option<Result<Message, axum::Error>>, tokio::time::error::Elapsed>;

/// Returns the bytes to send and whether the session negotiated successfully.
fn first_frame_reply(
    codec: &FrameCodec,
    config: &ForgeConfig,
    first: FirstFrame,
) -> (Option<Vec<u8>>, bool) {
    let Ok(Some(Ok(Message::Binary(bytes)))) = first else {
        return (Some(pre_negotiation_error("handshake_timeout")), false);
    };
    match decode_frame(codec, &bytes) {
        Ok(frame) => match frame.get_root::<control_frame::Reader>() {
            Ok(root) => match root.which() {
                Ok(control_frame::Which::Hello(Ok(hello))) => welcome_reply(config, hello),
                Ok(control_frame::Which::Hello(Err(_))) => {
                    (Some(pre_negotiation_error("malformed_hello")), false)
                }
                _ => (Some(pre_negotiation_error("hello_required")), false),
            },
            Err(_) => (Some(pre_negotiation_error("malformed_hello")), false),
        },
        Err(_) => (Some(pre_negotiation_error("malformed_hello")), false),
    }
}

fn welcome_reply(
    config: &ForgeConfig,
    hello: hello_envelope::Reader<'_>,
) -> (Option<Vec<u8>>, bool) {
    if hello.get_schema_version() != 1 {
        return (
            Some(pre_negotiation_error("unsupported_schema_version")),
            false,
        );
    }
    match hello.get_origin() {
        Ok(Origin::Frontend) => {}
        _ => {
            return (
                Some(pre_negotiation_error("origin_must_be_frontend")),
                false,
            );
        }
    }
    let payload = hello.get_payload();
    let Ok(supported) = payload.get_supported_protocol_versions() else {
        return (Some(pre_negotiation_error("malformed_hello")), false);
    };
    let supported_versions: Vec<u16> = supported.iter().collect();
    if !supported_versions.contains(&2) {
        return (
            Some(pre_negotiation_error("no_common_protocol_version")),
            false,
        );
    }
    let correlation_id = match hello.get_message_id() {
        Ok(id) => match id.to_str() {
            Ok(text) => text.to_string(),
            Err(_) => return (Some(pre_negotiation_error("malformed_hello")), false),
        },
        Err(_) => return (Some(pre_negotiation_error("malformed_hello")), false),
    };

    let bytes = encode_frame(FrameCodec::new().max_frame_bytes(), |message| {
        let mut control = message.init_root::<control_frame::Builder>();
        let mut welcome = control.reborrow().init_welcome();
        welcome.set_message_id(uuid::Uuid::new_v4().to_string().as_str());
        welcome.set_origin(Origin::Backend);
        welcome.set_protocol_version(2);
        welcome.set_schema_version(1);
        welcome.set_sent_at(crate::state::state_now_rfc3339().as_str());
        welcome.set_correlation_id(correlation_id.as_str());
        let mut reply = welcome.reborrow().init_payload();
        reply.set_connection_id(uuid::Uuid::new_v4().to_string().as_str());
        reply.set_heartbeat_interval_ms(
            u32::try_from(config.heartbeat_interval_ms()).unwrap_or(u32::MAX),
        );
        reply.set_heartbeat_timeout_ms(
            u32::try_from(config.heartbeat_timeout_ms()).unwrap_or(u32::MAX),
        );
        reply.set_journal_sequence(0);
        reply.set_stream_ticket(uuid::Uuid::new_v4().to_string().as_str());
        reply.init_current_event_cursors(0);
    });
    match bytes {
        Ok(bytes) => (Some(bytes), true),
        // A bound-exceeding welcome cannot occur at these field sizes; treat
        // any such surprise as close-after-error rather than sending garbage.
        Err(_) => (None, false),
    }
}

fn pre_negotiation_error(code: &'static str) -> Vec<u8> {
    encode_frame(FrameCodec::new().max_frame_bytes(), |message| {
        let mut control = message.init_root::<control_frame::Builder>();
        let mut error = control.reborrow().init_pre_negotiation_protocol_error();
        error.set_message_id(uuid::Uuid::new_v4().to_string().as_str());
        error.set_origin(Origin::Backend);
        error.set_schema_version(1);
        error.set_sent_at(crate::state::state_now_rfc3339().as_str());
        let mut detail = error.init_payload();
        detail.set_code(code);
        detail.set_message("the first control frame must be a valid version-2 hello");
        detail.set_retryable(true);
    })
    .unwrap_or_default()
}
