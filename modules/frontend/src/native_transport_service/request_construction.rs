//! Frame minting, request envelopes, and stable mutations over the
//! authenticated transport.
//!
//! The parent `native_transport_service` remains the owner of the session,
//! runtime custody, and command loop. Root mounts this file as a child module
//! so request construction stays a session-free, pure seam.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::*;

/// A real service frame identity and timestamp.
pub(super) struct FrameStamp {
    pub(super) frame_id: FrameId,
    pub(super) sent_at: UnixMillis,
}

/// One durable mutation and its exact wire identity.
///
/// This value is deliberately service-private and implements neither
/// `Debug` nor `Display`: a stable command may contain opaque routing values
/// that must never reach a log or the application bridge.
pub(super) struct StableMutation {
    pub(super) frame_id: FrameId,
    pub(super) sent_at: UnixMillis,
    pub(super) command: Command,
}

impl StableMutation {
    pub(super) fn envelope(
        &self,
        protocol_version: ProtocolVersion,
    ) -> Result<(WireEnvelope, RequestId), ServiceFailure> {
        let request_id = self
            .frame_id
            .to_request_id()
            .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
        if self.command.request_id() != &request_id {
            return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
        }
        let envelope = WireEnvelope {
            protocol_version,
            frame_id: self.frame_id.clone(),
            sent_at: self.sent_at,
            body: WireEnvelopeBody::Request(ClientRequest::Command(self.command.clone())),
        };
        envelope
            .validate_correlation()
            .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
        Ok((envelope, request_id))
    }
}

/// Service-private identity minting. The checked monotonic counter makes
/// identities unique even when the clock does not advance.
pub(super) struct FrameFactory {
    process_id: u32,
    counter: u64,
}

impl FrameFactory {
    pub(super) fn new() -> Self {
        Self {
            process_id: std::process::id(),
            counter: 0,
        }
    }

    pub(super) fn next(&mut self) -> Result<FrameStamp, StartupError> {
        let counter = self
            .counter
            .checked_add(1)
            .ok_or(StartupError::Stage(ServiceFailureStage::Request))?;
        self.counter = counter;
        let sent_at = real_unix_millis()?;
        let text = format!(
            "native-{}-{}-{}",
            self.process_id,
            sent_at.as_millis(),
            counter
        );
        let frame_id =
            FrameId::parse(text).map_err(|_| StartupError::Stage(ServiceFailureStage::Request))?;
        frame_id
            .to_request_id()
            .map_err(|_| StartupError::Stage(ServiceFailureStage::Request))?;
        Ok(FrameStamp { frame_id, sent_at })
    }
}

pub(super) fn real_unix_millis() -> Result<UnixMillis, StartupError> {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let millis = i64::try_from(duration.as_millis())
                .map_err(|_| StartupError::Stage(ServiceFailureStage::Request))?;
            Ok(UnixMillis::from_millis(millis))
        }
        Err(error) => {
            let millis = i64::try_from(error.duration().as_millis())
                .map_err(|_| StartupError::Stage(ServiceFailureStage::Request))?;
            Ok(UnixMillis::from_millis(millis.saturating_neg()))
        }
    }
}

pub(super) fn finite_duration(milliseconds: u64) -> Result<Duration, StartupError> {
    if milliseconds == 0 {
        return Err(StartupError::Stage(ServiceFailureStage::Instance));
    }
    Ok(Duration::from_millis(milliseconds))
}

fn build_snapshot_query(thread_id: ThreadId) -> Result<ConversationRequest, ServiceFailure> {
    let maximum_turn_count = QueryTurnCount::new(u64::from(CONVERSATION_QUERY_MAX_TURNS))
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(ConversationRequest::Query(ConversationQuery {
        thread_id,
        bounds: ConversationQueryBounds::Window { maximum_turn_count },
    }))
}

pub(super) fn query_request(request: Query) -> ClientRequest {
    ClientRequest::Query(request)
}

pub(super) fn project_request() -> ClientRequest {
    query_request(Query::ListAttachedProjects(ListAttachedProjects))
}

pub(super) fn threads_request(project_id: ProjectId) -> ClientRequest {
    query_request(Query::ListProjectThreads(ListProjectThreads { project_id }))
}

pub(super) fn snapshot_request(thread_id: ThreadId) -> Result<ClientRequest, ServiceFailure> {
    Ok(ClientRequest::Conversation(build_snapshot_query(
        thread_id,
    )?))
}

pub(super) fn thread_engine_settings_request(thread_id: ThreadId) -> ClientRequest {
    query_request(Query::ReadThreadEngineSettings(
        ReadThreadEngineSettings::new(thread_id),
    ))
}

pub(super) fn registered_profiles_request() -> ClientRequest {
    query_request(Query::ListRegisteredEngineProfiles(
        ListRegisteredEngineProfiles,
    ))
}

pub(super) fn composer_catalog_request(
    thread_id: ThreadId,
    profile_id: artisan_domain::EngineProfileId,
) -> ClientRequest {
    query_request(Query::ReadComposerCatalog(ReadComposerCatalog::new(
        thread_id, profile_id,
    )))
}

/// Builds one bounded rich-link resolve request plus its exact echoed URL.
///
/// The fragment is removed before sending because Forge's canonical
/// resolution key and echoed `requestedUrl` both drop it; keeping the exact
/// URL here lets the response filter prove correlation.
pub(super) fn rich_link_request(url: &str) -> Result<(ClientRequest, String), ServiceFailure> {
    let invalid = || ServiceFailure::invalid(ServiceFailureStage::Request);
    let url = crate::rich_link_url::rich_link_metadata_url(Some(url)).ok_or_else(invalid)?;
    let mut parsed = url::Url::parse(&url).map_err(|_| invalid())?;
    parsed.set_fragment(None);
    let canonical = parsed.as_str().to_owned();
    let request = ResolveRichLinkRequest::new(canonical.clone()).map_err(|_| invalid())?;
    Ok((ClientRequest::ResolveRichLink(request), canonical))
}

/// Builds one bounded repository-identity query for a single project.
pub(super) fn project_repository_request(project_id: ProjectId) -> ClientRequest {
    ClientRequest::QueryProjectRepository(
        ProjectRepositoryQuery::new(vec![project_id])
            .expect("one project identifier is within the query bound"),
    )
}

pub(super) fn model_favorites_request() -> ClientRequest {
    query_request(Query::ReadModelFavorites(ReadModelFavorites))
}

pub(super) fn account_usage_request(
    engine_id: &str,
    force: bool,
) -> Result<ClientRequest, ServiceFailure> {
    let query = artisan_domain::ReadAccountUsage::new(Some(engine_id.to_owned()), force)
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(query_request(Query::ReadAccountUsage(query)))
}

pub(super) fn engine_config_stable_mutation(
    command: Box<SetThreadEngineConfig>,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = command.request_id().clone();
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if &request_id != command.request_id() {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command: Command::SetThreadEngineConfig(command),
    })
}

pub(super) fn model_favorite_stable_mutation(
    command: SetModelFavorite,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = command.request_id.clone();
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let frame_request_id = frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if frame_request_id != request_id || command.request_id != request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command: Command::SetModelFavorite(command),
    })
}

pub(super) fn first_message_stable_mutation(
    command: QueueFirstMessage,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = command.request_id.clone();
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let frame_request_id = frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if frame_request_id != request_id || command.request_id != request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command: Command::QueueFirstMessage(command),
    })
}

pub(super) fn message_stable_mutation(
    command: QueueMessage,
    stored_attachments: &HashSet<artisan_domain::ComposerAttachmentDigest>,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = command.request_id.clone();
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let frame_request_id = frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if frame_request_id != request_id || command.request_id != request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command: super::composer_draft_operations::message_command(command, stored_attachments),
    })
}

pub(super) fn approval_stable_mutation(
    command: RespondApproval,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = command.request_id().clone();
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let frame_request_id = frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if frame_request_id != request_id || command.request_id() != &request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command: Command::RespondApproval(command),
    })
}

pub(super) fn question_stable_mutation(
    command: RespondQuestion,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = command.request_id().clone();
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let frame_request_id = frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if frame_request_id != request_id || command.request_id() != &request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command: Command::RespondQuestion(command),
    })
}

pub(super) fn make_request_frame(
    frames: &mut FrameFactory,
    protocol_version: ProtocolVersion,
    request: ClientRequest,
) -> Result<(WireEnvelope, RequestId), ServiceFailure> {
    let stamp = frames
        .next()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let request_id = stamp
        .frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let envelope = WireEnvelope {
        protocol_version,
        frame_id: stamp.frame_id,
        sent_at: stamp.sent_at,
        body: WireEnvelopeBody::Request(request),
    };
    envelope
        .validate_correlation()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok((envelope, request_id))
}

fn stable_mutation_from_stamp(
    stamp: FrameStamp,
    command: Command,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = stamp
        .frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if command.request_id() != &request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    Ok(StableMutation {
        frame_id: stamp.frame_id,
        sent_at: stamp.sent_at,
        command,
    })
}

pub(super) fn attach_mutation(
    frames: &mut FrameFactory,
    directory_id: DirectoryId,
) -> Result<StableMutation, ServiceFailure> {
    let stamp = frames
        .next()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let request_id = stamp
        .frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    stable_mutation_from_stamp(
        stamp,
        Command::AttachProject(AttachProject {
            request_id,
            directory_id,
        }),
    )
}

pub(super) fn create_mutation(
    frames: &mut FrameFactory,
    project_id: ProjectId,
    title: ThreadTitle,
) -> Result<StableMutation, ServiceFailure> {
    let stamp = frames
        .next()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let request_id = stamp
        .frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    stable_mutation_from_stamp(
        stamp,
        Command::CreateThread(CreateThread {
            request_id,
            project_id,
            title,
        }),
    )
}

struct ReconnectHelloHeader {
    frame_id: FrameId,
    sent_at: UnixMillis,
    supported_versions: VersionOffer,
}

fn reconnect_hello_header(
    frames: &mut FrameFactory,
) -> Result<ReconnectHelloHeader, ServiceFailure> {
    let stamp = frames
        .next()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Handshake))?;
    let supported_versions = VersionOffer::new(vec![1])
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Handshake))?;
    Ok(ReconnectHelloHeader {
        frame_id: stamp.frame_id,
        sent_at: stamp.sent_at,
        supported_versions,
    })
}

fn reconnect_hello_from_header(
    header: ReconnectHelloHeader,
    capability: artisan_protocol::ReconnectCapability,
) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: header.frame_id,
        sent_at: header.sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: header.supported_versions,
            credential: HelloCredential::Reconnect(capability),
            supports_lifecycle_control: false,
        }),
    }
}

pub(super) fn reconnect_hello_with_capability(
    frames: &mut FrameFactory,
    capability: artisan_protocol::ReconnectCapability,
) -> Result<WireEnvelope, (ServiceFailure, artisan_protocol::ReconnectCapability)> {
    let header = match reconnect_hello_header(frames) {
        Ok(header) => header,
        Err(failure) => return Err((failure, capability)),
    };
    Ok(reconnect_hello_from_header(header, capability))
}

#[cfg(test)]
pub(super) fn reconnect_hello(
    frames: &mut FrameFactory,
    capability: artisan_protocol::ReconnectCapability,
) -> Result<WireEnvelope, ServiceFailure> {
    reconnect_hello_with_capability(frames, capability).map_err(|(failure, _)| failure)
}

pub(super) fn build_reconnect_binding(
    instance_id: [u8; 16],
    target: LoopbackTarget,
    pinned_identity: PinnedIdentity,
    forge_pid: u32,
) -> Result<ReconnectBinding, StartupError> {
    ReconnectBinding::new(
        instance_id,
        target.addr().port(),
        *pinned_identity.as_bytes(),
        NonZeroU32::new(forge_pid).ok_or(StartupError::Stage(ServiceFailureStage::Readiness))?,
    )
    .map_err(|_| StartupError::Stage(ServiceFailureStage::Instance))
}
