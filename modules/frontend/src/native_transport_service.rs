//! Native Forge/session ownership for the shipping editor application.
//!
//! The service is deliberately a synchronous bounded bridge around one
//! service thread. The thread owns its Tokio runtime, authenticated session,
//! rotated capability, and owned Forge lease. Only owned domain values and
//! redacted typed diagnostics cross the bridge.

#![forbid(unsafe_code)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::module_name_repetitions)]

use std::{
    collections::HashSet,
    net::SocketAddr,
    num::NonZeroU32,
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TryRecvError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::forge_dev_endpoint as dev_endpoint;
use crate::native_profile_usage::{NativeUsageEntry, ProfileUsageGeneration};
use crate::native_transport::CatalogLoadGeneration;
use artisan_domain::{
    AttachProject, CONVERSATION_QUERY_MAX_TURNS, Command, ConversationCursor, ConversationQuery,
    ConversationQueryBounds, ConversationRequest, ConversationSnapshot, ConversationSubscribe,
    ConversationUnsubscribe, CreateThread, DirectoryId, EngineRunConfig, ListAttachedProjects,
    ListProjectThreads, ListRegisteredEngineProfiles, PatchBatch, ProjectId, ProjectListing,
    ProjectSummary, Query, QueryTurnCount, QueueFirstMessage, ReadComposerCatalog,
    ReadModelFavorites, ReadThreadEngineSettings, RequestId, RespondApproval, RespondQuestion,
    SetModelFavorite, SetThreadEngineConfig, SubmitComposerDraft, ThreadId, ThreadListing,
    ThreadSummary, ThreadTitle, UnixMillis,
};
use artisan_editor_cli::{
    credentials::{
        NativeClientCredentials, RECONNECT_LOCK_TIMEOUT, ReconnectBinding,
        ReconnectCapabilityStore, ReconnectSessionLease, load_client_credentials,
    },
    instance::NativeInstanceConfig,
    manifest::InstallationManifest,
    paths::Layout,
    process::{ForgeLaunchSpec, ForgeProcessLease, ForgeReadiness, start_owned},
};
use artisan_protocol::{
    ClientRequest, ConversationSubscriptionStarted, ConversationSubscriptionStopped, ErrorCode,
    FirstMessageReceipt, FrameId, Hello, HelloCredential, ProjectRepository,
    ProjectRepositoryQuery, ProtocolVersion, QueueMessageReceipt, RegisteredEngineProfilesResult,
    ResolveRichLinkRequest, ResponsePayload, ServerEvent, SetThreadEngineConfigResult,
    ThreadEngineSettingsResult, VersionOffer, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, ClientRequestError, ClientSession, ClientSessionLimits, DeadlineError,
    DeliveryReceiver, EnvelopeReceiveError, EnvelopeSendError, ExchangeError, FrameError,
    LoopbackTarget, PinnedIdentity, RequestOutcome,
};
use rustls_pki_types::CertificateDer;
use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// Maximum number of commands waiting for the service thread.
pub const COMMAND_CAPACITY: usize = 64;

/// Maximum number of events waiting for the application thread.
pub const EVENT_CAPACITY: usize = 64;

/// Application-minted monotonic identity for one authoritative settings read.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SettingsLoadGeneration(u64);

impl SettingsLoadGeneration {
    /// Returns the first valid generation.
    #[must_use]
    pub const fn first() -> Self {
        Self(1)
    }

    /// Returns the next generation, or `None` when the counter is exhausted.
    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    /// Returns the finite generation number for test and correlation checks.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[cfg(test)]
    pub(crate) const fn from_raw_for_test(value: u64) -> Self {
        Self(value)
    }
}

/// Commands accepted by the native service.
#[derive(Clone, Eq, PartialEq)]
pub enum NativeTransportCommand {
    ComposerState(ComposerStateCommand),
    /// Forge-owned composer draft and stored-attachment work.
    ComposerDraft(ComposerDraftCommand),
    /// Business decisions the Forge sends as data.
    ForgeDecision(ForgeDecisionCommand),
    /// Query exact live run ownership, fenced by the application's selection generation.
    ReadActiveRun {
        thread_id: ThreadId,
        generation: u64,
    },
    /// Signal cancellation for an exact thread/run pair.
    StopRun(artisan_domain::StopRun),
    /// Answer one pending approval with an explicit decision.
    ///
    /// The client-minted request identity is preserved end to end and never
    /// re-minted at send; the boxed wrapper follows the `QueueMessage` pattern
    /// for larger command payloads.
    RespondApproval(Box<RespondApproval>),
    /// Answer one pending question with explicit answers; identity rules
    /// match [`Self::RespondApproval`].
    RespondQuestion(Box<RespondQuestion>),
    /// Start a fresh opaque-directory project intake.
    BeginProjectIntake,
    /// A host-native path chosen in the local Windows WSL folder dialog.
    BeginProjectIntakeAt(String),
    /// Continue the one retained retry plan for project intake.
    RetryProjectIntake,
    /// Select an existing Forge-owned project.
    SelectProject(ProjectId),
    /// Refresh the sidebar without selecting a project or mounting a conversation.
    ReadSidebarThreads {
        project_id: ProjectId,
        generation: u64,
    },
    /// Create a new task in an existing, authoritative project.
    CreateTask(ProjectId),
    /// Ask the Forge to move one failed message into a new thread of its
    /// project, then open that thread like a created task.
    RecoverFailedMessage {
        /// Project of the failed message's thread.
        project_id: ProjectId,
        /// Exact durable recovery command.
        command: Box<artisan_domain::RecoverFailedMessage>,
    },
    /// Request a real snapshot for a host mounted on a known thread.
    RequestSnapshot(ThreadId),
    /// Load one persisted image named by a bounded history reference.
    ReadMessageImage(artisan_domain::ImageAttachmentRef),
    /// Load authoritative engine settings for one thread and generation.
    LoadThreadEngineSettings {
        /// Thread whose settings are being read.
        thread_id: ThreadId,
        /// Application-owned stale-response fence.
        generation: SettingsLoadGeneration,
    },
    /// Load the authenticated runtime catalog for one thread/profile scope.
    ReadComposerCatalog {
        /// Thread whose authoritative settings selected the profile.
        thread_id: ThreadId,
        /// Profile whose runtime owns discovery.
        profile_id: artisan_domain::EngineProfileId,
        /// Application-owned stale-response fence.
        generation: CatalogLoadGeneration,
    },
    /// Load the durable global model-favorites snapshot in one thread/profile
    /// scope so a late read cannot update a replacement selector.
    ReadModelFavorites {
        /// Current thread scope at admission time.
        thread_id: ThreadId,
        /// Current engine profile scope at admission time.
        profile_id: artisan_domain::EngineProfileId,
        /// Application-owned stale-response fence.
        generation: CatalogLoadGeneration,
    },
    /// Load the certified engine profile catalogue.
    ListRegisteredProfiles,
    /// Read one engine's provider-account usage, fenced by the profile-menu
    /// connection generation and a per-engine request sequence. The service
    /// fans out per engine so each snapshot `fetched_at` represents that
    /// provider; the application owns freshness, pending rows, and
    /// stale-response pairing.
    ///
    /// Each read executes on the one serial service loop through the existing
    /// bounded `runtime.request` path (existing request deadline, admission
    /// budget, and cancellation behavior; no separate transport owner). Six
    /// sequential reads can therefore delay composer/control commands by up
    /// to six bounded request timeouts in the worst case; per-engine
    /// deduplication in `plan_profile_usage_loads` keeps the common case to
    /// only missing or stale rows.
    ReadAccountUsage {
        /// Stable engine identity narrowed for this read.
        engine_id: String,
        /// Connection scope minted by the profile menu.
        generation: ProfileUsageGeneration,
        /// Per-engine request sequence; an older same-engine reply arriving
        /// after a forced refresh carries a superseded sequence and is
        /// dropped without settling the newer request.
        request_seq: u64,
        /// User-initiated refresh bypasses the backend freshness window.
        force: bool,
    },
    /// Durably save one complete thread engine configuration.
    SetThreadEngineConfig(Box<SetThreadEngineConfig>),
    /// Durably save one desired model-favorite state with stable retry
    /// identity.
    SetModelFavorite(Box<SetModelFavorite>),
    /// Durably queue the first exact message body on one known thread.
    QueueFirstMessage(Box<QueueFirstMessage>),
    /// Send one thread's composer draft at the revision its body was stored
    /// under; the Forge queues exactly that draft once.
    SubmitComposerDraft(Box<SubmitComposerDraft>),
    /// Resolve one assistant-authored HTTP(S) link's page title.
    ResolveRichLink {
        /// Canonical absolute URL selected by the rich-link URL policy.
        url: String,
    },
    /// Inspect the selected attached project's Git repository identity.
    QueryProjectRepository {
        /// Project whose stored root is inspected.
        project_id: ProjectId,
    },
    /// Begin or resume authoritative conversation subscription.
    Subscribe {
        /// Thread to observe.
        thread_id: ThreadId,
        /// Cursor after which to resume, or None for fresh.
        after: Option<ConversationCursor>,
    },
    /// End authoritative conversation subscription.
    Unsubscribe {
        /// Thread no longer observed.
        thread_id: ThreadId,
    },
    /// Acknowledge that the application has applied a batch to cursor.
    AcknowledgePatch {
        /// Thread whose patch was applied.
        thread_id: ThreadId,
        /// Cursor after the applied batch.
        cursor: ConversationCursor,
    },
    /// Stop accepting work and release the session and owned Forge.
    Shutdown,
}

/// Events crossing from the service thread to the GPUI application.
///
/// `Eq` is deliberately absent: the engine observation arm carries sanitized
/// usage rows with a finite `cost_usd: f64`, so only [`PartialEq`] applies.
#[derive(Clone, Debug, PartialEq)]
pub enum NativeTransportEvent {
    ComposerState(ComposerStateEvent),
    ComposerDraft(ComposerDraftEvent),
    ForgeDecision(ForgeDecisionEvent),
    ActiveRun {
        thread_id: ThreadId,
        generation: u64,
        result: artisan_protocol::ActiveRunResult,
    },
    ActiveRunFailed {
        thread_id: ThreadId,
        generation: u64,
        failure: ServiceFailure,
    },
    RunStopped(artisan_protocol::StopRunReceipt),
    StopRunFailed {
        command: artisan_domain::StopRun,
        failure: ServiceFailure,
    },
    /// One approval answer recorded by Forge with its correlated receipt.
    ///
    /// The complete answer intent travels with the receipt; the pairing layer
    /// settles the exact command that was dispatched, never a derivation
    /// from the receipt.
    ApprovalAnswered {
        /// Exact dispatched answer intent.
        command: artisan_domain::RespondApproval,
        /// Correlated Forge receipt.
        receipt: artisan_protocol::RespondApprovalReceipt,
    },
    /// One approval answer failed with a correlated gate-pairing failure.
    ///
    /// The complete answer intent is retained for the pairing layer, and the
    /// failure keeps the peer's exact classification when one answered.
    ApprovalFailed {
        command: artisan_domain::RespondApproval,
        failure: AnswerFailure,
    },
    /// One question answer recorded by Forge with its correlated receipt.
    ///
    /// Identity rules match [`Self::ApprovalAnswered`].
    QuestionAnswered {
        /// Exact dispatched answer intent.
        command: artisan_domain::RespondQuestion,
        /// Correlated Forge receipt.
        receipt: artisan_protocol::RespondQuestionReceipt,
    },
    /// One question answer failed with a correlated gate-pairing failure.
    ///
    /// Identity rules match [`Self::ApprovalFailed`].
    QuestionFailed {
        command: artisan_domain::RespondQuestion,
        failure: AnswerFailure,
    },
    /// Original image data loaded for an exact history reference.
    MessageImageLoaded {
        reference: artisan_domain::ImageAttachmentRef,
        image: artisan_domain::ImageAttachment,
    },
    /// An on-demand history image could not be loaded.
    MessageImageFailed {
        reference: artisan_domain::ImageAttachmentRef,
        failure: ServiceFailure,
    },
    /// The service thread has begun installation/session startup.
    Starting,
    /// Real attached-project rows in Forge order.
    Projects(ProjectListing),
    /// Real project-scoped thread rows in Forge order.
    Threads {
        /// Project whose rows were requested.
        project_id: ProjectId,
        /// Real thread listing.
        listing: ThreadListing,
    },
    /// Background catalog read, fenced independently from navigation.
    SidebarThreads {
        project_id: ProjectId,
        generation: u64,
        result: Result<ThreadListing, ServiceFailure>,
    },
    /// Real bounded conversation state.
    Snapshot(ConversationSnapshot),
    /// Forge returned no attached projects.
    EmptyProjects,
    /// Forge returned no threads for a real project.
    EmptyThreads {
        /// Project with no threads.
        project_id: ProjectId,
    },
    /// One redacted phase of the active project intake.
    ProjectIntakeProgress(NativeProjectIntakeStage),
    /// The user dismissed the native directory chooser.
    ProjectIntakeCancelled,
    /// The intake completed with authoritative listings and identities.
    ProjectIntakeReady {
        /// Complete authoritative attached-project catalog.
        projects: ProjectListing,
        /// Project contained in `projects` and returned by attach.
        project_id: ProjectId,
        /// Complete authoritative thread catalog for `project_id`.
        threads: ThreadListing,
        /// Thread contained in `threads` and returned by create.
        thread_id: ThreadId,
    },
    /// One redacted intake failure and its single retry classification.
    ProjectIntakeFailed {
        /// Operation that failed.
        operation: NativeProjectIntakeOperation,
        /// Typed redacted failure.
        failure: ServiceFailure,
        /// Whether the one retained plan may be explicitly retried.
        retryable: bool,
    },
    /// Redacted startup, request, bridge, or cleanup failure.
    Failed(ServiceFailure),
    /// Authoritative persisted thread engine settings with its load fence.
    ThreadEngineSettings {
        /// Generation minted for this read.
        generation: SettingsLoadGeneration,
        /// Authoritative settings result.
        result: ThreadEngineSettingsResult,
    },
    /// Registered engine profile catalogue.
    RegisteredProfiles(RegisteredEngineProfilesResult),
    /// Registered engine profile catalogue read failure.
    RegisteredProfilesFailed(ServiceFailure),
    /// One engine's provider-account usage with its connection fence.
    AccountUsage {
        /// Engine narrowed by the triggering read.
        engine_id: String,
        /// Connection scope minted by the profile menu.
        generation: ProfileUsageGeneration,
        /// Per-engine request sequence echoed from the triggering read.
        request_seq: u64,
        /// Provider-owned row; its `fetched_at_ms` is the snapshot's own
        /// observation time, never the client receipt clock.
        entry: NativeUsageEntry,
    },
    /// One engine's provider-account usage read failure with its fence.
    /// The application preserves last-good meters and records this failure
    /// only on the failing engine row.
    AccountUsageFailed {
        /// Engine narrowed by the triggering read.
        engine_id: String,
        /// Connection scope minted by the profile menu.
        generation: ProfileUsageGeneration,
        /// Per-engine request sequence echoed from the triggering read.
        request_seq: u64,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Runtime model catalog for one exact thread/profile/generation scope.
    ComposerCatalog {
        /// Thread that owns the discovery request.
        thread_id: ThreadId,
        /// Profile used for discovery.
        profile_id: artisan_domain::EngineProfileId,
        /// Application-owned stale-response fence.
        generation: CatalogLoadGeneration,
        /// Validated shared catalog response.
        result: artisan_protocol::ComposerCatalogResult,
    },
    /// Runtime model catalog discovery failed for one exact scope.
    ComposerCatalogFailed {
        /// Thread that owns the discovery request.
        thread_id: ThreadId,
        /// Profile used for discovery.
        profile_id: artisan_domain::EngineProfileId,
        /// Application-owned stale-response fence.
        generation: CatalogLoadGeneration,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Durable model favorites for one exact frontend scope.
    ModelFavorites {
        /// Thread scope captured before the query was admitted.
        thread_id: ThreadId,
        /// Profile scope captured before the query was admitted.
        profile_id: artisan_domain::EngineProfileId,
        /// Application-owned stale-response fence.
        generation: CatalogLoadGeneration,
        /// Complete authoritative ordered snapshot.
        result: artisan_protocol::ModelFavoritesSnapshot,
    },
    /// Durable model favorites read failed for one exact scope.
    ModelFavoritesFailed {
        /// Thread scope captured before the query was admitted.
        thread_id: ThreadId,
        /// Profile scope captured before the query was admitted.
        profile_id: artisan_domain::EngineProfileId,
        /// Application-owned stale-response fence.
        generation: CatalogLoadGeneration,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Correlated durable favorite mutation receipt.
    ModelFavoriteSet {
        /// Thread scope captured by the mutation.
        thread_id: ThreadId,
        /// Profile scope captured by the mutation.
        profile_id: artisan_domain::EngineProfileId,
        /// Stable mutation identity.
        request_id: RequestId,
        /// Authoritative mutation receipt and post-state.
        receipt: artisan_protocol::SetModelFavoriteReceipt,
    },
    /// One resolved rich-link page title.
    RichLinkResolved {
        /// Optional bounded favicon image.
        favicon: Vec<u8>,
        /// Canonical URL the resolution answers.
        requested_url: String,
        /// Resolved display title.
        page_name: String,
        /// Backend cache expiry as Unix epoch milliseconds.
        expires_at_ms: i64,
    },
    /// One rich-link resolution failure; the authored label stays.
    RichLinkFailed {
        /// Canonical URL whose resolution failed.
        requested_url: String,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Inspected repository identity for one project.
    ///
    /// `repository` is `None` for a root that is not (or is no longer) a
    /// repository: the titlebar keeps its project-folder fallback rather than
    /// inventing repository facts.
    ProjectRepository {
        /// Project whose stored root was inspected.
        project_id: ProjectId,
        /// Observed repository identity, when the root is a repository.
        repository: Option<ProjectRepository>,
    },
    /// One project-repository read failed; the folder fallback stays.
    ProjectRepositoryFailed {
        /// Project whose read failed.
        project_id: ProjectId,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Correlated durable favorite mutation failure. The application retains
    /// the command for an explicit same-identity retry.
    ModelFavoriteFailed {
        /// Thread scope captured by the mutation.
        thread_id: ThreadId,
        /// Profile scope captured by the mutation.
        profile_id: artisan_domain::EngineProfileId,
        /// Stable mutation identity.
        request_id: RequestId,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Durable thread engine configuration applied.
    ThreadEngineConfigSet(SetThreadEngineConfigResult, Box<EngineRunConfig>),
    /// Thread engine configuration precondition was stale.
    ThreadEngineConfigConflict {
        /// Thread whose save conflicted.
        thread_id: ThreadId,
        /// Exact save request that conflicted.
        request_id: RequestId,
    },
    /// Durable engine configuration save failed with a redacted diagnostic.
    ThreadEngineConfigFailed {
        /// Thread whose save failed.
        thread_id: ThreadId,
        /// Exact save request that failed.
        request_id: RequestId,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Durable first-message queue accepted or replayed by Forge.
    FirstMessageQueued(FirstMessageReceipt),
    /// Durable first-message queue failed with a redacted diagnostic.
    FirstMessageFailed {
        /// Thread whose queue request failed.
        thread_id: ThreadId,
        /// Exact queue request that failed.
        request_id: RequestId,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// General message accepted or replayed by Forge.
    MessageQueued(QueueMessageReceipt),
    /// A draft submission failed; the draft stays in the composer.
    MessageFailed {
        thread_id: ThreadId,
        request_id: RequestId,
        failure: ServiceFailure,
    },
    /// The Forge refused a draft submission because the draft is at another
    /// revision; nothing was queued.
    MessageStale {
        /// Thread whose draft was submitted.
        thread_id: ThreadId,
        /// The refused submission.
        request_id: RequestId,
        /// The thread's current draft revision.
        current_revision: Option<artisan_domain::ComposerDraftRevision>,
    },
    /// Authoritative thread-settings read failure with its load fence.
    ThreadEngineSettingsFailed {
        /// Thread whose settings were requested.
        thread_id: ThreadId,
        /// Generation minted for this read.
        generation: SettingsLoadGeneration,
        /// Redacted failure.
        failure: ServiceFailure,
    },
    /// Correlated subscription start acknowledgement.
    ConversationSubscriptionStarted {
        /// Thread whose subscription started.
        thread_id: ThreadId,
        /// Request that started the subscription.
        request_id: RequestId,
        /// Fresh or resumed start payload.
        started: ConversationSubscriptionStarted,
    },
    /// Correlated subscription stop acknowledgement.
    ConversationSubscriptionStopped {
        /// Thread whose subscription stopped.
        thread_id: ThreadId,
        /// Request that stopped the subscription.
        request_id: RequestId,
        /// Stop payload.
        stopped: ConversationSubscriptionStopped,
    },
    /// Uni-stream patch batch.
    PatchBatch(PatchBatch),
    /// Uni-stream engine observation with its connection replay cursor.
    ///
    /// The application pairs the observation into presentation state and owns
    /// reconnect replay ordering; the service never advances a cursor here.
    EngineObservation(ServerEvent),
    /// The subscribed thread's complete message outbox, pushed by the Forge
    /// whenever its undelivered messages change.
    MessageOutbox(artisan_domain::MessageOutbox),
    /// Bounded path-free delivery loss.
    DeliveryLost(ServiceFailure),
    /// Terminal service state.
    Stopped(ServiceStopStatus),
}

/// One cloneable application-side handle to the native service.
#[derive(Clone)]
pub struct NativeTransportService {
    commands: tokio::sync::mpsc::Sender<QueuedCommand>,
    events: Arc<Mutex<Receiver<NativeTransportEvent>>>,
    finished: Arc<AtomicBool>,
    shutdown_requested: Arc<AtomicBool>,
    holds: Arc<ConnectionHolds>,
    join: Arc<Mutex<Option<JoinHandle<()>>>>,
}

struct ServiceRuntime {
    preserve_reconnect: bool,
    session: Option<ClientSession>,
    reconnect_lease: Option<ReconnectSessionLease>,
    reconnect_binding: ReconnectBinding,
    certificate: CertificateDer<'static>,
    target: artisan_transport::SessionTarget,
    pinned_identity: PinnedIdentity,
    limits: ClientSessionLimits,
    lease: Option<ForgeProcessLease>,
    cancel: CancelHandle,
    shutdown_grace: Duration,
    known_threads: HashSet<ThreadId>,
    /// Attachments this connection uploaded or read back from the Forge store.
    intake: IntakeState,
    custody: SubscriptionCustody,
    delivery_cancel: Option<Arc<CancelHandle>>,
    delivery_join: Option<tokio::task::JoinHandle<()>>,
    delivery_tx: Option<tokio::sync::mpsc::Sender<PrivateDelivery>>,
}

fn publish(
    events: &SyncSender<NativeTransportEvent>,
    event: NativeTransportEvent,
) -> Result<(), ServiceFailure> {
    events.send(event).map_err(|_| ServiceFailure::bridge())
}

#[cfg(test)]
fn custody_trace() -> Vec<CustodyStep> {
    cleanup_plan(true, true, true)
}

#[path = "native_composer_transport.rs"]
mod composer_operations;

#[path = "native_composer_state_transport.rs"]
mod composer_state_operations;
pub(crate) use composer_state_operations::{ComposerStateCommand, ComposerStateEvent};

#[path = "native_composer_draft_transport.rs"]
mod composer_draft_operations;
pub(crate) use composer_draft_operations::{ComposerDraftCommand, ComposerDraftEvent};

#[path = "native_forge_decisions_transport.rs"]
mod forge_decision_operations;
pub(crate) use forge_decision_operations::{ForgeDecisionCommand, ForgeDecisionEvent};

#[path = "native_profile_usage_transport.rs"]
mod profile_usage_operations;

#[path = "native_transport_service/connection_holds.rs"]
mod connection_holds;
pub use connection_holds::{ConnectionHolds, Hold, HoldKind, HoldState, QueuedCommand};

#[path = "native_transport_service/diagnostics.rs"]
mod diagnostics;

pub use diagnostics::{
    AnswerFailure, CommandSendError, EventReceiveError, NativeProjectIntakeOperation,
    NativeProjectIntakeStage, PrivateDelivery, ReadinessValidationError, ServiceFailure,
    ServiceFailureCategory, ServiceFailureStage, ServiceJoinError, ServiceSpawnError,
    ServiceStopStatus, StartupError, SubscriptionFailureDisposition, SubscriptionRequestKind,
    subscription_failure_disposition, validate_readiness,
};
#[cfg(test)]
use diagnostics::{
    DurableSaveRetryClassification, durable_save_retry_classification, payload_health_decision,
};
use diagnostics::{
    PeerFailure, RequestAttemptError, RequestFailure, local_session_request_loss_is_retryable,
};

#[path = "native_transport_service/request_construction.rs"]
mod request_construction;

#[cfg(test)]
use request_construction::reconnect_hello;
use request_construction::{
    FrameFactory, StableMutation, account_usage_request, approval_stable_mutation, attach_mutation,
    build_reconnect_binding, composer_catalog_request, create_mutation, draft_submission_mutation,
    engine_config_stable_mutation, finite_duration, first_message_stable_mutation,
    make_request_frame, model_favorite_stable_mutation, model_favorites_request,
    project_repository_request, project_request, query_request, question_stable_mutation,
    real_unix_millis, reconnect_hello_with_capability, registered_profiles_request,
    rich_link_request, snapshot_request, thread_engine_settings_request, threads_request,
};

#[path = "native_transport_service/response_validation.rs"]
mod response_validation;

use response_validation::{
    ExpectedResponse, ThreadSelectionDecision, optional_request_id_matches, request_id_matches,
    thread_selection_decision, validate_response_family,
};
pub use response_validation::{
    UniDelivery, validate_started_correlation, validate_stopped_correlation, validate_uni_envelope,
};

mod remote;
#[path = "native_transport_service/service_lifecycle.rs"]
mod service_lifecycle;

pub use service_lifecycle::try_send_command;
#[cfg(test)]
use service_lifecycle::{CustodyStep, cleanup_plan};

#[path = "native_transport_service/request_delivery.rs"]
mod request_delivery;

pub use request_delivery::delivery_task_loop;
#[cfg(test)]
use request_delivery::session_needs_reconnect;
use request_delivery::{command_loop_with_delivery, request_envelope_payload};

#[path = "native_transport_service/subscriptions.rs"]
mod subscriptions;

pub use subscriptions::SubscriptionCustody;
use subscriptions::{
    handle_acknowledge_patch, handle_delivery_lost_reconnect, handle_subscribe_command,
    handle_unsubscribe,
};

#[path = "native_transport_service/project_intake.rs"]
mod project_intake;

#[cfg(test)]
use project_intake::{
    IntakeRetry, attach_retry_allowed, contains_exact_project, contains_exact_thread,
    create_command_values,
};
use project_intake::{
    IntakeState, begin_project_intake, create_task_in_project, retry_project_intake,
};

#[path = "native_transport_service/handlers.rs"]
mod handlers;

#[path = "native_transport_service/answer_handlers.rs"]
mod answer_handlers;

use answer_handlers::{respond_approval, respond_question};
use handlers::{
    durable_save_request, known_thread_for_queue, list_registered_profiles, load_initial_catalog,
    load_thread_engine_settings, query_project_repository, queue_first_message, read_message_image,
    request_snapshot, resolve_rich_link, select_project, set_thread_engine_config,
    submit_composer_draft,
};

#[cfg(test)]
#[path = "native_transport_service/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "native_transport_service/command_loop_tests.rs"]
mod command_loop_tests;
