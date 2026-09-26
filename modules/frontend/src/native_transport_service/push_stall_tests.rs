//! The Editor keeps reading the Forge's pushes while it waits for a response.
//!
//! A scripted Forge on its own thread and runtime serves one connection the
//! way the real one does: strictly in order, pushing after a response has
//! been written, and failing the connection when a push does not settle
//! within its send limit. The Editor side is the real service runtime with a
//! real session and delivery task.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use artisan_domain::{
    ConversationCursor, ConversationPatch, IncrementalText, ItemId, PatchBatch, PatchId,
    PatchSequence, Revision, ThreadId, UnixMillis,
};
use artisan_protocol::{
    ConnectionId, ErrorCode, ErrorDetail, FrameId, Hello, HelloCredential, LocalCapability,
    ProtocolFailure, ProtocolVersion, ReconnectCapability, VersionOffer, Welcome, WireEnvelope,
    WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, ClientSession, ClientSessionLimits, LoopbackTarget, PinnedIdentity,
};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

use super::{
    FrameFactory, NativeTransportCommand, NativeTransportEvent, PrivateDelivery, QueuedCommand,
    ServiceRuntime, command_loop_with_delivery, delivery_task_loop,
};

/// The Forge's per-push send limit, shortened from its production 30 s.
const PUSH_LIMIT: Duration = Duration::from_secs(2);

/// Failure-only watchdog for every other wait.
const WATCHDOG: Duration = Duration::from_secs(20);

/// Enough pushed batches to exceed the Editor's delivery channel and the
/// stream's receive window many times over.
const PUSHED_BATCHES: u64 = 200;

/// Patches per batch; each carries a full text fragment.
const PATCHES_PER_BATCH: u64 = 5;

fn identity() -> (CertificateDer<'static>, PrivatePkcs8KeyDer<'static>) {
    let key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("identity");
    (
        key.cert.der().clone(),
        PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()),
    )
}

fn thread() -> ThreadId {
    ThreadId::parse("push-stall-thread").expect("thread")
}

fn pushed_batch(index: u64) -> PatchBatch {
    let from = index * PATCHES_PER_BATCH;
    let fragment = "x".repeat(artisan_domain::CONVERSATION_TEXT_FRAGMENT_MAX_BYTES);
    let patches = (1..=PATCHES_PER_BATCH)
        .map(|offset| {
            let sequence = from + offset;
            ConversationPatch::ItemAppend {
                patch_id: PatchId::parse(format!("patch-{sequence}")).expect("patch"),
                sequence: PatchSequence::new(sequence).expect("sequence"),
                item_id: ItemId::parse("item-a").expect("item"),
                revision: Revision::new(sequence),
                text: IncrementalText::parse(fragment.clone()).expect("fragment"),
                updated_at: UnixMillis::EPOCH,
            }
        })
        .collect();
    PatchBatch::new(
        thread(),
        ConversationCursor::new(from),
        ConversationCursor::new(from + PATCHES_PER_BATCH),
        patches,
    )
    .expect("batch")
}

fn server_frame(name: String, body: WireEnvelopeBody) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(name).expect("frame"),
        sent_at: UnixMillis::from_millis(1),
        body,
    }
}

/// What the scripted Forge observed.
#[derive(Debug, Eq, PartialEq)]
enum ForgeOutcome {
    /// Every push settled and the next request was answered.
    Served,
    /// A push did not settle within its send limit.
    PushStalled { batch: u64 },
    /// A scripted step failed for another reason.
    Broken(String),
}

/// Serves: handshake, one request, the pushes, a second request.
async fn serve(
    connection: quinn::Connection,
    delivery: &mut Option<quinn::SendStream>,
) -> ForgeOutcome {
    let welcome = || {
        server_frame(
            "forge-welcome".into(),
            WireEnvelopeBody::Welcome(Welcome {
                negotiated_version: ProtocolVersion::V1,
                connection_id: ConnectionId::parse("push-stall").expect("connection"),
                reconnect_capability: ReconnectCapability::from_bytes([3; 32]),
                lifecycle_control_supported: false,
            }),
        )
    };
    let step = async {
        let (mut send, mut receive) = connection.accept_bi().await.map_err(|e| e.to_string())?;
        let hello = artisan_transport::receive_client_hello(&mut receive)
            .await
            .map_err(|e| e.to_string())?;
        artisan_transport::send_server_welcome(
            &mut send,
            &welcome(),
            &hello.hello.supported_versions,
        )
        .await
        .map_err(|e| e.to_string())?;
        send.finish().map_err(|e| e.to_string())?;
        answer_one(&connection, "first").await?;
        Ok::<_, String>(())
    };
    if let Err(error) = tokio::time::timeout(WATCHDOG, step)
        .await
        .unwrap_or_else(|_| Err("handshake or first request timed out".into()))
    {
        return ForgeOutcome::Broken(error);
    }
    let Ok(Ok(push)) = tokio::time::timeout(WATCHDOG, connection.open_uni()).await else {
        return ForgeOutcome::Broken("delivery stream did not open".into());
    };
    // Like the Forge, the delivery stream lives as long as the connection.
    let push = delivery.insert(push);
    for index in 0..PUSHED_BATCHES {
        let envelope = server_frame(
            format!("push-{index}"),
            WireEnvelopeBody::PatchBatch(pushed_batch(index)),
        );
        match tokio::time::timeout(
            PUSH_LIMIT,
            artisan_transport::send_envelope(push, &envelope),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return ForgeOutcome::Broken(error.to_string()),
            Err(_) => {
                connection.close(1u32.into(), b"send did not settle");
                return ForgeOutcome::PushStalled { batch: index };
            }
        }
    }
    match tokio::time::timeout(WATCHDOG, answer_one(&connection, "second")).await {
        Ok(Ok(())) => ForgeOutcome::Served,
        Ok(Err(error)) => ForgeOutcome::Broken(error),
        Err(_) => ForgeOutcome::Broken("second request timed out".into()),
    }
}

/// Accepts one request and answers it with a correlated failure.
async fn answer_one(connection: &quinn::Connection, name: &str) -> Result<(), String> {
    let (mut send, mut receive) = connection.accept_bi().await.map_err(|e| e.to_string())?;
    let request = artisan_transport::receive_envelope(&mut receive)
        .await
        .map_err(|e| e.to_string())?;
    let request_id = request
        .frame_id
        .to_request_id()
        .map_err(|e| e.to_string())?;
    let reply = server_frame(
        format!("reply-{name}"),
        WireEnvelopeBody::ProtocolError(ProtocolFailure {
            code: ErrorCode::Internal,
            detail: ErrorDetail::parse("scripted").expect("detail"),
            retryable: false,
            request_id: Some(request_id),
        }),
    );
    artisan_transport::send_envelope(&mut send, &reply)
        .await
        .map_err(|e| e.to_string())?;
    send.finish().map_err(|e| e.to_string())
}

/// Starts the scripted Forge on its own thread and runtime.
fn start_forge(
    certificate: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
) -> (SocketAddr, Receiver<ForgeOutcome>, JoinHandle<()>) {
    let (address_tx, address_rx) = sync_channel(1);
    let (outcome_tx, outcome_rx) = sync_channel(1);
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("forge runtime");
        runtime.block_on(async move {
            let config = artisan_transport::server_config(vec![certificate], key).expect("config");
            let server = artisan_transport::bind_loopback_server(config).expect("bind");
            address_tx
                .send(server.local_addr().expect("address"))
                .expect("address");
            let Ok(Some(incoming)) = tokio::time::timeout(WATCHDOG, server.accept()).await else {
                let _ = outcome_tx.send(ForgeOutcome::Broken("no connection".into()));
                return;
            };
            let Ok(connection) = incoming.await else {
                let _ = outcome_tx.send(ForgeOutcome::Broken("handshake failed".into()));
                return;
            };
            let mut delivery = None;
            let outcome = serve(connection.clone(), &mut delivery).await;
            let _ = outcome_tx.send(outcome);
            // Keep the connection until the Editor closes it.
            let _ = tokio::time::timeout(WATCHDOG, connection.closed()).await;
            server.close(0u32.into(), b"done");
            server.wait_idle().await;
        });
    });
    let address = address_rx.recv_timeout(WATCHDOG).expect("forge address");
    (address, outcome_rx, thread)
}

fn hello() -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("editor-hello").expect("frame"),
        sent_at: UnixMillis::from_millis(1),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1]).expect("versions"),
            credential: HelloCredential::Initial(LocalCapability::from_bytes([7; 32])),
            supports_lifecycle_control: false,
        }),
    }
}

/// Connects a real session and starts its delivery task, as the service
/// does after startup.
async fn connected_runtime(
    address: SocketAddr,
    certificate: CertificateDer<'static>,
    events: std::sync::mpsc::SyncSender<NativeTransportEvent>,
) -> ServiceRuntime {
    let mut runtime = ServiceRuntime::new_for_batch_tests();
    runtime.limits = ClientSessionLimits {
        connect: WATCHDOG,
        handshake: WATCHDOG,
        request: WATCHDOG,
        shutdown: WATCHDOG,
        admission_budget: 64,
    };
    let pin = PinnedIdentity::from_certificate(&certificate);
    let (session, _welcome) = ClientSession::connect(
        LoopbackTarget::new(address).expect("target"),
        certificate,
        pin,
        hello(),
        runtime.limits,
        &runtime.cancel,
    )
    .await
    .expect("connect");
    let (session, receiver) = session.take_delivery().expect("delivery");
    runtime.session = Some(session);
    let (delivery_tx, delivery_rx) = tokio::sync::mpsc::channel::<PrivateDelivery>(64);
    runtime.delivery_tx = Some(delivery_tx.clone());
    runtime.deliveries.attach(delivery_rx, events);
    let cancel = Arc::new(CancelHandle::new());
    runtime.delivery_join = Some(tokio::spawn(delivery_task_loop(
        receiver,
        delivery_tx,
        Arc::clone(&cancel),
        ProtocolVersion::V1,
    )));
    runtime.delivery_cancel = Some(cancel);
    runtime
        .custody
        .on_subscribe(thread(), Some(ConversationCursor::new(0)));
    runtime
}

#[test]
fn a_request_sent_during_a_long_push_is_answered_without_a_send_stall() {
    let (certificate, key) = identity();
    let (address, outcome, forge) = start_forge(certificate.clone(), key);
    let (command_tx, mut command_rx) = tokio::sync::mpsc::channel::<QueuedCommand>(8);
    let (event_tx, event_rx) = sync_channel::<NativeTransportEvent>(64);

    // The application: drains events like the UI poll, and stops the
    // service once both answers and every batch arrived.
    let shutdown = command_tx.clone();
    let application = std::thread::spawn(move || {
        let deadline = Instant::now() + WATCHDOG;
        let (mut answers, mut batches) = (0, 0);
        while Instant::now() < deadline && (answers < 2 || batches < PUSHED_BATCHES) {
            match event_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(NativeTransportEvent::RegisteredProfilesFailed(_)) => answers += 1,
                Ok(NativeTransportEvent::PatchBatch(_)) => batches += 1,
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = shutdown.blocking_send(NativeTransportCommand::Shutdown.into());
        // Keep draining so the service never blocks on a full bridge.
        while event_rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
        (answers, batches)
    });

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("editor runtime");
    runtime.block_on(async {
        // The Editor asks twice in a row; the second request is sent while
        // the Forge is still pushing what followed the first answer.
        for _ in 0..2 {
            command_tx
                .send(NativeTransportCommand::ListRegisteredProfiles.into())
                .await
                .expect("queued");
        }
        let mut service = connected_runtime(address, certificate, event_tx.clone()).await;
        let mut frames = FrameFactory::new();
        let result = tokio::time::timeout(
            WATCHDOG,
            command_loop_with_delivery(&mut command_rx, &mut service, &mut frames, &event_tx),
        )
        .await;
        if !matches!(result, Ok(Ok(()))) {
            panic!(
                "the service loop ends cleanly: {result:?}, forge: {:?}",
                outcome.recv_timeout(WATCHDOG)
            );
        }
        // The service's own teardown: stop the delivery task, then drain the
        // session so the Forge observes the close.
        assert!(service.cleanup().await.is_ok(), "clean teardown");
    });
    drop(event_tx);
    let forge_outcome = outcome.recv_timeout(WATCHDOG).expect("forge outcome");
    let (answers, batches) = application.join().expect("application thread");
    forge.join().expect("forge thread");
    assert_eq!(
        forge_outcome,
        ForgeOutcome::Served,
        "the Forge's pushes settle and the next request is answered"
    );
    assert_eq!(answers, 2, "both requests are answered");
    assert_eq!(
        batches, PUSHED_BATCHES,
        "every pushed batch reaches the application"
    );
}

#[test]
fn cancelling_the_delivery_task_ends_it_even_when_its_channel_is_full() {
    let (certificate, key) = identity();
    let (address, outcome, forge) = start_forge(certificate.clone(), key);
    let (event_tx, _event_rx) = sync_channel::<NativeTransportEvent>(4096);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("editor runtime");
    runtime.block_on(async {
        let mut service = connected_runtime(address, certificate, event_tx).await;
        // One request makes the Forge start pushing; nobody forwards the
        // deliveries, so the delivery channel fills and the task blocks.
        let mut frames = FrameFactory::new();
        let _ = service
            .request(
                &mut frames,
                super::registered_profiles_request(),
                super::ExpectedResponse::RegisteredProfiles,
            )
            .await;
        // Taking the inbox's receiver away leaves nobody to drain it.
        let parked_receiver = service.deliveries.receiver.take();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let cancel = service.delivery_cancel.take().expect("cancel");
        cancel.cancel();
        let join = service.delivery_join.take().expect("join");
        assert!(
            tokio::time::timeout(Duration::from_secs(2), join)
                .await
                .is_ok(),
            "a reconnect's cancel and join must not wait for a full channel"
        );
        drop(parked_receiver);
        assert!(service.cleanup().await.is_ok(), "clean teardown");
    });
    let _ = outcome.recv_timeout(WATCHDOG);
    forge.join().expect("forge thread");
}
