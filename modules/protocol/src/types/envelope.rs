//! Response payloads, protocol failures, and the owned wire envelope.

#![forbid(unsafe_code)]

use std::fmt;

use artisan_domain::{
    ConversationSnapshot, DirectoryListing, Event, ImageAttachmentRef, PatchBatch, ProjectListing,
    ProjectSummary, RequestId, ThreadListing, ThreadSummary, UnixMillis,
};

use crate::repository::ProjectRepositoryQueryResult;

use super::ProtocolValueError;
use super::catalog::{
    CatalogSnapshotWire, ComposerCatalogResult, ModelFavoritesSnapshot,
    RegisteredEngineProfilesResult, RichLinkPageMetadata, SetModelFavoriteReceipt,
    ThreadEngineSettingsResult,
};
use super::dispatch::{
    ActiveRunResult, ClientRequest, ConversationSubscriptionStarted,
    ConversationSubscriptionStopped, DirectoryPickOutcome, FirstMessageReceipt,
    QueueMessageReceipt, RespondApprovalReceipt, RespondQuestionReceipt,
    SetThreadEngineConfigResult, StopRunReceipt,
};
use super::handshake::{
    ErrorDetail, EventCursor, FrameId, Hello, LifecycleResponse, ProtocolVersion, Welcome,
};
/// Successful first-workflow response payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponsePayload {
    /// Forge-visible directory listing.
    DirectoryListing(DirectoryListing),
    /// Complete bounded catalog of attached projects.
    ProjectListing(ProjectListing),
    /// Idempotent project attachment result.
    AttachedProject {
        /// Original durable project summary.
        project: ProjectSummary,
        /// Accepted now or replayed.
        disposition: artisan_domain::ReceiptDisposition,
    },
    /// Project-scoped thread listing.
    ThreadListing(ThreadListing),
    /// Idempotent thread creation result.
    CreatedThread {
        /// Original durable thread summary.
        thread: ThreadSummary,
        /// Accepted now or replayed.
        disposition: artisan_domain::ReceiptDisposition,
    },
    /// Durable first-message receipt.
    FirstMessageQueued(FirstMessageReceipt),
    /// Durable general text/image message receipt.
    MessageQueued(QueueMessageReceipt),
    /// One authenticated image read result.
    MessageImage(MessageImageResult),
    /// Correlated exact-run cancellation signal result.
    RunStopped(StopRunReceipt),
    /// Correlated approval response result with its target outcome.
    ApprovalResponse(RespondApprovalReceipt),
    /// Correlated question response result with its target outcome.
    QuestionResponse(RespondQuestionReceipt),
    /// Authoritative live-run query result.
    ActiveRun(ActiveRunResult),
    /// Complete bounded conversation projection.
    ConversationSnapshot(ConversationSnapshot),
    /// Fresh snapshot-first or resumed subscription acknowledgement.
    ConversationSubscriptionStarted(ConversationSubscriptionStarted),
    /// Clean conversation subscription stop acknowledgement.
    ConversationSubscriptionStopped(ConversationSubscriptionStopped),
    /// Outcome of one explicit native directory-picker interaction.
    DirectoryPicked(DirectoryPickOutcome),
    /// Negotiated native lifecycle status or stop result.
    Lifecycle(LifecycleResponse),
    /// Durable thread engine-configuration result.
    ThreadEngineConfigSet(SetThreadEngineConfigResult),
    /// Authoritative persisted thread engine settings.
    ThreadEngineSettings(ThreadEngineSettingsResult),
    /// Registered engine profile catalogue with absence semantics.
    RegisteredEngineProfiles(RegisteredEngineProfilesResult),
    /// Runtime model catalog for one authenticated thread/profile scope.
    ComposerCatalog(ComposerCatalogResult),
    /// Scope-free host catalog with the Forge's readiness applied.
    HostCatalog(CatalogSnapshotWire),
    /// A model selection resolved into a configuration, or refused.
    ModelSelectionResolved(artisan_domain::ModelSelectionResolution),
    /// A manual configuration document built into a configuration, or
    /// refused.
    EngineConfigurationResolved(artisan_domain::EngineConfigurationResolution),
    /// Complete durable model-favorites projection.
    ModelFavorites(ModelFavoritesSnapshot),
    /// Correlated favorite mutation receipt with complete post-state.
    ModelFavoriteSet(SetModelFavoriteReceipt),
    QueuedMessages(artisan_domain::QueuedMessageListing),
    /// Terminally failed dispatches for one thread, newest failures first.
    FailedMessages(artisan_domain::FailedMessageListing),
    MessageWithdrawn(artisan_domain::composer_state::QueuedMessageWithdrawalResult),
    RecalledMessage(artisan_domain::RecalledMessageResult),
    RunUsage(artisan_domain::RunUsageResult),
    /// Provider-account usage snapshot for the requested engines.
    AccountUsage(artisan_domain::EngineUsageSnapshot),
    /// Resolved rich-link page metadata for one requested URL.
    RichLink(RichLinkPageMetadata),
    /// Repository identity per requested project.
    ProjectRepository(ProjectRepositoryQueryResult),
    /// Correlated draft-save acknowledgement with the stored revision.
    ComposerDraftSaved(artisan_domain::ComposerDraftSaved),
    /// Stored draft of one composer scope.
    ComposerDraft(artisan_domain::ComposerDraftResult),
    /// Correlated stored-attachment reference.
    ComposerAttachmentUploaded(artisan_domain::ComposerAttachmentUploaded),
    /// Bytes of one stored attachment.
    ComposerAttachment(artisan_domain::ComposerAttachmentResult),
    /// Correlated answer to a failed-message retry.
    FailedMessageRetried(artisan_domain::FailedMessageRetried),
    /// Correlated answer to a failed-message recovery into a new thread.
    FailedMessageRecovered(artisan_domain::FailedMessageRecovered),
    /// Correlated answer to a draft submission.
    ComposerDraftSubmitted(artisan_domain::ComposerDraftSubmitted),
    /// The Forge user's preferences, after a read or a recorded navigation.
    UserPreferences(artisan_domain::UserPreferences),
    /// The Forge's answer to a one-time legacy preference import.
    LegacyPreferencesImported(artisan_domain::LegacyPreferencesImported),
}

/// Successful response correlated to a client request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerResponse {
    /// Triggering client request identity.
    pub request_id: RequestId,
    /// Successful result.
    pub payload: ResponsePayload,
}

/// One bounded authenticated image read returned on demand.
#[derive(Clone, Eq, PartialEq)]
pub struct MessageImageResult {
    /// Byte-free ownership and integrity metadata.
    pub reference: ImageAttachmentRef,
    /// Original encoded image bytes, bounded to one image.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for MessageImageResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MessageImageResult")
            .field("reference", &self.reference)
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}

/// Durable Forge-originated event with its connection replay sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerEvent {
    /// One-based sequence used to detect duplicate, missing, or regressed events.
    pub cursor: EventCursor,
    /// Domain event delivered at this sequence.
    pub event: Event,
}

/// Stable protocol rejection classification.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ErrorCode {
    /// No mutually supported protocol revision exists.
    UnsupportedVersion,
    /// One field violated its documented validation rule.
    InvalidInput,
    /// Opaque directory identity is unknown or stale.
    DirectoryUnknown,
    /// Attached project does not exist.
    ProjectUnknown,
    /// Thread does not exist.
    ThreadUnknown,
    /// Forge failed internally; retry may later succeed.
    Internal,
    /// The same stable request identity was previously accepted for a
    /// different command kind or immutable payload. The originally accepted
    /// outcome stands, and repeating the conflicting request is never
    /// retryable.
    IdempotencyConflict,
    /// The peer requested lifecycle control without a negotiated capability.
    UnsupportedFeature,
    /// Lifecycle control cannot be accepted while lifecycle work is busy.
    LifecycleBusy,
    /// Thread engine configuration revision was stale.
    EngineConfigConflict,
}

/// Typed application-protocol rejection or failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolFailure {
    /// Stable classification.
    pub code: ErrorCode,
    /// Bounded human-readable evidence.
    pub detail: ErrorDetail,
    /// Whether repeating the identical request later may succeed.
    pub retryable: bool,
    /// Triggering request, or `None` for connection/hello failures.
    pub request_id: Option<RequestId>,
}

/// Failure settling exactly one dispatched client request.
///
/// The wire keeps the general [`ProtocolFailure`] shape because hello-time
/// version rejections legitimately implicate no request. A received client
/// request, however, must always be settled by a failure that names it: this
/// owned value makes that correlation mandatory. The triggering
/// [`RequestId`] is derived from the settled frame itself rather than passed
/// separately, so a dispatch failure can never disagree with the request it
/// answers, the fields stay private so the correlation cannot be mutated
/// afterwards, and conversion into [`ProtocolFailure`] always selects the
/// correlated arm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchFailure {
    code: ErrorCode,
    detail: ErrorDetail,
    retryable: bool,
    request_id: RequestId,
}

impl DispatchFailure {
    /// Builds the rejection that settles the client request carried by
    /// `frame`.
    ///
    /// The correlation identity comes from the frame's own client-minted id
    /// -- the same id every conforming response echoes -- never from a
    /// second argument that could drift from the settled request. Returns
    /// [`None`] when the frame carries no client request body (hello,
    /// welcome, response, event, protocol error, patch batch): those
    /// failures stay uncorrelated on the wire.
    #[must_use]
    pub fn settling(
        frame: &WireEnvelope,
        code: ErrorCode,
        detail: ErrorDetail,
        retryable: bool,
    ) -> Option<Self> {
        if !matches!(frame.body, WireEnvelopeBody::Request(_)) {
            return None;
        }
        let request_id = frame.frame_id.to_request_id().ok()?;
        Some(Self {
            code,
            detail,
            retryable,
            request_id,
        })
    }

    /// Stable classification of the failure.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Bounded human-readable evidence.
    #[must_use]
    pub const fn detail(&self) -> &ErrorDetail {
        &self.detail
    }

    /// Whether repeating the identical request later may succeed.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    /// Mandatory triggering request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }
}

impl From<DispatchFailure> for ProtocolFailure {
    fn from(value: DispatchFailure) -> Self {
        Self {
            code: value.code,
            detail: value.detail,
            retryable: value.retryable,
            request_id: Some(value.request_id),
        }
    }
}

/// Owned application frame body.
#[derive(PartialEq)]
pub enum WireEnvelopeBody {
    /// Authenticated client negotiation offer.
    Hello(Hello),
    /// Successful server negotiation answer.
    Welcome(Welcome),
    /// First-workflow client request.
    Request(ClientRequest),
    /// Successful correlated server response.
    Response(ServerResponse),
    /// Durable Forge-originated event with its replay sequence.
    Event(ServerEvent),
    /// Typed rejection or failure.
    ProtocolError(ProtocolFailure),
    /// Contiguous conversation replay after a known cursor.
    PatchBatch(PatchBatch),
}

/// One fully owned application-protocol frame.
#[derive(PartialEq)]
pub struct WireEnvelope {
    /// Revision stamped on this frame.
    pub protocol_version: ProtocolVersion,
    /// Sender-minted frame identity.
    pub frame_id: FrameId,
    /// Sender timestamp as signed Unix epoch milliseconds.
    pub sent_at: UnixMillis,
    /// Exactly one message family.
    pub body: WireEnvelopeBody,
}

impl WireEnvelope {
    /// Validates command/frame request-correlation invariants.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::RequestCorrelationMismatch`] when an
    /// idempotent mutation's domain request id differs from its frame id.
    pub fn validate_correlation(&self) -> Result<(), ProtocolValueError> {
        match &self.body {
            WireEnvelopeBody::Request(ClientRequest::Command(command))
                if command.request_id().as_str() != self.frame_id.as_str() =>
            {
                Err(ProtocolValueError::RequestCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::FirstMessageQueued(receipt),
            }) if request_id != &receipt.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::MessageQueued(receipt),
            }) if request_id != &receipt.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::RunStopped(receipt),
            }) if request_id != &receipt.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::ApprovalResponse(receipt),
            }) if request_id != &receipt.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::QuestionResponse(receipt),
            }) if request_id != &receipt.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::ThreadEngineConfigSet(result),
            }) if request_id != &result.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::ModelFavoriteSet(receipt),
            }) if request_id != &receipt.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::ComposerDraftSaved(saved),
            }) if request_id != &saved.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::ComposerAttachmentUploaded(uploaded),
            }) if request_id != &uploaded.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::FailedMessageRetried(retried),
            }) if request_id != &retried.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::FailedMessageRecovered(recovered),
            }) if request_id != &recovered.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            WireEnvelopeBody::Response(ServerResponse {
                request_id,
                payload: ResponsePayload::ComposerDraftSubmitted(submitted),
            }) if request_id != &submitted.request_id => {
                Err(ProtocolValueError::ResponseCorrelationMismatch)
            }
            _ => Ok(()),
        }
    }
}
