//! Focused evidence for the connection-owned conversation delivery driver.
//!
//! The state tests cover lease fencing and coalesced wake behavior. The
//! loopback test drives the public listener, real repository replay, QUIC
//! request/response streams, and the server-initiated client delivery stream
//! together.

use std::error::Error;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use artisan_backend::conversation_commit_notifier::ConversationCommitNotifier;
use artisan_backend::conversation_subscription_registry::{
    ActivateError, ConversationSubscriptionRegistry, SubscriptionState, UnsubscribeOutcome,
};
use artisan_backend::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, ForgeApp, ForgeConfig,
    ForgeListener, ForgeStartupError, ListenerLimits, RequestHandler, RequestTermination,
};
use artisan_database::{
    AssistantChange, AttachProjectInput, BindRunProvider, BindRunProviderOutcome, CheckpointUpdate,
    ClaimMessageDispatch, CommitRunBatch, CompleteRun, ConversationPatchReplay, CreateThreadInput,
    DispatchLeaseOwner, LaunchClaimedRun, LaunchClaimedRunOutcome, ProviderBindingBytes,
    QueueFirstMessageInput, Repository, RunBatchScope, RunLaunchCredentials, RunStartKey,
    SetThreadEngineConfigInput, SqliteConfig,
};
use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantMessagePhase, ByteLimit, ConversationCursor,
    ConversationRequest, ConversationSubscribe, ConversationUnsubscribe, CountLimit, DisplayName,
    EngineAgentId, EngineConfigUpdatePrecondition, EngineId, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, Event, FilesystemAccess, FiniteMillis, ItemId,
    MessageBody, MessageId, NetworkAccess, Observation, ObservationId, ObservationSequence,
    OpenCode2Selection, PatchId, PermissionId, ProjectId, RequestId, Revision, RootPath, ThreadId,
    ThreadTitle, ToolAction, ToolObservation, UnixMillis, WebSearchAccess,
};
use artisan_protocol::{
    APPLICATION_PROTOCOL_VERSION, ClientRequest, ConversationSubscriptionStarted, FrameId, Hello,
    HelloCredential, LocalCapability, ProtocolVersion, ResponsePayload, VersionOffer, WireEnvelope,
    WireEnvelopeBody,
};
use artisan_transport::{CancelHandle, DeadlineError, OperationKind, PinnedIdentity};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};

const TEST_DEADLINE: Duration = Duration::from_secs(5);
const INITIAL_CAPABILITY: [u8; 32] = [0x4d; 32];
const CLIENT_HELLO_FRAME: &str = "delivery-client-hello";

#[derive(Debug)]
struct TestOrigin {
    next: AtomicU64,
}

impl TestOrigin {
    fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
        }
    }
}

impl CommandOrigin for TestOrigin {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        Ok(format!(
            "delivery-origin-{}",
            self.next.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        Ok(UnixMillis::from_millis(1_000))
    }
}

#[test]
fn replacement_fences_old_lease_and_unsubscribe_drops_the_entry() {
    let thread_id = ThreadId::parse("delivery-thread").expect("valid thread id");
    let mut registry = ConversationSubscriptionRegistry::new();
    let old = registry
        .register_pending(thread_id.clone(), ConversationCursor::new(3))
        .expect("first lease");
    assert_eq!(registry.activate(&old), Ok(ConversationCursor::new(3)));

    let replacement = registry
        .register_pending(thread_id.clone(), ConversationCursor::new(7))
        .expect("replacement lease");
    assert_eq!(registry.activate(&old), Err(ActivateError::StaleLease));
    let view = registry.view(&thread_id).expect("replacement remains");
    assert_eq!(view.state(), SubscriptionState::Pending);
    assert_eq!(view.cursor(), ConversationCursor::new(7));

    let removed = registry.unsubscribe(&thread_id);
    assert!(matches!(
        removed,
        UnsubscribeOutcome::Removed(ref entry) if entry.lease() == &replacement
    ));
    assert!(registry.is_empty());
}

#[test]
fn observation_cursor_starts_separately_from_the_patch_cursor() {
    use artisan_backend::conversation_subscription_registry::ConversationSubscriptionRegistry;

    let thread_id = ThreadId::parse("delivery-observation-cursor").expect("valid thread id");
    let mut registry = ConversationSubscriptionRegistry::new();
    let lease = registry
        .register_pending(thread_id.clone(), ConversationCursor::new(9))
        .expect("pending registration");
    // The patch cursor declares replay position; the observation cursor always
    // restarts at the thread-scoped origin so reconnect replay redelivers the
    // durable history including settled turns.
    let view = registry.view(&thread_id).expect("pending view");
    assert_eq!(view.cursor(), ConversationCursor::new(9));
    assert_eq!(view.observation_cursor(), 0);
    registry.activate(&lease).expect("activation");
    let view = registry.view(&thread_id).expect("active view");
    assert_eq!(view.cursor(), ConversationCursor::new(9));
    assert_eq!(view.observation_cursor(), 0);
}

#[tokio::test]
async fn notifier_coalesces_repeated_commit_wakes_without_payload() {
    let notifier = ConversationCommitNotifier::new();
    let thread_id = ThreadId::parse("delivery-wake-thread").expect("valid thread id");
    let mut subscription = notifier
        .subscribe(thread_id.clone())
        .expect("wake subscription");

    assert_eq!(
        notifier.publish(&thread_id),
        artisan_backend::conversation_commit_notifier::ConversationCommitPublish::Notified
    );
    subscription.wait().await.expect("first wake");

    assert_eq!(
        notifier.publish(&thread_id),
        artisan_backend::conversation_commit_notifier::ConversationCommitPublish::Notified
    );
    assert_eq!(
        notifier.publish(&thread_id),
        artisan_backend::conversation_commit_notifier::ConversationCommitPublish::Notified
    );
    subscription.wait().await.expect("coalesced wake");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), subscription.wait())
            .await
            .is_err()
    );
}

struct TestPki {
    certificate: CertificateDer<'static>,
    private_key: PrivatePkcs8KeyDer<'static>,
    pinned_identity: PinnedIdentity,
}

fn test_pki() -> TestPki {
    let certified_key =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("valid SAN");
    let certificate = certified_key.cert.der().clone();
    TestPki {
        pinned_identity: PinnedIdentity::from_certificate(&certificate),
        private_key: PrivatePkcs8KeyDer::from(certified_key.signing_key.serialize_der()),
        certificate,
    }
}

fn server_config(pki: &TestPki) -> ServerConfig {
    artisan_transport::server_config(vec![pki.certificate.clone()], pki.private_key.clone_key())
        .expect("server configuration")
}

fn client_config(pki: &TestPki) -> ClientConfig {
    artisan_transport::client_config(pki.certificate.clone(), pki.pinned_identity)
        .expect("client configuration")
}

fn listener_limits() -> ListenerLimits {
    ListenerLimits {
        admission: Duration::from_secs(2),
        handshake: Duration::from_secs(2),
        next_request: Duration::from_secs(2),
        drain: Duration::from_secs(2),
    }
}

static TEMPORARY_DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TemporaryDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TemporaryDatabase {
    fn new() -> Self {
        let sequence = TEMPORARY_DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "artisan-forge-delivery-driver-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("temporary database directory");
        Self {
            path: directory.join("forge.sqlite3"),
            directory,
        }
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.directory);
    }
}

async fn opened_app() -> Result<(TemporaryDatabase, ForgeApp), ForgeStartupError> {
    let temporary = TemporaryDatabase::new();
    let app = ForgeApp::start(ForgeConfig::new(
        SqliteConfig::file(&temporary.path).sqlx_logging(false),
    ))
    .await?;
    Ok((temporary, app))
}

fn fixture_engine_config() -> EngineRunConfig {
    let one = FiniteMillis::new(1).expect("one millisecond");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget"),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness line limit"),
        max_header_count: CountLimit::new(8).expect("header count"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer limit"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr limit"),
        observation_capacity: CountLimit::new(16).expect("observation capacity"),
    })
    .expect("runtime relationships");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("delivery-permission").expect("permission id"),
        EngineAgentId::parse("delivery-agent").expect("agent id"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("delivery-profile").expect("profile id"),
            EngineModelId::parse("delivery-model").expect("model id"),
            EngineRouteId::parse("delivery-route").expect("route id"),
            None,
            permission,
        )),
        runtime,
    )
}

struct SeededRun {
    claimed: artisan_database::ClaimedMessageDispatch,
    launched: artisan_database::LaunchedRunReceipt,
    bound: artisan_database::BoundRunReceipt,
    start_key: RunStartKey,
    credentials: RunLaunchCredentials,
}

struct SeededThread {
    thread_id: ThreadId,
    run: SeededRun,
}

async fn seed_project_thread_and_message(
    repository: &Repository,
) -> Result<ThreadId, Box<dyn Error>> {
    repository
        .attach_project(AttachProjectInput {
            request_id: RequestId::parse("delivery-project-request")?,
            directory_id: artisan_domain::DirectoryId::parse("delivery-directory")?,
            project_id: ProjectId::parse("delivery-project")?,
            root_path: RootPath::parse("C:/repos/delivery")?,
            display_name: DisplayName::parse("Delivery")?,
            attached_at: UnixMillis::from_millis(100),
        })
        .await?;
    let thread_id = ThreadId::parse("delivery-thread")?;
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse("delivery-thread-request")?,
            thread_id: thread_id.clone(),
            project_id: ProjectId::parse("delivery-project")?,
            title: ThreadTitle::parse("New thread")?,
            created_at: UnixMillis::from_millis(200),
            updated_at: UnixMillis::from_millis(200),
        })
        .await?;
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("delivery-engine-request")?,
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: fixture_engine_config(),
            accepted_at: UnixMillis::from_millis(250),
        })
        .await?;
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("delivery-message-request")?,
            message_id: MessageId::parse("delivery-message")?,
            thread_id: thread_id.clone(),
            body: MessageBody::parse("delivery body")?,
            accepted_at: UnixMillis::from_millis(300),
        })
        .await?;
    Ok(thread_id)
}

async fn seed_thread(repository: &Repository) -> Result<SeededThread, Box<dyn Error>> {
    let thread_id = seed_project_thread_and_message(repository).await?;

    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x11; 32]),
            claimed_at: UnixMillis::from_millis(400),
            lease_expires_at: UnixMillis::from_millis(900),
        })
        .await?
        .ok_or("delivery dispatch should be claimable")?;
    let run_id = artisan_domain::RunId::parse("delivery-run")?;
    let turn_id = artisan_domain::TurnId::parse("delivery-turn")?;
    let item_id = ItemId::parse("delivery-item")?;
    let first_patch_id = PatchId::parse("delivery-patch-first")?;
    let second_patch_id = PatchId::parse("delivery-patch-second")?;
    let run_start_key = RunStartKey::new([0x44; 32]);
    let credentials = RunLaunchCredentials::new([0xa1; 32], [0xb2; 32], [0xc3; 32]);
    let settings = repository
        .read_thread_engine_settings(&thread_id)
        .await?
        .ok_or("delivery settings should exist")?;
    let launched = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &item_id,
            first_patch_id: &first_patch_id,
            second_patch_id: &second_patch_id,
            operated_at: UnixMillis::from_millis(500),
            run_start_key: &run_start_key,
            credentials: &credentials,
            engine_settings: &settings,
        })
        .await?;
    let launched = match launched {
        LaunchClaimedRunOutcome::Started(receipt)
        | LaunchClaimedRunOutcome::AlreadyStarted(receipt) => receipt,
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16])?;
    let bound = match repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &run_start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(500),
            bound_at: UnixMillis::from_millis(600),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await?
    {
        BindRunProviderOutcome::Bound(receipt) | BindRunProviderOutcome::AlreadyBound(receipt) => {
            receipt
        }
    };
    Ok(SeededThread {
        thread_id,
        run: SeededRun {
            claimed,
            launched,
            bound,
            start_key: run_start_key,
            credentials,
        },
    })
}

async fn commit_assistant_start(
    repository: &Repository,
    run: &SeededRun,
) -> Result<(), Box<dyn Error>> {
    let item_id = ItemId::parse("delivery-assistant-item")?;
    let activation_patch = PatchId::parse("delivery-assistant-activation")?;
    let item_patch = PatchId::parse("delivery-assistant-patch")?;
    let body = AssistantBody::parse("delivery assistant output")?;
    let outcome = repository
        .commit_run_batch(artisan_database::CommitRunBatch {
            scope: artisan_database::RunBatchScope {
                claimed: &run.claimed,
                launched: &run.launched,
                bound: &run.bound,
                run_start_key: &run.start_key,
                credentials: &run.credentials,
                expected_launch_at: UnixMillis::from_millis(500),
                expected_updated_at: UnixMillis::from_millis(600),
            },
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(700),
            activate_turn_patch_id: Some(&activation_patch),
            changes: &[artisan_database::AssistantChange::Start {
                item_id: &item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: &item_patch,
            }],
            checkpoint: artisan_database::CheckpointUpdate::Keep,
        })
        .await?;
    if !matches!(
        outcome,
        artisan_database::CommitRunBatchOutcome::Committed(_)
    ) {
        return Err("assistant batch should be newly committed".into());
    }
    Ok(())
}

#[tokio::test]
async fn fresh_activation_retains_the_snapshot_cursor() -> Result<(), Box<dyn Error>> {
    let (_temporary, app) = opened_app().await?;
    let seeded = seed_thread(app.repository()).await?;
    let thread_id = seeded.thread_id.clone();
    let handler = RequestHandler::with_subscriptions(app.repository().clone());
    let request = ClientRequest::Conversation(ConversationRequest::Subscribe(
        ConversationSubscribe::fresh(thread_id.clone()),
    ));
    let request_id = RequestId::parse("delivery-fresh-request")?;
    let (answered, receipt) = handler
        .respond_with_receipt(&request_id, &request)
        .await
        .into_parts();
    let response = answered.expect("fresh subscription response");
    let ResponsePayload::ConversationSubscriptionStarted(ConversationSubscriptionStarted::Fresh(
        started,
    )) = response.payload
    else {
        return Err("expected a fresh subscription response".into());
    };
    let snapshot_cursor = started.snapshot().cursor();
    let activated = handler
        .activate_after_response(receipt)
        .await?
        .expect("fresh receipt activates");
    assert_eq!(activated.cursor(), snapshot_cursor);
    assert_eq!(
        handler
            .subscription_view(&thread_id)
            .await
            .expect("active subscription view")
            .state(),
        SubscriptionState::Active
    );

    drop(handler);
    app.shutdown().await?;
    Ok(())
}

fn hello_envelope() -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(CLIENT_HELLO_FRAME).expect("hello frame id"),
        sent_at: UnixMillis::from_millis(10),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![APPLICATION_PROTOCOL_VERSION])
                .expect("version offer"),
            credential: HelloCredential::Initial(LocalCapability::from_bytes(INITIAL_CAPABILITY)),
            supports_lifecycle_control: false,
        }),
    }
}

fn resume_request(thread_id: ThreadId) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("delivery-subscribe").expect("request frame id"),
        sent_at: UnixMillis::from_millis(20),
        body: WireEnvelopeBody::Request(ClientRequest::Conversation(
            ConversationRequest::Subscribe(ConversationSubscribe::resume(
                thread_id,
                ConversationCursor::default(),
            )),
        )),
    }
}

fn unsubscribe_request(thread_id: ThreadId) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("delivery-unsubscribe").expect("unsubscribe frame id"),
        sent_at: UnixMillis::from_millis(30),
        body: WireEnvelopeBody::Request(ClientRequest::Conversation(
            ConversationRequest::Unsubscribe(ConversationUnsubscribe { thread_id }),
        )),
    }
}

async fn connect_client(
    endpoint: &Endpoint,
    address: SocketAddr,
) -> Result<Connection, Box<dyn Error>> {
    let connecting = endpoint.connect(address, artisan_transport::LOOPBACK_SERVER_NAME)?;
    Ok(tokio::time::timeout(TEST_DEADLINE, connecting).await??)
}

async fn subscribed_client(
    endpoint: &Endpoint,
    address: SocketAddr,
    thread_id: ThreadId,
) -> Result<(Connection, quinn::RecvStream), Box<dyn Error>> {
    let connection = connect_client(endpoint, address).await?;
    let (mut control_send, mut control_recv) = connection.open_bi().await?;
    let _welcome =
        artisan_transport::client_handshake(&mut control_send, &mut control_recv, hello_envelope())
            .await?;
    let (mut request_send, mut request_recv) = connection.open_bi().await?;
    artisan_transport::send_envelope(&mut request_send, &resume_request(thread_id.clone())).await?;
    drop(request_send);
    let _response = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut request_recv),
    )
    .await??;
    let mut delivery_stream =
        tokio::time::timeout(TEST_DEADLINE, connection.accept_uni()).await??;
    let delivery = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut delivery_stream),
    )
    .await??;
    if !matches!(delivery.body, WireEnvelopeBody::PatchBatch(_)) {
        return Err("expected the initial delivery batch".into());
    }
    receive_activation_outbox(&mut delivery_stream, &thread_id).await?;
    drop(control_send);
    drop(control_recv);
    drop(request_recv);
    Ok((connection, delivery_stream))
}

async fn serve_resumed_delivery(
    listener: ForgeListener,
    handler: &RequestHandler,
    cancel: &CancelHandle,
) -> Result<(), Box<dyn Error>> {
    let (listener, report) = listener.serve_one(handler, cancel).await?;
    assert_eq!(report.completed_requests, 2);
    assert!(matches!(
        report.termination,
        RequestTermination::Failed {
            source: DeadlineError::Cancelled {
                operation: OperationKind::Receive
            }
        }
    ));
    listener.drain().await?;
    Ok(())
}

async fn resumed_delivery_client(
    endpoint: &Endpoint,
    address: SocketAddr,
    seeded: &SeededThread,
    first_cursor: ConversationCursor,
    repository: &Repository,
    notifier: &ConversationCommitNotifier,
    cancel: &CancelHandle,
) -> Result<(WireEnvelope, WireEnvelope, WireEnvelope, WireEnvelope), Box<dyn Error>> {
    let thread_id = &seeded.thread_id;
    let connection = connect_client(endpoint, address).await?;
    let (mut control_send, mut control_recv) = connection.open_bi().await?;
    let _welcome =
        artisan_transport::client_handshake(&mut control_send, &mut control_recv, hello_envelope())
            .await?;
    let (mut request_send, mut request_recv) = connection.open_bi().await?;
    artisan_transport::send_envelope(&mut request_send, &resume_request(thread_id.clone())).await?;
    drop(request_send);
    let response = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut request_recv),
    )
    .await??;
    let mut delivery_stream =
        tokio::time::timeout(TEST_DEADLINE, connection.accept_uni()).await??;
    let delivery = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut delivery_stream),
    )
    .await??;
    receive_activation_outbox(&mut delivery_stream, thread_id).await?;
    commit_assistant_start(repository, &seeded.run).await?;
    let expected_second = match repository
        .read_conversation_patch_replay(thread_id, first_cursor)
        .await?
    {
        ConversationPatchReplay::Batch(batch) => batch,
        other => return Err(format!("expected a contiguous wake batch, got {other:?}").into()),
    };
    assert_eq!(expected_second.from_cursor(), first_cursor);
    let _ = notifier.publish(thread_id);
    let second_delivery = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut delivery_stream),
    )
    .await??;
    match &second_delivery.body {
        WireEnvelopeBody::PatchBatch(actual) => assert_eq!(actual, &expected_second),
        _ => return Err("expected the wake patch batch on the same delivery stream".into()),
    }
    let _ = notifier.publish(thread_id);
    let _ = notifier.publish(thread_id);
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            artisan_transport::receive_envelope(&mut delivery_stream),
        )
        .await
        .is_err()
    );

    let (mut unsubscribe_send, mut unsubscribe_recv) = connection.open_bi().await?;
    artisan_transport::send_envelope(
        &mut unsubscribe_send,
        &unsubscribe_request(thread_id.clone()),
    )
    .await?;
    drop(unsubscribe_send);
    let stopped = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut unsubscribe_recv),
    )
    .await??;
    cancel.cancel();
    let eof = tokio::time::timeout(
        TEST_DEADLINE,
        artisan_transport::receive_envelope(&mut delivery_stream),
    )
    .await?;
    assert!(
        eof.is_err(),
        "finished delivery stream should contain no frame"
    );
    drop(control_send);
    drop(control_recv);
    drop(request_recv);
    drop(unsubscribe_recv);
    drop(connection);
    Ok((response, delivery, stopped, second_delivery))
}

#[tokio::test]
async fn resumed_activation_sends_exact_replay_on_real_forge_delivery_stream()
-> Result<(), Box<dyn Error>> {
    let (_temporary, app) = opened_app().await?;
    let seeded = seed_thread(app.repository()).await?;
    let thread_id = seeded.thread_id.clone();
    let expected = match app
        .repository()
        .read_conversation_patch_replay(&thread_id, ConversationCursor::default())
        .await?
    {
        ConversationPatchReplay::Batch(batch) => batch,
        other => return Err(format!("expected a replay batch, got {other:?}").into()),
    };
    let first_cursor = expected.to_cursor();

    let pki = test_pki();
    let endpoint = artisan_transport::bind_loopback_client(client_config(&pki))?;
    let notifier = ConversationCommitNotifier::new();
    let repository = app.repository().clone();
    let handler = RequestHandler::new(app.repository().clone())
        .with_conversation_commit_notifier(notifier.clone());
    let listener = ForgeListener::bind(
        server_config(&pki),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        Box::new(TestOrigin::new()),
        listener_limits(),
        std::num::NonZeroU32::new(1).expect("admission capacity"),
        std::num::NonZeroU32::new(3).expect("request capacity"),
    )?;
    let address = listener.local_addr()?;
    let cancel = CancelHandle::new();

    let server = serve_resumed_delivery(listener, &handler, &cancel);
    let client = resumed_delivery_client(
        &endpoint,
        address,
        &seeded,
        first_cursor,
        &repository,
        &notifier,
        &cancel,
    );

    let (server_result, client_result) = tokio::join!(server, client);
    server_result?;
    let (response, delivery, stopped, second_delivery) = client_result?;
    let WireEnvelopeBody::Response(response) = response.body else {
        return Err("expected the correlated subscription response".into());
    };
    let ResponsePayload::ConversationSubscriptionStarted(
        ConversationSubscriptionStarted::Resumed {
            thread_id: response_thread,
            cursor,
        },
    ) = response.payload
    else {
        return Err("expected a resumed subscription acknowledgement".into());
    };
    assert_eq!(response_thread, thread_id);
    assert_eq!(cursor, ConversationCursor::default());

    let WireEnvelopeBody::PatchBatch(actual) = delivery.body else {
        return Err("expected one patch batch on the delivery stream".into());
    };
    assert_eq!(actual, expected);
    assert!(matches!(
        second_delivery.body,
        WireEnvelopeBody::PatchBatch(_)
    ));

    let WireEnvelopeBody::Response(stopped) = stopped.body else {
        return Err("expected the unsubscribe response".into());
    };
    let ResponsePayload::ConversationSubscriptionStopped(stopped) = stopped.payload else {
        return Err("expected the unsubscribe acknowledgement".into());
    };
    assert_eq!(stopped.thread_id, thread_id);

    artisan_transport::shutdown(
        &endpoint,
        quinn::VarInt::from_u32(0),
        b"delivery driver test complete",
        TEST_DEADLINE,
    )
    .await?;
    drop(endpoint);
    drop(handler);
    drop(repository);
    app.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn cancellation_cleans_connection_owned_delivery() -> Result<(), Box<dyn Error>> {
    let (_temporary, app) = opened_app().await?;
    let seeded = seed_thread(app.repository()).await?;
    let thread_id = seeded.thread_id.clone();
    let pki = test_pki();
    let endpoint = artisan_transport::bind_loopback_client(client_config(&pki))?;
    let notifier = ConversationCommitNotifier::new();
    let handler =
        RequestHandler::new(app.repository().clone()).with_conversation_commit_notifier(notifier);
    let listener = ForgeListener::bind(
        server_config(&pki),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        Box::new(TestOrigin::new()),
        listener_limits(),
        std::num::NonZeroU32::new(1).expect("admission capacity"),
        std::num::NonZeroU32::new(4).expect("request capacity"),
    )?;
    let address = listener.local_addr()?;
    let cancel = CancelHandle::new();

    let server = async {
        let (listener, report) = listener.serve_one(&handler, &cancel).await?;
        assert_eq!(report.completed_requests, 1);
        assert!(matches!(
            report.termination,
            RequestTermination::Failed {
                source: DeadlineError::Cancelled {
                    operation: OperationKind::Receive
                }
            }
        ));
        listener.drain().await?;
        Ok::<(), Box<dyn Error>>(())
    };
    let client = async {
        let (connection, mut delivery_stream) =
            subscribed_client(&endpoint, address, thread_id).await?;
        cancel.cancel();
        let eof = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::receive_envelope(&mut delivery_stream),
        )
        .await?;
        assert!(eof.is_err(), "cancellation must not publish another frame");
        drop(delivery_stream);
        drop(connection);
        Ok::<(), Box<dyn Error>>(())
    };

    let (server_result, client_result) = tokio::join!(server, client);
    server_result?;
    client_result?;
    artisan_transport::shutdown(
        &endpoint,
        quinn::VarInt::from_u32(0),
        b"delivery cancellation test complete",
        TEST_DEADLINE,
    )
    .await?;
    drop(endpoint);
    drop(handler);
    app.shutdown().await?;
    Ok(())
}

/// Accepting a message changes the thread's outbox: the wake pushes the
/// complete outbox with the new row in its Forge-owned state, and a wake
/// that changes nothing pushes nothing.
#[tokio::test]
async fn accepted_message_pushes_the_thread_outbox() -> Result<(), Box<dyn Error>> {
    let (_temporary, app) = opened_app().await?;
    let repository = app.repository().clone();
    let seeded = seed_thread(&repository).await?;
    let thread_id = seeded.thread_id.clone();
    let pki = test_pki();
    let endpoint = artisan_transport::bind_loopback_client(client_config(&pki))?;
    let notifier = ConversationCommitNotifier::new();
    let handler =
        RequestHandler::new(repository.clone()).with_conversation_commit_notifier(notifier.clone());
    let listener = ForgeListener::bind(
        server_config(&pki),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        Box::new(TestOrigin::new()),
        listener_limits(),
        std::num::NonZeroU32::new(1).expect("admission capacity"),
        std::num::NonZeroU32::new(4).expect("request capacity"),
    )?;
    let address = listener.local_addr()?;
    let cancel = CancelHandle::new();

    let server = async {
        let (listener, report) = listener.serve_one(&handler, &cancel).await?;
        assert_eq!(report.completed_requests, 1);
        listener.drain().await?;
        Ok::<(), Box<dyn Error>>(())
    };
    let client = async {
        let (connection, mut delivery_stream) =
            subscribed_client(&endpoint, address, thread_id.clone()).await?;
        repository
            .queue_message(artisan_database::QueueMessageInput {
                request_id: RequestId::parse("delivery-live-queue")?,
                message_id: MessageId::parse("delivery-live-message")?,
                thread_id: thread_id.clone(),
                payload: artisan_domain::QueueMessagePayload::text_only("while running")?,
                steer_run_id: None,
                accepted_at: UnixMillis::from_millis(700),
            })
            .await?;
        let _ = notifier.publish(&thread_id);
        let frame = receive_delivery_frame(&mut delivery_stream).await?;
        let WireEnvelopeBody::Event(event) = frame.body else {
            return Err("expected the outbox event".into());
        };
        let Event::MessageOutbox(outbox) = event.event else {
            return Err("expected a message outbox".into());
        };
        let [row] = outbox.queued().messages() else {
            return Err("expected exactly the accepted message".into());
        };
        assert_eq!(row.message_id.as_str(), "delivery-live-message");
        assert_eq!(row.state, artisan_domain::QueuedMessageState::Queued);
        assert_eq!(row.engine, Some(EngineId::OpenCode2));
        assert!(outbox.failed().messages().is_empty());
        let _ = notifier.publish(&thread_id);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                artisan_transport::receive_envelope(&mut delivery_stream),
            )
            .await
            .is_err(),
            "an unchanged outbox is not pushed again"
        );
        cancel.cancel();
        drop(delivery_stream);
        drop(connection);
        Ok::<(), Box<dyn Error>>(())
    };

    let (server_result, client_result) = tokio::join!(server, client);
    server_result?;
    client_result?;
    artisan_transport::shutdown(
        &endpoint,
        quinn::VarInt::from_u32(0),
        b"delivery outbox test complete",
        TEST_DEADLINE,
    )
    .await?;
    drop(endpoint);
    drop(handler);
    app.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn peer_loss_releases_connection_owned_delivery() -> Result<(), Box<dyn Error>> {
    let (_temporary, app) = opened_app().await?;
    let seeded = seed_thread(app.repository()).await?;
    let thread_id = seeded.thread_id.clone();
    let pki = test_pki();
    let endpoint = artisan_transport::bind_loopback_client(client_config(&pki))?;
    let notifier = ConversationCommitNotifier::new();
    let handler =
        RequestHandler::new(app.repository().clone()).with_conversation_commit_notifier(notifier);
    let listener = ForgeListener::bind(
        server_config(&pki),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        Box::new(TestOrigin::new()),
        listener_limits(),
        std::num::NonZeroU32::new(1).expect("admission capacity"),
        std::num::NonZeroU32::new(4).expect("request capacity"),
    )?;
    let address = listener.local_addr()?;
    let cancel = CancelHandle::new();

    let server = async {
        let (listener, report) = listener.serve_one(&handler, &cancel).await?;
        assert_eq!(report.completed_requests, 1);
        assert!(matches!(
            report.termination,
            RequestTermination::Failed {
                source: DeadlineError::Peer {
                    operation: OperationKind::Receive,
                    error: artisan_backend::RequestStageError::Accept { .. },
                }
            }
        ));
        listener.drain().await?;
        Ok::<(), Box<dyn Error>>(())
    };
    let client = async {
        let (connection, delivery_stream) =
            subscribed_client(&endpoint, address, thread_id).await?;
        drop(delivery_stream);
        connection.close(quinn::VarInt::from_u32(2), b"peer loss");
        drop(connection);
        Ok::<(), Box<dyn Error>>(())
    };

    let (server_result, client_result) = tokio::join!(server, client);
    server_result?;
    client_result?;
    artisan_transport::shutdown(
        &endpoint,
        quinn::VarInt::from_u32(0),
        b"delivery peer-loss test complete",
        TEST_DEADLINE,
    )
    .await?;
    drop(endpoint);
    drop(handler);
    app.shutdown().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Persisted activity history: live wake delivery plus reconnect replay
// ---------------------------------------------------------------------------

fn activity_tool_observation(
    id: &str,
    tool_id: &str,
    tool_name: &str,
    detail: &str,
) -> Observation {
    Observation::Tool(
        ToolObservation::new(
            ObservationId::parse(id).expect("fixture observation id is valid"),
            ObservationSequence::new(1).expect("run-local sequence restarts per run"),
            ObservationId::parse(tool_id).expect("fixture tool id is valid"),
            tool_name.to_owned(),
            ToolAction::Completed,
            Some(detail.to_owned()),
        )
        .expect("fixture tool row is valid"),
    )
}

/// Commits one activity row through the canonical S1b batch path the
/// dispatcher uses: fresh run-local base, checkpoint encode under the run
/// bind, content-neutral assistant rewrite, existing fencing and notifier.
#[expect(
    clippy::too_many_arguments,
    reason = "fixture commit helper mirrors the batch request fields one-for-one; a wrapper struct would only rename them"
)]
async fn commit_activity_batch(
    repository: &Repository,
    run: &SeededRun,
    batch_sequence: i64,
    expected_launch_at_ms: i64,
    expected_updated_at_ms: i64,
    operated_at_ms: i64,
    observation: Observation,
    item_id: &str,
    patch_id: &str,
) -> Result<(), Box<dyn Error>> {
    let base = repository
        .last_committed_observation_sequence(&run.launched.run_id)
        .await?;
    let checkpoint = artisan_database::encode_observation_checkpoint(
        EngineId::OpenCode2,
        run.bound.binding_version,
        base,
        &[observation],
    )
    .map_err(|_| "activity checkpoint should encode")?;
    artisan_database::validate_observation_bind(run.bound.binding_version, &run.bound)
        .map_err(|_| "activity bind should validate")?;
    let item_id = ItemId::parse(item_id)?;
    let patch_id = PatchId::parse(patch_id)?;
    let body = AssistantBody::parse("delivery assistant output")?;
    let outcome = repository
        .commit_run_batch(CommitRunBatch {
            scope: RunBatchScope {
                claimed: &run.claimed,
                launched: &run.launched,
                bound: &run.bound,
                run_start_key: &run.start_key,
                credentials: &run.credentials,
                expected_launch_at: UnixMillis::from_millis(expected_launch_at_ms),
                expected_updated_at: UnixMillis::from_millis(expected_updated_at_ms),
            },
            batch_sequence,
            operated_at: UnixMillis::from_millis(operated_at_ms),
            activate_turn_patch_id: None,
            changes: &[AssistantChange::Replace {
                item_id: &item_id,
                expected_revision: Revision::new(0),
                body: &body,
                phase: AssistantMessagePhase::Unspecified,
                patch_id: &patch_id,
            }],
            checkpoint: CheckpointUpdate::Replace(&checkpoint),
        })
        .await
        .map_err(|_| "activity batch should commit")?;
    if !matches!(
        outcome,
        artisan_database::CommitRunBatchOutcome::Committed(_)
    ) {
        return Err("activity batch should be newly committed".into());
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "fixture activation helper threads the exact identity fields under test; a wrapper struct would only rename them"
)]
async fn commit_assistant_start_at(
    repository: &Repository,
    run: &SeededRun,
    item_id: &str,
    activation_patch_id: &str,
    item_patch_id: &str,
    expected_launch_at_ms: i64,
    expected_updated_at_ms: i64,
    operated_at_ms: i64,
) -> Result<(), Box<dyn Error>> {
    let item_id = ItemId::parse(item_id)?;
    let activation_patch = PatchId::parse(activation_patch_id)?;
    let item_patch = PatchId::parse(item_patch_id)?;
    let body = AssistantBody::parse("delivery assistant output")?;
    let outcome = repository
        .commit_run_batch(CommitRunBatch {
            scope: RunBatchScope {
                claimed: &run.claimed,
                launched: &run.launched,
                bound: &run.bound,
                run_start_key: &run.start_key,
                credentials: &run.credentials,
                expected_launch_at: UnixMillis::from_millis(expected_launch_at_ms),
                expected_updated_at: UnixMillis::from_millis(expected_updated_at_ms),
            },
            batch_sequence: 1,
            operated_at: UnixMillis::from_millis(operated_at_ms),
            activate_turn_patch_id: Some(&activation_patch),
            changes: &[AssistantChange::Start {
                item_id: &item_id,
                phase: AssistantMessagePhase::Final,
                body: &body,
                patch_id: &item_patch,
            }],
            checkpoint: CheckpointUpdate::Keep,
        })
        .await?;
    if !matches!(
        outcome,
        artisan_database::CommitRunBatchOutcome::Committed(_)
    ) {
        return Err("assistant batch should be newly committed".into());
    }
    Ok(())
}

/// Settles one run terminal so its turn counts as settled history while its
/// committed observations stay replayable.
async fn settle_run_completed(
    repository: &Repository,
    run: &SeededRun,
    expected_updated_at_ms: i64,
    operated_at_ms: i64,
    item_id: &str,
) -> Result<(), Box<dyn Error>> {
    let item_id = ItemId::parse(item_id)?;
    let body = AssistantBody::parse("delivery assistant output")?;
    let item_patch = PatchId::parse("delivery-settle-item")?;
    let turn_patch = PatchId::parse("delivery-settle-turn")?;
    repository
        .complete_run(CompleteRun {
            scope: RunBatchScope {
                claimed: &run.claimed,
                launched: &run.launched,
                bound: &run.bound,
                run_start_key: &run.start_key,
                credentials: &run.credentials,
                expected_launch_at: UnixMillis::from_millis(500),
                expected_updated_at: UnixMillis::from_millis(expected_updated_at_ms),
            },
            operated_at: UnixMillis::from_millis(operated_at_ms),
            item_id: &item_id,
            expected_revision: Revision::new(1),
            body: &body,
            phase: AssistantMessagePhase::Final,
            item_patch_id: &item_patch,
            turn_patch_id: &turn_patch,
        })
        .await?;
    Ok(())
}

/// Seeds the production follow-up run on the same thread: second message,
/// claim, launch, and bind with Forge-minted follow-up identities.
async fn seed_followup_run(
    repository: &Repository,
    thread_id: &ThreadId,
) -> Result<SeededRun, Box<dyn Error>> {
    repository
        .queue_message(artisan_database::QueueMessageInput {
            request_id: RequestId::parse("delivery-message-2-request")?,
            message_id: MessageId::parse("delivery-message-2")?,
            thread_id: thread_id.clone(),
            payload: artisan_domain::QueueMessagePayload::text_only("delivery follow-up body")?,
            steer_run_id: None,
            accepted_at: UnixMillis::from_millis(950),
        })
        .await?;
    let claimed = repository
        .claim_next_message_dispatch(ClaimMessageDispatch {
            owner: DispatchLeaseOwner::new([0x22; 32]),
            claimed_at: UnixMillis::from_millis(1_000),
            lease_expires_at: UnixMillis::from_millis(2_000),
        })
        .await?
        .ok_or("follow-up dispatch should be claimable")?;
    let run_id = artisan_domain::RunId::parse("delivery-run-2")?;
    let turn_id = artisan_domain::TurnId::parse("delivery-turn-2")?;
    let item_id = ItemId::parse("delivery-item-2")?;
    let first_patch_id = PatchId::parse("delivery-patch-2-first")?;
    let second_patch_id = PatchId::parse("delivery-patch-2-second")?;
    let run_start_key = RunStartKey::new([0x45; 32]);
    let credentials = RunLaunchCredentials::new([0xa2; 32], [0xb3; 32], [0xc4; 32]);
    let settings = repository
        .read_thread_engine_settings(thread_id)
        .await?
        .ok_or("delivery settings should exist")?;
    let launched = repository
        .launch_claimed_run(LaunchClaimedRun {
            claimed: &claimed,
            run_id: &run_id,
            turn_id: &turn_id,
            item_id: &item_id,
            first_patch_id: &first_patch_id,
            second_patch_id: &second_patch_id,
            operated_at: UnixMillis::from_millis(1_100),
            run_start_key: &run_start_key,
            credentials: &credentials,
            engine_settings: &settings,
        })
        .await?;
    let launched = match launched {
        LaunchClaimedRunOutcome::Started(receipt)
        | LaunchClaimedRunOutcome::AlreadyStarted(receipt) => receipt,
    };
    let binding = ProviderBindingBytes::new(vec![0xab; 16])?;
    let bound = match repository
        .bind_run_provider(BindRunProvider {
            claimed: &claimed,
            receipt: &launched,
            run_start_key: &run_start_key,
            credentials: &credentials,
            expected_launch_at: UnixMillis::from_millis(1_100),
            bound_at: UnixMillis::from_millis(1_200),
            binding_version: 1,
            binding_bytes: &binding,
        })
        .await?
    {
        BindRunProviderOutcome::Bound(receipt) | BindRunProviderOutcome::AlreadyBound(receipt) => {
            receipt
        }
    };
    Ok(SeededRun {
        claimed,
        launched,
        bound,
        start_key: run_start_key,
        credentials,
    })
}

fn activity_subscribe_envelope(thread_id: ThreadId, frame_id: &str) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("request frame id"),
        sent_at: UnixMillis::from_millis(20),
        body: WireEnvelopeBody::Request(ClientRequest::Conversation(
            ConversationRequest::Subscribe(ConversationSubscribe::resume(
                thread_id,
                ConversationCursor::default(),
            )),
        )),
    }
}

fn activity_unsubscribe_envelope(thread_id: ThreadId, frame_id: &str) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("unsubscribe frame id"),
        sent_at: UnixMillis::from_millis(30),
        body: WireEnvelopeBody::Request(ClientRequest::Conversation(
            ConversationRequest::Unsubscribe(ConversationUnsubscribe { thread_id }),
        )),
    }
}

async fn receive_delivery_frame(
    stream: &mut quinn::RecvStream,
) -> Result<WireEnvelope, Box<dyn Error>> {
    Ok(tokio::time::timeout(TEST_DEADLINE, artisan_transport::receive_envelope(stream)).await??)
}

/// Consumes the message outbox every activation pushes after its patches
/// and observation history, returning its event cursor.
async fn receive_activation_outbox(
    stream: &mut quinn::RecvStream,
    thread_id: &ThreadId,
) -> Result<u64, Box<dyn Error>> {
    let frame = receive_delivery_frame(stream).await?;
    let kind = frame_kind(&frame.body);
    let WireEnvelopeBody::Event(event) = frame.body else {
        return Err(format!("expected the activation outbox, got {kind}").into());
    };
    let Event::MessageOutbox(outbox) = event.event else {
        return Err("expected the activation outbox event".into());
    };
    assert_eq!(outbox.thread_id(), thread_id);
    Ok(event.cursor.get())
}

/// Names the received frame shape for mismatch diagnostics without decoding
/// or logging any payload.
fn frame_kind(body: &WireEnvelopeBody) -> &'static str {
    match body {
        WireEnvelopeBody::Event(_) => "event",
        WireEnvelopeBody::PatchBatch(_) => "patch batch",
        WireEnvelopeBody::Hello(_)
        | WireEnvelopeBody::Welcome(_)
        | WireEnvelopeBody::Request(_)
        | WireEnvelopeBody::Response(_)
        | WireEnvelopeBody::ProtocolError(_) => "control",
    }
}

struct DeliveredActivity {
    /// Exact owning thread of the delivered envelope; retained for fixture
    /// diagnostics even when a given assertion reads only the observation.
    #[allow(dead_code)]
    thread_id: ThreadId,
    observation: Observation,
    run_id: String,
    turn_id: String,
    committed_at_ms: i64,
    delivery_sequence: u64,
    event_cursor: u64,
}

fn decoded_activity(envelope: WireEnvelope, thread_id: &ThreadId) -> DeliveredActivity {
    let kind = frame_kind(&envelope.body);
    let WireEnvelopeBody::Event(event) = envelope.body else {
        panic!("an activity delivery must arrive as an event frame, got {kind}");
    };
    let Event::EngineObservation(delivered) = event.event else {
        panic!("an activity delivery must carry an engine observation");
    };
    assert_eq!(&delivered.thread_id, thread_id);
    let attribution = delivered
        .attribution
        .as_ref()
        .expect("persisted rows carry attribution");
    DeliveredActivity {
        thread_id: delivered.thread_id,
        observation: delivered.observation,
        run_id: attribution.run_id.as_str().to_owned(),
        turn_id: attribution.turn_id.as_str().to_owned(),
        committed_at_ms: attribution.committed_at.as_millis(),
        delivery_sequence: attribution.delivery_sequence,
        event_cursor: event.cursor.get(),
    }
}

async fn serve_activity_delivery(
    listener: ForgeListener,
    handler: &RequestHandler,
    cancel: &CancelHandle,
) -> Result<(), Box<dyn Error>> {
    let (listener, report) = listener.serve_one(handler, cancel).await?;
    // Diagnostic-first assertions: the termination Debug names the exact
    // failing delivery stage (Replay = patch/observation history read,
    // Writer/Send = wire send, Registry = registrar advance,
    // ResnapshotRequired = cursor beyond the tail), and completed_requests
    // locates it (1 = initial activation delivery, else the wake drain).
    // Five requests: subscribe, barrier, unsubscribe, resubscribe, final
    // unsubscribe.
    if report.completed_requests != 5 {
        return Err(format!(
            "activity delivery served {} of 5 requests (termination={:?})",
            report.completed_requests, report.termination
        )
        .into());
    }
    if !matches!(
        report.termination,
        RequestTermination::Failed {
            source: DeadlineError::Cancelled {
                operation: OperationKind::Receive
            }
        }
    ) {
        return Err(format!(
            "activity delivery ended with unexpected termination (termination={:?})",
            report.termination
        )
        .into());
    }
    listener.drain().await?;
    Ok(())
}

/// Two real S1b observation commits on two runs share one thread and settle
/// the first turn; one commit wake then drives both attributed events in
/// thread-scoped order over the real delivery stream, and a resubscribe
/// replays the same persisted history from the durable ledger.
///
/// Both durable rows restart at run-local sequence 1 while their
/// `delivery_sequence` values strictly increase (1, 2) with Forge-persisted
/// run/turn/committed-at attribution — the exact live-plus-reconnect route.
#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
#[tokio::test]
async fn activity_history_drives_live_delivery_and_reconnect_replay() -> Result<(), Box<dyn Error>>
{
    let (_temporary, app) = opened_app().await?;
    let repository = app.repository().clone();
    let seeded = seed_thread(&repository).await?;
    let thread_id = seeded.thread_id.clone();

    let pki = test_pki();
    let endpoint = artisan_transport::bind_loopback_client(client_config(&pki))?;
    let notifier = ConversationCommitNotifier::new();
    let handler =
        RequestHandler::new(repository.clone()).with_conversation_commit_notifier(notifier.clone());
    let listener = ForgeListener::bind(
        server_config(&pki),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        Box::new(TestOrigin::new()),
        listener_limits(),
        std::num::NonZeroU32::new(1).expect("admission capacity"),
        std::num::NonZeroU32::new(8).expect("request capacity"),
    )?;
    let address = listener.local_addr()?;
    let cancel = CancelHandle::new();

    let server = serve_activity_delivery(listener, &handler, &cancel);
    let client = async {
        let connection = connect_client(&endpoint, address).await?;
        let (mut control_send, mut control_recv) = connection.open_bi().await?;
        let _welcome = artisan_transport::client_handshake(
            &mut control_send,
            &mut control_recv,
            hello_envelope(),
        )
        .await?;
        let (mut request_send, mut request_recv) = connection.open_bi().await?;
        artisan_transport::send_envelope(
            &mut request_send,
            &activity_subscribe_envelope(thread_id.clone(), "delivery-activity-subscribe"),
        )
        .await?;
        drop(request_send);
        let response = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::receive_envelope(&mut request_recv),
        )
        .await??;
        let WireEnvelopeBody::Response(response) = response.body else {
            return Err("expected the correlated subscription response".into());
        };
        let ResponsePayload::ConversationSubscriptionStarted(
            ConversationSubscriptionStarted::Resumed { cursor, .. },
        ) = response.payload
        else {
            return Err("expected a resumed subscription acknowledgement".into());
        };
        assert_eq!(cursor, ConversationCursor::default());
        let mut delivery_stream =
            tokio::time::timeout(TEST_DEADLINE, connection.accept_uni()).await??;
        let initial = receive_delivery_frame(&mut delivery_stream).await?;
        let initial_kind = frame_kind(&initial.body);
        let WireEnvelopeBody::PatchBatch(initial_batch) = initial.body else {
            return Err(format!("expected the initial replay batch, got {initial_kind}").into());
        };
        let base_cursor = initial_batch.to_cursor();
        assert_eq!(
            receive_activation_outbox(&mut delivery_stream, &thread_id).await?,
            1
        );

        // Ordered barrier proving the initial activation drain (patches plus
        // the still-empty observation history) finished before any commit:
        // the driver loop is strictly sequential, so this response can only
        // arrive after the activation delivery completed. Without it, commits
        // could land mid-activation and the initial drain would legally send
        // activity events ahead of the wake patch batch on the shared
        // stream. Unsubscribing an unknown thread mutates nothing.
        let (mut barrier_send, mut barrier_recv) = connection.open_bi().await?;
        artisan_transport::send_envelope(
            &mut barrier_send,
            &activity_unsubscribe_envelope(
                ThreadId::parse("delivery-unknown-thread").expect("thread id"),
                "delivery-activity-barrier",
            ),
        )
        .await?;
        drop(barrier_send);
        let barrier = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::receive_envelope(&mut barrier_recv),
        )
        .await??;
        let WireEnvelopeBody::Response(barrier) = barrier.body else {
            return Err("expected the barrier unsubscribe response".into());
        };
        let ResponsePayload::ConversationSubscriptionStopped(stopped) = barrier.payload else {
            return Err("expected the barrier stop acknowledgement".into());
        };
        assert_eq!(stopped.thread_id.as_str(), "delivery-unknown-thread");
        drop(barrier_recv);

        // Two real commits on two runs — the first turn settled — before one
        // coalesced wake.
        commit_assistant_start(&repository, &seeded.run).await?;
        commit_activity_batch(
            &repository,
            &seeded.run,
            2,
            500,
            700,
            750,
            activity_tool_observation("obs-activity-1", "tool-activity-1", "read", "read 42 lines"),
            "delivery-assistant-item",
            "delivery-obs-1-patch",
        )
        .await?;
        settle_run_completed(
            &repository,
            &seeded.run,
            750,
            800,
            "delivery-assistant-item",
        )
        .await?;
        let run2 = seed_followup_run(&repository, &thread_id).await?;
        commit_assistant_start_at(
            &repository,
            &run2,
            "delivery-assistant-item-2",
            "delivery-assistant-2-activation",
            "delivery-assistant-2-patch",
            1_100,
            1_200,
            1_300,
        )
        .await?;
        commit_activity_batch(
            &repository,
            &run2,
            2,
            1_100,
            1_300,
            1_400,
            activity_tool_observation("obs-activity-2", "tool-activity-2", "grep", "3 matches"),
            "delivery-assistant-item-2",
            "delivery-obs-2-patch",
        )
        .await?;

        // The authoritative read already orders both rows thread-scoped before
        // the wake fires.
        let history = repository
            .read_observation_history(&thread_id, 0, 64)
            .await?;
        assert_eq!(history.len(), 2, "both runs must persist one row each");

        let _ = notifier.publish(&thread_id);
        let wake_frame = receive_delivery_frame(&mut delivery_stream).await?;
        let wake_kind = frame_kind(&wake_frame.body);
        let WireEnvelopeBody::PatchBatch(wake_batch) = wake_frame.body else {
            return Err(format!(
                "expected the wake patch batch before activity events, got {wake_kind}"
            )
            .into());
        };
        assert_eq!(wake_batch.from_cursor(), base_cursor);

        let first = decoded_activity(
            receive_delivery_frame(&mut delivery_stream).await?,
            &thread_id,
        );
        assert_eq!(first.event_cursor, 2);
        assert_eq!(first.delivery_sequence, 1);
        assert_eq!(first.run_id, "delivery-run");
        assert_eq!(first.turn_id, "delivery-turn");
        assert_eq!(first.committed_at_ms, 750);
        match &first.observation {
            Observation::Tool(row) => {
                assert_eq!(row.sequence().get(), 1);
                assert_eq!(row.tool_name(), "read");
                assert_eq!(row.detail(), Some("read 42 lines"));
            }
            other => {
                return Err(format!("first row must stay a tool row, got {}", other.tag()).into());
            }
        }
        let second = decoded_activity(
            receive_delivery_frame(&mut delivery_stream).await?,
            &thread_id,
        );
        assert_eq!(second.event_cursor, 3);
        assert_eq!(second.delivery_sequence, 2);
        assert_eq!(second.run_id, "delivery-run-2");
        assert_eq!(second.turn_id, "delivery-turn-2");
        assert_eq!(second.committed_at_ms, 1_400);
        match &second.observation {
            Observation::Tool(row) => {
                assert_eq!(row.sequence().get(), 1);
                assert_eq!(row.tool_name(), "grep");
                assert_eq!(row.detail(), Some("3 matches"));
            }
            other => {
                return Err(format!("second row must stay a tool row, got {}", other.tag()).into());
            }
        }
        // One wake delivered everything: no duplicate or trailing frame.
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                artisan_transport::receive_envelope(&mut delivery_stream),
            )
            .await
            .is_err()
        );

        let (mut unsubscribe_send, mut unsubscribe_recv) = connection.open_bi().await?;
        artisan_transport::send_envelope(
            &mut unsubscribe_send,
            &activity_unsubscribe_envelope(thread_id.clone(), "delivery-activity-unsubscribe"),
        )
        .await?;
        drop(unsubscribe_send);
        let stopped = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::receive_envelope(&mut unsubscribe_recv),
        )
        .await??;
        let WireEnvelopeBody::Response(stopped) = stopped.body else {
            return Err("expected the unsubscribe response".into());
        };
        assert!(matches!(
            stopped.payload,
            ResponsePayload::ConversationSubscriptionStopped(_)
        ));

        // Resubscribe replays the same persisted history from the durable
        // ledger, including the settled first turn, on the shared stream.
        let (mut resubscribe_send, mut resubscribe_recv) = connection.open_bi().await?;
        artisan_transport::send_envelope(
            &mut resubscribe_send,
            &activity_subscribe_envelope(thread_id.clone(), "delivery-activity-resubscribe"),
        )
        .await?;
        drop(resubscribe_send);
        let resumed = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::receive_envelope(&mut resubscribe_recv),
        )
        .await??;
        let WireEnvelopeBody::Response(resumed) = resumed.body else {
            return Err("expected the resubscribe response".into());
        };
        assert!(matches!(
            resumed.payload,
            ResponsePayload::ConversationSubscriptionStarted(
                ConversationSubscriptionStarted::Resumed { .. }
            )
        ));
        let replay_frame = receive_delivery_frame(&mut delivery_stream).await?;
        let replay_kind = frame_kind(&replay_frame.body);
        let WireEnvelopeBody::PatchBatch(replay_batch) = replay_frame.body else {
            return Err(format!("expected the replay patch batch, got {replay_kind}").into());
        };
        assert_eq!(replay_batch.from_cursor(), ConversationCursor::default());
        let replayed_first = decoded_activity(
            receive_delivery_frame(&mut delivery_stream).await?,
            &thread_id,
        );
        assert_eq!(replayed_first.event_cursor, 4);
        assert_eq!(replayed_first.delivery_sequence, 1);
        assert_eq!(replayed_first.run_id, "delivery-run");
        assert_eq!(replayed_first.committed_at_ms, 750);
        let replayed_second = decoded_activity(
            receive_delivery_frame(&mut delivery_stream).await?,
            &thread_id,
        );
        assert_eq!(replayed_second.event_cursor, 5);
        assert_eq!(replayed_second.delivery_sequence, 2);
        assert_eq!(replayed_second.run_id, "delivery-run-2");
        assert_eq!(replayed_second.committed_at_ms, 1_400);
        // Every activation ends with the thread's current message outbox.
        assert_eq!(
            receive_activation_outbox(&mut delivery_stream, &thread_id).await?,
            6
        );

        let (mut stop_send, mut stop_recv) = connection.open_bi().await?;
        artisan_transport::send_envelope(
            &mut stop_send,
            &activity_unsubscribe_envelope(thread_id.clone(), "delivery-activity-unsubscribe-2"),
        )
        .await?;
        drop(stop_send);
        let stopped = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::receive_envelope(&mut stop_recv),
        )
        .await??;
        let WireEnvelopeBody::Response(stopped) = stopped.body else {
            return Err("expected the final unsubscribe response".into());
        };
        assert!(matches!(
            stopped.payload,
            ResponsePayload::ConversationSubscriptionStopped(_)
        ));
        cancel.cancel();
        let eof = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::receive_envelope(&mut delivery_stream),
        )
        .await?;
        assert!(
            eof.is_err(),
            "finished delivery stream should contain no frame"
        );
        drop(control_send);
        drop(control_recv);
        drop(request_recv);
        drop(unsubscribe_recv);
        drop(resubscribe_recv);
        drop(stop_recv);
        drop(connection);
        Ok::<(), Box<dyn Error>>(())
    };

    let (server_result, client_result) = tokio::join!(server, client);
    // Propagate both sides together: the server termination names the
    // failing delivery stage while the client error names the failed read,
    // so a rerun exposes the actual failure instead of masking one side.
    match (server_result, client_result) {
        (Ok(()), Ok(())) => {}
        (server, client) => {
            let server_note = match server {
                Ok(()) => String::from("ok"),
                Err(error) => format!("FAILED ({error})"),
            };
            let client_note = match client {
                Ok(()) => String::from("ok"),
                Err(error) => format!("FAILED ({error})"),
            };
            return Err(format!(
                "activity delivery failed: server={server_note}, client={client_note}"
            )
            .into());
        }
    }
    artisan_transport::shutdown(
        &endpoint,
        quinn::VarInt::from_u32(0),
        b"delivery activity test complete",
        TEST_DEADLINE,
    )
    .await?;
    drop(endpoint);
    drop(handler);
    app.shutdown().await?;
    Ok(())
}

/// Host state reaches a connected Editor without a request: a changed
/// navigation record is pushed as the user's preferences, and a subscribed
/// thread's refined title is pushed as soon as it is recorded. Nothing is
/// pushed when neither changed.
#[tokio::test]
async fn preferences_and_refined_titles_are_pushed_when_they_change() -> Result<(), Box<dyn Error>>
{
    let (_temporary, app) = opened_app().await?;
    let repository = app.repository().clone();
    let seeded = seed_thread(&repository).await?;
    let thread_id = seeded.thread_id.clone();
    let pki = test_pki();
    let endpoint = artisan_transport::bind_loopback_client(client_config(&pki))?;
    let notifier = ConversationCommitNotifier::new();
    let handler =
        RequestHandler::new(repository.clone()).with_conversation_commit_notifier(notifier.clone());
    let listener = ForgeListener::bind(
        server_config(&pki),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        Box::new(TestOrigin::new()),
        listener_limits(),
        std::num::NonZeroU32::new(1).expect("admission capacity"),
        std::num::NonZeroU32::new(4).expect("request capacity"),
    )?;
    let address = listener.local_addr()?;
    let cancel = CancelHandle::new();

    let server = async {
        let (listener, _report) = listener.serve_one(&handler, &cancel).await?;
        listener.drain().await?;
        Ok::<(), Box<dyn Error>>(())
    };
    let client = async {
        let (connection, mut delivery_stream) =
            subscribed_client(&endpoint, address, thread_id.clone()).await?;
        // Like an Editor, read the preferences once; the connection pushes
        // only their later changes.
        let (mut read_send, mut read_recv) = connection.open_bi().await?;
        artisan_transport::send_envelope(
            &mut read_send,
            &WireEnvelope {
                protocol_version: ProtocolVersion::V1,
                frame_id: FrameId::parse("delivery-read-preferences")?,
                sent_at: UnixMillis::from_millis(40),
                body: WireEnvelopeBody::Request(ClientRequest::Query(
                    artisan_domain::Query::ReadUserPreferences(artisan_domain::ReadUserPreferences),
                )),
            },
        )
        .await?;
        drop(read_send);
        let read = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::receive_envelope(&mut read_recv),
        )
        .await??;
        assert!(matches!(
            read.body,
            WireEnvelopeBody::Response(ref response)
                if matches!(response.payload, ResponsePayload::UserPreferences(_))
        ));
        repository
            .record_navigation(
                &ProjectId::parse("delivery-project")?,
                Some(&thread_id),
                UnixMillis::from_millis(900),
            )
            .await?;
        notifier.publish_host_state();
        let frame = receive_delivery_frame(&mut delivery_stream).await?;
        let WireEnvelopeBody::Event(event) = frame.body else {
            return Err("expected the preferences event".into());
        };
        let Event::UserPreferences(preferences) = event.event else {
            return Err("expected the user's preferences".into());
        };
        let route = preferences.navigation.route().ok_or("route recorded")?;
        assert_eq!(route.thread_id.as_ref(), Some(&thread_id));

        repository
            .record_generated_thread_title(&thread_id, &ThreadTitle::parse("Delivery plan")?)
            .await?;
        let _ = notifier.publish(&thread_id);
        let frame = receive_delivery_frame(&mut delivery_stream).await?;
        let WireEnvelopeBody::Event(event) = frame.body else {
            return Err("expected the title event".into());
        };
        let Event::ThreadRetitled(retitled) = event.event else {
            return Err("expected the refined title".into());
        };
        assert_eq!(retitled.thread_id, thread_id);
        assert_eq!(retitled.title.as_str(), "Delivery plan");

        notifier.publish_host_state();
        let _ = notifier.publish(&thread_id);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                artisan_transport::receive_envelope(&mut delivery_stream),
            )
            .await
            .is_err(),
            "unchanged host state and titles are not pushed again"
        );
        cancel.cancel();
        drop(delivery_stream);
        drop(connection);
        Ok::<(), Box<dyn Error>>(())
    };

    let (server_result, client_result) = tokio::join!(server, client);
    server_result?;
    client_result?;
    artisan_transport::shutdown(
        &endpoint,
        quinn::VarInt::from_u32(0),
        b"delivery host state test complete",
        TEST_DEADLINE,
    )
    .await?;
    drop(endpoint);
    drop(handler);
    app.shutdown().await?;
    Ok(())
}

/// One signed-in engine for the usage push test.
#[derive(Debug)]
struct SignedInReader;

impl artisan_backend::account_usage_service::AccountUsageReader for SignedInReader {
    fn engine_id(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        "Codex"
    }

    fn read(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        artisan_native_engine::account_usage::ProviderUsage,
                        artisan_backend::account_usage_service::ReaderFailure,
                    >,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async {
            Ok(artisan_native_engine::account_usage::ProviderUsage::authenticated(Vec::new()))
        })
    }
}

/// A connection receives every engine's usage with its readiness verdict
/// from its first request on, without asking for it.
#[tokio::test]
async fn account_usage_is_pushed_from_the_first_request() -> Result<(), Box<dyn Error>> {
    use artisan_backend::account_usage_service::{
        ACCOUNT_USAGE_FRESHNESS, ACCOUNT_USAGE_PER_ENGINE_TIMEOUT, AccountUsageService,
    };
    let (_temporary, app) = opened_app().await?;
    let repository = app.repository().clone();
    let seeded = seed_thread(&repository).await?;
    let pki = test_pki();
    let endpoint = artisan_transport::bind_loopback_client(client_config(&pki))?;
    let notifier = ConversationCommitNotifier::new();
    let usage = std::sync::Arc::new(
        AccountUsageService::with_readers(
            vec![std::sync::Arc::new(SignedInReader)],
            ACCOUNT_USAGE_FRESHNESS,
            ACCOUNT_USAGE_PER_ENGINE_TIMEOUT,
        )
        .with_host_state_notifier(notifier.clone()),
    );
    usage
        .read(&artisan_domain::ReadAccountUsage::new(None, false)?)
        .await;
    let handler = RequestHandler::new(repository.clone())
        .with_conversation_commit_notifier(notifier.clone())
        .with_shared_account_usage_service(usage);
    let listener = ForgeListener::bind(
        server_config(&pki),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        Box::new(TestOrigin::new()),
        listener_limits(),
        std::num::NonZeroU32::new(1).expect("admission capacity"),
        std::num::NonZeroU32::new(4).expect("request capacity"),
    )?;
    let address = listener.local_addr()?;
    let cancel = CancelHandle::new();
    let server = async {
        let (listener, _report) = listener.serve_one(&handler, &cancel).await?;
        listener.drain().await?;
        Ok::<(), Box<dyn Error>>(())
    };
    let client = async {
        let (connection, mut delivery_stream) =
            subscribed_client(&endpoint, address, seeded.thread_id.clone()).await?;
        let frame = receive_delivery_frame(&mut delivery_stream).await?;
        let WireEnvelopeBody::Event(event) = frame.body else {
            return Err("expected the usage event".into());
        };
        let Event::AccountUsage(snapshot) = event.event else {
            return Err("expected account usage".into());
        };
        let [report] = snapshot.engines() else {
            return Err("expected one narrowed report".into());
        };
        assert_eq!(report.engine_id(), "codex");
        assert!(report.readiness().is_ready());
        cancel.cancel();
        drop(delivery_stream);
        drop(connection);
        Ok::<(), Box<dyn Error>>(())
    };
    let (server_result, client_result) = tokio::join!(server, client);
    server_result?;
    client_result?;
    artisan_transport::shutdown(
        &endpoint,
        quinn::VarInt::from_u32(0),
        b"delivery usage test complete",
        TEST_DEADLINE,
    )
    .await?;
    drop(endpoint);
    drop(handler);
    app.shutdown().await?;
    Ok(())
}

/// The live run's usage reaches its thread's subscriber as it is recorded:
/// on activation, and again when a newer report commits. Nothing is pushed
/// for a report the subscriber already has.
#[tokio::test]
async fn live_run_usage_is_pushed_as_it_changes() -> Result<(), Box<dyn Error>> {
    let (_temporary, app) = opened_app().await?;
    let repository = app.repository().clone();
    let seeded = seed_thread(&repository).await?;
    let thread_id = seeded.thread_id.clone();
    let run_id = seeded.run.launched.run_id.clone();
    let registry = artisan_backend::run_cancellation::RunCancellationRegistry::new(4)?;
    let _lease = registry.register(thread_id.clone(), run_id.clone())?;
    let usage = |sequence: u64, input: u64| {
        artisan_domain::RunUsageReport::new(artisan_domain::RunUsageReportInput {
            run_id: run_id.clone(),
            thread_id: thread_id.clone(),
            provider_session_id: "delivery-session".to_owned(),
            source_sequence: sequence,
            model_id: EngineModelId::parse("delivery-model").expect("model id"),
            provider_route_id: EngineRouteId::parse("delivery-route").expect("route id"),
            variant_id: None,
            basis: artisan_domain::RunUsageBasis::Cumulative,
            provider_turn_id: None,
            input_tokens: Some(input),
            cached_input_tokens: None,
            output_tokens: None,
            context_tokens: Some(input),
            context_window_tokens: Some(200_000),
            observed_at: UnixMillis::from_millis(900),
        })
        .expect("usage report")
    };
    let record = |report: artisan_domain::RunUsageReport| {
        let repository = repository.clone();
        async move {
            repository
                .record_run_usage(artisan_database::RecordRunUsage {
                    run_id: report.run_id(),
                    thread_id: report.thread_id(),
                    report: &report,
                })
                .await
                .map(|_| ())
        }
    };
    record(usage(1, 100)).await?;
    let pki = test_pki();
    let endpoint = artisan_transport::bind_loopback_client(client_config(&pki))?;
    let notifier = ConversationCommitNotifier::new();
    let handler = RequestHandler::new(repository.clone())
        .with_conversation_commit_notifier(notifier.clone())
        .with_run_cancellation_registry(registry.clone());
    let listener = ForgeListener::bind(
        server_config(&pki),
        LocalCapability::from_bytes(INITIAL_CAPABILITY),
        Box::new(TestOrigin::new()),
        listener_limits(),
        std::num::NonZeroU32::new(1).expect("admission capacity"),
        std::num::NonZeroU32::new(4).expect("request capacity"),
    )?;
    let address = listener.local_addr()?;
    let cancel = CancelHandle::new();
    let server = async {
        let (listener, _report) = listener.serve_one(&handler, &cancel).await?;
        listener.drain().await?;
        Ok::<(), Box<dyn Error>>(())
    };
    let pushed = |frame: WireEnvelope| -> Result<u64, Box<dyn Error>> {
        let WireEnvelopeBody::Event(event) = frame.body else {
            return Err("expected a usage event".into());
        };
        let Event::RunUsage(result) = event.event else {
            return Err("expected the live run usage".into());
        };
        let report = result.report.ok_or("usage report present")?;
        Ok(report.input_tokens().unwrap_or_default())
    };
    let client = async {
        let (connection, mut delivery_stream) =
            subscribed_client(&endpoint, address, thread_id.clone()).await?;
        assert_eq!(
            pushed(receive_delivery_frame(&mut delivery_stream).await?)?,
            100
        );
        record(usage(2, 250)).await?;
        let _ = notifier.publish(&thread_id);
        assert_eq!(
            pushed(receive_delivery_frame(&mut delivery_stream).await?)?,
            250
        );
        let _ = notifier.publish(&thread_id);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                artisan_transport::receive_envelope(&mut delivery_stream),
            )
            .await
            .is_err(),
            "unchanged usage is not pushed again"
        );
        cancel.cancel();
        drop(delivery_stream);
        drop(connection);
        Ok::<(), Box<dyn Error>>(())
    };
    let (server_result, client_result) = tokio::join!(server, client);
    server_result?;
    client_result?;
    artisan_transport::shutdown(
        &endpoint,
        quinn::VarInt::from_u32(0),
        b"delivery run usage test complete",
        TEST_DEADLINE,
    )
    .await?;
    drop(endpoint);
    drop(handler);
    app.shutdown().await?;
    Ok(())
}
