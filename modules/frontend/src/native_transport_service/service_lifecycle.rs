//! Service lifecycle for the native transport service: the application-side
//! handle, the owned service thread, custody-ordered shutdown, startup, and
//! the main service loop.
//!
//! The parent `native_transport_service` remains the owner of the command and
//! event vocabulary, session runtime fields, and domain request handlers.
//! Root mounts this file as a child module so start/stop/thread management
//! stays in one reviewable home.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::*;

impl NativeTransportService {
    #[cfg(test)]
    pub(crate) fn pending_for_test() -> (
        Self,
        tokio::sync::mpsc::Receiver<QueuedCommand>,
        Arc<AtomicBool>,
    ) {
        let (commands, receiver) = tokio::sync::mpsc::channel(8);
        let (_, events) = sync_channel(8);
        let finished = Arc::new(AtomicBool::new(false));
        (
            Self {
                commands,
                events: Arc::new(Mutex::new(events)),
                finished: finished.clone(),
                shutdown_requested: Arc::new(AtomicBool::new(false)),
                holds: ConnectionHolds::new(),
                join: Arc::new(Mutex::new(None)),
            },
            receiver,
            finished,
        )
    }

    #[cfg(test)]
    pub(crate) fn completed_for_test(events: Vec<NativeTransportEvent>) -> Self {
        let (mut service, _, finished) = Self::pending_for_test();
        let (sender, receiver) = sync_channel(events.len().max(1));
        for event in events {
            sender.send(event).unwrap();
        }
        drop(sender);
        service.events = Arc::new(Mutex::new(receiver));
        finished.store(true, Ordering::Release);
        service
    }

    /// Starts the one service thread and its owned Tokio runtime.
    ///
    /// No application or GPUI value is captured by the service closure.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceSpawnError::Thread`] if the service thread cannot be
    /// created.
    pub fn spawn() -> Result<Self, ServiceSpawnError> {
        Self::spawn_for_host(crate::native_hosts::selected_home())
    }

    /// Starts a connection for an explicitly selected host, independently of process arguments.
    ///
    /// # Errors
    /// Returns [`ServiceSpawnError::Thread`] if the service thread cannot be created.
    pub fn spawn_for_host(home: Option<std::path::PathBuf>) -> Result<Self, ServiceSpawnError> {
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(COMMAND_CAPACITY);
        let (event_tx, event_rx) = sync_channel(EVENT_CAPACITY);
        let finished = Arc::new(AtomicBool::new(false));
        let finished_for_thread = Arc::clone(&finished);
        let join = thread::Builder::new()
            .name("artisan-native-transport".to_owned())
            .spawn(move || {
                let starting_sent = event_tx.send(NativeTransportEvent::Starting).is_ok();
                if starting_sent {
                    if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        runtime.block_on(service_main(command_rx, event_tx, home));
                    } else {
                        let _ = event_tx.send(NativeTransportEvent::Failed(
                            ServiceFailure::unavailable(ServiceFailureStage::Handshake),
                        ));
                        let _ =
                            event_tx.send(NativeTransportEvent::Stopped(ServiceStopStatus::Failed));
                    }
                }
                finished_for_thread.store(true, Ordering::Release);
            })
            .map_err(|_| ServiceSpawnError::Thread)?;

        Ok(Self {
            commands: command_tx,
            events: Arc::new(Mutex::new(event_rx)),
            finished,
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            holds: ConnectionHolds::new(),
            join: Arc::new(Mutex::new(Some(join))),
        })
    }

    /// The holds that keep this connection open while mutations are in flight.
    #[must_use]
    pub const fn holds(&self) -> &Arc<ConnectionHolds> {
        &self.holds
    }

    /// Tries to admit one command without waiting for capacity.
    ///
    /// A mutating command takes a connection hold on admission; the hold
    /// travels with it and releases once its handler has returned.
    ///
    /// # Errors
    ///
    /// Returns [`CommandSendError::Busy`] when the bounded command queue is
    /// full or the connection is sealed for a host switch or quit, or
    /// [`CommandSendError::Stopped`] after the service has stopped.
    pub fn submit(&self, command: NativeTransportCommand) -> Result<(), CommandSendError> {
        match command {
            NativeTransportCommand::Shutdown => self.request_shutdown(),
            command => {
                if self.shutdown_requested.load(Ordering::Acquire) {
                    return Err(CommandSendError::Stopped);
                }
                let hold = match command.hold_kind() {
                    Some(kind) => Some(self.holds.try_hold(kind).ok_or(CommandSendError::Busy)?),
                    None => None,
                };
                try_send_command(&self.commands, QueuedCommand { command, hold })
            }
        }
    }

    /// Admits one mutating command under a live application-level hold.
    ///
    /// The command takes an extension of `parent` instead of a fresh hold,
    /// so work that began before the connection was sealed (a draft's next
    /// coalesced save) is still admitted while the connection drains.
    ///
    /// # Errors
    ///
    /// Returns [`CommandSendError::Busy`] when the bounded command queue is
    /// full, or [`CommandSendError::Stopped`] after the service has stopped.
    pub fn submit_under(
        &self,
        command: NativeTransportCommand,
        parent: &Hold,
    ) -> Result<(), CommandSendError> {
        if self.shutdown_requested.load(Ordering::Acquire) {
            return Err(CommandSendError::Stopped);
        }
        let hold = command.hold_kind().map(|_| parent.extend());
        try_send_command(&self.commands, QueuedCommand { command, hold })
    }

    /// Requests shutdown once, retaining nonblocking admission semantics.
    ///
    /// # Errors
    ///
    /// Returns [`CommandSendError::Busy`] when the bounded command queue is
    /// full, or [`CommandSendError::Stopped`] when its receiver is gone.
    pub fn request_shutdown(&self) -> Result<(), CommandSendError> {
        if self
            .shutdown_requested
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }
        match try_send_command(&self.commands, NativeTransportCommand::Shutdown) {
            Ok(()) => Ok(()),
            Err(CommandSendError::Busy) => {
                self.shutdown_requested.store(false, Ordering::Release);
                Err(CommandSendError::Busy)
            }
            Err(CommandSendError::Stopped) => Err(CommandSendError::Stopped),
        }
    }

    /// Receives one event without blocking the GPUI thread.
    ///
    /// # Errors
    ///
    /// Returns [`EventReceiveError::Stopped`] when the service has released
    /// the event sender.
    pub fn try_recv(&self) -> Result<Option<NativeTransportEvent>, EventReceiveError> {
        let receiver = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        match receiver.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(EventReceiveError::Stopped),
        }
    }

    /// Returns whether the service thread has completed its final event path.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Joins after completion has been observed.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceJoinError::NotFinished`] if joining would block, or
    /// [`ServiceJoinError::Panicked`] if the service thread panicked.
    pub fn join(&self) -> Result<(), ServiceJoinError> {
        if !self.is_finished() {
            return Err(ServiceJoinError::NotFinished);
        }
        let mut join = self.join.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(handle) = join.take() else {
            return Ok(());
        };
        handle.join().map_err(|_| ServiceJoinError::Panicked)
    }
}

/// Tries to admit one command to the bounded Tokio queue. A refused command
/// is dropped here, releasing any hold it carried.
///
/// # Errors
///
/// Returns [`CommandSendError::Busy`] when the queue is full, or
/// [`CommandSendError::Stopped`] when its receiver is closed.
pub fn try_send_command(
    sender: &tokio::sync::mpsc::Sender<QueuedCommand>,
    command: impl Into<QueuedCommand>,
) -> Result<(), CommandSendError> {
    match sender.try_send(command.into()) {
        Ok(()) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Err(CommandSendError::Busy),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(CommandSendError::Stopped),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CustodyStep {
    SessionShutdown,
    ReconnectQuarantine,
    LeaseShutdown,
    ReconnectRelease,
    Stopped,
}

pub(super) fn cleanup_plan(
    has_session: bool,
    has_reconnect_lease: bool,
    has_lease: bool,
) -> Vec<CustodyStep> {
    let mut plan = Vec::with_capacity(5);
    if has_session {
        plan.push(CustodyStep::SessionShutdown);
    }
    if has_reconnect_lease {
        plan.push(CustodyStep::ReconnectQuarantine);
    }
    if has_lease {
        plan.push(CustodyStep::LeaseShutdown);
    }
    if has_reconnect_lease {
        plan.push(CustodyStep::ReconnectRelease);
    }
    plan.push(CustodyStep::Stopped);
    plan
}

struct SessionMaterial {
    certificate: CertificateDer<'static>,
    target: LoopbackTarget,
    pinned_identity: PinnedIdentity,
    limits: ClientSessionLimits,
    binding: ReconnectBinding,
}

impl ServiceRuntime {
    #[cfg(test)]
    pub(super) fn new_for_batch_tests() -> Self {
        let certificate = CertificateDer::from(vec![7u8; 32]);
        let pinned_identity = PinnedIdentity::from_certificate(&certificate);
        let target = LoopbackTarget::new("127.0.0.1:40123".parse().expect("loopback"))
            .expect("loopback target");
        let binding = build_reconnect_binding([9u8; 16], target, pinned_identity, 19)
            .expect("reconnect binding");
        Self {
            preserve_reconnect: false,
            session: None,
            reconnect_lease: None,
            reconnect_binding: binding,
            certificate,
            target: target.into(),
            pinned_identity,
            limits: ClientSessionLimits {
                connect: Duration::from_secs(1),
                handshake: Duration::from_secs(1),
                request: Duration::from_secs(1),
                shutdown: Duration::from_secs(1),
                admission_budget: 1,
            },
            lease: None,
            cancel: CancelHandle::new(),
            shutdown_grace: Duration::ZERO,
            known_threads: HashSet::new(),
            stored_attachments: HashSet::new(),
            intake: IntakeState::new(),
            custody: SubscriptionCustody::new(),
            delivery_cancel: None,
            delivery_join: None,
            delivery_tx: None,
        }
    }

    pub(super) async fn cleanup(&mut self) -> Result<(), ServiceFailure> {
        if let Some(thread_id) = self.custody.active_thread().cloned() {
            self.custody.on_unsubscribe(&thread_id);
        }
        if let Some(cancel) = self.delivery_cancel.take() {
            cancel.cancel();
        }
        if let Some(join) = self.delivery_join.take() {
            let _ = join.await;
        }
        let mut failed = false;
        for step in cleanup_plan(
            self.session.is_some(),
            self.reconnect_lease.is_some(),
            self.lease.is_some(),
        ) {
            match step {
                CustodyStep::SessionShutdown => {
                    if let Some(session) = self.session.take()
                        && let Err(error) = session.shutdown(&self.cancel).await
                    {
                        eprintln!("Forge session disconnect failed: {error}");
                        failed = true;
                    }
                }
                CustodyStep::ReconnectQuarantine => {
                    // Endpoint closure never presents the next credential to Forge.
                    // Preserve it for an external daemon even if draining times out.
                    if self.preserve_reconnect {
                        continue;
                    }
                    if let Some(lease) = self.reconnect_lease.take() {
                        match lease.quarantine_for_shutdown() {
                            Ok(lease) => self.reconnect_lease = Some(lease),
                            Err(_) => failed = true,
                        }
                    }
                }
                CustodyStep::LeaseShutdown => {
                    if let Some(lease) = self.lease.take()
                        && lease.shutdown(self.shutdown_grace).await.is_err()
                    {
                        failed = true;
                    }
                }
                CustodyStep::ReconnectRelease => drop(self.reconnect_lease.take()),
                CustodyStep::Stopped => {}
            }
        }
        if failed {
            Err(ServiceFailure::new(
                ServiceFailureStage::Cleanup,
                ServiceFailureCategory::Cleanup,
            ))
        } else {
            Ok(())
        }
    }
}

async fn start_native_service(
    home: Option<&Path>,
) -> Result<(ServiceRuntime, FrameFactory), StartupError> {
    if let Some(home) = home {
        return super::remote::start(home).await;
    }
    if let Some(home) = dev_endpoint::dev_home_from_env() {
        return start_dev_service(&home).await;
    }
    let layout =
        Layout::discover().map_err(|_| StartupError::Stage(ServiceFailureStage::Layout))?;
    let manifest = InstallationManifest::load(&layout.manifest)
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Manifest))?;
    if manifest.install_root != layout.root {
        return Err(StartupError::Stage(ServiceFailureStage::Manifest));
    }
    match artisan_editor_cli::payload::verify(&manifest.version_root()) {
        artisan_editor_cli::payload::PayloadHealth::Verified => {}
        artisan_editor_cli::payload::PayloadHealth::Modified(_)
        | artisan_editor_cli::payload::PayloadHealth::Unverifiable => {
            return Err(StartupError::PayloadUnverified);
        }
    }
    let config = NativeInstanceConfig::load_from_home(&layout.root)
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Instance))?;
    let credentials = load_client_credentials(&layout.root)
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Credentials))?;
    let launch_spec = ForgeLaunchSpec::new(&manifest, &config, credentials.paths())
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Forge))?;
    let lease = start_owned(&launch_spec)
        .await
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Forge))?;
    let mut frames = FrameFactory::new();
    let runtime =
        attach_to_owned_forge(lease, &layout.root, &config, credentials, &mut frames).await?;
    Ok((runtime, frames))
}

async fn attach_to_owned_forge(
    lease: ForgeProcessLease,
    home: &Path,
    config: &NativeInstanceConfig,
    credentials: NativeClientCredentials,
    frames: &mut FrameFactory,
) -> Result<ServiceRuntime, StartupError> {
    let result = establish_session(home, config, &lease, credentials, frames).await;
    match result {
        Ok((session, reconnect_lease, cancel, shutdown_grace, material)) => Ok(ServiceRuntime {
            preserve_reconnect: false,
            session: Some(session),
            reconnect_lease: Some(reconnect_lease),
            reconnect_binding: material.binding,
            certificate: material.certificate,
            target: material.target.into(),
            pinned_identity: material.pinned_identity,
            limits: material.limits,
            lease: Some(lease),
            cancel,
            shutdown_grace,
            known_threads: HashSet::new(),
            stored_attachments: HashSet::new(),
            intake: IntakeState::new(),
            custody: SubscriptionCustody::new(),
            delivery_cancel: None,
            delivery_join: None,
            delivery_tx: None,
        }),
        Err(error) => {
            let shutdown_grace =
                finite_duration(config.listener().drain_timeout_ms()).unwrap_or(Duration::ZERO);
            let _ = lease.shutdown(shutdown_grace).await;
            Err(error)
        }
    }
}

/// Starts the explicitly opted-in development session against a manually
/// started dev Forge.
///
/// The dev home shares credential files and the readiness receipt with the
/// backend process. The QUIC handshake, bootstrap capability, reconnect
/// store, and request surface are identical to the owned path; only process
/// custody differs (there is no owned lease to shut down, so `lease` stays
/// `None` and cleanup skips the lease step).
async fn start_dev_service(home: &Path) -> Result<(ServiceRuntime, FrameFactory), StartupError> {
    if !home.is_absolute() {
        return Err(StartupError::Stage(ServiceFailureStage::Instance));
    }
    if dev_endpoint::dev_home_is_installed(home) {
        return Err(StartupError::Stage(ServiceFailureStage::Instance));
    }
    // Provisioning first means the first dev run creates the credential
    // files the manually started backend needs; the readiness wait below
    // then fails honestly until that backend is up.
    let credentials = load_client_credentials(home).map_err(|error| {
        eprintln!("artisan dev forge: credential load failed: {error}");
        StartupError::Stage(ServiceFailureStage::Credentials)
    })?;
    let ready_path =
        dev_endpoint::dev_ready_path(home, dev_endpoint::dev_ready_override_from_env().as_deref());
    let readiness = wait_for_dev_readiness(&ready_path).await?;
    eprintln!("artisan dev forge: readiness ok ({})", readiness.endpoint());
    let (certificate, capability) = credentials.into_parts();
    let pinned_identity = PinnedIdentity::from_certificate(&certificate);
    if readiness.certificate_sha256() != pinned_identity.to_hex() {
        return Err(StartupError::Stage(ServiceFailureStage::Readiness));
    }
    let target = LoopbackTarget::new(
        readiness
            .endpoint()
            .parse::<SocketAddr>()
            .map_err(|_| StartupError::Stage(ServiceFailureStage::Readiness))?,
    )
    .map_err(|_| StartupError::Stage(ServiceFailureStage::Readiness))?;
    let forge_pid = NonZeroU32::new(readiness.pid())
        .ok_or(StartupError::Stage(ServiceFailureStage::Readiness))?;
    let binding = ReconnectBinding::new(
        dev_endpoint::mint_dev_instance_id(),
        target.addr().port(),
        *pinned_identity.as_bytes(),
        forge_pid,
    )
    .map_err(|_| StartupError::Stage(ServiceFailureStage::Instance))?;
    let mut frames = FrameFactory::new();
    let hello_stamp = frames.next()?;
    let hello = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: hello_stamp.frame_id,
        sent_at: hello_stamp.sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])
                .map_err(|_| StartupError::Stage(ServiceFailureStage::Handshake))?,
            credential: HelloCredential::Initial(capability),
            supports_lifecycle_control: false,
        }),
    };
    let limits = ClientSessionLimits {
        connect: finite_duration(dev_endpoint::DEV_CONNECT_TIMEOUT_MS)?,
        handshake: finite_duration(dev_endpoint::DEV_HANDSHAKE_TIMEOUT_MS)?,
        request: finite_duration(dev_endpoint::DEV_REQUEST_TIMEOUT_MS)?,
        shutdown: finite_duration(dev_endpoint::DEV_SHUTDOWN_TIMEOUT_MS)?,
        admission_budget: dev_endpoint::DEV_ADMISSION_BUDGET,
    };
    let shutdown_grace = limits.shutdown;
    let trusted_certificate = certificate.clone();
    let cancel = CancelHandle::new();
    let (session, welcome) =
        ClientSession::connect(target, certificate, pinned_identity, hello, limits, &cancel)
            .await
            .map_err(|error| {
                eprintln!("artisan dev forge: connect failed: {error}");
                StartupError::Stage(ServiceFailureStage::Handshake)
            })?;
    let reconnect_store = ReconnectCapabilityStore::from_home(home)
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Credentials))?;
    let reconnect_lease = reconnect_store
        .initialize_owner_lease(
            binding,
            welcome.welcome.reconnect_capability,
            RECONNECT_LOCK_TIMEOUT,
        )
        .map_err(|_| StartupError::Stage(ServiceFailureStage::Credentials))?;
    eprintln!("artisan dev forge: connected ({})", readiness.endpoint());
    Ok((
        ServiceRuntime {
            preserve_reconnect: false,
            session: Some(session),
            reconnect_lease: Some(reconnect_lease),
            reconnect_binding: binding,
            certificate: trusted_certificate,
            target: target.into(),
            pinned_identity,
            limits,
            lease: None,
            cancel,
            shutdown_grace,
            known_threads: HashSet::new(),
            stored_attachments: HashSet::new(),
            intake: IntakeState::new(),
            custody: SubscriptionCustody::new(),
            delivery_cancel: None,
            delivery_join: None,
            delivery_tx: None,
        },
        frames,
    ))
}

/// Waits for the manually started dev Forge to publish its readiness receipt.
///
/// The wait is bounded: the dev Forge needs a moment to migrate storage and
/// bind its loopback listener after launch. Expiry is a typed readiness
/// failure, never a fabricated endpoint.
async fn wait_for_dev_readiness(path: &Path) -> Result<ForgeReadiness, StartupError> {
    let deadline =
        std::time::Instant::now() + Duration::from_millis(dev_endpoint::DEV_READY_WAIT_MS);
    loop {
        if let Some(readiness) = read_dev_readiness(path) {
            return Ok(readiness);
        }
        if std::time::Instant::now() >= deadline {
            return Err(StartupError::Stage(ServiceFailureStage::Readiness));
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        tokio::time::sleep(remaining.min(Duration::from_millis(dev_endpoint::DEV_READY_POLL_MS)))
            .await;
    }
}

/// Reads and validates one dev readiness receipt.
///
/// Returns `None` while the receipt is absent or not yet valid; the caller
/// polls until its bounded deadline. Only fully validated receipts are
/// returned: exact schema, exact loopback endpoint, lowercase pin, and
/// nonzero PID are enforced by the shared readiness parser.
fn read_dev_readiness(path: &Path) -> Option<ForgeReadiness> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > dev_endpoint::DEV_READY_MAX_BYTES {
        return None;
    }
    ForgeReadiness::from_json(&bytes).ok()
}

async fn establish_session(
    home: &Path,
    config: &NativeInstanceConfig,
    lease: &ForgeProcessLease,
    credentials: NativeClientCredentials,
    frames: &mut FrameFactory,
) -> Result<
    (
        ClientSession,
        ReconnectSessionLease,
        CancelHandle,
        Duration,
        SessionMaterial,
    ),
    StartupError,
> {
    let readiness_endpoint = lease.readiness().endpoint().to_owned();
    let readiness_process_id = lease.readiness().pid();
    let readiness_certificate_pin = lease.readiness().certificate_sha256().to_owned();
    let (certificate, capability) = credentials.into_parts();
    let pinned_identity = PinnedIdentity::from_certificate(&certificate);
    let expected_pin = pinned_identity.to_hex();
    let target = validate_readiness(
        &readiness_endpoint,
        readiness_process_id,
        lease.pid(),
        &readiness_certificate_pin,
        &expected_pin,
    )
    .map_err(|_| StartupError::Stage(ServiceFailureStage::Readiness))?;
    let binding =
        build_reconnect_binding(config.instance_id(), target, pinned_identity, lease.pid())?;

    let hello_stamp = frames.next()?;
    let hello = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: hello_stamp.frame_id,
        sent_at: hello_stamp.sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])
                .map_err(|_| StartupError::Stage(ServiceFailureStage::Handshake))?,
            credential: HelloCredential::Initial(capability),
            supports_lifecycle_control: false,
        }),
    };
    let listener = config.listener();
    let limits = ClientSessionLimits {
        connect: finite_duration(listener.admission_timeout_ms())?,
        handshake: finite_duration(listener.handshake_timeout_ms())?,
        request: finite_duration(listener.request_timeout_ms())?,
        shutdown: finite_duration(listener.drain_timeout_ms())?,
        admission_budget: usize::try_from(listener.requests_per_connection().get())
            .map_err(|_| StartupError::Stage(ServiceFailureStage::Instance))?,
    };
    let trusted_certificate = certificate.clone();
    let cancel = CancelHandle::new();
    let (session, welcome) =
        ClientSession::connect(target, certificate, pinned_identity, hello, limits, &cancel)
            .await
            .map_err(|_| StartupError::Stage(ServiceFailureStage::Handshake))?;
    let Ok(reconnect_store) = ReconnectCapabilityStore::from_home(home) else {
        let _ = session.shutdown(&cancel).await;
        return Err(StartupError::Stage(ServiceFailureStage::Credentials));
    };
    let Ok(reconnect_lease) = reconnect_store.initialize_owner_lease(
        binding,
        welcome.welcome.reconnect_capability,
        RECONNECT_LOCK_TIMEOUT,
    ) else {
        let _ = session.shutdown(&cancel).await;
        return Err(StartupError::Stage(ServiceFailureStage::Credentials));
    };
    Ok((
        session,
        reconnect_lease,
        cancel,
        limits.shutdown,
        SessionMaterial {
            certificate: trusted_certificate,
            target,
            pinned_identity,
            limits,
            binding,
        },
    ))
}

async fn service_main(
    mut commands: tokio::sync::mpsc::Receiver<QueuedCommand>,
    events: SyncSender<NativeTransportEvent>,
    home: Option<std::path::PathBuf>,
) {
    let mut status = ServiceStopStatus::Clean;
    let started = start_native_service(home.as_deref()).await;
    match started {
        Ok((mut runtime, mut frames)) => {
            let (delivery_tx, mut delivery_rx) = tokio::sync::mpsc::channel::<PrivateDelivery>(64);
            runtime.delivery_tx = Some(delivery_tx.clone());
            // take_delivery exactly once for this session
            let delivery_started = {
                let session = runtime.session.take();
                match session {
                    Some(session) => match session.take_delivery() {
                        Ok((session, receiver)) => {
                            runtime.session = Some(session);
                            let cancel = Arc::new(CancelHandle::new());
                            let version = runtime
                                .session
                                .as_ref()
                                .map_or(ProtocolVersion::V1, ClientSession::protocol_version);
                            let join = tokio::spawn(delivery_task_loop(
                                receiver,
                                delivery_tx.clone(),
                                Arc::clone(&cancel),
                                version,
                            ));
                            runtime.delivery_cancel = Some(cancel);
                            runtime.delivery_join = Some(join);
                            Ok(())
                        }
                        Err(_) => Err(ServiceFailure::local_session()),
                    },
                    None => Err(ServiceFailure::local_session()),
                }
            };
            if let Err(failure) = delivery_started {
                status = ServiceStopStatus::Failed;
                crate::dev_startup_receipt::report_failed(failure);
                let _ = publish(&events, NativeTransportEvent::Failed(failure));
            } else {
                let run_result = load_initial_catalog(&mut runtime, &mut frames, &events).await;
                if let Err(failure) = run_result {
                    status = ServiceStopStatus::Failed;
                    crate::dev_startup_receipt::report_failed(failure);
                    let _ = publish(&events, NativeTransportEvent::Failed(failure));
                } else {
                    crate::dev_startup_receipt::report_ready();
                    let command_result = command_loop_with_delivery(
                        &mut commands,
                        &mut delivery_rx,
                        &mut runtime,
                        &mut frames,
                        &events,
                    )
                    .await;
                    if let Err(failure) = command_result {
                        status = ServiceStopStatus::Failed;
                        crate::dev_startup_receipt::report_failed(failure);
                        let _ = publish(&events, NativeTransportEvent::Failed(failure));
                    }
                }
            }
            if runtime.cleanup().await.is_err() {
                status = ServiceStopStatus::Failed;
                let _ = publish(
                    &events,
                    NativeTransportEvent::Failed(ServiceFailure::new(
                        ServiceFailureStage::Cleanup,
                        ServiceFailureCategory::Cleanup,
                    )),
                );
            }
        }
        Err(error) => {
            status = ServiceStopStatus::Failed;
            let failure = error.failure();
            crate::dev_startup_receipt::report_failed(failure);
            let _ = publish(&events, NativeTransportEvent::Failed(failure));
        }
    }
    let _ = publish(&events, NativeTransportEvent::Stopped(status));
}
