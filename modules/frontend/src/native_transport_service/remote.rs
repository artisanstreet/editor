//! External Forge startup. No process lease is acquired and remote shutdown is never requested.
use super::*;
use artisan_editor_cli::credentials::{ForgeCredentialError, hosts};
use artisan_transport::SessionTarget;

fn credential_failure(step: &str, error: &ForgeCredentialError) -> StartupError {
    eprintln!("Remote Forge {step}: {error}");
    StartupError::Stage(ServiceFailureStage::Credentials)
}

pub(super) async fn start(home: &Path) -> Result<(ServiceRuntime, FrameFactory), StartupError> {
    let failed = || StartupError::Stage(ServiceFailureStage::Credentials);
    let resolved_home = crate::native_hosts::resolve_home(home)
        .map_err(|error| credential_failure("invitation refresh", &error))?;
    let home = resolved_home.as_path();
    let bytes = hosts::read_private(home, "host.json").map_err(|_| failed())?;
    let host = hosts::HostInvitation::decode(&bytes).map_err(|_| failed())?;
    let certificate = CertificateDer::from(host.certificate_der().map_err(|_| failed())?);
    let pinned_identity = PinnedIdentity::from_certificate(&certificate);
    let target = SessionTarget::remote(host.endpoint).map_err(|_| failed())?;
    let binding = ReconnectBinding::new(
        host.incarnation,
        host.endpoint.port(),
        *pinned_identity.as_bytes(),
        NonZeroU32::new(host.pid).ok_or_else(failed)?,
    )
    .map_err(|_| failed())?;
    let store = ReconnectCapabilityStore::from_home(home).map_err(|_| failed())?;
    let mut attempt = match store.checkout(binding, RECONNECT_LOCK_TIMEOUT) {
        Ok(attempt) => Some(attempt),
        Err(ForgeCredentialError::ReconnectRecordMissing) => None,
        Err(ForgeCredentialError::CapabilityBusy) => return Err(StartupError::ConnectionBusy),
        Err(error) => return Err(credential_failure("reconnect checkout", &error)),
    };
    let credential = match attempt.as_mut() {
        Some(attempt) => {
            HelloCredential::Reconnect(attempt.take_credential().map_err(|_| failed())?)
        }
        None => HelloCredential::Initial(host.capability().map_err(|_| failed())?),
    };
    let mut frames = FrameFactory::new();
    let stamp = frames.next()?;
    let hello = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: stamp.frame_id,
        sent_at: stamp.sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![1]).map_err(|_| failed())?,
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
    let Ok((session, welcome)) = connected else {
        if let Some(attempt) = attempt {
            let _ = attempt.quarantine();
        }
        return Err(StartupError::Stage(ServiceFailureStage::Handshake));
    };
    let lease = match attempt {
        Some(attempt) => attempt.publish_next(binding, welcome.welcome.reconnect_capability),
        None => store.initialize_owner_lease(
            binding,
            welcome.welcome.reconnect_capability,
            RECONNECT_LOCK_TIMEOUT,
        ),
    }
    .map_err(|_| failed())?;
    Ok((
        ServiceRuntime {
            session: Some(session),
            reconnect_lease: Some(lease),
            reconnect_binding: binding,
            certificate,
            target,
            pinned_identity,
            limits,
            lease: None,
            preserve_reconnect: true,
            cancel,
            shutdown_grace: limits.shutdown,
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
