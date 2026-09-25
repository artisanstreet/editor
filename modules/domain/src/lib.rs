//! Product concepts that do not depend on transport, storage, or presentation.
//!
//! This crate owns the application-domain vocabulary of the first native
//! end-to-end workflow: attach one Forge-visible directory by opaque identity,
//! create a project-scoped thread, and durably queue the first bounded text
//! message behind an accepted-or-duplicate receipt. Engine dispatch and
//! execution stay outside the crate: the conversation vocabulary stores
//! assistant output under an opaque Forge-minted run routing id, but models
//! no engine, provider, or run-lifecycle state.
//!
//! Structure:
//!
//! - [`bounds`] documents every ceiling and its unit (UTF-8 bytes throughout);
//! - [`identifiers`] validates the wire-facing identities, split by who mints
//!   them: clients mint only [`RequestId`], Forge mints everything else;
//! - [`text`] holds bounded display and message values;
//! - [`model`] holds listed and durable state values;
//! - [`conversation`] holds renderer snapshots, replay batches, and both
//!   durable item kinds;
//! - [`commands`] and [`events`] hold the workflow's mutations with their
//!   request correlation and its durable facts;
//! - [`time`] carries the schema's signed Unix epoch milliseconds.
//!
//! The domain is independent of Cap'n Proto, Quinn, `SeaORM`, GPUI, Tokio,
//! filesystem APIs, and wall-clock acquisition; it depends only on
//! `thiserror`, plus `uuid` for [`RequestId::mint`], the single client
//! request-id mint (a `UUIDv7` reads the clock and the OS random source).
//! External values return typed errors instead of panicking.
//! Filesystem paths are carried as opaque descriptions without
//! canonicalization.

pub mod bounds;
pub mod commands;
pub mod conversation;
pub mod engine_config;
pub mod engine_socket;
pub mod events;
pub mod identifiers;
mod legacy_workspace_id;
pub mod message;
pub mod model;
pub mod observation;
pub mod text;
pub mod time;

pub use bounds::{
    COMPOSER_ATTACHMENT_MAX_BYTES, COMPOSER_ATTACHMENTS_MAX_TOTAL_BYTES,
    CONVERSATION_PATCH_BATCH_MAX_PATCHES, CONVERSATION_QUERY_MAX_TURNS,
    CONVERSATION_TEXT_FRAGMENT_MAX_BYTES, DIRECTORY_LISTING_MAX_ENTRIES,
    DIRECTORY_LISTING_MAX_PLACES, DISPLAY_NAME_MAX_BYTES, ENGINE_CONFIG_MAX_ENCODED_BYTES,
    ENGINE_PROFILE_ID_MAX_BYTES, ENGINE_RUNTIME_MAX_BODY_BYTES, ENGINE_RUNTIME_MAX_HEADER_COUNT,
    ENGINE_RUNTIME_MAX_LINE_BYTES, ENGINE_RUNTIME_MAX_MILLIS, ENGINE_RUNTIME_MAX_OBSERVATIONS,
    ENGINE_RUNTIME_MAX_SSE_EVENT_BYTES, ENGINE_RUNTIME_MAX_STDERR_BYTES,
    ENGINE_USAGE_EMAIL_MAX_BYTES, ENGINE_USAGE_ENGINES_MAX, ENGINE_USAGE_REASON_MAX_BYTES,
    ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE, IDENTIFIER_MAX_BYTES, MESSAGE_BODY_MAX_BYTES,
    MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES, MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
    MESSAGE_IMAGE_ATTACHMENT_MIME_MAX_BYTES, MESSAGE_IMAGE_ATTACHMENT_NAME_MAX_BYTES,
    MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES, PROJECT_LISTING_MAX_PROJECTS, ROOT_PATH_MAX_BYTES,
    THREAD_LISTING_MAX_THREADS, THREAD_TITLE_MAX_BYTES,
};
pub use commands::{
    AttachProject, Command, CreateThread, ListAttachedProjects, ListDirectories,
    ListProjectThreads, ListRegisteredEngineProfiles, Query, QueueFirstMessage, QueueMessage,
    ReadActiveRun, ReadMessageImage, ReadThreadEngineSettings, SetThreadEngineConfig, SteerTarget,
    StopRun,
};
pub use conversation::{
    AssistantBody, AssistantBodyError, AssistantMessageItem, AssistantMessagePhase,
    ConversationCursor, ConversationItem, ConversationLifecycle, ConversationPatch,
    ConversationQuery, ConversationQueryBounds, ConversationRequest, ConversationSnapshot,
    ConversationSnapshotError, ConversationSubscribe, ConversationSubscriptionStart,
    ConversationTurn, ConversationUnsubscribe, CounterError, IncrementalText, IncrementalTextError,
    ItemOrdinal, LifecycleTransitionError, MultimodalUserMessageItem, PatchBatch, PatchBatchError,
    PatchSequence, QueryTurnCount, QueryTurnCountError, Revision, TurnOrdinal, UserMessageItem,
};
pub use engine_config::{
    ApprovalMode, ByteLimit, ClaudeEffort, ClaudePermissionMode, ClaudeSelection,
    CodexModelContextWindow, CodexReasoningEffort, CodexSelection, CodexServiceTier, CountLimit,
    CursorPermissionMode, CursorReasoningEffort, CursorSelection, CursorSpeed, EngineConfigError,
    EngineConfigReason, EngineConfigRevision, EngineConfigUpdatePrecondition, EngineId,
    EnginePermissionPolicy, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, GrokPermissionMode, GrokReasoningEffort,
    GrokSelection, NetworkAccess, OpenCode2Selection, WebSearchAccess,
};
pub use engine_socket::{
    EngineCapabilityName, EngineCapabilityState, EngineCommandTag, EngineDescriptor,
    EngineObservationTag, EngineOpenError, EngineOpenFuture, EngineOpenInput, EngineOpenOutcome,
    EngineOpenResult, EngineProbe, EngineResumeToken, EngineRun, EngineRunTerminalState,
    EngineSocket, EngineSocketSession,
};
pub use events::{
    EngineObservationAttribution, EngineObservationEvent, Event, FirstMessageQueued,
    ProjectAttached, ThreadCreated,
};
pub use identifiers::{
    DirectoryId, EngineAgentId, EngineModelId, EngineProfileId, EngineProfileIdError,
    EngineRouteId, EngineVariantId, IdentifierError, ItemId, MessageId, PatchId, PermissionId,
    ProjectId, RequestId, RunId, ThreadId, TurnId,
};
pub use message::{
    AuthoredText, AuthoredTextError, ImageAttachment, ImageAttachmentError, ImageAttachmentRef,
    ImageAttachmentRefError, ImageMimeType, ImageMimeTypeError, QueueMessagePayload,
    QueueMessagePayloadError,
};
pub use model::{
    CommandReceipt, DirectoryEntry, DirectoryKind, DirectoryListing, DirectoryListingError,
    DirectoryPlace, PlaceKind, ProjectListing, ProjectListingError, ProjectSummary, QueuedMessage,
    ReceiptDisposition, ThreadListing, ThreadListingError, ThreadSummary,
};
pub use observation::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation, ApprovalKind,
    ApprovalObservation, ApprovalRequest, ApprovalState, ArtisanCode, CompactionObservation,
    CompactionState, DiagnosticLevel, EngineErrorRef, EngineErrorRefInput, FileAction,
    FileObservation, LimitScope, MessagePhase, NativeActionObservation,
    OBSERVATION_ANSWER_MAX_BYTES, OBSERVATION_ANSWERS_MAX, OBSERVATION_ARTISAN_CODE_MAX_BYTES,
    OBSERVATION_COMMAND_MAX_BYTES, OBSERVATION_COUNT_MAX, OBSERVATION_DELTA_MAX_BYTES,
    OBSERVATION_DESCRIPTION_MAX_BYTES, OBSERVATION_DURATION_MAX_MILLIS, OBSERVATION_ID_MAX_BYTES,
    OBSERVATION_LABEL_MAX_BYTES, OBSERVATION_LIMIT_LABEL_MAX_BYTES, OBSERVATION_MESSAGE_MAX_BYTES,
    OBSERVATION_OUTPUT_MAX_BYTES, OBSERVATION_PATH_MAX_BYTES, OBSERVATION_PLAN_MAX_ENTRIES,
    OBSERVATION_PLAN_TEXT_MAX_BYTES, OBSERVATION_PROVIDER_CODE_MAX_BYTES,
    OBSERVATION_QUERY_MAX_BYTES, OBSERVATION_QUESTION_MAX_OPTIONS, OBSERVATION_REASON_MAX_BYTES,
    OBSERVATION_SEQUENCE_MAX, OBSERVATION_SUMMARY_INDEX_MAX, OBSERVATION_TEXT_MAX_BYTES,
    OBSERVATION_TIMESTAMP_MAX_BYTES, OBSERVATION_TITLE_MAX_BYTES, Observation, ObservationError,
    ObservationId, ObservationSequence, PlanEntry, PlanEntryStatus, PlanObservation,
    ProcessDiagnosticObservation, ProtocolDiagnosticObservation, QuestionInput,
    QuestionObservation, QuestionOption, QuestionState, ReasoningSummaryCompletedObservation,
    ReasoningSummaryDeltaObservation, RetryAttemptState, RetryObservation, RunState,
    RunStateObservation, RunTerminalObservation, RunTerminalState, SearchObservation, SearchScope,
    SearchState, SubagentInput, SubagentObservation, SubagentState, SubagentTranscriptObservation,
    TerminalActivityInput, TerminalActivityObservation, TerminalActivityState, TerminalChannel,
    ToolAction, ToolObservation, TranscriptAgentMessageCompleted, TranscriptAgentMessageDelta,
    TranscriptContent, TranscriptFile, TranscriptReasoningSummaryCompleted,
    TranscriptReasoningSummaryDelta, TranscriptSearch, TranscriptTerminalActivity, TranscriptTool,
    TurnState, TurnStateObservation, UsageBasis, UsageInput, UsageObservation,
};
pub use text::{
    DisplayName, DisplayNameError, MessageBody, MessageBodyError, RootPath, RootPathError,
    ThreadTitle, ThreadTitleError,
};
pub use time::UnixMillis;

pub use legacy_workspace_id::{WorkspaceId, WorkspaceIdError};

mod model_favorites;
pub use model_favorites::{
    MODEL_FAVORITES_MAX_MODELS, MODEL_FAVORITES_MAX_SNAPSHOT_BYTES, ModelFavoriteId,
    ModelFavoriteIdError, ModelFavoritesRevision, ModelFavoritesRevisionError,
    ModelFavoritesSnapshot, ModelFavoritesSnapshotError,
};

mod run_usage;
pub use run_usage::{
    RUN_USAGE_MAX_SOURCE_SEQUENCE, RUN_USAGE_MAX_TOKEN_COUNT, RUN_USAGE_PROVIDER_SESSION_MAX_BYTES,
    RUN_USAGE_PROVIDER_TURN_MAX_BYTES, RunUsageBasis, RunUsageReport, RunUsageReportError,
    RunUsageReportInput,
};

mod queued_message;
pub use queued_message::{
    DispatchError, DispatchErrorParseError, FAILED_MESSAGE_LIST_MAX, FailedMessageListError,
    FailedMessageListing, FailedMessageListingError, FailedMessageSummary, ListFailedMessages,
    ListQueuedMessages, QUEUED_MESSAGE_LIST_MAX, QueuedMessageListError, QueuedMessageListOrder,
    QueuedMessageListing, QueuedMessageListingError, QueuedMessageState, QueuedMessageSummary,
    QueuedMessageWithdrawalOutcome, WithdrawQueuedMessage, WithdrawQueuedMessageResult,
};
mod message_outbox;
pub use message_outbox::{
    FailedMessageRecovered, FailedMessageRetried, FailedMessageRetryOutcome, FailedMessageTarget,
    MessageOutbox, MessageOutboxError, RecoverFailedMessage, RetryFailedMessage,
};
mod run_interaction;
pub use run_interaction::{
    InteractionKind, InteractionOutcome, RespondApproval, RespondQuestion, RunInteractionError,
};

pub use model_favorites::MODEL_FAVORITE_ID_MAX_BYTES;

pub mod catalog_selection;
pub use catalog_selection::{
    CATALOG_OPTION_ID_MAX_BYTES, CatalogOptionId, CatalogSelection, CatalogSelectionError,
    ModelSelectionResolution, ResolveModelSelection, SUBMISSION_REFUSAL_MESSAGE_MAX_BYTES,
    SubmissionRefusal, SubmissionRefusalKind,
};

pub mod composer_catalog;
pub use composer_catalog::{
    CATALOG_REVISION_MAX_BYTES, CatalogRevision, CatalogRevisionError, ReadComposerCatalog,
    ReadHostCatalog, ReadModelFavorites, SetModelFavorite,
};

pub mod account_usage;
pub use account_usage::{
    EngineReadiness, EngineReadinessVerdict, EngineUsageAuth, EngineUsageAuthentication,
    EngineUsageError, EngineUsageReport, EngineUsageSnapshot, EngineUsageWindow,
    EngineUsageWindowKind, QuotaSurface, ReadAccountUsage, clamp_percent_used, iso_millis, utc_ymd,
    validate_iso_timestamp,
};

pub mod composer_state;
pub use composer_state::{
    QueuedMessageWithdrawalResult, ReadRecalledMessage, ReadRunUsage, RecalledMessageResult,
    RunUsageResult, WithdrawQueuedMessageCommand,
};

pub mod composer_draft;
mod draft_submission;
pub use composer_draft::{
    ComposerAttachmentDigest, ComposerAttachmentRef, ComposerAttachmentResult,
    ComposerAttachmentUploaded, ComposerDraft, ComposerDraftError, ComposerDraftResult,
    ComposerDraftRevision, ComposerDraftSaved, ComposerDraftScope, ComposerImage,
    QueueStoredMessage, ReadComposerAttachment, ReadComposerDraft, SaveComposerDraft,
    UploadComposerAttachment,
};
pub use draft_submission::{ComposerDraftSubmitted, DraftSubmissionOutcome, SubmitComposerDraft};

pub mod user_preferences;
pub use user_preferences::{
    AccountProfile, ImportLegacyPreferences, LegacyImportOutcome, LegacyPreferencesImported,
    NAVIGATION_PROJECTS_MAX, NavigationProject, NavigationRecord, NavigationRecordError,
    NavigationRoute, ReadUserPreferences, RecordNavigation, UserPreferences,
    UserPreferencesRevision,
};
