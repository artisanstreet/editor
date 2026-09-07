//! `SQLite` persistence boundary owned by Forge.
//!
//! Forge is the only production process that opens this database. The crate
//! owns the connection policy and, as the native schema lands, its entities,
//! repositories, and transaction boundaries.

mod connection;
mod engine_run_config;
pub mod entities;
mod repository;

pub use artisan_domain::WorkspaceId;
pub use connection::{ConnectError, SqliteConfig, connect};
pub use repository::{
    AssistantChange, AttachProjectInput, AttachProjectResult, AuxiliaryTerminalError,
    BindRunProvider, BindRunProviderOutcome, BoundRunReceipt, CancelRun, CancelRunError,
    CancelRunOutcome, CheckpointUpdate, ClaimMessageDispatch, ClaimedMessageDispatch,
    CommitRunBatch, CommitRunBatchOutcome, CompleteMessageDispatch, CompleteRun, CompleteRunError,
    CompleteRunOutcome, ConversationPatchReplay, CreateThreadInput, CreateThreadResult,
    DispatchFailureReason, DispatchFailureReasonError, DispatchLeaseOwner, DispatchLeaseOwnerError,
    EngineCheckpoint, FailMessageDispatch, FailRun, FailRunError, FailRunOutcome, InterruptRun,
    InterruptRunError, InterruptRunOutcome, InterruptedRunReceipt, LaunchClaimedRun,
    LaunchClaimedRunOutcome, LaunchedRunReceipt, MessageDispatchPayload, ProviderBindingBytes,
    QueueFirstMessageInput, QueueFirstMessageResult, Repository, RepositoryError,
    RequeueMessageDispatch, RunBatchReceiptInfo, RunBatchScope, RunBindingError, RunErrorCode,
    RunErrorMessage, RunLaunchCredentials, RunLaunchError, RunObservationError, RunStartKey,
    SetThreadEngineConfigInput, SetThreadEngineConfigResult, StartupReconciliationCandidate,
    StartupReconciliationCandidates, StartupReconciliationDisposition,
    StartupReconciliationDispositionError, StartupReconciliationDispositionOutcome,
    StartupReconciliationDispositionReceipt, StartupReconciliationError,
    StartupReconciliationQuery, StartupRunLifecycle, TerminalRunReceipt, ThreadEngineSettings,
    TransitionedMessageDispatch,
};
