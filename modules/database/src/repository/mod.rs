//! Domain-typed repositories for the native schema.

mod common;
mod composer_attachment_upload;
mod composer_draft;
mod conversation_patch_replay;
mod conversation_projection;
mod dispatch_payload;
mod draft_submission;
mod first_message;
mod message_dispatch;
mod observation_ledger;
mod project_catalog;
mod project_draft_submission;
mod project_threads;
mod queue_message;
mod recent_threads;
mod run_binding;
mod run_interaction;
mod run_launch;
mod run_observation;
mod startup_reconciliation;
mod startup_reconciliation_disposition;
mod thread_engine_config;

use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use thiserror::Error;

use artisan_domain::{
    MessageId, ProjectId, ProjectListingError, RequestId, RootPath, ThreadId, ThreadListingError,
    UnixMillis,
};

use crate::entities;

pub(crate) use common::{
    RepositoryFailure, corrupt_data, database_error, negative_counter, row_value,
};

pub use composer_attachment_upload::ComposerAttachmentChunkOutcome;
pub use composer_draft::{
    COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS, ComposerDraftRepositoryError,
    ComposerDraftSaveOutcome, SaveComposerDraftInput,
};
pub use conversation_patch_replay::ConversationPatchReplay;
pub use dispatch_payload::{MessageDispatchPayload, QueueMessageDispatchPayload};
pub use draft_submission::{DraftSubmission, DraftSubmissionError, SubmitComposerDraftInput};
pub use first_message::{QueueFirstMessageInput, QueueFirstMessageResult};
pub use message_dispatch::{
    ClaimMessageDispatch, ClaimedMessageDispatch, CompleteMessageDispatch, DispatchFailureReason,
    DispatchFailureReasonError, DispatchLeaseOwner, DispatchLeaseOwnerError, FailMessageDispatch,
    RequeueMessageDispatch, TransitionedMessageDispatch,
};
pub use project_draft_submission::{ProjectDraftSubmission, SubmitProjectDraftInput};
pub use project_threads::{
    AttachProjectInput, AttachProjectResult, CreateThreadInput, CreateThreadResult,
};
pub use queue_message::{MessageImageRead, QueueMessageInput, QueueMessageResult};
pub use recent_threads::RecentThreadsFingerprint;
pub use run_binding::{
    BindRunProvider, BindRunProviderOutcome, BoundRunReceipt, ProviderBindingBytes, RunBindingError,
};
pub use run_interaction::{
    AppliedInteraction, ApprovalSnapshot, PendingInteractionView, QuestionSnapshot,
    RecordApprovalRequest, RecordInteractionOutcome, RecordQuestionRequest,
    RequestedInteractionSnapshot, ResolveInteractionOutcome, ResolveScope, RunInteractionError,
    StoredInteractionReceipt,
};
pub use run_launch::{
    FailUnstartedRun, FailUnstartedRunOutcome, LaunchClaimedRun, LaunchClaimedRunOutcome,
    LaunchedRunReceipt, ProjectSteeredMessage, ProjectSteeredMessageOutcome, RunLaunchCredentials,
    RunLaunchError, RunStartKey, SteeredMessageReceipt,
};
pub use run_observation::terminal::{
    AuxiliaryTerminalError, CancelRun, CancelRunError, CancelRunOutcome, CompleteRun,
    CompleteRunError, CompleteRunOutcome, FailRun, FailRunError, FailRunOutcome, InterruptRun,
    InterruptRunError, InterruptRunOutcome, InterruptedRunReceipt, RunErrorCode, RunErrorMessage,
    TerminalRunReceipt,
};
pub use run_observation::{
    AssistantChange, CheckpointUpdate, CommitRunBatch, CommitRunBatchOutcome,
    DecodedObservationBatch, EngineCheckpoint, OBSERVATION_BATCH_MAX_OBSERVATIONS,
    OBSERVATION_CHECKPOINT_VERSION, OBSERVATION_FORMAT_TAG, ObservationCommitError,
    RunBatchReceiptInfo, RunBatchScope, RunObservationError, decode_observation_checkpoint,
    encode_observation_bytes, encode_observation_checkpoint, validate_observation_bind,
    validate_observation_engine,
};
pub use startup_reconciliation::{
    ExpiredLeaseRecovery, StartupReconciliationCandidate, StartupReconciliationCandidates,
    StartupReconciliationError, StartupReconciliationQuery, StartupRunLifecycle,
};
pub use startup_reconciliation_disposition::{
    StartupReconciliationDisposition, StartupReconciliationDispositionError,
    StartupReconciliationDispositionOutcome, StartupReconciliationDispositionReceipt,
};
pub use thread_engine_config::{
    SetThreadEngineConfigInput, SetThreadEngineConfigResult, ThreadEngineSettings,
};

/// Typed failures at the native persistence boundary.
#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("project `{project_id}` is not attached")]
    ProjectNotFound { project_id: ProjectId },

    #[error(
        "project id `{project_id}` is already attached at `{existing_root_path}`, not `{requested_root_path}`"
    )]
    ProjectConflict {
        project_id: ProjectId,
        existing_root_path: RootPath,
        requested_root_path: RootPath,
    },

    #[error("thread `{thread_id}` is not attached to a known project")]
    ThreadNotFound { thread_id: ThreadId },

    #[error("thread `{thread_id}` has no engine configuration; select a model before sending")]
    ThreadEngineNotConfigured { thread_id: ThreadId },

    #[error("thread `{thread_id}` engine configuration revision does not match the precondition")]
    EngineConfigRevisionConflict {
        thread_id: ThreadId,
        expected_revision: Option<artisan_domain::EngineConfigRevision>,
        actual_revision: Option<artisan_domain::EngineConfigRevision>,
    },

    #[error("thread `{thread_id}` already exists with different persisted values")]
    ThreadConflict { thread_id: ThreadId },

    #[error("message id `{message_id}` already identifies a different message")]
    MessageConflict { message_id: MessageId },

    #[error(
        "thread `{thread_id}` already has first message `{existing_message_id}` from another request"
    )]
    FirstMessageAlreadyExists {
        thread_id: ThreadId,
        existing_message_id: MessageId,
    },

    #[error("request id `{request_id}` was already used for a different command")]
    IdempotencyConflict { request_id: RequestId },

    #[error(
        "dispatch lease expiry {lease_expires_at_ms} must be later than claim time {claimed_at_ms}"
    )]
    InvalidDispatchLeaseWindow {
        claimed_at_ms: i64,
        lease_expires_at_ms: i64,
    },

    #[error("message dispatch for `{message_id}` exhausted its persisted attempt counter")]
    DispatchAttemptLimit { message_id: MessageId },

    #[error("message dispatch `{message_id}` does not exist")]
    DispatchNotFound { message_id: MessageId },

    #[error(
        "message dispatch `{message_id}` is {state}; only a live leased dispatch accepts lifecycle transitions"
    )]
    InvalidDispatchState {
        message_id: MessageId,
        state: &'static str,
    },

    #[error("message dispatch `{message_id}` belongs to a different lease owner")]
    DispatchOwnerMismatch { message_id: MessageId },

    #[error(
        "lease on message dispatch `{message_id}` expired at {lease_expires_at_ms}, no later than the operation time {operated_at_ms}"
    )]
    DispatchLeaseExpired {
        message_id: MessageId,
        lease_expires_at_ms: i64,
        operated_at_ms: i64,
    },

    #[error("{later_field} timestamp precedes {earlier_field} timestamp")]
    InvalidChronology {
        earlier_field: &'static str,
        later_field: &'static str,
    },

    #[error("persisted `{table}.{field}` violates the domain contract: {reason}")]
    CorruptData {
        table: &'static str,
        field: &'static str,
        reason: String,
    },

    #[error("persisted thread listing exceeds its domain bound")]
    ThreadListing {
        #[source]
        source: ThreadListingError,
    },

    #[error("persisted project listing exceeds its domain bound")]
    ProjectListing {
        #[source]
        source: ProjectListingError,
    },

    #[error("native database invariant failed: {reason}")]
    Invariant { reason: &'static str },

    #[error("database operation `{operation}` failed")]
    Database {
        operation: &'static str,
        #[source]
        source: DbErr,
    },
}

/// Cloneable access to native database repositories.
#[derive(Clone, Debug)]
pub struct Repository {
    database: DatabaseConnection,
}

impl Repository {
    #[must_use]
    pub const fn new(database: DatabaseConnection) -> Self {
        Self { database }
    }

    /// Reads the persisted project root for a thread without consulting the
    /// process working directory, source tree, or environment.  A caller
    /// uses this only while it still owns the immutable thread/run snapshot;
    /// the root itself is validated at the domain boundary before it leaves
    /// the repository.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] if the thread or attached project is
    /// missing, persisted data is corrupt, or a database query fails.
    pub async fn read_thread_project_root(
        &self,
        thread_id: &ThreadId,
    ) -> Result<RootPath, RepositoryError> {
        let project_id = self.read_thread_project(thread_id).await?;
        self.read_project_root(project_id).await
    }

    /// Reads the persisted root of an attached project.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] if the project is missing, its root is
    /// corrupt, or a database query fails.
    pub async fn read_project_root(
        &self,
        project_id: ProjectId,
    ) -> Result<RootPath, RepositoryError> {
        let project = entities::attached_project::Entity::find_by_id(project_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| database_error("read attached project", source))?
            .ok_or(RepositoryError::ProjectNotFound { project_id })?;
        RootPath::parse(project.root_path)
            .map_err(|error| corrupt_data("attached_projects", "root_path", error))
    }

    /// Reads the attached project that owns a thread.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] if the thread is missing, its project id
    /// is corrupt, or a database query fails.
    pub async fn read_thread_project(
        &self,
        thread_id: &ThreadId,
    ) -> Result<ProjectId, RepositoryError> {
        let thread = entities::thread::Entity::find_by_id(thread_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| database_error("read thread project", source))?
            .ok_or_else(|| RepositoryError::ThreadNotFound {
                thread_id: thread_id.clone(),
            })?;
        ProjectId::parse(thread.project_id)
            .map_err(|error| corrupt_data("threads", "project_id", error))
    }
}

fn millis(value: UnixMillis) -> i64 {
    value.as_millis()
}

mod model_favorites;
pub use model_favorites::{
    ModelFavoritesRepositoryError, SetModelFavoriteInput, SetModelFavoriteResult,
};

mod run_usage;
pub use run_usage::{
    RecordRunUsage, RecordRunUsageInput, RecordRunUsageOutcome, RunUsageRepositoryError,
    RunUsageWriteReceipt,
};

mod queued_message;
pub use queued_message::{
    FailedMessageRecovery, MessageOutboxFingerprint, QueuedMessageRepositoryError,
};

mod user_preferences;
pub use user_preferences::{LegacyImport, StoredUserPreferences};

mod session_continuation;
pub use session_continuation::{
    PriorRunFacts, ProviderSessionId, SessionContinuation, SessionContinuationCheckpoint,
    SessionContinuationIncompatibility, SessionContinuationIncompatible, SessionContinuationLookup,
    SessionContinuationQuery, SessionContinuationSequence, SessionContinuationUnavailable,
    SessionContinuationUnavailableReason,
};
