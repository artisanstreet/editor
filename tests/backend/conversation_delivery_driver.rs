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
    AttachProjectInput, BindRunProvider, BindRunProviderOutcome, ClaimMessageDispatch,
    ConversationPatchReplay, CreateThreadInput, DispatchLeaseOwner, LaunchClaimedRun,
    LaunchClaimedRunOutcome, ProviderBindingBytes, QueueFirstMessageInput, Repository,
    RunLaunchCredentials, RunStartKey, SetThreadEngineConfigInput, SqliteConfig,
};
use artisan_domain::{
    ApprovalMode, AssistantBody, AssistantMessagePhase, ByteLimit, ConversationCursor,
    ConversationRequest, ConversationSubscribe, ConversationUnsubscribe, CountLimit, DisplayName,
    EngineAgentId, EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FilesystemAccess, FiniteMillis, ItemId,
    MessageBody, MessageId, NetworkAccess, OpenCode2Selection, PatchId, PermissionId, ProjectId,
    RequestId, RootPath, ThreadId, ThreadTitle, UnixMillis, WebSearchAccess,
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

async fn seed_thread(repository: &Repository) -> Result<SeededThread, Box<dyn Error>> {
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
            title: ThreadTitle::parse("Delivery thread")?,
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
    artisan_transport::send_envelope(&mut request_send, &resume_request(thread_id)).await?;
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
    drop(control_send);
    drop(control_recv);
    drop(request_recv);
    Ok((connection, delivery_stream))
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

    let server = async {
        let (listener, report) = listener.serve_one(&handler, &cancel).await?;
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
        Ok::<(), Box<dyn Error>>(())
    };
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
        artisan_transport::send_envelope(&mut request_send, &resume_request(thread_id.clone()))
            .await?;
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
        commit_assistant_start(&repository, &seeded.run).await?;
        let expected_second = match repository
            .read_conversation_patch_replay(&thread_id, first_cursor)
            .await?
        {
            ConversationPatchReplay::Batch(batch) => batch,
            other => {
                return Err(format!("expected a contiguous wake batch, got {other:?}").into());
            }
        };
        assert_eq!(expected_second.from_cursor(), first_cursor);
        let _ = notifier.publish(&thread_id);
        let second_delivery = tokio::time::timeout(
            TEST_DEADLINE,
            artisan_transport::receive_envelope(&mut delivery_stream),
        )
        .await??;
        match &second_delivery.body {
            WireEnvelopeBody::PatchBatch(actual) => assert_eq!(actual, &expected_second),
            _ => return Err("expected the wake patch batch on the same delivery stream".into()),
        }
        let _ = notifier.publish(&thread_id);
        let _ = notifier.publish(&thread_id);
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
        Ok::<(WireEnvelope, WireEnvelope, WireEnvelope, WireEnvelope), Box<dyn Error>>((
            response,
            delivery,
            stopped,
            second_delivery,
        ))
    };

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
