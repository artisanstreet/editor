//! Exact response-family validation and request-id correlation for the
//! native transport service.
//!
//! The parent `native_transport_service` remains the owner of the session,
//! runtime custody, and command loop. Root mounts this file as a child module
//! so each accepted response shape stays in one exhaustive, reviewable table.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::*;

#[derive(Clone)]
pub(super) enum ExpectedResponse {
    MessageWithdrawn {
        thread_id: ThreadId,
        message_id: artisan_domain::MessageId,
        original_request_id: RequestId,
        request_id: RequestId,
    },
    FailedMessageRetried {
        request_id: RequestId,
    },
    FailedMessageRecovered {
        request_id: RequestId,
    },
    RunUsage {
        thread_id: ThreadId,
        run_id: artisan_domain::RunId,
    },

    ActiveRun(ThreadId),
    RunStopped(artisan_domain::StopRun),
    Directory,
    Projects,
    AttachedProject,
    CreatedThread,
    Threads(ProjectId),
    Snapshot(ThreadId),
    MessageImage(artisan_domain::ImageAttachmentRef),
    ThreadEngineSettings(ThreadId),
    RegisteredProfiles,
    AccountUsage {
        engine_id: String,
    },
    ComposerCatalog {
        thread_id: ThreadId,
        profile_id: artisan_domain::EngineProfileId,
    },
    RichLink {
        requested_url: String,
    },
    ProjectRepository {
        project_id: ProjectId,
    },
    ModelFavorites,
    ModelFavoriteSet {
        request_id: RequestId,
        model_id: artisan_domain::ModelFavoriteId,
        favorite: bool,
    },
    ThreadEngineConfigSet {
        thread_id: ThreadId,
        request_id: RequestId,
    },
    FirstMessageQueued {
        thread_id: ThreadId,
        request_id: RequestId,
    },
    DraftSubmitted {
        thread_id: ThreadId,
        request_id: RequestId,
    },
    ApprovalAnswered {
        thread_id: ThreadId,
        request_id: RequestId,
    },
    QuestionAnswered {
        thread_id: ThreadId,
        request_id: RequestId,
    },
    ConversationSubscriptionStarted {
        thread_id: ThreadId,
    },
    ConversationSubscriptionStopped {
        thread_id: ThreadId,
    },
    ComposerDraft(super::composer_draft_operations::ComposerDraftExpectation),
    ForgeDecision(super::forge_decision_operations::ForgeDecisionExpectation),
    Preferences(super::preferences_operations::PreferencesExpectation),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ThreadSelectionDecision {
    AwaitHostSnapshot(ThreadId),
    Empty,
}

pub(super) fn thread_selection_decision(listing: &ThreadListing) -> ThreadSelectionDecision {
    listing
        .threads()
        .first()
        .map_or(ThreadSelectionDecision::Empty, |thread| {
            ThreadSelectionDecision::AwaitHostSnapshot(thread.thread_id.clone())
        })
}

pub(super) fn request_id_matches(expected: &RequestId, actual: &RequestId) -> bool {
    expected == actual
}

pub(super) fn optional_request_id_matches(
    expected: &RequestId,
    actual: Option<&RequestId>,
) -> bool {
    actual == Some(expected)
}

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive response-family table keeps each request's accepted payload shapes in a single reviewable match"
)]
pub(super) fn validate_response_family(
    expected: ExpectedResponse,
    payload: ResponsePayload,
) -> Result<ResponsePayload, ServiceFailure> {
    match (expected, payload) {
        (
            ExpectedResponse::MessageWithdrawn {
                thread_id,
                message_id,
                original_request_id,
                request_id,
            },
            ResponsePayload::MessageWithdrawn(value),
        ) if value.thread_id == thread_id
            && value.message_id == message_id
            && value.original_request_id == original_request_id
            && value.withdrawal_request_id() == &request_id =>
        {
            Ok(ResponsePayload::MessageWithdrawn(value))
        }
        (
            ExpectedResponse::FailedMessageRetried { request_id },
            ResponsePayload::FailedMessageRetried(value),
        ) if value.request_id == request_id => Ok(ResponsePayload::FailedMessageRetried(value)),
        (
            ExpectedResponse::FailedMessageRecovered { request_id },
            ResponsePayload::FailedMessageRecovered(value),
        ) if value.request_id == request_id => Ok(ResponsePayload::FailedMessageRecovered(value)),
        (ExpectedResponse::RunUsage { thread_id, run_id }, ResponsePayload::RunUsage(value))
            if value.thread_id == thread_id && value.run_id == run_id =>
        {
            Ok(ResponsePayload::RunUsage(value))
        }

        (ExpectedResponse::ComposerDraft(expected), payload) if expected.accepts(&payload) => {
            Ok(payload)
        }
        (ExpectedResponse::ForgeDecision(expected), payload) if expected.accepts(&payload) => {
            Ok(payload)
        }
        (ExpectedResponse::Preferences(expected), payload) if expected.accepts(&payload) => {
            Ok(payload)
        }
        (ExpectedResponse::Directory, ResponsePayload::DirectoryPicked(outcome)) => {
            Ok(ResponsePayload::DirectoryPicked(outcome))
        }
        (ExpectedResponse::Projects, ResponsePayload::ProjectListing(listing)) => {
            Ok(ResponsePayload::ProjectListing(listing))
        }
        (
            ExpectedResponse::AttachedProject,
            ResponsePayload::AttachedProject {
                project,
                disposition,
            },
        ) => Ok(ResponsePayload::AttachedProject {
            project,
            disposition,
        }),
        (
            ExpectedResponse::CreatedThread,
            ResponsePayload::CreatedThread {
                thread,
                disposition,
            },
        ) => Ok(ResponsePayload::CreatedThread {
            thread,
            disposition,
        }),
        (ExpectedResponse::Threads(project_id), ResponsePayload::ThreadListing(listing)) => {
            if listing
                .threads()
                .iter()
                .any(|thread| thread.project_id != project_id)
            {
                return Err(ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ));
            }
            Ok(ResponsePayload::ThreadListing(listing))
        }
        (
            ExpectedResponse::Snapshot(thread_id),
            ResponsePayload::ConversationSnapshot(snapshot),
        ) if snapshot.thread_id() == &thread_id => {
            Ok(ResponsePayload::ConversationSnapshot(snapshot))
        }
        (ExpectedResponse::ActiveRun(expected), ResponsePayload::ActiveRun(result))
            if match &result {
                artisan_protocol::ActiveRunResult::NoActive { thread_id }
                | artisan_protocol::ActiveRunResult::Active { thread_id, .. } => {
                    thread_id == &expected
                }
            } =>
        {
            Ok(ResponsePayload::ActiveRun(result))
        }
        (ExpectedResponse::RunStopped(expected), ResponsePayload::RunStopped(receipt))
            if receipt.request_id == expected.request_id
                && receipt.thread_id == expected.thread_id
                && receipt.run_id == expected.run_id =>
        {
            Ok(ResponsePayload::RunStopped(receipt))
        }
        (ExpectedResponse::MessageImage(expected), ResponsePayload::MessageImage(result))
            if result.reference == expected
                && result.bytes.len() == expected.size_bytes as usize
                && Sha256::digest(&result.bytes).as_slice() == expected.digest =>
        {
            Ok(ResponsePayload::MessageImage(result))
        }
        (
            ExpectedResponse::ThreadEngineSettings(thread_id),
            ResponsePayload::ThreadEngineSettings(result),
        ) if result.thread_id() == &thread_id => Ok(ResponsePayload::ThreadEngineSettings(result)),
        (
            ExpectedResponse::RegisteredProfiles,
            ResponsePayload::RegisteredEngineProfiles(result),
        ) => Ok(ResponsePayload::RegisteredEngineProfiles(result)),
        (ExpectedResponse::AccountUsage { engine_id }, ResponsePayload::AccountUsage(snapshot))
            if snapshot.engines().len() == 1
                && snapshot
                    .engines()
                    .first()
                    .is_some_and(|report| report.engine_id() == engine_id) =>
        {
            Ok(ResponsePayload::AccountUsage(snapshot))
        }
        (
            ExpectedResponse::ComposerCatalog {
                thread_id,
                profile_id,
            },
            ResponsePayload::ComposerCatalog(result),
        ) if result.thread_id == thread_id && result.profile_id == profile_id => {
            Ok(ResponsePayload::ComposerCatalog(result))
        }
        (ExpectedResponse::RichLink { requested_url }, ResponsePayload::RichLink(result))
            if result.requested_url == requested_url =>
        {
            Ok(ResponsePayload::RichLink(result))
        }
        (
            ExpectedResponse::ProjectRepository { project_id },
            ResponsePayload::ProjectRepository(result),
        ) if result
            .repositories()
            .iter()
            .any(|entry| entry.project_id() == &project_id) =>
        {
            Ok(ResponsePayload::ProjectRepository(result))
        }
        (ExpectedResponse::ModelFavorites, ResponsePayload::ModelFavorites(result)) => {
            Ok(ResponsePayload::ModelFavorites(result))
        }
        (
            ExpectedResponse::ModelFavoriteSet {
                request_id,
                model_id,
                favorite,
            },
            ResponsePayload::ModelFavoriteSet(receipt),
        ) if receipt.request_id == request_id
            && receipt.model_id == model_id
            && receipt.favorite == favorite =>
        {
            Ok(ResponsePayload::ModelFavoriteSet(receipt))
        }
        (
            ExpectedResponse::ThreadEngineConfigSet {
                thread_id,
                request_id,
            },
            ResponsePayload::ThreadEngineConfigSet(result),
        ) if result.thread_id == thread_id && result.request_id == request_id => {
            Ok(ResponsePayload::ThreadEngineConfigSet(result))
        }
        (
            ExpectedResponse::FirstMessageQueued {
                thread_id,
                request_id,
            },
            ResponsePayload::FirstMessageQueued(receipt),
        ) if receipt.thread_id == thread_id && receipt.request_id == request_id => {
            Ok(ResponsePayload::FirstMessageQueued(receipt))
        }
        (
            ExpectedResponse::DraftSubmitted {
                thread_id,
                request_id,
            },
            ResponsePayload::ComposerDraftSubmitted(submitted),
        ) if submitted.thread_id == thread_id && submitted.request_id == request_id => {
            Ok(ResponsePayload::ComposerDraftSubmitted(submitted))
        }
        (
            ExpectedResponse::ApprovalAnswered {
                thread_id,
                request_id,
            },
            ResponsePayload::ApprovalResponse(receipt),
        ) if receipt.thread_id == thread_id && receipt.request_id == request_id => {
            Ok(ResponsePayload::ApprovalResponse(receipt))
        }
        (
            ExpectedResponse::QuestionAnswered {
                thread_id,
                request_id,
            },
            ResponsePayload::QuestionResponse(receipt),
        ) if receipt.thread_id == thread_id && receipt.request_id == request_id => {
            Ok(ResponsePayload::QuestionResponse(receipt))
        }
        (
            ExpectedResponse::ConversationSubscriptionStarted { thread_id },
            ResponsePayload::ConversationSubscriptionStarted(started),
        ) => {
            let actual_thread = match &started {
                ConversationSubscriptionStarted::Fresh(start) => start.snapshot().thread_id(),
                ConversationSubscriptionStarted::Resumed { thread_id, .. } => thread_id,
            };
            if actual_thread != &thread_id {
                return Err(ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ));
            }
            Ok(ResponsePayload::ConversationSubscriptionStarted(started))
        }
        (
            ExpectedResponse::ConversationSubscriptionStopped { thread_id },
            ResponsePayload::ConversationSubscriptionStopped(stopped),
        ) if stopped.thread_id == thread_id => {
            Ok(ResponsePayload::ConversationSubscriptionStopped(stopped))
        }
        _ => Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        )),
    }
}

/// One validated uni-stream delivery frame, extracted from one envelope.
///
/// Patch batches carry conversation replay; engine observation events carry
/// one committed observation row with its connection replay cursor. Any other
/// envelope family on the delivery stream fails closed as an integrity loss.
#[derive(Clone, Debug, PartialEq)]
pub enum UniDelivery {
    /// Valid uni patch batch.
    Batch(PatchBatch),
    /// Valid uni engine observation event.
    Observation(ServerEvent),
    /// A thread's complete message outbox.
    Outbox(artisan_domain::MessageOutbox),
    /// Connection-scoped state the Forge pushed.
    HostState(HostStateEvent),
}

/// Connection-scoped state the Forge pushes whenever it changes: every
/// engine's usage with its readiness verdict, the user's preferences, and a
/// subscribed thread's display title.
#[derive(Clone, Debug, PartialEq)]
pub enum HostStateEvent {
    /// Every observed engine's usage report and readiness verdict.
    AccountUsage(artisan_domain::EngineUsageSnapshot),
    /// The user's preferences.
    Preferences(artisan_domain::UserPreferences),
    /// A subscribed thread's display title.
    ThreadRetitled(artisan_domain::ThreadRetitled),
    /// A subscribed thread's live run usage.
    RunUsage(artisan_domain::RunUsageResult),
}

/// Validates the delivery family of one uni-stream envelope.
///
/// # Errors
///
/// Returns an integrity failure when the protocol version or envelope body
/// does not match the expected delivery family.
pub fn validate_uni_envelope(
    envelope: &WireEnvelope,
    expected_version: ProtocolVersion,
) -> Result<UniDelivery, ServiceFailure> {
    if envelope.protocol_version != expected_version {
        return Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        ));
    }
    match &envelope.body {
        WireEnvelopeBody::PatchBatch(batch) => Ok(UniDelivery::Batch(batch.clone())),
        WireEnvelopeBody::Event(server_event) => match &server_event.event {
            artisan_domain::Event::EngineObservation(_) => {
                Ok(UniDelivery::Observation(server_event.clone()))
            }
            artisan_domain::Event::MessageOutbox(outbox) => Ok(UniDelivery::Outbox(outbox.clone())),
            artisan_domain::Event::AccountUsage(usage) => Ok(UniDelivery::HostState(
                HostStateEvent::AccountUsage(usage.clone()),
            )),
            artisan_domain::Event::UserPreferences(preferences) => Ok(UniDelivery::HostState(
                HostStateEvent::Preferences(preferences.clone()),
            )),
            artisan_domain::Event::ThreadRetitled(retitled) => Ok(UniDelivery::HostState(
                HostStateEvent::ThreadRetitled(retitled.clone()),
            )),
            artisan_domain::Event::RunUsage(usage) => Ok(UniDelivery::HostState(
                HostStateEvent::RunUsage(usage.clone()),
            )),
            artisan_domain::Event::ProjectAttached(_)
            | artisan_domain::Event::ThreadCreated(_)
            | artisan_domain::Event::FirstMessageQueued(_) => Err(ServiceFailure::new(
                ServiceFailureStage::Delivery,
                ServiceFailureCategory::Integrity,
            )),
        },
        _ => Err(ServiceFailure::new(
            ServiceFailureStage::Delivery,
            ServiceFailureCategory::Integrity,
        )),
    }
}

/// Validates the thread identity in a subscription-start response.
///
/// # Errors
///
/// Returns an integrity failure when the response names a different thread.
pub fn validate_started_correlation(
    expected_thread_id: &ThreadId,
    started: &ConversationSubscriptionStarted,
) -> Result<(), ServiceFailure> {
    let actual_thread = match started {
        ConversationSubscriptionStarted::Fresh(start) => start.snapshot().thread_id(),
        ConversationSubscriptionStarted::Resumed { thread_id, .. } => thread_id,
    };
    if actual_thread != expected_thread_id {
        return Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        ));
    }
    Ok(())
}

/// Validates the thread identity in a subscription-stop response.
///
/// # Errors
///
/// Returns an integrity failure when the response names a different thread.
pub fn validate_stopped_correlation(
    expected_thread_id: &ThreadId,
    stopped: &ConversationSubscriptionStopped,
) -> Result<(), ServiceFailure> {
    if &stopped.thread_id != expected_thread_id {
        return Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        ));
    }
    Ok(())
}
