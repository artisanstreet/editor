//! Loopback proof for the serial conversation delivery writer.
//!
//! The fixtures use real migrated storage to obtain accepted subscription
//! activations and replay values, then send those values over real loopback
//! Quinn connections. The writer itself remains the only owner of the server
//! unidirectional stream; no connection-driver or delivery task is involved.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use artisan_backend::ServerFrameStamp;
use artisan_backend::activated_conversation_replay::{
    ActivatedConversationReplay, read_activated_conversation_replay,
};
use artisan_backend::conversation_delivery_writer::{
    ConversationDeliveryError, ConversationDeliveryWriter, ConversationReplayDelivery,
};
use artisan_backend::conversation_subscription_registry::ApplyBatchError;
use artisan_backend::request_handler::{
    ActivatedConversationSubscription, ConversationSubscriptionRegistrar, RequestHandler,
};
use artisan_database::{
    BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch, ConversationPatchReplay,
    CreateThreadInput, DispatchLeaseOwner, LaunchClaimedRun, LaunchClaimedRunOutcome,
    ProviderBindingBytes, QueueFirstMessageInput, Repository, RunLaunchCredentials, RunStartKey,
    SqliteConfig, connect,
};
use artisan_domain::{
    AssistantBody, AssistantMessagePhase, ConversationCursor, ConversationRequest,
    ConversationSubscribe, ConversationUnsubscribe, ItemId, MessageBody, MessageId, PatchBatch,
    PatchId, ProjectId, RequestId, RootPath, ThreadId, ThreadTitle, TurnId, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use artisan_protocol::{ClientRequest, FrameId, ProtocolVersion, WireEnvelope, WireEnvelopeBody};
use artisan_transport::{EnvelopeReceiveError, EnvelopeSendError, FrameError};
use quinn::{
    ClientConfig, Connection, Endpoint, ReadError, RecvStream, ServerConfig, TransportConfig,
    VarInt, WriteError,
};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};

const TEST_DEADLINE: Duration = Duration::from_secs(5);
const NO_STREAM_DEADLINE: Duration = Duration::from_millis(100);
const STREAM_RESET_CODE: VarInt = VarInt::from_u32(0x01);
const PEER_STOP_CODE: VarInt = VarInt::from_u32(0x22);

// ---------------------------------------------------------------------------
// Real loopback Quinn fixture
// ---------------------------------------------------------------------------

struct Loopback {
    server_addr: SocketAddr,
    client: Endpoint,
    server_connections: tokio::sync::mpsc::Receiver<Connection>,
    stop_server: Option<tokio::sync::oneshot::Sender<()>>,
    server_thread: Option<JoinHandle<()>>,
}

impl Loopback {
    fn join_server_thread(&mut self) {
        if let Some(stop) = self.stop_server.take() {
            let _signalled = stop.send(());
        }
        if let Some(thread) = self.server_thread.take() {
            thread.join().expect("server thread should finish");
        }
    }

    async fn drain(mut self) {
        self.join_server_thread();
        artisan_transport::shutdown(
            &self.client,
            VarInt::from_u32(0),
            b"conversation delivery writer test complete",
            TEST_DEADLINE,
        )
        .await
        .expect("client endpoint should drain");
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.join_server_thread();
    }
}

struct TestPki {
    certificate: CertificateDer<'static>,
    private_key: PrivatePkcs8KeyDer<'static>,
    pinned_identity: artisan_transport::PinnedIdentity,
}

fn test_pki() -> TestPki {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("test PKI");
    let certificate = certified_key.cert.der().clone();
    TestPki {
        pinned_identity: artisan_transport::PinnedIdentity::from_certificate(&certificate),
        private_key: PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der()),
        certificate,
    }
}

fn server_config(pki: &TestPki) -> ServerConfig {
    artisan_transport::server_config(vec![pki.certificate.clone()], pki.private_key.clone_key())
        .expect("server config")
}

fn client_config(pki: &TestPki, constrained: bool) -> ClientConfig {
    let mut config = artisan_transport::client_config(pki.certificate.clone(), pki.pinned_identity)
        .expect("client config");
    if constrained {
        let mut transport = TransportConfig::default();
        transport
            .stream_receive_window(VarInt::from_u32(16))
            .receive_window(VarInt::from_u32(16));
        config.transport_config(Arc::new(transport));
    }
    config
}

fn spawn_loopback(constrained: bool) -> Loopback {
    let pki = test_pki();
    let server = server_config(&pki);
    let client_config = client_config(&pki, constrained);
    let (address_sender, address_receiver) = channel();
    let (connections_sender, connections_receiver) = tokio::sync::mpsc::channel(1);
    let (stop_sender, mut stop_receiver) = tokio::sync::oneshot::channel();

    let server_thread = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("server runtime");
        runtime.block_on(async move {
            let server = artisan_transport::bind_loopback_server(server).expect("server bind");
            let server_addr = server.local_addr().expect("server address");
            address_sender
                .send(server_addr)
                .expect("test should receive server address");

            loop {
                let incoming = tokio::select! {
                    _ = &mut stop_receiver => break,
                    incoming = server.accept() => incoming,
                };
                let Some(incoming) = incoming else {
                    break;
                };
                let established = tokio::time::timeout(TEST_DEADLINE, incoming)
                    .await
                    .expect("server handshake should finish")
                    .expect("server connection should establish");
                if connections_sender.send(established).await.is_err() {
                    break;
                }
            }

            artisan_transport::shutdown(
                &server,
                VarInt::from_u32(0),
                b"conversation delivery writer server complete",
                TEST_DEADLINE,
            )
            .await
            .expect("server endpoint should drain");
        });
    });

    let server_addr = match address_receiver.recv_timeout(TEST_DEADLINE) {
        Ok(address) => address,
        Err(RecvTimeoutError::Timeout) => panic!("server should bind under the deadline"),
        Err(RecvTimeoutError::Disconnected) => panic!("server thread should stay alive"),
    };
    let client = artisan_transport::bind_loopback_client(client_config).expect("client bind");

    Loopback {
        server_addr,
        client,
        server_connections: connections_receiver,
        stop_server: Some(stop_sender),
        server_thread: Some(server_thread),
    }
}

async fn connected(loopback: &mut Loopback) -> (Connection, Connection) {
    let connecting = loopback
        .client
        .connect(
            loopback.server_addr,
            artisan_transport::LOOPBACK_SERVER_NAME,
        )
        .expect("connect request should be accepted");
    let client = tokio::time::timeout(TEST_DEADLINE, connecting)
        .await
        .expect("client handshake should finish")
        .expect("client connection should establish");
    let server = tokio::time::timeout(TEST_DEADLINE, loopback.server_connections.recv())
        .await
        .expect("server connection should arrive")
        .expect("server accept loop should stay alive");
    (server, client)
}

// ---------------------------------------------------------------------------
// Real repository and accepted-activation fixtures
// ---------------------------------------------------------------------------

async fn memory_repository() -> (DatabaseConnection, Repository) {
    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database)
        .await
        .expect("memory database should migrate");
    (database.clone(), Repository::new(database))
}

async fn seed_thread(repository: &Repository, label: &str) -> ThreadId {
    let project_id = ProjectId::parse(format!("project-{label}")).expect("project id");
    repository
        .attach_project(artisan_database::AttachProjectInput {
            request_id: RequestId::parse(format!("request-project-{label}")).expect("request id"),
            directory_id: artisan_domain::DirectoryId::parse(format!("directory-{label}"))
                .expect("directory id"),
            project_id: project_id.clone(),
            root_path: RootPath::parse(format!("C:/repos/{label}")).expect("root path"),
            display_name: artisan_domain::DisplayName::parse(format!("Project {label}"))
                .expect("display name"),
            attached_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("project should attach");

    let thread_id = ThreadId::parse(format!("thread-{label}")).expect("thread id");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse(format!("request-thread-{label}")).expect("request id"),
            thread_id: thread_id.clone(),
            project_id,
            title: ThreadTitle::parse(format!("Thread {label}")).expect("thread title"),
            created_at: UnixMillis::from_millis(20),
            updated_at: UnixMillis::from_millis(20),
        })
        .await
        .expect("thread should create");
    thread_id
}

struct SeededRun {
    claimed: artisan_database::ClaimedMessageDispatch,
    launched: artisan_database::LaunchedRunReceipt,
    bound: artisan_database::BoundRunReceipt,
    start_key: RunStartKey,
    credentials: RunLaunchCredentials,
}

async fn seed_run(
    repository: &Repository,
    thread_id: &ThreadId,
    label: &str,
    body: &str,
) -> SeededRun {
    let message_id = MessageId::parse(format!("message-{label}")).expect("message id");
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse(format!("request-message-{label}")).expect("request id"),
            message_id,
            thread_id: thread_id.clone(),
            body: MessageBody::parse(body.to_owned()).expect("message body"),
            accepted_at: UnixMillis::from_millis(50),
        })
        .await
        .expect("message should queue");

    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x11; 32]),
            claimed_at: UnixMillis::from_millis(100),
            lease_expires_at: UnixMillis::from_millis(600),
        })
        .await
        .expect("message should claim")
        .expect("a dispatch should be claimed");
    let run_id = artisan_domain::RunId::parse(format!("run-{label}")).expect("run id");
    let turn_id = TurnId::parse(format!("turn-{label}")).expect("turn id");
    let item_id = ItemId::parse(format!("item-{label}")).expect("item id");
    let first_patch_id = PatchId::parse(format!("patch-{label}-first")).expect("patch id");
    let second_patch_id = PatchId::parse(format!("patch-{label}-second")).expect("patch id");
    let start_key = RunStartKey::new([0x44; 32]);
    let credentials = RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]);
    let launched = match repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &item_id,
            first_patch_id: &first_patch_id,
            second_patch_id: &second_patch_id,
            operated_at: UnixMillis::from_millis(150),
            run_start_key: &start_key,
            credentials: &credentials,
        })
        .await
        .expect("run should launch")
    {
        LaunchClaimedRunOutcome::Started(receipt) => receipt,
        LaunchClaimedRunOutcome::AlreadyStarted(_) => panic!("fixture launch should be fresh"),
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16]).expect("provider binding");
    let bound = match repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(150),
            bound_at: UnixMillis::from_millis(200),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await
        .expect("run provider should bind")
    {
        BindRunProviderOutcome::Bound(receipt) => receipt,
        BindRunProviderOutcome::AlreadyBound(_) => panic!("fixture binding should be fresh"),
    };
    SeededRun {
        claimed,
        launched,
        bound,
        start_key,
        credentials,
    }
}

async fn activate(
    repository: &Repository,
    registrar: &ConversationSubscriptionRegistrar,
    request_id: &str,
    subscribe: ConversationSubscribe,
) -> ActivatedConversationSubscription {
    let handler =
        RequestHandler::with_subscription_registrar(repository.clone(), registrar.clone());
    let (response, receipt) = handler
        .respond_with_receipt(
            &RequestId::parse(request_id).expect("request id"),
            &ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe)),
        )
        .await
        .into_parts();
    assert!(response.is_ok(), "subscription preparation should succeed");
    handler
        .activate_after_response(receipt)
        .await
        .expect("subscription activation should succeed")
        .expect("subscription response should carry activation")
}

async fn unsubscribe(
    repository: &Repository,
    registrar: &ConversationSubscriptionRegistrar,
    thread_id: &ThreadId,
    request_id: &str,
) {
    let handler =
        RequestHandler::with_subscription_registrar(repository.clone(), registrar.clone());
    let (response, receipt) = handler
        .respond_with_receipt(
            &RequestId::parse(request_id).expect("request id"),
            &ClientRequest::Conversation(ConversationRequest::Unsubscribe(
                ConversationUnsubscribe {
                    thread_id: thread_id.clone(),
                },
            )),
        )
        .await
        .into_parts();
    assert!(response.is_ok(), "unsubscribe response should succeed");
    assert!(
        receipt.is_no_work(),
        "unsubscribe should not create activation work"
    );
}

async fn read_batch(
    repository: &Repository,
    subscription: ActivatedConversationSubscription,
) -> (ActivatedConversationReplay, PatchBatch) {
    let replay = read_activated_conversation_replay(repository, subscription)
        .await
        .expect("replay read should succeed");
    let batch = match replay.replay() {
        ConversationPatchReplay::Batch(batch) => batch.clone(),
        ConversationPatchReplay::Current { .. } => panic!("fixture should contain a batch"),
        ConversationPatchReplay::ResnapshotRequired { .. } => {
            panic!("fixture cursor should not require a resnapshot")
        }
    };
    (replay, batch)
}

async fn commit_assistant_start(repository: &Repository, run: &SeededRun, label: &str) {
    let item_id = ItemId::parse(format!("assistant-{label}")).expect("assistant item id");
    let activation_patch = PatchId::parse(format!("patch-{label}-activate")).expect("patch id");
    let item_patch = PatchId::parse(format!("patch-{label}-assistant")).expect("patch id");
    let body = AssistantBody::parse("assistant output").expect("assistant body");
    let outcome = repository
        .commit_run_batch(artisan_database::CommitRunBatch {
            scope: artisan_database::RunBatchScope {
                claimed: &run.claimed,
                launched: &run.launched,
                bound: &run.bound,
                run_start_key: &run.start_key,
                credentials: &run.credentials,
                expected_launch_at: UnixMillis::from_millis(150),
                expected_updated_at: UnixMillis::from_millis(200),
            },
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(250),
            activate_turn_patch_id: Some(&activation_patch),
            changes: &[artisan_database::AssistantChange::Start {
                item_id: &item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: &item_patch,
            }],
            checkpoint: artisan_database::CheckpointUpdate::Keep,
        })
        .await
        .expect("assistant batch should commit");
    assert!(
        matches!(
            outcome,
            artisan_database::CommitRunBatchOutcome::Committed(_)
        ),
        "assistant batch should be newly committed"
    );
}

fn stamp(frame_id: &str, sent_at: i64) -> ServerFrameStamp {
    ServerFrameStamp {
        frame_id: FrameId::parse(frame_id).expect("frame id"),
        sent_at: UnixMillis::from_millis(sent_at),
    }
}

async fn assert_no_uni(connection: &Connection) {
    assert!(
        tokio::time::timeout(NO_STREAM_DEADLINE, connection.accept_uni())
            .await
            .is_err(),
        "no server uni stream should be visible"
    );
}

async fn receive_envelope(stream: &mut RecvStream) -> WireEnvelope {
    tokio::time::timeout(TEST_DEADLINE, artisan_transport::receive_envelope(stream))
        .await
        .expect("envelope receive should finish")
        .expect("envelope should decode")
}

fn assert_patch_batch_frame(
    envelope: WireEnvelope,
    expected: &PatchBatch,
    frame_id: &str,
    sent_at: i64,
) {
    assert_eq!(envelope.protocol_version, ProtocolVersion::V1);
    assert_eq!(envelope.frame_id.as_str(), frame_id);
    assert_eq!(envelope.sent_at, UnixMillis::from_millis(sent_at));
    match envelope.body {
        WireEnvelopeBody::PatchBatch(actual) => assert_eq!(&actual, expected),
        WireEnvelopeBody::Event(_) => panic!("a delivery writer must not send an Event"),
        WireEnvelopeBody::Hello(_)
        | WireEnvelopeBody::Welcome(_)
        | WireEnvelopeBody::Request(_)
        | WireEnvelopeBody::Response(_)
        | WireEnvelopeBody::ProtocolError(_) => {
            panic!("a delivery writer must send the patch batch as its first frame")
        }
    }
}

async fn assert_clean_eof(stream: &mut RecvStream) {
    let result = tokio::time::timeout(TEST_DEADLINE, artisan_transport::receive_envelope(stream))
        .await
        .expect("clean EOF should be observed");
    match result {
        Err(EnvelopeReceiveError::Frame(FrameError::Truncated {
            expected: 4,
            received: 0,
        })) => {}
        Err(EnvelopeReceiveError::Frame(error)) => {
            panic!("clean EOF should produce a zero-byte truncation: {error:?}")
        }
        Err(EnvelopeReceiveError::Decode(error)) => {
            panic!("clean EOF should not produce a decode error: {error:?}")
        }
        Ok(_) => panic!("clean EOF should not produce another envelope"),
    }
}

// ---------------------------------------------------------------------------
// No-op replay outcomes and lazy opening
// ---------------------------------------------------------------------------

#[tokio::test]
async fn new_current_and_finish_without_a_frame_do_not_open_or_mutate() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "current").await;
    let registrar = ConversationSubscriptionRegistrar::new();
    let subscription = activate(
        &repository,
        &registrar,
        "request-current",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    let expected_lease = subscription.lease().clone();
    let expected_cursor = subscription.cursor();
    let replay = read_activated_conversation_replay(&repository, subscription)
        .await
        .expect("current replay should succeed");
    assert!(matches!(
        replay.replay(),
        ConversationPatchReplay::Current { cursor } if *cursor == expected_cursor
    ));

    let mut loopback = spawn_loopback(false);
    let (server_connection, client_connection) = connected(&mut loopback).await;
    let _server_keepalive = server_connection.clone();
    let writer =
        ConversationDeliveryWriter::new(server_connection, registrar.clone(), ProtocolVersion::V1);
    assert_no_uni(&client_connection).await;
    let (writer, outcome) = tokio::time::timeout(
        TEST_DEADLINE,
        writer.deliver(stamp("frame-current", 1_001), replay),
    )
    .await
    .expect("current delivery should finish")
    .expect("current delivery should succeed");
    match outcome {
        ConversationReplayDelivery::Current {
            subscription,
            cursor,
        } => {
            assert_eq!(subscription.lease(), &expected_lease);
            assert_eq!(subscription.cursor(), expected_cursor);
            assert_eq!(cursor, expected_cursor);
        }
        ConversationReplayDelivery::Published { .. }
        | ConversationReplayDelivery::ResnapshotRequired { .. } => {
            panic!("current replay must remain current")
        }
    }
    assert_no_uni(&client_connection).await;
    let view = registrar
        .subscription_view(&thread_id)
        .await
        .expect("active subscription should remain registered");
    assert_eq!(view.lease(), &expected_lease);
    assert_eq!(view.cursor(), expected_cursor);
    writer
        .finish()
        .expect("a never-opened writer should finish cleanly");
    assert_no_uni(&client_connection).await;
    drop(client_connection);
    loopback.drain().await;
}

#[tokio::test]
async fn resnapshot_required_returns_exact_cursors_without_opening_or_mutating() {
    let (database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "resnapshot").await;
    let run = seed_run(&repository, &thread_id, "resnapshot", "first body").await;
    let registrar = ConversationSubscriptionRegistrar::new();
    let subscription = activate(
        &repository,
        &registrar,
        "request-resnapshot",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    let expected_lease = subscription.lease().clone();
    let expected_cursor = subscription.cursor();
    assert_eq!(expected_cursor, ConversationCursor::new(2));
    drop(run);

    let state =
        artisan_database::entities::conversation_state::Entity::find_by_id(thread_id.as_str())
            .one(&database)
            .await
            .expect("conversation state should load")
            .expect("conversation state should exist");
    let mut state = artisan_database::entities::conversation_state::ActiveModel::from(state);
    state.last_patch_sequence = Set(0);
    state
        .update(&database)
        .await
        .expect("fixture state should rewind");

    let replay = read_activated_conversation_replay(&repository, subscription)
        .await
        .expect("resnapshot replay should succeed");
    assert!(matches!(
        replay.replay(),
        ConversationPatchReplay::ResnapshotRequired {
            requested_cursor,
            current_cursor,
        } if *requested_cursor == expected_cursor && *current_cursor == ConversationCursor::default()
    ));

    let mut loopback = spawn_loopback(false);
    let (server_connection, client_connection) = connected(&mut loopback).await;
    let _server_keepalive = server_connection.clone();
    let writer =
        ConversationDeliveryWriter::new(server_connection, registrar.clone(), ProtocolVersion::V1);
    assert_no_uni(&client_connection).await;
    let (writer, outcome) = tokio::time::timeout(
        TEST_DEADLINE,
        writer.deliver(stamp("frame-resnapshot", 1_002), replay),
    )
    .await
    .expect("resnapshot delivery should finish")
    .expect("resnapshot delivery should succeed");
    match outcome {
        ConversationReplayDelivery::ResnapshotRequired {
            subscription,
            requested_cursor,
            current_cursor,
        } => {
            assert_eq!(subscription.lease(), &expected_lease);
            assert_eq!(subscription.cursor(), expected_cursor);
            assert_eq!(requested_cursor, expected_cursor);
            assert_eq!(current_cursor, ConversationCursor::default());
        }
        ConversationReplayDelivery::Current { .. }
        | ConversationReplayDelivery::Published { .. } => {
            panic!("resnapshot replay must remain resnapshot-required")
        }
    }
    assert_no_uni(&client_connection).await;
    let view = registrar
        .subscription_view(&thread_id)
        .await
        .expect("subscription should remain registered");
    assert_eq!(view.lease(), &expected_lease);
    assert_eq!(view.cursor(), expected_cursor);
    writer
        .finish()
        .expect("never-opened resnapshot writer should finish");
    drop(client_connection);
    loopback.drain().await;
}

// ---------------------------------------------------------------------------
// Successful publication and stream reuse
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_batch_is_the_first_wire_frame_and_advances_after_send() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "first").await;
    let registrar = ConversationSubscriptionRegistrar::new();
    let subscription = activate(
        &repository,
        &registrar,
        "request-first",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    let expected_lease = subscription.lease().clone();
    let expected_activation_cursor = subscription.cursor();
    let _run = seed_run(&repository, &thread_id, "first", "first body").await;
    let (replay, expected_batch) = read_batch(&repository, subscription).await;

    let mut loopback = spawn_loopback(false);
    let (server_connection, client_connection) = connected(&mut loopback).await;
    let _server_keepalive = server_connection.clone();
    let writer =
        ConversationDeliveryWriter::new(server_connection, registrar.clone(), ProtocolVersion::V1);
    assert_no_uni(&client_connection).await;
    let (writer, outcome) = tokio::time::timeout(
        TEST_DEADLINE,
        writer.deliver(stamp("frame-first-batch", 2_001), replay),
    )
    .await
    .expect("first batch delivery should finish")
    .expect("first batch delivery should succeed");
    match outcome {
        ConversationReplayDelivery::Published {
            subscription,
            cursor,
        } => {
            assert_eq!(subscription.lease(), &expected_lease);
            assert_eq!(subscription.cursor(), expected_activation_cursor);
            assert_eq!(cursor, ConversationCursor::new(2));
        }
        ConversationReplayDelivery::Current { .. }
        | ConversationReplayDelivery::ResnapshotRequired { .. } => {
            panic!("a real replay batch must publish")
        }
    }

    let mut incoming = tokio::time::timeout(TEST_DEADLINE, client_connection.accept_uni())
        .await
        .expect("one incoming uni stream should become visible")
        .expect("incoming uni stream should be accepted");
    let envelope = receive_envelope(&mut incoming).await;
    assert_patch_batch_frame(envelope, &expected_batch, "frame-first-batch", 2_001);
    let view = registrar
        .subscription_view(&thread_id)
        .await
        .expect("published subscription should remain registered");
    assert_eq!(view.cursor(), ConversationCursor::new(2));
    writer
        .finish()
        .expect("published stream should finish cleanly");
    assert_clean_eof(&mut incoming).await;
    drop(incoming);
    drop(client_connection);
    loopback.drain().await;
}

#[tokio::test]
async fn second_contiguous_batch_reuses_one_stream_and_advances_once() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "reuse").await;
    let registrar = ConversationSubscriptionRegistrar::new();
    let subscription = activate(
        &repository,
        &registrar,
        "request-reuse-first",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    let run = seed_run(&repository, &thread_id, "reuse", "first body").await;
    let (replay, first_batch) = read_batch(&repository, subscription).await;

    let mut loopback = spawn_loopback(false);
    let (server_connection, client_connection) = connected(&mut loopback).await;
    let _server_keepalive = server_connection.clone();
    let writer =
        ConversationDeliveryWriter::new(server_connection, registrar.clone(), ProtocolVersion::V1);
    let (writer, first_outcome) = tokio::time::timeout(
        TEST_DEADLINE,
        writer.deliver(stamp("frame-reuse-first", 2_101), replay),
    )
    .await
    .expect("first reuse delivery should finish")
    .expect("first reuse delivery should succeed");
    let first_cursor = match first_outcome {
        ConversationReplayDelivery::Published {
            subscription: _,
            cursor,
        } => cursor,
        ConversationReplayDelivery::Current { .. }
        | ConversationReplayDelivery::ResnapshotRequired { .. } => {
            panic!("first reuse replay should publish")
        }
    };
    assert_eq!(first_cursor, ConversationCursor::new(2));
    let mut incoming = tokio::time::timeout(TEST_DEADLINE, client_connection.accept_uni())
        .await
        .expect("first stream should be visible")
        .expect("first stream should be accepted");
    assert_patch_batch_frame(
        receive_envelope(&mut incoming).await,
        &first_batch,
        "frame-reuse-first",
        2_101,
    );

    commit_assistant_start(&repository, &run, "reuse").await;
    let second_subscription = activate(
        &repository,
        &registrar,
        "request-reuse-second",
        ConversationSubscribe::resume(thread_id.clone(), first_cursor),
    )
    .await;
    let (second_replay, second_batch) = read_batch(&repository, second_subscription).await;
    assert_eq!(second_batch.from_cursor(), first_cursor);
    assert_eq!(second_batch.to_cursor(), ConversationCursor::new(4));
    assert_no_uni(&client_connection).await;

    let (writer, second_outcome) = tokio::time::timeout(
        TEST_DEADLINE,
        writer.deliver(stamp("frame-reuse-second", 2_102), second_replay),
    )
    .await
    .expect("second reuse delivery should finish")
    .expect("second reuse delivery should succeed");
    match second_outcome {
        ConversationReplayDelivery::Published { cursor, .. } => {
            assert_eq!(cursor, ConversationCursor::new(4));
        }
        ConversationReplayDelivery::Current { .. }
        | ConversationReplayDelivery::ResnapshotRequired { .. } => {
            panic!("second reuse replay should publish")
        }
    }
    assert_patch_batch_frame(
        receive_envelope(&mut incoming).await,
        &second_batch,
        "frame-reuse-second",
        2_102,
    );
    assert_no_uni(&client_connection).await;
    let view = registrar
        .subscription_view(&thread_id)
        .await
        .expect("second subscription should remain registered");
    assert_eq!(view.cursor(), ConversationCursor::new(4));
    writer
        .finish()
        .expect("reused stream should finish cleanly");
    assert_clean_eof(&mut incoming).await;
    drop(incoming);
    drop(client_connection);
    loopback.drain().await;
}

// ---------------------------------------------------------------------------
// Typed send failure and post-send registry fences
// ---------------------------------------------------------------------------

#[tokio::test]
async fn peer_stop_preserves_typed_send_failure_and_cursor() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "send-failure").await;
    let registrar = ConversationSubscriptionRegistrar::new();
    let subscription = activate(
        &repository,
        &registrar,
        "request-send-failure",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    let _run = seed_run(&repository, &thread_id, "send-failure", &"x".repeat(60_000)).await;
    let (replay, _expected_batch) = read_batch(&repository, subscription).await;

    let mut loopback = spawn_loopback(true);
    let (server_connection, client_connection) = connected(&mut loopback).await;
    let _server_keepalive = server_connection.clone();
    let writer =
        ConversationDeliveryWriter::new(server_connection, registrar.clone(), ProtocolVersion::V1);
    let mut delivery = Box::pin(writer.deliver(stamp("frame-send-failure", 3_001), replay));
    let mut accept = Box::pin(client_connection.accept_uni());
    let mut incoming = tokio::time::timeout(TEST_DEADLINE, async {
        tokio::select! {
            incoming = &mut accept => incoming.expect("uni stream should open"),
            _ = &mut delivery => panic!("a constrained first frame should still be in flight"),
        }
    })
    .await
    .expect("stream opening should finish under the deadline");
    incoming
        .stop(PEER_STOP_CODE)
        .expect("peer stop should be accepted");
    let result = tokio::time::timeout(TEST_DEADLINE, &mut delivery)
        .await
        .expect("peer stop should settle the send");
    let error = result.expect_err("peer stop must consume the writer with an error");
    match error {
        ConversationDeliveryError::Send(EnvelopeSendError::Frame(FrameError::Write(
            WriteError::Stopped(code),
        ))) => assert_eq!(code, PEER_STOP_CODE),
        other => panic!("expected the exact stopped send source, got {other:?}"),
    }
    assert_eq!(
        registrar
            .subscription_view(&thread_id)
            .await
            .expect("subscription should remain registered")
            .cursor(),
        ConversationCursor::default()
    );
    drop(accept);
    drop(delivery);
    drop(incoming);
    drop(client_connection);
    loopback.drain().await;
}

async fn assert_registry_failure_after_wire_send(
    repository: &Repository,
    registrar: &ConversationSubscriptionRegistrar,
    thread_id: &ThreadId,
    replay: ActivatedConversationReplay,
    expected_cursor: ConversationCursor,
    replace: bool,
) {
    if replace {
        let replacement = activate(
            repository,
            registrar,
            "request-replaced",
            ConversationSubscribe::resume(thread_id.clone(), expected_cursor),
        )
        .await;
        assert_eq!(replacement.cursor(), expected_cursor);
    } else {
        unsubscribe(repository, registrar, thread_id, "request-unsubscribed").await;
    }

    let mut loopback = spawn_loopback(false);
    let (server_connection, client_connection) = connected(&mut loopback).await;
    let _server_keepalive = server_connection.clone();
    let writer =
        ConversationDeliveryWriter::new(server_connection, registrar.clone(), ProtocolVersion::V1);
    let result = tokio::time::timeout(
        TEST_DEADLINE,
        writer.deliver(stamp("frame-registry-fence", 3_101), replay),
    )
    .await
    .expect("registry-fenced delivery should finish");
    let error = result.expect_err("post-send registry failure must be terminal");
    match error {
        ConversationDeliveryError::Registry(ApplyBatchError::StaleLease) => {}
        other => panic!("expected the exact stale-lease source, got {other:?}"),
    }

    let mut incoming = tokio::time::timeout(TEST_DEADLINE, client_connection.accept_uni())
        .await
        .expect("the successful send should expose one uni stream")
        .expect("the uni stream should be accepted");
    let reset = tokio::time::timeout(TEST_DEADLINE, incoming.read_to_end(4 * 1024 * 1024))
        .await
        .expect("terminal registry failure should reset the stream");
    match reset {
        Err(quinn::ReadToEndError::Read(ReadError::Reset(code))) => {
            assert_eq!(code, STREAM_RESET_CODE);
        }
        other => panic!("expected the writer's fixed reset code, got {other:?}"),
    }
    let view = registrar.subscription_view(thread_id).await;
    if replace {
        assert_eq!(
            view.expect("replacement should remain registered").cursor(),
            expected_cursor
        );
    } else {
        assert!(view.is_none(), "unsubscription should remove the entry");
    }
    drop(incoming);
    drop(client_connection);
    loopback.drain().await;
}

#[tokio::test]
async fn unsubscribed_lease_after_wire_send_preserves_apply_batch_error() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "unsubscribed").await;
    let registrar = ConversationSubscriptionRegistrar::new();
    let subscription = activate(
        &repository,
        &registrar,
        "request-unsubscribed-source",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    let expected_cursor = subscription.cursor();
    let _run = seed_run(&repository, &thread_id, "unsubscribed", "first body").await;
    let (replay, _expected_batch) = read_batch(&repository, subscription).await;
    assert_registry_failure_after_wire_send(
        &repository,
        &registrar,
        &thread_id,
        replay,
        expected_cursor,
        false,
    )
    .await;
}

#[tokio::test]
async fn replaced_lease_after_wire_send_preserves_apply_batch_error() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "replaced").await;
    let registrar = ConversationSubscriptionRegistrar::new();
    let subscription = activate(
        &repository,
        &registrar,
        "request-replaced-source",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    let expected_cursor = subscription.cursor();
    let _run = seed_run(&repository, &thread_id, "replaced", "first body").await;
    let (replay, _expected_batch) = read_batch(&repository, subscription).await;
    assert_registry_failure_after_wire_send(
        &repository,
        &registrar,
        &thread_id,
        replay,
        expected_cursor,
        true,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Cancellation-safe reset and explicit finish
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancelling_an_in_flight_first_write_resets_the_installed_direction() {
    let (_database, repository) = memory_repository().await;
    let thread_id = seed_thread(&repository, "cancel").await;
    let registrar = ConversationSubscriptionRegistrar::new();
    let subscription = activate(
        &repository,
        &registrar,
        "request-cancel",
        ConversationSubscribe::fresh(thread_id.clone()),
    )
    .await;
    let _run = seed_run(&repository, &thread_id, "cancel", &"x".repeat(60_000)).await;
    let (replay, _expected_batch) = read_batch(&repository, subscription).await;

    let mut loopback = spawn_loopback(true);
    let (server_connection, client_connection) = connected(&mut loopback).await;
    let _server_keepalive = server_connection.clone();
    let writer =
        ConversationDeliveryWriter::new(server_connection, registrar.clone(), ProtocolVersion::V1);
    let mut delivery = Box::pin(writer.deliver(stamp("frame-cancel", 4_001), replay));
    let mut accept = Box::pin(client_connection.accept_uni());
    let mut incoming = tokio::time::timeout(TEST_DEADLINE, async {
        tokio::select! {
            incoming = &mut accept => incoming.expect("uni stream should open"),
            _ = &mut delivery => panic!("the constrained first write should remain in flight"),
        }
    })
    .await
    .expect("stream opening should finish under the deadline");
    drop(delivery);
    drop(accept);

    let result = tokio::time::timeout(TEST_DEADLINE, incoming.read_to_end(4 * 1024 * 1024))
        .await
        .expect("stream reset should be observed under the deadline");
    match result {
        Err(quinn::ReadToEndError::Read(ReadError::Reset(code))) => {
            assert_eq!(code, STREAM_RESET_CODE);
        }
        other => panic!("expected the writer's fixed reset code, got {other:?}"),
    }
    assert_eq!(
        registrar
            .subscription_view(&thread_id)
            .await
            .expect("cancelled subscription should remain registered")
            .cursor(),
        ConversationCursor::default()
    );
    drop(incoming);
    drop(client_connection);
    loopback.drain().await;
}
