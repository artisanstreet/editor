//! External Forge startup. No process lease is acquired and remote shutdown is never requested.
use super::*;
use artisan_domain::ErrorChain;
use artisan_editor_cli::credentials::{ForgeCredentialError, hosts};
use artisan_transport::SessionTarget;

/// Reports the step a remote Forge connection stopped at, with its whole
/// cause chain, and classifies it as a credentials-stage failure.
///
/// Credential errors name paths and stages only, never secret bytes.
fn credential_failure(step: &str, error: &(dyn std::error::Error + 'static)) -> StartupError {
    eprintln!(
        "Remote Forge connection failed (credentials): {step}: {}",
        ErrorChain(error)
    );
    StartupError::Stage(ServiceFailureStage::Credentials)
}

#[expect(
    clippy::too_many_lines,
    reason = "one linear connection sequence; each step reports its own failure"
)]
pub(super) async fn start(home: &Path) -> Result<(ServiceRuntime, FrameFactory), StartupError> {
    let resolved_home = crate::native_hosts::resolve_home(home)
        .map_err(|error| credential_failure("refreshing the host invitation", &error))?;
    let home = resolved_home.as_path();
    let bytes = hosts::read_private(home, "host.json")
        .map_err(|error| credential_failure("reading the host invitation (host.json)", &error))?;
    let host = hosts::HostInvitation::decode(&bytes)
        .map_err(|error| credential_failure("decoding the host invitation", &error))?;
    let certificate = CertificateDer::from(host.certificate_der().map_err(|error| {
        credential_failure("decoding the host certificate from the invitation", &error)
    })?);
    let pinned_identity = PinnedIdentity::from_certificate(&certificate);
    let target = SessionTarget::remote(host.endpoint).map_err(|error| {
        credential_failure("building the session target from the host endpoint", &error)
    })?;
    let pid = NonZeroU32::new(host.pid).ok_or_else(|| {
        eprintln!(
            "Remote Forge connection failed (credentials): the host invitation carries process id 0"
        );
        StartupError::Stage(ServiceFailureStage::Credentials)
    })?;
    let binding = ReconnectBinding::new(
        host.incarnation,
        host.endpoint.port(),
        *pinned_identity.as_bytes(),
        pid,
    )
    .map_err(|error| credential_failure("building the reconnect binding", &error))?;
    let store = ReconnectCapabilityStore::from_home(home)
        .map_err(|error| credential_failure("opening the reconnect capability store", &error))?;
    let mut attempt = match store.checkout(binding, RECONNECT_LOCK_TIMEOUT) {
        Ok(attempt) => Some(attempt),
        Err(ForgeCredentialError::ReconnectRecordMissing) => None,
        Err(ForgeCredentialError::CapabilityBusy) => {
            eprintln!(
                "Remote Forge connection refused: another Editor on this machine holds this host's reconnect lease"
            );
            return Err(StartupError::ConnectionBusy);
        }
        Err(error) => {
            return Err(credential_failure(
                "checking out the reconnect capability",
                &error,
            ));
        }
    };
    let credential = match attempt.as_mut() {
        Some(attempt) => {
            HelloCredential::Reconnect(attempt.take_credential().map_err(|error| {
                credential_failure("taking the stored reconnect credential", &error)
            })?)
        }
        None => HelloCredential::Initial(host.capability().map_err(|error| {
            credential_failure("reading the initial capability from the invitation", &error)
        })?),
    };
    let mut frames = FrameFactory::new();
    let stamp = frames.next()?;
    let hello = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: stamp.frame_id,
        sent_at: stamp.sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1])
                .map_err(|error| credential_failure("building the version offer", &error))?,
            credential,
            supports_lifecycle_control: false,
        }),
    };
    let limits = ClientSessionLimits {
        connect: Duration::from_secs(5),
        handshake: Duration::from_secs(5),
        request: Duration::from_secs(30),
        shutdown: Duration::from_secs(10),
        admission_budget: 65_536,
    };
    let cancel = CancelHandle::new();
    let connected = ClientSession::connect(
        target,
        certificate.clone(),
        pinned_identity,
        hello,
        limits,
        &cancel,
    )
    .await;
    let (session, welcome) = match connected {
        Ok(connected) => connected,
        Err(error) => {
            eprintln!(
                "Remote Forge connection failed (handshake) to {}: {}",
                host.endpoint,
                ErrorChain(&error)
            );
            if let Some(attempt) = attempt
                && let Err(error) = attempt.quarantine()
            {
                eprintln!(
                    "Remote Forge reconnect credential could not be quarantined after the failed handshake: {}",
                    ErrorChain(&error)
                );
            }
            return Err(StartupError::Stage(ServiceFailureStage::Handshake));
        }
    };
    let lease = match attempt {
        Some(attempt) => attempt.publish_next(binding, welcome.welcome.reconnect_capability),
        None => store.initialize_owner_lease(
            binding,
            welcome.welcome.reconnect_capability,
            RECONNECT_LOCK_TIMEOUT,
        ),
    }
    .map_err(|error| credential_failure("storing the next reconnect capability", &error))?;
    Ok((
        ServiceRuntime {
            session: Some(session),
            reconnect_lease: Some(lease),
            reconnect_binding: binding,
            certificate,
            target,
            pinned_identity,
            limits,
            preserve_reconnect: true,
            cancel,
            known_threads: HashSet::new(),
            intake: IntakeState::new(),
            custody: SubscriptionCustody::new(),
            delivery_cancel: None,
            delivery_join: None,
            delivery_tx: None,
            deliveries: DeliveryInbox::default(),
            resolved_home: Some(resolved_home),
        },
        frames,
    ))
}
